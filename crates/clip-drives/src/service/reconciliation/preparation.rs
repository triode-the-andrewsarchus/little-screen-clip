//! Fresh root preparation shares reconciliation's retained four-worker budget.
use super::*;
use little_clip_core::mount_inventory::Snapshot;

pub(super) struct Input {
    pub pair: SyncPair,
    storage: policy::Storage,
    ram: ram_sessions::Reconciliation,
    cached: Option<(Arc<Root>, Arc<Root>)>,
    observed: bool,
}
impl Input {
    pub fn capture(owner: &Owner, pair: SyncPair) -> Result<Self> {
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
        })
    }
    fn current(&self, owner: &Owner) -> Result<bool> {
        Ok(
            owner.db.get::<SyncPair>("pair", &self.pair.id)?.as_ref() == Some(&self.pair)
                && policy::Storage::capture(
                    &owner.db,
                    &owner.state,
                    owner.config.backup_directory.as_deref(),
                )? == self.storage
                && self.ram.current(owner, &self.pair.id)?,
        )
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
        let selected = partition.directory(&pair.relative, false)?;
        let portable = Arc::new(Root::open(&crate::root::anchor(&selected))?);
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
        let plan = self.prepare_pair(&drive)?;
        cancelled(cancel)?;
        if discovery::select(inventory()?, &drive.id)? != drive {
            return Err(
                "Drive changed while pinning sync roots; retry with fresh inventory".into(),
            );
        }
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
pub(in crate::service) struct Pending {
    pub drive: String,
    pub intent: Intent,
    epoch: u64,
    input: Arc<Input>,
    task: Task<Result<Prepared>>,
}
impl Owner {
    pub(super) fn prepare_sync_roots(&mut self, intent: Intent, pair: SyncPair) {
        let input = match Input::capture(self, pair) {
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
                intent,
                epoch: self.sync_epoch,
                input,
                task,
            },
        );
    }
    pub(super) fn poll_sync_preparation(&mut self) {
        for (id, job) in std::mem::take(&mut self.sync_preparing) {
            let Some(delivered) = job.task.poll() else {
                self.sync_preparing.insert(id, job);
                continue;
            };
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
            match result {
                Err(error) => self.sync_result(&job.intent, Err(error), None),
                Ok(prepared) => {
                    let current = job.input.current(self).and_then(|current| {
                        Ok(current
                            && job.epoch == self.sync_epoch
                            && Snapshot::capture()?.signature == prepared.mounts)
                    });
                    match current {
                        Ok(true) => self.admit_sync_plan(job.intent, prepared.plan),
                        Ok(false) => self.queue_sync(job.intent),
                        Err(error) => self.sync_result(&job.intent, Err(error), None),
                    }
                }
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
