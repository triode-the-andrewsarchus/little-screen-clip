//! Camera RAW format detection, metadata inspection, and decoding.
//!
//! Powered by [`flashraw`] for fast, in-process, memory-safe decoding and embedded
//! preview extraction, with fallback to `exiftool` / ImageMagick for edge-case files.

use crate::process::CommandRequest;
use flashraw::{DecodeOptions, FlashRaw, OutputFormat};
use image::RgbaImage;
use std::path::Path;

/// Returns `true` if the path has a known camera RAW file extension.
pub fn is_raw_image_path(p: &Path) -> bool {
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
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
    } else {
        false
    }
}

/// Returns `true` if the byte slice starts with known camera RAW magic bytes.
pub fn is_raw_image_bytes(bytes: &[u8]) -> bool {
    flashraw::supports(bytes).is_some()
}

/// Summary of basic camera RAW metadata (dimensions and orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMetadataSummary {
    pub width: u32,
    pub height: u32,
    pub orientation: u8,
}

/// Detailed EXIF and camera exposure metadata from a camera RAW image.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawDetails {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub date_time: Option<String>,
    pub iso: Option<u32>,
    pub f_number: Option<f32>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<f32>,
    pub software: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude: Option<f64>,
}

fn raw_details_from_meta(meta: &flashraw::RawMetadata) -> RawDetails {
    let make = if !meta.make.trim().is_empty() {
        Some(meta.make.trim().to_string())
    } else {
        None
    };
    let model = if !meta.model.trim().is_empty() {
        Some(meta.model.trim().to_string())
    } else {
        None
    };
    let lens_model = meta
        .lens
        .model
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    let date_time = meta
        .capture_time
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned();
    let software = meta
        .software
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned();

    let iso = meta.exposure.iso;
    let f_number = meta.exposure.f_number;
    let focal_length = meta.exposure.focal_length;
    let exposure_time = meta.exposure.exposure_time.map(|sec| {
        if sec <= 0.0 {
            format!("{sec} s")
        } else if sec < 1.0 {
            let denom = (1.0 / sec).round();
            format!("1/{denom:.0} s")
        } else {
            format!("{sec:.2} s")
        }
    });

    let (gps_latitude, gps_longitude, gps_altitude) = if let Some(gps) = &meta.gps {
        (gps.latitude, gps.longitude, gps.altitude)
    } else {
        (None, None, None)
    };

    RawDetails {
        make,
        model,
        lens_model,
        date_time,
        iso,
        f_number,
        exposure_time,
        focal_length,
        software,
        gps_latitude,
        gps_longitude,
        gps_altitude,
    }
}

/// Reads detailed EXIF/camera metadata from a camera RAW file on disk.
pub fn read_raw_details_from_path(path: &Path) -> Option<RawDetails> {
    let raw = FlashRaw::open_path(path).ok()?;
    Some(raw_details_from_meta(raw.metadata()))
}

/// Reads detailed EXIF/camera metadata from camera RAW bytes in memory.
pub fn read_raw_details_from_bytes(bytes: &[u8]) -> Option<RawDetails> {
    let raw = FlashRaw::open(bytes).ok()?;
    Some(raw_details_from_meta(raw.metadata()))
}

/// Reads dimensions and EXIF orientation from a camera RAW image file.
pub fn read_raw_metadata(path: &Path) -> Result<RawMetadataSummary, String> {
    if let Ok(raw) = FlashRaw::open_path(path) {
        let meta = raw.metadata();
        let orientation = meta.orientation.exif() as u8;
        let orientation = if (1..=8).contains(&orientation) {
            orientation
        } else {
            1
        };

        let (width, height) = if meta.width > 0 && meta.height > 0 {
            (meta.width, meta.height)
        } else if let Some((w, h)) = meta.image_size() {
            (w, h)
        } else if let Some(p) = raw.metadata().largest_preview() {
            (p.width, p.height)
        } else {
            let fi = raw.frame_info();
            (fi.width, fi.height)
        };

        if width > 0 && height > 0 {
            return Ok(RawMetadataSummary {
                width,
                height,
                orientation,
            });
        }
    }

    // Fallback to exiftool subprocess
    let output = CommandRequest::new("exiftool")
        .args([
            "-j",
            "-n",
            "-ImageWidth",
            "-ImageHeight",
            "-Orientation",
            "-PreviewImageWidth",
            "-PreviewImageHeight",
        ])
        .arg_content_file(path)
        .map_err(|error| error.to_string())?
        .timeout(std::time::Duration::from_secs(5))
        .max_output_bytes(1024 * 1024)
        .run()
        .map_err(|error| error.to_string())?;

    if !output.status.success() {
        return Err("Exiftool failed to read metadata".into());
    }

    let records: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let record = records
        .first()
        .ok_or_else(|| "Empty exiftool output".to_string())?;

    let width = record
        .get("ImageWidth")
        .or_else(|| record.get("PreviewImageWidth"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let height = record
        .get("ImageHeight")
        .or_else(|| record.get("PreviewImageHeight"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let orientation = record
        .get("Orientation")
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as u8;

    if width > 0 && height > 0 {
        Ok(RawMetadataSummary {
            width,
            height,
            orientation,
        })
    } else {
        Err("Could not determine RAW dimensions".into())
    }
}

/// Extracts embedded preview JPEG bytes and preview-specific orientation from a camera RAW image file.
pub fn extract_raw_preview_bytes(path: &Path, target_edge: u32) -> Result<(Vec<u8>, u8), String> {
    if let Ok(raw) = FlashRaw::open_path(path) {
        let meta = raw.metadata();
        let fallback_orient = meta.orientation.exif() as u8;

        let mut best: Option<(Vec<u8>, u8)> = None;
        let mut largest: Option<(Vec<u8>, u8, usize)> = None;

        for p in raw.previews() {
            if let Ok(bytes) = raw.preview_bytes(p) {
                let orient = {
                    let o = p.orientation.exif() as u8;
                    if (1..=8).contains(&o) {
                        o
                    } else if (1..=8).contains(&fallback_orient) {
                        fallback_orient
                    } else {
                        1
                    }
                };
                let max_dim = p.width.max(p.height) as usize;
                if max_dim >= target_edge as usize {
                    best = Some((bytes.to_vec(), orient));
                    break;
                }
                if largest.as_ref().is_none_or(|(_, _, d)| max_dim > *d) {
                    largest = Some((bytes.to_vec(), orient, max_dim));
                }
            }
        }

        if let Some((b, o)) = best.or_else(|| largest.map(|(b, o, _)| (b, o))) {
            return Ok((b, o));
        }
    }

    // Fallback: extract full preview
    let (bytes, _, _) = extract_raw_image_preview(path)?;
    Ok((bytes, 1))
}

/// Extracts embedded preview bytes and dimensions from a camera RAW image file.
pub fn extract_raw_image_preview(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    // 1. Try FlashRaw embedded preview extraction (in-process, instant)
    if let Ok(raw) = FlashRaw::open_path(path)
        && let Some(p) = raw.metadata().largest_preview().or_else(|| raw.preview())
        && let Ok(bytes) = raw.preview_bytes(p)
        && let Ok(img) = image::load_from_memory(bytes)
    {
        return Ok((bytes.to_vec(), img.width(), img.height()));
    }

    // 2. Try exiftool embedded preview extraction
    let exiftool_tags = ["-PreviewImage", "-JpgFromRaw", "-ThumbnailImage"];
    for tag in exiftool_tags {
        let out = CommandRequest::new("exiftool")
            .arg("-b")
            .arg(tag)
            .arg_content_file(path)
            .map_err(|e| e.to_string())?
            .max_output_bytes(16 * 1024 * 1024)
            .run();

        if let Ok(output) = out
            && output.status.success()
            && output.stdout.len() > 1024
            && let Ok(img) = image::load_from_memory(&output.stdout)
        {
            return Ok((output.stdout, img.width(), img.height()));
        }
    }

    // 3. Fallback to ImageMagick
    let out = CommandRequest::new("magick")
        .arg_content_file_with_suffix(path, "[0]")
        .map_err(|e| e.to_string())?
        .arg("-resize")
        .arg("1600x1200>")
        .arg("jpeg:-")
        .max_output_bytes(16 * 1024 * 1024)
        .run();

    if let Ok(output) = out
        && output.status.success()
        && output.stdout.len() > 1024
        && let Ok(img) = image::load_from_memory(&output.stdout)
    {
        return Ok((output.stdout, img.width(), img.height()));
    }

    Err(format!(
        "Could not extract camera RAW preview from {:?}",
        path
    ))
}

/// Fully decodes a camera RAW file on disk into an 8-bit sRGB RGBA image buffer.
pub fn decode_raw_image(path: &Path) -> Result<RgbaImage, String> {
    let raw = FlashRaw::open_path(path)
        .map_err(|error| format!("Failed to open RAW file {:?}: {error}", path))?;
    decode_flashraw(&raw)
}

/// Fully decodes camera RAW bytes in memory into an 8-bit sRGB RGBA image buffer.
pub fn decode_raw_image_from_bytes(bytes: &[u8]) -> Result<RgbaImage, String> {
    let raw = FlashRaw::open(bytes)
        .map_err(|error| format!("Failed to parse RAW image bytes: {error}"))?;
    decode_flashraw(&raw)
}

fn decode_flashraw(raw: &FlashRaw<'_>) -> Result<RgbaImage, String> {
    let frame = raw
        .decode(DecodeOptions {
            output: OutputFormat::Rgba8,
            crop: true,
            preview_fallback: true,
            ..Default::default()
        })
        .map_err(|error| format!("FlashRaw decode error: {error}"))?;

    let width = frame.width();
    let height = frame.height();
    let slice = frame
        .as_u8()
        .ok_or_else(|| "Decoded frame missing Rgba8 byte buffer".to_string())?;

    RgbaImage::from_raw(width, height, slice.to_vec())
        .ok_or_else(|| "Failed to construct RgbaImage from decoded RAW pixels".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_raw_image_path() {
        assert!(is_raw_image_path(Path::new("photo.CR2")));
        assert!(is_raw_image_path(Path::new("photo.CR3")));
        assert!(is_raw_image_path(Path::new("/path/to/image.NEF")));
        assert!(is_raw_image_path(Path::new("sony.arw")));
        assert!(is_raw_image_path(Path::new("fuji.RAF")));
        assert!(is_raw_image_path(Path::new("adobe.dng")));
        assert!(is_raw_image_path(Path::new("sensor.raw")));
        assert!(is_raw_image_path(Path::new("olympus.orf")));
        assert!(is_raw_image_path(Path::new("panasonic.rw2")));
        assert!(is_raw_image_path(Path::new("nikon.nrw")));
        assert!(is_raw_image_path(Path::new("hasselblad.3fr")));
        assert!(!is_raw_image_path(Path::new("photo.png")));
        assert!(!is_raw_image_path(Path::new("document.pdf")));
    }

    #[test]
    fn test_is_raw_image_bytes() {
        assert!(!is_raw_image_bytes(b"hello world"));
        assert!(!is_raw_image_bytes(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn test_decode_raw_invalid_bytes() {
        assert!(decode_raw_image_from_bytes(b"not a valid raw file").is_err());
    }
}
