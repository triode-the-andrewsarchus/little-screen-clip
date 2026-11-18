//! Metadata-only view boundaries include legacy headers and current registrations.
use super::*;
use std::collections::BTreeSet;

const HEADER_BYTES: u64 = 64 * 1024;

#[derive(serde::Deserialize)]
struct Location {
    #[serde(default, with = "little_clip_core::path_serde::optional")]
    mount: Option<PathBuf>,
}
#[derive(serde::Deserialize)]
struct ImageLocation {
    #[serde(default, with = "little_clip_core::path_serde::optional")]
    path: Option<PathBuf>,
}

fn ids(db: &Store, kind: &str, registry: &str) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    for registered in [false, true] {
        let mut after = String::new();
        loop {
            let page = if registered {
                db.ids(registry, &after, 128)?
            } else {
                db.mounted_ids(kind, &after, 128)?
            };
            let Some(last) = page.last() else {
                break;
            };
            after = last.clone();
            found.extend(page);
        }
    }
    Ok(found)
}

fn paths(db: &Store, kind: &str, registry: &str, id: &str) -> Result<Vec<PathBuf>> {
    let header = db
        .get_bounded::<Location>(kind, id, HEADER_BYTES)?
        .ok_or_else(|| format!("Registered {kind} view {id} has no saved header"))?;
    let registered = db.get_bounded::<serde_json::Value>(registry, id, HEADER_BYTES)?;
    let mut paths = header.mount.into_iter().collect::<Vec<_>>();
    if kind == "backup"
        && let Some(value) = registered.as_ref().filter(|value| value.is_object())
    {
        // A boundary is not read/unmount authority. Preserve both paths when
        // header and runtime observation disagree; provider validation stays separate.
        paths.extend(serde_json::from_value::<ImageLocation>(value.clone())?.path);
    }
    if paths.is_empty() && registered.is_some() {
        return Err(format!("Registered {kind} view {id} has no saved mount path").into());
    }
    if paths.iter().any(|path| {
        !path.is_absolute()
            || path.as_os_str().as_encoded_bytes().contains(&0)
            || path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
    }) {
        return Err(format!("Registered {kind} view {id} has an invalid mount path").into());
    }
    Ok(paths)
}

pub(in crate::service) fn mounted_views(db: &Store) -> Result<Vec<PathBuf>> {
    let snapshot =
        db.0.is_autocommit()
            .then(|| db.0.unchecked_transaction())
            .transpose()?;
    let mut views = Vec::new();
    for (kind, registry) in [("catalog", "catalog-mount"), ("backup", "image-mount")] {
        for id in ids(db, kind, registry)? {
            views.extend(
                paths(db, kind, registry, &id).map_err(|error| {
                    format!("Cannot establish managed {kind} view {id}: {error}")
                })?,
            );
        }
    }
    if let Some(snapshot) = snapshot {
        snapshot.commit()?;
    }
    views.sort();
    views.dedup();
    Ok(views)
}
