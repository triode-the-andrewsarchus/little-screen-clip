//! One retained bulk worker leaves prompt writeback on the storage coordinator.
use super::*;
use little_clip_core::jobs::task::Task;

pub(super) struct Pending {
    pub(super) drive: String,
    pub(super) roots: Vec<Arc<Root>>,
    pub(super) destination: Option<PathBuf>,
    image: Option<partition::ImageContext>,
    id: String,
    pub(super) cancel: Arc<AtomicBool>,
    task: Task<Result<String>>,
}
impl Pending {
    pub(super) fn scopes(&self) -> impl Iterator<Item = crate::root::Scope> + '_ {
        self.roots.iter().map(|root| root.scope()).chain(
            self.image
                .iter()
                .flat_map(|image| image.claims.iter().cloned()),
        )
    }
}
impl Owner {
    pub(super) fn check_busy(&self, drive: &str) -> Result<()> {
        if self
            .ram_setup
            .as_ref()
            .is_some_and(|job| job.drive.id == drive)
            || self
                .partition
                .as_ref()
                .is_some_and(|job| job.drive == drive)
            || self
                .background
                .as_ref()
                .is_some_and(|job| job.drive == drive)
            || self.sync_jobs.values().any(|job| job.drive == drive)
        {
            return Err("This partition has an active storage operation; synchronization resumes when it finishes".into());
        }
        Ok(())
    }
    pub(super) fn start_background(&mut self, envelope: Envelope) {
        if matches!(envelope.request, DriveRequest::Backup { .. })
            && self.config.backup_directory.is_none()
        {
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err("Choose a DriveBackupDir before starting a backup".into()),
            );
            return;
        }
        if self.background.is_some()
            || self
                .partition
                .as_ref()
                .is_some_and(partition::Pending::is_image)
        {
            self.defer_operation(envelope);
            return;
        }
        let resources = (|| -> Result<(Drive, Vec<Arc<Root>>)> {
            let selector = match &envelope.request {
                DriveRequest::Backup { drive, .. } | DriveRequest::Catalog { drive } => drive,
                _ => return Err("Missing bulk drive".into()),
            };
            let drive = self.drive(selector)?;
            let mut roots = Vec::new();
            let path = drive
                .mount
                .as_ref()
                .ok_or("Mount the partition before saving a file backup or catalog")?;
            roots.push(Arc::new(Root::open(path)?));
            if matches!(envelope.request, DriveRequest::Backup { .. }) {
                let source = roots.first().map(Arc::as_ref);
                roots.push(self.backup_destination(source)?);
            } else {
                roots.push(Arc::new(Root::open(&self.state)?));
            }
            if self.drive(selector)? != drive {
                return Err("Drive changed while pinning bulk-operation roots".into());
            }
            Ok((drive, roots))
        })();
        let (drive, roots) = match resources {
            Ok(value) => value,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        if self.claims_busy(&roots.iter().map(|root| root.scope()).collect::<Vec<_>>()) {
            self.defer_operation(envelope);
            return;
        }
        self.launch_bulk(envelope, drive, self.config.clone(), roots, None);
    }
    pub(super) fn launch_bulk(
        &mut self,
        envelope: Envelope,
        drive: Drive,
        config: DriveConfig,
        roots: Vec<Arc<Root>>,
        image: Option<partition::ImageContext>,
    ) {
        if let Ok(Some(mut operation)) = self.db.get::<DriveOperation>("operation", &envelope.id) {
            operation.state = "running".into();
            let _ = self.db.put("operation", &envelope.id, &operation);
        }
        let state = self.state.clone();
        let id = envelope.id.clone();
        let cancel = envelope.cancel.clone();
        let drive_id = drive.id.clone();
        let destination = matches!(envelope.request, DriveRequest::Backup { .. })
            .then(|| config.backup_directory.clone())
            .flatten();
        let worker_roots = roots.clone();
        let worker_image = image.clone();
        let authority = self.authority.clone();
        let inventory = self.inventory.clone();
        let task = write_task(&self.backup_jobs, move || {
            let validation = (|| -> Result<()> {
                cancelled(&cancel)?;
                for root in &worker_roots {
                    root.validate()?;
                }
                Ok(())
            })();
            let validation = match validation {
                Err(error) if worker_image.is_none() => return Err(error),
                other => other,
            };
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut progress = job_progress(state.clone(), id.clone());
            let result = validation.and_then(|()| match envelope.request {
                DriveRequest::Backup { kind, .. } => {
                    backup::recover_publications(&db)?;
                    let destination = worker_roots
                        .last()
                        .ok_or("Missing admitted backup destination")?;
                    let version = match kind {
                        BackupKind::Files => backup::files(
                            &db,
                            &drive,
                            worker_roots
                                .first()
                                .ok_or("Missing admitted partition root")?,
                            destination,
                            &cancel,
                            &mut progress,
                        )?,
                        BackupKind::Image => {
                            let context = worker_image
                                .as_ref()
                                .ok_or("Missing original device admission context")?;
                            let source = &context.source;
                            let mut previous = None;
                            let version = backup::image(
                                &db,
                                &drive,
                                source,
                                destination,
                                &cancel,
                                &authority.open_image,
                                |phase, bytes| {
                                    if previous != Some(phase) {
                                        partition::phase(&db, &id, phase, None)?;
                                        previous = Some(phase);
                                    }
                                    progress(0, bytes);
                                    Ok(())
                                },
                            )?;
                            if let Err(error) = partition::phase(
                                &db,
                                &id,
                                PartitionPhase::ImagePublished,
                                Some(&version),
                            ) {
                                return Ok(format!(
                                    "Backup completed: {}; completion journal update failed: {error}",
                                    version.path.display()
                                ));
                            }
                            version
                        }
                    };
                    Ok(format!(
                        "Backup completed: {} ({} bytes written)",
                        version.path.display(),
                        version.bytes_written
                    ))
                }
                DriveRequest::Catalog { .. } => {
                    let catalog = catalog::capture(
                        &db,
                        worker_roots
                            .last()
                            .ok_or("Missing admitted catalog state root")?,
                        &drive,
                        worker_roots
                            .first()
                            .ok_or("Missing admitted partition root")?,
                        &config,
                        &cancel,
                        &mut progress,
                    )?;
                    Ok(format!(
                        "Catalog saved: {} · {}",
                        catalog.id, catalog.coverage
                    ))
                }
                _ => Err("Invalid bulk operation".into()),
            });
            match &worker_image {
                Some(context) => context.finish(&db, &id, &inventory, &authority, result),
                None => result,
            }
        });
        self.background = Some(Pending {
            drive: drive_id,
            roots,
            destination,
            image,
            id: envelope.id,
            cancel: envelope.cancel,
            task,
        });
    }
    pub(super) fn poll_background(&mut self) {
        let result = self.background.as_ref().and_then(|job| job.task.poll());
        if let Some(result) = result {
            let job = self.background.take().expect("polled retained job");
            let result = write_result(result);
            self.finish_operation(&job.id, &job.cancel, result);
            self.automatic();
        }
    }
    pub(super) fn finish_operation(&self, id: &str, cancel: &AtomicBool, result: Result<String>) {
        if let Err(error) = finish_record(&self.db, id, cancel, result) {
            tracing::error!(operation = id, %error, "Could not persist storage operation completion");
        }
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(id);
        }
        self.discover();
        let _ = self.publish_policy();
    }
}

pub(super) fn finish_record(
    db: &Store,
    id: &str,
    cancel: &AtomicBool,
    result: Result<String>,
) -> Result<()> {
    if let Some(mut operation) = db.get::<DriveOperation>("operation", id)? {
        operation.finished = Some(now());
        match result {
            Ok(message) => {
                operation.state = if operation
                    .partition
                    .as_ref()
                    .is_some_and(|p| p.restore_error.is_some())
                {
                    "complete_with_warning"
                } else {
                    "complete"
                }
                .into();
                operation.message = message;
            }
            Err(error) => {
                operation.state = if cancel.load(Ordering::Acquire) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                operation.message = error.to_string();
            }
        }
        db.put("operation", id, &operation)?;
    }
    Ok(())
}
