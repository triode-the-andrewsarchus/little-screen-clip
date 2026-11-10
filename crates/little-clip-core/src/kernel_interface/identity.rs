//! Metadata-only namespace and mount identity observations.
use serde::{Deserialize, Serialize};

/// Namespace inode observations; absent identities do not authorize reuse.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Namespace {
    /// Mount namespace inode.
    pub mount: Option<u64>,
    /// PID namespace inode.
    pub pid: Option<u64>,
    /// Network namespace inode.
    pub network: Option<u64>,
    /// Cgroup namespace inode, absent in older serialized observations.
    #[serde(default)]
    pub cgroup: Option<u64>,
}
impl Namespace {
    /// Returns whether every required namespace was observed.
    pub fn known(&self) -> bool {
        self.mount.is_some()
            && self.pid.is_some()
            && self.network.is_some()
            && self.cgroup.is_some()
    }
    /// Observes the current process namespace handles through metadata only.
    ///
    /// Failed lookups remain `None`; callers must not substitute a prior identity.
    pub fn current() -> Self {
        use std::os::unix::fs::MetadataExt;
        let inode = |name: &str| {
            std::fs::metadata(format!("/proc/self/ns/{name}"))
                .ok()
                .map(|m| m.ino())
        };
        Self {
            mount: inode("mnt"),
            pid: inode("pid"),
            network: inode("net"),
            cgroup: inode("cgroup"),
        }
    }
}
/// Object and mount identity from one pinned metadata handle, without payload I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MountIdentity {
    /// Filesystem device number from the pinned object.
    pub device: u64,
    /// Object inode on the observed filesystem.
    pub inode: u64,
    /// Kernel mount ID, which may be reused after unmounting.
    pub mount: u64,
}
impl MountIdentity {
    /// Pins a directory and observes its object and mount identity.
    ///
    /// # Errors
    /// Returns an error if pinning or metadata observation fails, the object is
    /// not a directory, or the kernel does not supply a mount ID.
    pub fn directory(path: &std::path::Path) -> std::io::Result<Self> {
        let file = super::pin(path, true)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Not a directory hierarchy object",
            ));
        }
        Self::pinned(&file)
    }
    /// Observes identity from a retained file handle without reading payloads.
    ///
    /// # Errors
    /// Returns metadata or `statx` failures, including an unavailable mount ID.
    pub fn pinned(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
        let metadata = file.metadata()?;
        let mut stats = std::mem::MaybeUninit::<libc::statx>::uninit();
        // SAFETY: the live O_PATH descriptor is queried using an empty C string;
        // stats points to writable storage for the complete statx structure.
        if unsafe {
            libc::statx(
                file.as_raw_fd(),
                c"".as_ptr(),
                libc::AT_EMPTY_PATH,
                libc::STATX_MNT_ID,
                stats.as_mut_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful statx initialized the structure.
        let stats = unsafe { stats.assume_init() };
        if stats.stx_mask & libc::STATX_MNT_ID == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Mount identity not supplied by the kernel",
            ));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            mount: stats.stx_mnt_id,
        })
    }
    /// Returns whether two distinct objects belong to the same observed mount.
    ///
    /// A same-object alias is excluded. This is not a persistent lifetime check:
    /// callers retaining observations must also validate namespace and lifetime.
    pub fn same_hierarchy_mount(self, other: Self) -> bool {
        self.device == other.device && self.mount == other.mount && self.inode != other.inode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hierarchy_mount_identity_excludes_bind_boundaries_and_same_object_aliases() {
        let root = MountIdentity {
            device: 7,
            inode: 10,
            mount: 5,
        };
        assert!(root.same_hierarchy_mount(MountIdentity { inode: 11, ..root }));
        assert!(!root.same_hierarchy_mount(MountIdentity {
            inode: 11,
            mount: 6,
            ..root
        }));
        assert!(!root.same_hierarchy_mount(root));
        assert!(!root.same_hierarchy_mount(MountIdentity {
            inode: 11,
            device: 8,
            ..root
        }));
    }
    #[test]
    fn pinned_directory_mount_identity_tracks_same_mount_children_and_rejects_files() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let root = MountIdentity::directory(temp.path()).unwrap();
        assert!(root.same_hierarchy_mount(MountIdentity::directory(&child).unwrap()));
        let file = temp.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        assert_eq!(
            MountIdentity::directory(&file).unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
    }
}
