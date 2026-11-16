//! Portable storage configuration and data-only daemon protocol.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveConfig {
    #[serde(with = "crate::path_serde::optional")]
    pub backup_directory: Option<PathBuf>,
    pub ram_capacity_limit: u64,
    pub ram_min_available: u64,
    #[serde(with = "crate::path_serde")]
    pub ram_directory: PathBuf,
    pub catalog_max_entries: u64,
    pub catalog_max_bytes: u64,
    pub catalog_max_depth: usize,
    pub catalog_timeout_seconds: u64,
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
            catalog_max_entries: 250_000,
            catalog_max_bytes: 256 * 1024 * 1024,
            catalog_max_depth: 64,
            catalog_timeout_seconds: 600,
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
    pub removable: bool,
    pub read_only: bool,
    /// Hardware/partition evidence distinguishes cloned filesystem identifiers.
    pub evidence: String,
    pub ambiguous: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackupKind {
    Files,
    Image,
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
    BackupDirectory {
        #[serde(with = "crate::path_serde")]
        path: PathBuf,
    },
    CopyToRam {
        drive: String,
    },
    Backup {
        drive: String,
        kind: BackupKind,
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
    MountCatalog {
        catalog: String,
    },
    UnmountCatalog {
        catalog: String,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DriveReply {
    Status(Box<DriveStatus>),
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
    #[serde(with = "crate::path_serde::optional")]
    pub mount: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// A daemon-published exclusion list; these rules do not affect backups or sync.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndexPolicy {
    #[serde(with = "crate::path_serde::list")]
    pub excluded: Vec<PathBuf>,
}

/// Checks the bounded local drive-policy registry on background search workers.
///
/// An existing unreadable or malformed registry fails closed. This policy only
/// excludes work and never authorizes payload access or enrolls a search root.
pub fn index_excluded(path: &std::path::Path) -> bool {
    use std::{
        os::unix::fs::MetadataExt,
        sync::{LazyLock, Mutex},
    };
    type Cache = Option<(PathBuf, [u64; 5], Option<IndexPolicy>)>;
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(None));
    let registry = state_directory().join("index-policy.json");
    let metadata = match std::fs::symlink_metadata(&registry) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let signature = [
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
    ];
    let Ok(mut cache) = CACHE.lock() else {
        return true;
    };
    if cache
        .as_ref()
        .is_none_or(|(p, s, _)| *p != registry || *s != signature)
    {
        let value = crate::kernel_interface::read_content_bounded(&registry, 1024 * 1024)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<IndexPolicy>(&bytes).ok());
        *cache = Some((registry, signature, value));
    }
    cache.as_ref().is_none_or(|(_, _, policy)| {
        policy
            .as_ref()
            .is_none_or(|p| p.excluded.iter().any(|root| path.starts_with(root)))
    })
}

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
