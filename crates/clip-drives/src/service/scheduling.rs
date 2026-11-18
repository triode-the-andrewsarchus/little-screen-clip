//! Retained scheduling and bulk preparation never occupy the coordinator.
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
            jobs: little_clip_core::jobs::BackgroundJobs::new_named(
                WORKERS,
                WORKERS,
                "drive-scope",
            ),
            pending: HashMap::new(),
        }
    }
}
pub(super) struct Pending {
    pub envelope: Envelope,
    epoch: u64,
    inputs: Option<configuration::Inputs>,
    task: Task<Result<Observation>>,
}
struct Observation {
    drive: Option<String>,
    claims: Vec<Scope>,
    mounts: String,
    source: Option<Source>,
}
/// Retain admitted source and optional bulk destination behind scheduling claims.
pub(super) struct Source {
    pub drive: Drive,
    pub root: Arc<Root>,
    pub destination: Option<Arc<Root>>,
}
struct Input {
    selector: Option<String>,
    drives: Vec<Drive>,
    save: Option<(PathBuf, Option<PathBuf>)>,
    retain_source: bool,
    bulk: Option<background::Input>,
}
impl Input {
    fn observe(mut self, inventory: &Inventory, cancel: &AtomicBool) -> Result<Observation> {
        cancelled(cancel)?;
        let mounts = Snapshot::capture()?.signature.clone();
        let source = self.source(inventory);
        let source = if self.retain_source {
            source?
        } else {
            source.ok().flatten()
        };
        let source = match self.bulk.take() {
            Some(bulk) => Some(bulk.prepare(source, inventory, cancel)?),
            None => source,
        };
        let claims = self.claims(source.as_ref())?;
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
            source: source.filter(|_| self.retain_source),
        })
    }
    fn source(&mut self, inventory: &Inventory) -> Result<Option<Source>> {
        if let Some(selector) = &self.selector
            && (selector.starts_with("/dev/") || self.retain_source)
        {
            self.drives = inventory()?;
            self.selector = Some(discovery::select(self.drives.clone(), selector)?.id);
        }
        let Some(drive) = self
            .drives
            .iter()
            .find(|drive| Some(&drive.id) == self.selector.as_ref())
        else {
            return Ok(None);
        };
        drive
            .mount
            .as_ref()
            .map(|path| {
                Ok(Source {
                    drive: drive.clone(),
                    root: Arc::new(Root::open(path)?),
                    destination: None,
                })
            })
            .transpose()
    }
    fn claims(&self, source: Option<&Source>) -> Result<Vec<Scope>> {
        let mut claims = source
            .into_iter()
            .map(|source| source.root.scope())
            .collect::<Vec<_>>();
        claims.extend(
            source
                .and_then(|source| source.destination.as_ref())
                .map(|root| root.scope()),
        );
        if let Some((destination, source)) = &self.save {
            if let Some(source) = source
                && let Ok(root) = Root::open(source)
            {
                claims.push(root.scope());
            }
            // Enrollment and RAM export retain the destination as the last claim.
            claims.push(Root::planned_scope(destination)?);
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
            let save = match &envelope.request {
                DriveRequest::SaveRam {
                    destination,
                    session,
                } => Some((
                    destination.clone(),
                    self.db.get::<RamSession>("ram", session)?.map(|s| s.path),
                )),
                DriveRequest::Pair { local, .. } => Some((local.clone(), None)),
                DriveRequest::BackupDirectory { path } => Some((path.clone(), None)),
                _ => None,
            };
            let bulk = background::Input::capture(self, &envelope);
            let retain_source = bulk.is_some()
                || matches!(
                    envelope.request,
                    DriveRequest::CopyToRam { .. } | DriveRequest::Pair { .. }
                );
            Ok(Input {
                selector: self.request_selector(&envelope.request)?,
                drives: self
                    .shared
                    .lock()
                    .map_err(|_| "Drive status unavailable")?
                    .drives
                    .clone(),
                save,
                retain_source,
                bulk,
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
            self.dispatch_scoped(envelope, None, Vec::new(), None);
            return;
        }
        if self.scheduling.pending.len() >= WORKERS {
            self.defer_operation(envelope);
            return;
        }
        let inputs = if input.bulk.is_some() {
            match configuration::Inputs::capture(self, true) {
                Ok(inputs) => Some(inputs),
                Err(error) => {
                    self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                    return;
                }
            }
        } else {
            None
        };
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
                inputs,
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
            let result = cancelled(&envelope.cancel).and(result).and_then(|observation| {
                if let Some(inputs) = &job.inputs
                    && !inputs.current(self, Some(&observation.mounts))? {
                    return Err("Bulk preparation inputs changed; created directories are retained, retry".into());
                }
                Ok(observation)
            });
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
                        self.dispatch_scoped(
                            envelope,
                            observation.drive,
                            observation.claims,
                            observation.source,
                        );
                    }
                }
                Err(error) => self.finish_operation(&id, &envelope.cancel, Err(error)),
            }
        }
    }
}
