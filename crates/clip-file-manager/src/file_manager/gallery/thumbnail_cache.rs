//! Compressed, resolution-specific thumbnails, separate from dimensions and layout.
use crate::file_source::FileSource;
use little_clip_core::ImageThumbnail;
use std::{
    hash::{Hash, Hasher},
    io::Cursor,
    path::PathBuf,
};

pub(super) fn path(source: &FileSource, edge: u32) -> Option<PathBuf> {
    let store = little_clip_core::staging::ArtifactStore::new().ok()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    format!("{source:?}").hash(&mut hash);
    edge.hash(&mut hash);
    Some(
        store
            .base_dir()
            .join("gallery-thumbnails-v1")
            .join(format!("{:016x}.png", hash.finish())),
    )
}
pub(super) fn load(path: &std::path::Path) -> Option<ImageThumbnail> {
    let bytes = crate::file_source::read_file_bytes(path, 96 * 1024 * 1024, true, false).ok()?;
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(96 * 1024 * 1024);
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    reader.limits(limits);
    let image = reader.decode().ok()?.to_rgba8();
    Some(ImageThumbnail {
        width: image.width(),
        height: image.height(),
        premultiplied_rgba: image.into_raw(),
    })
}
pub(super) fn save(path: &std::path::Path, thumbnail: &ImageThumbnail) {
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = std::fs::create_dir_all(parent);
    let Some(image) = image::RgbaImage::from_raw(
        thumbnail.width,
        thumbnail.height,
        thumbnail.premultiplied_rgba.clone(),
    ) else {
        return;
    };
    if let Ok(mut temporary) = tempfile::NamedTempFile::new_in(parent) {
        if image::DynamicImage::ImageRgba8(image)
            .write_to(temporary.as_file_mut(), image::ImageFormat::Png)
            .is_ok()
        {
            let _ = temporary.persist(path);
        }
    }
}
