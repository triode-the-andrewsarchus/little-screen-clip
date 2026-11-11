//! Converted PDFs with bounded storage and reader-aware eviction.
use crate::{
    file_source::FileSource,
    staging::{cache::CacheBudget, dirs_or_fallback_cache},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
};

static BUDGET: CacheBudget = CacheBudget::new(512 * 1024 * 1024);

/// Retains a converted PDF for the duration of a preview or copy.
///
/// Keep this value alive while using its path. The shared file lock prevents
/// cache eviction; filesystems without advisory locks use a private temporary file.
#[derive(Debug)]
pub struct RenderedPdf {
    path: PathBuf,
    _file: File,
    // Temporary paths contain the directory descriptor, so it must outlive cleanup.
    _temporary: Option<(tempfile::NamedTempFile, crate::staging::TemporaryWorkspace)>,
    _directory: File,
}
impl RenderedPdf {
    /// Returns the PDF path protected by this value's lifetime.
    pub fn as_path(&self) -> &Path {
        &self.path
    }
}
impl std::ops::Deref for RenderedPdf {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.as_path()
    }
}
impl AsRef<Path> for RenderedPdf {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}
impl Drop for RenderedPdf {
    fn drop(&mut self) {
        // A concurrently spawned child can briefly inherit this descriptor
        // before exec applies CLOEXEC. It is not a preview owner. Release this
        // guard's lock explicitly rather than waiting for such copies to close.
        let _ = self._file.unlock();
        if self._temporary.is_none() {
            let anchor = PathBuf::from(format!("/proc/self/fd/{}", self._directory.as_raw_fd()));
            // Retry a sweep that was deferred by active readers, even when no
            // further conversions are written after the preview closes.
            BUDGET.maintain(&anchor, 0, cache_filename);
        }
    }
}

/// Returns a cached Office PDF protected from eviction while the result is alive.
///
/// Cache identity includes the byte-exact resolved input path and file revision.
/// Idle committed PDFs are evicted oldest first toward a 512 MiB budget on first
/// access and after each 32 MiB of writes. Active readers may exceed that budget.
///
/// # Errors
/// Returns an error if the guarded source, cache directory, conversion, or
/// completed PDF cannot be read, or if the source changes during conversion.
pub fn ensure_rendered_pdf(input: &Path) -> Result<RenderedPdf, String> {
    let source = FileSource::capture(input)?;
    let mut hasher = Sha256::new();
    hasher.update(source.path().as_os_str().as_bytes());
    hasher.update(source.revision_key());
    let name = format!("{:x}.pdf", hasher.finalize());
    let cache_dir = dirs_or_fallback_cache()
        .join("little-screen-clip")
        .join("office_previews");
    ensure_in(input, &source, &cache_dir, &name, |output| {
        super::render_to_pdf(input, output)
    })
}

fn ensure_in(
    input: &Path,
    source: &FileSource,
    cache_dir: &Path,
    name: &str,
    render: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<RenderedPdf, String> {
    let directory = crate::kernel_interface::create_content_directory(cache_dir)
        .map_err(|error| error.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    BUDGET.maintain(&anchor, 0, cache_filename);
    let cached = anchor.join(name);
    if let Ok(pdf) = open_cached(&cached, &directory) {
        source.check()?;
        return Ok(pdf);
    }
    let workspace = crate::staging::ArtifactStore::new()
        .and_then(|store| store.create_temporary_workspace("office-cache-writes", "pdf"))
        .map_err(|error| error.to_string())?;
    let temporary = tempfile::Builder::new()
        .prefix(".render-")
        .suffix(".pdf")
        .tempfile_in(workspace.path())
        .map_err(|error| error.to_string())?;
    render(temporary.path())?;
    source
        .check()
        .map_err(|error| format!("{}: {error}", input.display()))?;
    let file = crate::file_source::open_regular_file(temporary.path(), false)
        .map_err(|error| error.to_string())?;
    let bytes = file.metadata().map_err(|error| error.to_string())?.len();
    if bytes == 0 {
        return Err("PDF rendering produced an empty output file".into());
    }
    let temporary = if file.try_lock_shared().is_ok() {
        match temporary.persist_noclobber(&cached) {
            Ok(_) => {
                let pdf = held_pdf(file, None, directory).map_err(|error| error.to_string())?;
                BUDGET.maintain(&anchor, bytes, cache_filename);
                return Ok(pdf);
            }
            Err(error) => {
                if let Ok(pdf) = open_cached(&cached, &directory) {
                    return Ok(pdf);
                }
                error.file
            }
        }
    } else {
        // No shared-lock support: private ownership gives readers the same
        // lifetime guarantee without leaving an unprotected committed cache file.
        temporary
    };
    held_pdf(file, Some((temporary, workspace)), directory).map_err(|error| error.to_string())
}

fn open_cached(path: &Path, directory: &File) -> io::Result<RenderedPdf> {
    use std::os::unix::fs::MetadataExt;
    let file = crate::file_source::open_regular_file(path, false)?;
    file.try_lock_shared().map_err(io::Error::other)?;
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if held.len() == 0 || !named.is_file() || (held.dev(), held.ino()) != (named.dev(), named.ino())
    {
        return Err(io::Error::other("Converted PDF changed before acquisition"));
    }
    held_pdf(file, None, directory.try_clone()?)
}

fn held_pdf(
    file: File,
    temporary: Option<(tempfile::NamedTempFile, crate::staging::TemporaryWorkspace)>,
    directory: File,
) -> io::Result<RenderedPdf> {
    let path = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    Ok(RenderedPdf {
        path,
        _file: file,
        _temporary: temporary,
        _directory: directory,
    })
}

fn cache_filename(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_suffix(".pdf"))
        .is_some_and(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_private_payload_removed(path: &Path) {
        assert!(!path.exists());
        // Concurrent process startup can briefly inherit a workspace lock
        // before exec. Recovery owns any marker left until that handle closes;
        // failed conversion bytes must already have been removed.
        if let Ok(entries) = fs::read_dir(path.parent().unwrap()) {
            for entry in entries.flatten() {
                assert_eq!(entry.file_name(), ".owner-lock");
            }
        }
    }

    #[test]
    fn eviction_preserves_each_reader_and_only_removes_committed_cache_files() {
        let store = crate::staging::ArtifactStore::new().unwrap();
        let root = tempfile::tempdir_in(store.base_dir()).unwrap();
        let input = root.path().join("input.docx");
        fs::write(&input, b"source").unwrap();
        let source = FileSource::capture(&input).unwrap();
        let cache = root.path().join("cache");
        let name = format!("{}.pdf", "a".repeat(64));
        let first = ensure_in(&input, &source, &cache, &name, |output| {
            fs::write(output, b"converted PDF").map_err(|error| error.to_string())
        })
        .unwrap();
        let second = ensure_in(&input, &source, &cache, &name, |_| {
            panic!("an unchanged input must reuse the conversion")
        })
        .unwrap();
        assert_eq!(first.as_path(), second.as_path());
        let old = cache.join(format!("{}.pdf", "b".repeat(64)));
        fs::write(&old, b"old conversion").unwrap();
        let unknown = cache.join("notes.pdf");
        let writing = cache.join(".render-in-progress.pdf");
        fs::write(&unknown, b"keep").unwrap();
        fs::write(&writing, b"partial").unwrap();
        let alias = cache.join(format!("{}.pdf", "c".repeat(64)));
        std::os::unix::fs::symlink(&unknown, &alias).unwrap();
        crate::staging::cache::sweep(&cache, 0, cache_filename).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read(&first).unwrap(), b"converted PDF");
        drop(first);
        crate::staging::cache::sweep(&cache, 0, cache_filename).unwrap();
        assert!(
            second.exists(),
            "each reader holds an independent shared lock"
        );
        let inherited = second._file.try_clone().unwrap();
        drop(second);
        crate::staging::cache::sweep(&cache, 0, cache_filename).unwrap();
        assert!(!cache.join(name).exists());
        drop(inherited);
        assert_eq!(fs::read(&unknown).unwrap(), b"keep");
        assert_eq!(fs::read(&writing).unwrap(), b"partial");
        assert!(alias.is_symlink());
    }

    #[test]
    fn failed_or_stale_conversions_remove_their_private_payloads() {
        let store = crate::staging::ArtifactStore::new().unwrap();
        let root = tempfile::tempdir_in(store.base_dir()).unwrap();
        let input = root.path().join("input.docx");
        fs::write(&input, b"source").unwrap();
        let cache = root.path().join("cache");
        let name = format!("{}.pdf", "d".repeat(64));
        for change_source in [false, true] {
            let source = FileSource::capture(&input).unwrap();
            let mut staged = None;
            let result = ensure_in(&input, &source, &cache, &name, |output| {
                staged = Some(output.to_owned());
                fs::write(output, b"partial PDF").unwrap();
                if change_source {
                    fs::write(&input, b"changed source").unwrap();
                    Ok(())
                } else {
                    Err("conversion failed".into())
                }
            });
            assert!(result.is_err());
            let staged = staged.unwrap();
            assert_private_payload_removed(&staged);
            assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        }
    }

    #[test]
    fn failed_cache_installation_returns_an_owned_preview_without_overwriting() {
        let store = crate::staging::ArtifactStore::new().unwrap();
        let root = tempfile::tempdir_in(store.base_dir()).unwrap();
        let input = root.path().join("input.docx");
        fs::write(&input, b"source").unwrap();
        let source = FileSource::capture(&input).unwrap();
        let cache = root.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let name = format!("{}.pdf", "e".repeat(64));
        let cached = cache.join(&name);
        // An invalid existing entry must not be overwritten while a reader
        // might still own it. The completed preview can use private storage.
        fs::write(&cached, b"").unwrap();
        let pdf = ensure_in(&input, &source, &cache, &name, |output| {
            fs::write(output, b"usable PDF").map_err(|error| error.to_string())
        })
        .unwrap();
        let path = pdf.as_path().to_owned();
        assert_ne!(path, cached);
        assert_eq!(fs::read(&pdf).unwrap(), b"usable PDF");
        assert_eq!(fs::read(&cached).unwrap(), b"");
        drop(pdf);
        assert_private_payload_removed(&path);
    }
}
