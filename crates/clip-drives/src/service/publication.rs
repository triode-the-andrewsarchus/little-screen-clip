//! One retained policy observer; only the coordinator installs current results.
use super::*;
use little_clip_core::jobs::task::Task;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Input {
    pub paths: Vec<PathBuf>,
    pub drives: Vec<Drive>,
    pub rules: Vec<DriveRule>,
}
type Observer = Arc<dyn Fn(Input, Option<IndexPolicy>) -> Result<IndexPolicy> + Send + Sync>;

pub(super) struct Publication {
    jobs: little_clip_core::jobs::BackgroundJobs,
    observer: Observer,
    input: Option<Input>,
    previous: Option<IndexPolicy>,
    epoch: u64,
    dirty: bool,
    pending: Option<(u64, Task<Result<IndexPolicy>>)>,
}
impl Default for Publication {
    fn default() -> Self {
        Self::new(Arc::new(observe))
    }
}
impl Publication {
    pub(super) fn new(observer: Observer) -> Self {
        Self {
            jobs: little_clip_core::jobs::BackgroundJobs::new(1, 1),
            observer,
            input: None,
            previous: None,
            epoch: 0,
            dirty: false,
            pending: None,
        }
    }
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.dirty
    }
    fn blocked(&self, state: &Path, input: &Input) -> Result<()> {
        let mut excluded = input.paths.clone();
        excluded.push(PathBuf::from("/"));
        for rule in input.rules.iter().filter(|rule| !rule.index) {
            excluded.extend(
                input
                    .drives
                    .iter()
                    .filter(|drive| drive.id == rule.drive)
                    .filter_map(|drive| drive.mount.as_ref().map(|path| path.join(&rule.relative))),
            );
        }
        excluded.sort();
        excluded.dedup();
        install(
            state,
            &IndexPolicy {
                excluded,
                // Retain descriptor-free offline-owner observations across a
                // crash during refresh. The literal root denies every client.
                context: self
                    .previous
                    .as_ref()
                    .and_then(|policy| policy.context.clone()),
            },
        )
    }
    pub(super) fn request(&mut self, state: &Path, input: Input, fresh: bool) -> Result<u64> {
        let changed = self.input.as_ref() != Some(&input);
        if !changed && self.busy() && !fresh {
            return Ok(self.epoch);
        }
        if self.input.is_none() {
            self.previous = kernel_interface::read_content_bounded(
                &state.join("index-policy.json"),
                1024 * 1024,
            )
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        }
        let epoch = self
            .epoch
            .checked_add(1)
            .ok_or("Policy revision exhausted")?;
        if changed {
            self.blocked(state, &input)?;
        }
        self.input = Some(input);
        self.epoch = epoch;
        self.dirty = true;
        self.start();
        Ok(epoch)
    }
    fn start(&mut self) {
        if self.pending.is_some() || !self.dirty {
            return;
        }
        let input = self.input.clone().expect("queued policy input");
        let previous = self.previous.clone();
        let observer = self.observer.clone();
        self.pending = Some((
            self.epoch,
            write_task(&self.jobs, move || observer(input, previous)),
        ));
        self.dirty = false;
    }
    pub(super) fn poll(&mut self, state: &Path) -> Option<(u64, Result<()>)> {
        let delivered = self.pending.as_ref()?.1.poll()?;
        let (epoch, _) = self.pending.take().expect("delivered policy task");
        let observed = write_result(delivered);
        if epoch != self.epoch {
            // Old results cannot release indexing or complete a newer rule.
            // Their observations may still protect an owner whose origin has
            // disappeared while this worker was busy.
            if let Ok(policy) = observed {
                self.previous = Some(policy);
                if let Err(error) =
                    self.blocked(state, self.input.as_ref().expect("current policy input"))
                {
                    tracing::warn!(%error, "Could not persist retained policy observations");
                }
            }
            self.start();
            return None;
        }
        let result = match observed {
            Ok(policy) => install(state, &policy).map(|()| self.previous = Some(policy)),
            Err(error) => self.blocked(state, self.input.as_ref().expect("current policy input")).and(Err(format!(
                "Background indexing paused: could not establish drive-policy mount context: {error}"
            ).into())),
        };
        self.start();
        Some((epoch, result))
    }
}

pub(super) struct Waiter {
    pub epoch: u64,
    pub id: String,
    pub cancel: Arc<AtomicBool>,
    pub message: String,
}

impl Owner {
    pub(super) fn rule_operation(&mut self, envelope: Envelope) {
        if self.policy_waiting.len() >= 16 {
            self.defer_operation(envelope);
            return;
        }
        // A rule edits local state and must not pin or wait on its source.
        let result = self.execute_recorded(&envelope.id, envelope.request, &envelope.cancel);
        match result {
            Ok(message) => {
                if let Ok(Some(mut operation)) =
                    self.db.get::<DriveOperation>("operation", &envelope.id)
                {
                    operation.state = "publishing".into();
                    operation.message = "Updating indexing exclusions".into();
                    let _ = self.db.put("operation", &envelope.id, &operation);
                }
                self.policy_waiting.push_back(Waiter {
                    epoch: self.publication.epoch(),
                    id: envelope.id,
                    cancel: envelope.cancel,
                    message,
                });
            }
            result => self.finish_operation(&envelope.id, &envelope.cancel, result),
        }
    }
    pub(super) fn poll_policy(&mut self) {
        let Some((epoch, result)) = self.publication.poll(&self.state) else {
            return;
        };
        if let Err(error) = &result {
            tracing::warn!(%error, "Drive index policy remains excluded");
        }
        let mut waiting = std::mem::take(&mut self.policy_waiting);
        while let Some(waiter) = waiting.pop_front() {
            if waiter.epoch > epoch {
                self.policy_waiting.push_back(waiter);
                continue;
            }
            let outcome = match &result {
                Ok(()) => Ok(waiter.message),
                Err(error) => Err(error.to_string().into()),
            };
            self.finish_operation(&waiter.id, &waiter.cancel, outcome);
        }
    }
}

pub(super) fn observe(input: Input, previous: Option<IndexPolicy>) -> Result<IndexPolicy> {
    let routed = little_clip_core::mount_inventory::Snapshot::capture()?;
    let sources = rules::IndexSources::observed(&input.drives, &routed.mounts);
    let mut requests = input
        .rules
        .iter()
        .flat_map(|rule| sources.requests(rule))
        .collect::<Vec<_>>();
    requests.extend(input.paths.into_iter().map(|path| {
        (
            format!(
                "path:{}",
                little_clip_core::path_address::display_path(&path)
            ),
            Some(path),
        )
    }));
    let policy = IndexPolicy::observed_rules(requests, previous.as_ref())?;
    if !policy
        .context
        .as_ref()
        .is_some_and(|context| context.signature == routed.signature)
    {
        return Err("Mount routing changed during policy publication".into());
    }
    Ok(policy)
}

fn install(state: &Path, policy: &IndexPolicy) -> Result<()> {
    let json = serde_json::to_vec(policy)?;
    let root = kernel_interface::pin_content_directory(state)?;
    let path = state.join("index-policy.json");
    if !path.exists() {
        little_clip_core::file_source::create_file_in(&path, &json, &root)?;
        return Ok(());
    }
    let unchanged = kernel_interface::open_content(&path, false)
        .and_then(|mut file| {
            use std::io::Read;
            let mut old = Vec::new();
            file.by_ref()
                .take((json.len() as u64).saturating_add(1))
                .read_to_end(&mut old)?;
            Ok(old == json)
        })
        .unwrap_or(false);
    if !unchanged {
        little_clip_core::file_source::FileSource::capture(&path)?.write_in(&path, &json, &root)?;
    }
    Ok(())
}
