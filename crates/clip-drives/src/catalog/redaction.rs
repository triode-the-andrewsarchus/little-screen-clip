//! Durable catalog exclusions and snapshot leases; never mutates source payloads.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub id: String,
    pub catalog: String,
    #[serde(with = "little_clip_core::path_serde")]
    pub relative: PathBuf,
    pub requested: u64,
}

/// A mounted view retains its shared lease until the FUSE server exits.
/// Writers never wait for that lease on a coordinator or mutation worker.
pub(crate) struct Lease {
    state: Arc<Root>,
    parent: File,
    file: File,
    pub path: PathBuf,
}
impl Lease {
    pub fn acquire(state: Arc<Root>, id: &str, exclusive: bool) -> Result<Option<Self>> {
        uuid::Uuid::parse_str(id).map_err(|_| "Invalid catalog identifier")?;
        state.validate()?;
        // Older helpers predate file leases. Their observed mounted snapshots
        // still have to be retired before metadata or rowid compaction changes.
        if exclusive {
            let source = PathBuf::from(format!("little-clip-catalog:{id}"));
            if crate::mounts::inventory()?
                .iter()
                .any(|mount| mount.source == source && mount.filesystem.starts_with("fuse"))
            {
                return Ok(None);
            }
        }
        let parent = state.directory(Path::new("catalogs"), false)?;
        let path = crate::root::anchor(&parent).join(format!("{id}.sqlite"));
        let file = little_clip_core::kernel_interface::open_content(&path, false)?;
        if !file.metadata()?.is_file() {
            return Err("Catalog metadata is not an ordinary file".into());
        }
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        // SAFETY: flock borrows one owned live descriptor and has no pointer arguments.
        if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error.into());
        }
        let lease = Self {
            state,
            parent,
            file,
            path,
        };
        lease.validate()?;
        Ok(Some(lease))
    }
    fn validate(&self) -> Result<()> {
        self.state.validate()?;
        let current = std::fs::symlink_metadata(&self.path)?;
        let held = self.file.metadata()?;
        if !current.is_file() || current.dev() != held.dev() || current.ino() != held.ino() {
            return Err("Catalog database was replaced; metadata retained".into());
        }
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        self.validate()?;
        self.file.sync_all()?;
        File::open(crate::root::anchor(&self.parent))?.sync_all()?;
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn release_for_legacy_reader_test(&self) {
        // SAFETY: this test-only unlock borrows a live descriptor, with no pointers.
        assert_eq!(
            unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) },
            0
        );
    }
}

fn enqueue(db: &Store, catalog: &mut Catalog, relative: PathBuf) -> Result<()> {
    use sha2::{Digest, Sha256};
    let id = format!(
        "{}:{:x}",
        catalog.id,
        Sha256::digest(relative.as_os_str().as_bytes())
    );
    if db.get::<Intent>("catalog-redaction", &id)?.is_some() {
        return Ok(());
    }
    let intent = Intent {
        id,
        catalog: catalog.id.clone(),
        relative,
        requested: crate::now(),
    };
    db.put("catalog-redaction", &intent.id, &intent)?;
    catalog.pending_redactions = catalog
        .pending_redactions
        .checked_add(1)
        .ok_or("Too many pending catalog exclusions")?;
    catalog.redaction_error = None;
    db.put("catalog", &catalog.id, catalog)
}

pub(crate) fn set_rule(db: &Store, rule: &DriveRule) -> Result<()> {
    if !little_clip_core::drives::valid_relative(&rule.relative) {
        return Err("Invalid rule subtree".into());
    }
    let transaction = db.0.unchecked_transaction()?;
    db.put("rule", &rule.id, rule)?;
    enqueue_rule(db, rule)?;
    transaction.commit()?;
    Ok(())
}

fn enqueue_rule(db: &Store, rule: &DriveRule) -> Result<()> {
    if rule.catalog {
        return Ok(());
    }
    db.visit::<Catalog>("catalog", |mut catalog| {
        if catalog.drive == rule.drive {
            enqueue(db, &mut catalog, rule.relative.clone())?;
        }
        Ok(())
    })
}

/// Existing exclusions gain cleanup intents once, without replaying source writes.
pub(crate) fn migrate(db: &Store) -> Result<()> {
    match db.get::<u32>("migration", "catalog-redaction")? {
        Some(1) => return Ok(()),
        Some(_) => return Err("Unsupported catalog exclusion migration".into()),
        None => (),
    }
    let transaction = db.0.unchecked_transaction()?;
    db.visit::<DriveRule>("rule", |rule| enqueue_rule(db, &rule))?;
    db.put("migration", "catalog-redaction", &1_u32)?;
    transaction.commit()?;
    Ok(())
}

/// Serialize publication with policy updates, including exclusions added mid-scan.
pub(crate) fn publish(db: &Store, catalog: &Catalog, applied: &[PathBuf]) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    let mut catalog = catalog.clone();
    db.put("catalog", &catalog.id, &catalog)?;
    let drive = catalog.drive.clone();
    visit_exclusions(db, &drive, |relative| {
        if !applied.iter().any(|prefix| relative.starts_with(prefix)) {
            enqueue(db, &mut catalog, relative)?;
        }
        Ok(())
    })?;
    db.remove(super::staging::KIND, &catalog.id)?;
    db.remove("recovery", &catalog.id)?;
    transaction.commit()?;
    Ok(())
}

/// Shares the same catalog policy selection for capture and late-rule publication.
pub(super) fn capture_exclusions(db: &Store, drive: &str) -> Result<Vec<PathBuf>> {
    let transaction = db.0.unchecked_transaction()?;
    let mut exclusions = Vec::new();
    visit_exclusions(db, drive, |relative| {
        if relative.as_os_str().is_empty() {
            return Err("Cataloging is disabled for this drive".into());
        }
        exclusions.push(relative);
        Ok(())
    })?;
    transaction.commit()?;
    Ok(exclusions)
}

/// Shares the same catalog policy selection for capture and late-rule publication.
/// The caller retains its policy transaction; visits stay paginated in Store.
pub(super) fn visit_exclusions(
    db: &Store,
    drive: &str,
    mut visit: impl FnMut(PathBuf) -> Result<()>,
) -> Result<()> {
    db.visit::<DriveRule>("rule", |rule| {
        if rule.drive == drive && !rule.catalog {
            visit(rule.relative)
        } else {
            Ok(())
        }
    })
}

fn redact(db: &Store, intent: &Intent, catalog: &mut Catalog) -> Result<()> {
    if db.get::<u64>("redaction", &intent.id)?.is_some() {
        return Ok(());
    }
    let transaction = db.0.unchecked_transaction()?;
    let prefix = intent.relative.as_os_str().as_bytes();
    let removed = if prefix.is_empty() {
        catalog.root_excluded = true;
        db.0.execute("DELETE FROM entries WHERE scan='catalog'", [])?
    } else {
        let mut lower = prefix.to_vec();
        lower.push(b'/');
        let mut upper = prefix.to_vec();
        upper.push(b'0');
        let removed = db.0.execute(
            "DELETE FROM entries WHERE scan='catalog' AND path>=?1 AND path<?2",
            params![lower, upper],
        )?;
        if let Some(mut entry) = scan::entry(db, "catalog", &intent.relative)? {
            entry.kind = "excluded".into();
            entry.hash = None;
            entry.link = None;
            entry.warning = Some("Metadata removed by an explicit catalog exclusion".into());
            entry.failure = None;
            entry.directory_coverage = None;
            scan::put(db, "catalog", &entry)?;
        }
        removed
    };
    // Completeness of retained ancestors changes when descendants are removed.
    if let Some(details) = &mut catalog.details {
        details.directory.subtree_complete = false;
        details.directory.incomplete_child = true;
    }
    for parent in intent
        .relative
        .ancestors()
        .skip(1)
        .filter(|path| !path.as_os_str().is_empty())
    {
        if let Some(mut entry) = scan::entry(db, "catalog", parent)? {
            if let Some(coverage) = &mut entry.directory_coverage {
                coverage.subtree_complete = false;
                coverage.incomplete_child = true;
            }
            scan::put(db, "catalog", &entry)?;
        }
    }
    catalog.entries = db.0.query_row(
        "SELECT count(*) FROM entries WHERE scan='catalog'",
        [],
        |row| row.get(0),
    )?;
    // This is an explicit edit of a dated observation, never a fresh source capture.
    catalog.redacted = Some(
        crate::now()
            .max(intent.requested)
            .max(catalog.captured)
            .max(catalog.redacted.unwrap_or(0)),
    );
    catalog.redaction_generation = catalog
        .redaction_generation
        .checked_add(1)
        .ok_or("Catalog metadata revision exhausted")?;
    let original_coverage = db
        .get::<String>("capture-coverage", &catalog.id)?
        .unwrap_or_else(|| catalog.coverage.clone());
    db.put("capture-coverage", &catalog.id, &original_coverage)?;
    let removed = db
        .get::<u64>("redaction-removed", &catalog.id)?
        .unwrap_or(0)
        .checked_add(u64::try_from(removed).map_err(|_| "Catalog redaction count overflow")?)
        .ok_or("Catalog redaction count overflow")?;
    db.put("redaction-removed", &catalog.id, &removed)?;
    catalog.complete = false;
    catalog.coverage = format!(
        "{original_coverage}; explicit metadata redaction: {removed} descendant records removed"
    );
    db.put("catalog", &catalog.id, catalog)?;
    db.put("redaction", &intent.id, &catalog.redacted)?;
    transaction.commit()?;
    Ok(())
}

/// Returns false while an old view retains its snapshot. The intent remains durable.
pub(crate) fn apply(state: Arc<Root>, intent: &Intent, cancel: &AtomicBool) -> Result<bool> {
    crate::cancelled(cancel)?;
    if !little_clip_core::drives::valid_relative(&intent.relative) {
        return Err("Invalid saved catalog exclusion".into());
    }
    let Some(lease) = Lease::acquire(state.clone(), &intent.catalog, true)? else {
        return Ok(false);
    };
    let db = Store::existing(&lease.path, false)?;
    lease.validate()?;
    let registry = Store::existing(&state.path.join("drives.sqlite"), false)?;
    let expected: Catalog = registry
        .get("catalog", &intent.catalog)?
        .ok_or("Unknown catalog registration")?;
    let mut catalog: Catalog = db
        .get("catalog", &intent.catalog)?
        .ok_or("Missing catalog header")?;
    if catalog.id != intent.catalog
        || catalog.drive != expected.drive
        || catalog.evidence != expected.evidence
        || catalog.captured != expected.captured
    {
        return Err("Catalog header identity changed".into());
    }
    // Catalog databases have ordinary tables only. Remove deleted bytes as well as
    // their rows, and retire any legacy WAL before editing this dated snapshot.
    db.0.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; PRAGMA secure_delete=ON;",
    )?;
    redact(&db, intent, &mut catalog)?;
    // No FUSE reader retains rowid-derived inodes across this compaction.
    db.0.execute_batch("VACUUM")?;
    lease.sync()?;
    complete(&registry, intent, catalog)?;
    Ok(true)
}

/// Consumes the intent only after its separate catalog file is durably redacted.
fn complete(registry: &Store, intent: &Intent, catalog: Catalog) -> Result<()> {
    let transaction = registry.0.unchecked_transaction()?;
    if registry
        .get::<Intent>("catalog-redaction", &intent.id)?
        .is_some()
    {
        let mut current: Catalog = registry
            .get("catalog", &intent.catalog)?
            .ok_or("Catalog registration disappeared")?;
        current.entries = catalog.entries;
        current.complete = catalog.complete;
        current.coverage = catalog.coverage;
        current.details = catalog.details;
        current.redacted = catalog.redacted;
        current.redaction_generation = catalog.redaction_generation;
        current.root_excluded = catalog.root_excluded;
        current.pending_redactions = current
            .pending_redactions
            .checked_sub(1)
            .ok_or("Catalog cleanup accounting changed")?;
        current.redaction_error = None;
        registry.put("catalog", &current.id, &current)?;
        registry.remove("catalog-redaction", &intent.id)?;
    }
    transaction.commit()?;
    Ok(())
}

pub(crate) fn failure(db: &Store, intent: &Intent, message: String) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    if db.get::<Intent>("catalog-redaction", &intent.id)?.is_some()
        && let Some(mut catalog) = db.get::<Catalog>("catalog", &intent.catalog)?
    {
        catalog.redaction_error = Some(message);
        db.put("catalog", &catalog.id, &catalog)?;
    }
    transaction.commit()?;
    Ok(())
}
