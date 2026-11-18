//! Bounded storage recovery shares the retained bulk pool, never RAM workers.
use super::*;
use little_clip_core::jobs::task::Task;
use rusqlite::OptionalExtension;

const CANDIDATES: &str = "kind IN ('catalog-redaction','catalog-capture','backup-publication','backup-prune','image-control') OR (kind='image-loop' AND CASE WHEN json_valid(json) THEN coalesce(json_extract(json,'$.node.boot') <> ?1,1) ELSE 1 END)";

pub(super) struct Cleanup {
    task: Option<Task<Result<bool>>>,
    next: Instant,
    after: i64,
    through: Option<i64>,
}
impl Default for Cleanup {
    fn default() -> Self {
        Self {
            task: None,
            next: Instant::now(),
            after: 0,
            through: None,
        }
    }
}
impl Cleanup {
    pub(super) fn busy(&self) -> bool {
        self.task.is_some()
    }

    pub(super) fn poll(
        &mut self,
        pool: &little_clip_core::jobs::BackgroundJobs,
        db: &Store,
        state: &Path,
        closing: &Arc<AtomicBool>,
        admit: bool,
    ) {
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task.take();
            if let Err(error) = write_result(result) {
                tracing::warn!(%error, "Storage maintenance remains pending");
            }
            self.next = Instant::now() + Duration::from_secs(1);
        }
        if self.busy() || !admit || Instant::now() < self.next {
            return;
        }
        self.next = Instant::now() + Duration::from_secs(1);
        if let Err(error) = self.start(pool, db, state, closing) {
            tracing::warn!(%error, "Could not admit storage maintenance");
        }
    }
    fn next(&mut self, db: &Store) -> Result<Option<(String, String)>> {
        let boot = kernel_interface::current_boot_id()
            .ok()
            .map(|id| id.to_string());
        // Freeze each pass's upper rowid so new requests cannot indefinitely
        // postpone revisiting an older, formerly reader-blocked intent.
        if self.through.is_none() {
            self.through = db.0.query_row(
                &format!("SELECT max(rowid) FROM entities WHERE ({CANDIDATES})"),
                [&boot],
                |row| row.get(0),
            )?;
        }
        let Some(through) = self.through else {
            return Ok(None);
        };
        let next: Option<(i64, String, Option<String>)> = db.0.query_row(
            &format!("SELECT rowid,kind,CASE WHEN length(CAST(id AS BLOB))<=128 THEN id END FROM entities WHERE ({CANDIDATES}) AND rowid>?2 AND rowid<=?3 ORDER BY rowid LIMIT 1"),
            rusqlite::params![boot, self.after, through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        let Some((rowid, kind, id)) = next else {
            self.after = 0;
            self.through = None;
            return Ok(None);
        };
        self.after = rowid;
        Ok(Some((
            kind,
            id.ok_or("Maintenance identifier exceeds its decoding budget")?,
        )))
    }
    fn start(
        &mut self,
        pool: &little_clip_core::jobs::BackgroundJobs,
        db: &Store,
        state: &Path,
        closing: &Arc<AtomicBool>,
    ) -> Result<()> {
        let Some((kind, id)) = self.next(db)? else {
            return Ok(());
        };
        let state = Arc::new(Root::open(state)?);
        let cancel = closing.clone();
        self.task = Some(write_task(pool, move || run(state, kind, id, cancel)));
        Ok(())
    }
}

fn run(state: Arc<Root>, kind: String, id: String, cancel: Arc<AtomicBool>) -> Result<bool> {
    if kind == catalog::staging::KIND {
        return catalog::staging::apply(state, &id, &cancel);
    }
    state.validate()?;
    let db = Store::existing(&state.path.join("drives.sqlite"), false)?;
    if kind == image_controls::KIND {
        return image_controls::recover(&db, &id, &cancel);
    }
    if matches!(
        kind.as_str(),
        "backup-publication" | "backup-prune" | "image-loop"
    ) {
        return backup::recover_record(&db, &kind, &id, &cancel);
    }
    if kind != "catalog-redaction" {
        return Err("Unknown storage maintenance record".into());
    }
    let intent = db
        .get::<catalog::redaction::Intent>("catalog-redaction", &id)?
        .ok_or("Pending catalog exclusion disappeared")?;
    let result = catalog::redaction::apply(state.clone(), &intent, &cancel);
    if let Err(error) = &result {
        state.validate()?;
        catalog::redaction::failure(&db, &intent, error.to_string())?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_boot_attachments_do_not_delay_an_old_record_beyond_status_pagination() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state")).unwrap();
        let boot = kernel_interface::current_boot_id().unwrap();
        let transaction = db.0.unchecked_transaction().unwrap();
        for number in 0..2202 {
            db.put(
                "image-loop",
                &format!("current-{number}"),
                &serde_json::json!({"node":{"boot":boot}}),
            )
            .unwrap();
        }
        db.put(
            "image-loop",
            "old-boot",
            &serde_json::json!({"node":{"boot":uuid::Uuid::new_v4()}}),
        )
        .unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            Cleanup::default().next(&db).unwrap(),
            Some(("image-loop".into(), "old-boot".into()))
        );
    }
}
