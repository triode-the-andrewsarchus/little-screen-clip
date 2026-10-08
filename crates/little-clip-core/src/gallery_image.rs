//! Select an embedded HEIF preview before decoding the primary image.
use crate::ImageThumbnail;
use std::sync::atomic::{AtomicBool, Ordering};

/// Reads the primary HEIF image's displayed dimensions, including native crop/rotation.
/// Inspects container metadata through a seekable stream without decoding pixels.
pub fn heif_dimensions(file: std::fs::File) -> Option<(u32, u32)> {
    let context = read_context(file)?;
    let handle = context.primary_image_handle().ok()?;
    Some((handle.width(), handle.height()))
}

/// Decodes the smallest suitable HEIF thumbnail (or its primary image).
/// Returns `None` for unsupported containers, cancelled work, or oversized rasters.
pub fn heif_thumbnail(
    file: std::fs::File,
    edge: u32,
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    use libheif_rs::{ColorSpace, LibHeif, RgbChroma};
    let context = read_context(file)?;
    let primary = context.primary_image_handle().ok()?;
    let mut ids = vec![0; primary.number_of_thumbnails()];
    primary.thumbnail_ids(&mut ids);
    let mut choices: Vec<_> = ids
        .into_iter()
        .filter_map(|id| primary.thumbnail(id).ok())
        .collect();
    choices.push(primary);
    choices.sort_by_key(|handle| u64::from(handle.width()) * u64::from(handle.height()));
    let handle = choices
        .iter()
        .find(|handle| handle.width().max(handle.height()) >= edge)
        .or_else(|| choices.last())?;
    if cancelled.load(Ordering::Relaxed)
        || u64::from(handle.width()) * u64::from(handle.height()) > 32 * 1024 * 1024
    {
        return None;
    }
    let decoded = LibHeif::new()
        .decode(handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .ok()?;
    let planes = decoded.planes();
    let plane = planes.interleaved?;
    let mut pixels = Vec::with_capacity(plane.width as usize * plane.height as usize * 4);
    for row in plane.data.chunks(plane.stride).take(plane.height as usize) {
        pixels.extend_from_slice(&row[..plane.width as usize * 4]);
    }
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let image = image::RgbaImage::from_raw(plane.width, plane.height, pixels)?;
    let target = (edge.min(image.width()), edge.min(image.height()));
    let small = image::DynamicImage::ImageRgba8(image)
        .thumbnail(target.0, target.1)
        .to_rgba8();
    Some(ImageThumbnail::from_rgba(small))
}

fn read_context(file: std::fs::File) -> Option<libheif_rs::HeifContext<'static>> {
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let reader = libheif_rs::StreamReader::new(std::io::BufReader::new(file), metadata.len());
    libheif_rs::HeifContext::read_from_reader(Box::new(reader)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn heif_and_avif_previews_obey_requested_resolution_and_cancellation() {
        for extension in ["heic", "avif"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(format!("gradient.{extension}"));
            let dimensions = heif_dimensions(std::fs::File::open(&path).unwrap()).unwrap();
            assert!(dimensions.0 > 0 && dimensions.1 > 0);
            let thumbnail = heif_thumbnail(
                std::fs::File::open(&path).unwrap(),
                32,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(thumbnail.width <= 32 && thumbnail.height <= 32);
            assert_eq!(
                thumbnail.premultiplied_rgba.len(),
                (thumbnail.width * thumbnail.height * 4) as usize
            );
            assert!(
                heif_thumbnail(
                    std::fs::File::open(&path).unwrap(),
                    32,
                    &AtomicBool::new(true)
                )
                .is_none()
            );
        }
    }
}
