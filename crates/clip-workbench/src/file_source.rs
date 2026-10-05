//! Source identity and guarded atomic saves for file-backed desktop editors.
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSource {
    path: PathBuf,
    version: [u64; 7],
    mode: u32,
}
impl FileSource {
    /// Captures the resolved regular file's identity and timestamps on a worker.
    pub fn capture(path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("Source is not a regular file".into());
        }
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
    pub fn write(&self, destination: &Path, bytes: &[u8]) -> Result<Self, String> {
        let parent = destination.parent().unwrap_or(Path::new("."));
        let parent = parent.canonicalize().map_err(|e| e.to_string())?;
        let destination = parent.join(destination.file_name().ok_or("Missing filename")?);
        let mut temporary = tempfile::NamedTempFile::new_in(&parent).map_err(|e| e.to_string())?;
        temporary.write_all(bytes).map_err(|e| e.to_string())?;
        let replacing = destination == self.path;
        if replacing {
            temporary
                .as_file()
                .set_permissions(std::fs::Permissions::from_mode(self.mode))
                .map_err(|e| e.to_string())?;
        }
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        if replacing {
            let current = Self::capture(&destination)?;
            if current != *self
                || std::fs::symlink_metadata(&destination)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink()
            {
                return Err(
                    "Source changed outside Files. Reload it or save to a new filename.".into(),
                );
            }
            temporary.persist(&destination).map_err(|e| e.to_string())?;
        } else {
            temporary
                .persist_noclobber(&destination)
                .map_err(|e| format!("Choose an unused filename: {e}"))?;
        }
        Self::capture(&destination)
    }
}

pub(crate) fn read_file_bytes(
    path: &Path,
    limit: usize,
    complete: bool,
    follow_symlinks: bool,
) -> Result<Vec<u8>, std::io::Error> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | if follow_symlinks { 0 } else { libc::O_NOFOLLOW })
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "Preview source is not a regular file",
        ));
    }
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
