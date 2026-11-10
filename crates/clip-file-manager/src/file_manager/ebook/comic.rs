//! Lazy archive pages. Only the ZIP index and selected member enter memory.
use crate::file_source::FileSource;
use little_clip_core::{ebook::Chapter, natural_sort::natural_cmp, zip_content};
use std::{
    fs::File,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Document {
    archive: Mutex<zip::ZipArchive<File>>,
    source_file: File,
    source: Mutex<FileSource>,
    pub chapters: Vec<Chapter>,
}
impl Document {
    pub fn load(path: &Path, token: &AtomicBool) -> Result<Self, String> {
        let file = little_clip_core::kernel_interface::open_content(path, true)
            .map_err(|e| e.to_string())?;
        let source = FileSource::capture_file(&file)?;
        // This duplicate is used only for identity/metadata, never for seeking or reading.
        let source_file = file.try_clone().map_err(|e| e.to_string())?;
        let archive = zip_content::archive_file(file, 2 * 1024 * 1024 * 1024)?;
        let mut pages: Vec<_> = archive
            .file_names()
            .filter(|name| page_name(name))
            .map(str::to_owned)
            .collect();
        pages.sort_by(|a, b| natural_cmp(a, b));
        if token.load(Ordering::Relaxed) {
            return Err("Comic loading cancelled".into());
        }
        if pages.is_empty() {
            return Err("Comic archive has no supported raster pages".into());
        }
        if FileSource::capture_file(&source_file)? != source {
            return Err("Comic changed while opening".into());
        }
        Ok(Self {
            archive: Mutex::new(archive),
            source_file,
            source: Mutex::new(source),
            chapters: pages
                .into_iter()
                .map(|resource| Chapter {
                    title: resource.clone(),
                    resource,
                })
                .collect(),
        })
    }
    fn check(&self) -> Result<(), String> {
        let expected = self
            .source
            .lock()
            .map_err(|_| "Comic source lock failed")?
            .clone();
        if FileSource::capture_file(&self.source_file)? != expected {
            return Err("Comic changed outside Files; reopen it before reading more pages".into());
        }
        Ok(())
    }
    pub fn image(&self, resource: &str) -> Result<Vec<u8>, String> {
        let mut archive = self
            .archive
            .lock()
            .map_err(|_| "Comic archive lock failed")?;
        self.check()?;
        let bytes = zip_content::entry(&mut archive, resource, 32 * 1024 * 1024)?;
        self.check()?;
        Ok(bytes)
    }
    pub fn relocate(&self, source: FileSource) {
        if let Ok(mut expected) = self.source.lock() {
            *expected = source;
        }
    }
}
fn page_name(name: &str) -> bool {
    !name
        .split('/')
        .any(|part| part.starts_with('.') || part == "__MACOSX")
        && Path::new(name)
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "avif"
                )
            })
}

#[cfg(test)]
mod tests {
    use super::*;
    const COMIC: &[u8] = include_bytes!("../../../tests/fixtures/comic.cbz");
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Document) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("comic.cbz");
        std::fs::write(&path, COMIC).unwrap();
        let comic = Document::load(&path, &AtomicBool::new(false)).unwrap();
        (dir, path, comic)
    }
    #[test]
    fn pages_follow_natural_order_and_accepted_renames_keep_the_pin() {
        let (dir, path, comic) = fixture();
        assert_eq!(
            comic
                .chapters
                .iter()
                .map(|p| p.resource.as_str())
                .collect::<Vec<_>>(),
            ["page1.png", "page2.png", "page10.png"]
        );
        let page = comic.image("page2.png").unwrap();
        let pixels = image::load_from_memory(&page).unwrap().to_rgb8();
        assert_eq!(pixels.get_pixel(100, 100).0, [30, 170, 70]);
        let renamed = dir.path().join("renamed.cbz");
        std::fs::rename(&path, &renamed).unwrap();
        comic.relocate(FileSource::capture(&renamed).unwrap());
        assert_eq!(comic.image("page2.png").unwrap(), page);
        assert_eq!(std::fs::read(&renamed).unwrap(), COMIC);
        std::fs::write(&renamed, b"external modification").unwrap();
        assert!(
            comic
                .image("page2.png")
                .unwrap_err()
                .contains("changed outside")
        );
    }
    #[test]
    fn path_replacement_cannot_retarget_the_archive_handle() {
        let (_dir, path, comic) = fixture();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, COMIC).unwrap();
        assert!(comic.image("page1.png").is_err());
        assert!(Document::load(&path, &AtomicBool::new(true)).is_err());
    }
}
