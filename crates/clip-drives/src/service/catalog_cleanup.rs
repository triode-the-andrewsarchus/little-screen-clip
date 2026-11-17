//! Bounded metadata maintenance shares the retained bulk pool, never RAM workers.
use super::*;
use little_clip_core::jobs::task::Task;
use rusqlite::OptionalExtension;

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
                tracing::warn!(%error, "Catalog metadata maintenance remains pending");
            }
            self.next = Instant::now() + Duration::from_secs(1);
        }
        if self.busy() || !admit || Instant::now() < self.next {
            return;
        }
        self.next = Instant::now() + Duration::from_secs(1);
        if let Err(error) = self.start(pool, db, state, closing) {
            tracing::warn!(%error, "Could not admit catalog metadata cleanup");
        }
    }
    fn next(&mut self, db: &Store) -> Result<Option<(String, String)>> {
        // Freeze each pass's upper rowid so new requests cannot indefinitely
        // postpone revisiting an older, formerly reader-blocked intent.
        if self.through.is_none() {
            self.through = db.0.query_row(
                "SELECT max(rowid) FROM entities WHERE kind IN ('catalog-redaction','catalog-capture')",
                [],
                |row| row.get(0),
            )?;
        }
        let Some(through) = self.through else {
            return Ok(None);
        };
        let next: Option<(i64, String, String)> = db.0.query_row(
            "SELECT rowid,kind,id FROM entities WHERE kind IN ('catalog-redaction','catalog-capture') AND rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 1",
            rusqlite::params![self.after, through], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        let Some((rowid, kind, id)) = next else {
            self.after = 0;
            self.through = None;
            return Ok(None);
        };
        self.after = rowid;
        Ok(Some((kind, id)))
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
        if kind == catalog::staging::KIND {
            self.task = Some(write_task(pool, move || {
                catalog::staging::apply(state, &id, &cancel)
            }));
            return Ok(());
        }
        let intent = db
            .get::<catalog::redaction::Intent>("catalog-redaction", &id)?
            .ok_or("Pending catalog exclusion disappeared")?;
        self.task = Some(write_task(pool, move || {
            let result = catalog::redaction::apply(state.clone(), &intent, &cancel);
            if let Err(error) = &result {
                state.validate()?;
                let db = Store::existing(&state.path.join("drives.sqlite"), false)?;
                catalog::redaction::failure(&db, &intent, error.to_string())?;
            }
            result
        }));
        Ok(())
    }
}
