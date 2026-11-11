//! Native embedded artwork extraction with bounded raster decoding.

use crate::ImageThumbnail;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use symphonia::core::{
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::{Limit, MetadataOptions, MetadataRevision, StandardVisualKey},
    probe::Hint,
};

/// Reads a music cover through the native format reader on a background worker.
///
/// # Errors
/// Returns an error for unsupported inputs, guarded-open failures or cancellation.
/// A supported input without decodable artwork returns `Ok(None)`.
pub fn read_audio_cover(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<ImageThumbnail>, String> {
    let check = || {
        if cancelled.load(Ordering::Acquire) {
            Err("Audio cover cancelled".to_owned())
        } else {
            Ok(())
        }
    };
    check()?;
    let file = crate::kernel_interface::open_content(path, true).map_err(|e| e.to_string())?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(file), Default::default()),
            &FormatOptions::default(),
            &MetadataOptions {
                limit_metadata_bytes: Limit::Maximum(64 * 1024),
                limit_visual_bytes: Limit::Maximum(8 * 1024 * 1024),
            },
        )
        .map_err(|error| error.to_string())?;
    check()?;
    let cover = probed
        .metadata
        .get()
        .and_then(|mut metadata| metadata.skip_to_latest().and_then(thumbnail))
        .or_else(|| {
            probed
                .format
                .metadata()
                .skip_to_latest()
                .and_then(thumbnail)
        });
    check()?;
    Ok(cover)
}

fn thumbnail(metadata: &MetadataRevision) -> Option<ImageThumbnail> {
    // Prefer the front cover, but retain artwork from formats without a role.
    let mut visuals: Vec<_> = metadata.visuals().iter().collect();
    visuals.sort_by_key(|visual| visual.usage != Some(StandardVisualKey::FrontCover));
    visuals.into_iter().find_map(|visual| {
        if visual.data.len() > 8 * 1024 * 1024 {
            return None;
        }
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(64 * 1024 * 1024);
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        let mut reader = image::ImageReader::new(std::io::Cursor::new(&visual.data))
            .with_guessed_format()
            .ok()?;
        reader.limits(limits);
        let image = reader.decode().ok()?.thumbnail(160, 160);
        Some(ImageThumbnail::from_rgba(image.to_rgba8()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::meta::{MetadataBuilder, Visual};

    fn visual(width: u32, height: u32, usage: StandardVisualKey) -> Visual {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        Visual {
            media_type: "image/png".into(),
            dimensions: None,
            bits_per_pixel: None,
            color_mode: None,
            usage: Some(usage),
            tags: Vec::new(),
            data: png.into_inner().into_boxed_slice(),
        }
    }

    #[test]
    fn front_cover_is_preferred_and_image_dimensions_are_bounded() {
        let mut metadata = MetadataBuilder::new();
        metadata.add_visual(visual(2, 4, StandardVisualKey::BackCover));
        metadata.add_visual(visual(4, 2, StandardVisualKey::FrontCover));
        let cover = thumbnail(&metadata.metadata()).unwrap();
        assert!(cover.width > cover.height);
        assert!(cover.width <= 160 && cover.height <= 160);

        let mut oversized = MetadataBuilder::new();
        oversized.add_visual(visual(8193, 1, StandardVisualKey::FrontCover));
        assert!(thumbnail(&oversized.metadata()).is_none());
    }

    #[test]
    fn ordinary_audio_without_artwork_and_cancelled_requests_need_no_decoder_process() {
        let (_directory, path) = crate::audio::test_support::generate_fixture_wav("cover", 0.1);
        assert!(
            read_audio_cover(&path, &AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
        assert!(read_audio_cover(&path, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn native_probe_extracts_id3_front_cover() {
        use std::io::Cursor;
        let (_directory, path) = crate::audio::test_support::generate_fixture_wav("cover", 0.1);
        let wav = std::fs::read(&path).unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 2)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut picture = b"\0image/png\0\x03\0".to_vec();
        picture.extend(png.into_inner());
        let mut frame = b"APIC".to_vec();
        frame.extend((picture.len() as u32).to_be_bytes());
        frame.extend([0, 0]);
        frame.extend(picture);
        let size = frame.len() as u32;
        let mut tagged = b"ID3\x03\0\0".to_vec();
        tagged.extend([
            (size >> 21) as u8 & 127,
            (size >> 14) as u8 & 127,
            (size >> 7) as u8 & 127,
            size as u8 & 127,
        ]);
        tagged.extend(frame);
        tagged.extend(wav);
        std::fs::write(&path, tagged).unwrap();
        assert!(
            read_audio_cover(&path, &AtomicBool::new(false))
                .unwrap()
                .is_some()
        );
    }
}
