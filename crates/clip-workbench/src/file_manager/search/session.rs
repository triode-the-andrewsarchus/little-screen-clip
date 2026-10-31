//! Mounted-source continuity, observed only in disposable source-reader helpers.
use super::policy::Root;
use crate::file_manager::system::Namespace;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, os::fd::AsRawFd};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SourceSession {
    boot: uuid::Uuid,
    namespace: u64,
    mount: u64,
    unique_mount: Option<u64>,
    filesystem: i64,
}
impl SourceSession {
    pub fn capture(root: &Root) -> Option<Self> {
        Some(Self {
            boot: boot_id()?,
            namespace: Namespace::current().mount?,
            mount: root.mount,
            unique_mount: unique_mount(&root.directory),
            filesystem: filesystem_type(&root.directory)?,
        })
    }
    pub fn unique(&self) -> bool {
        self.unique_mount.is_some()
    }
    pub fn unique_on(&self, mount: Option<u64>) -> bool {
        self.unique() && mount == Some(self.mount)
    }
    /// Network/unknown filesystems may change server sessions without a new mount.
    pub fn persistent(&self) -> bool {
        self.unique()
            && matches!(
                self.filesystem,
                0x9123683e | 0xef53 | 0x58465342 | 0x01021994
            )
    }
}
fn filesystem_type(file: &File) -> Option<i64> {
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file is live and fstatfs initializes the complete output on success.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: successful fstatfs initialized stats.
    Some(unsafe { stats.assume_init() }.f_type)
}
fn boot_id() -> Option<uuid::Uuid> {
    use std::os::unix::fs::OpenOptionsExt;
    // A fixed, documented, non-consuming procfs identity getter. This is never
    // selected by a content pathname, and runs under the reader's process budget.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open("/proc/sys/kernel/random/boot_id")
        .ok()?;
    if filesystem_type(&file)? != libc::PROC_SUPER_MAGIC {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(38).read_to_end(&mut bytes).ok()?;
    uuid::Uuid::parse_str(std::str::from_utf8(&bytes).ok()?.trim()).ok()
}
fn unique_mount(file: &File) -> Option<u64> {
    // Linux UAPI linux/stat.h: STATX_MNT_ID_UNIQUE. Older kernels may omit it;
    // a reusable mount ID must never be mistaken for this lifetime identifier.
    const UNIQUE: u32 = 0x00004000;
    let mut stats = std::mem::MaybeUninit::<libc::statx>::uninit();
    // SAFETY: the descriptor is live, the empty path is NUL terminated and stats
    // points to writable storage for the full kernel ABI structure.
    if unsafe {
        libc::statx(
            file.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH | libc::AT_STATX_DONT_SYNC,
            UNIQUE,
            stats.as_mut_ptr(),
        )
    } != 0
    {
        return None;
    }
    // SAFETY: successful statx initialized stats.
    let stats = unsafe { stats.assume_init() };
    (stats.stx_mask & UNIQUE != 0 && stats.stx_mnt_id != 0).then_some(stats.stx_mnt_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_continuity_requires_a_known_local_filesystem_and_unique_mount() {
        let source = tempfile::tempdir().unwrap();
        let root = Root::open(source.path()).unwrap();
        let session = SourceSession::capture(&root).unwrap();
        assert_eq!(Some(session.clone()), SourceSession::capture(&root));
        let mut uncertain = session.clone();
        uncertain.unique_mount = None;
        assert!(!uncertain.persistent());
        for filesystem in [0x6969, 0xff534d42, 0x12345678] {
            uncertain = session.clone();
            uncertain.filesystem = filesystem;
            assert!(!uncertain.persistent());
        }
    }
}
