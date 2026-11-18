//! Guarded source reads and descriptor-relative atomic geography asset installation.
use crate::{file_source::FileSource, kernel_interface};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Write},
    os::fd::{AsRawFd, RawFd},
    path::{Component, Path, PathBuf},
};

/// Ordinary directory retained throughout asset staging, installation and cleanup.
pub struct AssetDirectory {
    directory: File,
    display_path: PathBuf,
}
impl AssetDirectory {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::from_file(kernel_interface::pin_content_directory(path)?)
    }
    pub fn create(path: &Path) -> io::Result<Self> {
        Self::from_file(kernel_interface::create_content_directory(path)?)
    }
    fn from_file(directory: File) -> io::Result<Self> {
        let display_path = fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))?;
        Ok(Self {
            directory,
            display_path,
        })
    }
    pub fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }
    pub fn display_path(&self) -> &Path {
        &self.display_path
    }
    pub fn open_file(&self, name: &OsStr) -> io::Result<File> {
        kernel_interface::open_content(&self.target(name)?, true)
    }
    pub fn stage(&self) -> io::Result<tempfile::NamedTempFile> {
        kernel_interface::pin_content_directory(&self.path())?;
        tempfile::NamedTempFile::new_in(self.path())
    }
    fn target(&self, name: &OsStr) -> io::Result<PathBuf> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Asset names must be a single ordinary filename",
            ));
        }
        Ok(self.path().join(name))
    }
    /// Installs a prewritten owned staging descriptor without removing the old
    /// destination first. Existing symlinks, special files and kernel aliases
    /// are rejected. All lookups remain relative to this held ordinary parent.
    pub fn install(&self, staged: tempfile::NamedTempFile, name: &OsStr) -> io::Result<File> {
        let target = self.target(name)?;
        kernel_interface::pin_content_directory(&self.path())?;
        let previous = match kernel_interface::pin(&target, false) {
            Ok(file) => {
                if !file.metadata()?.is_file()
                    || kernel_interface::classify_handle(&file)?.protected()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "Asset destination is not an ordinary regular file",
                    ));
                }
                Some(FileSource::capture_file(&file).map_err(io::Error::other)?)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        staged.as_file().sync_all()?;
        if let Some(previous) = previous {
            let now = kernel_interface::pin(&target, false)?;
            if FileSource::capture_file(&now).map_err(io::Error::other)? != previous {
                return Err(io::Error::other(
                    "Asset destination changed before installation",
                ));
            }
            staged.persist(&target).map_err(|error| error.error)
        } else {
            staged
                .persist_noclobber(&target)
                .map_err(|error| error.error)
        }
    }
    pub fn write(&self, name: &OsStr, bytes: &[u8]) -> io::Result<File> {
        let mut staged = self.stage()?;
        staged.write_all(bytes)?;
        self.install(staged, name)
    }
}
impl AsRawFd for AssetDirectory {
    fn as_raw_fd(&self) -> RawFd {
        self.directory.as_raw_fd()
    }
}

/// Reads one complete bounded snapshot using core's source identity checks.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    crate::file_source::read_file_snapshot_with_source(path.as_ref(), 512 * 1024 * 1024)
        .map(|(bytes, _)| bytes)
}
/// Atomically updates a derived cache through its retained ordinary directory.
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let directory = AssetDirectory::open(
        path.parent()
            .ok_or_else(|| io::Error::other("Missing asset directory"))?,
    )?;
    directory.write(
        path.file_name()
            .ok_or_else(|| io::Error::other("Missing asset filename"))?,
        bytes.as_ref(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn installation_rejects_symlinks_and_keeps_the_previous_destination() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, b"original").unwrap();
        let directory = AssetDirectory::create(&root.path().join("assets")).unwrap();
        symlink(&outside, directory.path().join("tile.tif")).unwrap();
        assert!(
            directory
                .write(OsStr::new("tile.tif"), b"replacement")
                .is_err()
        );
        assert_eq!(fs::read(outside).unwrap(), b"original");
        directory.write(OsStr::new("cache"), b"previous").unwrap();
        let mut staged = directory.stage().unwrap();
        staged.write_all(b"new").unwrap();
        assert!(directory.install(staged, OsStr::new("../escape")).is_err());
        assert_eq!(
            fs::read(directory.path().join("cache")).unwrap(),
            b"previous"
        );
    }
    #[test]
    fn staging_and_installation_keep_the_original_directory_after_a_rename() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("assets");
        let directory = AssetDirectory::create(&original).unwrap();
        let mut staged = directory.stage().unwrap();
        staged.write_all(b"held directory").unwrap();
        let moved = root.path().join("moved");
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        let installed = directory.install(staged, OsStr::new("tile.tif")).unwrap();
        assert!(installed.metadata().unwrap().is_file());
        assert_eq!(fs::read(moved.join("tile.tif")).unwrap(), b"held directory");
        assert!(!original.join("tile.tif").exists());
    }
    #[test]
    fn asset_reads_reject_devices_and_fifos_without_reading_payloads() {
        assert!(read("/dev/zero").is_err());
        let root = tempfile::tempdir().unwrap();
        let fifo = root.path().join("tile.tif");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: this private fixture pathname is NUL-terminated and not shared.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read(&fifo).is_err());
        symlink("/proc/self/status", root.path().join("cache")).unwrap();
        assert!(read(root.path().join("cache")).is_err());
    }
}
