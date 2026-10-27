//! One guarded worker prepares the audible stem group before any player starts.
use super::*;
use crate::audio_preview::playback::{FileDecoder, load_decoder};

#[derive(Clone, PartialEq)]
struct GroupInput {
    index: usize,
    path: PathBuf,
    check: crate::file_source::ReadCheck,
    identity: Option<crate::file_source::FileSource>,
}

pub(super) struct GroupLoad {
    ticket: JobTicket,
    cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<Vec<(usize, FileDecoder)>, String>, JobError>>,
    inputs: Vec<GroupInput>,
}
impl Drop for GroupLoad {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

fn load_group(
    inputs: &[GroupInput],
    offset: f32,
    cancel: &AtomicBool,
) -> Result<Vec<(usize, FileDecoder)>, String> {
    let decoders = inputs
        .iter()
        .map(|input| {
            load_decoder(
                &input.path,
                &input.check,
                input.identity.as_ref(),
                cancel,
                offset,
            )
            .map(|decoder| (input.index, decoder))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // A later decoder may have taken time to prepare; revalidate the whole
    // batch before publishing any of it to the players.
    for input in inputs {
        input.check.validate(cancel)?;
        if let Some(identity) = &input.identity {
            identity.check()?;
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err("Stem playback cancelled".into());
    }
    Ok(decoders)
}

impl AudioPreviewState {
    pub(crate) fn group_duration(&self) -> f32 {
        self.stem_separation
            .stems
            .iter()
            .map(|stem| stem.preview.duration_sec)
            .fold(self.duration_sec, f32::max)
            .max(0.01)
    }

    fn group_inputs(&self) -> Vec<GroupInput> {
        self.stem_separation
            .stems
            .iter()
            .enumerate()
            .filter(|(index, stem)| {
                stem.group_player.is_some()
                    && stem_is_audible(*index, stem.muted, self.stem_separation.solo_stem)
            })
            .map(|(index, stem)| GroupInput {
                index,
                path: stem.preview.path.clone(),
                check: stem.preview.read_check.clone(),
                identity: stem.preview.file_source.clone(),
            })
            .collect()
    }

    fn queue_group_playback(&mut self, offset: f32) {
        self.stem_separation.group_load = None;
        for stem in &self.stem_separation.stems {
            if let Some(player) = &stem.group_player {
                player.clear();
                player.pause();
            }
        }
        let inputs = self.group_inputs();
        let captured = inputs.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::from_secs(60), move || {
                load_group(&captured, offset, &worker_cancel)
            });
        self.stem_separation.group_load = Some(GroupLoad {
            ticket,
            cancel,
            receiver,
            inputs,
        });
        self.stem_separation.group_playback_offset_sec = offset;
        self.stem_separation.group_playback_start = None;
        self.stem_separation.group_is_playing = true;
        self.stem_separation.error = None;
    }

    fn poll_group_playback(&mut self, ctx: &egui::Context) {
        let Some(job) = self.stem_separation.group_load.take() else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Err(TryRecvError::Empty) => {
                self.stem_separation.group_load = Some(job);
                ctx.request_repaint_after(Duration::from_millis(20));
                return;
            }
            Err(TryRecvError::Disconnected) => Err("Stem playback worker stopped".into()),
        };
        let result = if self.group_inputs() != job.inputs {
            Err("Stem playback discarded because its sources or routing changed".into())
        } else {
            result
        };
        match result {
            Ok(decoders) => {
                // Queue every track while paused, then start the group together.
                for (index, decoder) in decoders {
                    if let Some(player) = &self.stem_separation.stems[index].group_player {
                        player.append(decoder);
                    }
                }
                for stem in &self.stem_separation.stems {
                    if let Some(player) = &stem.group_player {
                        player.play();
                    }
                }
                self.stem_separation.group_playback_start = Some(Instant::now());
            }
            Err(error) => {
                self.pause_group_playback();
                self.stem_separation.error = Some(format!("Could not play stems: {error}"));
            }
        }
    }

    pub(crate) fn start_group_playback(&mut self) {
        self.pause();
        self.stem_separation.editor_playhead_active = false;
        if self.stem_separation.group_current_time_sec >= self.group_duration() - 0.05 {
            self.stem_separation.group_current_time_sec = 0.0;
        }
        for stem in &mut self.stem_separation.stems {
            stem.preview.pause();
        }
        self.queue_group_playback(self.stem_separation.group_current_time_sec);
    }

    pub(crate) fn update_group_playback(&mut self, ctx: &egui::Context) {
        self.poll_group_playback(ctx);
        if !self.stem_separation.group_is_playing {
            return;
        }
        ctx.request_repaint();
        if let Some(started) = self.stem_separation.group_playback_start {
            let current =
                self.stem_separation.group_playback_offset_sec + started.elapsed().as_secs_f32();
            let duration = self.group_duration();
            if current >= duration {
                self.stop_group_playback();
                self.stem_separation.group_current_time_sec = duration;
            } else {
                self.stem_separation.group_current_time_sec = current;
            }
        }
    }

    pub(crate) fn pause_group_playback(&mut self) {
        self.stem_separation.group_load = None;
        if self.stem_separation.group_is_playing {
            if let Some(started) = self.stem_separation.group_playback_start {
                self.stem_separation.group_current_time_sec =
                    self.stem_separation.group_playback_offset_sec
                        + started.elapsed().as_secs_f32();
            }
        }
        for stem in &self.stem_separation.stems {
            if let Some(player) = &stem.group_player {
                player.pause();
            }
        }
        self.stem_separation.group_is_playing = false;
        self.stem_separation.group_playback_start = None;
    }

    pub(crate) fn stop_group_playback(&mut self) {
        self.pause_group_playback();
        self.stem_separation.editor_playhead_active = false;
        for stem in &self.stem_separation.stems {
            if let Some(player) = &stem.group_player {
                player.stop();
            }
        }
        self.stem_separation.group_playback_offset_sec = 0.0;
        self.stem_separation.group_current_time_sec = 0.0;
    }

    pub(crate) fn seek_group_playback(&mut self, time_sec: f32) {
        self.stem_separation.editor_playhead_active = false;
        let target = time_sec.clamp(0.0, self.group_duration());
        self.stem_separation.group_current_time_sec = target;
        self.stem_separation.group_playback_offset_sec = target;
        if self.stem_separation.group_is_playing {
            self.queue_group_playback(target);
        }
    }

    pub(crate) fn refresh_group_routing(&mut self) {
        if self.stem_separation.group_is_playing {
            self.queue_group_playback(self.stem_separation.group_current_time_sec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_preview::tests::{empty_audio_analysis, generate_fixture_wav};

    #[test]
    fn group_load_rechecks_earlier_sources_and_rejects_changed_file_identity() {
        let (_directory, path) = generate_fixture_wav("group-read", 0.1);
        let expired = Arc::new(AtomicBool::new(false));
        let first = expired.clone();
        let second = expired.clone();
        let input = GroupInput {
            index: 0,
            path: path.clone(),
            identity: None,
            check: crate::file_source::ReadCheck::new(move |_| {
                if first.load(Ordering::Acquire) {
                    Err("first source expired".into())
                } else {
                    Ok(())
                }
            }),
        };
        let later = GroupInput {
            index: 1,
            path: path.clone(),
            identity: None,
            check: crate::file_source::ReadCheck::new(move |_| {
                second.store(true, Ordering::Release);
                Ok(())
            }),
        };
        let result = load_group(&[input, later], 0.04, &AtomicBool::new(false));
        assert_eq!(result.err().as_deref(), Some("first source expired"));
        let identity = crate::file_source::FileSource::capture(&path).unwrap();
        std::fs::write(&path, b"replacement must not be decoded").unwrap();
        let input = GroupInput {
            index: 0,
            path,
            identity: Some(identity),
            check: Default::default(),
        };
        let result = load_group(&[input], 0.0, &AtomicBool::new(false));
        assert!(result.err().unwrap().contains("File source changed"));
        assert!(load_group(&[], 0.0, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn group_completion_rejects_changed_routing_without_starting_any_player() {
        let (_directory, path) = generate_fixture_wav("group-stale", 0.1);
        let mut state = AudioPreviewState::from_completed_source(
            path.clone(),
            "group".into(),
            None,
            Arc::new(empty_audio_analysis()),
        );
        let (player, _output) = Player::new();
        state.stem_separation.stems.push(SeparatedStem {
            name: "vocals".into(),
            muted: false,
            group_player: Some(player),
            preview: AudioPreviewState::from_completed_source(
                path.clone(),
                "vocals".into(),
                None,
                Arc::new(empty_audio_analysis()),
            ),
        });
        // Supply an already completed real decoder to exercise publication,
        // independently of the worker's scheduling and without an audio device.
        let inputs = state.group_inputs();
        let decoders = load_group(&inputs, 0.0, &AtomicBool::new(false)).unwrap();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(5), move || Ok(decoders));
        state.stem_separation.group_load = Some(GroupLoad {
            ticket,
            receiver,
            inputs,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        state.stem_separation.group_is_playing = true;
        state.stem_separation.stems[0].muted = true;
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.stem_separation.group_load.is_some() {
            assert!(Instant::now() < deadline, "group completion timed out");
            state.update_group_playback(&egui::Context::default());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!state.stem_separation.group_is_playing);
        assert!(state.stem_separation.group_playback_start.is_none());
        assert!(
            state
                .stem_separation
                .error
                .as_ref()
                .unwrap()
                .contains("sources or routing changed")
        );
        assert!(
            state.stem_separation.stems[0]
                .group_player
                .as_ref()
                .unwrap()
                .empty()
        );
    }

    #[test]
    fn group_seek_pause_and_stop_retire_pending_preparation() {
        let mut state = AudioPreviewState::from_completed_source(
            PathBuf::from("/missing-group.wav"),
            "group".into(),
            None,
            Arc::new(empty_audio_analysis()),
        );
        state.duration_sec = 1.0;
        state.start_group_playback();
        let old = state
            .stem_separation
            .group_load
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        assert!(state.stem_separation.group_is_playing);
        assert!(state.stem_separation.group_playback_start.is_none());
        state.seek_group_playback(0.4);
        assert!(old.load(Ordering::Acquire));
        assert_eq!(state.stem_separation.group_playback_offset_sec, 0.4);
        let next = state
            .stem_separation
            .group_load
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        state.pause_group_playback();
        assert!(next.load(Ordering::Acquire));
        state.update_group_playback(&egui::Context::default());
        assert!(!state.stem_separation.group_is_playing);
        assert!(state.stem_separation.group_playback_start.is_none());
        state.start_group_playback();
        let last = state
            .stem_separation
            .group_load
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        state.stop_group_playback();
        assert!(last.load(Ordering::Acquire));
        assert_eq!(state.stem_separation.group_current_time_sec, 0.0);
    }
}
