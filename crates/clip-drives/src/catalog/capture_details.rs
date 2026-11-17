//! Source-driver precision is descriptive metadata, never content equivalence.
use super::*;
use little_clip_core::{
    catalog_metadata::{CatalogCaptureDetails, CatalogDirectoryCoverage, CatalogScanIssue},
    kernel_interface::{MountIdentity, Namespace},
};

/// Reserve final header space independently of transactional inventory batches.
pub(super) fn database(path: &Path, bytes: u64) -> Result<(Store, u64)> {
    if bytes < 128 * 1024 {
        return Err("Catalog byte limit must be at least 128 KiB".into());
    }
    let db = Store::open(path)?;
    db.0.execute("UPDATE version SET value=2 WHERE id=1", [])?;
    // Rowid is the final implicit key of this ordinary SQLite index, giving
    // each directory an ordered range seek without sorting its whole listing.
    db.0.execute_batch("CREATE INDEX entry_parent_offset ON entries(scan,parent)")?;
    db.0.execute_batch("PRAGMA journal_mode=DELETE")?;
    let page_size: u64 = db.0.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let pages = bytes / page_size;
    db.0.pragma_update(None, "max_page_count", pages.saturating_sub(16))?;
    Ok((db, pages))
}

pub(super) fn source(
    root: &Root,
    directory: CatalogDirectoryCoverage,
    stopped: Option<CatalogScanIssue>,
) -> Result<CatalogCaptureDetails> {
    root.validate()?;
    let pin = root.directory(Path::new(""), false)?;
    let identity = MountIdentity::pinned(&pin)?;
    let filesystem = crate::mounts::inventory()?
        .into_iter()
        .find(|mount| mount.id == identity.mount)
        .map(|mount| mount.filesystem);
    // Storage format granularity, not a measurement of this mount's clock or
    // proof that distinct files/revisions have distinct timestamps. Keep all
    // other drivers unknown, including FUSE adapters and emulated filesystems.
    let mtime_resolution_ns = match filesystem.as_deref() {
        Some("vfat" | "msdos") => Some(2_000_000_000),
        Some("exfat") => Some(10_000_000),
        Some("ntfs3" | "ntfs3_64") => Some(100),
        _ => None,
    };
    Ok(CatalogCaptureDetails {
        directory,
        stopped,
        source_mount: identity,
        namespace: Namespace::current(),
        filesystem,
        mtime_resolution_ns,
    })
}
