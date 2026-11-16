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

pub(crate) struct CopySpec<'a> {
    pub source: &'a Path,
    pub destination: &'a Path,
    pub source_hash: Option<&'a str>,
    pub destination_hash: Option<&'a str>,
    pub delta: bool,
}

pub(crate) struct Root {
    pub path: PathBuf,
    file: File,
    identity: MountIdentity,
    namespace: Namespace,
    physical: Option<PathBuf>,
}
/// Scheduling evidence only: this never authorizes content access or keeps a mount busy.
#[derive(Clone)]
pub(crate) struct Scope {
    path: PathBuf,
    identity: MountIdentity,
    namespace: Namespace,
    physical: Option<PathBuf>,
}
impl Scope {
    pub fn overlap(&self, other: &Self) -> bool {
        if self.namespace != other.namespace
            || self.path.starts_with(&other.path)
            || other.path.starts_with(&self.path)
            || (self.identity.device == other.identity.device
                && self.identity.inode == other.identity.inode)
        {
            return true;
        }
        if self.identity.device != other.identity.device {
            return false;
        }
        match (&self.physical, &other.physical) {
            (Some(a), Some(b)) => a.starts_with(b) || b.starts_with(a),
            _ => true,
        }
    }
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
        let physical = crate::mounts::inventory().ok().and_then(|mounts| {
            mounts
                .into_iter()
                .find(|mount| mount.id == identity.mount)
                .and_then(|mount| project_path(&path, &mount))
        });
        Ok(Self {
            path,
            file,
            identity,
            namespace,
            physical,
        })
    }
    pub fn create(path: &Path) -> Result<Self> {
        let file = kernel_interface::create_content_directory(path)?;
        Self::open(&anchor(&file))
    }
    pub fn storage_identity(&self) -> Result<little_clip_core::drives::StorageIdentity> {
        // Linux's FUSE statfs protocol omits f_fsid, even when the server supplies it.
        // Our owned RAM mount has a persistent session namespace; callers additionally
        // validate its retained tmpfs image and live mount before authorizing writes.
        if rustix::fs::fstatvfs(&self.file)?.f_fsid == 0 {
            let mount = crate::mounts::inventory()?
                .into_iter()
                .find(|m| m.id == self.identity.mount && m.filesystem == "fuse.clip-ram")
                .ok_or("Filesystem has no persistent identity")?;
            let id = mount
                .source
                .to_str()
                .and_then(|s| s.strip_prefix("little-clip-ram:"))
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                .ok_or("Unknown RAM mount identity")?;
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(id.as_bytes());
            return Ok(little_clip_core::drives::StorageIdentity {
                filesystem: u64::from_le_bytes(
                    digest[..8]
                        .try_into()
                        .map_err(|_| "Invalid identity digest")?,
                )
                .max(1),
                inode: self.file.metadata()?.ino(),
                birth: None,
            });
        }
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
    /// Only known local filesystems provide sufficiently precise change metadata.
    /// Removable FAT/NTFS, network and arbitrary FUSE sources require fresh hashes.
    pub fn reliable_revision_cache(&self) -> Result<bool> {
        Ok(reliable_change_metadata(
            rustix::fs::fstatfs(&self.file)?.f_type as u64,
        ))
    }
    pub fn same_root(&self, other: &Self) -> bool {
        self.identity == other.identity && self.namespace == other.namespace
    }
    pub fn overlap(&self, other: &Self) -> bool {
        self.scope().overlap(&other.scope())
    }
    pub fn scope(&self) -> Scope {
        Scope {
            path: self.path.clone(),
            identity: self.identity,
            namespace: self.namespace.clone(),
            physical: self.physical.clone(),
        }
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
                    Ok(()) => {
                        File::open(anchor(&held))?.sync_all()?;
                    }
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
        if relative.as_os_str().is_empty() {
            self.validate()?;
            return Ok(Some(self.file.metadata()?));
        }
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
    pub fn digest(&self, relative: &Path, max_bytes: u64, cancel: &AtomicBool) -> Result<String> {
        let file = self.input(relative)?;
        let source = FileSource::capture_file(&file)?;
        if file.metadata()?.len() > max_bytes {
            return Err("File exceeds the permitted fingerprint byte budget; data retained".into());
        }
        Ok(source.sha256(&file, cancel, std::time::Duration::MAX)?)
    }
    pub fn ensure_directory(&self, relative: &Path) -> Result<()> {
        let directory = self.directory(relative, true)?;
        File::open(anchor(&directory))?.sync_all()?;
        Ok(())
    }
    pub fn copy(&self, from: &Root, spec: CopySpec<'_>, cancel: &AtomicBool) -> Result<u64> {
        let CopySpec {
            source: path,
            destination: target,
            source_hash,
            destination_hash: expected,
            delta,
        } = spec;
        cancelled(cancel)?;
        from.validate()?;
        let mut input = from.input(path)?;
        let observed = FileSource::capture_file(&input)?;
        let metadata = input.metadata()?;
        let (parent, destination) = self.parent(target, true)?;
        let previous = self.metadata(target)?;
        if previous.as_ref().is_some_and(|m| !m.is_file()) {
            return Err("Destination type conflict".into());
        }
        let old = previous.as_ref().map(|_| self.input(target)).transpose()?;
        let old_source = old.as_ref().map(FileSource::capture_file).transpose()?;
        if let Some(expected) = expected {
            if self.digest(target, u64::MAX, cancel)? != expected {
                return Err("Destination changed during synchronization".into());
            }
        } else if previous.is_some() {
            return Err("Destination appeared during synchronization".into());
        }
        if rustix::fs::fstatfs(&parent)?.f_type as u32 == 0x4d44 && metadata.len() > u32::MAX as u64
        {
            return Err("FAT32 cannot store a file larger than 4 GB minus one byte".into());
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
        let stats = copy_checked(
            &mut input,
            temporary.as_file_mut(),
            cancel,
            cloned,
            source_hash,
            |_| {},
        )?;
        if let Some(expected) = expected
            && self.digest(target, u64::MAX, cancel)? != expected
        {
            return Err("Destination changed before installation".into());
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
    fn remove_verified(
        &self,
        relative: &Path,
        cancel: &AtomicBool,
        verify: impl Fn(&Path) -> Result<()>,
    ) -> Result<()> {
        let (parent, path) = self.parent(relative, false)?;
        verify(&path)?;
        cancelled(cancel)?;
        let name = format!(".little-clip-delete-{}", uuid::Uuid::new_v4());
        let quarantine = anchor(&parent).join(&name);
        little_clip_core::file_source::rename_noreplace(&path, &quarantine)?;
        let result: Result<()> = (|| {
            self.validate()?;
            cancelled(cancel)?;
            verify(&quarantine)?;
            fs::remove_file(&quarantine)?;
            Ok(())
        })();
        if let Err(error) = result {
            let recovery =
                if little_clip_core::file_source::rename_noreplace(&quarantine, &path).is_ok() {
                    "original restored".into()
                } else {
                    format!(
                        "retained at {}",
                        self.path
                            .join(relative.parent().unwrap_or(Path::new("")))
                            .join(name)
                            .display()
                    )
                };
            return Err(format!("Deletion stopped: {error}; {recovery}").into());
        }
        File::open(anchor(&parent))?.sync_all()?;
        Ok(())
    }
    pub fn delete_file(&self, relative: &Path, expected: &str, cancel: &AtomicBool) -> Result<()> {
        let file = self.input(relative)?;
        let identity = file.metadata()?;
        self.remove_verified(relative, cancel, |path| {
            let held = kernel_interface::open_content(path, false)?;
            let metadata = held.metadata()?;
            if metadata.dev() != identity.dev()
                || metadata.ino() != identity.ino()
                || FileSource::capture_file(&held)?.sha256(
                    &held,
                    cancel,
                    std::time::Duration::MAX,
                )? != expected
            {
                return Err("Deletion target changed".into());
            }
            Ok(())
        })
    }
    pub fn delete_symlink(
        &self,
        relative: &Path,
        target: &Path,
        identity: (u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.remove_verified(relative, cancel, |path| {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.file_type().is_symlink()
                || (metadata.dev(), metadata.ino()) != identity
                || fs::read_link(path)? != target
            {
                return Err("Symlink deletion target changed".into());
            }
            Ok(())
        })
    }
    pub fn same_file(&self, relative: &Path, dev: u64, ino: u64) -> Result<bool> {
        Ok(self
            .metadata(relative)?
            .is_some_and(|m| m.dev() == dev && m.ino() == ino))
    }
}

fn project_path(path: &Path, mount: &crate::mounts::Mount) -> Option<PathBuf> {
    Some(mount.root.join(path.strip_prefix(&mount.target).ok()?))
}

fn reliable_change_metadata(filesystem: u64) -> bool {
    matches!(
        filesystem as u32,
        0xef53 | 0x9123683e | 0x58465342 | 0x01021994
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bind_subtrees_project_into_the_original_filesystem_coordinates() {
        let mount = crate::mounts::Mount {
            id: 1,
            target: "/alias".into(),
            root: "/original/child".into(),
            source: "/dev/fixture".into(),
            filesystem: "ext4".into(),
            read_only: false,
        };
        assert_eq!(
            project_path(Path::new("/alias/nested"), &mount),
            Some(PathBuf::from("/original/child/nested"))
        );
        assert!(project_path(Path::new("/elsewhere"), &mount).is_none());
        let namespace = Namespace::current();
        let source = Scope {
            path: "/mounted".into(),
            identity: MountIdentity {
                device: 7,
                inode: 1,
                mount: 1,
            },
            namespace: namespace.clone(),
            physical: Some("/original".into()),
        };
        let alias = Scope {
            path: "/alias/nested".into(),
            identity: MountIdentity {
                device: 7,
                inode: 2,
                mount: 2,
            },
            physical: project_path(Path::new("/alias/nested"), &mount),
            namespace,
        };
        assert!(source.overlap(&alias));
        let unrelated = Scope {
            path: "/other".into(),
            physical: Some("/unrelated".into()),
            ..alias.clone()
        };
        assert!(!source.overlap(&unrelated));
        let uncertain = Scope {
            physical: None,
            ..unrelated
        };
        assert!(source.overlap(&uncertain));
    }
    #[test]
    fn coarse_or_external_filesystems_never_authorize_fingerprint_cache_reuse() {
        for magic in [
            0x4d44, 0x2011bab0, 0x5346544e, 0x7366746e, 0xff534d42, 0x65735546, 0,
        ] {
            assert!(!reliable_change_metadata(magic));
        }
        for magic in [0xef53, 0x9123683e, 0x58465342, 0x01021994] {
            assert!(reliable_change_metadata(magic));
        }
    }
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
            .copy(
                &source,
                CopySpec {
                    source: name,
                    destination: name,
                    source_hash: None,
                    destination_hash: None,
                    delta: false,
                },
                &cancel,
            )
            .unwrap();
        let hash = target.digest(name, u64::MAX, &cancel).unwrap();
        let old = fs::read(b.join(name)).unwrap();
        fs::write(a.join(name), b"replacement").unwrap();
        assert!(
            target
                .copy(
                    &source,
                    CopySpec {
                        source: name,
                        destination: name,
                        source_hash: None,
                        destination_hash: None,
                        delta: false
                    },
                    &cancel
                )
                .is_err()
        );
        assert_eq!(fs::read(b.join(name)).unwrap(), old);
        target
            .copy(
                &source,
                CopySpec {
                    source: name,
                    destination: name,
                    source_hash: None,
                    destination_hash: Some(&hash),
                    delta: true,
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(fs::read(b.join(name)).unwrap(), b"replacement");
    }
}
