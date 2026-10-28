//! Resolution-aware thumbnails; full RAW development is never a gallery operation.
use image::ImageDecoder;
use little_clip_core::ImageThumbnail;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub(in crate::file_manager) fn load(
    path: &Path,
    edge: u32,
    orientation: u8,
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    let source = crate::file_source::FileSource::capture(path).ok()?;
    let cache = super::thumbnail_cache::path(&source, edge);
    if let Some(image) = cache
        .as_ref()
        .and_then(|path| super::thumbnail_cache::load(path))
    {
        source.check().ok()?;
        return Some(image);
    }
    let lock = source_lock(path)?;
    let _guard = lock.lock().ok()?;
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    // Another resolution request may have produced this representation while
    // we waited. Retired decodes cannot overlap a replacement for the same file.
    if let Some(image) = cache
        .as_ref()
        .and_then(|path| super::thumbnail_cache::load(path))
    {
        source.check().ok()?;
        return Some(image);
    }
    let result = decode_source(path, edge, orientation, cancelled);
    source.check().ok()?;
    if crate::file_source::FileSource::capture(path).ok()? != source {
        return None;
    }
    if let (Some(path), Some(image)) = (cache, &result) {
        super::thumbnail_cache::save(&path, image);
    }
    result
}

fn decode_source(
    path: &Path,
    edge: u32,
    orientation: u8,
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    let mime = little_clip_core::item::guess_image_mime(path);
    if matches!(mime.as_str(), "image/heic" | "image/heif" | "image/avif") {
        return little_clip_core::gallery_image::heif_thumbnail(
            crate::file_source::open_regular_file(path, true).ok()?,
            edge,
            cancelled,
        );
    }
    if little_clip_core::raw_image::is_raw_image_path(path) {
        return raw_preview(path, edge, orientation, cancelled);
    }
    let embedded = match mime.as_str() {
        "image/jpeg" | "image/jpg" => jpeg_preview(path, edge, orientation, cancelled),
        "image/tiff" => raw_preview(path, edge, orientation, cancelled),
        _ => None,
    };
    if embedded.is_some() {
        return embedded;
    }
    let bytes = crate::file_source::read_file_bytes(path, 64 * 1024 * 1024, true, true).ok()?;
    raster(&bytes, &mime, edge, 1, cancelled)
}

fn jpeg_preview(
    path: &Path,
    edge: u32,
    orientation: u8,
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    let file = crate::file_source::open_regular_file(path, true).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    let field = |tag| {
        exif.get_field(tag, exif::In::THUMBNAIL)?
            .value
            .get_uint(0)
            .map(|value| value as usize)
    };
    let offset = field(exif::Tag::JPEGInterchangeFormat)?;
    let length = field(exif::Tag::JPEGInterchangeFormatLength)?;
    let bytes = exif.buf().get(offset..offset.checked_add(length)?)?;
    let size = imagesize::blob_size(bytes).ok()?;
    if size.width.max(size.height) < edge as usize {
        return None;
    }
    let orientation = field(exif::Tag::Orientation).map_or(orientation, |value| value as u8);
    raster(bytes, "image/jpeg", edge, orientation, cancelled)
}

fn raw_preview(
    path: &Path,
    edge: u32,
    orientation: u8,
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    if let Ok((bytes, preview_orientation)) =
        little_clip_core::raw_image::extract_raw_preview_bytes(path, edge)
    {
        let orient = if preview_orientation > 1 {
            preview_orientation
        } else {
            orientation
        };
        return raster(&bytes, "image/jpeg", edge, orient, cancelled);
    }
    let mut largest = None;
    for tag in ["-ThumbnailImage", "-PreviewImage", "-JpgFromRaw"] {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let output = little_clip_core::process::CommandRequest::new("exiftool")
            .arg("-b")
            .arg(tag)
            .arg_content_file(path)
            .ok()?
            .timeout(Duration::from_secs(5))
            .max_output_bytes(16 * 1024 * 1024)
            .run()
            .ok()?;
        if let Ok(size) = imagesize::blob_size(&output.stdout) {
            if size.width.max(size.height) >= edge as usize {
                return raster(&output.stdout, "image/jpeg", edge, orientation, cancelled);
            }
            largest = Some(output.stdout);
        }
    }
    largest
        .filter(|_| little_clip_core::raw_image::is_raw_image_path(path))
        .and_then(|bytes| raster(&bytes, "image/jpeg", edge, orientation, cancelled))
}

fn raster(
    bytes: &[u8],
    mime: &str,
    edge: u32,
    fallback_orientation: u8,
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    let limits = super::super::media::raster_preview_limits();
    let mut reader = little_clip_core::item::image_reader(bytes, Some(mime)).ok()?;
    reader.limits(limits.clone());
    let mut decoder = match reader.into_decoder() {
        Ok(decoder) => decoder,
        Err(_) => {
            return ImageThumbnail::generate_with_limits(bytes, Some(mime), edge, edge, limits);
        }
    };
    let orientation = decoder
        .orientation()
        .ok()
        .filter(|orientation| *orientation != image::metadata::Orientation::NoTransforms)
        .or_else(|| image::metadata::Orientation::from_exif(fallback_orientation))
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let mut decoded = image::DynamicImage::from_decoder(decoder).ok()?;
    decoded.apply_orientation(orientation);
    let small = decoded
        .thumbnail(edge.min(decoded.width()), edge.min(decoded.height()))
        .to_rgba8();
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    Some(ImageThumbnail::from_rgba(small))
}

fn source_lock(path: &Path) -> Option<std::sync::Arc<std::sync::Mutex<()>>> {
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::{Arc, LazyLock, Mutex, Weak},
    };
    static ACTIVE: LazyLock<Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> =
        LazyLock::new(|| Mutex::new(BTreeMap::new()));
    let path = path.canonicalize().ok()?;
    let mut active = ACTIVE.lock().ok()?;
    active.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = active.get(&path).and_then(Weak::upgrade) {
        return Some(lock);
    }
    let lock = Arc::new(Mutex::new(()));
    active.insert(path, Arc::downgrade(&lock));
    Some(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_replacements_and_aliases_share_one_decode_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.png");
        let alias = directory.path().join("alias.png");
        std::fs::write(&path, b"header").unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let first = source_lock(&path).unwrap();
        let replacement = source_lock(&alias).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &replacement));
        let guard = first.lock().unwrap();
        assert!(replacement.try_lock().is_err());
        drop(guard);
        assert!(replacement.try_lock().is_ok());
    }
    #[test]
    fn suitable_embedded_jpeg_is_used_without_decoding_the_primary_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("embedded.jpg");
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            160,
            80,
            image::Rgb([255, 0, 0]),
        ))
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
        let jpeg = jpeg.into_inner();
        let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0, 14, 0, 0, 0, 2, 0];
        for (tag, value) in [(0x0201_u16, 44_u32), (0x0202_u16, jpeg.len() as u32)] {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&4_u16.to_le_bytes());
            tiff.extend_from_slice(&1_u32.to_le_bytes());
            tiff.extend_from_slice(&value.to_le_bytes());
        }
        tiff.extend_from_slice(&0_u32.to_le_bytes());
        tiff.extend_from_slice(&jpeg);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(&payload);
        // No primary SOF/entropy data exists: only the embedded preview is valid.
        std::fs::write(&path, bytes).unwrap();
        let preview = load(&path, 128, 1, &AtomicBool::new(false)).unwrap();
        assert_eq!((preview.width, preview.height), (128, 64));
        assert!(preview.premultiplied_rgba[0] > 200 && preview.premultiplied_rgba[2] < 50);
    }
}
