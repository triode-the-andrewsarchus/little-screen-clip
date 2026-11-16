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
                tracing::warn!(%error, "Catalog exclusion remains pending");
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
    fn start(
        &mut self,
        pool: &little_clip_core::jobs::BackgroundJobs,
        db: &Store,
        state: &Path,
        closing: &Arc<AtomicBool>,
    ) -> Result<()> {
        // Freeze each pass's upper rowid so new requests cannot indefinitely
        // postpone revisiting an older, formerly reader-blocked intent.
        if self.through.is_none() {
            self.through = db.0.query_row(
                "SELECT max(rowid) FROM entities WHERE kind='catalog-redaction'",
                [],
                |row| row.get(0),
            )?;
        }
        let Some(through) = self.through else {
            return Ok(());
        };
        let next: Option<(i64, String)> = db.0.query_row(
            "SELECT rowid,id FROM entities WHERE kind='catalog-redaction' AND rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 1",
            rusqlite::params![self.after, through], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let Some((rowid, id)) = next else {
            self.after = 0;
            self.through = None;
            return Ok(());
        };
        self.after = rowid;
        let intent = db
            .get::<catalog::redaction::Intent>("catalog-redaction", &id)?
            .ok_or("Pending catalog exclusion disappeared")?;
        let state = Arc::new(Root::open(state)?);
        let cancel = closing.clone();
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
