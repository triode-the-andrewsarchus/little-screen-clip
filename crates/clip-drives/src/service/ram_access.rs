//! Recovery and export preparation retain the RAM worker through actual delivery.
use super::*;
use little_clip_core::{jobs::task::Task, mount_inventory::Snapshot};

pub(super) struct Pending {
    pub drive: String,
    pub claims: Vec<crate::root::Scope>,
    envelope: Envelope,
    storage: policy::Storage,
    task: Task<Result<Outcome>>,
}
struct Outcome {
    // A recovered server must be adopted even if later preparation fails/cancels.
    helper: Option<crate::ram::Mounted>,
    result: Result<Prepared>,
}
struct Prepared {
    session: RamSession,
    pair: SyncPair,
    mounts: String,
    save: Option<Arc<ram_save::Save>>,
}

struct Input {
    session: RamSession,
    pair: SyncPair,
    live: bool,
    destination: Option<(PathBuf, crate::root::Scope)>,
}
impl Input {
    fn run(self, state: &Path, cancel: &AtomicBool) -> Outcome {
        let mut helper = None;
        let result = (|| {
            cancelled(cancel)?;
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let (session, pair) = self.recover(&db, &mut helper)?;
            self.prepare(&db, session, pair, self.live || helper.is_some(), cancel)
        })();
        Outcome { helper, result }
    }
    fn recover(
        &self,
        db: &Store,
        helper: &mut Option<crate::ram::Mounted>,
    ) -> Result<(RamSession, SyncPair)> {
        let (session, pair) =
            ram_sessions::recover_view(db, &self.session, Some(&self.pair), self.live, helper)?;
        Ok((
            session,
            pair.ok_or("RAM enrollment disappeared during recovery")?,
        ))
    }

    fn prepare(
        &self,
        db: &Store,
        session: RamSession,
        pair: SyncPair,
        live: bool,
        cancel: &AtomicBool,
    ) -> Result<Prepared> {
        let mounts = Snapshot::capture()?.signature.clone();
        cancelled(cancel)?;
        let save = self
            .destination
            .as_ref()
            .map(|(destination, claim)| {
                ram_save::Save::prepare(db, session.clone(), destination, claim, cancel)
            })
            .transpose()?;
        ram_sessions::available(&session, live)?;
        if Snapshot::capture()?.signature != mounts {
            return Err("Mount inventory changed during RAM preparation; retry".into());
        }
        if db.get::<SyncPair>("pair", &session.id)?.as_ref() != Some(&pair) {
            return Err("RAM enrollment changed during preparation; retry".into());
        }
        Ok(Prepared {
            session,
            pair,
            mounts,
            save,
        })
    }
}

impl Owner {
    pub(super) fn drain_ram_preparation(&mut self) {
        while self.ram_setup.is_some() || self.ram_access.is_some() {
            if let Some(job) = &self.ram_access {
                job.envelope.cancel.store(true, Ordering::Release);
            }
            self.poll_ram_setup();
            self.poll_ram_access();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub(super) fn start_ram_access(
        &mut self,
        envelope: Envelope,
        session: RamSession,
        claims: Vec<crate::root::Scope>,
    ) {
        if self.ram_setup.is_some() || self.ram_access.is_some() || self.ram_recovery.busy() {
            self.defer_operation(envelope);
            return;
        }
        let input = (|| {
            let pair: SyncPair = self
                .db
                .get("pair", &session.id)?
                .ok_or("RAM enrollment is missing")?;
            if !pair.ram || pair.drive != session.drive || pair.local != session.path {
                return Err("RAM enrollment differs from retained session; refusing access".into());
            }
            let destination = if let DriveRequest::SaveRam { destination, .. } = &envelope.request {
                Some((
                    destination.clone(),
                    claims
                        .last()
                        .ok_or("Missing RAM save destination claim")?
                        .clone(),
                ))
            } else {
                None
            };
            let storage = policy::Storage::capture(
                &self.db,
                &self.state,
                self.config.backup_directory.as_deref(),
            )?;
            Ok((
                Input {
                    live: self.ram_children.contains_key(&session.id),
                    session,
                    pair,
                    destination,
                },
                storage,
            ))
        })();
        let (input, storage) = match input {
            Ok(value) => value,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        let drive = input.pair.drive.clone();
        let state = self.state.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.ram_setup_jobs, move || Ok(input.run(&state, &cancel)));
        self.ram_access = Some(Pending {
            drive,
            claims,
            envelope,
            storage,
            task,
        });
    }

    pub(super) fn poll_ram_access(&mut self) {
        let Some(delivered) = self.ram_access.as_ref().and_then(|job| job.task.poll()) else {
            return;
        };
        let job = self
            .ram_access
            .take()
            .expect("delivered RAM access preparation");
        let result = write_result(delivered).and_then(|outcome| {
            if let Some(helper) = outcome.helper {
                let id = match &job.envelope.request {
                    DriveRequest::Sync { pair } => pair,
                    DriveRequest::SaveRam { session, .. } | DriveRequest::FlushRam { session } => {
                        session
                    }
                    _ => unreachable!("RAM access request"),
                };
                self.ram_children.insert(id.clone(), helper);
                self.sync_roots.remove(id);
            }
            outcome.result
        });
        let result = cancelled(&job.envelope.cancel)
            .and(result)
            .and_then(|prepared| {
                if self.closing.load(Ordering::Acquire) {
                    return Err("Daemon stopping; RAM view retained before reconciliation".into());
                }
                let session: RamSession = self
                    .db
                    .get("ram", &prepared.session.id)?
                    .ok_or("RAM session disappeared during preparation")?;
                if !ram_sessions::same_source(&session, &prepared.session)
                    || self.db.get::<SyncPair>("pair", &session.id)?.as_ref()
                        != Some(&prepared.pair)
                    || policy::Storage::capture(
                        &self.db,
                        &self.state,
                        self.config.backup_directory.as_deref(),
                    )? != job.storage
                    || Snapshot::capture()?.signature != prepared.mounts
                {
                    return Err(
                        "RAM preparation inputs changed; retained contents are safe, retry".into(),
                    );
                }
                Ok(prepared)
            });
        match result {
            Ok(prepared) => self.queue_sync(reconciliation::Intent {
                queued: Instant::now(),
                pair: prepared.pair.id,
                operation: Some(job.envelope.id),
                cancel: job.envelope.cancel,
                paths: None,
                initialize: !prepared.session.ready && prepared.save.is_none(),
                save: prepared.save,
                writeback: None,
            }),
            Err(error) => self.finish_operation(&job.envelope.id, &job.envelope.cancel, Err(error)),
        }
    }
}
