//! Settings publish only after retained filesystem preparation and fresh metadata admission.
use super::*;
use little_clip_core::{jobs::task::Task, mount_inventory::Snapshot};

pub(super) struct Configuration {
    jobs: little_clip_core::jobs::BackgroundJobs,
    pending: Option<Pending>,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            jobs: little_clip_core::jobs::BackgroundJobs::new_named(1, 1, "drive-settings"),
            pending: None,
        }
    }
}
struct Pending {
    envelope: Envelope,
    inputs: Inputs,
    task: Task<Result<Prepared>>,
}
pub(super) struct Inputs {
    previous: DriveConfig,
    generation: u64,
    roots: Option<(policy::Storage, Vec<SyncPair>)>,
}
struct Prepared {
    config: DriveConfig,
    // Keep the validated target pinned until metadata publication.
    _target: Option<Root>,
    mounts: Option<String>,
}
impl Prepared {
    fn prepare(state: &Path, config: DriveConfig, cancel: &AtomicBool) -> Result<Self> {
        cancelled(cancel)?;
        let db = Store::existing(&state.join("drives.sqlite"), false)?;
        policy::validate_config(&config)?;
        validate_quotas(&db, &config)?;
        let mounts = config
            .backup_directory
            .as_ref()
            .map(|_| Snapshot::capture().map(|snapshot| snapshot.signature.clone()))
            .transpose()?;
        let target = config
            .backup_directory
            .as_ref()
            .map(|path| {
                let root = Root::create(path)?;
                policy::validate_backup_storage(&db, state, &root)?;
                root.validate()?;
                Ok::<_, Error>(root)
            })
            .transpose()?;
        cancelled(cancel)?;
        if let Some(mounts) = &mounts
            && Snapshot::capture()?.signature != *mounts
        {
            return Err("Mount inventory changed during configuration preparation; retry".into());
        }
        Ok(Self {
            config,
            _target: target,
            mounts,
        })
    }
}
impl Inputs {
    pub fn begin(owner: &mut Owner, roots: bool) -> Result<Self> {
        owner.settings_generation = owner
            .settings_generation
            .checked_add(1)
            .expect("settings revision exhausted");
        let roots = roots
            .then(|| {
                Ok::<_, Error>((
                    policy::Storage::capture(
                        &owner.db,
                        &owner.state,
                        owner.config.backup_directory.as_deref(),
                    )?,
                    policy::records::<SyncPair>(&owner.db, "pair")?,
                ))
            })
            .transpose()?;
        Ok(Self {
            previous: owner.config.clone(),
            generation: owner.settings_generation,
            roots,
        })
    }

    pub fn current(&self, owner: &Owner, mounts: Option<&str>) -> Result<bool> {
        if owner.config != self.previous || owner.settings_generation != self.generation {
            return Ok(false);
        }
        if let Some((storage, targets)) = &self.roots
            && (policy::Storage::capture(
                &owner.db,
                &owner.state,
                owner.config.backup_directory.as_deref(),
            )? != *storage
                || !policy::same_pair_roots(&owner.db, targets)?)
        {
            return Ok(false);
        }
        Ok(match mounts {
            Some(mounts) => Snapshot::capture()?.signature == mounts,
            None => true,
        })
    }
}

impl Prepared {
    fn apply(self, owner: &mut Owner, pending: &Pending) -> Result<String> {
        cancelled(&pending.envelope.cancel)?;
        if owner.closing.load(Ordering::Acquire) {
            return Err("Daemon stopping; prepared configuration was not applied".into());
        }
        let transaction = owner.db.0.unchecked_transaction()?;
        if !pending.inputs.current(owner, self.mounts.as_deref())? {
            return Err("Configuration inputs changed during preparation; created directories are retained, retry".into());
        }
        validate_quotas(&owner.db, &self.config)?;
        owner.db.put("settings", "config", &self.config)?;
        transaction.commit()?;
        owner.config = self.config;
        Ok("Drive configuration reloaded".into())
    }
}

fn validate_quotas(db: &Store, config: &DriveConfig) -> Result<()> {
    let budgets = policy::records::<RamSession>(db, "ram")?
        .iter()
        .map(|session| crate::ram::Budget {
            bytes: session
                .backing
                .as_ref()
                .map_or(0, |backing| backing.capacity),
            inodes: session.backing.as_ref().map_or(0, |backing| backing.inodes),
        })
        .collect::<Vec<_>>();
    crate::ram::validate_quotas(config, &budgets)
}
impl Configuration {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}
impl Owner {
    pub(super) fn start_configuration(&mut self, envelope: Envelope) {
        if self.configuration.busy() || self.preferences.committing() {
            self.defer_operation(envelope);
            return;
        }
        let DriveRequest::Configure { config } = &envelope.request else {
            unreachable!("configuration request");
        };
        let config = config.clone();
        let inputs = match Inputs::begin(self, config.backup_directory.is_some()) {
            Ok(inputs) => inputs,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        let state = self.state.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.configuration.jobs, move || {
            Prepared::prepare(&state, config, &cancel)
        });
        self.configuration.pending = Some(Pending {
            envelope,
            inputs,
            task,
        });
    }
    pub(super) fn poll_configuration(&mut self) {
        let Some(delivered) = self
            .configuration
            .pending
            .as_ref()
            .and_then(|pending| pending.task.poll())
        else {
            return;
        };
        let pending = self
            .configuration
            .pending
            .take()
            .expect("delivered settings preparation");
        let result = write_result(delivered).and_then(|prepared| prepared.apply(self, &pending));
        self.finish_operation(&pending.envelope.id, &pending.envelope.cancel, result);
    }
    pub(super) fn drain_configuration(&mut self) {
        while self.configuration.pending.is_some() || self.preferences.pending.is_some() {
            if let Some(pending) = &self.configuration.pending {
                pending.envelope.cancel.store(true, Ordering::Release);
            }
            if let Some(pending) = &self.preferences.pending {
                pending.cancel();
            }
            self.poll_configuration();
            self.poll_preference();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
