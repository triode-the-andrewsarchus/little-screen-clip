//! Camera RAW detection, embedded previews, and explicit sensor development.

use crate::process::CommandRequest;
use flashraw::{
    CorruptionPolicy, DecodeOptions, FlashRaw, Limits, OutputFormat, PreviewEncoding, PreviewInfo,
};
use image::{DynamicImage, ImageDecoder, ImageFormat, RgbImage, RgbaImage, metadata::Orientation};
use std::{
    fs::File,
    io::Cursor,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// Encoded input limit for developing a RAW file in Picture Studio.
pub const DEVELOP_MAX_BYTES: usize = 128 * 1024 * 1024;
const PREVIEW_SOURCE_MAX_BYTES: usize = 64 * 1024 * 1024;
const EMBEDDED_MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;
const MAX_DEVELOP_SAMPLES: u64 = 256 * 1024 * 1024;

/// Returns whether the path has a known camera RAW extension.
pub fn is_raw_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "raw"
            | "cr2"
            | "cr3"
            | "crw"
            | "nef"
            | "nrw"
            | "arw"
            | "srf"
            | "sr2"
            | "dng"
            | "raf"
            | "orf"
            | "rw2"
            | "pef"
            | "srw"
            | "iiq"
            | "mos"
            | "3fr"
            | "fff"
            | "mrw"
            | "x3f"
            | "kdc"
            | "dcr"
            | "dcs"
            | "erf"
            | "mef"
            | "mdc"
            | "ari"
    )
}

/// Recognizes camera RAW signatures that distinguish RAW from ordinary raster images.
pub fn is_raw_image_bytes(bytes: &[u8]) -> bool {
    let Some(format) = flashraw::supports(bytes) else {
        return false;
    };
    // FlashRAW can identify TIFF-based formats from the camera make alone.
    // Rendered TIFFs carry the same make, so those formats need an extension hint.
    !matches!(image::guess_format(bytes), Ok(ImageFormat::Tiff))
        || matches!(
            format,
            flashraw::RawFormat::Dng | flashraw::RawFormat::Cr2 | flashraw::RawFormat::Mos
        )
}

/// An oriented, developed sRGB frame and its source camera details.
pub struct DevelopedImage {
    /// Cropped, EXIF-oriented pixels in sRGB.
    pub pixels: RgbaImage,
    /// Camera and capture details parsed from the same input.
    pub metadata: flashraw::RawMetadata,
}

/// Develops sensor pixels from a bounded snapshot of a camera RAW file.
///
/// # Errors
/// Returns an error for unsupported, oversized, or corrupt input.
pub fn develop(bytes: &[u8]) -> Result<DevelopedImage, String> {
    if bytes.len() > DEVELOP_MAX_BYTES {
        return Err("RAW file exceeds the 128 MiB opening limit".into());
    }
    let limits = raw_limits();
    let raw = FlashRaw::open_with(bytes, limits)
        .map_err(|error| format!("Failed to parse RAW image: {error}"))?;
    let sensor = raw.frame_info();
    limits
        .check_dims(sensor.width, sensor.height, 4)
        .map_err(|error| format!("RAW image exceeds the development limit: {error}"))?;
    let frame = raw
        .decode(DecodeOptions {
            output: OutputFormat::Rgba8,
            limits,
            crop: true,
            preview_fallback: false,
            corruption_policy: CorruptionPolicy::Strict,
            threads: Some(2),
            ..Default::default()
        })
        .map_err(|error| format!("Failed to develop RAW image: {error}"))?;
    let flashraw::RawPixelData::U8(pixels) = frame.data else {
        return Err("RAW decoder did not return an 8-bit RGBA frame".into());
    };
    let pixels = RgbaImage::from_raw(frame.width, frame.height, pixels)
        .ok_or("RAW decoder returned an invalid RGBA buffer")?;
    Ok(DevelopedImage {
        pixels,
        metadata: frame.metadata,
    })
}

enum EmbeddedPixels {
    Encoded(Vec<u8>, &'static str),
    Rgb(RgbImage),
}

struct EmbeddedPreview {
    pub pixels: EmbeddedPixels,
    pub orientation: u8,
}

fn embedded_preview<T>(
    path: &Path,
    edge: u32,
    cancelled: &AtomicBool,
    accept: impl FnMut(EmbeddedPreview) -> Option<T>,
) -> Option<T> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let bytes = crate::file_source::read_file_snapshot(path, PREVIEW_SOURCE_MAX_BYTES).ok()?;
    select_embedded_preview(&bytes, edge, cancelled, accept)
}

fn select_embedded_preview<T>(
    bytes: &[u8],
    edge: u32,
    cancelled: &AtomicBool,
    mut accept: impl FnMut(EmbeddedPreview) -> Option<T>,
) -> Option<T> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let raw = FlashRaw::open_with(bytes, raw_limits()).ok()?;
    let mut candidates: Vec<_> = raw
        .previews()
        .iter()
        .filter_map(|preview| preview_candidate(&raw, preview))
        .collect();
    candidates.sort_by_key(|(_, _, size)| {
        if *size >= edge {
            (false, *size)
        } else {
            (true, u32::MAX - *size)
        }
    });
    for (preview, data, _) in candidates {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        if let Some(result) = materialize_preview(preview, data).and_then(&mut accept) {
            return (!cancelled.load(Ordering::Relaxed)).then_some(result);
        }
    }
    None
}

fn materialize_preview(preview: &PreviewInfo, data: &[u8]) -> Option<EmbeddedPreview> {
    let pixels = match preview.encoding {
        PreviewEncoding::Jpeg => EmbeddedPixels::Encoded(data.to_vec(), "image/jpeg"),
        PreviewEncoding::JpegXl => EmbeddedPixels::Encoded(data.to_vec(), "image/jxl"),
        PreviewEncoding::Rgb8 => EmbeddedPixels::Rgb(RgbImage::from_raw(
            preview.width,
            preview.height,
            data.to_vec(),
        )?),
        _ => return None,
    };
    Some(EmbeddedPreview {
        pixels,
        orientation: preview.orientation.exif() as u8,
    })
}

fn raw_limits() -> Limits {
    Limits::default()
        .with_max_dimension(MAX_DIMENSION)
        .with_max_samples(MAX_DEVELOP_SAMPLES)
}

fn preview_candidate<'a>(
    raw: &'a FlashRaw<'_>,
    preview: &'a PreviewInfo,
) -> Option<(&'a PreviewInfo, &'a [u8], u32)> {
    if preview.length > EMBEDDED_MAX_BYTES as u64 {
        return None;
    }
    let bytes = raw.preview_bytes(preview).ok()?;
    let (width, height) = match preview.encoding {
        PreviewEncoding::Rgb8 => {
            let samples = u64::from(preview.width)
                .checked_mul(u64::from(preview.height))?
                .checked_mul(3)?;
            if samples != bytes.len() as u64 || samples == 0 {
                return None;
            }
            (preview.width, preview.height)
        }
        PreviewEncoding::Jpeg | PreviewEncoding::JpegXl => preview_dimensions(bytes)?,
        _ => return None,
    };
    let edge = width.max(height);
    (edge <= MAX_DIMENSION).then_some((preview, bytes, edge))
}

/// Decodes an oriented embedded RAW preview using a retained ordinary-file handle.
///
/// Seeks to the start for the bounded native read. ExifTool can supply an
/// embedded preview when native extraction is unavailable.
pub fn decode_embedded_preview(
    file: &mut File,
    edge: u32,
    fallback_orientation: u8,
    cancelled: &AtomicBool,
) -> Option<DynamicImage> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let native = crate::file_source::read_held_snapshot(file, PREVIEW_SOURCE_MAX_BYTES)
        .ok()
        .and_then(|(bytes, _)| select_embedded_preview(&bytes, edge, cancelled, oriented_preview));
    native.or_else(|| exiftool_raster(file, edge, fallback_orientation, true, cancelled))
}

fn oriented_preview(preview: EmbeddedPreview) -> Option<DynamicImage> {
    match preview.pixels {
        EmbeddedPixels::Encoded(bytes, mime) => {
            decode_preview(&bytes, Some(mime), Some(preview.orientation))
        }
        EmbeddedPixels::Rgb(pixels) => {
            let mut image = DynamicImage::ImageRgb8(pixels);
            if let Some(orientation) = Orientation::from_exif(preview.orientation) {
                image.apply_orientation(orientation);
            }
            Some(image)
        }
    }
}

pub(crate) fn exiftool_raster(
    file: &File,
    edge: u32,
    orientation: u8,
    allow_smaller: bool,
    cancelled: &AtomicBool,
) -> Option<DynamicImage> {
    let mut largest = None;
    for tag in ["-ThumbnailImage", "-PreviewImage", "-JpgFromRaw"] {
        let Some(bytes) = exiftool_preview(file, tag, cancelled) else {
            continue;
        };
        let Some((width, height)) = preview_dimensions(&bytes) else {
            continue;
        };
        let size = width.max(height);
        if size < edge
            && (!allow_smaller || largest.as_ref().is_some_and(|(best, _)| size <= *best))
        {
            continue;
        }
        let Some(image) = decode_preview(&bytes, Some("image/jpeg"), Some(orientation)) else {
            continue;
        };
        if size >= edge {
            return Some(image);
        }
        largest = Some((size, image));
    }
    largest.map(|(_, image)| image)
}

/// Extracts an image preview and its dimensions for clipboard image items.
///
/// # Errors
/// Returns an error when no usable preview can be produced.
pub fn extract_raw_image_preview(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let cancelled = AtomicBool::new(false);
    if let Some(encoded) = embedded_preview(path, u32::MAX, &cancelled, clipboard_jpeg) {
        return Ok(encoded);
    }

    let file =
        crate::file_source::open_regular_file(path, true).map_err(|error| error.to_string())?;
    for tag in ["-PreviewImage", "-JpgFromRaw", "-ThumbnailImage"] {
        if let Some(bytes) = exiftool_preview(&file, tag, &cancelled)
            && let Some(image) = decode_preview(&bytes, None, None)
        {
            return Ok((bytes, image.width(), image.height()));
        }
    }

    // The clipboard's existing converter fallback is independent of the gallery path.
    let output = CommandRequest::new("magick")
        .arg_content_file_with_suffix(path, "[0]")
        .map_err(|error| error.to_string())?
        .args(["-resize", "1600x1200>", "jpeg:-"])
        .timeout(Duration::from_secs(5))
        .max_output_bytes(EMBEDDED_MAX_BYTES)
        .run();
    if let Ok(output) = output
        && output.status.success()
        && !output.stdout_truncated
        && let Some(image) = decode_preview(&output.stdout, Some("image/jpeg"), None)
    {
        return Ok((output.stdout, image.width(), image.height()));
    }
    Err(format!(
        "Could not extract camera RAW preview from {path:?}"
    ))
}

fn clipboard_jpeg(preview: EmbeddedPreview) -> Option<(Vec<u8>, u32, u32)> {
    let mut image = match preview.pixels {
        EmbeddedPixels::Encoded(bytes, mime) => {
            let image = decode_preview(&bytes, Some(mime), None)?;
            if mime == "image/jpeg" && preview.orientation == 1 {
                return Some((bytes, image.width(), image.height()));
            }
            image
        }
        EmbeddedPixels::Rgb(image) => DynamicImage::ImageRgb8(image),
    };
    if let Some(orientation) = Orientation::from_exif(preview.orientation) {
        image.apply_orientation(orientation);
    }
    let image = image.into_rgb8();
    let mut jpeg = Cursor::new(Vec::new());
    image.write_to(&mut jpeg, ImageFormat::Jpeg).ok()?;
    Some((jpeg.into_inner(), image.width(), image.height()))
}

fn decode_preview(
    bytes: &[u8],
    mime: Option<&str>,
    fallback_orientation: Option<u8>,
) -> Option<DynamicImage> {
    let mut reader = crate::item::image_reader(bytes, mime).ok()?;
    let mut limits = crate::item::raster_preview_limits();
    reader.limits(limits.clone());
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = fallback_orientation.and_then(|fallback| {
        decoder
            .orientation()
            .ok()
            .filter(|orientation| *orientation != Orientation::NoTransforms)
            .or_else(|| Orientation::from_exif(fallback))
    });
    limits.reserve(decoder.total_bytes()).ok()?;
    decoder.set_limits(limits).ok()?;
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Some(image)
}

fn exiftool_preview(file: &File, tag: &str, cancelled: &AtomicBool) -> Option<Vec<u8>> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let output = CommandRequest::new("exiftool")
        .die_with_parent()
        .args(["-b", tag])
        .arg_owned_content_file(file.try_clone().ok()?)
        .ok()?
        .timeout(Duration::from_secs(5))
        .max_output_bytes(EMBEDDED_MAX_BYTES)
        .run_cancellable(cancelled)
        .ok()?;
    (output.status.success() && !output.stdout_truncated && !output.stdout.is_empty())
        .then_some(output.stdout)
}

fn preview_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let size = imagesize::blob_size(bytes).ok()?;
    let width = u32::try_from(size.width).ok()?;
    let height = u32::try_from(size.height).ok()?;
    (width > 0 && height > 0 && width.max(height) <= MAX_DIMENSION).then_some((width, height))
}

#[cfg(test)]
mod tests;
