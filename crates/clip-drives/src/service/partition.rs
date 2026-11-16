//! Retained pre-unmount flushes and authorization leave the coordinator responsive.
use super::*;
use crate::root::Scope;
use little_clip_core::jobs::task::Task;

pub(super) type FilesystemAction = Arc<dyn Fn(&Drive) -> Result<()> + Send + Sync>;
pub(super) struct Authority {
    pub unmount: FilesystemAction,
    pub mount: FilesystemAction,
    pub open_image: backup::ImageOpener,
}
impl Default for Authority {
    fn default() -> Self {
        Self {
            unmount: Arc::new(unmount),
            mount: Arc::new(|drive| filesystem_action(drive, true)),
            open_image: Arc::new(backup::open_image),
        }
    }
}

#[derive(Clone)]
pub(super) struct ImageContext {
    pub original: Drive,
    pub source: Arc<crate::device::Source>,
    pub claims: Vec<Scope>,
    pub restore_mounted: bool,
    pub unmounted_by_job: bool,
}
impl ImageContext {
    pub fn finish(
        &self,
        db: &Store,
        id: &str,
        inventory: &Inventory,
        authority: &Authority,
        result: Result<String>,
    ) -> Result<String> {
        if !self.restore_mounted || !self.unmounted_by_job {
            return result;
        }
        // Restoration is accepted cleanup: cancellation cannot discard a mount
        // already requested, nor erase the completed backup result.
        let restored = phase(db, id, PartitionPhase::RestoringMount, None)
            .and_then(|()| self.restore(inventory, authority));
        let message = match &restored {
            Ok(drive) => format!(
                "Partition mounted again at {}",
                drive
                    .mount
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            Err(error) => format!("Mounted-state restoration failed: {error}"),
        };
        let recorded = (|| -> Result<()> {
            let mut operation = db
                .get::<DriveOperation>("operation", id)?
                .ok_or("Missing partition operation during restoration")?;
            let partition = operation
                .partition
                .as_mut()
                .ok_or("Missing partition restoration context")?;
            partition.phase = if restored.is_ok() {
                PartitionPhase::MountRestored
            } else {
                PartitionPhase::MountRestoreFailed
            };
            partition.restore_error = restored.as_ref().err().map(ToString::to_string);
            db.put("operation", id, &operation)
        })();
        let message = match recorded {
            Ok(()) => message,
            Err(error) => format!("{message}; restoration journal update failed: {error}"),
        };
        match result {
            Ok(result) => Ok(format!("{result}; {message}")),
            Err(error) => Err(format!("{error}; {message}").into()),
        }
    }
    fn restore(&self, inventory: &Inventory, authority: &Authority) -> Result<Drive> {
        self.source.validate()?;
        let before = self.observed(inventory)?;
        if before.mount.is_none()
            && let Err(error) = (authority.mount)(&self.original)
        {
            return Err(format!(
                "Remount request failed: {error}; {}",
                mount_state(self.observed(inventory))
            )
            .into());
        }
        self.source.validate()?;
        let after = self.observed(inventory)?;
        if after.mount.is_none() || (self.original.read_only && !after.read_only) {
            return Err(
                "Mount request returned, but the restored mount/access mode could not be confirmed"
                    .into(),
            );
        }
        Ok(after)
    }
    fn observed(&self, inventory: &Inventory) -> Result<Drive> {
        let mut drives = inventory()?
            .into_iter()
            .filter(|drive| drive.device == self.original.device);
        let observed = drives
            .next()
            .ok_or("Original partition is no longer available for remount")?;
        if drives.next().is_some() || !same_partition(&observed, &self.original) {
            return Err(
                "Partition lifetime or identity changed; refusing to mount a replacement".into(),
            );
        }
        Ok(observed)
    }
}

fn same_partition(actual: &Drive, expected: &Drive) -> bool {
    let mut comparison = actual.clone();
    comparison.mount = expected.mount.clone();
    // A read-only mount can become an unmounted writable block device; these
    // access observations are checked separately from the partition's lifetime.
    comparison.read_only = expected.read_only;
    comparison == *expected
}
fn mount_state(observed: Result<Drive>) -> &'static str {
    match observed {
        Ok(drive) if drive.mount.is_none() => "partition now appears unmounted",
        Ok(_) => "partition still appears mounted",
        Err(_) => "current partition state is unavailable",
    }
}

pub(super) struct Pending {
    pub drive: String,
    pub claims: Vec<Scope>,
    pub cancel: Arc<AtomicBool>,
    pub destination: Option<Arc<Root>>,
    envelope: Envelope,
    snapshot: Drive,
    config: DriveConfig,
    source: Option<Arc<crate::device::Source>>,
    stage: Stage,
}
enum Stage {
    Flush(Task<Result<()>>),
    Unmount(Task<Result<Drive>>),
    Restore(Task<Result<String>>),
}
enum Completion {
    Flush(Result<()>),
    Unmount(Result<Drive>),
    Restored(Result<String>),
}
struct Admission {
    drive: Drive,
    source: Option<Arc<crate::device::Source>>,
    destination: Option<Arc<Root>>,
    plans: Vec<reconciliation::Plan>,
    claims: Vec<Scope>,
}
impl Pending {
    pub fn is_image(&self) -> bool {
        matches!(self.envelope.request, DriveRequest::Backup { .. })
    }
    fn poll(&self) -> Option<Completion> {
        match &self.stage {
            Stage::Flush(task) => task
                .poll()
                .map(|result| Completion::Flush(write_result(result))),
            Stage::Unmount(task) => task
                .poll()
                .map(|result| Completion::Unmount(write_result(result))),
            Stage::Restore(task) => task
                .poll()
                .map(|result| Completion::Restored(write_result(result))),
        }
    }
}

impl Owner {
    pub(super) fn start_partition(&mut self, envelope: Envelope) {
        let image = matches!(envelope.request, DriveRequest::Backup { .. });
        if image && self.config.backup_directory.is_none() {
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err("Choose a DriveBackupDir before starting a backup".into()),
            );
            return;
        }
        if self.partition.is_some() || (image && self.background.is_some()) {
            self.defer_operation(envelope);
            return;
        }
        let prepared = self.partition_plan(&envelope, image);
        let Admission {
            drive,
            source,
            destination,
            plans,
            claims,
        } = match prepared {
            Ok(value) => value,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        let initialized = (|| -> Result<()> {
            let mut operation = self
                .db
                .get::<DriveOperation>("operation", &envelope.id)?
                .ok_or("Missing accepted partition operation")?;
            operation.partition = Some(PartitionPreparation {
                drive: drive.clone(),
                destination: destination.as_ref().map(|root| root.path.clone()),
                phase: PartitionPhase::Flushing,
                unmounted_by_job: false,
                restore_mounted: matches!(
                    envelope.request,
                    DriveRequest::Backup {
                        restore_mounted: true,
                        ..
                    }
                ),
                completed_backup: None,
                restore_error: None,
            });
            self.db.put("operation", &envelope.id, &operation)
        })();
        if let Err(error) = initialized {
            self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
            return;
        }
        let state = self.state.clone();
        let operation = envelope.id.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.partition_jobs, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut progress = job_progress(state, operation.clone());
            phase(&db, &operation, PartitionPhase::Flushing, None)?;
            for plan in plans {
                cancelled(&cancel)?;
                let intent = reconciliation::Intent {
                    queued: Instant::now(),
                    pair: plan.pair.id.clone(),
                    operation: None,
                    cancel: cancel.clone(),
                    paths: None,
                    initialize: false,
                    save: None,
                };
                let result = plan.run_with_cancel(&db, &intent, &cancel, &mut progress);
                reconciliation::persist_result(&db, &intent, &result)?;
                result?;
            }
            cancelled(&cancel)
        });
        self.partition = Some(Pending {
            drive: drive.id.clone(),
            claims,
            cancel: envelope.cancel.clone(),
            destination,
            envelope,
            snapshot: drive,
            config: self.config.clone(),
            source,
            stage: Stage::Flush(task),
        });
    }

    fn partition_plan(&self, envelope: &Envelope, image: bool) -> Result<Admission> {
        cancelled(&envelope.cancel)?;
        let selector = selector(&envelope.request)?;
        let drive = self.drive(selector)?;
        self.check_busy(&drive.id)?;
        if image && drive.capacity == 0 {
            return Err("Partition capacity is unknown; cannot create a complete image".into());
        }
        let device_source = image
            .then(|| crate::device::Source::capture(&drive).map(Arc::new))
            .transpose()?;
        let source = drive
            .mount
            .as_ref()
            .map(|path| Root::open(path))
            .transpose()?;
        let destination = image
            .then(|| self.backup_destination(source.as_ref()))
            .transpose()?;
        let mut plans = Vec::new();
        if source.is_some() {
            for pair in self.db.list::<SyncPair>("pair", 0, 1000)? {
                if pair.drive == drive.id && !pair.paused {
                    plans.push(self.prepare_sync_pair(pair, &drive)?);
                }
            }
        }
        let mut claims = Vec::new();
        claims.extend(source.as_ref().map(Root::scope));
        claims.extend(destination.as_ref().map(|root| root.scope()));
        for plan in &plans {
            claims.extend(plan.scopes());
        }
        if self.claims_busy(&claims) {
            return Err("Partition preparation overlaps an active storage operation; retry after it finishes".into());
        }
        if self.drive(selector)? != drive {
            return Err("Drive changed while pinning pre-unmount roots".into());
        }
        Ok(Admission {
            drive,
            source: device_source,
            destination,
            plans,
            claims,
        })
    }

    pub(super) fn poll_partition(&mut self) {
        let Some(completion) = self.partition.as_ref().and_then(Pending::poll) else {
            return;
        };
        let mut job = self
            .partition
            .take()
            .expect("polled retained partition job");
        match completion {
            Completion::Flush(Ok(())) => {
                if let Some(source) = &job.source
                    && let Err(error) = source.validate()
                {
                    self.finish_operation(&job.envelope.id, &job.cancel, Err(error));
                    return;
                }
                if job.cancel.load(Ordering::Acquire) || self.closing.load(Ordering::Acquire) {
                    self.finish_operation(
                        &job.envelope.id,
                        &job.cancel,
                        Err("Flush completed; unmount cancelled before admission".into()),
                    );
                    return;
                }
                if job.snapshot.mount.is_none() {
                    self.partition_ready(job, Ok(None));
                    return;
                }
                let retirement = self.retire_partition(&job.snapshot);
                if let Err(error) = retirement {
                    self.finish_operation(&job.envelope.id, &job.cancel, Err(error));
                    return;
                }
                let drive = job.snapshot.clone();
                let selector = selector(&job.envelope.request)
                    .expect("validated partition request")
                    .to_owned();
                let inventory = self.inventory.clone();
                let unmount = self.authority.unmount.clone();
                let cancel = job.cancel.clone();
                let state = self.state.clone();
                let operation = job.envelope.id.clone();
                let source = job.source.clone();
                job.stage = Stage::Unmount(write_task(&self.partition_jobs, move || {
                    let db = Store::existing(&state.join("drives.sqlite"), false)?;
                    phase(&db, &operation, PartitionPhase::Unmounting, None)?;
                    unmount_checked(
                        &drive,
                        &selector,
                        &inventory,
                        &unmount,
                        &cancel,
                        source.as_deref(),
                    )
                }));
                self.partition = Some(job);
            }
            Completion::Flush(Err(error)) => {
                self.finish_operation(&job.envelope.id, &job.cancel, Err(error));
            }
            Completion::Unmount(result) => self.partition_ready(job, result.map(Some)),
            Completion::Restored(result) => {
                self.finish_operation(&job.envelope.id, &job.cancel, result);
                self.automatic();
            }
        }
    }

    fn partition_ready(&mut self, job: Pending, result: Result<Option<Drive>>) {
        let image = job.is_image();
        let unmounted_by_job =
            result.as_ref().is_ok_and(|drive| drive.is_some()) && job.snapshot.mount.is_some();
        let drive = match result {
            Ok(drive) => drive.unwrap_or_else(|| job.snapshot.clone()),
            Err(error) => {
                self.finish_operation(&job.envelope.id, &job.cancel, Err(error));
                self.automatic();
                return;
            }
        };
        let checkpoint = phase(&self.db, &job.envelope.id, PartitionPhase::Unmounted, None);
        if image {
            let Some(source) = job.source.clone() else {
                self.finish_operation(&job.envelope.id, &job.cancel, Err("Partition unmounted; missing retained device pin; no image or remount was started".into()));
                return;
            };
            let context = ImageContext {
                original: job.snapshot.clone(),
                source,
                claims: job.claims.clone(),
                restore_mounted: matches!(
                    job.envelope.request,
                    DriveRequest::Backup {
                        restore_mounted: true,
                        ..
                    }
                ),
                unmounted_by_job,
            };
            let stopped = checkpoint.map_err(|error| format!("Partition is confirmed unmounted; checkpoint failed: {error}; no image copy was started").into())
                .and_then(|()| {
                    if job.cancel.load(Ordering::Acquire) || self.closing.load(Ordering::Acquire) {
                        Err("Partition is unmounted; image copy cancelled before admission. Local and RAM edits are retained".into())
                    } else { Ok(()) }
                });
            if let Err(error) = stopped {
                self.finish_partition_image(job, context, Err(error));
            } else {
                self.launch_bulk(
                    job.envelope,
                    drive,
                    job.config,
                    job.destination.into_iter().collect(),
                    Some(context),
                );
            }
        } else {
            let message = match checkpoint {
                Ok(()) => "Partition unmounted; RAM copy retained".into(),
                Err(error) => format!(
                    "Partition is confirmed unmounted; checkpoint failed: {error}; RAM copy retained"
                ),
            };
            self.finish_operation(&job.envelope.id, &job.cancel, Ok(message));
        }
        self.automatic();
    }

    fn finish_partition_image(
        &mut self,
        mut job: Pending,
        context: ImageContext,
        result: Result<String>,
    ) {
        if !context.restore_mounted || !context.unmounted_by_job {
            self.finish_operation(&job.envelope.id, &job.cancel, result);
            return;
        }
        let state = self.state.clone();
        let operation = job.envelope.id.clone();
        let inventory = self.inventory.clone();
        let authority = self.authority.clone();
        job.stage = Stage::Restore(write_task(&self.partition_jobs, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            context.finish(&db, &operation, &inventory, &authority, result)
        }));
        self.partition = Some(job);
    }

    fn retire_partition(&mut self, drive: &Drive) -> Result<()> {
        if let Some(path) = &drive.mount {
            let scope = Root::open(path)?.scope();
            self.sync_roots
                .retain(|_, (a, b)| !scope.overlap(&a.scope()) && !scope.overlap(&b.scope()));
            self.unidentified_ram
                .retain(|_, root| !scope.overlap(&root.scope()));
            if let Some(watch) = &mut self.watch {
                watch.remove_scope(&scope);
            }
        }
        Ok(())
    }
}

fn selector(request: &DriveRequest) -> Result<&str> {
    match request {
        DriveRequest::Eject { drive }
        | DriveRequest::Backup {
            drive,
            kind: BackupKind::Image,
            ..
        } => Ok(drive),
        _ => Err("Invalid pre-unmount request".into()),
    }
}
pub(super) fn phase(
    db: &Store,
    id: &str,
    phase: PartitionPhase,
    backup: Option<&DriveBackup>,
) -> Result<()> {
    let mut operation = db
        .get::<DriveOperation>("operation", id)?
        .ok_or("Missing accepted partition operation")?;
    let partition = operation
        .partition
        .as_mut()
        .ok_or("Missing partition preparation journal")?;
    partition.phase = phase;
    if phase == PartitionPhase::Unmounted {
        partition.unmounted_by_job = partition.drive.mount.is_some();
    }
    if let Some(backup) = backup {
        partition.completed_backup = Some(backup.id.clone());
        operation.files = 1;
        operation.bytes = backup.source_size;
    }
    operation.state = if matches!(
        phase,
        PartitionPhase::Flushing | PartitionPhase::Unmounting | PartitionPhase::Unmounted
    ) {
        "preparing"
    } else {
        "running"
    }
    .into();
    operation.message = phase.description().into();
    db.put("operation", id, &operation)
}
fn unmount_checked(
    drive: &Drive,
    selector: &str,
    inventory: &Inventory,
    unmount: &FilesystemAction,
    cancel: &AtomicBool,
    source: Option<&crate::device::Source>,
) -> Result<Drive> {
    if let Some(source) = source {
        source.validate()?;
    }
    if discovery::select(inventory()?, selector)? != *drive {
        return Err("Partition changed before unmount; refusing to unmount a replacement".into());
    }
    cancelled(cancel)?;
    if let Err(error) = unmount(drive) {
        let observed = inventory().and_then(|drives| discovery::select(drives, selector));
        let state = mount_state(observed);
        return Err(
            format!("Unmount request failed: {error}; {state}. No image copy was started").into(),
        );
    }
    if let Some(source) = source {
        source
            .validate()
            .map_err(|error| format!("Unmount completed; {error}. No image copy was started"))?;
    }
    let observed = inventory().and_then(|drives| discovery::select(drives, selector))
        .map_err(|error| format!("Unmount completed; could not confirm current partition state: {error}. No image copy was started"))?;
    if !same_partition(&observed, drive) || observed.mount.is_some() {
        return Err("Unmount returned, but partition identity or mount state could not be confirmed; image copy was not started".into());
    }
    Ok(observed)
}
pub(super) fn unmount(drive: &Drive) -> Result<()> {
    filesystem_action(drive, false)
}
fn filesystem_action(drive: &Drive, mounting: bool) -> Result<()> {
    let verb = if mounting { "mount" } else { "unmount" };
    let mut request = CommandRequest::new("udisksctl")
        .args([verb, "--block-device"])
        .arg(&drive.device);
    if mounting && drive.read_only {
        request = request.args(["--options", "ro"]);
    }
    let output = request
        // Keep the accepted authority request until its actual reply/child exit.
        .timeout(Duration::MAX)
        .max_output_bytes(64 * 1024)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not {verb} partition; local and RAM edits retained: {}",
            output.stderr_lossy()
        )
        .into());
    }
    Ok(())
}
