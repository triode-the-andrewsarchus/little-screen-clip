//! Bounded, lossless observations of this process's own mount namespace.
use std::{
    fs::File,
    io::{self, Read, Seek},
    os::fd::AsRawFd,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::Arc,
};
/// One mount observation; callers validate namespace and pinned identities before reuse.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mount {
    pub id: u64,
    /// Device number identifying the observed backing superblock.
    pub device: u64,
    #[serde(with = "crate::path_serde")]
    pub target: PathBuf,
    #[serde(with = "crate::path_serde")]
    pub source: PathBuf,
    pub filesystem: String,
    #[serde(with = "crate::path_serde")]
    pub root: PathBuf,
    pub read_only: bool,
    /// Whether the backing filesystem advertises a read-only superblock.
    /// A read-only bind mount alone does not establish this property.
    pub super_read_only: bool,
}
/// Reads a bounded mount inventory for the current process namespace.
///
/// Paths retain their Unix bytes. These observations do not authorize content access.
///
/// # Errors
/// Returns metadata I/O, malformed input or the two-MiB inventory limit.
pub fn inventory() -> io::Result<Vec<Mount>> {
    parse(&read(&mut crate::kernel_interface::proc_metadata(
        crate::kernel_interface::ProcMetadata::Mounts,
    )?)?)
}
fn read(file: &mut File) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.rewind()?;
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Mount inventory exceeds the bounded metadata limit",
        ));
    }
    Ok(bytes)
}

impl Mount {
    /// Projects a namespace path into this observed filesystem's coordinates.
    ///
    /// This only compares metadata paths; it never authorizes content access.
    pub fn project(&self, path: &Path) -> Option<PathBuf> {
        Some(self.root.join(path.strip_prefix(&self.target).ok()?))
    }
}

/// A bounded mount-table observation from one known namespace.
#[derive(Debug)]
pub struct Snapshot {
    /// Mount records decoded without loss of Unix path bytes.
    pub mounts: Vec<Mount>,
    /// Digest of the namespace and the exact observed mount table.
    pub signature: String,
    /// Namespace in which the table was observed.
    pub namespace: crate::kernel_interface::Namespace,
    /// Boot accompanying mount-lifetime observations.
    pub boot: uuid::Uuid,
}
impl Snapshot {
    /// Captures a fresh observation without retaining any content mount handle.
    ///
    /// # Errors
    /// Returns inventory, namespace or concurrent mount-change failures.
    pub fn capture() -> io::Result<Arc<Self>> {
        Watcher::new()?.observe()
    }
}

/// Watches mount-table events without rescanning the table for every file.
///
/// The retained descriptor is the fixed procfs mount inventory, not a content
/// directory. Changed namespaces and changes during an observation fail closed.
pub struct Watcher {
    file: File,
    namespace: crate::kernel_interface::Namespace,
    boot: uuid::Uuid,
    snapshot: Option<Arc<Snapshot>>,
}
impl Watcher {
    /// Opens the current namespace's non-consuming mount inventory.
    ///
    /// # Errors
    /// Returns unknown namespace context or failure to open procfs metadata.
    ///
    /// # Examples
    /// ```no_run
    /// let mut mounts = little_clip_core::mount_inventory::Watcher::new()?;
    /// let observed = mounts.observe()?;
    /// assert!(observed.namespace.known());
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn new() -> io::Result<Self> {
        let namespace = crate::kernel_interface::Namespace::current();
        if !namespace.known() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unknown mount namespace",
            ));
        }
        Ok(Self {
            file: crate::kernel_interface::proc_metadata(
                crate::kernel_interface::ProcMetadata::Mounts,
            )?,
            namespace,
            boot: crate::kernel_interface::current_boot_id()?,
            snapshot: None,
        })
    }
    fn changed(&self) -> io::Result<bool> {
        let mut poll = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLPRI,
            revents: 0,
        };
        // SAFETY: poll borrows one live descriptor and its initialized fixed-size entry.
        if unsafe { libc::poll(&mut poll, 1, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if poll.revents & libc::POLLNVAL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Mount watcher descriptor is unavailable",
            ));
        }
        Ok(poll.revents & (libc::POLLPRI | libc::POLLERR | libc::POLLHUP) != 0)
    }
    /// Returns the cached observation, refreshing only after a kernel mount event.
    ///
    /// # Errors
    /// Returns namespace changes, bounded-read failures or changes during refresh.
    pub fn observe(&mut self) -> io::Result<Arc<Snapshot>> {
        if crate::kernel_interface::Namespace::current() != self.namespace {
            self.snapshot = None;
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Mount namespace changed",
            ));
        }
        if self.changed()? || self.snapshot.is_none() {
            // Clear before reading: a consumed event followed by an error must
            // never make the older cached observation appear valid again.
            self.snapshot = None;
            let bytes = read(&mut self.file)?;
            if self.changed()? {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Mounts changed during observation",
                ));
            }
            use sha2::{Digest, Sha256};
            let mut digest = Sha256::new();
            digest.update(serde_json::to_vec(&self.namespace)?);
            digest.update(self.boot.as_bytes());
            digest.update(&bytes);
            let mut mounts = parse(&bytes)?;
            mounts.sort_by_key(|mount| mount.id);
            if crate::kernel_interface::Namespace::current() != self.namespace {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Mount namespace changed during observation",
                ));
            }
            self.snapshot = Some(Arc::new(Snapshot {
                mounts,
                namespace: self.namespace.clone(),
                boot: self.boot,
                signature: format!("{:x}", digest.finalize()),
            }));
        }
        self.snapshot
            .clone()
            .ok_or_else(|| io::Error::other("No mount observation"))
    }
}

fn path(bytes: &[u8]) -> PathBuf {
    let mut output = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value = (bytes[i + 1] - b'0') as u16 * 64
                + (bytes[i + 2] - b'0') as u16 * 8
                + (bytes[i + 3] - b'0') as u16;
            if value <= 255 {
                output.push(value as u8);
                i += 4;
                continue;
            }
        }
        output.push(bytes[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(output).into()
}
fn parse(bytes: &[u8]) -> io::Result<Vec<Mount>> {
    let mut result = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let fields = line
            .split(|b| *b == b' ')
            .filter(|f| !f.is_empty())
            .collect::<Vec<_>>();
        let separator = fields.iter().position(|f| *f == b"-").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Malformed mount metadata")
        })?;
        if separator < 6 || fields.len() < separator + 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Incomplete mount metadata",
            ));
        }
        let id = std::str::from_utf8(fields[0])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Unknown mount identity"))?;
        let device = std::str::from_utf8(fields[2])
            .ok()
            .and_then(|value| value.split_once(':'))
            .and_then(|(major, minor)| {
                Some(libc::makedev(major.parse().ok()?, minor.parse().ok()?))
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Unknown mount device"))?;
        result.push(Mount {
            id,
            device,
            target: path(fields[4]),
            source: path(fields[separator + 2]),
            filesystem: String::from_utf8_lossy(fields[separator + 1]).into(),
            root: path(fields[3]),
            read_only: fields[5].split(|b| *b == b',').any(|v| v == b"ro"),
            super_read_only: fields[separator + 3]
                .split(|b| *b == b',')
                .any(|v| v == b"ro"),
        });
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_view_and_backing_filesystem_are_distinct_observations() {
        let mounts = parse(b"11 1 8:1 / /bind ro - vfat /dev/fixture rw\n12 1 8:2 / /exclusive ro - exfat /dev/fixture2 ro\n").unwrap();
        assert!(mounts[0].read_only);
        assert!(!mounts[0].super_read_only);
        assert!(mounts[1].read_only);
        assert!(mounts[1].super_read_only);
    }
    #[test]
    fn octal_and_non_unicode_mount_names_remain_lossless() {
        let data=b"45 1 8:1 / /media/a\\040b_\xff rw - vfat /dev/sdb1 rw\n46 1 0:3 / /tmp/catalog ro - fuse little-clip-catalog:fixture rw\n";
        let mounts = parse(data).unwrap();
        assert_eq!(
            mounts[0].target.as_os_str().as_encoded_bytes(),
            b"/media/a b_\xff"
        );
        assert!(mounts[1].read_only);
        assert_eq!(mounts[1].id, 46);
        assert!(parse(b"broken").is_err());
    }
}
