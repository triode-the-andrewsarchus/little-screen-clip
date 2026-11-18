//! Accepted partition preparation retains metadata snapshots and actual worker exit.
use super::*;
use little_clip_core::mount_inventory::Snapshot;

pub(in crate::service) struct Pending {
    pub drive: String,
    pub claims: Vec<Scope>,
    envelope: Envelope,
    input: Arc<Input>,
    task: Task<Result<Prepared>>,
}
impl Pending {
    fn is_backup(&self) -> bool {
        super::is_backup(&self.envelope.request)
    }
}
struct Input {
    expected: Drive,
    state: PathBuf,
    backup: Option<PathBuf>,
    kind: Option<BackupKind>,
    review: Option<preflight::Review>,
    pairs: Vec<reconciliation::preparation::Input>,
    incomplete_ram: bool,
    settings: configuration::Inputs,
    epoch: u64,
}
struct Prepared {
    admission: Admission,
    mounts: String,
}
impl Input {
    fn capture(owner: &Owner, envelope: &Envelope, expected: Drive) -> Result<Self> {
        let transaction = owner.db.0.unchecked_transaction()?;
        let settings = configuration::Inputs::capture(owner, true)?;
        let mut pairs = Vec::new();
        let mut incomplete_ram = false;
        for pair in policy::records::<SyncPair>(&owner.db, "pair")? {
            if pair.drive != expected.id || (pair.paused && !pair.ram) {
                continue;
            }
            if pair.ram
                && owner
                    .db
                    .get::<RamSession>("ram", &pair.id)?
                    .is_some_and(|session| !session.ready)
            {
                incomplete_ram = true;
            }
            pairs.push(reconciliation::preparation::Input::existing(owner, pair)?);
        }
        let kind = match envelope.request {
            DriveRequest::Backup { kind, .. } => Some(kind),
            DriveRequest::ReadOnlyBackup { .. } => Some(BackupKind::Files),
            _ => None,
        };
        transaction.commit()?;
        Ok(Self {
            expected,
            state: owner.state.clone(),
            backup: owner.config.backup_directory.clone(),
            kind,
            review: envelope.review.clone(),
            pairs,
            incomplete_ram,
            settings,
            epoch: owner.sync_epoch,
        })
    }
    fn prepare(&self, inventory: &Inventory, cancel: &AtomicBool) -> Result<Prepared> {
        cancelled(cancel)?;
        let mounts = Snapshot::capture()?.signature.clone();
        let drive = discovery::select(inventory()?, &self.expected.id)?;
        self.validate_drive(&drive)?;
        cancelled(cancel)?;
        let admission = self.pin(drive, cancel)?;
        cancelled(cancel)?;
        if discovery::select(inventory()?, &admission.drive.id)? != admission.drive {
            return Err("Drive changed while pinning pre-unmount roots".into());
        }
        if Snapshot::capture()?.signature != mounts {
            return Err("Mount inventory changed during partition preparation; retry".into());
        }
        Ok(Prepared { admission, mounts })
    }
    fn validate_drive(&self, drive: &Drive) -> Result<()> {
        if drive != &self.expected {
            return Err("Partition changed since scheduling; retry".into());
        }
        if self.kind == Some(BackupKind::Image) && drive.capacity == 0 {
            return Err("Partition capacity is unknown; cannot create a complete image".into());
        }
        if self.kind == Some(BackupKind::Files) && drive.mount.is_none() {
            return Err("Mount the partition before selecting read-only file collection".into());
        }
        Ok(())
    }
    fn pin(&self, drive: Drive, cancel: &AtomicBool) -> Result<Admission> {
        let source = self
            .kind
            .is_some()
            .then(|| crate::device::Source::capture(&drive).map(Arc::new))
            .transpose()?;
        let flush = preflight::source(self.review.as_ref(), &drive)?;
        cancelled(cancel)?;
        let destination = self.destination(&drive, flush.as_deref())?;
        let plans = if flush.is_some() {
            self.plans(&drive, cancel)?
        } else {
            Vec::new()
        };
        let mut claims = Vec::new();
        claims.extend(flush.as_ref().map(|root| root.scope()));
        claims.extend(destination.as_ref().map(|root| root.scope()));
        for plan in &plans {
            claims.extend(plan.scopes());
        }
        Ok(Admission {
            drive,
            source,
            flush,
            destination,
            plans,
            claims,
        })
    }
    fn destination(&self, drive: &Drive, source: Option<&Root>) -> Result<Option<Arc<Root>>> {
        self.kind
            .map(|kind| {
                let db = Store::existing(&self.state.join("drives.sqlite"), false)?;
                let root = preflight::destination(
                    &db,
                    &self.state,
                    self.backup.as_deref(),
                    self.review.as_ref(),
                    source,
                )?;
                preflight::space(&root, drive, kind)?;
                Ok(root)
            })
            .transpose()
    }
    fn plans(&self, drive: &Drive, cancel: &AtomicBool) -> Result<Vec<reconciliation::Plan>> {
        if self.incomplete_ram {
            return Err("RAM population is incomplete; resume synchronization or explicitly discard this RAM copy before ejecting or imaging. Save it elsewhere first if you need its current contents".into());
        }
        self.pairs
            .iter()
            .map(|input| {
                cancelled(cancel)?;
                input.prepare_pair(drive)
            })
            .collect()
    }
    fn current(&self, owner: &Owner, mounts: &str) -> Result<bool> {
        if self.epoch != owner.sync_epoch || !self.settings.current(owner, Some(mounts))? {
            return Ok(false);
        }
        for input in &self.pairs {
            if !input.current(owner)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
impl Prepared {
    fn accept(self, owner: &Owner, input: &Input) -> Result<Admission> {
        if !input.current(owner, &self.mounts)? {
            return Err(
                "Partition preparation inputs changed; created directories are retained, retry"
                    .into(),
            );
        }
        owner.check_busy(&self.admission.drive.id)?;
        if owner.claims_busy(&self.admission.claims) {
            return Err("Partition preparation overlaps an active storage operation; retry after it finishes".into());
        }
        Ok(self.admission)
    }
}
impl Owner {
    pub(in crate::service) fn partition_busy(&self) -> bool {
        self.partition.is_some() || self.partition_preparing.is_some()
    }
    pub(in crate::service) fn partition_backup_pending(&self) -> bool {
        self.partition
            .as_ref()
            .is_some_and(super::Pending::is_backup)
            || self
                .partition_preparing
                .as_ref()
                .is_some_and(Pending::is_backup)
    }
    pub(in crate::service) fn start_partition(
        &mut self,
        envelope: Envelope,
        selected: Option<Drive>,
        mut claims: Vec<Scope>,
    ) {
        if self.preference_blocked(&envelope)
            || self.partition_busy()
            || (super::is_backup(&envelope.request)
                && (!self.background.is_empty() || self.maintenance.busy()))
        {
            self.defer_operation(envelope);
            return;
        }
        let input = (|| {
            let drive = selected.ok_or("Missing scheduled partition identity")?;
            self.check_busy(&drive.id)?;
            Input::capture(self, &envelope, drive).map(Arc::new)
        })();
        let input = match input {
            Ok(input) => input,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        if let Some(review) = &envelope.review {
            claims.push(review.destination.scope());
        }
        let worker = input.clone();
        let inventory = self.inventory.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.partition_jobs, move || {
            worker.prepare(&inventory, &cancel)
        });
        self.partition_preparing = Some(Pending {
            drive: input.expected.id.clone(),
            claims,
            envelope,
            input,
            task,
        });
    }
    pub(in crate::service) fn poll_partition_preparation(&mut self) {
        let Some(delivered) = self
            .partition_preparing
            .as_ref()
            .and_then(|job| job.task.poll())
        else {
            return;
        };
        let job = self
            .partition_preparing
            .take()
            .expect("delivered partition preparation");
        let result = cancelled(&job.envelope.cancel).and(write_result(delivered));
        let result = result.and_then(|prepared| {
            if self.closing.load(Ordering::Acquire) {
                return Err(
                    "Daemon stopping; partition preparation retired before execution".into(),
                );
            }
            prepared.accept(self, &job.input)
        });
        let admission = match result {
            Ok(admission) => admission,
            Err(error) => {
                self.finish_operation(&job.envelope.id, &job.envelope.cancel, Err(error));
                return;
            }
        };
        if job
            .envelope
            .review
            .as_ref()
            .is_some_and(|review| review.remember)
        {
            let drive = admission.drive.id.clone();
            let claims = admission.claims.clone();
            self.start_preference(
                job.envelope,
                preference::Continuation::Partition(Box::new(admission)),
                Some(drive),
                claims,
            );
        } else {
            self.continue_partition(job.envelope, admission);
        }
    }
    pub(in crate::service) fn drain_partition_preparation(&mut self) {
        while let Some(job) = &self.partition_preparing {
            job.envelope.cancel.store(true, Ordering::Release);
            self.poll_partition_preparation();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
