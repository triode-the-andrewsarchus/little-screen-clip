use super::*;

impl AudioPreviewState {
    pub fn toggle_play(&mut self) {
        if self.is_playing {
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

        if let Some(ref p) = self.player {
            p.clear();
            if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    p.append(ChannelMonitorSource::new(
                        EditedPreviewSource::new(decoder, Arc::clone(&self.playback_edits)),
                        Arc::clone(&self.channel_tools.monitor),
                    ));
                    let target_pos = Duration::from_secs_f32(s);
                    let _ = p.try_seek(target_pos);
                    p.play();
                    self.is_playing = true;
                }
            }
        }
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

        if let Some(ref p) = self.player {
            p.clear();
            if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    p.append(ChannelMonitorSource::new(
                        EditedPreviewSource::new(decoder, Arc::clone(&self.playback_edits)),
                        Arc::clone(&self.channel_tools.monitor),
                    ));
                    let target_pos = Duration::from_secs_f32(self.current_time_sec);
                    let _ = p.try_seek(target_pos);
                    p.play();
                    self.is_playing = true;
                    self.playback_start_instant = Some(Instant::now());
                    self.playback_start_offset_sec = self.current_time_sec;
                }
            }
        }
    }

    pub fn pause(&mut self) {
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

        if let Some(ref p) = self.player {
            if self.is_playing {
                let target_pos = Duration::from_secs_f32(clamped);
                if p.try_seek(target_pos).is_err() {
                    let can_reload = self
                        .last_seek_reload
                        .map(|t| t.elapsed().as_millis() >= 150)
                        .unwrap_or(true);
                    if can_reload {
                        self.last_seek_reload = Some(Instant::now());
                        p.clear();
                        if let Ok(file) = File::open(&self.path) {
                            if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                                p.append(ChannelMonitorSource::new(
                                    EditedPreviewSource::new(
                                        decoder,
                                        Arc::clone(&self.playback_edits),
                                    ),
                                    Arc::clone(&self.channel_tools.monitor),
                                ));
                                let _ = p.try_seek(target_pos);
                                p.play();
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn seek_to(&mut self, target_sec: f32) {
        self.seek_to_internal(target_sec, true);
    }

    pub fn loop_seek_to(&mut self, target_sec: f32) {
        self.seek_to_internal(target_sec, false);
    }
}
