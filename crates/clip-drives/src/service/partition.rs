//! Retained pre-unmount flushes and authorization leave the coordinator responsive.
use super::*;
use crate::root::Scope;
use little_clip_core::jobs::task::Task;

pub(super) type Unmounter = Arc<dyn Fn(&Drive) -> Result<()> + Send + Sync>;

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
}
enum Completion {
    Flush(Result<()>),
    Unmount(Result<Drive>),
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
        let state = self.state.clone();
        let operation = envelope.id.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.partition_jobs, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut progress = job_progress(state, operation.clone());
            phase(&db, &operation, "Flushing enrolled folders before unmount")?;
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
                let unmount = self.unmounter.clone();
                let cancel = job.cancel.clone();
                let state = self.state.clone();
                let operation = job.envelope.id.clone();
                let source = job.source.clone();
                job.stage = Stage::Unmount(write_task(&self.partition_jobs, move || {
                    let db = Store::existing(&state.join("drives.sqlite"), false)?;
                    phase(
                        &db,
                        &operation,
                        "Waiting for partition unmount / authorization",
                    )?;
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
        }
    }

    fn partition_ready(&mut self, job: Pending, result: Result<Option<Drive>>) {
        let image = job.is_image();
        let drive = match result {
            Ok(drive) => drive.unwrap_or(job.snapshot),
            Err(error) => {
                self.finish_operation(&job.envelope.id, &job.cancel, Err(error));
                self.automatic();
                return;
            }
        };
        if image {
            if job.cancel.load(Ordering::Acquire) || self.closing.load(Ordering::Acquire) {
                self.finish_operation(&job.envelope.id, &job.cancel, Err("Partition is unmounted; image copy cancelled before admission. Local and RAM edits are retained".into()));
            } else {
                self.launch_bulk(
                    job.envelope,
                    drive,
                    job.config,
                    job.destination.into_iter().collect(),
                    job.source,
                );
            }
        } else {
            self.finish_operation(
                &job.envelope.id,
                &job.cancel,
                Ok("Partition unmounted; RAM copy retained".into()),
            );
        }
        self.automatic();
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
        } => Ok(drive),
        _ => Err("Invalid pre-unmount request".into()),
    }
}
fn phase(db: &Store, id: &str, message: &str) -> Result<()> {
    if let Some(mut operation) = db.get::<DriveOperation>("operation", id)? {
        operation.state = "preparing".into();
        operation.message = message.into();
        db.put("operation", id, &operation)?;
    }
    Ok(())
}
fn unmount_checked(
    drive: &Drive,
    selector: &str,
    inventory: &Inventory,
    unmount: &Unmounter,
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
        let state = match observed {
            Ok(drive) if drive.mount.is_none() => "partition now appears unmounted",
            Ok(_) => "partition still appears mounted",
            Err(_) => "current partition state is unavailable",
        };
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
    let mut comparison = observed.clone();
    comparison.mount = drive.mount.clone();
    if comparison != *drive || observed.mount.is_some() {
        return Err("Unmount returned, but partition identity or mount state could not be confirmed; image copy was not started".into());
    }
    Ok(observed)
}
pub(super) fn unmount(drive: &Drive) -> Result<()> {
    let output = CommandRequest::new("udisksctl")
        .args(["unmount", "--block-device"])
        .arg(&drive.device)
        // Keep the accepted authority request until its actual reply/child exit.
        .timeout(Duration::MAX)
        .max_output_bytes(64 * 1024)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not unmount partition; local and RAM edits retained: {}",
            output.stderr_lossy()
        )
        .into());
    }
    Ok(())
}
