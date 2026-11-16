//! Descriptor-relative ordinary content traversal with no symlink or mount escapes.
use crate::{Result, cancelled};
use little_clip_core::{
    drives::valid_relative,
    file_source::{FileSource, copy_checked},
    kernel_interface::{self, MountIdentity, Namespace},
};
use std::{
    fs::{self, File, Metadata},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

pub(crate) struct Root {
    pub path: PathBuf,
    file: File,
    identity: MountIdentity,
    namespace: Namespace,
}
pub(crate) fn anchor(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}
impl Root {
    pub fn open(path: &Path) -> Result<Self> {
        let file = kernel_interface::pin_content_directory(path)?;
        let path = fs::read_link(anchor(&file))?;
        let identity = MountIdentity::pinned(&file)?;
        let namespace = Namespace::current();
        if !namespace.known() {
            return Err("Cannot establish filesystem namespace".into());
        }
        Ok(Self {
            path,
            file,
            identity,
            namespace,
        })
    }
    pub fn create(path: &Path) -> Result<Self> {
        let file = kernel_interface::create_content_directory(path)?;
        Self::open(&anchor(&file))
    }
    pub fn storage_identity(&self) -> Result<little_clip_core::drives::StorageIdentity> {
        Ok(little_clip_core::drives::StorageIdentity::pinned(
            &self.file,
        )?)
    }
    pub fn validate(&self) -> Result<()> {
        if Namespace::current() != self.namespace
            || MountIdentity::directory(&self.path)? != self.identity
        {
            return Err("Directory or mount was replaced or removed".into());
        }
        Ok(())
    }
    pub fn overlap(&self, other: &Self) -> bool {
        self.path.starts_with(&other.path)
            || other.path.starts_with(&self.path)
            || (self.identity.device == other.identity.device
                && self.identity.inode == other.identity.inode)
    }
    pub fn directory(&self, relative: &Path, create: bool) -> Result<File> {
        if !valid_relative(relative) {
            return Err("Invalid relative directory".into());
        }
        self.validate()?;
        let mut held = self.file.try_clone()?;
        for component in relative.components() {
            let child = anchor(&held).join(component.as_os_str());
            if create {
                match fs::create_dir(&child) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.into()),
                }
            }
            let pin = kernel_interface::pin(&child, false)?;
            let id = MountIdentity::pinned(&pin)?;
            if !pin.metadata()?.is_dir()
                || kernel_interface::classify_handle(&pin)?.protected()
                || id.device != self.identity.device
                || id.mount != self.identity.mount
            {
                return Err("Symlink, special filesystem or nested mount in drive tree".into());
            }
            held = pin;
        }
        Ok(held)
    }
    pub fn parent(&self, relative: &Path, create: bool) -> Result<(File, PathBuf)> {
        if !valid_relative(relative) || relative.as_os_str().is_empty() {
            return Err("Invalid file address".into());
        }
        let parent = self.directory(relative.parent().unwrap_or(Path::new("")), create)?;
        let path = anchor(&parent).join(relative.file_name().ok_or("Missing filename")?);
        Ok((parent, path))
    }
    pub fn metadata(&self, relative: &Path) -> Result<Option<Metadata>> {
        let (_parent, path) = match self.parent(relative, false) {
            Ok(value) => value,
            Err(crate::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        match fs::symlink_metadata(path) {
            Ok(m) => Ok(Some(m)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn input(&self, relative: &Path) -> Result<File> {
        let (_parent, path) = self.parent(relative, false)?;
        Ok(kernel_interface::open_content(&path, false)?)
    }
    pub fn digest(&self, relative: &Path, cancel: &AtomicBool) -> Result<String> {
        let file = self.input(relative)?;
        Ok(FileSource::capture_file(&file)?.sha256(&file, cancel, std::time::Duration::MAX)?)
    }
    pub fn ensure_directory(&self, relative: &Path) -> Result<()> {
        self.directory(relative, true)?;
        Ok(())
    }
    pub fn copy(
        &self,
        from: &Root,
        path: &Path,
        target: &Path,
        expected: Option<&str>,
        source_hash: Option<&str>,
        cancel: &AtomicBool,
        delta: bool,
    ) -> Result<u64> {
        cancelled(cancel)?;
        from.validate()?;
        let mut input = from.input(path)?;
        let observed = FileSource::capture_file(&input)?;
        if let Some(hash) = source_hash {
            if observed.sha256(&input, cancel, std::time::Duration::MAX)? != hash {
                return Err("Source changed after inventory; retry reconciliation".into());
            }
        }
        let metadata = input.metadata()?;
        let (parent, destination) = self.parent(target, true)?;
        let previous = self.metadata(target)?;
        if previous.as_ref().is_some_and(|m| !m.is_file()) {
            return Err("Destination type conflict".into());
        }
        let old = previous.as_ref().map(|_| self.input(target)).transpose()?;
        let old_source = old.as_ref().map(FileSource::capture_file).transpose()?;
        if let Some(expected) = expected {
            if self.digest(target, cancel)? != expected {
                return Err("Destination changed during synchronization".into());
            }
        } else if previous.is_some() {
            return Err("Destination appeared during synchronization".into());
        }
        let mut temporary = tempfile::NamedTempFile::new_in(anchor(&parent))?;
        let cloned = if delta {
            old.as_ref().is_some_and(|old| {
                // SAFETY: FICLONE borrows two live regular-file descriptors.
                unsafe {
                    libc::ioctl(
                        temporary.as_file().as_raw_fd(),
                        0x40049409_u64 as libc::c_ulong,
                        old.as_raw_fd(),
                    ) == 0
                }
            })
        } else {
            false
        };
        let stats = copy_checked(&mut input, temporary.as_file_mut(), cancel, cloned, |_| {})?;
        if let Some(expected) = expected {
            if self.digest(target, cancel)? != expected {
                return Err("Destination changed before installation".into());
            }
        }
        observed.check_file(&input)?;
        if let (Some(old), Some(source)) = (&old, &old_source) {
            source.check_file(old)?;
        }
        temporary
            .as_file()
            .set_permissions(metadata.permissions())?;
        temporary
            .as_file()
            .set_times(fs::FileTimes::new().set_modified(metadata.modified()?))?;
        temporary.as_file().sync_all()?;
        self.validate()?;
        from.validate()?;
        if old.is_some() {
            temporary.persist(&destination).map_err(|e| e.error)?;
        } else {
            temporary
                .persist_noclobber(&destination)
                .map_err(|e| e.error)?;
        }
        File::open(anchor(&parent))?.sync_all()?;
        Ok(stats.written)
    }
    pub fn delete_file(&self, relative: &Path, expected: &str, cancel: &AtomicBool) -> Result<()> {
        let (parent, path) = self.parent(relative, false)?;
        let file = self.input(relative)?;
        let source = FileSource::capture_file(&file)?;
        if self.digest(relative, cancel)? != expected {
            return Err("Deletion target changed".into());
        }
        cancelled(cancel)?;
        source.check_file(&file)?;
        fs::remove_file(path)?;
        File::open(anchor(&parent))?.sync_all()?;
        Ok(())
    }
    pub fn same_file(&self, relative: &Path, dev: u64, ino: u64) -> Result<bool> {
        Ok(self
            .metadata(relative)?
            .is_some_and(|m| m.dev() == dev && m.ino() == ino))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_replacement_and_symlink_escapes_are_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir(&data).unwrap();
        let root = Root::open(&data).unwrap();
        fs::create_dir(data.join("actual")).unwrap();
        symlink("actual", data.join("alias")).unwrap();
        assert!(root.directory(Path::new("alias"), false).is_err());
        assert!(root.directory(Path::new("../outside"), true).is_err());
        fs::rename(&data, temp.path().join("old")).unwrap();
        fs::create_dir(&data).unwrap();
        assert!(root.validate().is_err());
    }
    #[test]
    fn atomic_copy_checks_existing_destinations_and_preserves_old_open_handles() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let source = Root::open(&a).unwrap();
        let target = Root::open(&b).unwrap();
        let cancel = AtomicBool::new(false);
        let name = Path::new("file");
        fs::write(a.join(name), b"original").unwrap();
        target
            .copy(&source, name, name, None, None, &cancel, false)
            .unwrap();
        let hash = target.digest(name, &cancel).unwrap();
        let old = fs::read(b.join(name)).unwrap();
        fs::write(a.join(name), b"replacement").unwrap();
        assert!(
            target
                .copy(&source, name, name, None, None, &cancel, false)
                .is_err()
        );
        assert_eq!(fs::read(b.join(name)).unwrap(), old);
        target
            .copy(&source, name, name, Some(&hash), None, &cancel, true)
            .unwrap();
        assert_eq!(fs::read(b.join(name)).unwrap(), b"replacement");
    }
}
