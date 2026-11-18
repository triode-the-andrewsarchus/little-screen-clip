//! Read-only scheduling probes never occupy the coordinator or authorize writes.
use super::*;
use crate::root::Scope;
use little_clip_core::{jobs::task::Task, mount_inventory::Snapshot};

const WORKERS: usize = 4;

pub(super) struct Probes {
    jobs: little_clip_core::jobs::BackgroundJobs,
    pub pending: HashMap<String, Pending>,
}
impl Default for Probes {
    fn default() -> Self {
        Self {
            jobs: little_clip_core::jobs::BackgroundJobs::new(WORKERS, WORKERS),
            pending: HashMap::new(),
        }
    }
}
pub(super) struct Pending {
    pub envelope: Envelope,
    epoch: u64,
    task: Task<Result<Observation>>,
}
struct Observation {
    drive: Option<String>,
    claims: Vec<Scope>,
    mounts: String,
}
struct Input {
    selector: Option<String>,
    drives: Vec<Drive>,
    save: Option<(PathBuf, Option<PathBuf>)>,
}
impl Input {
    fn observe(mut self, inventory: &Inventory, cancel: &AtomicBool) -> Result<Observation> {
        cancelled(cancel)?;
        let mounts = Snapshot::capture()?.signature.clone();
        if let Some(selector) = &self.selector
            && selector.starts_with("/dev/")
        {
            self.drives = inventory()?;
            self.selector = Some(discovery::select(self.drives.clone(), selector)?.id);
        }
        let claims = self.claims()?;
        cancelled(cancel)?;
        if Snapshot::capture()?.signature != mounts {
            return Err(
                "Mount inventory changed during storage scheduling admission; retry".into(),
            );
        }
        Ok(Observation {
            drive: self.selector,
            claims,
            mounts,
        })
    }
    fn claims(&self) -> Result<Vec<Scope>> {
        let mut claims = Vec::new();
        if let Some(path) = self
            .drives
            .iter()
            .find(|drive| Some(&drive.id) == self.selector.as_ref())
            .and_then(|drive| drive.mount.as_ref())
            && let Ok(root) = Root::open(path)
        {
            claims.push(root.scope());
        }
        if let Some((destination, source)) = &self.save {
            claims.push(Root::planned_scope(destination)?);
            if let Some(source) = source
                && let Ok(root) = Root::open(source)
            {
                claims.push(root.scope());
            }
        }
        Ok(claims)
    }
}

impl Owner {
    pub(super) fn drain_scopes(&mut self) {
        while !self.scheduling.pending.is_empty() {
            for job in self.scheduling.pending.values() {
                job.envelope.cancel.store(true, Ordering::Release);
            }
            self.poll_scopes();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub(super) fn start_scopes(&mut self, envelope: Envelope) {
        let input = (|| -> Result<Input> {
            let save = if let DriveRequest::SaveRam {
                destination,
                session,
            } = &envelope.request
            {
                Some((
                    destination.clone(),
                    self.db.get::<RamSession>("ram", session)?.map(|s| s.path),
                ))
            } else {
                None
            };
            Ok(Input {
                selector: self.request_selector(&envelope.request)?,
                drives: self
                    .shared
                    .lock()
                    .map_err(|_| "Drive status unavailable")?
                    .drives
                    .clone(),
                save,
            })
        })();
        let input = match input {
            Ok(input) => input,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        if input.selector.is_none() && input.save.is_none() {
            self.dispatch_scoped(envelope, None, Vec::new());
            return;
        }
        if self.scheduling.pending.len() >= WORKERS {
            self.defer_operation(envelope);
            return;
        }
        let inventory = self.inventory.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.scheduling.jobs, move || {
            input.observe(&inventory, &cancel)
        });
        self.scheduling.pending.insert(
            envelope.id.clone(),
            Pending {
                envelope,
                epoch: self.sync_epoch,
                task,
            },
        );
    }
    pub(super) fn poll_scopes(&mut self) {
        let finished = self
            .scheduling
            .pending
            .iter()
            .filter_map(|(id, job)| {
                job.task
                    .poll()
                    .map(|result| (id.clone(), write_result(result)))
            })
            .collect::<Vec<_>>();
        for (id, result) in finished {
            let job = self
                .scheduling
                .pending
                .remove(&id)
                .expect("delivered scheduling probe");
            let envelope = job.envelope;
            let result = cancelled(&envelope.cancel).and(result);
            if self.closing.load(Ordering::Acquire) {
                self.finish_operation(
                    &id,
                    &envelope.cancel,
                    Err("Daemon stopping; scheduling probe retired before execution".into()),
                );
                continue;
            }
            match result {
                Ok(observation) => {
                    if job.epoch != self.sync_epoch
                        || !Snapshot::capture().is_ok_and(|now| now.signature == observation.mounts)
                    {
                        self.defer_operation(envelope);
                    } else {
                        self.dispatch_scoped(envelope, observation.drive, observation.claims);
                    }
                }
                Err(error) => self.finish_operation(&id, &envelope.cancel, Err(error)),
            }
        }
    }
}
