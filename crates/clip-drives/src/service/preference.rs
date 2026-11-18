//! Explicit preferences retain both filesystem completion and reviewed-backup roots.
use super::*;
use little_clip_core::{jobs::task::Task, mount_inventory::Snapshot};

pub(super) enum Continuation {
    Explicit,
    Bulk {
        drive: Box<Drive>,
        roots: Vec<Arc<Root>>,
    },
    Partition(Box<partition::Admission>),
}
pub(super) struct Preferences {
    jobs: little_clip_core::jobs::BackgroundJobs,
    pub pending: Option<Pending>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            jobs: little_clip_core::jobs::BackgroundJobs::new_named(1, 1, "drive-preference"),
            pending: None,
        }
    }
}
impl Preferences {
    pub fn writing(&self) -> bool {
        self.pending.as_ref().is_some_and(Pending::writing)
    }
    pub fn committing(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.stage, Stage::Saving(_)))
    }
}
pub(super) struct Pending {
    pub drive: Option<String>,
    pub claims: Vec<crate::root::Scope>,
    envelope: Envelope,
    inputs: configuration::Inputs,
    continuation: Continuation,
    stage: Stage,
}
enum Stage {
    Preparing(Task<Result<Prepared>>),
    Saving(Task<Result<Written>>),
    Ready,
}
struct Prepared {
    target: Arc<Root>,
    mounts: String,
}
struct Written {
    selected: Option<DriveConfig>,
    result: Result<String>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Receipt {
    #[serde(with = "little_clip_core::path_serde")]
    destination: PathBuf,
    backup: bool,
}
impl Pending {
    pub fn writing(&self) -> bool {
        !matches!(self.stage, Stage::Ready)
    }
    pub fn cancel(&self) {
        self.envelope.cancel.store(true, Ordering::Release);
    }
    fn ready(&self, owner: &Owner) -> bool {
        match self.continuation {
            Continuation::Explicit => true,
            Continuation::Bulk { .. } => {
                owner.background.is_empty()
                    && !owner.maintenance.busy()
                    && !owner.partition_backup_pending()
            }
            Continuation::Partition(_) => {
                !owner.partition_busy() && owner.background.is_empty() && !owner.maintenance.busy()
            }
        }
    }
}
impl Prepared {
    fn prepare(
        state: &Path,
        request: &DriveRequest,
        review: Option<&preflight::Review>,
        claims: &[crate::root::Scope],
        inventory: &Inventory,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        cancelled(cancel)?;
        let mounts = Snapshot::capture()?.signature.clone();
        let target = Self::destination(request, review, claims, inventory)?;
        let db = Store::existing(&state.join("drives.sqlite"), false)?;
        policy::validate_backup_storage(&db, state, &target)?;
        target.validate()?;
        cancelled(cancel)?;
        if Snapshot::capture()?.signature != mounts {
            return Err("Mount inventory changed during preference preparation; retry".into());
        }
        Ok(Self { target, mounts })
    }
    fn destination(
        request: &DriveRequest,
        review: Option<&preflight::Review>,
        claims: &[crate::root::Scope],
        inventory: &Inventory,
    ) -> Result<Arc<Root>> {
        if let Some(review) = review {
            let drive = discovery::select(inventory()?, &review.info.drive.id)?;
            review.validate(&drive)?;
            preflight::space(&review.destination, &drive, review.info.kind)?;
            Ok(review.destination.clone())
        } else {
            let DriveRequest::BackupDirectory { path } = request else {
                return Err("Missing backup preference destination".into());
            };
            if Root::planned_scope(path)?
                != *claims
                    .last()
                    .ok_or("Missing preference destination claim")?
            {
                return Err("Preference destination changed since admission; retry".into());
            }
            Ok(Arc::new(Root::create(path)?))
        }
    }
    fn write(
        self,
        state: &Path,
        path: Option<&Path>,
        mut config: DriveConfig,
        operation: &str,
        backup: bool,
        cancel: &AtomicBool,
    ) -> Written {
        let mut selected = None;
        let result = (|| {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            self.target.validate()?;
            policy::validate_backup_storage(&db, state, &self.target)?;
            cancelled(cancel)?;
            config.backup_directory = Some(self.target.path.clone());
            if let Some(path) = path {
                little_clip_core::config::save_drive_backup_directory_at(path, &self.target.path)
                    .map_err(|error| format!("Could not save backup directory preference: {error}; installation may be incomplete, reload before retrying"))?;
                // An accepted file write is a completed effect even if later
                // registry publication fails or cancellation has arrived.
                selected = Some(config.clone());
            }
            let transaction = db.0.unchecked_transaction()?;
            db.put("settings", "config", &config)?;
            db.put(
                "preference",
                operation,
                &Receipt {
                    destination: self.target.path.clone(),
                    backup,
                },
            )?;
            transaction.commit()?;
            selected = Some(config);
            Ok("Backup directory configured".into())
        })();
        let result = result.map_err(|error: Error| {
            if selected.is_some() {
                format!("Backup preference file saved; registry publication failed: {error}").into()
            } else {
                error
            }
        });
        Written { selected, result }
    }
}
pub(super) fn annotate(db: &Store, id: &str, result: Result<String>) -> Result<String> {
    let suffix = match db.get::<Receipt>("preference", id) {
        Ok(Some(receipt)) if receipt.backup => Some(format!(
            "Backup directory preference saved: {}",
            receipt.destination.display()
        )),
        Err(error) => Some(format!("Preference result could not be read: {error}")),
        _ => None,
    };
    match (suffix, result) {
        (Some(suffix), Ok(message)) => Ok(format!("{message}; {suffix}")),
        (Some(suffix), Err(error)) => Err(format!("{error}; {suffix}").into()),
        (None, result) => result,
    }
}
impl Owner {
    pub(super) fn preference_blocked(&self, envelope: &Envelope) -> bool {
        envelope
            .review
            .as_ref()
            .is_some_and(|review| review.remember)
            && (self.preferences.pending.is_some() || self.ram_setup.is_some())
    }
    pub(super) fn start_preference(
        &mut self,
        envelope: Envelope,
        continuation: Continuation,
        drive: Option<String>,
        claims: Vec<crate::root::Scope>,
    ) {
        if self.preferences.pending.is_some() || self.ram_setup.is_some() {
            self.defer_operation(envelope);
            return;
        }
        let inputs = match configuration::Inputs::begin(self, true) {
            Ok(inputs) => inputs,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        let state = self.state.clone();
        let request = envelope.request.clone();
        let review = envelope.review.clone();
        let worker_claims = claims.clone();
        let inventory = self.inventory.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.preferences.jobs, move || {
            Prepared::prepare(
                &state,
                &request,
                review.as_ref(),
                &worker_claims,
                &inventory,
                &cancel,
            )
        });
        self.preferences.pending = Some(Pending {
            drive,
            claims,
            envelope,
            inputs,
            continuation,
            stage: Stage::Preparing(task),
        });
    }
    pub(super) fn poll_preference(&mut self) {
        let Some(mut pending) = self.preferences.pending.take() else {
            return;
        };
        let result = match &pending.stage {
            Stage::Preparing(task) => {
                let Some(result) = task.poll() else {
                    self.preferences.pending = Some(pending);
                    return;
                };
                write_result(result)
                    .and_then(|prepared| self.save_preference(&pending, prepared))
                    .map(|stage| {
                        pending.stage = stage;
                    })
            }
            Stage::Saving(task) => {
                let Some(result) = task.poll() else {
                    self.preferences.pending = Some(pending);
                    return;
                };
                write_result(result).and_then(|written| {
                    if let Some(config) = written.selected {
                        self.config = config;
                    }
                    written.result.map(|_| {
                        pending.stage = Stage::Ready;
                    })
                })
            }
            Stage::Ready => Ok(()),
        };
        if let Err(error) = result {
            self.finish_operation(&pending.envelope.id, &pending.envelope.cancel, Err(error));
            return;
        }
        if !matches!(pending.stage, Stage::Ready) {
            self.preferences.pending = Some(pending);
            return;
        }
        if matches!(pending.continuation, Continuation::Explicit) {
            self.finish_operation(
                &pending.envelope.id,
                &pending.envelope.cancel,
                Ok("Backup directory configured".into()),
            );
        } else if pending.envelope.cancel.load(Ordering::Acquire)
            || self.closing.load(Ordering::Acquire)
        {
            self.finish_operation(
                &pending.envelope.id,
                &pending.envelope.cancel,
                Err("Backup cancelled after saving its directory preference".into()),
            );
        } else if !pending.ready(self) {
            self.preferences.pending = Some(pending);
        } else {
            if let Some(review) = &mut pending.envelope.review {
                review.remember = false;
            }
            match pending.continuation {
                Continuation::Bulk { drive, roots } => {
                    self.continue_bulk(pending.envelope, *drive, roots)
                }
                Continuation::Partition(admission) => {
                    self.continue_partition(pending.envelope, *admission)
                }
                Continuation::Explicit => unreachable!(),
            }
        }
    }
    fn save_preference(&self, pending: &Pending, prepared: Prepared) -> Result<Stage> {
        cancelled(&pending.envelope.cancel)?;
        if self.closing.load(Ordering::Acquire) {
            return Err("Daemon stopping; preference was not saved".into());
        }
        if !pending.inputs.current(self, Some(&prepared.mounts))? {
            return Err("Preference inputs changed during preparation; retry".into());
        }
        let state = self.state.clone();
        let path = self.config_path.clone();
        let config = self.config.clone();
        let id = pending.envelope.id.clone();
        let backup = !matches!(pending.continuation, Continuation::Explicit);
        let cancel = pending.envelope.cancel.clone();
        Ok(Stage::Saving(write_task(
            &self.preferences.jobs,
            move || Ok(prepared.write(&state, path.as_deref(), config, &id, backup, &cancel)),
        )))
    }
}
