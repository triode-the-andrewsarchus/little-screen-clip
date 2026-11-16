//! One retained bulk worker leaves prompt writeback on the storage coordinator.
use super::*;
use little_clip_core::jobs::task::Task;

pub(super) struct Pending {
    drive: String,
    id: String,
    pub(super) cancel: Arc<AtomicBool>,
    task: Task<Result<String>>,
}
impl Owner {
    pub(super) fn check_busy(&self, drive: &str) -> Result<()> {
        if self
            .background
            .as_ref()
            .is_some_and(|job| job.drive == drive)
        {
            return Err("This partition has an active backup or catalog; synchronization resumes when it finishes".into());
        }
        Ok(())
    }
    fn prepare_backup(&mut self, drive: &str, image: bool, cancel: &AtomicBool) -> Result<Drive> {
        let mut drive = self.drive(drive)?;
        if image && drive.mount.is_some() {
            for pair in self.db.list::<SyncPair>("pair", 0, 1000)? {
                if pair.drive == drive.id && !pair.paused {
                    self.synchronize(&pair.id, cancel, &mut |_, _| {})?;
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
                    "Cannot unmount partition for imaging: {}",
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
            drive = self.drive(&drive.id)?;
        }
        Ok(drive)
    }
    pub(super) fn start_background(&mut self, envelope: Envelope) {
        let prepared = (|| {
            if self.background.is_some() {
                return Err("One backup/catalog is already active; retry after it finishes".into());
            }
            match &envelope.request {
                DriveRequest::Backup { drive, kind } => {
                    if self.config.backup_directory.is_none() {
                        return Err("Choose a DriveBackupDir before starting a backup".into());
                    }
                    self.prepare_backup(drive, *kind == BackupKind::Image, &envelope.cancel)
                }
                DriveRequest::Catalog { drive } => self.drive(drive),
                _ => Err("Invalid bulk operation".into()),
            }
        })();
        let drive = match prepared {
            Ok(drive) => drive,
            Err(error) => {
                self.finish_background(&envelope.id, &envelope.cancel, Err(error));
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
        let task = Task::start_unbounded(&self.backup_jobs, move |_| {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut last = Instant::now() - Duration::from_secs(1);
            let mut progress = |files, bytes| {
                if last.elapsed() < Duration::from_millis(250) {
                    return;
                }
                last = Instant::now();
                if let Ok(Some(mut operation)) = db.get::<DriveOperation>("operation", &id) {
                    operation.files = files;
                    operation.bytes = bytes;
                    let _ = db.put("operation", &id, &operation);
                }
            };
            cancelled(&cancel)?;
            match envelope.request {
                DriveRequest::Backup { kind, .. } => {
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
            self.finish_background(&job.id, &job.cancel, result);
            self.automatic();
        }
    }
    fn finish_background(&self, id: &str, cancel: &AtomicBool, result: Result<String>) {
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
