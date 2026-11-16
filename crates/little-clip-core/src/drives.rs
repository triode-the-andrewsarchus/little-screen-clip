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
    pub drives: Vec<Drive>,
    pub pairs: Vec<SyncPair>,
    pub ram: Vec<RamSession>,
    pub operations: Vec<DriveOperation>,
    pub catalogs: Vec<Catalog>,
    pub conflicts: Vec<DriveConflict>,
    pub rules: Vec<DriveRule>,
    #[serde(with = "crate::path_serde::optional")]
    pub backup_directory: Option<PathBuf>,
    pub ram_unavailable: Option<String>,
    pub next_offset: Option<usize>,
    pub discovery_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncPair {
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
