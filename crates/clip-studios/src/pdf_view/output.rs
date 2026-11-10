//! Directory ownership for staged PDF results awaiting viewer publication.
use super::STAGED_PDF_FILENAME;
use std::{
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct PdfOutput {
    // Cleanup must run before the parent descriptor it uses is released.
    _directory: tempfile::TempDir,
    parent: File,
    staging: File,
    destination: PathBuf,
}

impl PdfOutput {
    pub fn generate(
        input: &Path,
        destination: &Path,
        expected_pages: usize,
        check_source: bool,
        perform: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<Self, String> {
        let source = check_source
            .then(|| crate::file_source::FileSource::capture(input))
            .transpose()?;
        let output = Self::new(destination)?;
        let staged = output.staged_path();
        perform(input, &staged)?;
        if let Some(source) = source {
            source.check()?;
        }
        let info = little_clip_core::pdf::get_pdf_info(&staged)?;
        if info.page_count != expected_pages {
            return Err(format!(
                "PDF output has {} pages; expected {}",
                info.page_count, expected_pages
            ));
        }
        little_clip_core::pdf::render_pdf_page_preview(&staged, 1, 64)?;
        Ok(output)
    }

    pub fn new(destination: &Path) -> Result<Self, String> {
        destination
            .file_name()
            .ok_or("Missing PDF output filename")?;
        let parent_path = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = little_clip_core::kernel_interface::pin_content_directory(parent_path)
            .map_err(|error| error.to_string())?;
        let anchor = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
        let directory = tempfile::tempdir_in(&anchor).map_err(|error| error.to_string())?;
        let staging = little_clip_core::kernel_interface::pin_content_directory(directory.path())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            _directory: directory,
            parent,
            staging,
            destination: destination.into(),
        })
    }

    pub fn staged_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.staging.as_raw_fd()))
            .join(STAGED_PDF_FILENAME)
    }

    pub fn publish(self, protected: bool) -> Result<PathBuf, String> {
        let parent = PathBuf::from(format!("/proc/self/fd/{}", self.parent.as_raw_fd()));
        let name = self
            .destination
            .file_name()
            .ok_or("Missing PDF output filename")?;
        let target = parent.join(name);
        if protected {
            std::fs::hard_link(self.staged_path(), &target).map_err(|error| {
                format!(
                    "Could not create PDF output; choose/remove the existing sibling first: {error}"
                )
            })?;
        } else {
            match little_clip_core::kernel_interface::require_content(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            std::fs::rename(self.staged_path(), &target).map_err(|error| error.to_string())?;
        }
        Ok(std::fs::read_link(&parent)
            .map(|parent| parent.join(name))
            .unwrap_or_else(|_| self.destination.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_uses_held_directories_and_cleans_staging_after_parent_replacement() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let output = PdfOutput::new(&original.join("result.pdf")).unwrap();
        std::fs::write(output.staged_path(), b"completed PDF").unwrap();
        let moved = root.path().join("moved");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();
        assert_eq!(output.publish(true).unwrap(), moved.join("result.pdf"));
        assert_eq!(
            std::fs::read(moved.join("result.pdf")).unwrap(),
            b"completed PDF"
        );
        assert_eq!(std::fs::read_dir(moved).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(original).unwrap().count(), 0);
    }

    #[test]
    fn publication_and_staging_reject_protected_references() {
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join("kernel");
        std::os::unix::fs::symlink("/proc", &alias).unwrap();
        assert!(PdfOutput::new(&alias.join("forbidden.pdf")).is_err());
        let target = root.path().join("result.pdf");
        std::os::unix::fs::symlink("/proc/uptime", &target).unwrap();
        let output = PdfOutput::new(&target).unwrap();
        std::fs::write(output.staged_path(), b"new PDF").unwrap();
        assert!(
            output
                .publish(false)
                .unwrap_err()
                .contains("inspection-only")
        );
        assert_eq!(
            std::fs::read_link(target).unwrap(),
            Path::new("/proc/uptime")
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
