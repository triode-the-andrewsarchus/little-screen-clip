//! Discovers, monitors, and manages filesystem mounts, removable drives,
//! fuse-binwalk mounts, and unmounted block devices.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use little_clip_core::process::{CommandError, CommandRequest};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};

use crate::icons::Icon;

/// Specific category of a mount or block device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MountKind {
    /// FUSE mount from `fuse-binwalk`.
    FuseBinwalk,
    /// Removable media (USB flash drive, external drive, SD card).
    Removable,
    /// Disc image (mounted ISO or optical disc).
    DiscImage,
    /// Fixed internal, secondary, or network drive partition.
    Drive,
}

impl MountKind {
    pub fn icon(&self) -> Icon {
        match self {
            Self::FuseBinwalk => Icon::BinaryDiff,
            Self::Removable => Icon::Usb,
            Self::DiscImage => Icon::Tape,
            Self::Drive => Icon::Drive,
        }
    }
}

/// A discovered mounted or unmounted storage target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MountEntry {
    /// Unique identifier for egui IDs and deduplication.
    pub id: String,
    /// Human-facing display name.
    pub label: String,
    /// Subtitle or detailed info (e.g. "ext4 · 465.8 GB", "vfat · unmounted").
    pub detail: String,
    /// Mountpoint path if currently mounted.
    pub mount_point: Option<PathBuf>,
    /// Underlying block device path if backed by one (e.g. `/dev/sdb1`).
    pub device_path: Option<PathBuf>,
    /// Filesystem type (e.g. "ext4", "vfat", "fuse.fuse-binwalk", "iso9660").
    pub filesystem: Option<String>,
    /// Whether this storage volume is currently mounted.
    pub is_mounted: bool,
    /// Whether this volume is removable or ejectable (flash drives, fuse-binwalk, ISOs, etc.).
    pub is_removable: bool,
    /// Classification of this mount.
    pub kind: MountKind,
}

/// Outcome emitted by [`Mounts::tick`] when an asynchronous operation finishes.
pub(crate) enum MountEvent {
    Mounted(PathBuf),
    Unmounted(PathBuf),
    Error(String),
}

enum PendingAction {
    Mount,
    Unmount,
}

struct PendingJob {
    action: PendingAction,
    _ticket: JobTicket,
    receiver: mpsc::Receiver<Result<Result<PathBuf, String>, JobError>>,
}

/// Manages mount discovery, unmounted devices, and asynchronous mount/eject actions.
pub(crate) struct Mounts {
    pub entries: Vec<MountEntry>,
    pending: Option<PendingJob>,
    last_scan: Option<Instant>,
    scan_ticket: Option<JobTicket>,
    scan_receiver: Option<mpsc::Receiver<Result<Vec<MountEntry>, JobError>>>,
    pub status: Option<String>,
}

impl Default for Mounts {
    fn default() -> Self {
        Self::new()
    }
}

impl Mounts {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            pending: None,
            last_scan: None,
            scan_ticket: None,
            scan_receiver: None,
            status: None,
        }
    }

    /// Whether an explicit mount or unmount user action is currently in progress.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Triggers an asynchronous scan of mounts and unmounted devices.
    pub fn request_scan(&mut self, jobs: &BackgroundJobs) {
        // Explicit refreshes retire the previous observation, including results
        // already queued before a mount/unmount completed.
        if let Some(ticket) = self.scan_ticket.take() {
            ticket.cancel();
        }
        let (ticket, receiver) = jobs.submit(Duration::from_secs(10), scan_mounts_sync);
        self.scan_ticket = Some(ticket);
        self.scan_receiver = Some(receiver);
        self.last_scan = Some(Instant::now());
    }

    /// Requests mounting an unmounted block device.
    pub fn request_mount(&mut self, entry: &MountEntry, jobs: &BackgroundJobs) {
        if self.pending.is_some() {
            return;
        }
        let Some(device_path) = entry.device_path.clone() else {
            return;
        };
        let label = entry.label.clone();
        self.status = Some(format!("Mounting '{label}'…"));
        let target = device_path.clone();
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || mount_block_device(&target));
        self.pending = Some(PendingJob {
            action: PendingAction::Mount,
            _ticket: ticket,
            receiver,
        });
    }

    /// Requests unmounting or ejecting a mounted volume.
    pub fn request_unmount(&mut self, entry: &MountEntry, jobs: &BackgroundJobs) {
        if self.pending.is_some() {
            return;
        }
        let Some(mount_point) = entry.mount_point.clone() else {
            return;
        };
        let device_path = entry.device_path.clone();
        let filesystem = entry.filesystem.clone();
        let kind = entry.kind;
        let label = entry.label.clone();
        self.status = Some(format!("Unmounting '{label}'…"));
        let mp = mount_point.clone();
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            unmount_target(&mp, device_path.as_deref(), filesystem.as_deref(), kind).map(|()| mp)
        });
        self.pending = Some(PendingJob {
            action: PendingAction::Unmount,
            _ticket: ticket,
            receiver,
        });
    }

    /// Polls asynchronous jobs and triggers periodic background scans.
    pub fn tick(&mut self, jobs: &BackgroundJobs) -> Option<MountEvent> {
        // Poll background scan
        if let Some(rx) = &self.scan_receiver {
            match rx.try_recv() {
                Ok(Ok(entries)) => {
                    if self.entries != entries {
                        self.entries = entries;
                    }
                    self.scan_receiver = None;
                    self.scan_ticket = None;
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan_receiver = None;
                    self.scan_ticket = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // Periodic background scan every 3 seconds if idle
        if self.scan_receiver.is_none()
            && self
                .last_scan
                .is_none_or(|scan| scan.elapsed() >= Duration::from_secs(3))
        {
            self.request_scan(jobs);
        }

        // Poll pending mount/unmount job
        let pending = self.pending.as_ref()?;
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };

        let action = self.pending.take().map(|p| p.action);
        self.status = None;
        self.request_scan(jobs);

        match result {
            Ok(Ok(mount_point)) => match action {
                Some(PendingAction::Mount) => Some(MountEvent::Mounted(mount_point)),
                Some(PendingAction::Unmount) => Some(MountEvent::Unmounted(mount_point)),
                None => None,
            },
            Ok(Err(err)) => Some(MountEvent::Error(err)),
            Err(JobError::QueueFull) => Some(MountEvent::Error("Mount worker queue full".into())),
            Err(JobError::TimedOut) => Some(MountEvent::Error("Mount operation timed out".into())),
            Err(JobError::Cancelled) => None,
        }
    }
}

/// Unescapes octal sequences (e.g. `\040` for space) from `/proc/mounts`.
pub(crate) fn unescape_octal(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Ok(octal) = std::str::from_utf8(&bytes[i + 1..i + 4])
            && let Ok(b) = u8::from_str_radix(octal, 8)
        {
            out.push(b);
            i += 4;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Scans mounts from `/proc/mounts` and block devices from `lsblk`.
pub(crate) fn scan_mounts_sync() -> Vec<MountEntry> {
    let proc_mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let lsblk_json = CommandRequest::new("lsblk")
        .args([
            "--list",
            "--json",
            "-o",
            "NAME,PATH,PKNAME,LABEL,PARTLABEL,FSTYPE,MOUNTPOINT,SIZE,RM,HOTPLUG,MODEL,TYPE",
        ])
        .timeout(Duration::from_secs(5))
        .max_output_bytes(2 * 1024 * 1024)
        .own_process_group()
        .run()
        .ok()
        .and_then(|output| {
            if output.status.success() && !output.stdout_truncated {
                String::from_utf8(output.stdout).ok()
            } else {
                None
            }
        });

    parse_mounts(&proc_mounts, lsblk_json.as_deref())
}

/// Core parsing function that filters out snaps, pseudo-filesystems, and system roots,
/// and returns the combined list of active mounts and unmounted devices.
pub(crate) fn parse_mounts(proc_mounts: &str, lsblk_json: Option<&str>) -> Vec<MountEntry> {
    let mut entries: Vec<MountEntry> = Vec::new();
    let mut seen_targets: BTreeSet<PathBuf> = BTreeSet::new();

    // 1. Parse active mounts from /proc/mounts
    for line in proc_mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let raw_source = parts[0];
        let raw_target = parts[1];
        let fstype = parts[2];

        let target_str = unescape_octal(raw_target);
        let target = PathBuf::from(&target_str);
        let source_str = unescape_octal(raw_source);
        let source = PathBuf::from(&source_str);

        // Strict snap filtering: never include any snap mounts
        if is_snap_mount(&target, &source) {
            continue;
        }

        // Filter out pseudo/virtual filesystems
        if is_pseudo_filesystem(fstype) {
            continue;
        }

        // Filter out core system roots
        if is_system_root(&target) {
            continue;
        }

        // Filter out desktop daemon internal endpoints (doc, gvfs)
        if is_desktop_internal_mount(&target) {
            continue;
        }

        // Identify fuse-binwalk mounts
        let is_binwalk = super::fuse_binwalk::is_binwalk_mount(&source_str, fstype);

        if is_binwalk {
            let label = target
                .file_name()
                .map(|n| {
                    let s = n.to_string_lossy();
                    if let Some((base, _)) = s.rsplit_once('-') {
                        format!("{base} (binwalk)")
                    } else {
                        format!("{s} (binwalk)")
                    }
                })
                .unwrap_or_else(|| "Binary (binwalk)".into());

            entries.push(MountEntry {
                id: format!("binwalk:{}", target.display()),
                label,
                detail: "FUSE binwalk".into(),
                mount_point: Some(target.clone()),
                device_path: None,
                filesystem: Some(fstype.to_owned()),
                is_mounted: true,
                is_removable: true,
                kind: MountKind::FuseBinwalk,
            });
            seen_targets.insert(target);
            continue;
        }

        // Identify Disc images (ISOs, optical discs)
        if fstype == "iso9660" || fstype == "udf" {
            let label = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Disc image".into());
            entries.push(MountEntry {
                id: format!("disc:{}", target.display()),
                label,
                detail: format!("{fstype} · Disc image"),
                mount_point: Some(target.clone()),
                device_path: if source_str.starts_with("/dev/") {
                    Some(source.clone())
                } else {
                    None
                },
                filesystem: Some(fstype.to_owned()),
                is_mounted: true,
                is_removable: true,
                kind: MountKind::DiscImage,
            });
            seen_targets.insert(target);
            continue;
        }

        // Identify removable media (/media/..., /run/media/...)
        let is_media_path = target_str.starts_with("/media/")
            || target_str.starts_with("/run/media/")
            || target_str == "/media"
            || target_str == "/run/media";

        if is_media_path {
            let label = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Removable drive".into());
            entries.push(MountEntry {
                id: format!("removable:{}", target.display()),
                label,
                detail: fstype.to_owned(),
                mount_point: Some(target.clone()),
                device_path: if source_str.starts_with("/dev/") {
                    Some(source.clone())
                } else {
                    None
                },
                filesystem: Some(fstype.to_owned()),
                is_mounted: true,
                is_removable: true,
                kind: MountKind::Removable,
            });
            seen_targets.insert(target);
            continue;
        }

        // Other mounted non-system locations (e.g. /mnt/projects, /mnt/Temp, network shares)
        if target_str.starts_with("/mnt")
            || source_str.starts_with("//")
            || fstype.starts_with("fuse.")
        {
            let label = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| target_str.clone());
            let is_removable = source_str.starts_with("//");
            entries.push(MountEntry {
                id: format!("drive:{}", target.display()),
                label,
                detail: fstype.to_owned(),
                mount_point: Some(target.clone()),
                device_path: if source_str.starts_with("/dev/") {
                    Some(source.clone())
                } else {
                    None
                },
                filesystem: Some(fstype.to_owned()),
                is_mounted: true,
                is_removable,
                kind: MountKind::Drive,
            });
            seen_targets.insert(target);
        }
    }

    // 2. Correlate and add devices from `lsblk` JSON
    if let Some(json_str) = lsblk_json
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str)
        && let Some(devices) = val.get("blockdevices").and_then(|v| v.as_array())
    {
        // Use the reported relationship; device names are not a parent schema.
        let partitioned_disks: BTreeSet<&str> = devices
            .iter()
            .filter(|dev| dev.get("type").and_then(|value| value.as_str()) == Some("part"))
            .filter_map(|dev| dev.get("pkname").and_then(|value| value.as_str()))
            .filter(|parent| !parent.is_empty())
            .collect();

        for dev in devices {
            let name = dev.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let path = dev.get("path").and_then(|v| v.as_str()).unwrap_or_default();
            let fstype = dev.get("fstype").and_then(|v| v.as_str());
            let label = dev
                .get("label")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let partlabel = dev
                .get("partlabel")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let model = dev
                .get("model")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());
            let size = dev.get("size").and_then(|v| v.as_str()).unwrap_or_default();
            let is_rm = dev.get("rm").and_then(|v| v.as_bool()).unwrap_or(false);
            let is_hotplug = dev
                .get("hotplug")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let dev_type = dev.get("type").and_then(|v| v.as_str()).unwrap_or_default();
            let mountpoint = dev
                .get("mountpoint")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty());

            // Skip snaps
            if let Some(mp) = mountpoint
                && (mp.starts_with("/snap") || mp.starts_with("/run/snapd"))
            {
                continue;
            }
            if name.starts_with("loop") {
                continue;
            }
            if fstype == Some("swap") {
                continue;
            }

            // Skip whole-disk entries that have child partitions
            if dev_type == "disk" && partitioned_disks.contains(name) {
                continue;
            }

            // Skip root and boot
            if let Some(mp) = mountpoint
                && (mp == "/" || mp == "/boot" || mp.starts_with("/boot/"))
            {
                continue;
            }

            if let Some(mp) = mountpoint {
                let mp_path = PathBuf::from(mp);
                // If already discovered from /proc/mounts, enrich label and metadata
                if let Some(existing) = entries
                    .iter_mut()
                    .find(|e| e.mount_point.as_deref() == Some(&mp_path))
                {
                    if let Some(lbl) = label {
                        existing.label = lbl.to_owned();
                    }
                    if !size.is_empty() {
                        if let Some(fs) = &existing.filesystem {
                            existing.detail = format!("{fs} · {size}");
                        } else {
                            existing.detail = size.to_owned();
                        }
                    }
                    if is_rm || is_hotplug {
                        existing.is_removable = true;
                        existing.kind = MountKind::Removable;
                    }
                    if existing.device_path.is_none() && !path.is_empty() {
                        existing.device_path = Some(PathBuf::from(path));
                    }
                } else if !seen_targets.contains(&mp_path) {
                    let item_label = label.or(partlabel).map(str::to_owned).unwrap_or_else(|| {
                        mp_path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| mp.to_owned())
                    });
                    let detail = match (fstype, size.is_empty()) {
                        (Some(fs), false) => format!("{fs} · {size}"),
                        (Some(fs), true) => fs.to_owned(),
                        (None, false) => size.to_owned(),
                        (None, true) => String::new(),
                    };
                    let kind = if is_rm || is_hotplug {
                        MountKind::Removable
                    } else {
                        MountKind::Drive
                    };
                    entries.push(MountEntry {
                        id: format!("blk:{}", path),
                        label: item_label,
                        detail,
                        mount_point: Some(mp_path.clone()),
                        device_path: Some(PathBuf::from(path)),
                        filesystem: fstype.map(str::to_owned),
                        is_mounted: true,
                        is_removable: is_rm
                            || is_hotplug
                            || mp.starts_with("/media")
                            || mp.starts_with("/run/media"),
                        kind,
                    });
                    seen_targets.insert(mp_path);
                }
            } else {
                // Unmounted device or partition with a filesystem or removable status
                let has_fs = fstype.is_some();
                let is_removable_device = is_rm || is_hotplug;

                if has_fs || is_removable_device {
                    let item_label = label
                        .or(partlabel)
                        .map(str::to_owned)
                        .or_else(|| {
                            model.map(|m| {
                                if !size.is_empty() {
                                    format!("{m} ({size})")
                                } else {
                                    m.to_owned()
                                }
                            })
                        })
                        .unwrap_or_else(|| {
                            if !size.is_empty() {
                                format!("Drive {name} ({size})")
                            } else {
                                format!("Drive {name}")
                            }
                        });

                    let detail = match fstype {
                        Some(fs) if !size.is_empty() => {
                            format!("{fs} · {size} (unmounted)")
                        }
                        Some(fs) => format!("{fs} (unmounted)"),
                        None => "Unmounted".into(),
                    };

                    let kind = if is_removable_device {
                        MountKind::Removable
                    } else {
                        MountKind::Drive
                    };

                    entries.push(MountEntry {
                        id: format!("unmounted:{}", path),
                        label: item_label,
                        detail,
                        mount_point: None,
                        device_path: Some(PathBuf::from(path)),
                        filesystem: fstype.map(str::to_owned),
                        is_mounted: false,
                        is_removable: true,
                        kind,
                    });
                }
            }
        }
    }

    // Sort: Removable media first, then FUSE binwalk, Disc images, and fixed Drives.
    // Within each group, mounted items come before unmounted items.
    entries.sort_by(|a, b| {
        let kind_rank = |k: MountKind| match k {
            MountKind::Removable => 0,
            MountKind::FuseBinwalk => 1,
            MountKind::DiscImage => 2,
            MountKind::Drive => 3,
        };
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| (!a.is_mounted).cmp(&(!b.is_mounted)))
            .then_with(|| a.label.cmp(&b.label))
    });

    entries
}

fn is_snap_mount(target: &Path, source: &Path) -> bool {
    let t = target.to_string_lossy();
    let s = source.to_string_lossy();
    t.starts_with("/snap")
        || t.starts_with("/var/lib/snapd")
        || t.starts_with("/run/snapd")
        || s.starts_with("/snap")
        || s.starts_with("/run/snapd")
        || s.starts_with("/var/lib/snapd")
}

fn is_pseudo_filesystem(fstype: &str) -> bool {
    matches!(
        fstype,
        "proc"
            | "sysfs"
            | "devtmpfs"
            | "devpts"
            | "tmpfs"
            | "cgroup"
            | "cgroup2"
            | "pstore"
            | "efivarfs"
            | "bpf"
            | "fusectl"
            | "configfs"
            | "debugfs"
            | "tracefs"
            | "securityfs"
            | "binfmt_misc"
            | "hugetlbfs"
            | "mqueue"
            | "autofs"
            | "overlay"
            | "ramfs"
            | "nsfs"
            | "rpc_pipefs"
    )
}

fn is_system_root(target: &Path) -> bool {
    target == Path::new("/") || target == Path::new("/boot") || target.starts_with("/boot/")
}

fn is_desktop_internal_mount(target: &Path) -> bool {
    let s = target.to_string_lossy();
    s.starts_with("/run/user/") && (s.ends_with("/doc") || s.ends_with("/gvfs"))
}

pub(super) fn mount_command(program: &str) -> CommandRequest {
    CommandRequest::new(program)
        .timeout(Duration::from_secs(20))
        .max_output_bytes(64 * 1024)
        .own_process_group()
}

/// Mounts an unmounted block device using `udisksctl mount -b`.
pub(crate) fn mount_block_device(device_path: &Path) -> Result<PathBuf, String> {
    let output = mount_command("udisksctl")
        .args(["mount", "--no-user-interaction", "-b"])
        .arg(device_path)
        .run()
        .map_err(|e| format!("Failed to run udisksctl: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if !stderr.is_empty() {
            stderr
        } else {
            format!("Mount failed with exit code {:?}", output.status.code())
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    extract_mount_point_from_udisksctl_output(&stdout)
        .ok_or_else(|| "Device mounted, but mountpoint could not be parsed".into())
}

/// Parses `Mounted /dev/XXX at /path/to/mount.` from `udisksctl mount` output.
pub(crate) fn extract_mount_point_from_udisksctl_output(stdout: &str) -> Option<PathBuf> {
    for line in stdout.lines() {
        if let Some(idx) = line.find(" at ") {
            let after = &line[idx + 4..].trim();
            let path_str = after.trim_end_matches('.');
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
    }
    None
}

/// Unmounts a mounted target.
pub(crate) fn unmount_target(
    mount_point: &Path,
    device_path: Option<&Path>,
    filesystem: Option<&str>,
    kind: MountKind,
) -> Result<(), String> {
    // If it's a fuse-binwalk mount, use fusermount3/fusermount
    if kind == MountKind::FuseBinwalk {
        return super::fuse_binwalk::unmount_fuse_binwalk(mount_point);
    }

    // Try udisksctl unmount
    let udisks_cmd = mount_command("udisksctl")
        .args(["unmount", "--no-user-interaction", "-b"])
        .arg(device_path.unwrap_or(mount_point));
    match udisks_cmd.run() {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(_) | Err(CommandError::Spawn { .. }) => {}
        Err(error) => return Err(format!("Could not confirm UDisks unmount: {error}")),
    }

    if filesystem.is_some_and(|filesystem| {
        filesystem == "fuse" || filesystem == "fuseblk" || filesystem.starts_with("fuse.")
    }) {
        match super::fuse_binwalk::run_fusermount(mount_point) {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(_) | Err(CommandError::Spawn { .. }) => {}
            Err(error) => return Err(format!("Could not confirm FUSE unmount: {error}")),
        }
    }

    // Fallback: standard umount
    let status = mount_command("umount")
        .arg(mount_point)
        .run()
        .map_err(|e| format!("Failed to run umount: {e}"))?
        .status;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Unmount failed with code {:?}", status.code()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_scan_is_deferred_and_refresh_retires_stale_results() {
        let mut mounts = Mounts::new();
        assert!(mounts.entries.is_empty());
        assert!(mounts.last_scan.is_none());
        assert!(mounts.scan_receiver.is_none());

        // Occupy the worker so ticking and refreshing must return while discovery waits.
        let jobs = BackgroundJobs::new(1, 4);
        let (release, wait) = mpsc::channel();
        let (_ticket, _completion) = jobs.submit(Duration::from_secs(10), move || {
            let _ = wait.recv();
        });
        assert!(mounts.tick(&jobs).is_none());
        let first_revision = mounts.scan_ticket.as_ref().unwrap().revision();
        assert!(mounts.scan_receiver.is_some());
        mounts.request_scan(&jobs);
        assert_ne!(
            mounts.scan_ticket.as_ref().unwrap().revision(),
            first_revision
        );
        // A retired sender cannot publish into the replacement receiver.
        let (stale_sender, stale_receiver) = mpsc::channel();
        mounts.scan_receiver = Some(stale_receiver);
        mounts.request_scan(&jobs);
        assert!(stale_sender.send(Ok(Vec::new())).is_err());
        mounts.scan_ticket.as_ref().unwrap().cancel();
        release.send(()).unwrap();
    }

    #[test]
    fn ordinary_unmount_never_uses_fuse_cleanup() {
        const FIXTURE_ENV: &str = "LITTLE_CLIP_UNMOUNT_TEST_DIRECTORY";
        let Some(directory) = std::env::var_os(FIXTURE_ENV).map(PathBuf::from) else {
            use std::os::unix::fs::PermissionsExt;
            let directory = tempfile::tempdir().unwrap();
            // The fake FUSE command refuses lazy detach, as a busy filesystem should.
            let fuse = r#"case " $* " in *" -z "*) exit 23;; esac
printf 'fuse\n' >> "$UNMOUNT_TEST_LOG""#;
            for (program, body) in [
                ("udisksctl", "exit 1"),
                ("umount", "printf 'umount\\n' >> \"$UNMOUNT_TEST_LOG\""),
                ("fusermount3", fuse),
                ("fusermount", fuse),
            ] {
                let path = directory.path().join(program);
                std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "file_manager::mounts::tests::ordinary_unmount_never_uses_fuse_cleanup",
                    "--nocapture",
                ])
                .env(FIXTURE_ENV, directory.path())
                .env("UNMOUNT_TEST_LOG", directory.path().join("commands"))
                .env("PATH", directory.path())
                .env_remove("DBUS_SESSION_BUS_ADDRESS")
                .env_remove("DBUS_SYSTEM_BUS_ADDRESS")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated unmount test failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        };
        let log = directory.join("commands");
        for kind in [MountKind::Drive, MountKind::Removable, MountKind::DiscImage] {
            let mount = directory.join("mount");
            std::fs::create_dir(&mount).unwrap();
            std::fs::write(&log, "").unwrap();
            unmount_target(
                &mount,
                Some(Path::new("/dev/fake-test-device")),
                Some("ext4"),
                kind,
            )
            .unwrap();
            assert!(
                mount.is_dir(),
                "ordinary unmount removed its mount directory"
            );
            assert_eq!(std::fs::read_to_string(&log).unwrap(), "umount\n");
            std::fs::remove_dir(&mount).unwrap();
        }
        let mount = directory.join("fuse-mount");
        std::fs::create_dir(&mount).unwrap();
        std::fs::write(&log, "").unwrap();
        unmount_target(&mount, None, Some("fuse.sshfs"), MountKind::Drive).unwrap();
        assert!(
            mount.is_dir(),
            "generic FUSE unmount must preserve its mount directory"
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "fuse\n");
    }

    #[test]
    fn test_unescape_octal() {
        assert_eq!(unescape_octal(r"/mnt/my\040folder"), "/mnt/my folder");
        assert_eq!(unescape_octal(r"/mnt/normal"), "/mnt/normal");
    }

    #[test]
    fn test_extract_mount_point() {
        let out = "Mounted /dev/sdb1 at /run/media/user/Flash.\n";
        assert_eq!(
            extract_mount_point_from_udisksctl_output(out),
            Some(PathBuf::from("/run/media/user/Flash"))
        );
    }

    #[test]
    fn test_snaps_and_pseudo_filesystems_are_strictly_excluded() {
        let proc_mounts = r#"
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
/dev/nvme0n1p2 / ext4 rw,relatime 0 0
/dev/nvme0n1p1 /boot/efi vfat rw,relatime 0 0
/dev/loop1 /snap/firefox/1234 squashfs ro,nodev,relatime 0 0
/dev/loop2 /snap/core22/567 squashfs ro,nodev,relatime 0 0
/dev/nvme1n1p1 /mnt/projects ext4 rw,relatime 0 0
/dev/sdb1 /media/user/USB_STICK vfat rw,relatime 0 0
fuse-binwalk /run/user/1001/fuse-binwalk/app-abc1234 fuse.fuse-binwalk rw,nosuid,nodev,relatime 0 0
/dev/loop10 /mnt/myiso iso9660 ro,relatime 0 0
"#;

        let entries = parse_mounts(proc_mounts, None);

        // Verify snaps, sysfs, proc, root, and boot/efi are completely excluded
        for entry in &entries {
            if let Some(mp) = &entry.mount_point {
                assert!(
                    !mp.starts_with("/snap"),
                    "Snap mount should be excluded: {:?}",
                    mp
                );
                assert_ne!(mp, Path::new("/"));
                assert_ne!(mp, Path::new("/boot/efi"));
            }
        }

        // Verify our expected mounts ARE included:
        let targets: Vec<PathBuf> = entries
            .iter()
            .filter_map(|e| e.mount_point.clone())
            .collect();
        assert!(targets.contains(&PathBuf::from("/mnt/projects")));
        assert!(targets.contains(&PathBuf::from("/media/user/USB_STICK")));
        assert!(targets.contains(&PathBuf::from("/run/user/1001/fuse-binwalk/app-abc1234")));
        assert!(targets.contains(&PathBuf::from("/mnt/myiso")));

        // Verify removable flags
        let binwalk_entry = entries
            .iter()
            .find(|e| e.kind == MountKind::FuseBinwalk)
            .unwrap();
        assert!(binwalk_entry.is_removable);
        assert_eq!(binwalk_entry.label, "app (binwalk)");

        let usb_entry = entries
            .iter()
            .find(|e| e.kind == MountKind::Removable)
            .unwrap();
        assert!(usb_entry.is_removable);

        let iso_entry = entries
            .iter()
            .find(|e| e.kind == MountKind::DiscImage)
            .unwrap();
        assert!(iso_entry.is_removable);

        let project_entry = entries
            .iter()
            .find(|e| e.mount_point.as_deref() == Some(Path::new("/mnt/projects")))
            .unwrap();
        assert!(!project_entry.is_removable);
    }

    #[test]
    fn partition_parents_use_reported_relationships_for_numbered_disks() {
        for (disk, partition, independent) in [
            ("nbd0", "nbd0p1", "nbd1"),
            ("pmem0", "pmem0p1", "pmem1"),
            ("nvme0n1", "nvme0n1p1", "nvme1n1"),
            ("sda", "sda1", "sdb"),
        ] {
            let devices = serde_json::json!({"blockdevices": [
                {"name": disk, "path": format!("/dev/{disk}"), "type": "disk", "rm": true},
                {"name": partition, "path": format!("/dev/{partition}"), "type": "part", "pkname": disk, "fstype": "ext4"},
                {"name": independent, "path": format!("/dev/{independent}"), "type": "disk", "fstype": "ext4"}
            ]});
            let entries = parse_mounts("", Some(&devices.to_string()));
            let paths: Vec<_> = entries
                .iter()
                .filter_map(|entry| entry.device_path.clone())
                .collect();
            assert!(
                !paths.contains(&PathBuf::from(format!("/dev/{disk}"))),
                "offered partitioned whole disk {disk}"
            );
            assert!(paths.contains(&PathBuf::from(format!("/dev/{partition}"))));
            assert!(paths.contains(&PathBuf::from(format!("/dev/{independent}"))));
            assert_eq!(paths.len(), 2);
        }
    }

    #[test]
    fn test_unmounted_devices_from_lsblk_are_included() {
        let proc_mounts = "/dev/nvme0n1p2 / ext4 rw,relatime 0 0\n";
        let lsblk_json = r#"{
            "blockdevices": [
                {
                    "name": "sdb",
                    "path": "/dev/sdb",
                    "label": null,
                    "partlabel": null,
                    "fstype": null,
                    "mountpoint": null,
                    "size": "32G",
                    "rm": true,
                    "hotplug": true,
                    "model": "SanDisk Ultra",
                    "type": "disk"
                },
                {
                    "name": "sdb1",
                    "pkname": "sdb",
                    "path": "/dev/sdb1",
                    "label": "FLASH_DATA",
                    "partlabel": null,
                    "fstype": "vfat",
                    "mountpoint": null,
                    "size": "32G",
                    "rm": true,
                    "hotplug": true,
                    "model": null,
                    "type": "part"
                }
            ]
        }"#;

        let entries = parse_mounts(proc_mounts, Some(lsblk_json));
        let unmounted_entry = entries
            .iter()
            .find(|e| e.device_path == Some(PathBuf::from("/dev/sdb1")))
            .unwrap();
        assert!(!unmounted_entry.is_mounted);
        assert_eq!(unmounted_entry.label, "FLASH_DATA");
        assert!(unmounted_entry.detail.contains("vfat"));
        assert!(unmounted_entry.detail.contains("unmounted"));
        assert!(unmounted_entry.is_removable);
    }

    #[test]
    fn test_mount_entry_sorting_order() {
        let mut entries = [
            MountEntry {
                id: "drive1".into(),
                label: "Z Drive".into(),
                detail: "".into(),
                mount_point: Some(PathBuf::from("/mnt/z")),
                device_path: None,
                filesystem: None,
                is_mounted: true,
                is_removable: false,
                kind: MountKind::Drive,
            },
            MountEntry {
                id: "usb_unmounted".into(),
                label: "Unmounted Flash".into(),
                detail: "".into(),
                mount_point: None,
                device_path: Some(PathBuf::from("/dev/sdc1")),
                filesystem: None,
                is_mounted: false,
                is_removable: true,
                kind: MountKind::Removable,
            },
            MountEntry {
                id: "usb_mounted".into(),
                label: "Active Flash".into(),
                detail: "".into(),
                mount_point: Some(PathBuf::from("/media/user/active")),
                device_path: Some(PathBuf::from("/dev/sdb1")),
                filesystem: None,
                is_mounted: true,
                is_removable: true,
                kind: MountKind::Removable,
            },
            MountEntry {
                id: "binwalk".into(),
                label: "app (binwalk)".into(),
                detail: "".into(),
                mount_point: Some(PathBuf::from("/run/user/1000/fuse-binwalk/app")),
                device_path: None,
                filesystem: None,
                is_mounted: true,
                is_removable: true,
                kind: MountKind::FuseBinwalk,
            },
            MountEntry {
                id: "iso".into(),
                label: "Ubuntu ISO".into(),
                detail: "".into(),
                mount_point: Some(PathBuf::from("/mnt/iso")),
                device_path: None,
                filesystem: None,
                is_mounted: true,
                is_removable: true,
                kind: MountKind::DiscImage,
            },
        ];

        entries.sort_by(|a, b| {
            let kind_rank = |k: MountKind| match k {
                MountKind::Removable => 0,
                MountKind::FuseBinwalk => 1,
                MountKind::DiscImage => 2,
                MountKind::Drive => 3,
            };
            kind_rank(a.kind)
                .cmp(&kind_rank(b.kind))
                .then_with(|| (!a.is_mounted).cmp(&(!b.is_mounted)))
                .then_with(|| a.label.cmp(&b.label))
        });

        assert_eq!(entries[0].id, "usb_mounted");
        assert_eq!(entries[1].id, "usb_unmounted");
        assert_eq!(entries[2].id, "binwalk");
        assert_eq!(entries[3].id, "iso");
        assert_eq!(entries[4].id, "drive1");
    }

    #[test]
    fn test_mounts_lifecycle() {
        let mounts = Mounts {
            entries: Vec::new(),
            pending: None,
            last_scan: None,
            scan_ticket: None,
            scan_receiver: None,
            status: Some("Test status".into()),
        };
        assert!(!mounts.busy());
        assert_eq!(mounts.status_text(), Some("Test status"));
    }
}
