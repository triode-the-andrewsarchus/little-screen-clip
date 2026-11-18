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
    /// Restore mode/mtime here; false when the caller owns richer restoration.
    pub basic_metadata: bool,
}

pub(crate) struct Root {
    pub path: PathBuf,
    file: File,
    identity: MountIdentity,
    namespace: Namespace,
    physical: Option<PathBuf>,
    read_only_required: bool,
}
/// Scheduling evidence only: this never authorizes content access or keeps a mount busy.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    path: PathBuf,
    identity: MountIdentity,
    namespace: Namespace,
    physical: Option<PathBuf>,
    object: bool,
}
impl Scope {
    pub fn overlap(&self, other: &Self) -> bool {
        if self.namespace != other.namespace
            || self.path.starts_with(&other.path)
            || other.path.starts_with(&self.path)
            || (self.identity.device == other.identity.device
                && self.identity.inode == other.identity.inode
                && self.object == other.object)
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
        Self::open_with_read_only(path, false)
    }
    pub fn open_with_read_only(path: &Path, read_only_required: bool) -> Result<Self> {
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
                .and_then(|mount| mount.project(&path))
        });
        let root = Self {
            path,
            file,
            identity,
            namespace,
            physical,
            read_only_required,
        };
        if read_only_required {
            root.validate()?;
        }
        Ok(root)
    }
    pub fn read_only_required(&self) -> bool {
        self.read_only_required
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
            || fs::read_link(anchor(&self.file))? != self.path
            || MountIdentity::directory(&self.path)? != self.identity
        {
            return Err("Directory or mount was replaced or removed".into());
        }
        if self.read_only_required {
            self.validate_read_only()?;
        }
        Ok(())
    }
    fn validate_read_only(&self) -> Result<()> {
        let mounted_read_only = rustix::fs::fstatvfs(&self.file)?
            .f_flag
            .contains(rustix::fs::StatVfsMountFlags::RDONLY);
        let mount = crate::mounts::inventory()?
            .into_iter()
            .find(|mount| mount.id == self.identity.mount)
            .ok_or("Read-only collection mount is no longer present")?;
        if !mounted_read_only || !mount.super_read_only {
            return Err("Read-only collection requires the source filesystem to remain read-only; its current mount or backing filesystem is writable".into());
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
    /// Stronger eligibility for importing fingerprints without event coverage.
    /// Keep the conservative kernel/driver allowlist separate from nanosecond
    /// storage precision. Legacy/coarse inode timestamps are rejected per entry.
    pub fn queried_change_tracking(&self) -> Result<bool> {
        if !self.reliable_revision_cache()? {
            return Ok(false);
        }
        let filesystem = crate::mounts::inventory()?
            .into_iter()
            .find(|mount| mount.id == self.identity.mount)
            .map(|mount| mount.filesystem);
        let uname = rustix::system::uname();
        Ok(filesystem.is_some_and(|filesystem| {
            queried_change_metadata(uname.release().to_bytes(), &filesystem)
        }))
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
            physical: (fs::read_link(anchor(&self.file)).ok().as_ref() == Some(&self.path))
                .then(|| self.physical.clone())
                .flatten(),
            object: true,
        }
    }
    /// Plans a destination region without creating it or authorizing a write.
    /// Missing siblings share uncertain parent-name semantics until created.
    pub fn planned_scope(path: &Path) -> Result<Scope> {
        let absolute = std::path::absolute(path)?;
        for ancestor in absolute.ancestors() {
            let root = match Self::open(ancestor) {
                Ok(root) => root,
                Err(crate::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let relative = absolute
                .strip_prefix(ancestor)
                .map_err(|_| "Invalid destination ancestry")?;
            if !valid_relative(relative) {
                return Err("Invalid future destination address".into());
            }
            root.ordinary_names(relative)?;
            let mut scope = root.scope();
            if !relative.as_os_str().is_empty() {
                scope.path.push(relative);
                scope.physical = scope.physical.map(|path| path.join(relative));
                scope.object = false;
            }
            root.validate()?;
            return Ok(scope);
        }
        Err("No ordinary destination ancestor is available".into())
    }
    fn ordinary_names(&self, relative: &Path) -> Result<()> {
        let stream = relative.as_os_str().as_encoded_bytes().contains(&b':');
        let trailing = relative.components().any(|part| {
            matches!(
                part.as_os_str().as_encoded_bytes().last(),
                Some(b'.' | b' ')
            )
        });
        if !stream && !trailing {
            return Ok(());
        }
        self.validate()?;
        let mount = crate::mounts::inventory()?
            .into_iter()
            .find(|mount| mount.id == self.identity.mount)
            .ok_or("Cannot establish filesystem name semantics; data retained")?;
        // NTFS3 treats ':' as alternate-stream syntax. A staging rename can
        // otherwise report success while installing only the basename prefix.
        // Select semantics from the pinned mount, not a label or pathname hint.
        if stream && matches!(mount.filesystem.as_str(), "ntfs3" | "ntfs3_64") {
            return Err(format!(
                "NTFS alternate-stream syntax is not an ordinary path: {}. Rename this item before synchronizing",
                little_clip_core::path_address::display_path(relative)
            ).into());
        }
        if trailing && matches!(mount.filesystem.as_str(), "vfat" | "msdos" | "exfat") {
            return Err(format!(
                "This filesystem normalizes trailing dots or spaces in path \"{}\". Rename this item before synchronizing",
                little_clip_core::path_address::display_path(relative)
            ).into());
        }
        Ok(())
    }
    pub fn directory(&self, relative: &Path, create: bool) -> Result<File> {
        if !valid_relative(relative) {
            return Err("Invalid relative directory".into());
        }
        self.validate()?;
        self.ordinary_names(relative)?;
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
            let pin = self.pin_child(&held, component.as_os_str())?;
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
    /// Selects a pinned ordinary subtree through the existing no-crossing walk.
    pub fn subtree(&self, relative: &Path) -> Result<Self> {
        let directory = self.directory(relative, false)?;
        Self::open_with_read_only(&anchor(&directory), self.read_only_required)
    }
    /// Refuse mount crossings during lookup, before foreign getattr/open hooks.
    /// Keep the resulting pin through guarded reopen instead of resolving its name.
    pub(crate) fn pin_child(&self, parent: &File, name: &std::ffi::OsStr) -> std::io::Result<File> {
        use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
        Ok(File::from(openat2(
            parent,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )?))
    }
    pub fn parent(&self, relative: &Path, create: bool) -> Result<(File, PathBuf)> {
        if !valid_relative(relative) || relative.as_os_str().is_empty() {
            return Err("Invalid file address".into());
        }
        self.ordinary_names(relative)?;
        let parent = self.directory(relative.parent().unwrap_or(Path::new("")), create)?;
        let path = anchor(&parent).join(relative.file_name().ok_or("Missing filename")?);
        Ok((parent, path))
    }
    /// Retains the directory used for effective-user access and read-only checks.
    /// Mutations still revalidate their own held parents at execution.
    pub fn writable_directory(&self) -> Result<(File, rustix::fs::StatVfs)> {
        let pin = self.directory(Path::new(""), false)?;
        let stats = rustix::fs::fstatvfs(&pin)?;
        if stats.f_flag.contains(rustix::fs::StatVfsMountFlags::RDONLY) {
            return Err("Directory is read-only".into());
        }
        rustix::fs::accessat(
            &pin,
            ".",
            rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
            rustix::fs::AtFlags::EACCESS,
        )?;
        Ok((pin, stats))
    }
    pub fn metadata(&self, relative: &Path) -> Result<Option<Metadata>> {
        if relative.as_os_str().is_empty() {
            self.validate()?;
            return Ok(Some(self.file.metadata()?));
        }
        let (parent, _) = match self.parent(relative, false) {
            Ok(value) => value,
            Err(crate::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        match self
            .pin_child(&parent, relative.file_name().ok_or("Missing filename")?)
            .and_then(|file| file.metadata())
        {
            Ok(m) => Ok(Some(m)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn input(&self, relative: &Path) -> Result<File> {
        let (parent, _) = self.parent(relative, false)?;
        let pin = self.pin_child(&parent, relative.file_name().ok_or("Missing filename")?)?;
        if !pin.metadata()?.is_file() {
            return Err(
                "Drive content inputs must be ordinary files, not links or special entries".into(),
            );
        }
        let file = kernel_interface::open_content(&anchor(&pin), true)?;
        let identity = MountIdentity::pinned(&file)?;
        if identity.device != self.identity.device || identity.mount != self.identity.mount {
            return Err("Nested file mount in drive tree; refusing content access".into());
        }
        Ok(file)
    }
    pub fn digest(&self, relative: &Path, max_bytes: u64, cancel: &AtomicBool) -> Result<String> {
        let file = self.input(relative)?;
        let source = FileSource::capture_file(&file)?;
        if file.metadata()?.len() > max_bytes {
            return Err("File exceeds the permitted fingerprint byte budget; data retained".into());
        }
        Ok(source.sha256(&file, cancel, std::time::Duration::MAX)?)
    }
    pub fn flush(&self) -> Result<()> {
        self.validate()?;
        let directory = File::open(anchor(&self.file))?;
        rustix::fs::syncfs(&directory)?;
        self.validate()
    }
    pub fn ensure_directory(&self, relative: &Path) -> Result<()> {
        let directory = self.directory(relative, true)?;
        File::open(anchor(&directory))?.sync_all()?;
        Ok(())
    }
    pub fn check_file_size(&self, bytes: u64) -> Result<()> {
        self.validate()?;
        if bytes > u32::MAX as u64 && rustix::fs::fstatfs(&self.file)?.f_type as u32 == 0x4d44 {
            return Err(
                "FAT32 cannot store a file larger than 4 GiB minus one byte (4294967295 bytes)"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn copy(&self, from: &Root, spec: CopySpec<'_>, cancel: &AtomicBool) -> Result<u64> {
        let CopySpec {
            source: path,
            destination: target,
            source_hash,
            destination_hash: expected,
            delta,
            basic_metadata,
        } = spec;
        cancelled(cancel)?;
        from.validate()?;
        let mut input = from.input(path)?;
        let observed = FileSource::capture_file(&input)?;
        let metadata = input.metadata()?;
        self.check_file_size(metadata.len())?;
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
        if basic_metadata {
            temporary
                .as_file()
                .set_permissions(metadata.permissions())?;
            temporary
                .as_file()
                .set_times(fs::FileTimes::new().set_modified(metadata.modified()?))?;
        }
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

fn reliable_change_metadata(filesystem: u64) -> bool {
    matches!(
        filesystem as u32,
        0xef53 | 0x9123683e | 0x58465342 | 0x01021994
    )
}
fn queried_change_metadata(release: &[u8], filesystem: &str) -> bool {
    let version = std::str::from_utf8(release).ok().and_then(|release| {
        let (major, rest) = release.split_once('.')?;
        let minor = rest.split('.').next()?.parse::<u32>().ok()?;
        Some((major.parse::<u32>().ok()?, minor))
    });
    version.is_some_and(|(major, _)| major >= 7)
        && matches!(filesystem, "ext4" | "btrfs" | "xfs" | "tmpfs")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn future_destination_scopes_never_create_paths_or_invent_object_identity() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first/new");
        let second = temp.path().join("second/new");
        let planned = Root::planned_scope(&first).unwrap();
        let sibling = Root::planned_scope(&second).unwrap();
        assert!(!first.parent().unwrap().exists());
        assert!(!second.parent().unwrap().exists());
        assert!(
            planned.overlap(&sibling),
            "Uncreated sibling names have uncertain equivalence"
        );
        assert!(planned.overlap(&Root::open(temp.path()).unwrap().scope()));
        fs::create_dir_all(&first).unwrap();
        let installed = Root::open(&first).unwrap().scope();
        assert!(planned.overlap(&installed));
        assert!(
            !installed.overlap(&sibling),
            "An existing disjoint sibling is not its parent object"
        );
    }
    #[test]
    fn future_destination_scopes_resolve_existing_aliases_and_reject_files() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let planned = Root::planned_scope(&alias.join("future")).unwrap();
        assert!(planned.overlap(&Root::planned_scope(&actual.join("future")).unwrap()));
        assert!(!actual.join("future").exists());
        fs::write(actual.join("file"), b"ordinary bytes").unwrap();
        assert!(Root::planned_scope(&actual.join("file/child")).is_err());
        assert_eq!(fs::read(actual.join("file")).unwrap(), b"ordinary bytes");
    }
    #[test]
    fn queried_change_tracking_requires_a_known_kernel_and_opted_in_driver() {
        for filesystem in ["ext4", "btrfs", "xfs", "tmpfs"] {
            for release in [b"7.0.0".as_slice(), b"7.3.4-custom", b"8.1.0"] {
                assert!(queried_change_metadata(release, filesystem));
            }
            for release in [
                b"6.12.0".as_slice(),
                b"6.19.2",
                b"7.invalid",
                b"7",
                b"",
                b"\xff",
            ] {
                assert!(!queried_change_metadata(release, filesystem));
            }
        }
        for filesystem in [
            "ext2",
            "ext3",
            "vfat",
            "exfat",
            "ntfs3",
            "fuse.ntfs",
            "nfs",
            "cifs",
            "unknown",
        ] {
            assert!(!queried_change_metadata(b"7.3.4", filesystem));
        }
    }
    #[test]
    fn leaf_links_remain_metadata_without_authorizing_content_reads() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = Root::open(temp.path()).unwrap();
        fs::write(temp.path().join("payload"), b"ordinary bytes").unwrap();
        for (name, target) in [
            ("relative", PathBuf::from("payload")),
            ("absolute", temp.path().join("payload")),
            ("outside", PathBuf::from("/proc/self/status")),
            ("missing", PathBuf::from("no-such-file")),
        ] {
            symlink(target, temp.path().join(name)).unwrap();
            assert!(
                root.metadata(Path::new(name))
                    .unwrap()
                    .unwrap()
                    .is_symlink()
            );
            assert!(root.input(Path::new(name)).is_err());
        }
        let mut bytes = [0; 14];
        use std::os::unix::fs::FileExt;
        root.input(Path::new("payload"))
            .unwrap()
            .read_exact_at(&mut bytes, 0)
            .unwrap();
        assert_eq!(&bytes, b"ordinary bytes");
        assert!(root.metadata(Path::new("absent")).unwrap().is_none());
    }
    #[test]
    fn read_only_collection_refuses_an_ordinary_writable_source() {
        let temp = tempfile::tempdir().unwrap();
        let error = Root::open_with_read_only(temp.path(), true)
            .err()
            .expect("writable source refused");
        assert!(
            error.to_string().contains("Read-only collection requires"),
            "{error}"
        );
        Root::open(temp.path()).unwrap().validate().unwrap();
    }
    #[test]
    fn ordinary_unix_colon_names_remain_data_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = Root::open(temp.path()).unwrap();
        fs::write(temp.path().join("literal:name.txt"), b"ordinary content").unwrap();
        assert!(
            root.metadata(Path::new("literal:name.txt"))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            root.input(Path::new("literal:name.txt"))
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            16
        );
        root.ensure_directory(Path::new("folder:part")).unwrap();
        assert!(temp.path().join("folder:part").is_dir());
        for name in ["trailing-dot.", "trailing-space "] {
            root.ensure_directory(Path::new(name)).unwrap();
            assert!(temp.path().join(name).is_dir());
        }
    }

    #[test]
    fn bind_subtrees_project_into_the_original_filesystem_coordinates() {
        let mount = crate::mounts::Mount {
            device: 0,
            id: 1,
            target: "/alias".into(),
            root: "/original/child".into(),
            source: "/dev/fixture".into(),
            filesystem: "ext4".into(),
            read_only: false,
            super_read_only: false,
        };
        assert_eq!(
            mount.project(Path::new("/alias/nested")),
            Some(PathBuf::from("/original/child/nested"))
        );
        assert!(mount.project(Path::new("/elsewhere")).is_none());
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
            object: true,
        };
        let alias = Scope {
            path: "/alias/nested".into(),
            identity: MountIdentity {
                device: 7,
                inode: 2,
                mount: 2,
            },
            physical: mount.project(Path::new("/alias/nested")),
            namespace,
            object: true,
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
        assert!(root.subtree(Path::new("alias")).is_err());
        assert!(root.subtree(Path::new("../outside")).is_err());
        assert_eq!(
            root.subtree(Path::new("actual")).unwrap().path,
            data.join("actual")
        );
        assert!(root.directory(Path::new("../outside"), true).is_err());
        fs::rename(&data, temp.path().join("old")).unwrap();
        symlink(temp.path().join("old"), &data).unwrap();
        assert!(
            root.validate().is_err(),
            "a new symlink cannot preserve the old root's named authorization"
        );
        let unrelated = Root::create(&temp.path().join("unrelated")).unwrap();
        assert!(
            root.scope().overlap(&unrelated.scope()),
            "a moved root has uncertain coordinates until its task retires"
        );
        fs::remove_file(&data).unwrap();
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
                    basic_metadata: true,
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
                        delta: false,
                        basic_metadata: true,
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
                    basic_metadata: true,
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(fs::read(b.join(name)).unwrap(), b"replacement");
    }
}
