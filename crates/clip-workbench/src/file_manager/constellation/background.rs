//! Bounded background loading and pack operations. Accepted writes retain completion results.
use super::*;
use little_clip_core::jobs::{BackgroundJobs, JobError, JobTicket};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Task {
    Folder(PathBuf, u64),
    List(PathBuf, u64),
    Bank(PathBuf, bool, u64),
    Audio(PathBuf, u64),
    Save,
    Delete,
}
pub(super) enum Output {
    Samples(Vec<ConstellationSample>),
    Packs(Vec<sample_pack::SamplePackSummary>),
    Bank(SamplePackBank),
    Audio(sample_pack::SlotSample),
    Saved(PathBuf),
    Deleted,
}
type Completion = Result<Result<Output, String>, JobError>;
type Work = Arc<dyn Fn() -> Result<Output, String> + Send + Sync>;

/// Retains admission input until a worker accepts it. Completed operations are never retried.
pub(super) struct Pending {
    task: Task,
    work: Work,
    receiver: Option<mpsc::Receiver<Completion>>,
    ticket: Option<JobTicket>,
}

impl Task {
    fn current(&self, state: &ConstellationState) -> bool {
        match self {
            Self::Folder(path, generation) => {
                state.current_indexed_folder.as_ref() == Some(path)
                    && *generation == state.folder_generation
            }
            Self::List(path, generation) => {
                state.current_indexed_folder.as_ref() == Some(path)
                    && *generation == state.browser_generation
            }
            Self::Bank(path, false, generation) => {
                *generation == state.browser_generation
                    && state.browser_open
                    && state
                        .browser_selected_idx
                        .and_then(|index| state.browser_packs.get(index))
                        .is_some_and(|pack| pack.folder_path == *path)
            }
            Self::Bank(_, true, generation) | Self::Audio(_, generation) => {
                *generation == state.bank_generation
            }
            Self::Save | Self::Delete => true,
        }
    }
}

impl Pending {
    pub(super) fn cancel_read(&self) {
        if !matches!(self.task, Task::Save | Task::Delete) {
            if let Some(ticket) = &self.ticket {
                ticket.cancel();
            }
        }
    }

    fn poll(&mut self, jobs: &BackgroundJobs) -> Option<Completion> {
        let Some(receiver) = &self.receiver else {
            let work = self.work.clone();
            let (ticket, receiver) = jobs.submit(Duration::MAX, move || work());
            self.ticket = Some(ticket);
            self.receiver = Some(receiver);
            return None;
        };
        match receiver.try_recv() {
            Ok(Err(JobError::QueueFull)) => {
                // Rejected admission proves the closure did not run. Retain it for
                // a later tick; retrying a completed mutation could duplicate effects.
                self.receiver = None;
                self.ticket = None;
                None
            }
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Ok(Err("Constellation worker stopped".into())))
            }
        }
    }
}

impl ConstellationState {
    /// Reports work that needs regular UI ticks, including executor admission retries.
    pub fn busy(&self) -> bool {
        !self.background.is_empty()
            || self.active_job.is_some()
            || self.pending_hover_path.is_some()
            || !self.active_players.is_empty()
    }

    /// Reports accepted mutations whose completions must be observed before close.
    pub fn writing_pack(&self) -> bool {
        self.background
            .iter()
            .any(|job| matches!(job.task, Task::Save | Task::Delete))
    }

    pub(super) fn submit_background(
        &mut self,
        task: Task,
        work: impl Fn() -> Result<Output, String> + Send + Sync + 'static,
    ) {
        // Superseded reads cannot crowd a fresh folder or browser request out.
        self.background = std::mem::take(&mut self.background)
            .into_iter()
            .filter(|job| {
                if job.task.current(self) {
                    true
                } else {
                    job.cancel_read();
                    false
                }
            })
            .collect();
        if self.background.iter().any(|job| job.task == task) {
            return;
        }
        if self.background.len() >= 16 {
            self.browser_status = Some((
                "Constellation is busy; try the operation again".into(),
                Instant::now(),
            ));
            return;
        }
        self.background.push(Pending {
            task,
            work: Arc::new(work),
            receiver: None,
            ticket: None,
        });
    }

    /// Releases browser-only PCM and retires listing/preview requests on close.
    pub fn close_pack_browser(&mut self) {
        self.browser_open = false;
        self.browser_preview = None;
        self.browser_selected_idx = None;
        self.browser_generation = self.browser_generation.wrapping_add(1);
    }

    /// Replaces the editable bank and invalidates older asynchronous loads.
    pub fn clear_bank(&mut self) {
        self.bank_generation = self.bank_generation.wrapping_add(1);
        self.stop_playback();
        self.pack_bank = SamplePackBank::default();
        self.failed_audio.clear();
    }

    /// Requests metadata for a browser preview or an explicit bank replacement.
    pub fn load_browser_bank(&mut self, path: PathBuf, install: bool) {
        if install {
            self.bank_generation = self.bank_generation.wrapping_add(1);
        }
        self.submit_background(
            Task::Bank(
                path.clone(),
                install,
                if install {
                    self.bank_generation
                } else {
                    self.browser_generation
                },
            ),
            move || sample_pack::load_sample_pack(&path).map(Output::Bank),
        );
    }

    /// Submits an immutable bank snapshot; later edits do not alter the accepted save.
    pub fn save_bank(&mut self, root: PathBuf, bank: SamplePackBank) {
        self.submit_background(Task::Save, move || {
            sample_pack::save_sample_pack(&root, &bank).map(Output::Saved)
        });
    }

    /// Submits deletion while retaining its completion through navigation and close.
    pub fn delete_bank(&mut self, path: PathBuf) {
        self.submit_background(Task::Delete, move || {
            sample_pack::delete_sample_pack(&path).map(|()| Output::Deleted)
        });
    }

    pub(super) fn poll_background(&mut self, jobs: &BackgroundJobs) {
        let mut pending = Vec::new();
        for mut job in std::mem::take(&mut self.background) {
            if !job.task.current(self) {
                job.cancel_read();
                continue;
            }
            let Some(result) = job.poll(jobs) else {
                pending.push(job);
                continue;
            };
            match result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                Ok(output) => self.accept_background(job.task, output),
                Err(error) => {
                    if let Task::Audio(path, _) = job.task {
                        self.failed_audio.insert(path);
                    }
                    self.browser_status = Some((format!("Error: {error}"), Instant::now()));
                }
            }
        }
        pending.append(&mut self.background);
        self.background = pending;
    }

    fn accept_background(&mut self, task: Task, output: Output) {
        if !task.current(self) {
            return;
        }
        match (task, output) {
            (Task::Folder(..), Output::Samples(samples)) => {
                self.samples = samples;
            }
            (Task::List(..), Output::Packs(packs)) => {
                self.browser_packs = packs;
            }
            (Task::Bank(path, install, _), Output::Bank(bank)) => {
                if install {
                    self.stop_playback();
                    self.pack_bank = bank;
                    self.failed_audio.clear();
                    self.close_pack_browser();
                    self.browser_status = Some(("Loaded sample pack".into(), Instant::now()));
                } else {
                    self.browser_preview = Some((path, bank));
                }
            }
            (Task::Audio(path, _), Output::Audio(decoded)) => {
                for bank in std::iter::once(&mut self.pack_bank)
                    .chain(self.browser_preview.iter_mut().map(|(_, bank)| bank))
                {
                    for sample in bank
                        .slots
                        .iter_mut()
                        .flat_map(|slot| &mut slot.samples)
                        .filter(|sample| sample.file_path == path)
                    {
                        if let Some(audio) = &decoded.cached_audio {
                            sample.set_cached_audio(audio.clone());
                        }
                    }
                }
            }
            (Task::Save, Output::Saved(path)) => {
                self.browser_status = Some((format!("Saved {}", path.display()), Instant::now()));
                if let Some(root) = self.current_indexed_folder.clone() {
                    self.refresh_browser_packs(&root);
                }
            }
            (Task::Delete, Output::Deleted) => {
                self.browser_status = Some(("Deleted sample pack".into(), Instant::now()));
                if let Some(root) = self.current_indexed_folder.clone() {
                    self.refresh_browser_packs(&root);
                }
            }
            _ => {}
        }
    }

    pub(super) fn queue_audio_loads(&mut self) {
        // One submitted decode at a time bounds allocation and keeps mutations responsive.
        if self
            .background
            .iter()
            .any(|job| matches!(job.task, Task::Audio(..)))
        {
            return;
        }
        let next = std::iter::once(&self.pack_bank)
            .chain(self.browser_preview.iter().map(|(_, bank)| bank))
            .flat_map(|bank| &bank.slots)
            .flat_map(|slot| &slot.samples)
            .find(|sample| {
                sample.cached_audio.is_none() && !self.failed_audio.contains(&sample.file_path)
            })
            .cloned();
        if let Some(sample) = next {
            let mut seen = std::collections::HashSet::new();
            let used: usize = std::iter::once(&self.pack_bank)
                .chain(self.browser_preview.iter().map(|(_, bank)| bank))
                .flat_map(|bank| &bank.slots)
                .flat_map(|slot| &slot.samples)
                .filter_map(|sample| sample.cached_audio.as_ref())
                .filter(|audio| seen.insert(Arc::as_ptr(audio)))
                .map(|audio| {
                    (audio.samples.len() + audio.mono_samples.len()) * std::mem::size_of::<f32>()
                })
                .sum();
            if used > 112 * 1024 * 1024 {
                self.browser_status = Some((
                    "PCM memory limit reached; remove layers to load more samples".into(),
                    Instant::now(),
                ));
                return;
            }
            let path = sample.file_path.clone();
            self.submit_background(Task::Audio(path, self.bank_generation), move || {
                let mut sample = sample.clone();
                sample.ensure_audio_loaded();
                if sample.cached_audio.is_none() {
                    return Err(format!("Cannot decode {}", sample.filename));
                }
                Ok(Output::Audio(sample))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturated_executor_retries_admission_without_repeating_an_accepted_save() {
        use std::sync::atomic::AtomicUsize;
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_, running) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (_, queued) = jobs.submit(Duration::MAX, || {});
        let effects = Arc::new(AtomicUsize::new(0));
        let worker_effects = effects.clone();
        let mut state = ConstellationState::new();
        state.submit_background(Task::Save, move || {
            worker_effects.fetch_add(1, Ordering::SeqCst);
            Ok(Output::Saved(PathBuf::from("/owned/Kit")))
        });
        state.poll_background(&jobs);
        state.poll_background(&jobs);
        assert!(state.writing_pack());
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert!(state.browser_status.is_none());
        release_tx.send(()).unwrap();
        running
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        queued
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.writing_pack() {
            assert!(Instant::now() < deadline, "save admission did not recover");
            state.poll_background(&jobs);
            std::thread::sleep(Duration::from_millis(1));
        }
        state.poll_background(&jobs);
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert!(
            state
                .browser_status
                .as_ref()
                .unwrap()
                .0
                .contains("Saved /owned/Kit")
        );
    }

    #[test]
    fn superseded_folder_reads_do_not_exhaust_local_admission() {
        let mut state = ConstellationState::new();
        for generation in 0..32 {
            state.current_indexed_folder = Some(PathBuf::from("/samples"));
            state.folder_generation = generation;
            state.submit_background(Task::Folder(PathBuf::from("/samples"), generation), || {
                Ok(Output::Samples(vec![]))
            });
        }
        assert_eq!(state.background.len(), 1);
        assert_eq!(
            state.background[0].task,
            Task::Folder(PathBuf::from("/samples"), 31)
        );
        assert!(state.browser_status.is_none());
    }

    #[test]
    fn closing_browser_retires_metadata_and_releases_preview_pcm() {
        let mut state = ConstellationState::new();
        state.browser_open = true;
        state.browser_generation = 5;
        state.browser_preview = Some((PathBuf::from("/pack"), SamplePackBank::default()));
        state.close_pack_browser();
        assert!(state.browser_preview.is_none());
        assert!(!Task::Bank(PathBuf::from("/pack"), false, 5).current(&state));
    }

    #[test]
    fn cleared_bank_rejects_older_load_completion() {
        let mut state = ConstellationState::new();
        let old_generation = state.bank_generation;
        state.clear_bank();
        state.accept_background(
            Task::Bank(PathBuf::from("/old"), true, old_generation),
            Output::Bank(SamplePackBank::new("Stale bank")),
        );
        assert_eq!(state.pack_bank.name, "Untitled Pack");
    }

    #[test]
    fn refreshed_browser_rejects_older_listing() {
        let mut state = ConstellationState::new();
        state.current_indexed_folder = Some(PathBuf::from("/samples"));
        state.browser_generation = 2;
        state.accept_background(
            Task::List(PathBuf::from("/samples"), 1),
            Output::Packs(vec![sample_pack::SamplePackSummary {
                name: "Old pack".into(),
                folder_path: PathBuf::from("/samples/old"),
                slot_count: 0,
                sample_count: 0,
                modified: std::time::UNIX_EPOCH,
            }]),
        );
        assert!(state.browser_packs.is_empty());
    }

    #[test]
    fn accepted_save_completion_survives_playback_stop_and_bank_clear() {
        let mut state = ConstellationState::new();
        let (sender, receiver) = mpsc::channel();
        state.background.push(Pending {
            task: Task::Save,
            receiver: Some(receiver),
            ticket: None,
            work: Arc::new(|| unreachable!("accepted completion must not be retried")),
        });
        state.stop_playback();
        state.clear_bank();
        assert!(state.writing_pack());
        sender
            .send(Ok(Ok(Output::Saved(PathBuf::from("/saved/Kit")))))
            .unwrap();
        state.poll_background(BackgroundJobs::global());
        assert!(!state.writing_pack());
        assert!(
            state
                .browser_status
                .as_ref()
                .unwrap()
                .0
                .contains("/saved/Kit")
        );
    }
}
