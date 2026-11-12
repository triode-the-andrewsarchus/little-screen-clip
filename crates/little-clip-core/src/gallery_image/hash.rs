//! Shared gallery preprocessing and versioned appearance hashes.
use crate::file_source::FileSource;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Recovers straight RGBA pixels from the existing premultiplied thumbnail.
pub fn straight_pixels(thumbnail: &crate::ImageThumbnail) -> Option<image::RgbaImage> {
    let mut pixels = thumbnail.premultiplied_rgba.clone();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(thumbnail.width, thumbnail.height, pixels)
}

/// Parses only the dHash field; version and mean-color bits are not distances.
pub fn parse_dhash(encoded: &str) -> Option<u64> {
    (encoded.len() == 20
        && encoded.starts_with('1')
        && encoded.bytes().all(|b| b.is_ascii_hexdigit()))
    .then(|| u64::from_str_radix(&encoded[4..], 16).ok())
    .flatten()
}

/// Uses shared gallery preprocessing without full-file hashing or analysis.
///
/// Run on a worker. Header/thumbnail caches and source guards are the same as
/// photo rows; raw formats use embedded previews rather than development.
///
/// # Errors
/// Returns guarded-source, revision, thumbnail or hash-representation failures.
pub fn perceptual(path: &Path, revision: u64, cancelled: &AtomicBool) -> Result<u64, String> {
    let source = FileSource::capture(path)?;
    let (_, thumbnail) = thumbnail(path, revision, cancelled)?;
    let image = straight_pixels(&thumbnail).ok_or("Invalid image thumbnail")?;
    let hash = parse_dhash(&difference_hash(&image)).ok_or("Invalid perceptual hash")?;
    source.check()?;
    crate::file_inventory::check_revision(path, revision)?;
    Ok(hash)
}

/// Loads the existing 256-pixel hash input with observed source orientation.
///
/// # Errors
/// Returns metadata, cancellation or unavailable-preview failures.
pub fn thumbnail(
    path: &Path,
    revision: u64,
    cancelled: &AtomicBool,
) -> Result<(super::metadata::Metadata, crate::ImageThumbnail), String> {
    let dimensions = super::metadata::probe(path, revision)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let thumbnail = super::thumbnail::load(path, 256, dimensions.orientation, cancelled)
        .ok_or("Image preview unavailable")?;
    Ok((dimensions, thumbnail))
}

/// Encodes the existing versioned mean-color and luminance-difference hash.
///
/// Four format bits and twelve coarse color bits precede 64 dHash bits. This
/// describes appearance, not source identity; use [`parse_dhash`] for distance.
pub fn difference_hash(image: &image::RgbaImage) -> String {
    let gray = image::DynamicImage::ImageRgba8(image.clone()).into_luma8();
    let small = image::imageops::resize(&gray, 9, 8, image::imageops::FilterType::Triangle);
    let mut hash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            hash = (hash << 1) | u64::from(small[(x, y)][0] > small[(x + 1, y)][0]);
        }
    }
    let mut sums = [0_u64; 3];
    let mut weight = 0_u64;
    for pixel in image.pixels() {
        let alpha = u64::from(pixel[3]);
        weight += alpha;
        for channel in 0..3 {
            sums[channel] += u64::from(pixel[channel]) * alpha;
        }
    }
    let color = sums.map(|sum| (sum / weight.max(1)) as u8 >> 4);
    format!("1{:x}{:x}{:x}{hash:016x}", color[0], color[1], color[2])
}
