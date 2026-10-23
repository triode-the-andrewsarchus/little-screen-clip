//! Bounded background loading and pack operations. Accepted writes retain completion results.
use super::*;
use little_clip_core::jobs::{BackgroundJobs, JobError};
use std::time::Duration;

#[derive(Clone, PartialEq, Eq)]
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
pub(super) struct Pending {
    task: Task,
    receiver: mpsc::Receiver<Result<Result<Output, String>, JobError>>,
}

impl ConstellationState {
    /// Reports accepted mutations whose completions must be observed before close.
    pub fn writing_pack(&self) -> bool {
        self.background
            .iter()
            .any(|job| matches!(job.task, Task::Save | Task::Delete))
    }

    pub(super) fn submit_background(
        &mut self,
        task: Task,
        work: impl FnOnce() -> Result<Output, String> + Send + 'static,
    ) {
        if self.background.len() >= 16 || self.background.iter().any(|job| job.task == task) {
            return;
        }
        let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, work);
        self.background.push(Pending { task, receiver });
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
            Task::Bank(path.clone(), install, self.bank_generation),
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

    pub(super) fn poll_background(&mut self) {
        let mut pending = Vec::new();
        let jobs = std::mem::take(&mut self.background);
        for job in jobs {
            let result = match job.receiver.try_recv() {
                Ok(result) => result.map_err(|e| e.to_string()).and_then(|result| result),
                Err(mpsc::TryRecvError::Empty) => {
                    pending.push(job);
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => Err("Constellation worker stopped".into()),
            };
            match result {
                Ok(output) => self.accept_background(job.task, output),
                Err(error) => {
                    if let Task::Audio(path, generation) = job.task {
                        if generation != self.bank_generation {
                            continue;
                        }
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
        match (task, output) {
            (Task::Folder(folder, generation), Output::Samples(samples))
                if self.current_indexed_folder.as_ref() == Some(&folder)
                    && generation == self.folder_generation =>
            {
                self.samples = samples;
            }
            (Task::List(root, generation), Output::Packs(packs))
                if self.current_indexed_folder.as_ref() == Some(&root)
                    && generation == self.browser_generation =>
            {
                self.browser_packs = packs;
            }
            (Task::Bank(path, install, generation), Output::Bank(bank))
                if generation == self.bank_generation =>
            {
                if install {
                    self.stop_playback();
                    self.pack_bank = bank;
                    self.failed_audio.clear();
                    self.browser_open = false;
                    self.browser_status = Some(("Loaded sample pack".into(), Instant::now()));
                } else if self
                    .browser_selected_idx
                    .and_then(|idx| self.browser_packs.get(idx))
                    .is_some_and(|pack| pack.folder_path == path)
                {
                    self.browser_preview = Some((path, bank));
                }
            }
            (Task::Audio(path, generation), Output::Audio(decoded))
                if generation == self.bank_generation =>
            {
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
        if let Some(mut sample) = next {
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
                self.failed_audio.insert(sample.file_path);
                self.browser_status = Some((
                    "PCM memory limit reached; remove layers to load more samples".into(),
                    Instant::now(),
                ));
                return;
            }
            let path = sample.file_path.clone();
            self.submit_background(Task::Audio(path, self.bank_generation), move || {
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
            receiver,
        });
        state.stop_playback();
        state.clear_bank();
        assert!(state.writing_pack());
        sender
            .send(Ok(Ok(Output::Saved(PathBuf::from("/saved/Kit")))))
            .unwrap();
        state.poll_background();
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
