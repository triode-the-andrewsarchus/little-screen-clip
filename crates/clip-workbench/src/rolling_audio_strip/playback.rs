//! A cancellable read prepares a paused, range-bounded player before UI delivery.
use super::*;
use little_clip_core::file_source::ReadCheck;
use rodio::Source;

pub(super) struct PlaybackLoad {
    task: Task<Result<Player, String>>,
    path: PathBuf,
    source: FileSource,
    range: (f32, f32),
}

// Rolling snapshots have a fixed PCM channel/rate layout. Count complete frames
// instead of accumulating truncated nanosecond durations for individual samples.
struct SelectionSource {
    decoder: crate::audio_preview::AudioDecoder,
    remaining: usize,
    length: usize,
}
impl Iterator for SelectionSource {
    type Item = f32;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        match self.decoder.next() {
            Some(sample) => {
                self.remaining -= 1;
                Some(sample)
            }
            None => {
                self.remaining = 0;
                None
            }
        }
    }
}
impl Source for SelectionSource {
    fn current_span_len(&self) -> Option<usize> {
        Some(
            self.decoder
                .current_span_len()
                .unwrap_or(self.remaining)
                .min(self.remaining),
        )
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.decoder.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.decoder.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.length as f64 / self.channels().get() as f64 / self.sample_rate().get() as f64,
        ))
    }
}

fn prepare_source(
    path: &Path,
    expected: &FileSource,
    range: (f32, f32),
    cancel: &AtomicBool,
) -> Result<SelectionSource, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Rolling playback cancelled".into());
    }
    let (file, observed) = ReadCheck::default().open(path, Some(expected), cancel)?;
    let held = file.try_clone().map_err(|error| error.to_string())?;
    let mut decoder = crate::audio_preview::decode_audio_source(Box::new(file))
        .map_err(|error| error.to_string())?;
    let rate = decoder.sample_rate().get() as f64;
    let first = (range.0 as f64 * rate).floor();
    let last = (range.1 as f64 * rate).floor();
    decoder
        .try_seek(Duration::from_secs_f64(first / rate))
        .map_err(|error| error.to_string())?;
    if FileSource::capture_file(&held)? != observed {
        return Err("Rolling snapshot changed during playback preparation; refresh it.".into());
    }
    observed.check()?;
    if cancel.load(Ordering::Acquire) {
        return Err("Rolling playback cancelled".into());
    }
    // Bound the source itself: an idle UI must not play beyond the selection.
    let length = ((last - first) as usize) * decoder.channels().get() as usize;
    Ok(SelectionSource {
        decoder,
        remaining: length,
        length,
    })
}

impl RollingAudioStripState {
    pub fn play_selection(&mut self) {
        self.queue_playback_on(
            BackgroundJobs::global(),
            crate::ui_ext::new_shared_audio_player,
        );
    }

    fn queue_playback_on(
        &mut self,
        jobs: &BackgroundJobs,
        player_factory: impl FnOnce() -> Option<Player> + Send + 'static,
    ) {
        let Some(range) = self.playback_range() else {
            return;
        };
        self.stop();
        if self.is_silent() {
            self.playback_notice.set(
                "Buffer is currently silence — play audio through speakers first, then click Refresh",
                feedback_time(),
            );
            return;
        }
        if self.selected_region_peak() < 0.001 {
            self.playback_notice.set(
                "Selected region is silence — drag over colored waveform peaks to hear audio",
                feedback_time(),
            );
        }
        let (Some(path), Some(source)) = (self.path.clone(), self.source.clone()) else {
            self.playback_notice.set(
                StatusMessage::error("Refresh the rolling snapshot before playing it."),
                feedback_time(),
            );
            return;
        };
        let captured_path = path.clone();
        let captured_source = source.clone();
        let snapshot_lease = self.snapshot_lease.clone();
        let task = Task::start(jobs, Duration::from_secs(20), move |cancel| {
            let _snapshot_lease = snapshot_lease;
            let decoder = prepare_source(&captured_path, &captured_source, range, cancel)?;
            // Device discovery and queue initialization also belong off the UI thread.
            let player = player_factory().ok_or("Audio output is unavailable")?;
            player.pause();
            player.set_volume(1.0);
            player.append(decoder);
            if cancel.load(Ordering::Acquire) {
                return Err("Rolling playback cancelled".into());
            }
            Ok(player)
        })
        .with_response_deadline(Duration::from_secs(20));
        self.playhead_sec = range.0;
        self.playback_job = Some(PlaybackLoad {
            task,
            path,
            source,
            range,
        });
    }

    fn playback_range(&self) -> Option<(f32, f32)> {
        let (a, b) = self.selected_region?;
        if !a.is_finite() || !b.is_finite() || a.min(b) < 0.0 {
            return None;
        }
        let end = a.max(b).min(self.duration_sec);
        (end - a.min(b) > 0.05).then_some((a.min(b), end))
    }

    pub(super) fn poll_playback(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.playback_job else {
            return;
        };
        if self.path.as_ref() != Some(&job.path)
            || self.source.as_ref() != Some(&job.source)
            || self.playback_range() != Some(job.range)
        {
            self.stop();
            return;
        }
        let Some(result) = job.task.poll() else {
            ctx.request_repaint_after(Duration::from_millis(20));
            return;
        };
        let range = job.range;
        self.playback_job = None;
        match result
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        {
            Ok(player) => {
                self.playback_start_offset_sec = range.0;
                self.play_stop_time = Some(range.1);
                player.play();
                self.player = Some(player);
                self.is_playing = true;
            }
            Err(error) => self.playback_notice.set(
                StatusMessage::error(format!("Could not play rolling audio: {error}")),
                feedback_time(),
            ),
        }
        ctx.request_repaint();
    }

    pub fn update_playback(&mut self) {
        if !self.is_playing {
            return;
        }
        let Some(player) = &self.player else {
            return;
        };
        self.playhead_sec = (self.playback_start_offset_sec + player.get_pos().as_secs_f32())
            .min(self.play_stop_time.unwrap_or(self.duration_sec));
        if player.empty() {
            self.stop();
        }
    }

    pub fn stop(&mut self) {
        self.playback_job = None;
        self.player = None;
        self.playback_notice.clear();
        self.is_playing = false;
        self.play_stop_time = None;
        if let Some((a, b)) = self.selected_region {
            self.playhead_sec = a.min(b);
        }
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing || self.playback_job.is_some() {
            self.stop();
        } else {
            self.play_selection();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rate: u32) -> (tempfile::TempDir, RollingAudioStripState, Vec<i16>) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.wav");
        let samples: Vec<i16> = (0..rate * 2)
            .flat_map(|index| {
                let sample = index as i16;
                [sample, sample.wrapping_neg()]
            })
            .collect();
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(2, rate, 2);
        buffer.push_samples(&samples);
        buffer.write_wav(&path).unwrap();
        let snapshot = read_snapshot(&path, &AtomicBool::new(false)).unwrap();
        let mut state = RollingAudioStripState::empty_test_strip();
        state.path = Some(path);
        state.source = Some(snapshot.source);
        state.duration_sec = 2.0;
        state.sample_rate = rate;
        // Opposite channels average to zero; keep a non-silent display fixture
        // to exercise preparation instead of the explicit silence hint.
        state.mono_samples = vec![0.5; rate as usize * 2];
        state.selected_region = Some((0.25, 0.75));
        (directory, state, samples)
    }

    fn poll_until_prepared(state: &mut RollingAudioStripState) {
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.playback_job.is_some() {
            state.poll_playback(&ctx);
            assert!(
                Instant::now() < deadline,
                "playback preparation did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn prepared_source_seeks_and_bounds_both_channels_without_a_ui_timer() {
        for rate in [8000, 44_100, 48_000] {
            let (_directory, state, samples) = fixture(rate);
            let source = prepare_source(
                state.path.as_deref().unwrap(),
                state.source.as_ref().unwrap(),
                (0.25, 0.75),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(source.channels().get(), 2);
            assert_eq!(source.sample_rate().get(), rate);
            let actual: Vec<f32> = source.collect();
            let first = rate as usize / 4 * 2;
            let last = rate as usize * 3 / 4 * 2;
            let expected: Vec<f32> = samples[first..last]
                .iter()
                .map(|s| *s as f32 / 32768.0)
                .collect();
            assert_eq!(actual.len(), expected.len(), "range length at {rate} Hz");
            for (actual, expected) in actual.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 0.00001,
                    "range samples at {rate} Hz"
                );
            }
        }
    }

    #[test]
    fn queued_playback_is_nonblocking_and_stop_cancels_before_device_creation() {
        let (_directory, mut state, _) = fixture(8000);
        let jobs = BackgroundJobs::new(1, 2);
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let (_, finished) = jobs.submit(Duration::from_secs(5), move || {
            started.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        state.queue_playback_on(&jobs, || {
            panic!("cancelled playback opened an audio device")
        });
        let token = state.playback_job.as_ref().unwrap().task.token.clone();
        assert!(!state.is_playing);
        assert!(!token.load(Ordering::Acquire));
        state.toggle_playback();
        assert!(state.playback_job.is_none());
        assert!(token.load(Ordering::Acquire));
        release.send(()).unwrap();
        finished
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        // A subsequent admitted task proves the cancelled queue entry was retired.
        let (_, drained) = jobs.submit(Duration::from_secs(5), || ());
        drained
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(!state.is_playing);
    }

    #[test]
    fn stale_snapshot_and_explicit_cancellation_reject_before_device_creation() {
        let (_directory, mut state, _) = fixture(8000);
        let jobs = BackgroundJobs::new(1, 2);
        let path = state.path.clone().unwrap();
        assert!(
            prepare_source(
                &path,
                state.source.as_ref().unwrap(),
                (0.25, 0.75),
                &AtomicBool::new(true)
            )
            .is_err()
        );
        let old = path.with_extension("old.wav");
        std::fs::rename(&path, &old).unwrap();
        std::fs::copy(&old, &path).unwrap();
        state.queue_playback_on(&jobs, || panic!("stale source opened an audio device"));
        poll_until_prepared(&mut state);
        assert!(!state.is_playing);
        assert!(
            state
                .playback_notice
                .message(feedback_time() + 60.0)
                .unwrap()
                .contains("changed")
        );
    }

    #[test]
    fn device_setup_runs_on_worker_and_player_waits_for_ui_delivery() {
        let (_directory, mut state, _) = fixture(8000);
        let jobs = BackgroundJobs::new(1, 2);
        let ui_thread = std::thread::current().id();
        state.queue_playback_on(&jobs, move || {
            assert_ne!(std::thread::current().id(), ui_thread);
            Some(Player::new().0)
        });
        assert!(!state.is_playing);
        assert!(state.player.is_none());
        poll_until_prepared(&mut state);
        assert!(state.is_playing);
        assert!(!state.player.as_ref().unwrap().is_paused());
        state.stop();
        assert!(state.player.is_none());
        assert!(!state.is_playing);
    }

    #[test]
    fn invalid_ranges_do_not_admit_playback_and_reversed_ranges_are_normalized() {
        let (_directory, mut state, _) = fixture(8000);
        let jobs = BackgroundJobs::new(1, 2);
        for range in [
            (f32::NAN, 1.0),
            (0.0, f32::INFINITY),
            (-1.0, 1.0),
            (3.0, 4.0),
            (0.0, 0.01),
        ] {
            state.selected_region = Some(range);
            state.queue_playback_on(&jobs, || panic!("invalid range reached audio setup"));
            assert!(state.playback_job.is_none());
        }
        state.selected_region = Some((3.0, 1.0));
        assert_eq!(state.playback_range(), Some((1.0, 2.0)));
    }

    #[test]
    fn selection_change_retires_ready_player_without_starting_it() {
        let (_directory, mut state, _) = fixture(8000);
        let jobs = BackgroundJobs::new(1, 2);
        let (sender, receiver) = std::sync::mpsc::channel();
        state.queue_playback_on(&jobs, || Some(Player::new().0));
        // Serialize behind preparation, then change the range before UI delivery.
        let (_, done) = jobs.submit(Duration::from_secs(5), move || sender.send(()).unwrap());
        receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        state.selected_region = Some((0.5, 1.0));
        state.poll_playback(&egui::Context::default());
        assert!(state.playback_job.is_none());
        assert!(state.player.is_none());
        assert!(!state.is_playing);
    }
}
