//! Source identity and guarded atomic saves for file-backed desktop editors.
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

type InputValidation = dyn Fn(&std::sync::atomic::AtomicBool) -> Result<(), String> + Send + Sync;

/// Optional source-observation validation retained across deferred studio jobs.
#[derive(Clone, Default)]
pub(crate) struct ReadCheck(Option<std::sync::Arc<InputValidation>>);
impl PartialEq for ReadCheck {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(left), Some(right)) => std::sync::Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}
impl std::fmt::Debug for ReadCheck {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadCheck")
            .field("active", &self.active())
            .finish()
    }
}
impl ReadCheck {
    pub fn new(
        check: impl Fn(&std::sync::atomic::AtomicBool) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self(Some(std::sync::Arc::new(check)))
    }
    pub fn active(&self) -> bool {
        self.0.is_some()
    }
    pub fn validate(&self, cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
        self.0.as_ref().map_or(Ok(()), |check| check(cancel))
    }
    /// Pins ordinary content and checks its observed identity before payload access.
    pub fn open(
        &self,
        path: &Path,
        expected: Option<&FileSource>,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(File, FileSource), String> {
        self.validate(cancel)?;
        let input = little_clip_core::kernel_interface::open_content(path, true)
            .map_err(|error| error.to_string())?;
        let observed = FileSource::capture_file(&input)?;
        if expected.is_some_and(|expected| *expected != observed) {
            return Err("File source changed; reload before reading.".into());
        }
        self.validate(cancel)?;
        Ok((input, observed))
    }
    pub fn read<T>(
        &self,
        cancel: &std::sync::atomic::AtomicBool,
        read: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.validate(cancel)?;
        let result = read()?;
        self.validate(cancel)?;
        Ok(result)
    }
}

#[cfg(test)]
pub(crate) mod read_check_tests {
    use super::ReadCheck;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    pub fn assert_deferred_rejected<T>(
        receiver: &std::sync::mpsc::Receiver<Result<Result<T, String>, little_clip_core::JobError>>,
    ) {
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(result.err().as_deref(), Some("source context expired"));
    }

    #[test]
    fn deferred_checks_reject_before_read_and_before_returning_content() {
        let changed = Arc::new(AtomicBool::new(false));
        let observed = changed.clone();
        let check = ReadCheck::new(move |_| {
            if observed.load(Ordering::Relaxed) {
                Err("source changed".into())
            } else {
                Ok(())
            }
        });
        let cancel = AtomicBool::new(false);
        assert!(check.active());
        assert_eq!(
            check
                .read(&cancel, || {
                    changed.store(true, Ordering::Relaxed);
                    Ok(42)
                })
                .unwrap_err(),
            "source changed"
        );
        assert_eq!(
            check
                .read(&cancel, || -> Result<(), String> {
                    panic!("a stale source must not be read")
                })
                .unwrap_err(),
            "source changed"
        );
        assert_eq!(ReadCheck::default().read(&cancel, || Ok(42)).unwrap(), 42);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSource {
    path: PathBuf,
    version: [u64; 7],
    mode: u32,
}
impl FileSource {
    /// The pathname retained with this observed or successfully saved revision.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Captures the resolved regular file's identity and timestamps on a worker.
    ///
    /// Classification, metadata and the resolved name come from the same pinned file.
    ///
    /// # Errors
    /// Returns an error for protected interfaces, non-regular files or unavailable metadata.
    pub fn capture(path: &Path) -> Result<Self, String> {
        let source =
            little_clip_core::kernel_interface::pin(path, true).map_err(|e| e.to_string())?;
        Self::capture_file(&source)
    }
    /// Captures identity from a retained ordinary-file handle without reopening its name.
    ///
    /// # Errors
    /// Returns an error for protected or non-regular handles, unavailable metadata,
    /// or a descriptor whose resolved name cannot be obtained.
    pub(crate) fn capture_file(source: &File) -> Result<Self, String> {
        if little_clip_core::kernel_interface::classify_handle(source)
            .map_err(|e| e.to_string())?
            .protected()
        {
            return Err("Kernel interfaces are inspection-only".into());
        }
        let metadata = source.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("Source is not a regular file".into());
        }
        let path = std::fs::read_link(format!("/proc/self/fd/{}", source.as_raw_fd()))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            path,
            version: [
                metadata.dev(),
                metadata.ino(),
                metadata.len(),
                metadata.mtime() as u64,
                metadata.mtime_nsec() as u64,
                metadata.ctime() as u64,
                metadata.ctime_nsec() as u64,
            ],
            mode: metadata.permissions().mode(),
        })
    }
    /// Returns a stable, byte-exact cache revision from the pinned file metadata.
    /// Includes device/inode, size, and mtime/ctime with nanosecond precision.
    /// This is a metadata cache token, not a content digest or read authorization;
    /// content readers must still open their inputs through the guarded path.
    pub(crate) fn revision_key(&self) -> Vec<u8> {
        self.version
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }
    /// Rejects a source whose identity or timestamps changed since capture.
    pub fn check(&self) -> Result<(), String> {
        if Self::capture(&self.path)? == *self {
            Ok(())
        } else {
            Err("Source changed outside Files. Reload it before saving.".into())
        }
    }
    /// Writes beside the destination and atomically installs the completed file.
    ///
    /// Replacing this source requires its identity and timestamps to match.
    /// Other destinations must be unused. Existing hard links retain their old
    /// contents after source replacement. The final check and rename are separate
    /// filesystem operations, so arbitrary concurrent writers are not locked out.
    /// Staging and installation retain the validated ordinary parent directory.
    ///
    /// # Errors
    /// Returns an error for protected/invalid parents, changed sources, existing Save As
    /// destinations or filesystem failures.
    pub fn write(&self, destination: &Path, bytes: &[u8]) -> Result<Self, String> {
        let parent = destination.parent().unwrap_or(Path::new("."));
        let directory = little_clip_core::kernel_interface::pin_content_directory(parent)
            .map_err(|e| e.to_string())?;
        let parent = std::fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            .map_err(|e| e.to_string())?;
        let destination = parent.join(destination.file_name().ok_or("Missing filename")?);
        self.write_in(&destination, bytes, &directory)
    }
    fn write_in(&self, destination: &Path, bytes: &[u8], directory: &File) -> Result<Self, String> {
        let parent = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let target = parent.join(destination.file_name().ok_or("Missing filename")?);
        let mut temporary = tempfile::NamedTempFile::new_in(&parent).map_err(|e| e.to_string())?;
        temporary.write_all(bytes).map_err(|e| e.to_string())?;
        let replacing = destination == self.path.as_path();
        if replacing {
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(self.mode))
                .map_err(|e| e.to_string())?;
        }
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        let installed = if replacing {
            let current = Self::capture(&target)?;
            if current != *self
                || std::fs::symlink_metadata(&target)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
            {
                return Err(
                    "Source changed outside Files. Reload it or save to a new filename.".into(),
                );
            }
            temporary.persist(&target).map_err(|e| e.to_string())?
        } else {
            temporary
                .persist_noclobber(&target)
                .map_err(|e| format!("Choose an unused filename: {e}"))?
        };
        Self::capture_file(&installed)
    }
}

/// Opens regular sources without blocking on a FIFO substituted after enumeration.
pub(crate) fn open_regular_file(path: &Path, follow_symlinks: bool) -> std::io::Result<File> {
    let file = little_clip_core::kernel_interface::open_content(path, follow_symlinks)?;
    Ok(file)
}

pub(crate) fn read_file_bytes(
    path: &Path,
    limit: usize,
    complete: bool,
    follow_symlinks: bool,
) -> Result<Vec<u8>, std::io::Error> {
    let file = open_regular_file(path, follow_symlinks)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + u64::from(complete))
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::other(
            "Preview source exceeds its read limit",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_source_metadata_cannot_adopt_a_replaced_path() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("installed.txt");
        let moved = temp.path().join("moved.txt");
        let mut installed = File::create(&target).unwrap();
        installed.write_all(b"installed snapshot").unwrap();
        let inode = installed.metadata().unwrap().ino();
        std::fs::rename(&target, &moved).unwrap();
        std::fs::write(&target, b"replacement").unwrap();
        let captured = FileSource::capture_file(&installed).unwrap();
        assert_eq!(captured.path, moved);
        assert_eq!(captured.version[1], inode);
        assert_ne!(captured, FileSource::capture(&target).unwrap());
        assert_eq!(
            std::fs::read(&captured.path).unwrap(),
            b"installed snapshot"
        );
        captured.check().unwrap();
    }

    #[test]
    fn save_as_uses_the_validated_directory_when_its_path_is_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        std::fs::write(&source, b"original").unwrap();
        let original = FileSource::capture(&source).unwrap();
        let parent = temp.path().join("parent");
        let moved = temp.path().join("moved");
        std::fs::create_dir(&parent).unwrap();
        let directory = little_clip_core::kernel_interface::pin_content_directory(&parent).unwrap();
        std::fs::rename(&parent, &moved).unwrap();
        // Cover replacement by a fresh directory; the naming fixture covers an alias.
        std::fs::create_dir(&parent).unwrap();
        let saved = original
            .write_in(&parent.join("saved.txt"), b"snapshot", &directory)
            .unwrap();
        assert_eq!(std::fs::read(moved.join("saved.txt")).unwrap(), b"snapshot");
        assert!(!parent.join("saved.txt").exists());
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
        assert_eq!(saved.path, moved.join("saved.txt"));
        saved.check().unwrap();
        original.check().unwrap();
    }

    #[test]
    fn source_replacement_updates_revision_preserves_mode_and_keeps_other_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let alias = dir.path().join("alias.txt");
        std::fs::write(&source, b"old").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        let original = FileSource::capture(&source).unwrap();
        let saved = original.write(&source, b"new bytes").unwrap();
        assert_ne!(saved, original);
        assert_eq!(std::fs::read(&source).unwrap(), b"new bytes");
        assert_eq!(std::fs::read(&alias).unwrap(), b"old");
        assert_eq!(
            std::fs::metadata(&source).unwrap().permissions().mode() & 0o777,
            0o640
        );
        saved.write(&source, b"second save").unwrap();
    }

    #[test]
    fn external_source_change_is_rejected_but_save_as_retains_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let output = dir.path().join("new.txt");
        std::fs::write(&source, b"old").unwrap();
        let original = FileSource::capture(&source).unwrap();
        std::fs::write(&source, b"external").unwrap();
        assert!(original.write(&source, b"edited").is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"external");
        let saved = original.write(&output, b"edited").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"edited");
        assert_eq!(std::fs::read(&source).unwrap(), b"external");
        saved.write(&output, b"next").unwrap();
    }

    #[test]
    fn save_as_never_replaces_an_existing_file_or_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let output = dir.path().join("existing.txt");
        let alias = dir.path().join("link.txt");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&output, b"existing").unwrap();
        std::os::unix::fs::symlink(&output, &alias).unwrap();
        let original = FileSource::capture(&source).unwrap();
        assert!(original.write(&output, b"edited").is_err());
        assert!(original.write(&alias, b"edited").is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"existing");
        assert!(
            std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn bounded_reads_distinguish_prefixes_whole_files_and_symlink_policy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let alias = dir.path().join("link.txt");
        std::fs::write(&source, b"abcdef").unwrap();
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        assert_eq!(read_file_bytes(&source, 3, false, false).unwrap(), b"abc");
        assert!(read_file_bytes(&source, 3, true, false).is_err());
        assert!(read_file_bytes(&alias, 6, true, false).is_err());
        assert_eq!(read_file_bytes(&alias, 6, true, true).unwrap(), b"abcdef");
        assert!(read_file_bytes(dir.path(), 6, true, true).is_err());
    }
}
