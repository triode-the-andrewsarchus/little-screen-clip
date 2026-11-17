//! Baseline-based union reconciliation; incomplete scans never authorize deletions.
use crate::{
    DriveConflict, Result, SyncPair, cancelled,
    root::{CopySpec, Root, anchor},
    scan::{self, Entry, Limits},
    store::Store,
};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::symlink},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

fn signature(entry: &Entry) -> Result<String> {
    match entry.kind.as_str() {
        "file" => entry
            .hash
            .clone()
            .ok_or_else(|| "Missing content fingerprint".into()),
        "directory" => Ok("directory".into()),
        "symlink" => Ok(format!(
            "link:{:x}",
            Sha256::digest(
                entry
                    .link
                    .as_ref()
                    .ok_or("Missing symlink target")?
                    .as_os_str()
                    .as_bytes()
            )
        )),
        _ => Err("Unverified tree entry cannot authorize synchronization".into()),
    }
}
pub(crate) fn install(
    from: &Root,
    to: &Root,
    entry: &Entry,
    target: &Path,
    existing: Option<&Entry>,
    cancel: &AtomicBool,
    delta: bool,
) -> Result<u64> {
    match entry.kind.as_str() {
        "directory" => {
            to.ensure_directory(target)?;
            Ok(0)
        }
        "file" => to.copy(
            from,
            CopySpec {
                source: &entry.path,
                destination: target,
                source_hash: entry.hash.as_deref(),
                destination_hash: existing.and_then(|e| e.hash.as_deref()),
                delta,
                basic_metadata: true,
            },
            cancel,
        ),
        "symlink" => {
            cancelled(cancel)?;
            let (source_parent, source) = from.parent(&entry.path, false)?;
            let link = fs::read_link(&source)?;
            if entry.link.as_ref() != Some(&link)
                || !from.same_file(&entry.path, entry.device, entry.inode)?
            {
                return Err("Symlink changed".into());
            }
            let (parent, destination) = to.parent(target, true)?;
            let held = tempfile::tempdir_in(anchor(&parent))?;
            let staged = held.path().join("link");
            symlink(&link, &staged)?;
            if let Some(old) = existing {
                if old.kind != "symlink"
                    || fs::read_link(&destination)?
                        != *old.link.as_ref().ok_or("Missing destination link")?
                {
                    return Err("Symlink destination changed".into());
                }
                to.validate()?;
                from.validate()?;
                fs::rename(&staged, &destination)?;
            } else {
                little_clip_core::file_source::rename_noreplace(&staged, &destination)?;
            }
            drop(source_parent);
            std::fs::File::open(anchor(&parent))?.sync_all()?;
            Ok(0)
        }
        _ => Err("Special or excluded files cannot be synchronized".into()),
    }
}
fn baseline(db: &Store, pair: &str, path: &Path) -> Result<Option<String>> {
    Ok(db
        .0
        .query_row(
            "SELECT hash FROM baseline WHERE pair=?1 AND path=?2",
            params![pair, path.as_os_str().as_bytes()],
            |r| r.get(0),
        )
        .optional()?)
}
fn save_baseline(db: &Store, pair: &str, path: &Path, hash: Option<&str>) -> Result<()> {
    if let Some(hash) = hash {
        db.0.execute("INSERT INTO baseline VALUES(?1,?2,?3) ON CONFLICT(pair,path) DO UPDATE SET hash=excluded.hash",params![pair,path.as_os_str().as_bytes(),hash])?;
    } else {
        db.0.execute(
            "DELETE FROM baseline WHERE pair=?1 AND path=?2",
            params![pair, path.as_os_str().as_bytes()],
        )?;
    }
    Ok(())
}
mod conflicts;
pub(crate) mod fingerprints;
fn delete(root: &Root, entry: &Entry, cancel: &AtomicBool) -> Result<()> {
    match entry.kind.as_str() {
        "file" => root.delete_file(
            &entry.path,
            entry
                .hash
                .as_deref()
                .ok_or("Missing deletion fingerprint")?,
            cancel,
        ),
        "symlink" => root.delete_symlink(
            &entry.path,
            entry.link.as_deref().ok_or("Missing link")?,
            (entry.device, entry.inode),
            cancel,
        ),
        _ => Ok(()),
    }
}
#[derive(Default)]
pub(crate) struct SyncReport {
    pub files: u64,
    pub bytes: u64,
    pub conflicts: u64,
}
struct Reconcile<'a> {
    db: &'a Store,
    pair: &'a SyncPair,
    portable: &'a Root,
    local: &'a Root,
    cancel: &'a AtomicBool,
    report: SyncReport,
}
impl Reconcile<'_> {
    fn path(&mut self, path: &Path, a: Option<Entry>, b: Option<Entry>) -> Result<()> {
        let blocked:bool=self.db.0.query_row("SELECT EXISTS(SELECT 1 FROM sync_blocked WHERE pair=?1 AND substr(?2,1,length(prefix))=prefix)",params![self.pair.id,path.as_os_str().as_bytes()],|r|r.get(0))?;
        if blocked {
            return Ok(());
        }
        cancelled(self.cancel)?;
        self.portable.validate()?;
        self.local.validate()?;
        let old = baseline(self.db, &self.pair.id, path)?;
        let ah = a.as_ref().map(signature).transpose()?;
        let bh = b.as_ref().map(signature).transpose()?;
        if ah == bh {
            if old != ah {
                save_baseline(self.db, &self.pair.id, path, ah.as_deref())?;
            }
            self.resolve_converged(path)?;
            return Ok(());
        }
        let direction = match (&a, &b) {
            (Some(_), None) => Some(true),
            (None, Some(_)) => Some(false),
            (Some(_), Some(_)) if old == ah => Some(false),
            (Some(_), Some(_)) if old == bh => Some(true),
            (Some(a), Some(b)) => {
                self.preserve_conflict(a, b)?;
                None
            }
            _ => None,
        };
        if let Some(portable_to_local) = direction {
            let (from, to, entry, existing, hash) = if portable_to_local {
                (
                    self.portable,
                    self.local,
                    a.as_ref().unwrap(),
                    b.as_ref(),
                    ah.as_deref(),
                )
            } else {
                (
                    self.local,
                    self.portable,
                    b.as_ref().unwrap(),
                    a.as_ref(),
                    bh.as_deref(),
                )
            };
            if existing.is_none()
                && self.pair.deletions
                && old.as_deref() == Some("directory")
                && entry.kind == "directory"
            {
                self.db.0.execute(
                    "INSERT OR IGNORE INTO sync_dirs VALUES(?1,?2)",
                    params![self.pair.id, path.as_os_str().as_bytes()],
                )?;
                return Ok(());
            }
            if existing.is_none()
                && self.pair.deletions
                && old.as_deref() == hash
                && entry.kind != "directory"
            {
                delete(from, entry, self.cancel)?;
                save_baseline(self.db, &self.pair.id, path, None)?;
            } else {
                self.report.bytes += install(from, to, entry, path, existing, self.cancel, false)?;
                self.report.files += 1;
                if existing.is_none() && old.is_some() && old.as_deref() != hash {
                    self.deleted_edit(path, entry, portable_to_local)?;
                }
                save_baseline(self.db, &self.pair.id, path, hash)?;
            }
        }
        Ok(())
    }
}
/// Reconcile only named file changes while both retained root identities are current.
/// Directory changes and unknown coverage deliberately take the complete scan path.
fn comparison_limits(pair: &SyncPair, local: &Root) -> Result<Limits> {
    let mut limits = Limits::default();
    if pair.ram {
        let stats = rustix::fs::fstatvfs(local.directory(Path::new(""), false)?)?;
        limits.hash_bytes = stats.f_blocks.saturating_mul(stats.f_frsize);
    }
    Ok(limits)
}
pub(crate) fn changed(
    db: &Store,
    pair: &SyncPair,
    portable: &Root,
    local: &Root,
    paths: &std::collections::HashSet<PathBuf>,
    cancel: &AtomicBool,
) -> Result<SyncReport> {
    let mut relative = std::collections::BTreeSet::new();
    for path in paths {
        for root in [portable, local] {
            if let Ok(path) = path.strip_prefix(&root.path) {
                relative.insert(path.to_owned());
            }
        }
    }
    let mut entries = Vec::new();
    let max_bytes = comparison_limits(pair, local)?.hash_bytes;
    let mut remaining = max_bytes.saturating_mul(2);
    for path in relative {
        if path.as_os_str().is_empty() {
            return synchronize_with(db, pair, (portable, local), cancel, |_, _| {}, false);
        }
        let mut observe = |root: &Root| -> Result<Option<Entry>> {
            let entry = scan::observe(root, &path, true, remaining.min(max_bytes), cancel)?;
            if let Some(file) = entry.as_ref().filter(|e| e.kind == "file") {
                remaining = remaining.saturating_sub(file.size);
            }
            Ok(entry)
        };
        let a = observe(portable)?;
        let b = observe(local)?;
        if baseline(db, &pair.id, &path)?.as_deref() == Some("directory")
            || a.as_ref().is_some_and(|e| e.kind == "directory")
            || b.as_ref().is_some_and(|e| e.kind == "directory")
        {
            return synchronize_with(db, pair, (portable, local), cancel, |_, _| {}, false);
        }
        entries.push((path, a, b));
    }
    db.0.execute("DELETE FROM sync_blocked WHERE pair=?1", [&pair.id])?;
    let cache_updates = fingerprints::existing(db, pair, portable).map(|provenance| {
        (
            provenance,
            entries
                .iter()
                .map(|(path, a, _)| (path.clone(), a.clone()))
                .collect::<Vec<_>>(),
        )
    });
    let mut reconcile = Reconcile {
        db,
        pair,
        portable,
        local,
        cancel,
        report: SyncReport::default(),
    };
    for (path, a, b) in entries {
        reconcile.path(&path, a, b)?;
    }
    portable.validate()?;
    local.validate()?;
    if let Some((provenance, entries)) = cache_updates {
        fingerprints::refresh(db, pair, portable, &provenance, entries)?;
    }
    Ok(reconcile.report)
}
#[cfg(test)]
pub(crate) fn synchronize(
    db: &Store,
    pair: &SyncPair,
    portable: &Root,
    local: &Root,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<SyncReport> {
    synchronize_with(db, pair, (portable, local), cancel, progress, false)
}
pub(crate) fn synchronize_with(
    db: &Store,
    pair: &SyncPair,
    roots: (&Root, &Root),
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
    reuse: bool,
) -> Result<SyncReport> {
    let (portable, local) = roots;
    if portable.overlap(local) {
        return Err("Sync roots overlap".into());
    }
    let a = format!("{}:portable:next", pair.id);
    let b = format!("{}:local:next", pair.id);
    let provenance = fingerprints::Provenance::capture(pair, portable).ok();
    let mut limits = comparison_limits(pair, local)?;
    for (root, id) in [(portable, &a), (local, &b)] {
        limits.reuse_scan = (reuse && root.reliable_revision_cache()?)
            .then(|| id.strip_suffix(":next").unwrap_or(id).to_owned());
        limits.reuse_generation = None;
        limits.capture_generation = None;
        if id == &a {
            limits.capture_generation = provenance
                .as_ref()
                .map(fingerprints::Provenance::generation);
            fingerprints::admit_reuse(db, pair, provenance.as_ref(), &mut limits);
        }
        let coverage = scan::scan(db, root, id, &limits, cancel, &mut progress)?;
        if !coverage.complete {
            return Err(format!(
                "Synchronization requires complete coverage: {}",
                coverage.message
            )
            .into());
        }
    }
    db.0.execute("DELETE FROM sync_blocked WHERE pair=?1", [&pair.id])?;
    db.0.execute("DELETE FROM sync_dirs WHERE pair=?1", [&pair.id])?;
    let mut after = Vec::new();
    let mut reconcile = Reconcile {
        db,
        pair,
        portable,
        local,
        cancel,
        report: SyncReport::default(),
    };
    loop {
        let paths = {
            let mut statement=db.0.prepare("SELECT path FROM entries WHERE scan IN (?1,?2) AND path>?4 UNION SELECT path FROM baseline WHERE pair=?3 AND path>?4 ORDER BY path LIMIT 128")?;
            statement
                .query_map(params![a, b, pair.id, &after], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if paths.is_empty() {
            break;
        }
        for bytes in paths {
            use std::os::unix::ffi::OsStringExt;
            let path = PathBuf::from(std::ffi::OsString::from_vec(bytes.clone()));
            reconcile.path(
                &path,
                scan::entry(db, &a, &path)?,
                scan::entry(db, &b, &path)?,
            )?;
            after = bytes;
            progress(reconcile.report.files, reconcile.report.bytes);
        }
    }
    reconcile.finish_directories()?;
    portable.validate()?;
    local.validate()?;
    for id in [&a, &b] {
        let previous = id.strip_suffix(":next").ok_or("Invalid scan generation")?;
        let transaction = db.0.unchecked_transaction()?;
        db.clear_scan(previous)?;
        db.0.execute(
            "UPDATE entries SET scan=?1 WHERE scan=?2",
            params![previous, id],
        )?;
        if id == &a {
            fingerprints::publish(db, previous, pair, portable, provenance.as_ref())?;
        }
        transaction.commit()?;
    }
    Ok(reconcile.report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> SyncPair {
        SyncPair {
            local_identity: None,
            id: "pair".into(),
            drive: "drive".into(),
            evidence: "test".into(),
            relative: PathBuf::new(),
            local: PathBuf::new(),
            paused: false,
            deletions: false,
            ram: false,
            last_success: None,
            status: String::new(),
        }
    }
    #[test]
    fn union_offline_conflicts_and_confirmed_deletions_preserve_data() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("portable");
        let b = temp.path().join("local");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("one"), b"one").unwrap();
        fs::write(b.join("two"), b"two").unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut pair = pair();
        let run = |pair: &SyncPair| {
            synchronize(
                &db,
                pair,
                &Root::open(&a).unwrap(),
                &Root::open(&b).unwrap(),
                &cancel,
                |_, _| {},
            )
            .unwrap()
        };
        run(&pair);
        assert_eq!(fs::read(b.join("one")).unwrap(), b"one");
        assert_eq!(fs::read(a.join("two")).unwrap(), b"two");
        fs::write(a.join("one"), b"portable edit").unwrap();
        fs::write(b.join("one"), b"local edit").unwrap();
        assert_eq!(run(&pair).conflicts, 1);
        let conflicts: Vec<DriveConflict> = db.list("conflict", 0, 100).unwrap();
        assert_eq!(conflicts.len(), 1);
        for root in [&a, &b] {
            assert_eq!(
                fs::read(root.join(&conflicts[0].portable)).unwrap(),
                b"portable edit"
            );
            assert_eq!(
                fs::read(root.join(&conflicts[0].local)).unwrap(),
                b"local edit"
            );
        }
        run(&pair);
        assert_eq!(
            db.list::<DriveConflict>("conflict", 0, 100).unwrap().len(),
            1
        );
        fs::remove_file(a.join("two")).unwrap();
        run(&pair);
        assert!(a.join("two").exists());
        pair.deletions = true;
        fs::remove_file(a.join("two")).unwrap();
        run(&pair);
        assert!(!b.join("two").exists());
    }
    #[test]
    fn same_size_same_timestamp_edits_still_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        fs::write(a.join("x"), b"0000").unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let cancel = AtomicBool::new(false);
        let pair = pair();
        let run = || {
            synchronize(
                &db,
                &pair,
                &Root::open(&a).unwrap(),
                &Root::open(&b).unwrap(),
                &cancel,
                |_, _| {},
            )
            .unwrap()
        };
        run();
        let time = fs::metadata(a.join("x")).unwrap().modified().unwrap();
        fs::write(a.join("x"), b"1111").unwrap();
        fs::write(b.join("x"), b"2222").unwrap();
        for root in [&a, &b] {
            fs::File::options()
                .write(true)
                .open(root.join("x"))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
        assert_eq!(run().conflicts, 1);
    }
}
