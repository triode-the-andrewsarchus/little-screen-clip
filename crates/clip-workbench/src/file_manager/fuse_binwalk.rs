//! FUSE-based binary exploration using `fuse-binwalk`.
//!
//! Mounts binary images (firmware, ELF, ROM, archives) as navigable, optionally writable
//! virtual filesystems under `$XDG_RUNTIME_DIR/fuse-binwalk/<binary_name>-<hash>/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use sha2::{Digest, Sha256};

static CACHED_MOUNTS: std::sync::OnceLock<std::sync::RwLock<MountTable>> =
    std::sync::OnceLock::new();

/// Returns the in-memory cached [`MountTable`] without performing any syscalls.
pub fn cached_mount_table() -> MountTable {
    let lock = CACHED_MOUNTS.get_or_init(|| std::sync::RwLock::new(MountTable::from_proc_mounts()));
    lock.read().map(|guard| guard.clone()).unwrap_or_default()
}

/// Refreshes the in-memory cached [`MountTable`] from `/proc/mounts`.
pub fn refresh_cached_mount_table() -> MountTable {
    let fresh = MountTable::from_proc_mounts();
    let lock = CACHED_MOUNTS.get_or_init(|| std::sync::RwLock::new(fresh.clone()));
    if let Ok(mut guard) = lock.write() {
        *guard = fresh.clone();
    }
    fresh
}

/// Access mode of an active `fuse-binwalk` mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountMode {
    ReadOnly,
    ReadWrite,
}

/// Cached snapshot of active `fuse-binwalk` mounts from `/proc/mounts`.
///
/// Kept in memory and refreshed only on folder navigation, refresh, or mount/unmount
/// completion so UI context menus never perform per-frame `/proc/mounts` reads or
/// blocking `stat` calls inside FUSE mounts.
#[derive(Clone, Debug, Default)]
pub struct MountTable {
    mounts: Arc<BTreeMap<PathBuf, MountMode>>,
}

impl MountTable {
    /// Reads `/proc/mounts` once and indexes all active `fuse-binwalk` mount points.
    pub fn from_proc_mounts() -> Self {
        let content = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
        Self::parse_proc_mounts(&content)
    }

    pub(crate) fn parse_proc_mounts(content: &str) -> Self {
        let mut mounts = BTreeMap::new();
        for line in content.lines() {
            let mut parts = line.split_whitespace();
            let (Some(fsname), Some(raw_target), Some(fstype), Some(options)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if fsname == "fuse-binwalk" || fstype == "fuse.binwalk" || fsname.contains("binwalk") {
                let target = PathBuf::from(unescape_proc_mount_field(raw_target));
                let mode = if options.split(',').any(|opt| opt == "rw") {
                    MountMode::ReadWrite
                } else {
                    MountMode::ReadOnly
                };
                mounts.insert(target, mode);
            }
        }
        Self {
            mounts: Arc::new(mounts),
        }
    }

    /// Returns the [`MountMode`] if `mount_point` is currently mounted.
    pub fn status(&self, mount_point: &Path) -> Option<MountMode> {
        self.mounts.get(mount_point).copied()
    }

    /// Returns the enclosing `fuse-binwalk` mount point if `path` is equal to or inside one.
    pub fn enclosing_mount_point(&self, path: &Path) -> Option<PathBuf> {
        self.mounts
            .keys()
            .find(|mount_root| path.starts_with(mount_root.as_path()))
            .cloned()
    }
}

/// Decodes octal escape sequences (`\040` for space, `\011` for tab, `\134` for backslash)
/// used in `/proc/mounts` target paths.
fn unescape_proc_mount_field(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && (b'0'..=b'7').contains(&bytes[i + 1])
            && (b'0'..=b'7').contains(&bytes[i + 2])
            && (b'0'..=b'7').contains(&bytes[i + 3])
        {
            let val =
                ((bytes[i + 1] - b'0') << 6) | ((bytes[i + 2] - b'0') << 3) | (bytes[i + 3] - b'0');
            out.push(val);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Returns the context menu label for mounting a binary based on whether Shift is held.
pub fn mount_menu_label(shift_held: bool) -> &'static str {
    if shift_held {
        "Mount to explore + write binary contents (Unsafe)"
    } else {
        "Mount to explore binary contents"
    }
}

/// Returns the context menu label for upgrading a read-only mount to writable when Shift is held.
pub fn remount_writable_menu_label() -> &'static str {
    "Remount to explore + write binary contents (Unsafe)"
}

/// Returns `true` if `entry` is a regular non-system file eligible for binary mounting
/// (excluding known plain-text and source-code files).
pub(super) fn is_mountable_binary(entry: &super::filesystem::Entry) -> bool {
    entry.regular_file && entry.system.is_none() && !super::text_strip::known_text(&entry.path)
}

/// Resolves a collision-free mount point directory for `binary_path` under
/// `$XDG_RUNTIME_DIR/fuse-binwalk/<file_name>-<short_hash>/`.
/// Falls back to `/tmp/fuse-binwalk-$USER/<file_name>-<short_hash>/` if `$XDG_RUNTIME_DIR` is unset.
pub fn mount_point_for_binary(binary_path: &Path) -> PathBuf {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("fuse-binwalk-{user}"))
        });
    let file_name = binary_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "binary".into());
    let digest = Sha256::digest(binary_path.as_os_str().as_encoded_bytes());
    let short_hash = format!(
        "{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    );
    runtime_dir
        .join("fuse-binwalk")
        .join(format!("{file_name}-{short_hash}"))
}

/// Checks whether the given mount point directory has an active FUSE mount in `/proc/mounts`.
pub fn is_fuse_binwalk_mounted(mount_point: &Path) -> bool {
    let table = MountTable::from_proc_mounts();
    if table.status(mount_point).is_some() {
        return true;
    }
    if let Ok(canonical) = mount_point.canonicalize() {
        return table.status(&canonical).is_some();
    }
    false
}

/// Unmounts an active FUSE mount at `mount_point` using `fusermount3` / `fusermount`.
pub fn unmount_fuse_binwalk(mount_point: &Path) -> Result<(), String> {
    let status = std::process::Command::new("fusermount3")
        .args(["-u", "-z"])
        .arg(mount_point)
        .status()
        .or_else(|_| {
            std::process::Command::new("fusermount")
                .args(["-u", "-z"])
                .arg(mount_point)
                .status()
        })
        .map_err(|e| format!("Failed to run fusermount: {e}"))?;

    if status.success() {
        let _ = std::fs::remove_dir(mount_point);
        Ok(())
    } else {
        Err(format!(
            "fusermount failed with exit code: {:?}",
            status.code()
        ))
    }
}

/// Mounts the specified binary file at its designated runtime mount point using the
/// built-in `--internal-fuse-binwalk-daemon` worker.
pub fn mount_fuse_binwalk(binary_path: &Path, writable: bool) -> Result<PathBuf, String> {
    if !binary_path.is_file() {
        return Err(format!("'{}' is not a regular file", binary_path.display()));
    }

    let mount_point = mount_point_for_binary(binary_path);
    if is_fuse_binwalk_mounted(&mount_point) {
        let _ = unmount_fuse_binwalk(&mount_point);
    }

    std::fs::create_dir_all(&mount_point).map_err(|e| {
        format!(
            "Failed to create mount directory '{}': {e}",
            mount_point.display()
        )
    })?;

    let current_exe =
        std::env::current_exe().map_err(|e| format!("Failed to locate current executable: {e}"))?;
    let mut cmd = std::process::Command::new(current_exe);
    cmd.arg("--internal-fuse-binwalk-daemon");
    if writable {
        cmd.arg("-w");
    }
    cmd.arg(binary_path);
    cmd.arg(&mount_point);

    let output = cmd
        .output()
        .map_err(|e| format!("Failed to execute mount worker: {e}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_dir(&mount_point);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            return Err(stderr);
        }
        return Err(format!(
            "Mount worker exited with code {:?}",
            output.status.code()
        ));
    }

    // Poll `/proc/mounts` until the daemon's `spawn_mount2` registers the mount.
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if is_fuse_binwalk_mounted(&mount_point) {
            return Ok(mount_point);
        }
        std::thread::sleep(Duration::from_millis(30));
    }

    let _ = std::fs::remove_dir(&mount_point);
    Err("Timed out waiting for FUSE mount to become ready (check /dev/fuse permissions)".into())
}

/// Runs the internal daemon for `fuse_binwalk` mounting (invoked by `--internal-fuse-binwalk-daemon`).
pub fn run_fuse_binwalk_daemon(
    binary_path: &Path,
    mount_point: &Path,
    writable: bool,
) -> Result<(), String> {
    fuse_binwalk::run_mount_daemon(binary_path, mount_point, writable).map_err(|e| format!("{e:#}"))
}

/// Completion outcome produced by [`MountManager::tick`].
pub(super) enum MountOutcome {
    Mounted {
        binary_path: PathBuf,
        mount_point: PathBuf,
        writable: bool,
    },
    Unmounted {
        mount_point: PathBuf,
    },
    Failed(String),
}

enum PendingKind {
    Mount {
        binary_path: PathBuf,
        writable: bool,
    },
    Unmount {
        mount_point: PathBuf,
    },
}

struct PendingMountJob {
    kind: PendingKind,
    _ticket: JobTicket,
    receiver: mpsc::Receiver<Result<Result<PathBuf, String>, JobError>>,
}

/// Manages asynchronous mount/unmount jobs for [`super::FileManager`].
#[derive(Default)]
pub(super) struct MountManager {
    pending: Option<PendingMountJob>,
}

impl MountManager {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn status_text(&self) -> Option<String> {
        let pending = self.pending.as_ref()?;
        match &pending.kind {
            PendingKind::Mount {
                binary_path,
                writable,
            } => {
                let name = binary_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                Some(if *writable {
                    format!("Mounting '{name}' (writable)…")
                } else {
                    format!("Mounting '{name}'…")
                })
            }
            PendingKind::Unmount { mount_point } => {
                let name = mount_point
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                Some(format!("Unmounting '{name}'…"))
            }
        }
    }

    pub fn request_mount(&mut self, binary_path: PathBuf, writable: bool, jobs: &BackgroundJobs) {
        if self.pending.is_some() {
            return;
        }
        let target = binary_path.clone();
        let (ticket, receiver) = jobs.submit(Duration::from_secs(120), move || {
            mount_fuse_binwalk(&target, writable)
        });
        self.pending = Some(PendingMountJob {
            kind: PendingKind::Mount {
                binary_path,
                writable,
            },
            _ticket: ticket,
            receiver,
        });
    }

    pub fn request_unmount(&mut self, mount_point: PathBuf, jobs: &BackgroundJobs) {
        if self.pending.is_some() {
            return;
        }
        let target = mount_point.clone();
        let (ticket, receiver) = jobs.submit(Duration::from_secs(30), move || {
            unmount_fuse_binwalk(&target).map(|()| target)
        });
        self.pending = Some(PendingMountJob {
            kind: PendingKind::Unmount { mount_point },
            _ticket: ticket,
            receiver,
        });
    }

    pub fn tick(&mut self) -> Option<MountOutcome> {
        let pending = self.pending.as_ref()?;
        let result = super::poll_job(&pending.receiver)?;
        let finished = self.pending.take().expect("checked Some");
        refresh_cached_mount_table();
        match (finished.kind, result) {
            (
                PendingKind::Mount {
                    binary_path,
                    writable,
                },
                Ok(mount_point),
            ) => Some(MountOutcome::Mounted {
                binary_path,
                mount_point,
                writable,
            }),
            (PendingKind::Unmount { .. }, Ok(mount_point)) => {
                Some(MountOutcome::Unmounted { mount_point })
            }
            (PendingKind::Mount { .. }, Err(err)) => {
                Some(MountOutcome::Failed(format!("Mount failed: {err}")))
            }
            (PendingKind::Unmount { .. }, Err(err)) => {
                Some(MountOutcome::Failed(format!("Unmount failed: {err}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mount_menu_label_respects_shift() {
        assert_eq!(mount_menu_label(false), "Mount to explore binary contents");
        assert_eq!(
            mount_menu_label(true),
            "Mount to explore + write binary contents (Unsafe)"
        );
        assert_eq!(
            remount_writable_menu_label(),
            "Remount to explore + write binary contents (Unsafe)"
        );
    }

    #[test]
    fn test_mount_point_for_binary_disambiguates_paths() {
        let p1 = Path::new("/var/firmware_a/u-boot.bin");
        let p2 = Path::new("/var/firmware_b/u-boot.bin");
        let mp1 = mount_point_for_binary(p1);
        let mp2 = mount_point_for_binary(p2);
        assert_ne!(mp1, mp2);
        let name1 = mp1.file_name().unwrap().to_str().unwrap();
        assert!(name1.starts_with("u-boot.bin-"));
        assert_eq!(
            mp1.parent().unwrap().file_name().unwrap().to_str().unwrap(),
            "fuse-binwalk"
        );
    }

    #[test]
    fn test_parse_proc_mounts_unescapes_and_detects_modes() {
        let sample = "\
fuse-binwalk /run/user/1000/fuse-binwalk/fw\\040image.bin-12345678 fuse.binwalk ro,nosuid,nodev,relatime,user_id=1000,group_id=1000 0 0
fuse-binwalk /run/user/1000/fuse-binwalk/rw.bin-abcdef01 fuse.binwalk rw,nosuid,nodev,relatime,user_id=1000,group_id=1000 0 0
tmpfs /run/user/1000 tmpfs rw,nosuid,nodev,relatime 0 0
";
        let table = MountTable::parse_proc_mounts(sample);
        assert_eq!(
            table.status(Path::new(
                "/run/user/1000/fuse-binwalk/fw image.bin-12345678"
            )),
            Some(MountMode::ReadOnly)
        );
        assert_eq!(
            table.status(Path::new("/run/user/1000/fuse-binwalk/rw.bin-abcdef01")),
            Some(MountMode::ReadWrite)
        );
        assert_eq!(
            table.enclosing_mount_point(Path::new(
                "/run/user/1000/fuse-binwalk/rw.bin-abcdef01/raw/0x0000_png.png"
            )),
            Some(PathBuf::from("/run/user/1000/fuse-binwalk/rw.bin-abcdef01"))
        );
    }

    #[test]
    fn test_is_fuse_binwalk_mounted_on_nonexistent_dir() {
        let nonexistent = Path::new("/tmp/this_should_never_exist_12345/nonexistent");
        assert!(!is_fuse_binwalk_mounted(nonexistent));
    }
}
