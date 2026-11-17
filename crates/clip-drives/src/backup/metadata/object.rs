//! Retained ordinary handles and descriptor-backed symlink metadata operations.
// Following our /proc/self/fd magic link selects the held symlink inode itself;
// it does not traverse the symlink's user-provided referent. Keep the FD alive
// throughout each call and use following xattr/time APIs on that controlled view.
use super::*;
use rustix::{
    fs::AtFlags,
    process::{Gid, Uid},
};
use std::ffi::OsStr;

pub(super) enum Object {
    File(File),
    Link { parent: File, pin: File },
}

impl Object {
    pub fn validate_name(&self, root: &Root, entry: &Entry, cancel: &AtomicBool) -> Result<()> {
        cancelled(cancel)?;
        root.validate()?;
        let held = self.metadata()?;
        if Entry::from_metadata(&entry.path, &held).kind != entry.kind
            || !root.same_file(&entry.path, held.dev(), held.ino())?
        {
            return Err("Backup metadata target was replaced".into());
        }
        Ok(())
    }
    pub fn open(root: &Root, entry: &Entry) -> Result<Self> {
        match entry.kind.as_str() {
            "file" => Ok(Self::File(root.input(&entry.path)?)),
            "directory" => {
                let pin = root.directory(&entry.path, false)?;
                Ok(Self::File(File::open(anchor(&pin))?))
            }
            "symlink" => Self::open_link(root, &entry.path),
            _ => Err("Unsupported backup metadata object".into()),
        }
    }
    fn open_link(root: &Root, relative: &Path) -> Result<Self> {
        let (parent, path) = root.parent(relative, false)?;
        let pin = kernel_interface::pin(&path, false)?;
        if !pin.metadata()?.file_type().is_symlink() {
            return Err("Backup symlink was replaced".into());
        }
        let hierarchy =
            kernel_interface::MountIdentity::pinned(&root.directory(Path::new(""), false)?)?;
        let identity = kernel_interface::MountIdentity::pinned(&pin)?;
        if !hierarchy.same_hierarchy_mount(identity)
            || kernel_interface::classify_handle(&pin)?.protected()
        {
            return Err("Nested or protected symlink mount in backup metadata tree".into());
        }
        Ok(Self::Link { parent, pin })
    }
    pub fn metadata(&self) -> std::io::Result<fs::Metadata> {
        match self {
            Self::File(file) => file.metadata(),
            Self::Link { pin, .. } => pin.metadata(),
        }
    }
    pub fn list(&self, bytes: &mut [u8]) -> rustix::io::Result<usize> {
        match self {
            Self::File(file) => rustix::fs::flistxattr(file, bytes),
            Self::Link { pin, .. } => rustix::fs::listxattr(anchor(pin), bytes),
        }
    }
    pub fn get(&self, name: &[u8], bytes: &mut [u8]) -> rustix::io::Result<usize> {
        match self {
            Self::File(file) => rustix::fs::fgetxattr(file, OsStr::from_bytes(name), bytes),
            Self::Link { pin, .. } => {
                rustix::fs::getxattr(anchor(pin), OsStr::from_bytes(name), bytes)
            }
        }
    }
    pub fn set(&self, name: &[u8], bytes: &[u8]) -> rustix::io::Result<()> {
        match self {
            Self::File(file) => rustix::fs::fsetxattr(
                file,
                OsStr::from_bytes(name),
                bytes,
                rustix::fs::XattrFlags::empty(),
            ),
            Self::Link { pin, .. } => rustix::fs::setxattr(
                anchor(pin),
                OsStr::from_bytes(name),
                bytes,
                rustix::fs::XattrFlags::empty(),
            ),
        }
    }
    pub fn remove(&self, name: &[u8]) -> rustix::io::Result<()> {
        match self {
            Self::File(file) => rustix::fs::fremovexattr(file, OsStr::from_bytes(name)),
            Self::Link { pin, .. } => rustix::fs::removexattr(anchor(pin), OsStr::from_bytes(name)),
        }
    }
    pub fn chown(&self, uid: u32, gid: u32) -> std::io::Result<()> {
        let uid = Some(Uid::from_raw(uid));
        let gid = Some(Gid::from_raw(gid));
        Ok(match self {
            Self::File(file) => rustix::fs::fchown(file, uid, gid),
            Self::Link { pin, .. } => rustix::fs::chownat(
                pin,
                "",
                uid,
                gid,
                AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
            ),
        }?)
    }
    pub fn chmod(&self, mode: u32) -> std::io::Result<()> {
        match self {
            Self::File(file) => file.set_permissions(fs::Permissions::from_mode(mode)),
            Self::Link { .. } => Err(std::io::ErrorKind::Unsupported.into()),
        }
    }
    pub fn times(&self, accessed: (i64, i64), modified: (i64, i64)) -> std::io::Result<()> {
        match self {
            Self::File(file) => file.set_times(
                fs::FileTimes::new()
                    .set_accessed(
                        time(accessed.0, accessed.1).ok_or(std::io::ErrorKind::InvalidInput)?,
                    )
                    .set_modified(
                        time(modified.0, modified.1).ok_or(std::io::ErrorKind::InvalidInput)?,
                    ),
            ),
            Self::Link { pin, .. } => Ok(rustix::fs::utimensat(
                rustix::fs::CWD,
                anchor(pin),
                &rustix::fs::Timestamps {
                    last_access: rustix::fs::Timespec {
                        tv_sec: accessed.0,
                        tv_nsec: accessed.1,
                    },
                    last_modification: rustix::fs::Timespec {
                        tv_sec: modified.0,
                        tv_nsec: modified.1,
                    },
                },
                AtFlags::empty(),
            )?),
        }
    }
    pub fn sync(&self) -> std::io::Result<()> {
        match self {
            Self::File(file) => file.sync_all(),
            Self::Link { parent, .. } => File::open(anchor(parent))?.sync_all(),
        }
    }
    pub fn link_to(&self, path: &Path) -> std::io::Result<()> {
        let file = match self {
            Self::File(file) | Self::Link { pin: file, .. } => file,
        };
        // Following only our retained descriptor magic link selects its inode,
        // including a held symlink inode, without resolving its user referent.
        Ok(rustix::fs::linkat(
            rustix::fs::CWD,
            anchor(file),
            rustix::fs::CWD,
            path,
            AtFlags::SYMLINK_FOLLOW,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the disposable guest for trusted xattr and replacement acceptance"]
    fn retained_file_and_symlink_xattrs_cannot_redirect_to_a_replacement_or_referent() {
        crate::require_disposable_guest();
        let temp = tempfile::tempdir().unwrap();
        let foreign = temp.path().join("referent");
        std::fs::write(&foreign, b"foreign payload").unwrap();
        let key = b"trusted.little_clip.fixture";
        rustix::fs::setxattr(
            &foreign,
            "trusted.little_clip.fixture",
            b"referent canary",
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        let referent_before = std::fs::metadata(&foreign).unwrap();
        for symlink in [false, true] {
            let path = temp.path().join(format!("node-{symlink}"));
            if symlink {
                std::os::unix::fs::symlink(&foreign, &path).unwrap();
            } else {
                std::fs::write(&path, b"original").unwrap();
            }
            let root = Root::open(temp.path()).unwrap();
            let cancel = AtomicBool::new(false);
            let entry = scan::observe(
                &root,
                path.file_name().unwrap().as_ref(),
                false,
                u64::MAX,
                &cancel,
            )
            .unwrap()
            .unwrap();
            let held = Object::open(&root, &entry).unwrap();
            held.set(key, b"original metadata").unwrap();
            let mut names = [0; 1024];
            let count = held.list(&mut names).unwrap();
            assert!(
                names[..count]
                    .split(|byte| *byte == 0)
                    .any(|name| name == key)
            );
            let mut bytes = [0; 64];
            let count = held.get(key, &mut bytes).unwrap();
            assert_eq!(&bytes[..count], b"original metadata");
            std::fs::rename(&path, path.with_extension("retained")).unwrap();
            if symlink {
                std::os::unix::fs::symlink(&foreign, &path).unwrap();
            } else {
                std::fs::write(&path, b"replacement").unwrap();
            }
            let entry = scan::observe(
                &root,
                path.file_name().unwrap().as_ref(),
                false,
                u64::MAX,
                &cancel,
            )
            .unwrap()
            .unwrap();
            let replacement = Object::open(&root, &entry).unwrap();
            let replacement_before = replacement.metadata().unwrap();
            replacement.set(key, b"replacement metadata").unwrap();
            held.set(key, b"updated original").unwrap();
            let count = held.get(key, &mut bytes).unwrap();
            assert_eq!(&bytes[..count], b"updated original");
            let count = replacement.get(key, &mut bytes).unwrap();
            assert_eq!(&bytes[..count], b"replacement metadata");
            let linked = path.with_extension("hard-link");
            held.link_to(&linked).unwrap();
            let linked_metadata = fs::symlink_metadata(&linked).unwrap();
            assert_eq!(linked_metadata.ino(), held.metadata().unwrap().ino());
            assert_eq!(linked_metadata.nlink(), 2);
            if symlink {
                assert_eq!(fs::read_link(&linked).unwrap(), foreign);
            } else {
                assert_eq!(fs::read(&linked).unwrap(), b"original");
            }
            held.chown(1001, 1002).unwrap();
            held.times((1_500_000_000, 0), (1_600_000_000, 123_456_789))
                .unwrap();
            let original = held.metadata().unwrap();
            let replacement_after = replacement.metadata().unwrap();
            assert_eq!(
                (
                    original.uid(),
                    original.gid(),
                    original.mtime(),
                    original.mtime_nsec()
                ),
                (1001, 1002, 1_600_000_000, 123_456_789)
            );
            assert_eq!(
                (
                    replacement_before.uid(),
                    replacement_before.gid(),
                    replacement_before.mtime(),
                    replacement_before.mtime_nsec()
                ),
                (
                    replacement_after.uid(),
                    replacement_after.gid(),
                    replacement_after.mtime(),
                    replacement_after.mtime_nsec()
                )
            );
            held.remove(key).unwrap();
            assert_eq!(held.get(key, &mut bytes), Err(rustix::io::Errno::NODATA));
            let count =
                rustix::fs::getxattr(&foreign, "trusted.little_clip.fixture", &mut bytes[..])
                    .unwrap();
            assert_eq!(&bytes[..count], b"referent canary");
            let referent_after = std::fs::metadata(&foreign).unwrap();
            assert_eq!(
                (
                    referent_after.uid(),
                    referent_after.gid(),
                    referent_after.mtime(),
                    referent_after.mtime_nsec()
                ),
                (
                    referent_before.uid(),
                    referent_before.gid(),
                    referent_before.mtime(),
                    referent_before.mtime_nsec()
                )
            );
        }
    }
}
