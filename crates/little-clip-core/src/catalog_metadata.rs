//! Fixed-size catalog coverage observations, never payload access authority.
use crate::kernel_interface::{self, MountIdentity, Namespace};
use std::{
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

/// Filesystem subtype used by the owned metadata catalog server.
pub const CATALOG_FS_SUBTYPE: &str = "clip-catalog";
/// Read-only catalog coverage attribute; its values are bounded protocol tokens.
pub const CATALOG_STATUS_XATTR: &str = "user.little-clip.catalog.status";
/// Structured capture coverage and timestamp facts, bounded independently of payloads.
pub const CATALOG_DETAILS_XATTR: &str = "user.little-clip.catalog.details";
/// Original saved entry metadata, including Unix mode/type and lossless link bytes.
///
/// The `system` namespace permits metadata-only `lgetxattr` on symlinks; Linux
/// rejects `user` attributes on symlinks before calling the filesystem.
pub const CATALOG_ENTRY_XATTR: &str = "system.little-clip.catalog.entry";

/// The stage at which a metadata-only inventory observation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CatalogScanStage {
    InspectEntry,
    OpenDirectory,
    ReadDirectory,
    Budget,
}

/// A typed failure or traversal limit; neither absence nor truncation means empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CatalogScanReason {
    Io,
    Changed,
    GuardOrUnknown,
    Entries,
    Bytes,
    Database,
    Depth,
    Time,
}

/// A bounded diagnostic. Entry-specific human-readable errors stay with that entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogScanIssue {
    pub stage: CatalogScanStage,
    pub reason: CatalogScanReason,
    pub errno: Option<i32>,
}

/// Coverage of one directory in the dated capture, including its descendants.
///
/// Completeness is an observation of traversal, never a transactional snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogDirectoryCoverage {
    /// True only after the directory iterator reached its end without an error.
    pub listing_complete: bool,
    /// False when any descendant is excluded, failed or not fully traversed.
    pub subtree_complete: bool,
    /// Immediate entries whose metadata or subtree could not be collected.
    pub failed_entries: u64,
    /// At least one immediate subtree did not have complete coverage.
    pub incomplete_child: bool,
    /// Direct directory or budget failure; propagated global limits stay separate.
    pub issue: Option<CatalogScanIssue>,
}

/// Source and root coverage facts recorded by newer metadata captures.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogCaptureDetails {
    pub directory: CatalogDirectoryCoverage,
    /// Limit that stopped the scan; unfinished directory records inherit this limit.
    pub stopped: Option<CatalogScanIssue>,
    /// Captured root identity; this dated observation never authorizes a later read.
    pub source_mount: MountIdentity,
    /// Namespace in which the source root was observed.
    pub namespace: Namespace,
    /// Observed kernel mount type, independently of labels or filename conventions.
    pub filesystem: Option<String>,
    /// Documented modification-time storage resolution for a known source driver.
    /// None means unknown. This does not authorize revision or content equivalence.
    pub mtime_resolution_ns: Option<u64>,
}

/// A saved special-file type whose catalog representation cannot perform I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CatalogSpecialKind {
    Fifo,
    Socket,
    CharacterDevice,
    BlockDevice,
    Unknown,
}
impl CatalogSpecialKind {
    /// Returns the saved special-file type from the original Unix mode bits.
    pub fn from_mode(mode: u32) -> Self {
        match mode & libc::S_IFMT {
            libc::S_IFIFO => Self::Fifo,
            libc::S_IFSOCK => Self::Socket,
            libc::S_IFCHR => Self::CharacterDevice,
            libc::S_IFBLK => Self::BlockDevice,
            _ => Self::Unknown,
        }
    }
}

/// A disclosed coverage boundary or inert entry in a dated metadata catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CatalogEntryStatus {
    /// Descendant contents were excluded, rather than observed to be empty.
    NotCatalogued,
    /// Some descendant metadata is missing, failed, excluded or truncated.
    Incomplete,
    /// A special file retained as metadata, without live device or IPC behavior.
    MetadataOnly(CatalogSpecialKind),
}
impl CatalogEntryStatus {
    /// Returns the saved special-file type when this entry has an inert view.
    pub fn special_kind(self) -> Option<CatalogSpecialKind> {
        match self {
            Self::MetadataOnly(kind) => Some(kind),
            _ => None,
        }
    }
    /// Returns the fixed metadata protocol token.
    pub fn token(self) -> &'static [u8] {
        match self {
            Self::NotCatalogued => b"not-catalogued",
            Self::Incomplete => b"incomplete",
            Self::MetadataOnly(CatalogSpecialKind::Fifo) => b"fifo-metadata",
            Self::MetadataOnly(CatalogSpecialKind::Socket) => b"socket-metadata",
            Self::MetadataOnly(CatalogSpecialKind::CharacterDevice) => b"char-device-metadata",
            Self::MetadataOnly(CatalogSpecialKind::BlockDevice) => b"block-device-metadata",
            Self::MetadataOnly(CatalogSpecialKind::Unknown) => b"special-metadata",
        }
    }
    /// Returns the displayed coverage label.
    pub fn label(self) -> &'static str {
        match self {
            Self::NotCatalogued => "Not catalogued",
            Self::Incomplete => "Partial catalog",
            Self::MetadataOnly(CatalogSpecialKind::Fifo) => "FIFO metadata",
            Self::MetadataOnly(CatalogSpecialKind::Socket) => "Socket metadata",
            Self::MetadataOnly(CatalogSpecialKind::CharacterDevice) => "Char device metadata",
            Self::MetadataOnly(CatalogSpecialKind::BlockDevice) => "Block device metadata",
            Self::MetadataOnly(CatalogSpecialKind::Unknown) => "Special metadata",
        }
    }
    /// Describes the observation without treating missing metadata as an empty folder.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::NotCatalogued => "Contents were excluded from this saved snapshot",
            Self::Incomplete => {
                "Some descendant metadata was excluded, failed or not fully inventoried"
            }
            Self::MetadataOnly(_) => {
                "Saved special-file metadata; opening cannot activate a device, pipe or socket"
            }
        }
    }
    fn from_token(token: &[u8]) -> Option<Self> {
        [
            Self::NotCatalogued,
            Self::Incomplete,
            Self::MetadataOnly(CatalogSpecialKind::Fifo),
            Self::MetadataOnly(CatalogSpecialKind::Socket),
            Self::MetadataOnly(CatalogSpecialKind::CharacterDevice),
            Self::MetadataOnly(CatalogSpecialKind::BlockDevice),
            Self::MetadataOnly(CatalogSpecialKind::Unknown),
        ]
        .into_iter()
        .find(|status| status.token() == token)
    }
}

/// A pinned catalog mount context for worker-side metadata observations.
pub struct CatalogMount {
    root: File,
    identity: MountIdentity,
    namespace: Namespace,
}
impl CatalogMount {
    /// Detects an owned catalog view using its pinned mount identity and subtype.
    ///
    /// Ordinary filesystems and unrelated FUSE mounts receive no attribute probe.
    /// Unknown/replaced context returns `None`. Run on a background worker.
    pub fn detect(path: &Path) -> Option<Self> {
        let root = kernel_interface::pin_content_directory(path).ok()?;
        if rustix::fs::fstatfs(&root).ok()?.f_type as u32 != 0x65735546 {
            return None;
        }
        let identity = MountIdentity::pinned(&root).ok()?;
        let namespace = Namespace::current();
        if !namespace.known() {
            return None;
        }
        let mount = crate::mount_inventory::inventory()
            .ok()?
            .into_iter()
            .find(|mount| mount.id == identity.mount)?;
        if mount.filesystem.strip_prefix("fuse.") != Some(CATALOG_FS_SUBTYPE) {
            return None;
        }
        uuid::Uuid::parse_str(
            mount
                .source
                .to_str()?
                .strip_prefix("little-clip-catalog:")?,
        )
        .ok()?;
        Some(Self {
            root,
            identity,
            namespace,
        })
    }

    /// Reads one fixed-size coverage token within the retained catalog mount.
    ///
    /// This is display metadata, not permission to read or index file contents.
    /// Missing/unknown attributes or changed namespace/mount context return `None`.
    pub fn status(&self, path: &Path) -> Option<CatalogEntryStatus> {
        if Namespace::current() != self.namespace
            || MountIdentity::pinned(&self.root).ok()? != self.identity
        {
            return None;
        }
        let file = kernel_interface::pin(path, true).ok()?;
        if MountIdentity::pinned(&file).ok()?.mount != self.identity.mount
            || kernel_interface::classify_handle(&file).ok()? != kernel_interface::Kind::Content
        {
            return None;
        }
        let anchored = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        let mut bytes = [0; 32];
        let count = rustix::fs::getxattr(anchored, CATALOG_STATUS_XATTR, &mut bytes[..]).ok()?;
        CatalogEntryStatus::from_token(&bytes[..count])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn special_metadata_tokens_are_typed_bounded_and_do_not_affect_ordinary_files() {
        for (mode, kind) in [
            (libc::S_IFIFO, CatalogSpecialKind::Fifo),
            (libc::S_IFSOCK, CatalogSpecialKind::Socket),
            (libc::S_IFCHR, CatalogSpecialKind::CharacterDevice),
            (libc::S_IFBLK, CatalogSpecialKind::BlockDevice),
            (0, CatalogSpecialKind::Unknown),
        ] {
            assert_eq!(CatalogSpecialKind::from_mode(mode | 0o600), kind);
            let status = CatalogEntryStatus::MetadataOnly(kind);
            assert!(status.token().len() <= 32);
            assert_eq!(CatalogEntryStatus::from_token(status.token()), Some(status));
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("ordinary.ts");
            std::fs::write(&path, b"ordinary content").unwrap();
            rustix::fs::setxattr(
                &path,
                CATALOG_STATUS_XATTR,
                status.token(),
                rustix::fs::XattrFlags::empty(),
            )
            .unwrap();
            let rows = crate::file_inventory::read_directory(
                temp.path(),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            assert!(rows.entries[0].regular_file);
            assert!(rows.entries[0].catalog_status.is_none());
        }
        assert!(CatalogEntryStatus::from_token(b"unrecognized").is_none());
    }
    #[test]
    fn ordinary_attributes_do_not_create_catalog_coverage_or_change_ordinary_revision_protocol() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("ordinary-empty");
        std::fs::create_dir(&folder).unwrap();
        rustix::fs::setxattr(
            &folder,
            CATALOG_STATUS_XATTR,
            CatalogEntryStatus::NotCatalogued.token(),
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        assert!(CatalogMount::detect(&folder).is_none());
        let snapshot = crate::file_inventory::read_directory(
            temp.path(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let entry = &snapshot.entries[0];
        assert_eq!(entry.catalog_status, None);
        assert_eq!(entry.size, "—");
        assert_eq!(
            entry.revision,
            crate::file_inventory::current_revision(&folder).unwrap()
        );
        let saved = serde_json::to_value(entry).unwrap();
        assert!(saved.get("catalog_status").is_none());
        let old: crate::file_inventory::Entry = serde_json::from_value(saved).unwrap();
        assert_eq!(old.catalog_status, None);
    }
}
