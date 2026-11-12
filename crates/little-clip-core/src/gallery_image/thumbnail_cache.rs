//! Compressed, resolution-specific thumbnails, separate from dimensions and layout.
use crate::ImageThumbnail;
use crate::file_source::FileSource;
use std::{
    hash::{Hash, Hasher},
    io::Cursor,
    path::PathBuf,
};

static BUDGET: crate::staging::cache::CacheBudget =
    crate::staging::cache::CacheBudget::new(512 * 1024 * 1024);

pub(super) fn path(source: &FileSource, edge: u32, orientation: u8) -> Option<PathBuf> {
    let store = crate::staging::ArtifactStore::new().ok()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    format!("{source:?}").hash(&mut hash);
    edge.hash(&mut hash);
    orientation.hash(&mut hash);
    let directory = store.base_dir().join("gallery-thumbnails-v1");
    BUDGET.maintain(&directory, 0, cache_filename);
    Some(directory.join(format!("{:016x}.png", hash.finish())))
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
    if let Ok(mut temporary) = tempfile::NamedTempFile::new_in(parent)
        && image::DynamicImage::ImageRgba8(image)
            .write_to(temporary.as_file_mut(), image::ImageFormat::Png)
            .is_ok()
    {
        let bytes = temporary.as_file().metadata().map_or(0, |meta| meta.len());
        if temporary.persist(path).is_ok() {
            BUDGET.maintain(parent, bytes, cache_filename);
        }
    }
}

fn cache_filename(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_suffix(".png"))
        .is_some_and(|key| key.len() == 16 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_sweep_reclaims_only_old_committed_images() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("thumbnails");
        std::fs::create_dir(&cache).unwrap();
        let old = cache.join("0000000000000001.png");
        let new = cache.join("0000000000000002.png");
        let staged = cache.join(".tmp-in-progress");
        let other = root.path().join("live-editor-file.png");
        for path in [&old, &new, &staged, &other] {
            std::fs::write(path, b"image").unwrap();
        }
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
        let alias = cache.join("0000000000000003.png");
        std::os::unix::fs::symlink(&other, &alias).unwrap();

        crate::staging::cache::sweep(&cache, 5, cache_filename).unwrap();

        assert!(!old.exists());
        assert!(new.exists());
        assert!(staged.exists());
        assert!(alias.is_symlink());
        assert!(other.exists());
    }
}
