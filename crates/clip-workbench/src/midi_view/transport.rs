use super::*;

impl MidiViewerState {
    pub fn play(&mut self) {
        if self.is_playing {
            return;
        }

        self.user_panned_during_playback = false;

        if self.wav_cache.is_none() {
            self.pending_play = true;
            if !self.is_synthesizing {
                self.is_synthesizing = true;
                let data = self.data.clone();
                let solo = self.solo_track;
                let (ticket, rx) = BackgroundJobs::global()
                    .submit(Duration::from_secs(120), move || {
                        (solo, data.synthesize_tracks_to_wav(solo))
                    });
                self.synth_rx = Some(rx);
                self.synth_ticket = Some(ticket);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64();
                self.status_toast.set("Synthesizing audio…", now);
            }
            return;
        }

        let wav_bytes = self.wav_cache.as_ref().unwrap();

        // Calculate offset in the WAV audio corresponding to playhead_sec so all tracks start at the playhead
        let sample_rate = 44100u32;
        let bytes_per_sample = 2usize; // 16-bit mono PCM
        let sample_offset = (self.playhead_sec.max(0.0) * sample_rate as f32) as usize;
        let byte_offset = 44 + sample_offset * bytes_per_sample;

        let sliced_wav: Vec<u8> = if byte_offset < wav_bytes.len() {
            let remaining_pcm = &wav_bytes[byte_offset..];
            let mut buf = Vec::with_capacity(44 + remaining_pcm.len());
            buf.extend_from_slice(&wav_bytes[0..4]); // "RIFF"
            let riff_size = (36 + remaining_pcm.len()) as u32;
            buf.extend_from_slice(&riff_size.to_le_bytes());
            buf.extend_from_slice(&wav_bytes[8..40]); // format chunk up to "data"
            let data_size = remaining_pcm.len() as u32;
            buf.extend_from_slice(&data_size.to_le_bytes());
            buf.extend_from_slice(remaining_pcm);
            buf
        } else {
            // Already at or beyond end of song
            return;
        };

        match crate::ui_ext::new_shared_audio_player() {
            Some(player) => {
                let cursor = Cursor::new(sliced_wav);
                match Decoder::new(cursor) {
                    Ok(source) => {
                        player.append(source);
                        self.player = Some(player);
                        self._sink = None;
                        self.is_playing = true;
                        self.playback_start_instant = Some(Instant::now());
                        self.playback_start_offset = self.playhead_sec;
                    }
                    Err(e) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64();
                        self.status_toast
                            .set(format!("Audio decode error: {}", e), now);
                    }
                }
            }
            None => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64();
                self.status_toast.set(
                    "Audio device error: could not open shared audio output",
                    now,
                );
            }
        }
    }

    pub fn play_selection(&mut self) {
        if let Some((s, e)) = self.selected_region {
            if (e - s).abs() > 0.05 {
                let start = s.min(e);
                let end = s.max(e);
                self.playhead_sec = start;
                self.play_stop_time = Some(end);
            } else {
                self.play_stop_time = None;
            }
        } else {
            self.play_stop_time = None;
        }
        self.play();
    }

    pub fn toggle_solo(&mut self, track_idx: usize) {
        if self.solo_track == Some(track_idx) {
            self.solo_track = None;
        } else {
            self.solo_track = Some(track_idx);
        }
        self.wav_cache = None;
        self.pending_play = false;
        self.is_synthesizing = false;
        self.synth_rx = None;
        if let Some(ticket) = self.synth_ticket.take() {
            ticket.cancel();
        }
        if self.is_playing {
            let cur = self.playhead_sec;
            let stop_time = self.play_stop_time;
            self.stop();
            self.playhead_sec = cur;
            self.play_stop_time = stop_time;
            self.play();
        }
    }

    pub fn clear_solo(&mut self) {
        if self.solo_track.is_some() {
            self.solo_track = None;
            self.wav_cache = None;
            self.pending_play = false;
            self.is_synthesizing = false;
            self.synth_rx = None;
            if let Some(ticket) = self.synth_ticket.take() {
                ticket.cancel();
            }
            if self.is_playing {
                let cur = self.playhead_sec;
                let stop_time = self.play_stop_time;
                self.stop();
                self.playhead_sec = cur;
                self.play_stop_time = stop_time;
                self.play();
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(player) = &self.player {
            player.pause();
        }
        self.pending_play = false;
        self.is_playing = false;
        self.playback_start_instant = None;
        self.user_panned_during_playback = false;
    }

    pub fn stop(&mut self) {
        if let Some(player) = &self.player {
            player.stop();
        }
        self.player = None;
        self._sink = None;
        self.pending_play = false;
        self.is_playing = false;
        if let Some((s, _)) = self.selected_region {
            self.playhead_sec = s;
        } else {
            self.playhead_sec = 0.0;
        }
        self.playback_start_instant = None;
        self.playback_start_offset = 0.0;
        self.play_stop_time = None;
        self.user_panned_during_playback = false;
    }

    pub fn update_playback(&mut self) {
        let synth_result = self.synth_rx.as_ref().map(|rx| rx.try_recv());
        if let Some(result) = synth_result {
            match result {
                Ok(Ok((solo, wav))) => {
                    self.is_synthesizing = false;
                    self.synth_rx = None;
                    self.synth_ticket = None;
                    if self.solo_track == solo {
                        self.wav_cache = Some(wav);
                        if self.pending_play {
                            self.pending_play = false;
                            self.play();
                        }
                    }
                }
                Ok(Err(error)) => {
                    self.is_synthesizing = false;
                    self.pending_play = false;
                    self.synth_rx = None;
                    self.synth_ticket = None;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_secs_f64())
                        .unwrap_or(0.0);
                    self.status_toast
                        .set(format!("MIDI synthesis failed: {error}"), now);
                }
                Err(TryRecvError::Disconnected) => {
                    self.is_synthesizing = false;
                    self.pending_play = false;
                    self.synth_rx = None;
                    self.synth_ticket = None;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_secs_f64())
                        .unwrap_or(0.0);
                    self.status_toast
                        .set("MIDI synthesis worker disconnected", now);
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if self.is_playing {
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                self.playhead_sec = self.playback_start_offset + elapsed;

                if let Some(stop_time) = self.play_stop_time {
                    if self.playhead_sec >= stop_time {
                        self.stop();
                        if let Some((s, _)) = self.selected_region {
                            self.playhead_sec = s;
                        }
                        return;
                    }
                }

                if self.playhead_sec >= self.data.duration_sec {
                    self.stop();
                } else {
                    // Auto-follow playhead when zoomed in, UNLESS user actively panned away during playback
                    if !self.user_panned_during_playback
                        && self.view_duration_sec < self.data.duration_sec
                    {
                        let view_end = self.view_start_sec + self.view_duration_sec;
                        if self.playhead_sec > view_end || self.playhead_sec < self.view_start_sec {
                            let new_start = (self.playhead_sec - 0.1 * self.view_duration_sec)
                                .clamp(
                                    0.0,
                                    (self.data.duration_sec - self.view_duration_sec).max(0.0),
                                );
                            self.view_start_sec = new_start;
                        }
                    }
                }
            }
        }
    }
}
