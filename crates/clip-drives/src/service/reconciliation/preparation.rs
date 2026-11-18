//! Fresh root preparation shares reconciliation's retained four-worker budget.
use super::*;
use little_clip_core::mount_inventory::Snapshot;

pub(super) struct Input {
    pub pair: SyncPair,
    storage: policy::Storage,
    ram: ram_sessions::Reconciliation,
    cached: Option<(Arc<Root>, Arc<Root>)>,
    observed: bool,
    enrollment: Option<enrollment::Input>,
}
impl Input {
    pub fn capture(
        owner: &Owner,
        pair: SyncPair,
        enrollment: Option<enrollment::Input>,
    ) -> Result<Self> {
        Ok(Self {
            storage: policy::Storage::capture(
                &owner.db,
                &owner.state,
                owner.config.backup_directory.as_deref(),
            )?,
            ram: ram_sessions::Reconciliation::capture(owner, &pair.id)?,
            cached: owner.sync_roots.get(&pair.id).cloned(),
            observed: owner
                .watch
                .as_ref()
                .is_some_and(Watch::has_complete_coverage),
            pair,
            enrollment,
        })
    }
    fn current(&self, owner: &Owner) -> Result<bool> {
        let pair = owner.db.get::<SyncPair>("pair", &self.pair.id)?;
        let current = if let Some(enrollment) = &self.enrollment {
            pair.is_none() && enrollment.current(&owner.db)?
        } else {
            pair.as_ref() == Some(&self.pair)
        };
        Ok(current
            && policy::Storage::capture(
                &owner.db,
                &owner.state,
                owner.config.backup_directory.as_deref(),
            )? == self.storage
            && self.ram.current(owner, &self.pair.id)?)
    }
    pub fn prepare_pair(&self, drive: &Drive) -> Result<Plan> {
        let pair = self.pair.clone();
        if drive.ambiguous || drive.evidence != pair.evidence {
            return Err("Drive identity differs from the enrollment; sync suspended".into());
        }
        let initial_ram = self.ram.validate(&pair, drive)?;
        if drive.read_only && !initial_ram {
            return Err("Drive is read-only; edits remain local".into());
        }
        let mount = drive
            .mount
            .as_ref()
            .ok_or("Drive is offline; local contents retained")?;
        let partition = Root::open(mount)?;
        let portable = Arc::new(partition.subtree(&pair.relative)?);
        let local = Arc::new(Root::open(&pair.local)?);
        if pair.local_identity != Some(local.storage_identity()?) {
            return Err("Paired local directory was replaced; re-enroll it explicitly".into());
        }
        self.storage
            .validate(&portable, &local, pair.ram.then_some(pair.id.as_str()))?;
        let cached = self.cached.as_ref().is_some_and(|(a, b)| {
            [(a, &portable), (b, &local)]
                .into_iter()
                .all(|(prior, current)| prior.validate().is_ok() && prior.same_root(current))
        });
        Ok(Plan {
            pair,
            portable,
            local,
            cached,
            observed: self.observed,
            storage: Some(self.storage.clone()),
        })
    }
    fn run(&self, inventory: &Inventory, cancel: &AtomicBool) -> Result<Prepared> {
        cancelled(cancel)?;
        let mounts = Snapshot::capture()?.signature.clone();
        let drive = discovery::select(inventory()?, &self.pair.drive)?;
        let plan = if let Some(enrollment) = &self.enrollment {
            enrollment.prepare(&self.pair, &self.storage, &drive, cancel)?
        } else {
            self.prepare_pair(&drive)?
        };
        cancelled(cancel)?;
        if discovery::select(inventory()?, &drive.id)? != drive {
            return Err(
                "Drive changed while pinning sync roots; retry with fresh inventory".into(),
            );
        }
        plan.validate_roots()?;
        if Snapshot::capture()?.signature != mounts {
            return Err("Mount inventory changed while preparing synchronization; retry".into());
        }
        Ok(Prepared { plan, mounts })
    }
}
struct Prepared {
    plan: Plan,
    mounts: String,
}
impl Prepared {
    fn accept(self, owner: &Owner, input: &Input, epoch: u64) -> Result<Option<Plan>> {
        let current = input.current(owner)?
            && epoch == owner.sync_epoch
            && Snapshot::capture()?.signature == self.mounts;
        if !current {
            return if input.enrollment.is_some() {
                Err("Enrollment inputs changed during preparation; no pair was registered. Created folders are retained; retry enrollment".into())
            } else {
                Ok(None)
            };
        }
        if input.enrollment.is_some() {
            owner.db.put("pair", &self.plan.pair.id, &self.plan.pair)?;
        }
        Ok(Some(self.plan))
    }
}
pub(in crate::service) struct Pending {
    pub drive: String,
    pub intent: Intent,
    pub claims: Vec<crate::root::Scope>,
    epoch: u64,
    input: Arc<Input>,
    task: Task<Result<Prepared>>,
}
impl Owner {
    pub(super) fn prepare_sync_roots(
        &mut self,
        intent: Intent,
        pair: SyncPair,
        enrollment: Option<enrollment::Input>,
    ) {
        let input = match Input::capture(self, pair, enrollment) {
            Ok(input) => Arc::new(input),
            Err(error) => {
                self.sync_result(&intent, Err(error), None);
                return;
            }
        };
        let worker = input.clone();
        let inventory = self.inventory.clone();
        let cancel = intent.cancel.clone();
        let task = write_task(&self.sync_pool, move || worker.run(&inventory, &cancel));
        self.sync_preparing.insert(
            intent.pair.clone(),
            Pending {
                drive: input.pair.drive.clone(),
                claims: input
                    .enrollment
                    .as_ref()
                    .map_or_else(Vec::new, |e| e.claims.clone()),
                intent,
                epoch: self.sync_epoch,
                input,
                task,
            },
        );
    }
    pub(super) fn poll_sync_preparation(&mut self) {
        let completed = self
            .sync_preparing
            .iter()
            .filter_map(|(id, job)| job.task.poll().map(|delivered| (id.clone(), delivered)))
            .collect::<Vec<_>>();
        for (id, delivered) in completed {
            let job = self
                .sync_preparing
                .remove(&id)
                .expect("delivered root preparation");
            let result = write_result(delivered);
            let result = cancelled(&job.intent.cancel).and(result);
            if self.closing.load(Ordering::Acquire) {
                self.sync_result(
                    &job.intent,
                    Err("Daemon stopping; prepared roots retired before reconciliation".into()),
                    None,
                );
                continue;
            }
            match result.and_then(|prepared| prepared.accept(self, &job.input, job.epoch)) {
                Err(error) => self.sync_result(&job.intent, Err(error), None),
                Ok(Some(plan)) => self.admit_sync_plan(job.intent, plan),
                Ok(None) => self.queue_sync(job.intent),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_worker_rechecks_a_managed_path_replaced_after_preparation() {
        let temp = tempfile::tempdir().unwrap();
        let state = Root::create(&temp.path().join("state")).unwrap();
        let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
        let portable = Arc::new(Root::create(&temp.path().join("portable")).unwrap());
        let local = Arc::new(Root::create(&temp.path().join("local")).unwrap());
        let managed = temp.path().join("backups");
        fs::create_dir(&managed).unwrap();
        fs::write(local.path.join("edit"), b"must stay local").unwrap();
        let storage = policy::Storage::capture(&db, &state.path, Some(&managed)).unwrap();
        storage.validate(&portable, &local, None).unwrap();
        let pair = SyncPair {
            id: "write-worker-guard".into(),
            drive: "owned".into(),
            evidence: "owned".into(),
            relative: PathBuf::new(),
            local: local.path.clone(),
            local_identity: Some(local.storage_identity().unwrap()),
            paused: false,
            deletions: false,
            ram: false,
            last_success: None,
            status: "prepared".into(),
        };
        let intent = Intent {
            queued: Instant::now(),
            pair: pair.id.clone(),
            operation: None,
            cancel: Arc::new(AtomicBool::new(false)),
            paths: None,
            initialize: false,
            save: None,
            writeback: None,
        };
        let plan = Plan {
            pair,
            portable,
            local,
            cached: false,
            observed: false,
            storage: Some(storage),
        };
        fs::remove_dir(&managed).unwrap();
        std::os::unix::fs::symlink(&plan.portable.path, &managed).unwrap();
        let error = plan
            .run_with_cancel(&db, &intent, &intent.cancel, &mut |_, _| {})
            .unwrap_err();
        assert!(error.to_string().contains("managed backups"), "{error}");
        assert!(!plan.portable.path.join("edit").exists());
        assert_eq!(
            fs::read(plan.local.path.join("edit")).unwrap(),
            b"must stay local"
        );
        assert_eq!(
            db.0.query_row("SELECT count(*) FROM entries", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
