//! Bounded independent captures share the retained bulk pool; backups stay exclusive.
use super::*;
use little_clip_core::jobs::task::Task;
mod worker;

pub(super) struct Pending {
    pub(super) drive: Option<String>,
    catalog: bool,
    pub(super) roots: Vec<Arc<Root>>,
    pub(super) destination: Option<PathBuf>,
    context: Option<partition::BackupContext>,
    cleanup_pending: bool,
    id: String,
    pub(super) cancel: Arc<AtomicBool>,
    task: Task<Result<String>>,
}
impl Pending {
    pub(super) fn scopes(&self) -> impl Iterator<Item = crate::root::Scope> + '_ {
        // Catalog output directories are private and separately journaled. Sharing
        // the registry root does not serialize independent source inventories.
        self.roots
            .iter()
            .take(if self.catalog { 1 } else { self.roots.len() })
            .map(|root| root.scope())
            .chain(
                self.context
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
                .values()
                .any(|job| job.drive.as_deref() == Some(drive))
            || self.sync_jobs.values().any(|job| job.drive == drive)
            || self.sync_preparing.values().any(|job| job.drive == drive)
        {
            return Err("This partition has an active storage operation; synchronization resumes when it finishes".into());
        }
        Ok(())
    }
    pub(super) fn start_background(&mut self, mut envelope: Envelope) {
        let catalog = matches!(
            envelope.request,
            DriveRequest::Catalog { .. } | DriveRequest::CatalogWithLimits { .. }
        );
        let available = if catalog {
            self.background.len() < self.config.catalog_workers
                && self.background.values().all(|job| job.catalog)
        } else {
            self.background.is_empty()
        };
        if !available
            || self.maintenance.busy()
            || self
                .partition
                .as_ref()
                .is_some_and(partition::Pending::is_backup)
        {
            self.defer_operation(envelope);
            return;
        }
        if matches!(envelope.request, DriveRequest::PruneBackups { .. }) {
            self.start_prune(envelope);
            return;
        }
        if matches!(envelope.request, DriveRequest::MeasureStorage) {
            let state = match Root::open(&self.state) {
                Ok(root) => Arc::new(root),
                Err(error) => {
                    self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                    return;
                }
            };
            let cancel = envelope.cancel.clone();
            self.launch(envelope, None, Vec::new(), None, move || {
                crate::storage::measure(state, cancel)
            });
            return;
        }
        let (drive, roots) = match self.bulk_roots(&envelope) {
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
        let destination = matches!(
            envelope.request,
            DriveRequest::Backup { .. } | DriveRequest::ReadOnlyBackup { .. }
        )
        .then(|| roots.last().map(Arc::as_ref))
        .flatten();
        if let Err(error) = self.admit_backup_destination(&envelope, destination) {
            self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
            return;
        }
        drop(envelope.review.take());
        self.launch_bulk(envelope, drive, self.config.clone(), roots, None);
    }
    fn start_prune(&mut self, envelope: Envelope) {
        let prepared = (|| {
            let drive = self
                .request_drive(&envelope.request)?
                .ok_or("Missing retirement drive")?;
            let keep = self
                .config
                .backup_keep_versions
                .ok_or("Set drives.backup_keep_versions before explicitly pruning backups")?;
            Ok::<_, Error>((drive, keep))
        })();
        let (drive, keep) = match prepared {
            Ok(value) => value,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        if self.check_busy(&drive).is_err() {
            self.defer_operation(envelope);
            return;
        }
        let state = self.state.clone();
        let cancel = envelope.cancel.clone();
        let selected = drive.clone();
        self.launch(envelope, Some(drive), Vec::new(), None, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            backup::prune(&db, &selected, keep, &cancel)
        });
    }
    fn bulk_roots(&self, envelope: &Envelope) -> Result<(Drive, Vec<Arc<Root>>)> {
        let selector = match &envelope.request {
            DriveRequest::Backup { drive, .. }
            | DriveRequest::ReadOnlyBackup { drive }
            | DriveRequest::Catalog { drive } => drive,
            DriveRequest::CatalogWithLimits { drive, .. } => drive,
            _ => return Err("Missing bulk drive".into()),
        };
        let drive = self.drive(selector)?;
        let mut roots = Vec::new();
        let source = self
            .admitted_source(envelope, &drive)?
            .ok_or("Mount the partition before saving a file backup or catalog")?;
        roots.push(source);
        if matches!(
            envelope.request,
            DriveRequest::Backup { .. } | DriveRequest::ReadOnlyBackup { .. }
        ) {
            let source = roots.first().map(Arc::as_ref);
            let destination = self.reviewed_backup_destination(envelope, source)?;
            preflight::space(&destination, &drive, BackupKind::Files)?;
            roots.push(destination);
        } else {
            roots.push(Arc::new(Root::open(&self.state)?));
        }
        if self.drive(selector)? != drive {
            return Err("Drive changed while pinning bulk-operation roots".into());
        }
        Ok((drive, roots))
    }
    pub(super) fn launch_bulk(
        &mut self,
        envelope: Envelope,
        drive: Drive,
        config: DriveConfig,
        roots: Vec<Arc<Root>>,
        context: Option<partition::BackupContext>,
    ) {
        let drive_id = drive.id.clone();
        let worker = worker::Worker {
            state: self.state.clone(),
            id: envelope.id.clone(),
            request: envelope.request.clone(),
            drive,
            config,
            roots: roots.clone(),
            context: context.clone(),
            cancel: envelope.cancel.clone(),
            authority: self.authority.clone(),
            inventory: self.inventory.clone(),
        };
        self.launch(envelope, Some(drive_id), roots, context, move || {
            worker.run()
        });
    }
    fn launch(
        &mut self,
        envelope: Envelope,
        drive_id: Option<String>,
        roots: Vec<Arc<Root>>,
        context: Option<partition::BackupContext>,
        work: impl FnOnce() -> Result<String> + Send + 'static,
    ) {
        if let Ok(Some(mut operation)) = self.db.get::<DriveOperation>("operation", &envelope.id) {
            operation.state = "running".into();
            let _ = self.db.put("operation", &envelope.id, &operation);
        }
        let destination = matches!(
            envelope.request,
            DriveRequest::Backup { .. } | DriveRequest::ReadOnlyBackup { .. }
        )
        .then(|| roots.last().map(|root| root.path.clone()))
        .flatten();
        let cleanup_pending = context
            .as_ref()
            .is_some_and(|context| context.file_mode == FileBackupMode::ReadOnly);
        let catalog = matches!(
            envelope.request,
            DriveRequest::Catalog { .. } | DriveRequest::CatalogWithLimits { .. }
        );
        let task = write_task(&self.backup_jobs, work);
        self.background.insert(
            envelope.id.clone(),
            Pending {
                drive: drive_id,
                catalog,
                roots,
                destination,
                context,
                cleanup_pending,
                id: envelope.id,
                cancel: envelope.cancel,
                task,
            },
        );
    }
    pub(super) fn poll_background(&mut self) {
        let completed = self
            .background
            .iter()
            .filter_map(|(id, job)| job.task.poll().map(|result| (id.clone(), result)))
            .collect::<Vec<_>>();
        for (id, result) in completed {
            let mut job = self.background.remove(&id).expect("polled retained job");
            let result = write_result(result);
            if job.cleanup_pending {
                let context = job.context.clone().expect("read-only cleanup context");
                // Delivery proves the worker closure returned. Release the
                // coordinator's source pin before cleanup can unmount it.
                job.roots.clear();
                let state = self.state.clone();
                let id = job.id.clone();
                let inventory = self.inventory.clone();
                let authority = self.authority.clone();
                // Backups are exclusive, reserving the bulk queue for cleanup.
                job.task = write_task(&self.backup_jobs, move || {
                    let db = Store::existing(&state.join("drives.sqlite"), false);
                    context.finish(
                        db.as_ref().map_err(|error| error.to_string().into()),
                        &id,
                        &inventory,
                        &authority,
                        result,
                    )
                });
                job.cleanup_pending = false;
                self.background.insert(job.id.clone(), job);
                continue;
            }
            self.finish_operation(&job.id, &job.cancel, result);
            self.automatic();
        }
    }
    pub(super) fn finish_operation(
        &mut self,
        id: &str,
        cancel: &AtomicBool,
        result: Result<String>,
    ) {
        // Status combines shared settings with durable operation pages. Publish
        // the accepted settings before a completed record becomes visible; the
        // later inventory refresh can be slow or unavailable.
        if let Ok(mut status) = self.shared.lock() {
            status.config = self.config.clone();
            status.backup_directory = self.config.backup_directory.clone();
        }
        if let Err(error) = finish_record(&self.db, id, cancel, result) {
            tracing::error!(operation = id, %error, "Could not persist storage operation completion");
        }
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(id);
        }
        self.request_refresh(false);
        let _ = self.publish_policy(false);
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
