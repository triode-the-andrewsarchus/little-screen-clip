use super::*;

pub(super) type FileDecoder = Decoder<BufReader<File>>;
pub(super) struct PlaybackLoad {
    ticket: JobTicket,
    pub(super) cancel: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<FileDecoder, String>, JobError>>,
    position: f32,
}
impl Drop for PlaybackLoad {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

pub(super) fn load_decoder(
    path: &Path,
    check: &crate::file_source::ReadCheck,
    expected: Option<&crate::file_source::FileSource>,
    cancel: &AtomicBool,
    position: f32,
) -> Result<FileDecoder, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Playback cancelled".into());
    }
    let (file, observed) = check.open(path, expected, cancel)?;
    let mut decoder = Decoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
    if position > 0.0 {
        decoder
            .try_seek(Duration::from_secs_f32(position))
            .map_err(|e| e.to_string())?;
    }
    check.validate(cancel)?;
    observed.check()?;
    if cancel.load(Ordering::Acquire) {
        return Err("Playback cancelled".into());
    }
    Ok(decoder)
}

impl AudioPreviewState {
    pub fn toggle_play(&mut self) {
        if self.is_playing || self.playback_load.is_some() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Master preview volume for the source, grouped stems, and stem editor.
    pub(crate) fn set_studio_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(player) = &self.player {
            player.set_volume(self.volume);
        }
        for stem in &mut self.stem_separation.stems {
            stem.preview.volume = self.volume;
            if let Some(player) = &stem.preview.player {
                player.set_volume(self.volume);
            }
            if let Some(player) = &stem.group_player {
                player.set_volume(self.volume);
            }
        }
    }

    pub(super) fn ensure_player(&mut self) {
        if self.player.is_none() {
            self.player = (self.player_factory)();
            if let Some(player) = &self.player {
                player.set_volume(self.volume);
            }
        }
    }

    pub fn play_slice_preview_ex(&mut self, start_sec: f32, end_sec: f32, looping: bool) {
        self.ensure_player();
        let (s, mut e) = if start_sec < end_sec {
            (start_sec, end_sec)
        } else {
            (end_sec, start_sec)
        };
        if e <= s {
            e = (s + 0.1).min(self.duration_sec);
        }
        self.preview_stop_time = Some(e);
        self.preview_slice_range = Some((s, e));
        self.preview_is_looping = looping;
        self.current_time_sec = s;
        self.playback_start_offset_sec = s;
        self.playback_start_instant = Some(Instant::now());

        if self.active_mode == AudioToolMode::HfAnalysis {
            self.play_hf_audio_at(s);
            return;
        }

        self.start_file_playback(s);
    }

    pub fn play_slice_preview(&mut self, start_sec: f32, end_sec: f32) {
        self.play_slice_preview_ex(start_sec, end_sec, false);
    }

    pub fn play_slice_preview_loop(&mut self, start_sec: f32, end_sec: f32) {
        self.play_slice_preview_ex(start_sec, end_sec, true);
    }

    pub fn play(&mut self) {
        self.ensure_player();
        self.preview_stop_time = None;
        self.preview_slice_range = None;
        self.preview_is_looping = false;
        if self.player.is_none() {
            return;
        }

        // If selection exists and playhead is outside, jump to selection start
        let sel = self.active_selection_range();
        if let Some((start, end)) = sel {
            if self.current_time_sec < start || self.current_time_sec >= end {
                self.current_time_sec = start;
            }
        }

        // If at end of track, wrap to start
        if self.current_time_sec >= self.duration_sec - 0.05 {
            self.current_time_sec = sel.map(|(s, _)| s).unwrap_or(0.0);
        }

        if self.active_mode == AudioToolMode::HfAnalysis {
            self.play_hf_audio_at(self.current_time_sec);
            return;
        }

        self.start_file_playback(self.current_time_sec);
    }

    pub fn pause(&mut self) {
        self.playback_load = None;
        self.preview_stop_time = None;
        self.preview_slice_range = None;
        self.preview_is_looping = false;
        if let Some(ref p) = self.player {
            p.pause();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
    }

    pub fn stop(&mut self) {
        self.playback_load = None;
        self.preview_stop_time = None;
        self.preview_slice_range = None;
        self.preview_is_looping = false;
        if let Some(ref p) = self.player {
            p.stop();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
        self.current_time_sec = self.active_selection_range().map(|(s, _)| s).unwrap_or(0.0);
    }

    pub fn seek_to_internal(&mut self, target_sec: f32, clear_preview: bool) {
        if clear_preview {
            self.preview_stop_time = None;
            self.preview_slice_range = None;
            self.preview_is_looping = false;
        }
        let clamped = target_sec.clamp(0.0, self.duration_sec);
        self.current_time_sec = clamped;
        self.playback_start_offset_sec = clamped;
        self.playback_start_instant = Some(Instant::now());

        if self.active_mode == AudioToolMode::HfAnalysis && self.is_playing {
            self.play_hf_audio_at(clamped);
            return;
        }

        let reload = self.playback_load.is_some()
            || (self.is_playing
                && self
                    .player
                    .as_ref()
                    .is_some_and(|p| p.try_seek(Duration::from_secs_f32(clamped)).is_err()));
        if reload {
            self.start_file_playback(clamped);
        }
    }

    pub(super) fn start_file_playback(&mut self, position: f32) {
        self.playback_load = None;
        if let Some(player) = &self.player {
            player.clear();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
        let path = self.path.clone();
        let check = self.read_check.clone();
        let expected = self.file_source.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::from_secs(60), move || {
                load_decoder(&path, &check, expected.as_ref(), &worker, position)
            });
        self.playback_load = Some(PlaybackLoad {
            ticket,
            cancel,
            receiver,
            position,
        });
    }

    pub(super) fn poll_file_playback(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.playback_load else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result.map_err(|e| e.to_string()).and_then(|r| r),
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(20));
                return;
            }
            Err(TryRecvError::Disconnected) => Err("Playback worker stopped".into()),
        };
        let position = self.playback_load.take().unwrap().position;
        match result {
            Ok(decoder) => {
                if let Some(player) = &self.player {
                    player.append(ChannelMonitorSource::new(
                        EditedPreviewSource::new(decoder, Arc::clone(&self.playback_edits)),
                        Arc::clone(&self.channel_tools.monitor),
                    ));
                    player.play();
                    self.is_playing = true;
                    self.playback_start_instant = Some(Instant::now());
                    self.playback_start_offset_sec = position;
                }
            }
            Err(error) => self.status_toast = Some((error.into(), ctx.input(|i| i.time))),
        }
    }

    pub fn seek_to(&mut self, target_sec: f32) {
        self.seek_to_internal(target_sec, true);
    }

    pub fn loop_seek_to(&mut self, target_sec: f32) {
        self.seek_to_internal(target_sec, false);
    }
}
