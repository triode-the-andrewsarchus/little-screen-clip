//! Bounded durable pending work; acknowledgements cannot consume newer events.
use super::*;
use rusqlite::{OptionalExtension, params};
use std::{collections::HashSet, os::unix::ffi::OsStrExt};

#[derive(Clone)]
pub(super) struct Checkpoint {
    pair: String,
    through: i64,
    paths: Option<Arc<HashSet<PathBuf>>>,
}

fn transaction_required(db: &Store) -> Result<()> {
    if db.0.is_autocommit() {
        return Err("Pending RAM changes require a retained transaction".into());
    }
    Ok(())
}

fn insert(db: &Store, pair: &str, path: &Path) -> Result<()> {
    // AUTOINCREMENT prevents old checkpoint replay consuming events after rows are deleted.
    db.0.execute(
        "INSERT OR REPLACE INTO ram_pending(pair,path) VALUES(?1,?2)",
        params![pair, path.as_os_str().as_bytes()],
    )?;
    Ok(())
}

pub(super) fn record<'a>(
    db: &Store,
    pair: &str,
    paths: impl Iterator<Item = &'a Path>,
    mut full: bool,
) -> Result<()> {
    transaction_required(db)?;
    let (mut count, mut bytes): (usize,usize) = db.0.query_row("SELECT count(*),coalesce(sum(length(path)),0) FROM ram_pending WHERE pair=?1 AND path<>x''",[pair],|row|Ok((row.get(0)?,row.get(1)?)))?;
    for path in paths {
        full |= !record_path(db, pair, path, &mut count, &mut bytes)?;
    }
    if full {
        insert(db, pair, Path::new(""))?;
    }
    Ok(())
}

/// Returns false when one observed path requires a complete comparison instead.
fn record_path(
    db: &Store,
    pair: &str,
    path: &Path,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<bool> {
    if !valid_relative(path) {
        return Err("Invalid RAM pending subtree".into());
    }
    if path.as_os_str().is_empty() {
        return Ok(false);
    }
    let exists =
        db.0.query_row(
            "SELECT 1 FROM ram_pending WHERE pair=?1 AND path=?2",
            params![pair, path.as_os_str().as_bytes()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    let size = path.as_os_str().as_bytes().len();
    if !exists
        && (*count >= crate::watch::MAX_CHANGED_PATHS
            || bytes.saturating_add(size) > crate::watch::MAX_CHANGED_BYTES)
    {
        return Ok(false);
    }
    insert(db, pair, path)?;
    if !exists {
        *count += 1;
        *bytes += size;
    }
    Ok(true)
}

pub(super) fn snapshot(db: &Store, pair: &str) -> Result<RamPending> {
    Ok(db.0.query_row("SELECT coalesce(sum(path<>x''),0),coalesce(max(path=x''),0) FROM ram_pending WHERE pair=?1",[pair],|row| Ok(RamPending { paths: row.get(0)?, full_check_required: row.get(1)? }))?)
}

pub(super) fn checkpoint(
    db: &Store,
    pair: &str,
    paths: Option<Arc<HashSet<PathBuf>>>,
) -> Result<Checkpoint> {
    let through = db.0.query_row(
        "SELECT coalesce(max(id),0) FROM ram_pending WHERE pair=?1",
        [pair],
        |row| row.get(0),
    )?;
    Ok(Checkpoint {
        pair: pair.into(),
        through,
        paths,
    })
}

pub(super) fn acknowledge(db: &Store, checkpoint: &Checkpoint) -> Result<()> {
    transaction_required(db)?;
    if let Some(paths) = &checkpoint.paths {
        for path in paths.iter().filter(|path| !path.as_os_str().is_empty()) {
            db.0.execute(
                "DELETE FROM ram_pending WHERE pair=?1 AND id<=?2 AND path=?3",
                params![
                    checkpoint.pair,
                    checkpoint.through,
                    path.as_os_str().as_bytes()
                ],
            )?;
        }
    } else {
        db.0.execute(
            "DELETE FROM ram_pending WHERE pair=?1 AND id<=?2",
            params![checkpoint.pair, checkpoint.through],
        )?;
    }
    Ok(())
}

pub(super) fn recover(db: &Store) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    db.visit::<RamSession>("ram", |session| insert(db, &session.id, Path::new("")))?;
    transaction.commit()?;
    Ok(())
}

impl Owner {
    pub(super) fn track_ram_changes(
        &self,
        paths: Option<&HashSet<PathBuf>>,
        full: bool,
    ) -> Result<bool> {
        let full = full
            || self
                .shared
                .lock()
                .map(|shared| shared.ram_pending_error.is_some())
                .unwrap_or(true);
        let result: Result<()> = (|| {
            let transaction = self.db.0.unchecked_transaction()?;
            self.db.visit::<RamSession>("ram", |session| {
                let relative = paths
                    .into_iter()
                    .flat_map(|paths| paths.iter())
                    .filter_map(|path| path.strip_prefix(&session.path).ok());
                record(&self.db, &session.id, relative, full)
            })?;
            transaction.commit()?;
            Ok(())
        })();
        if let Ok(mut shared) = self.shared.lock() {
            shared.ram_pending_error = result.as_ref().err().map(ToString::to_string);
        }
        result.map(|()| full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    fn record_paths(db: &Store, pair: &str, paths: &[&str], full: bool) {
        let tx = db.0.unchecked_transaction().unwrap();
        record(db, pair, paths.iter().map(Path::new), full).unwrap();
        tx.commit().unwrap();
    }
    fn ack(db: &Store, checkpoint: &Checkpoint) {
        let tx = db.0.unchecked_transaction().unwrap();
        acknowledge(db, checkpoint).unwrap();
        tx.commit().unwrap();
    }
    #[test]
    fn acknowledgements_preserve_newer_changes_and_do_not_reuse_deleted_sequences() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        record_paths(&db, "ram", &["x", "y"], false);
        record_paths(&db, "other", &["larger-row"], false);
        let cutoff = checkpoint(&db, "ram", None).unwrap();
        let other = checkpoint(&db, "other", None).unwrap();
        ack(&db, &other);
        record_paths(&db, "ram", &["x"], false);
        ack(&db, &cutoff);
        assert_eq!(snapshot(&db, "ram").unwrap().paths, 1);
        let partial = checkpoint(
            &db,
            "ram",
            Some(Arc::new(HashSet::from([PathBuf::from("x")]))),
        )
        .unwrap();
        record_paths(&db, "ram", &[], true);
        ack(&db, &partial);
        let remaining = snapshot(&db, "ram").unwrap();
        assert_eq!(remaining.paths, 0);
        assert!(remaining.full_check_required);
        let full = checkpoint(&db, "ram", None).unwrap();
        ack(&db, &full);
        assert!(!snapshot(&db, "ram").unwrap().full_check_required);
        record_paths(&db, "ram", &["after-empty-table"], false);
        ack(&db, &full);
        assert_eq!(snapshot(&db, "ram").unwrap().paths, 1);
    }
    #[test]
    fn pending_work_is_bounded_and_partial_checks_cannot_clear_unknown_coverage() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let names = (0..4100)
            .map(|n| PathBuf::from(format!("file-{n}")))
            .collect::<Vec<_>>();
        let tx = db.0.unchecked_transaction().unwrap();
        record(&db, "ram", names.iter().map(PathBuf::as_path), false).unwrap();
        tx.commit().unwrap();
        let status = snapshot(&db, "ram").unwrap();
        assert_eq!(status.paths, 4096);
        assert!(status.full_check_required);
        let partial = checkpoint(
            &db,
            "ram",
            Some(Arc::new(HashSet::from([names[0].clone()]))),
        )
        .unwrap();
        ack(&db, &partial);
        assert_eq!(snapshot(&db, "ram").unwrap().paths, 4095);
        assert!(snapshot(&db, "ram").unwrap().full_check_required);
    }
    #[test]
    fn oversized_paths_require_a_full_check_without_losing_raw_names() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"raw-\xff".to_vec()));
        let oversized = PathBuf::from("a".repeat(crate::watch::MAX_CHANGED_BYTES + 1));
        let tx = db.0.unchecked_transaction().unwrap();
        record(
            &db,
            "ram",
            [raw.as_path(), oversized.as_path()].into_iter(),
            false,
        )
        .unwrap();
        tx.commit().unwrap();
        let pending = snapshot(&db, "ram").unwrap();
        assert_eq!(pending.paths, 1);
        assert!(pending.full_check_required);
        let partial = checkpoint(&db, "ram", Some(Arc::new(HashSet::from([raw])))).unwrap();
        ack(&db, &partial);
        assert_eq!(snapshot(&db, "ram").unwrap().paths, 0);
        assert!(snapshot(&db, "ram").unwrap().full_check_required);
    }
}
