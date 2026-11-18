//! Portable storage configuration and data-only daemon protocol.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
/// Maximum number of independently admitted catalog captures.
pub const MAX_CATALOG_WORKERS: usize = 4;

mod catalog_limits;
pub use catalog_limits::{CatalogLimitOverrides, CatalogLimits};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveConfig {
    #[serde(with = "crate::path_serde::optional")]
    pub backup_directory: Option<PathBuf>,
    pub ram_capacity_limit: u64,
    pub ram_min_available: u64,
    #[serde(with = "crate::path_serde")]
    pub ram_directory: PathBuf,
    pub ram_session_max_bytes: u64,
    pub ram_total_max_bytes: u64,
    pub ram_session_max_entries: u64,
    pub ram_total_max_entries: u64,
    pub ram_max_sessions: usize,
    pub catalog_max_entries: u64,
    pub catalog_max_bytes: u64,
    pub catalog_max_depth: usize,
    pub catalog_timeout_seconds: u64,
    /// Concurrent captures on independent sources; lowering waits for active work.
    pub catalog_workers: usize,
    pub sync_interval_seconds: u64,
    /// None retains completed versions until an explicit pruning request.
    pub backup_keep_versions: Option<usize>,
}
impl Default for DriveConfig {
    fn default() -> Self {
        Self {
            backup_directory: None,
            ram_capacity_limit: 4_000_000_000,
            ram_min_available: 8_000_000_000,
            ram_directory: PathBuf::from("/tmp"),
            ram_session_max_bytes: 6_000_000_000,
            ram_total_max_bytes: 16_000_000_000,
            ram_session_max_entries: 2_000_000,
            ram_total_max_entries: 4_000_000,
            ram_max_sessions: 8,
            catalog_max_entries: 250_000,
            catalog_max_bytes: 256 * 1024 * 1024,
            catalog_max_depth: 64,
            catalog_timeout_seconds: 600,
            catalog_workers: 1,
            sync_interval_seconds: 30,
            backup_keep_versions: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drive {
    pub id: String,
    pub label: String,
    #[serde(with = "crate::path_serde")]
    pub device: PathBuf,
    #[serde(with = "crate::path_serde::optional")]
    pub mount: Option<PathBuf>,
    pub part_uuid: Option<String>,
    pub filesystem_uuid: Option<String>,
    pub filesystem: String,
    pub capacity: u64,
    pub drive_capacity: u64,
    /// Current kernel disk lifetime; deliberately excluded from enrollment evidence.
    #[serde(default)]
    pub disk_sequence: Option<u64>,
    /// Observed partition layout for image manifests; absent in older records.
    #[serde(default)]
    pub partition_layout: Option<Box<PartitionLayout>>,
    pub removable: bool,
    pub read_only: bool,
    /// Hardware/partition evidence distinguishes cloned filesystem identifiers.
    pub evidence: String,
    pub ambiguous: bool,
}

/// Partition-table facts captured without reading partition payload bytes.
///
/// Unknown fields remain absent. These describe a partition image's placement on
/// its parent device; the image itself contains only the selected partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionLayout {
    /// Partition number reported by the partition table.
    pub number: Option<u64>,
    /// Start offset in fixed 512-byte sectors, independently of logical sectors.
    pub start_512_sectors: Option<u64>,
    /// Checked byte conversion of `start_512_sectors`; absent on overflow.
    pub offset_bytes: Option<u64>,
    /// Reported logical sector size in bytes, without assuming a default.
    pub logical_sector_bytes: Option<u64>,
    /// Partition type code or UUID, kept in its reported representation.
    pub partition_type: Option<String>,
    /// Partition-table label, independently of the filesystem label.
    pub label: Option<String>,
    /// Reported flags, without assigning table-independent flag meanings.
    pub flags: Option<String>,
    /// Table type reported for the immediate containing device.
    pub table_type: Option<String>,
    /// Containing table identifier, which is separate from PARTUUID.
    pub table_uuid: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupKind {
    Files,
    Image,
}

/// A short-lived daemon-owned backup selection, reviewed before any unmount.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupPreview {
    pub token: String,
    pub drive: Drive,
    pub kind: BackupKind,
    #[serde(with = "crate::path_serde")]
    pub destination: PathBuf,
    pub space: BackupSpace,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupSpace {
    pub available_bytes: u64,
    /// Full independent image plus working headroom; sharing/sparsity can use less.
    pub full_image_bytes: Option<u64>,
    /// Destination filesystem capability; actual snapshot/reflink operations can fail.
    pub btrfs: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DriveRequest {
    Eject {
        drive: String,
    },
    MountImage {
        backup: String,
    },
    UnmountImage {
        backup: String,
    },
    PruneBackups {
        drive: String,
    },
    Configure {
        config: DriveConfig,
    },
    Status {
        offset: usize,
    },
    /// Measures local sync bookkeeping and optional catalog storage on a retained worker.
    MeasureStorage,
    BackupDirectory {
        #[serde(with = "crate::path_serde")]
        path: PathBuf,
    },
    CopyToRam {
        drive: String,
    },
    PreviewBackup {
        drive: String,
        kind: BackupKind,
        #[serde(with = "crate::path_serde::optional")]
        destination: Option<PathBuf>,
    },
    StartBackup {
        token: String,
        restore_mounted: bool,
        remember_destination: bool,
    },
    /// Starts a reviewed file backup with temporary read-only collection.
    /// Older daemons reject this command instead of silently copying live files.
    StartReadOnlyBackup {
        token: String,
        remember_destination: bool,
    },
    DiscardBackupReview {
        token: String,
    },
    Backup {
        drive: String,
        kind: BackupKind,
        #[serde(default)]
        restore_mounted: bool,
    },
    /// Collects files read-only, then restores the partition's original access mode.
    ReadOnlyBackup {
        drive: String,
    },
    Pair {
        drive: String,
        #[serde(with = "crate::path_serde")]
        relative: PathBuf,
        #[serde(with = "crate::path_serde")]
        local: PathBuf,
    },
    Sync {
        pair: String,
    },
    Pause {
        pair: String,
        paused: bool,
    },
    Deletions {
        pair: String,
        enabled: bool,
    },
    Catalog {
        drive: String,
    },
    /// Explicit limits for one capture; older daemons must reject this operation.
    CatalogWithLimits {
        drive: String,
        limits: CatalogLimitOverrides,
    },
    MountCatalog {
        catalog: String,
    },
    UnmountCatalog {
        catalog: String,
    },
    /// Reads an exact saved subtree rule, independently of status pagination.
    GetRule {
        drive: String,
        #[serde(with = "crate::path_serde")]
        relative: PathBuf,
    },
    Rule {
        drive: String,
        #[serde(with = "crate::path_serde")]
        relative: PathBuf,
        catalog: bool,
        index: bool,
    },
    FlushRam {
        session: String,
    },
    SaveRam {
        session: String,
        #[serde(with = "crate::path_serde")]
        destination: PathBuf,
    },
    DiscardRam {
        session: String,
        confirmed: bool,
    },
    Cancel {
        operation: String,
    },
    Resolve {
        conflict: String,
    },
}

/// Consistency policy for file collection; the on-disk backup format is unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileBackupMode {
    /// Checks each captured revision while retaining the current mounted view.
    #[default]
    Live,
    /// Temporarily mounts read-only and restores the original access mode.
    ReadOnly,
}
impl FileBackupMode {
    fn is_live(&self) -> bool {
        *self == Self::Live
    }
}

impl DriveRequest {
    /// Returns whether this request starts a retained operation with an operation ID.
    pub fn is_operation(&self) -> bool {
        !matches!(
            self,
            Self::Status { .. }
                | Self::GetRule { .. }
                | Self::PreviewBackup { .. }
                | Self::DiscardBackupReview { .. }
                | Self::Cancel { .. }
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DriveReply {
    Status(Box<DriveStatus>),
    BackupPreview(Box<BackupPreview>),
    /// Exact saved rule; `None` means this subtree has no explicit override.
    Rule(Option<DriveRule>),
    Accepted(String),
    Done(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DriveStatus {
    pub config: DriveConfig,
    pub drives: Vec<Drive>,
    pub pairs: Vec<SyncPair>,
    pub ram: Vec<RamSession>,
    pub operations: Vec<DriveOperation>,
    pub catalogs: Vec<Catalog>,
    pub conflicts: Vec<DriveConflict>,
    pub rules: Vec<DriveRule>,
    pub backups: Vec<DriveBackup>,
    #[serde(default)]
    pub recovery: Vec<DriveRecovery>,
    #[serde(with = "crate::path_serde::optional")]
    pub backup_directory: Option<PathBuf>,
    pub ram_unavailable: Option<String>,
    /// Lost pending-event persistence makes all RAM change counts uncertain.
    #[serde(default)]
    pub ram_pending_error: Option<String>,
    pub next_offset: Option<usize>,
    pub discovery_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncPair {
    #[serde(default)]
    pub local_identity: Option<StorageIdentity>,
    pub id: String,
    pub drive: String,
    pub evidence: String,
    #[serde(with = "crate::path_serde")]
    pub relative: PathBuf,
    #[serde(with = "crate::path_serde")]
    pub local: PathBuf,
    pub paused: bool,
    pub deletions: bool,
    pub ram: bool,
    pub last_success: Option<u64>,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RamSession {
    /// Captured RAM directory identity; replacements cannot authorize discard.
    pub identity: crate::kernel_interface::MountIdentity,
    pub id: String,
    pub drive: String,
    #[serde(with = "crate::path_serde")]
    pub path: PathBuf,
    pub ready: bool,
    pub connected: bool,
    pub status: String,
    /// Absent only for legacy, unbounded directory sessions.
    #[serde(default)]
    pub backing: Option<RamBacking>,
    #[serde(default)]
    pub last_saved: Option<RamSave>,
    #[serde(default)]
    pub save_status: Option<String>,
    /// Last successful drive reconciliation followed by a filesystem flush.
    #[serde(default)]
    pub last_flushed: Option<u64>,
    /// Advisory queued-change snapshot; never authorizes discard or eject.
    #[serde(default)]
    pub pending: Option<RamPending>,
}

/// Observed RAM changes awaiting reconciliation, independently of durable exports.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RamPending {
    /// Distinct observed relative paths, including renames and removals.
    pub paths: u64,
    /// Event loss, bounded tracking or restart requires a complete comparison.
    pub full_check_required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RamSave {
    #[serde(with = "crate::path_serde")]
    pub path: PathBuf,
    pub completed: u64,
    /// False when an explicitly saved initial RAM population was incomplete.
    pub populated: bool,
}

impl RamSession {
    /// Reports whether initial population and the current RAM mount are ready.
    pub fn browsable(&self) -> bool {
        self.ready && self.backing.as_ref().is_none_or(|b| b.mounted)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RamBacking {
    #[serde(with = "crate::path_serde")]
    pub directory: PathBuf,
    pub directory_identity: StorageIdentity,
    pub image_identity: StorageIdentity,
    pub capacity: u64,
    pub inodes: u64,
    pub mounted: bool,
    #[serde(default)]
    pub initialized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveOperation {
    pub id: String,
    pub title: String,
    pub state: String,
    pub message: String,
    pub files: u64,
    pub bytes: u64,
    pub started: u64,
    pub finished: Option<u64>,
    /// Retained partition effects and the last durable preparation checkpoint.
    #[serde(default)]
    pub partition: Option<PartitionPreparation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartitionPreparation {
    pub drive: Drive,
    #[serde(with = "crate::path_serde::optional")]
    pub destination: Option<PathBuf>,
    pub phase: PartitionPhase,
    pub unmounted_by_job: bool,
    pub restore_mounted: bool,
    pub completed_backup: Option<String>,
    pub restore_error: Option<String>,
    /// Selected file-collection policy; absent in older operation journals.
    #[serde(default, skip_serializing_if = "FileBackupMode::is_live")]
    pub file_mode: FileBackupMode,
    /// Last observed collection mount, retained for inspection after interruption.
    #[serde(default, with = "crate::path_serde::optional")]
    pub collection_mount: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionPhase {
    Flushing,
    Unmounting,
    Unmounted,
    MountingReadOnly,
    ReadOnlyMounted,
    CopyingFiles,
    FilesPublished,
    OpeningImage,
    CopyingImage,
    VerifyingImage,
    PublishingImage,
    ImagePublished,
    RestoringMount,
    MountRestored,
    MountRestoreFailed,
}
impl PartitionPhase {
    /// Describes the work whose checkpoint has been durably recorded.
    pub fn description(self) -> &'static str {
        match self {
            Self::Flushing => "Flushing enrolled folders before unmount",
            Self::Unmounting => "Waiting for partition unmount / authorization",
            Self::Unmounted => "Partition confirmed unmounted",
            Self::MountingReadOnly => "Mounting the partition read-only / authorization",
            Self::ReadOnlyMounted => "Read-only filesystem confirmed",
            Self::CopyingFiles => "Collecting files from the read-only filesystem",
            Self::FilesPublished => "Completed file backup published",
            Self::OpeningImage => "Waiting for exclusive image read / authorization",
            Self::CopyingImage => "Copying partition bytes",
            Self::VerifyingImage => "Verifying every image byte",
            Self::PublishingImage => "Publishing verified image",
            Self::ImagePublished => "Completed image published",
            Self::RestoringMount => "Restoring the partition's mounted state",
            Self::MountRestored => "Partition mounted again",
            Self::MountRestoreFailed => "Partition remount failed",
        }
    }
}
impl PartitionPreparation {
    /// Reports retained effects without assuming an interrupted authority call failed.
    pub fn interruption_message(&self) -> String {
        let effect = if self.phase == PartitionPhase::MountRestored {
            "This job confirmed the partition was mounted again before interruption."
        } else if self.unmounted_by_job {
            "This job confirmed the unmount before interruption."
        } else if self.phase == PartitionPhase::Unmounting {
            "The unmount request may have taken effect; inspect the current mount state."
        } else if self.drive.mount.is_none() {
            "The partition was already unmounted when this job started."
        } else {
            "This job did not record a confirmed unmount."
        };
        let image = self
            .completed_backup
            .as_ref()
            .map(|id| format!(" Completed backup: {id}."))
            .unwrap_or_default();
        let collection = if self.file_mode == FileBackupMode::ReadOnly {
            match &self.collection_mount {
                Some(path) => format!(
                    " Requested read-only collection; last observed mount: {}. Inspect its current access mode before retrying.",
                    crate::path_address::display_path(path)
                ),
                None if self.phase == PartitionPhase::MountingReadOnly => {
                    " A read-only mount request may have taken effect; inspect current mounts."
                        .into()
                }
                None => String::new(),
            }
        } else {
            String::new()
        };
        format!(
            "Daemon restarted during: {}. {effect}{collection}{image} Completed writes are retained. Check current drive state and completed/prepared backups before retrying; no mount action was replayed automatically.",
            self.phase.description()
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveConflict {
    #[serde(default)]
    pub complete: bool,
    pub id: String,
    pub pair: String,
    #[serde(with = "crate::path_serde")]
    pub path: PathBuf,
    #[serde(with = "crate::path_serde")]
    pub portable: PathBuf,
    #[serde(with = "crate::path_serde")]
    pub local: PathBuf,
    pub message: String,
    pub resolved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Catalog {
    pub id: String,
    pub drive: String,
    pub evidence: String,
    pub label: String,
    pub captured: u64,
    pub entries: u64,
    pub complete: bool,
    pub coverage: String,
    /// Effective capture budgets; absent when an older catalog did not record them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<CatalogLimits>,
    /// Structured source/coverage facts; absent in older saved catalogs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<crate::catalog_metadata::CatalogCaptureDetails>,
    /// Most recent explicit metadata redaction; the original capture date is retained.
    #[serde(default)]
    pub redacted: Option<u64>,
    /// Monotonic metadata revision used by the virtual filesystem, including same-second edits.
    #[serde(default)]
    pub redaction_generation: u64,
    /// Durable exclusion requests waiting for readers or metadata maintenance.
    #[serde(default)]
    pub pending_redactions: u64,
    #[serde(default)]
    pub redaction_error: Option<String>,
    /// An explicit whole-drive exclusion removed the saved root's descendants.
    #[serde(default)]
    pub root_excluded: bool,
    #[serde(with = "crate::path_serde::optional")]
    pub mount: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveRule {
    pub id: String,
    pub drive: String,
    #[serde(with = "crate::path_serde")]
    pub relative: PathBuf,
    pub catalog: bool,
    pub index: bool,
}

/// Returns whether an ordinary relative subtree address is safe to join to a pin.
pub fn valid_relative(path: &std::path::Path) -> bool {
    path.components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_commands_preserve_legacy_fingerprints_and_read_only_fails_closed_on_older_daemons() {
        #[derive(Serialize, Deserialize)]
        enum LegacyRequest {
            Backup {
                drive: String,
                kind: BackupKind,
                restore_mounted: bool,
            },
            StartBackup {
                token: String,
                restore_mounted: bool,
                remember_destination: bool,
            },
        }
        for old in [
            r#"{"Backup":{"drive":"part:portable","kind":"Files","restore_mounted":false}}"#,
            r#"{"StartBackup":{"token":"selection","restore_mounted":false,"remember_destination":false}}"#,
        ] {
            let request: DriveRequest = serde_json::from_str(old).unwrap();
            assert_eq!(serde_json::to_string(&request).unwrap(), old);
            let legacy: LegacyRequest = serde_json::from_str(old).unwrap();
            assert_eq!(serde_json::to_string(&legacy).unwrap(), old);
        }
        for request in [
            DriveRequest::ReadOnlyBackup {
                drive: "part:portable".into(),
            },
            DriveRequest::StartReadOnlyBackup {
                token: "selection".into(),
                remember_destination: false,
            },
        ] {
            assert!(request.is_operation());
            let encoded = serde_json::to_string(&request).unwrap();
            assert!(serde_json::from_str::<LegacyRequest>(&encoded).is_err());
            let decoded: DriveRequest = serde_json::from_str(&encoded).unwrap();
            assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
        }
    }
    #[test]
    fn legacy_backups_do_not_imply_verified_metadata_and_new_counts_round_trip() {
        let mut backup: DriveBackup = serde_json::from_value(serde_json::json!({
            "id":"saved", "drive":"part:portable", "evidence":"observed",
            "kind":"Files", "path":"/backup/version", "created":123,
            "incremental":false, "bytes_written":7, "source_size":128
        }))
        .unwrap();
        assert_eq!(backup.fidelity, None);
        backup.fidelity = Some(BackupFidelity {
            entries: 4,
            entries_with_omissions: 2,
            omitted_fields: 3,
            source_attributes_unavailable: 1,
        });
        let recovered: DriveBackup =
            serde_json::from_slice(&serde_json::to_vec(&backup).unwrap()).unwrap();
        assert_eq!(recovered.fidelity, backup.fidelity);
    }
    #[test]
    fn legacy_catalog_records_have_no_implied_redaction_or_pending_work() {
        let catalog: Catalog = serde_json::from_value(serde_json::json!({
            "id":"saved", "drive":"portable", "evidence":"observed", "label":"Card",
            "captured":123, "entries":9, "complete":true, "coverage":"Complete", "mount":null
        }))
        .unwrap();
        assert_eq!(catalog.captured, 123);
        assert_eq!(catalog.details, None);
        assert_eq!(catalog.limits, None);
        assert!(
            serde_json::to_value(&catalog)
                .unwrap()
                .get("details")
                .is_none()
        );
        assert_eq!(catalog.redacted, None);
        assert_eq!(catalog.redaction_generation, 0);
        assert_eq!(catalog.pending_redactions, 0);
        assert_eq!(catalog.redaction_error, None);
        assert!(!catalog.root_excluded);
    }
    #[test]
    fn drive_defaults_remain_opt_in_and_relative_paths_cannot_escape() {
        let defaults = DriveConfig::default();
        assert!(defaults.backup_directory.is_none());
        for name in ["", "a", "a/b"] {
            assert!(valid_relative(name.as_ref()));
        }
        for name in ["/tmp", "../a", "a/../b"] {
            assert!(!valid_relative(name.as_ref()));
        }
        let config: crate::AppConfig = toml::from_str("").unwrap();
        assert_eq!(
            config.drives.ram_capacity_limit,
            defaults.ram_capacity_limit
        );
    }
    #[test]
    fn legacy_ram_records_recover_without_requiring_a_new_backing_format() {
        let session = RamSession {
            identity: crate::kernel_interface::MountIdentity {
                device: 1,
                inode: 2,
                mount: 3,
            },
            id: "legacy".into(),
            drive: "fixture".into(),
            path: "/tmp/little-clip-ram-legacy".into(),
            ready: true,
            connected: false,
            status: "retained".into(),
            backing: None,
            last_saved: None,
            save_status: None,
            last_flushed: None,
            pending: None,
        };
        let mut json = serde_json::to_value(&session).unwrap();
        json.as_object_mut().unwrap().remove("backing");
        json.as_object_mut().unwrap().remove("last_saved");
        json.as_object_mut().unwrap().remove("save_status");
        json.as_object_mut().unwrap().remove("last_flushed");
        json.as_object_mut().unwrap().remove("pending");
        let recovered: RamSession = serde_json::from_value(json).unwrap();
        assert_eq!(recovered.path, session.path);
        assert!(recovered.backing.is_none());
        assert!(recovered.last_saved.is_none());
        assert!(recovered.save_status.is_none());
        assert!(recovered.last_flushed.is_none());
        assert!(recovered.pending.is_none());
        assert!(recovered.browsable());
    }
}

/// Returns the local persistent drive-service directory, independently from launch cwd.
pub fn state_directory() -> PathBuf {
    std::env::var_os("LITTLE_CLIP_DRIVE_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
                })
                .join("little-screen-clip/drives")
        })
}

mod index_policy;
pub use index_policy::{IndexContext, IndexPolicy, index_excluded, index_policy};

/// A prepared durable operation that could not finish publication automatically.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveRecovery {
    pub id: String,
    pub drive: String,
    pub cause: RecoveryCause,
    #[serde(with = "crate::path_serde")]
    pub artifact: PathBuf,
    pub message: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum RecoveryCause {
    Unavailable,
    PermissionDenied,
    ChangedIdentityOrContent,
    Storage,
}

/// A completed version; image mounts are explicit and read-only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriveBackup {
    /// Observed file-copy fidelity; absent for images and older version records.
    #[serde(default)]
    pub fidelity: Option<BackupFidelity>,
    #[serde(default, with = "crate::path_serde::optional")]
    pub manifest: Option<PathBuf>,
    #[serde(default)]
    pub manifest_sha256: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    pub id: String,
    pub drive: String,
    pub evidence: String,
    pub kind: BackupKind,
    #[serde(with = "crate::path_serde")]
    pub path: PathBuf,
    pub created: u64,
    pub incremental: bool,
    pub bytes_written: u64,
    pub source_size: u64,
    #[serde(default, with = "crate::path_serde::optional")]
    pub mount: Option<PathBuf>,
    #[serde(default)]
    pub loop_object: Option<String>,
    #[serde(default)]
    pub revision: Option<Vec<u8>>,
    #[serde(default)]
    pub identity: Option<StorageIdentity>,
}

/// Counts from the detailed, per-path metadata report in a file backup manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFidelity {
    pub entries: u64,
    pub entries_with_omissions: u64,
    pub omitted_fields: u64,
    pub source_attributes_unavailable: u64,
}

/// Persistent filesystem/object evidence, independent of transient kernel mount IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageIdentity {
    pub filesystem: u64,
    pub inode: u64,
    pub birth: Option<(i64, u32)>,
}
impl StorageIdentity {
    /// Observes filesystem ID, inode and available birth time from a retained handle.
    ///
    /// # Errors
    /// Returns missing metadata or filesystem identity errors.
    pub fn pinned(file: &std::fs::File) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let filesystem = rustix::fs::fstatvfs(file)?.f_fsid;
        if filesystem == 0 {
            return Err(std::io::Error::other(
                "Filesystem has no persistent identity",
            ));
        }
        let stat = rustix::fs::statx(
            file,
            "",
            rustix::fs::AtFlags::EMPTY_PATH,
            rustix::fs::StatxFlags::BTIME,
        )?;
        let birth = (stat.stx_mask & libc::STATX_BTIME != 0)
            .then_some((stat.stx_btime.tv_sec, stat.stx_btime.tv_nsec));
        Ok(Self {
            filesystem,
            inode: file.metadata()?.ino(),
            birth,
        })
    }
}
