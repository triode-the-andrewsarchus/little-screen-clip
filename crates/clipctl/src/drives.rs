//! Portable-drive CLI commands use the same retained daemon jobs as Files.
use clap::Subcommand;
use little_clip_core::drives::*;
use std::path::PathBuf;
#[derive(Subcommand)]
pub(super) enum DriveCommand {
    /// Flush enrolled live changes and unmount the partition
    Eject { drive: String },
    /// Mount a completed partition-image backup read-only
    MountImage { backup: String },
    /// Unmount a managed read-only image
    UnmountImage { backup: String },
    /// Prune old backups using drives.backup_keep_versions
    Prune { drive: String },
    /// List drives, jobs, sync pairs, RAM copies, catalogs and conflicts as JSON
    Status {
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Configure the backup destination (does not start a backup)
    BackupDirectory { path: PathBuf },
    /// Copy a small mounted partition into RAM and enable writeback
    Ram { drive: String },
    /// Back up a partition as files, or an unmounted partition as an image
    Backup {
        drive: String,
        #[arg(long)]
        image: bool,
        /// Restore the mounted state only if this image job unmounted it
        #[arg(long, requires = "image")]
        restore_mounted: bool,
    },
    /// Merge a partition/subtree with a folder and keep them synchronized
    Pair {
        drive: String,
        local: PathBuf,
        #[arg(long, default_value = "")]
        subtree: PathBuf,
    },
    /// Reconcile a sync pair now
    Sync { pair: String },
    /// Pause a pair, or use --resume to resume it
    Pause {
        pair: String,
        #[arg(long)]
        resume: bool,
    },
    /// Explicitly enable or disable deletion propagation after the initial merge
    Deletions {
        pair: String,
        #[arg(long,action=clap::ArgAction::Set)]
        enabled: bool,
    },
    /// Save a bounded metadata-only contents catalog
    Catalog { drive: String },
    /// Mount a saved catalog read-only
    MountCatalog { catalog: String },
    /// Unmount a saved catalog
    UnmountCatalog { catalog: String },
    /// Configure metadata catalog and search indexing admission for a subtree
    Policy {
        drive: String,
        #[arg(default_value = "")]
        relative: PathBuf,
        #[arg(long,action=clap::ArgAction::Set)]
        catalog: bool,
        #[arg(long,action=clap::ArgAction::Set)]
        index: bool,
    },
    /// Flush a RAM copy through the normal conflict-preserving reconciliation
    Flush { session: String },
    /// Save a RAM copy in a new folder within a destination
    SaveRam {
        session: String,
        destination: PathBuf,
    },
    /// Discard a RAM copy, including unsynchronized edits; requires --yes
    DiscardRam {
        session: String,
        #[arg(long)]
        yes: bool,
    },
    /// Cooperatively cancel remaining work; completed writes are retained
    Cancel { operation: String },
    /// Acknowledge a conflict while retaining its saved variants
    Acknowledge { conflict: String },
}
impl DriveCommand {
    pub fn request(self) -> DriveRequest {
        match self {
            Self::Eject { drive } => DriveRequest::Eject { drive },
            Self::MountImage { backup } => DriveRequest::MountImage { backup },
            Self::UnmountImage { backup } => DriveRequest::UnmountImage { backup },
            Self::Prune { drive } => DriveRequest::PruneBackups { drive },
            Self::Status { offset } => DriveRequest::Status { offset },
            Self::BackupDirectory { path } => DriveRequest::BackupDirectory { path },
            Self::Ram { drive } => DriveRequest::CopyToRam { drive },
            Self::Backup {
                drive,
                image,
                restore_mounted,
            } => DriveRequest::Backup {
                drive,
                restore_mounted,
                kind: if image {
                    BackupKind::Image
                } else {
                    BackupKind::Files
                },
            },
            Self::Pair {
                drive,
                local,
                subtree,
            } => DriveRequest::Pair {
                drive,
                relative: subtree,
                local,
            },
            Self::Sync { pair } => DriveRequest::Sync { pair },
            Self::Pause { pair, resume } => DriveRequest::Pause {
                pair,
                paused: !resume,
            },
            Self::Deletions { pair, enabled } => DriveRequest::Deletions { pair, enabled },
            Self::Catalog { drive } => DriveRequest::Catalog { drive },
            Self::MountCatalog { catalog } => DriveRequest::MountCatalog { catalog },
            Self::UnmountCatalog { catalog } => DriveRequest::UnmountCatalog { catalog },
            Self::Policy {
                drive,
                relative,
                catalog,
                index,
            } => DriveRequest::Rule {
                drive,
                relative,
                catalog,
                index,
            },
            Self::Flush { session } => DriveRequest::FlushRam { session },
            Self::SaveRam {
                session,
                destination,
            } => DriveRequest::SaveRam {
                session,
                destination,
            },
            Self::DiscardRam { session, yes } => DriveRequest::DiscardRam {
                session,
                confirmed: yes,
            },
            Self::Cancel { operation } => DriveRequest::Cancel { operation },
            Self::Acknowledge { conflict } => DriveRequest::Resolve { conflict },
        }
    }
}
