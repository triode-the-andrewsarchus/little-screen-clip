//! Bounded reconciliation tasks retain their roots until real write completion.
use super::*;
use little_clip_core::jobs::task::Task;
use std::collections::HashSet;

#[derive(Clone)]
pub(super) struct Plan {
    pair: SyncPair,
    portable: Arc<Root>,
    local: Arc<Root>,
    cached: bool,
}
#[derive(Clone)]
pub(super) struct Intent {
    pub queued: Instant,
    pub pair: String,
    pub operation: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub paths: Option<Arc<HashSet<PathBuf>>>,
    pub initialize: bool,
}
pub(super) struct Pending {
    pub drive: String,
    pub intent: Intent,
    pub(super) plan: Plan,
    epoch: u64,
    task: Task<Result<String>>,
}
impl Plan {
    pub(super) fn run_with_cancel(
        &self,
        db: &Store,
        intent: &Intent,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        cancelled(cancel)?;
        self.portable.validate()?;
        self.local.validate()?;
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
                        && self.portable.reliable_revision_cache()?
                        && self.local.reliable_revision_cache()?,
                )?
            };
            files += result.files;
            conflicts += result.conflicts;
        }
        Ok(format!("{files} files copied · {conflicts} conflicts"))
    }
}
impl Owner {
    pub(super) fn prepare_reconciliation(
        &mut self,
        request: &DriveRequest,
        cancel: &AtomicBool,
    ) -> Result<(String, bool)> {
        cancelled(cancel)?;
        match request {
            DriveRequest::CopyToRam { drive } => {
                let drive = self.drive(drive)?;
                Ok((self.prepare_ram(&drive, cancel)?, true))
            }
            DriveRequest::Sync { pair } | DriveRequest::FlushRam { session: pair } => {
                let initial = self
                    .db
                    .get::<RamSession>("ram", pair)?
                    .is_some_and(|s| !s.ready);
                if self.db.get::<RamSession>("ram", pair)?.is_some() {
                    self.mount_ram(pair)?;
                }
                Ok((pair.clone(), initial))
            }
            DriveRequest::Pair {
                drive,
                relative,
                local,
            } => {
                if !valid_relative(relative) {
                    return Err("Invalid sync subtree".into());
                }
                let drive = self.drive(drive)?;
                if drive.ambiguous || (drive.part_uuid.is_none() && drive.filesystem_uuid.is_none())
                {
                    return Err(
                        "Enrollment requires an unambiguous persistent media identity".into(),
                    );
                }
                let mount = drive
                    .mount
                    .as_ref()
                    .ok_or("Mount the drive before pairing a folder")?;
                let source = Root::open(&mount.join(relative))?;
                let local = Root::create(local)?;
                self.validate_pair_target(&source, &local)?;
                let id = uuid::Uuid::new_v4().to_string();
                self.db.put(
                    "pair",
                    &id,
                    &SyncPair {
                        id: id.clone(),
                        drive: drive.id,
                        evidence: drive.evidence,
                        relative: relative.clone(),
                        local_identity: Some(local.storage_identity()?),
                        local: local.path,
                        paused: false,
                        deletions: false,
                        ram: false,
                        last_success: None,
                        status: "Waiting for initial union sync".into(),
                    },
                )?;
                Ok((id, false))
            }
            _ => Err("Invalid reconciliation request".into()),
        }
    }
    pub(super) fn prepare_sync(&self, id: &str) -> Result<Plan> {
        let pair: SyncPair = self.db.get("pair", id)?.ok_or("Unknown sync pair")?;
        let drive = self.drive(&pair.drive)?;
        if drive.evidence != pair.evidence {
            return Err("Drive identity differs from the enrollment; sync suspended".into());
        }
        let session = self.db.get::<RamSession>("ram", id)?;
        if drive.read_only && !(pair.ram && session.as_ref().is_some_and(|s| !s.ready)) {
            return Err("Drive is read-only; edits remain local".into());
        }
        let mount = drive
            .mount
            .as_ref()
            .ok_or("Drive is offline; local contents retained")?;
        let portable = Arc::new(Root::open(&mount.join(&pair.relative))?);
        if pair.ram && drive.part_uuid.is_none() && drive.filesystem_uuid.is_none() {
            self.unidentified_ram
                .get(id)
                .ok_or("No persistent media identity; writeback requires a new explicit session")?
                .validate()?;
        }
        if let Some(session) = &session {
            self.ram_available(session)?;
        }
        let local = Arc::new(Root::open(&pair.local)?);
        if pair.local_identity != Some(local.storage_identity()?) {
            return Err("Paired local directory was replaced; re-enroll it explicitly".into());
        }
        let cached = self.sync_roots.get(id).is_some_and(|(a, b)| {
            a.validate().is_ok()
                && b.validate().is_ok()
                && a.same_root(&portable)
                && b.same_root(&local)
        });
        let current = self.drive(&pair.drive)?;
        if current != drive {
            return Err(
                "Drive changed while pinning sync roots; retry with fresh inventory".into(),
            );
        }
        Ok(Plan {
            pair,
            portable,
            local,
            cached,
        })
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
            if self.sync_jobs.len() >= 4 {
                self.queue_sync(intent);
                continue;
            }
            if self.sync_jobs.contains_key(&intent.pair) {
                self.queue_sync(intent);
                continue;
            }
            if let Ok(Some(pair)) = self.db.get::<SyncPair>("pair", &intent.pair)
                && self.check_busy(&pair.drive).is_err()
            {
                self.queue_sync(intent);
                continue;
            }
            let plan = match self.prepare_sync(&intent.pair) {
                Ok(plan) => plan,
                Err(error) => {
                    self.sync_result(&intent, Err(error), None);
                    continue;
                }
            };
            if intent.operation.is_none() && plan.pair.paused {
                continue;
            }
            if intent.operation.is_none()
                && self.deferred.iter().any(|waiting| {
                    self.request_drive(&waiting.request).ok().flatten().as_ref()
                        == Some(&plan.pair.drive)
                })
            {
                self.queue_sync(intent);
                continue;
            }
            if self
                .background
                .as_ref()
                .is_some_and(|job| job.roots.iter().any(|root| plan.overlaps_root(root)))
                || self.check_busy(&plan.pair.drive).is_err()
                || self
                    .sync_jobs
                    .values()
                    .any(|other| plan.overlaps(&other.plan))
            {
                self.queue_sync(intent);
                continue;
            }
            // Watch before population, retaining/coalescing notifications while the task runs.
            if let Some(watch) = &mut self.watch {
                watch.add(plan.portable.clone());
                watch.add(plan.local.clone());
            }
            if let Some(id) = &intent.operation
                && let Ok(Some(mut op)) = self.db.get::<DriveOperation>("operation", id)
            {
                op.state = "running".into();
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
    }
    pub(super) fn sync_result(
        &mut self,
        intent: &Intent,
        result: Result<String>,
        plan: Option<Plan>,
    ) {
        if let Ok(Some(mut pair)) = self.db.get::<SyncPair>("pair", &intent.pair) {
            match &result {
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
            let _ = self.db.put("pair", &pair.id, &pair);
            if let Ok(Some(mut session)) = self.db.get::<RamSession>("ram", &pair.id) {
                session.status = pair.status.clone();
                if result.is_ok() {
                    session.ready = true;
                    session.connected = true;
                }
                let _ = self.db.put("ram", &pair.id, &session);
            }
        }
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
            | DriveRequest::Catalog { drive }
            | DriveRequest::Eject { drive }
            | DriveRequest::PruneBackups { drive } => {
                if drive.starts_with("/dev/") {
                    Ok(Some(self.drive(drive)?.id))
                } else {
                    Ok(Some(drive.clone()))
                }
            }
            _ => Ok(None),
        }
    }
}
impl Plan {
    pub(super) fn overlaps_root(&self, root: &Root) -> bool {
        self.portable.overlap(root) || self.local.overlap(root)
    }
    fn overlaps(&self, other: &Self) -> bool {
        [
            (&self.portable, &other.portable),
            (&self.portable, &other.local),
            (&self.local, &other.portable),
            (&self.local, &other.local),
        ]
        .iter()
        .any(|(a, b)| a.overlap(b))
    }
}
