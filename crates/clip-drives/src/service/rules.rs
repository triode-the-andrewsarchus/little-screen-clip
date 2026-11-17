//! Persistent rule addresses are independent of device-node and spelling aliases.
use super::*;

pub(super) fn drive(drives: Vec<Drive>, selector: &str) -> Result<String> {
    if selector.is_empty() || selector.contains('\0') {
        return Err("Invalid policy drive identity".into());
    }
    if !selector.starts_with("/dev/") && !drives.iter().any(|drive| drive.id == selector) {
        // Stable identities remain editable while their media is absent.
        return Ok(selector.into());
    }
    let selected = discovery::select(drives.clone(), selector)?;
    // A device node can select one clone for an explicit operation, but a
    // persistent policy would subsequently apply to every clone with that ID.
    Ok(discovery::select(drives, &selected.id)?.id)
}

fn relative(path: &Path) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if !valid_relative(path) || path.as_os_str().as_bytes().contains(&0) {
        return Err("Invalid rule subtree".into());
    }
    Ok(path.components().collect())
}

pub(super) fn bind(request: DriveRequest, inventory: &Inventory) -> Result<DriveRequest> {
    match request {
        DriveRequest::Rule {
            drive: selector,
            relative: path,
            catalog,
            index,
        } => Ok(DriveRequest::Rule {
            drive: drive(inventory()?, &selector)?,
            relative: relative(&path)?,
            catalog,
            index,
        }),
        request => Ok(request),
    }
}

pub(super) fn get(db: &Store, drive: &str, path: &Path) -> Result<Option<DriveRule>> {
    let path = relative(path)?;
    let id = rule_id(drive, &path);
    let rule: Option<DriveRule> = db.get("rule", &id)?;
    if rule
        .as_ref()
        .is_some_and(|rule| rule.id != id || rule.drive != drive || rule.relative != path)
    {
        return Err("Saved rule address does not match its key".into());
    }
    Ok(rule)
}

pub(super) fn migrate(db: &Store) -> Result<()> {
    match db.get::<u32>("migration", "rule-address")? {
        Some(1) => return Ok(()),
        Some(_) => return Err("Unsupported rule address migration".into()),
        None => (),
    }
    let transaction = db.0.unchecked_transaction()?;
    db.visit::<DriveRule>("rule", |old| migrate_rule(db, old))?;
    db.put("migration", "rule-address", &1_u32)?;
    transaction.commit()?;
    Ok(())
}

// Keep the per-record merge separate from the atomic migration boundary.
fn migrate_rule(db: &Store, old: DriveRule) -> Result<()> {
    let path = relative(&old.relative)?;
    let id = rule_id(&old.drive, &path);
    if id == old.id && path.as_os_str() == old.relative.as_os_str() {
        return Ok(());
    }
    let stored: DriveRule = db
        .get("rule", &old.id)?
        .ok_or("Saved rule key is missing")?;
    if stored.drive != old.drive || stored.relative != old.relative {
        return Err("Saved rule address does not match its key".into());
    }
    let mut rule = DriveRule {
        id: id.clone(),
        relative: path,
        ..old.clone()
    };
    if let Some(existing) = get(db, &rule.drive, &rule.relative)? {
        // Legacy aliases may disagree. Preserve every existing exclusion
        // until the person explicitly saves a replacement policy.
        rule.catalog &= existing.catalog;
        rule.index &= existing.index;
    }
    db.put("rule", &id, &rule)?;
    if id != old.id {
        db.remove("rule", &old.id)?;
    }
    Ok(())
}

// Ephemeral routing only; persisted ownership remains in the shared policy file.
#[derive(Default)]
pub(super) struct IndexSources {
    views: HashMap<String, Vec<crate::mounts::Mount>>,
    roots: HashMap<String, Vec<PathBuf>>,
}
impl IndexSources {
    pub(super) fn observed(drives: &[Drive], mounts: &[crate::mounts::Mount]) -> Self {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let mut result = Self::default();
        for drive in drives {
            let views = if matches!(
                drive.filesystem.as_str(),
                "vfat" | "exfat" | "ntfs" | "ntfs3"
            ) {
                std::fs::metadata(&drive.device)
                    .ok()
                    .filter(|m| m.file_type().is_block_device())
                    .map(|m| {
                        discovery::device_mounts(mounts, m.rdev())
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if views.is_empty() {
                result
                    .roots
                    .entry(drive.id.clone())
                    .or_default()
                    .extend(drive.mount.clone());
            } else {
                result
                    .views
                    .entry(drive.id.clone())
                    .or_default()
                    .extend(views);
            }
        }
        result
    }
    pub(super) fn requests(&self, rule: &DriveRule) -> Vec<(String, Option<PathBuf>)> {
        if rule.index {
            return Vec::new();
        }
        let owner = format!("rule:{}", rule.id);
        let physical = Path::new("/").join(&rule.relative);
        let mut paths = self
            .views
            .get(&rule.drive)
            .into_iter()
            .flatten()
            .filter_map(|mount| mount.exclusion(&physical))
            .collect::<Vec<_>>();
        paths.extend(
            self.roots
                .get(&rule.drive)
                .into_iter()
                .flatten()
                .map(|root| root.join(&rule.relative)),
        );
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            vec![(owner, None)]
        } else {
            paths
                .into_iter()
                .map(|path| (owner.clone(), Some(path)))
                .collect()
        }
    }
}
