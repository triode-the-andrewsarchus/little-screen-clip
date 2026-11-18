//! Bounded reconciliation tasks retain their roots until real write completion.
use super::*;
use little_clip_core::jobs::task::Task;
use std::collections::HashSet;
mod enrollment;
#[cfg(test)]
mod population_tests;
pub(super) mod preparation;

#[derive(Clone)]
pub(super) struct Plan {
    pub(super) pair: SyncPair,
    portable: Arc<Root>,
    local: Arc<Root>,
    // Matching retained roots allow named paths to be freshly compared even
    // when unrelated directories lack watches. Full-scan fingerprint reuse
    // additionally requires complete observation coverage.
    cached: bool,
    observed: bool,
    storage: Option<policy::Storage>,
}
#[derive(Clone)]
pub(super) struct Intent {
    pub queued: Instant,
    pub pair: String,
    pub operation: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub paths: Option<Arc<HashSet<PathBuf>>>,
    pub initialize: bool,
    pub save: Option<Arc<super::ram_save::Save>>,
    pub writeback: Option<super::ram_pending::Checkpoint>,
}
pub(super) struct Pending {
    pub drive: String,
    pub intent: Intent,
    pub(super) plan: Plan,
    epoch: u64,
    task: Task<Result<String>>,
}
/// Persists completed reconciliation effects for both normal jobs and pre-unmount flushes.
pub(super) fn persist_result(db: &Store, intent: &Intent, result: &Result<String>) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    if let Some(mut pair) = db.get::<SyncPair>("pair", &intent.pair)? {
        match result {
            Ok(message) => {
                pair.last_success = Some(now());
                pair.status = message.clone();
                if intent.initialize {
                    pair.paused = false;
                }
            }
            Err(error) => {
                pair.status = error.to_string();
                if intent.operation.is_some() && pair.last_success.is_none() {
                    pair.paused = true;
                }
            }
        }
        db.put("pair", &pair.id, &pair)?;
        super::ram_pending::persist_session(db, &pair, intent.writeback.as_ref(), result.is_ok())?;
    }
    transaction.commit()?;
    Ok(())
}

impl Plan {
    fn validate_roots(&self) -> Result<()> {
        self.portable.validate()?;
        self.local.validate()?;
        if let Some(storage) = &self.storage {
            storage.validate(
                &self.portable,
                &self.local,
                self.pair.ram.then_some(self.pair.id.as_str()),
            )?;
        }
        Ok(())
    }
    pub(super) fn run_with_cancel(
        &self,
        db: &Store,
        intent: &Intent,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        cancelled(cancel)?;
        if let Some(save) = &intent.save {
            return save.run(db, cancel, progress);
        }
        self.validate_roots()?;
        if !self.pair.ram && self.pair.last_success.is_none() {
            policy::validate_pair_targets(
                &db.list("pair", 0, 1000)?,
                &self.local,
                Some(&self.pair.id),
            )?;
        }
        let passes = if intent.initialize { 2 } else { 1 };
        let mut files = 0;
        let mut conflicts = 0;
        for _ in 0..passes {
            let result = if let Some(paths) = intent.paths.as_ref().filter(|_| self.cached) {
                sync::changed(db, &self.pair, &self.portable, &self.local, paths, cancel)?
            } else {
                sync::synchronize_with(
                    db,
                    &self.pair,
                    (&self.portable, &self.local),
                    cancel,
                    &mut *progress,
                    self.cached
                        && self.observed
                        && self.portable.reliable_revision_cache()?
                        && self.local.reliable_revision_cache()?,
                )?
            };
            files += result.files;
            conflicts += result.conflicts;
        }
        if self.pair.ram {
            self.portable.flush()?;
        }
        Ok(format!("{files} files copied · {conflicts} conflicts"))
    }
    pub(super) fn writeback_checkpoint(
        &self,
        db: &Store,
        intent: &Intent,
    ) -> Result<Option<super::ram_pending::Checkpoint>> {
        if !self.pair.ram || intent.save.is_some() {
            return Ok(None);
        }
        let paths = intent.paths.as_ref().filter(|_| self.cached).map(|paths| {
            Arc::new(
                paths
                    .iter()
                    .filter_map(|path| {
                        path.strip_prefix(&self.local.path)
                            .or_else(|_| path.strip_prefix(&self.portable.path))
                            .ok()
                            .map(Path::to_path_buf)
                    })
                    .collect(),
            )
        });
        super::ram_pending::checkpoint(db, &self.pair.id, paths).map(Some)
    }
}
impl Owner {
    pub(super) fn drain_sync(&mut self) {
        let mut waiting = std::mem::take(&mut self.sync_waiting);
        waiting.extend(std::mem::take(&mut self.sync_auto).into_values());
        for intent in waiting {
            intent.cancel.store(true, Ordering::Release);
            self.sync_result(
                &intent,
                Err("Daemon stopping; queued reconciliation cancelled".into()),
                None,
            );
        }
        for job in self.sync_jobs.values() {
            job.intent.cancel.store(true, Ordering::Release);
        }
        for job in self.sync_preparing.values() {
            job.intent.cancel.store(true, Ordering::Release);
        }
        while !self.sync_jobs.is_empty() || !self.sync_preparing.is_empty() {
            self.poll_sync();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub(super) fn watch_changes(&mut self, periodic: bool) -> crate::watch::Changes {
        let unavailable = self.watch.is_none();
        if unavailable && periodic {
            self.watch = Watch::new().ok();
        }
        let mut changes = self
            .watch
            .as_mut()
            .map(|watch| watch.changes(periodic))
            .unwrap_or_default();
        // Missing coverage takes the periodic fallback, rather than resubmitting
        // a complete comparison every coordinator tick. Creation/resumption
        // invalidates earlier unobserved cached roots through the full flag.
        changes.full |= unavailable && periodic;
        changes
    }
    pub(super) fn start_reconciliation(&mut self, envelope: Envelope) {
        if self.sync_waiting.len() >= 16 {
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err("Reconciliation queue is full; retry admission".into()),
            );
            return;
        }
        let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.prepare_reconciliation(&envelope.request, &envelope.cancel)
        }))
        .unwrap_or_else(|_| {
            Err("Reconciliation setup failed; retained storage needs recovery".into())
        });
        match prepared {
            Ok((pair, initialize, save)) => self.queue_sync(reconciliation::Intent {
                queued: Instant::now(),
                pair,
                initialize,
                operation: Some(envelope.id),
                cancel: envelope.cancel,
                paths: None,
                save,
                writeback: None,
            }),
            Err(error) => self.finish_operation(&envelope.id, &envelope.cancel, Err(error)),
        }
    }
    pub(super) fn prepare_reconciliation(
        &mut self,
        request: &DriveRequest,
        cancel: &AtomicBool,
    ) -> Result<(String, bool, Option<Arc<super::ram_save::Save>>)> {
        cancelled(cancel)?;
        match request {
            DriveRequest::Sync { pair } | DriveRequest::FlushRam { session: pair } => {
                let initial = self
                    .db
                    .get::<RamSession>("ram", pair)?
                    .is_some_and(|s| !s.ready);
                if self.db.get::<RamSession>("ram", pair)?.is_some() {
                    self.mount_ram(pair)?;
                }
                Ok((pair.clone(), initial, None))
            }
            DriveRequest::SaveRam {
                session,
                destination,
            } => {
                if self.db.get::<SyncPair>("pair", session)?.is_none() {
                    return Err("RAM enrollment is missing".into());
                }
                Ok((
                    session.clone(),
                    false,
                    Some(self.prepare_ram_save(session, destination)?),
                ))
            }
            _ => Err("Invalid reconciliation request".into()),
        }
    }
    pub(super) fn prepare_sync_pair(&self, pair: SyncPair, drive: &Drive) -> Result<Plan> {
        preparation::Input::capture(self, pair, None)?.prepare_pair(drive)
    }
    pub(super) fn queue_sync(&mut self, intent: Intent) {
        if intent.operation.is_some() {
            if self.sync_waiting.len() >= 16 {
                self.sync_result(
                    &intent,
                    Err("Reconciliation queue is full; retry admission".into()),
                    None,
                );
            } else {
                self.sync_waiting.push_back(intent);
            }
        } else {
            self.sync_auto
                .entry(intent.pair.clone())
                .and_modify(|old| match (&mut old.paths, &intent.paths) {
                    (Some(a), Some(b)) if Arc::ptr_eq(a, b) => (),
                    (Some(a), Some(b))
                        if a.len().saturating_add(b.len()) <= 4096
                            && a.iter()
                                .chain(b.iter())
                                .map(|p| p.as_os_str().as_encoded_bytes().len())
                                .sum::<usize>()
                                <= 1024 * 1024 =>
                    {
                        if let Some(paths) = Arc::get_mut(a) {
                            paths.extend(b.iter().cloned());
                        } else {
                            old.paths = None;
                        }
                    }
                    _ => old.paths = None,
                })
                .or_insert(intent);
        }
    }
    pub(super) fn poll_sync(&mut self) {
        let finished = self
            .sync_jobs
            .iter()
            .filter_map(|(id, job)| job.task.poll().map(|result| (id.clone(), result)))
            .collect::<Vec<_>>();
        for (id, result) in finished {
            let job = self
                .sync_jobs
                .remove(&id)
                .expect("polled retained reconciliation");
            let result = result.unwrap_or_else(|e| {
                Err(format!("Reconciliation worker failed: {e}; completed writes retained").into())
            });
            let roots = (job.epoch == self.sync_epoch).then_some(job.plan);
            self.sync_result(&job.intent, result, roots);
        }
        self.poll_sync_preparation();
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let mut ready = std::mem::take(&mut self.sync_waiting);
        let mut automatic = std::mem::take(&mut self.sync_auto)
            .into_values()
            .collect::<Vec<_>>();
        automatic.sort_by_key(|intent| intent.queued);
        ready.extend(automatic);
        while let Some(intent) = ready.pop_front() {
            if intent.cancel.load(Ordering::Acquire) {
                self.sync_result(
                    &intent,
                    Err("Cancelled before reconciliation admission".into()),
                    None,
                );
                continue;
            }
            if self.sync_jobs.len() + self.sync_preparing.len() >= 4 {
                self.queue_sync(intent);
                continue;
            }
            if self.sync_jobs.contains_key(&intent.pair)
                || self.sync_preparing.contains_key(&intent.pair)
            {
                self.queue_sync(intent);
                continue;
            }
            if let Ok(Some(pair)) = self.db.get::<SyncPair>("pair", &intent.pair)
                && self.check_busy(&pair.drive).is_err()
            {
                self.queue_sync(intent);
                continue;
            }
            let pair = match self
                .db
                .get::<SyncPair>("pair", &intent.pair)
                .and_then(|pair| pair.ok_or_else(|| "Unknown sync pair".into()))
            {
                Ok(pair) => pair,
                Err(error) => {
                    self.sync_result(&intent, Err(error), None);
                    continue;
                }
            };
            if intent.operation.is_none() && pair.paused {
                continue;
            }
            if intent.operation.is_none() && self.waiting_for_drive(&pair.drive) {
                self.queue_sync(intent);
                continue;
            }
            if let Some(save) = &intent.save {
                let plan = Plan {
                    pair,
                    portable: save.source.clone(),
                    local: save.target.clone(),
                    cached: false,
                    observed: false,
                    storage: None,
                };
                self.admit_sync_plan(intent, plan);
            } else {
                self.prepare_sync_roots(intent, pair, None);
            }
        }
    }
    fn waiting_for_drive(&self, drive: &str) -> bool {
        let status = self.shared.lock().ok();
        self.deferred
            .iter()
            .chain(self.scheduling.pending.values().map(|job| &job.envelope))
            .filter_map(|waiting| self.request_selector(&waiting.request).ok().flatten())
            .any(|selector| {
                selector == drive
                    || (selector.starts_with("/dev/")
                        && status.as_ref().is_some_and(|status| {
                            status.drives.iter().any(|candidate| {
                                candidate.id == drive && candidate.device == Path::new(&selector)
                            })
                        }))
            })
    }
    pub(super) fn admit_sync_plan(&mut self, mut intent: Intent, mut plan: Plan) {
        // Coverage may have changed while roots were being prepared.
        plan.observed &= self
            .watch
            .as_ref()
            .is_some_and(Watch::has_complete_coverage);
        if intent.operation.is_none() && plan.pair.paused {
            return;
        }
        if intent.operation.is_none() && self.waiting_for_drive(&plan.pair.drive) {
            self.queue_sync(intent);
            return;
        }
        if self.claims_busy(&plan.scopes()) || self.check_busy(&plan.pair.drive).is_err() {
            self.queue_sync(intent);
            return;
        }
        match plan.writeback_checkpoint(&self.db, &intent) {
            Ok(checkpoint) => intent.writeback = checkpoint,
            Err(error) => {
                self.sync_result(&intent, Err(error), None);
                return;
            }
        }
        self.launch_sync(intent, plan);
    }
    fn launch_sync(&mut self, intent: Intent, plan: Plan) {
        // Watch before population, retaining/coalescing notifications while the task runs.
        if let Some(watch) = &mut self.watch
            && intent.save.is_none()
        {
            watch.add(plan.portable.clone());
            watch.add(plan.local.clone());
        }
        if let Some(id) = &intent.operation
            && let Ok(Some(mut op)) = self.db.get::<DriveOperation>("operation", id)
        {
            op.state = "running".into();
            if let Some(save) = &intent.save {
                op.message = format!("Saving RAM copy to {}", save.target.path.display());
            }
            let _ = self.db.put("operation", id, &op);
        }
        let state = self.state.clone();
        let worker = plan.clone();
        let worker_intent = intent.clone();
        let task = write_task(&self.sync_pool, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut progress = worker_intent
                .operation
                .clone()
                .map(|id| job_progress(state, id));
            worker.run_with_cancel(
                &db,
                &worker_intent,
                &worker_intent.cancel,
                &mut |files, bytes| {
                    if let Some(progress) = &mut progress {
                        progress(files, bytes);
                    }
                },
            )
        });
        self.sync_jobs.insert(
            intent.pair.clone(),
            Pending {
                drive: plan.pair.drive.clone(),
                intent,
                plan,
                epoch: self.sync_epoch,
                task,
            },
        );
    }
    pub(super) fn sync_result(
        &mut self,
        intent: &Intent,
        result: Result<String>,
        plan: Option<Plan>,
    ) {
        if let Some(save) = &intent.save {
            let result = match self.saved_ram_result(save, &result) {
                Ok(()) => result,
                Err(error) => Err(format!(
                    "{}; could not persist RAM save status: {error}",
                    result
                        .as_ref()
                        .map_or_else(|error| error.to_string(), Clone::clone)
                )
                .into()),
            };
            if let Some(id) = &intent.operation {
                self.finish_operation(id, &intent.cancel, result);
            }
            return;
        }
        let result = match persist_result(&self.db, intent, &result) {
            Ok(()) => result,
            Err(error) => Err(format!(
                "{}; could not persist synchronization status: {error}",
                result
                    .as_ref()
                    .map_or_else(|error| error.to_string(), Clone::clone)
            )
            .into()),
        };
        if result.is_ok()
            && let Some(plan) = plan
        {
            self.sync_roots
                .insert(intent.pair.clone(), (plan.portable, plan.local));
        } else {
            self.sync_roots.remove(&intent.pair);
        }
        if let Some(id) = &intent.operation {
            self.finish_operation(id, &intent.cancel, result);
        }
    }
    pub(super) fn request_drive(&self, request: &DriveRequest) -> Result<Option<String>> {
        let selector = self.request_selector(request)?;
        selector
            .map(|selector| {
                if selector.starts_with("/dev/") {
                    Ok(self.drive(&selector)?.id)
                } else {
                    Ok(selector)
                }
            })
            .transpose()
    }
    // Registry-only lookup: device-node resolution belongs to filesystem workers.
    pub(super) fn request_selector(&self, request: &DriveRequest) -> Result<Option<String>> {
        let pair = match request {
            DriveRequest::Sync { pair }
            | DriveRequest::Pause { pair, .. }
            | DriveRequest::Deletions { pair, .. } => Some(pair),
            DriveRequest::FlushRam { session }
            | DriveRequest::SaveRam { session, .. }
            | DriveRequest::DiscardRam { session, .. } => Some(session),
            _ => None,
        };
        if let Some(id) = pair {
            return Ok(self.db.get::<SyncPair>("pair", id)?.map(|p| p.drive));
        }
        match request {
            DriveRequest::CopyToRam { drive }
            | DriveRequest::Pair { drive, .. }
            | DriveRequest::Backup { drive, .. }
            | DriveRequest::ReadOnlyBackup { drive }
            | DriveRequest::Catalog { drive }
            | DriveRequest::CatalogWithLimits { drive, .. }
            | DriveRequest::Eject { drive }
            | DriveRequest::PruneBackups { drive } => Ok(Some(drive.clone())),
            _ => Ok(None),
        }
    }
}
impl Plan {
    pub(super) fn scopes(&self) -> [crate::root::Scope; 2] {
        [self.portable.scope(), self.local.scope()]
    }
}
