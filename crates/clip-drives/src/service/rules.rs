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
