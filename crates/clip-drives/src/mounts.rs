//! Catalog/managed mount ownership uses core's shared lossless namespace inventory.
use crate::Result;
pub(crate) use little_clip_core::mount_inventory::{Mount, inventory};
use std::path::Path;

pub(crate) fn catalog_mount(target: &Path, id: &str) -> Result<Option<Mount>> {
    owned_mount(target, "little-clip-catalog", id)
}
pub(crate) fn owned_mount(target: &Path, prefix: &str, id: &str) -> Result<Option<Mount>> {
    let matches = inventory()?
        .into_iter()
        .filter(|m| m.target == target)
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err("Overlapping mounts make filesystem ownership ambiguous".into());
    }
    Ok(matches.into_iter().next().filter(|m| {
        m.source == Path::new(&format!("{prefix}:{id}")) && m.filesystem.starts_with("fuse")
    }))
}
