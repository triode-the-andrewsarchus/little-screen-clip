//! Startup visits bounded key pages and retains the shared RAM worker until exit.
use super::*;
use little_clip_core::{jobs::task::Task, mount_inventory::Snapshot};

#[derive(Default)]
enum Phase {
    #[default]
    Allocations,
    Views,
    Done,
}
#[derive(Default)]
pub(in crate::service) struct Recovery {
    phase: Phase,
    after: String,
    pending: Option<Pending>,
}
struct Pending {
    id: String,
    drive: Option<String>,
    task: Task<Result<Outcome>>,
}
struct Outcome {
    helper: Option<crate::ram::Mounted>,
    result: Result<Option<Recovered>>,
}
struct Recovered {
    session: RamSession,
    pair: Option<SyncPair>,
    view: Arc<Root>,
    mounts: String,
}
enum Work {
    Allocation(String),
    View {
        session: Box<RamSession>,
        pair: Option<Box<SyncPair>>,
    },
}
impl Recovery {
    pub(in crate::service) fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Done) || self.pending.is_some()
    }
    pub(in crate::service) fn drive(&self) -> Option<&str> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.drive.as_deref())
    }
    fn advance(&mut self) {
        self.phase = match self.phase {
            Phase::Allocations => Phase::Views,
            _ => Phase::Done,
        };
        self.after.clear();
    }
    fn next(&mut self, db: &Store) -> Result<Option<(&'static str, String)>> {
        loop {
            let kind = match self.phase {
                Phase::Allocations => "ram-allocation",
                Phase::Views => "ram",
                Phase::Done => return Ok(None),
            };
            let ids = match db.ids(kind, &self.after, 1) {
                Ok(ids) => ids,
                Err(error) => {
                    self.advance();
                    return Err(error);
                }
            };
            if let Some(id) = ids.into_iter().next() {
                self.after = id.clone();
                return Ok(Some((kind, id)));
            }
            self.advance();
        }
    }
}
impl Work {
    fn run(self, state: &Path, cancel: &AtomicBool) -> Outcome {
        let mut helper = None;
        let result = (|| {
            cancelled(cancel)?;
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            match self {
                Self::Allocation(id) => ram_setup::recover_allocation(&db, &id).map(|()| None),
                Self::View { session, pair } => {
                    let (session, pair) =
                        recover_view(&db, &session, pair.as_deref(), false, &mut helper)?;
                    cancelled(cancel)?;
                    let view = Arc::new(Root::open(&session.path)?);
                    available(&session, helper.is_some())?;
                    view.validate()?;
                    Ok(Some(Recovered {
                        session,
                        pair,
                        view,
                        mounts: Snapshot::capture()?.signature.clone(),
                    }))
                }
            }
        })();
        Outcome { helper, result }
    }
}
impl Recovered {
    fn validate(&self, owner: &Owner) -> Result<()> {
        let session = owner
            .db
            .get::<RamSession>("ram", &self.session.id)?
            .ok_or("RAM session disappeared during startup recovery")?;
        if !same_source(&session, &self.session)
            || owner.db.get::<SyncPair>("pair", &session.id)? != self.pair
            || Snapshot::capture()?.signature != self.mounts
        {
            return Err("Startup RAM recovery inputs changed; retained contents are safe, retry synchronization".into());
        }
        Ok(())
    }
}
impl Owner {
    pub(in crate::service) fn poll_ram_recovery(&mut self) {
        self.finish_ram_recovery();
        if self.closing.load(Ordering::Acquire)
            || self.ram_recovery.pending.is_some()
            || self.ram_setup.is_some()
            || self.ram_access.is_some()
        {
            return;
        }
        // Legacy non-image records can be numerous. Bound each coordinator pass
        // without retaining a second inventory of all sessions.
        for _ in 0..32 {
            match self.start_ram_recovery() {
                Ok(true) => continue,
                Ok(false) => return,
                Err(error) => {
                    tracing::warn!(%error, "Startup RAM recovery remains incomplete");
                    return;
                }
            }
        }
    }
    fn start_ram_recovery(&mut self) -> Result<bool> {
        let Some((kind, id)) = self.ram_recovery.next(&self.db)? else {
            return Ok(false);
        };
        let (work, drive) = if kind == "ram-allocation" {
            (Work::Allocation(id.clone()), None)
        } else {
            let Some(session) = self.db.get::<RamSession>("ram", &id)? else {
                return Ok(true);
            };
            if session.id != id {
                return Err(
                    "RAM session address differs from its registry key; retaining data".into(),
                );
            }
            if session.backing.is_none() || self.ram_children.contains_key(&id) {
                return Ok(true);
            }
            let pair = self.db.get::<SyncPair>("pair", &id)?;
            let drive = Some(session.drive.clone());
            (
                Work::View {
                    session: Box::new(session),
                    pair: pair.map(Box::new),
                },
                drive,
            )
        };
        let state = self.state.clone();
        let cancel = self.closing.clone();
        let task = write_task(&self.ram_setup_jobs, move || Ok(work.run(&state, &cancel)));
        self.ram_recovery.pending = Some(Pending { id, drive, task });
        Ok(false)
    }
    fn finish_ram_recovery(&mut self) {
        let Some(delivered) = self
            .ram_recovery
            .pending
            .as_ref()
            .and_then(|job| job.task.poll())
        else {
            return;
        };
        let job = self
            .ram_recovery
            .pending
            .take()
            .expect("delivered startup RAM recovery");
        let result = write_result(delivered).and_then(|outcome| {
            if let Some(helper) = outcome.helper {
                self.ram_children.insert(job.id.clone(), helper);
                self.sync_roots.remove(&job.id);
            }
            outcome.result
        });
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let result = result.and_then(|recovered| {
            if let Some(recovered) = recovered {
                recovered.validate(self)?;
                if let Some(watch) = &mut self.watch {
                    watch.add(recovered.view);
                }
                self.automatic();
            }
            Ok(())
        });
        if let Err(error) = result {
            if job.drive.is_some()
                && let Ok(Some(mut session)) = self.db.get::<RamSession>("ram", &job.id)
            {
                session.status = if self.ram_children.contains_key(&job.id) {
                    format!("RAM view retained; reconciliation pending: {error}")
                } else {
                    format!("RAM image retained; recovery unavailable: {error}")
                };
                let _ = self.db.put("ram", &session.id, &session);
            }
            tracing::warn!(session = job.id, %error, "Startup RAM recovery remains incomplete");
        }
    }
    pub(in crate::service) fn drain_ram_recovery(&mut self) {
        while self.ram_recovery.pending.is_some() {
            self.finish_ram_recovery();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
