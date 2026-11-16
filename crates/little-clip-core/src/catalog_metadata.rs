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

/// A disclosed boundary in a dated metadata catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CatalogEntryStatus {
    /// Descendant contents were excluded, rather than observed to be empty.
    NotCatalogued,
}
impl CatalogEntryStatus {
    /// Returns the fixed metadata protocol token.
    pub fn token(self) -> &'static [u8] {
        match self {
            Self::NotCatalogued => b"not-catalogued",
        }
    }
    /// Returns the displayed coverage label.
    pub fn label(self) -> &'static str {
        match self {
            Self::NotCatalogued => "Not catalogued",
        }
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
        match &bytes[..count] {
            value if value == CatalogEntryStatus::NotCatalogued.token() => {
                Some(CatalogEntryStatus::NotCatalogued)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
