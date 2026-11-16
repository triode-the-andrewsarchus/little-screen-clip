//! One retained bulk worker leaves prompt writeback on the storage coordinator.
use super::*;
use little_clip_core::jobs::task::Task;

pub(super) struct Pending {
    pub(super) drive: String,
    pub(super) roots: Vec<Arc<Root>>,
    id: String,
    pub(super) cancel: Arc<AtomicBool>,
    task: Task<Result<String>>,
}
impl Owner {
    pub(super) fn check_busy(&self, drive: &str) -> Result<()> {
        if self
            .ram_setup
            .as_ref()
            .is_some_and(|job| job.drive.id == drive)
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
    pub(super) fn unmount_partition(&mut self, drive: &Drive) -> Result<()> {
        if let Some(path) = &drive.mount {
            self.sync_roots.retain(|_, (portable, local)| {
                !portable.path.starts_with(path) && !local.path.starts_with(path)
            });
            self.unidentified_ram
                .retain(|_, root| !root.path.starts_with(path));
            if let Some(watch) = &mut self.watch {
                watch.remove_tree(path);
            }
        }
        let output = CommandRequest::new("udisksctl")
            .args(["unmount", "--block-device"])
            .arg(&drive.device)
            .timeout(Duration::from_secs(60))
            .own_process_group()
            .run()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Could not unmount partition; local and RAM edits retained: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(())
    }
    fn prepare_backup(&mut self, drive: &str, image: bool, cancel: &AtomicBool) -> Result<Drive> {
        let selector = drive.to_owned();
        let mut drive = self.drive(drive)?;
        if image && drive.mount.is_some() {
            for pair in self.db.list::<SyncPair>("pair", 0, 1000)? {
                if pair.drive == drive.id && !pair.paused {
                    self.synchronize(&pair.id, cancel, &mut |_, _| {})?;
                }
            }
            self.unmount_partition(&drive)?;
            drive = self.drive(&selector)?;
        }
        Ok(drive)
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
        let resources = (|| -> Result<Vec<Arc<Root>>> {
            let selector = match &envelope.request {
                DriveRequest::Backup { drive, .. } | DriveRequest::Catalog { drive } => drive,
                _ => return Err("Missing bulk drive".into()),
            };
            let drive = self.drive(selector)?;
            let mut roots = Vec::new();
            if let Some(path) = drive.mount {
                roots.push(Arc::new(Root::open(&path)?));
            }
            if matches!(envelope.request, DriveRequest::Backup { .. })
                && let Some(path) = &self.config.backup_directory
            {
                roots.push(Arc::new(Root::create(path)?));
            }
            Ok(roots)
        })();
        if let Err(error) = resources.as_ref() {
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err(error.to_string().into()),
            );
            return;
        }
        if let Ok(roots) = &resources
            && (self
                .ram_setup
                .as_ref()
                .is_some_and(|job| roots.iter().any(|root| job.source.overlap(root)))
                || self
                    .sync_jobs
                    .values()
                    .any(|job| roots.iter().any(|root| job.plan.overlaps_root(root))))
        {
            self.defer_operation(envelope);
            return;
        }
        // Pre-unmount flushes must not retain the admission pins themselves.
        drop(resources);
        let prepared = (|| {
            if self.background.is_some() {
                return Err("One backup/catalog is already active; retry after it finishes".into());
            }
            match &envelope.request {
                DriveRequest::Backup { drive, kind } => {
                    self.prepare_backup(drive, *kind == BackupKind::Image, &envelope.cancel)
                }
                DriveRequest::Catalog { drive } => self.drive(drive),
                _ => Err("Invalid bulk operation".into()),
            }
        })();
        let drive = match prepared {
            Ok(drive) => drive,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        if let Ok(Some(mut operation)) = self.db.get::<DriveOperation>("operation", &envelope.id) {
            operation.state = "running".into();
            let _ = self.db.put("operation", &envelope.id, &operation);
        }
        let state = self.state.clone();
        let config = self.config.clone();
        let id = envelope.id.clone();
        let cancel = envelope.cancel.clone();
        let drive_id = drive.id.clone();
        let roots = (|| -> Result<Vec<Arc<Root>>> {
            let mut roots = Vec::new();
            if let Some(path) = &drive.mount {
                roots.push(Arc::new(Root::open(path)?));
            }
            if let Some(path) = &self.config.backup_directory {
                roots.push(Arc::new(Root::open(path)?));
            }
            Ok(roots)
        })();
        let roots = match roots {
            Ok(roots) => roots,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        let task = write_task(&self.backup_jobs, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut progress = job_progress(state.clone(), id);
            cancelled(&cancel)?;
            match envelope.request {
                DriveRequest::Backup { kind, .. } => {
                    backup::recover_publications(&db)?;
                    let destination = config
                        .backup_directory
                        .as_ref()
                        .ok_or("Backup directory unset")?;
                    let version = match kind {
                        BackupKind::Files => {
                            backup::files(&db, &drive, destination, &cancel, &mut progress)?
                        }
                        BackupKind::Image => {
                            backup::image(&db, &drive, destination, &cancel, &mut progress)?
                        }
                    };
                    Ok(format!(
                        "Backup completed: {} ({} bytes written)",
                        version.path.display(),
                        version.bytes_written
                    ))
                }
                DriveRequest::Catalog { .. } => {
                    let catalog =
                        catalog::capture(&db, &state, &drive, &config, &cancel, &mut progress)?;
                    Ok(format!(
                        "Catalog saved: {} · {}",
                        catalog.id, catalog.coverage
                    ))
                }
                _ => Err("Invalid bulk operation".into()),
            }
        });
        self.background = Some(Pending {
            drive: drive_id,
            roots,
            id: envelope.id,
            cancel: envelope.cancel,
            task,
        });
    }
    pub(super) fn poll_background(&mut self) {
        let result = self.background.as_ref().and_then(|job| job.task.poll());
        if let Some(result) = result {
            let job = self.background.take().expect("polled retained job");
            let result = result.unwrap_or_else(|e| {
                Err(format!("Storage worker failed: {e}; completed writes retained").into())
            });
            self.finish_operation(&job.id, &job.cancel, result);
            self.automatic();
        }
    }
    pub(super) fn finish_operation(&self, id: &str, cancel: &AtomicBool, result: Result<String>) {
        if let Ok(Some(mut operation)) = self.db.get::<DriveOperation>("operation", id) {
            operation.finished = Some(now());
            match result {
                Ok(message) => {
                    operation.state = "complete".into();
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
            let _ = self.db.put("operation", id, &operation);
        }
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(id);
        }
        self.discover();
        let _ = self.publish_policy();
    }
}
