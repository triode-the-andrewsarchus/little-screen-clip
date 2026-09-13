//! Piano roll visualization and MIDI playback workbench view.
//!
//! Renders multi-track MIDI piano rolls, measure grid lines, note inspection,
//! and live audio synthesis playback via Rodio.

use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_ops::{MidiData, MidiNote};
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use rodio::{Decoder, Player};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Instant;

/// Interactive UI state for piano roll inspection and MIDI playback.
pub struct MidiViewerState {
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
    pub title: String,
    pub data: MidiData,
    pub is_playing: bool,
    pub playhead_sec: f32,
    playback_start_instant: Option<Instant>,
    playback_start_offset: f32,
    player: Option<Player>,
    _sink: Option<MixerDeviceSink>,
    pub wav_cache: Option<Vec<u8>>,
    pub view_start_sec: f32,
    pub view_duration_sec: f32,
    pub user_panned_during_playback: bool,
    pub was_playing_before_drag: bool,
    pub hovered_note: Option<MidiNote>,
    pub status_toast: Option<(String, f64)>,
}

/// Computes a distinct palette color for notes in a given MIDI track and channel using theme harmonies.
pub fn get_track_color(
    track_idx: usize,
    channel: u8,
    total_tracks: usize,
    palette: &little_clip_core::theme::AppPalette,
) -> Color32 {
    let drum_color = crate::hex_to_c32(&palette.harmonies.warm_attention);
    let base_accent = crate::hex_to_c32(&palette.harmonies.base);
    if total_tracks <= 1 {
        if channel == 9 {
            drum_color
        } else {
            base_accent
        }
    } else {
        let track_palette: [Color32; 10] = [
            crate::hex_to_c32(&palette.harmonies.base),
            crate::hex_to_c32(&palette.harmonies.complementary),
            crate::hex_to_c32(&palette.harmonies.triadic_1),
            crate::hex_to_c32(&palette.harmonies.triadic_2),
            crate::hex_to_c32(&palette.harmonies.tetradic_1),
            crate::hex_to_c32(&palette.harmonies.tetradic_2),
            crate::hex_to_c32(&palette.diff_add),
            crate::hex_to_c32(&palette.diff_mod),
            crate::hex_to_c32(&palette.diff_del),
            crate::hex_to_c32(&palette.harmonies.warm_attention),
        ];
        if channel == 9 {
            drum_color
        } else {
            track_palette[track_idx % track_palette.len()]
        }
    }
}

impl MidiViewerState {
    /// Loads and parses a MIDI file into workbench viewer state.
    pub fn from_file(path: PathBuf) -> Result<Self, String> {
        let data = MidiData::parse_file(&path)?;
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("MIDI")
            .to_string();

        let total_dur = data.duration_sec.max(0.5);
        Ok(Self {
            path: Some(path),
            title,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset: 0.0,
            player: None,
            _sink: None,
            wav_cache: None,
            view_start_sec: 0.0,
            view_duration_sec: total_dur,
            user_panned_during_playback: false,
            was_playing_before_drag: false,
            hovered_note: None,
            status_toast: None,
            data,
        })
    }

    #[allow(dead_code)]
    pub fn from_bytes(bytes: &[u8], title: String) -> Result<Self, String> {
        let data = MidiData::parse_bytes(bytes, title.clone())?;
        let total_dur = data.duration_sec.max(0.5);
        Ok(Self {
            path: None,
            title,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset: 0.0,
            player: None,
            _sink: None,
            wav_cache: None,
            view_start_sec: 0.0,
            view_duration_sec: total_dur,
            user_panned_during_playback: false,
            was_playing_before_drag: false,
            hovered_note: None,
            status_toast: None,
            data,
        })
    }

    pub fn zoom(&mut self, factor: f32, anchor_time: f32) {
        let total_dur = self.data.duration_sec.max(0.5);
        let min_dur = 0.5f32.min(total_dur);
        let max_dur = total_dur;

        let old_dur = self.view_duration_sec.clamp(min_dur, max_dur);
        let new_dur = (old_dur * factor).clamp(min_dur, max_dur);

        let anchor = anchor_time.clamp(0.0, total_dur);
        let anchor_ratio = if old_dur > 0.0 {
            ((anchor - self.view_start_sec) / old_dur).clamp(0.0, 1.0)
        } else {
            0.5
        };

        let mut new_start = anchor - anchor_ratio * new_dur;
        if new_start + new_dur > total_dur {
            new_start = total_dur - new_dur;
        }
        if new_start < 0.0 {
            new_start = 0.0;
        }

        self.view_start_sec = new_start;
        self.view_duration_sec = new_dur;
    }

    pub fn pan(&mut self, delta_sec: f32) {
        if delta_sec.abs() > 0.0001 {
            if self.is_playing {
                self.user_panned_during_playback = true;
            }
            let total_dur = self.data.duration_sec.max(0.5);
            let max_start = (total_dur - self.view_duration_sec).max(0.0);
            let new_start = (self.view_start_sec + delta_sec).clamp(0.0, max_start);
            self.view_start_sec = new_start;
        }
    }

    pub fn reset_zoom(&mut self) {
        self.view_start_sec = 0.0;
        self.view_duration_sec = self.data.duration_sec.max(0.5);
        self.user_panned_during_playback = false;
    }

    pub fn play(&mut self) {
        if self.is_playing {
            return;
        }

        self.user_panned_during_playback = false;

        if self.wav_cache.is_none() {
            self.wav_cache = Some(self.data.synthesize_to_wav());
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

        match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                let cursor = Cursor::new(sliced_wav);
                match Decoder::new(cursor) {
                    Ok(source) => {
                        let player = Player::connect_new(sink.mixer());
                        player.append(source);
                        self.player = Some(player);
                        self._sink = Some(sink);
                        self.is_playing = true;
                        self.playback_start_instant = Some(Instant::now());
                        self.playback_start_offset = self.playhead_sec;
                    }
                    Err(e) => {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
                        self.status_toast = Some((format!("Audio decode error: {}", e), now));
                    }
                }
            }
            Err(e) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
                self.status_toast = Some((format!("Audio device error: {}", e), now));
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(player) = &self.player {
            player.pause();
        }
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
        self.is_playing = false;
        self.playhead_sec = 0.0;
        self.playback_start_instant = None;
        self.playback_start_offset = 0.0;
        self.user_panned_during_playback = false;
    }

    pub fn update_playback(&mut self) {
        if self.is_playing {
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                self.playhead_sec = self.playback_start_offset + elapsed;
                if self.playhead_sec >= self.data.duration_sec {
                    self.stop();
                } else {
                    // Auto-follow playhead when zoomed in, UNLESS user actively panned away during playback
                    if !self.user_panned_during_playback && self.view_duration_sec < self.data.duration_sec {
                        let view_end = self.view_start_sec + self.view_duration_sec;
                        if self.playhead_sec > view_end || self.playhead_sec < self.view_start_sec {
                            let new_start = (self.playhead_sec - 0.1 * self.view_duration_sec)
                                .clamp(0.0, (self.data.duration_sec - self.view_duration_sec).max(0.0));
                            self.view_start_sec = new_start;
                        }
                    }
                }
            }
        }
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, palette: &little_clip_core::theme::AppPalette, close: &mut bool) {
        self.update_playback();
        if self.is_playing {
            ui.ctx().request_repaint();
        }

        // 1. Hook up Escape key to immediately exit viewer
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop();
            *close = true;
            return;
        }

        // Handle Space key for Play/Pause
        if ui.input(|i| i.key_pressed(egui::Key::Space)) {
            if self.is_playing {
                self.pause();
            } else {
                self.play();
            }
        }

        // Handle + / - for keyboard zoom
        if ui.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
            self.zoom(0.8, self.playhead_sec);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
            self.zoom(1.25, self.playhead_sec);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        let bg = crate::hex_to_c32(&palette.theme.background);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let f_med = crate::hex_to_c32(&palette.theme.f_med);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let playhead_color = crate::hex_to_c32(&palette.harmonies.complementary);
        let success_color = crate::hex_to_c32(&palette.diff_add);
        let pause_color = crate::hex_to_c32(&palette.harmonies.warm_attention);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                // 1. Top Control Bar
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🎹")
                            .size(20.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(18.0)
                            .color(f_high),
                    );

                    ui.add_space(10.0);
                    let info_text = format!(
                        "{} notes • {:.1}s • {:.0} BPM • {} tracks",
                        self.data.total_notes, self.data.duration_sec, self.data.bpm, self.data.track_count
                    );
                    ui.label(
                        egui::RichText::new(info_text)
                            .small()
                            .color(f_low),
                    );

                    ui.add_space(14.0);
                    // Transport controls
                    if self.is_playing {
                        let pause_btn = egui::Button::new(
                            egui::RichText::new("⏸ Pause (Space)").small().strong().color(crate::hex_to_c32(&palette.theme.f_inv)),
                        )
                        .fill(pause_color)
                        .rounding(Rounding::same(4.0));
                        if ui.add(pause_btn).clicked() {
                            self.pause();
                        }
                    } else {
                        let play_btn = egui::Button::new(
                            egui::RichText::new("▶ Play (Space)").small().strong().color(crate::hex_to_c32(&palette.theme.f_inv)),
                        )
                        .fill(success_color)
                        .rounding(Rounding::same(4.0));
                        if ui.add(play_btn).clicked() {
                            self.play();
                        }
                    }

                    let stop_btn = egui::Button::new(
                        egui::RichText::new("⏹ Stop").small().strong().color(f_high),
                    )
                    .fill(b_med)
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(stop_btn).clicked() {
                        self.stop();
                    }

                    ui.add_space(6.0);
                    let time_text = format!("{:.1}s / {:.1}s", self.playhead_sec, self.data.duration_sec);
                    ui.label(
                        egui::RichText::new(time_text)
                            .monospace()
                            .small()
                            .strong()
                            .color(accent),
                    );

                    ui.add_space(12.0);
                    // Zoom Toolbar Controls
                    let zoom_out_btn = egui::Button::new(
                        egui::RichText::new("🔍 -").small().strong().color(f_high),
                    )
                    .fill(b_med)
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(zoom_out_btn).on_hover_text("Zoom Out (- / Wheel Down)").clicked() {
                        self.zoom(1.25, self.playhead_sec);
                    }

                    let zoom_in_btn = egui::Button::new(
                        egui::RichText::new("🔍 +").small().strong().color(f_high),
                    )
                    .fill(b_med)
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(zoom_in_btn).on_hover_text("Zoom In (+ / Wheel Up)").clicked() {
                        self.zoom(0.8, self.playhead_sec);
                    }

                    let fit_btn = egui::Button::new(
                        egui::RichText::new("Fit").small().color(f_med),
                    )
                    .fill(b_med)
                    .stroke(Stroke::new(1.0, b_low))
                    .rounding(Rounding::same(4.0));
                    if ui.add(fit_btn).on_hover_text("Reset Zoom / Fit Entire Track").clicked() {
                        self.reset_zoom();
                    }

                    let zoom_pct = ((self.data.duration_sec / self.view_duration_sec.max(0.01)) * 100.0).round() as u32;
                    ui.label(
                        egui::RichText::new(format!("{}%", zoom_pct))
                            .monospace()
                            .small()
                            .color(f_low),
                    );

                    // Right aligned Close button & toast
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui::RichText::new("✕ Close (Esc)")
                                .strong()
                                .color(crate::hex_to_c32(&palette.undo_border)),
                        )
                        .fill(crate::hex_to_c32(&palette.undo_card_bg))
                        .stroke(Stroke::new(1.0, crate::hex_to_c32(&palette.undo_border)))
                        .rounding(Rounding::same(6.0));
                        if ui.add(close_btn).clicked() {
                            self.stop();
                            *close = true;
                        }

                        if let Some((msg, ts)) = &self.status_toast {
                            if now - ts < 4.0 {
                                ui.label(
                                    egui::RichText::new(msg)
                                        .small()
                                        .color(success_color),
                                );
                            }
                        }
                    });
                });

                // 2. Track Legend (if multi-track)
                if self.data.track_count > 1 {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Tracks:")
                                .small()
                                .strong()
                                .color(f_low),
                        );
                        for t_idx in 0..self.data.track_count {
                            let note_count = self.data.notes.iter().filter(|n| n.track == t_idx).count();
                            if note_count == 0 {
                                continue;
                            }
                            let first_note = self.data.notes.iter().find(|n| n.track == t_idx);
                            let channel = first_note.map(|n| n.channel).unwrap_or(0);
                            let t_color = get_track_color(t_idx, channel, self.data.track_count, palette);
                            let t_name = self.data.track_names.get(t_idx).cloned().unwrap_or_else(|| format!("Track {}", t_idx + 1));

                            ui.label(
                                egui::RichText::new(format!("● {} ({})", t_name, note_count))
                                    .small()
                                    .color(t_color),
                            );
                            ui.add_space(6.0);
                        }
                    });
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                // 3. Interactive Piano Roll Area
                let available_size = ui.available_size();
                let (canvas_rect, response) = ui.allocate_exact_size(available_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(canvas_rect);

                // Background
                painter.rect_filled(canvas_rect, Rounding::ZERO, b_low);

                let kb_width = 48.0f32;
                let grid_rect = Rect::from_min_max(
                    Pos2::new(canvas_rect.min.x + kb_width, canvas_rect.min.y),
                    canvas_rect.max,
                );

                let min_k = (self.data.min_key.min(self.data.max_key) as f32).max(12.0) - 2.0;
                let max_k = (self.data.max_key.max(self.data.min_key + 1) as f32).min(115.0) + 2.0;
                let key_range = (max_k - min_k + 1.0).max(12.0);

                let row_height = (grid_rect.height() / key_range).clamp(8.0, 32.0);

                // Draw piano keys and horizontal lane dividers
                const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
                let is_black_key = |k: u8| -> bool {
                    let m = k % 12;
                    m == 1 || m == 3 || m == 6 || m == 8 || m == 10
                };

                for k_int in (min_k as u8)..=(max_k as u8) {
                    let norm_y = 1.0 - (k_int as f32 - min_k) / key_range;
                    let y_top = grid_rect.min.y + norm_y * (grid_rect.height() - row_height);
                    let lane_rect = Rect::from_min_size(Pos2::new(grid_rect.min.x, y_top), Vec2::new(grid_rect.width(), row_height));

                    let black = is_black_key(k_int);
                    let lane_bg = if black {
                        b_low
                    } else if (k_int % 12) == 0 {
                        b_med // C note emphasis
                    } else {
                        bg
                    };
                    painter.rect_filled(lane_rect, Rounding::ZERO, lane_bg);
                    painter.line_segment([lane_rect.left_bottom(), lane_rect.right_bottom()], Stroke::new(0.5, b_med));

                    // Left keyboard key
                    let key_rect = Rect::from_min_size(Pos2::new(canvas_rect.min.x, y_top), Vec2::new(kb_width, row_height));
                    let key_color = if black {
                        b_med
                    } else {
                        f_high
                    };
                    let key_text_color = if black {
                        f_low
                    } else {
                        bg
                    };
                    painter.rect_filled(key_rect, Rounding::same(2.0), key_color);
                    painter.rect_stroke(key_rect, Rounding::same(2.0), Stroke::new(0.5, b_low));

                    let note_name = format!("{}{}", NOTE_NAMES[(k_int % 12) as usize], (k_int as i32 / 12) - 1);
                    if row_height >= 12.0 || (k_int % 12) == 0 {
                        painter.text(
                            Pos2::new(key_rect.min.x + 4.0, key_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            note_name,
                            egui::FontId::monospace(9.0),
                            key_text_color,
                        );
                    }
                }

                // Mouse wheel zoom & pan interactions
                let hover_pos = response.hover_pos();
                let is_hovered = response.hovered();

                if is_hovered {
                    let scroll_y = ui.input(|i| i.raw_scroll_delta.y);
                    let scroll_x = ui.input(|i| i.raw_scroll_delta.x);

                    if scroll_y != 0.0 {
                        let anchor_time = if let Some(p) = hover_pos {
                            let rel_x = ((p.x - grid_rect.min.x) / grid_rect.width()).clamp(0.0, 1.0);
                            self.view_start_sec + rel_x * self.view_duration_sec
                        } else {
                            self.playhead_sec
                        };
                        let factor = if scroll_y > 0.0 { 0.82 } else { 1.22 };
                        self.zoom(factor, anchor_time);
                    } else if scroll_x != 0.0 {
                        let pan_sec = -(scroll_x / grid_rect.width()) * self.view_duration_sec;
                        self.pan(pan_sec);
                    }
                }

                // Middle or secondary drag to pan horizontally
                if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
                    let delta = response.drag_delta();
                    let pan_sec = -(delta.x / grid_rect.width()) * self.view_duration_sec;
                    self.pan(pan_sec);
                }

                // Left click / drag to seek playhead smoothly
                if response.drag_started_by(egui::PointerButton::Primary) {
                    if self.is_playing {
                        self.was_playing_before_drag = true;
                        self.pause();
                    }
                }

                if response.dragged_by(egui::PointerButton::Primary) {
                    if let Some(pos) = hover_pos {
                        if pos.x >= grid_rect.min.x {
                            let click_norm = ((pos.x - grid_rect.min.x) / grid_rect.width()).clamp(0.0, 1.0);
                            let target_sec = self.view_start_sec + click_norm * self.view_duration_sec;
                            self.playhead_sec = target_sec.clamp(0.0, self.data.duration_sec);
                            self.user_panned_during_playback = false;
                        }
                    }
                }

                if response.drag_stopped() {
                    if self.was_playing_before_drag {
                        self.was_playing_before_drag = false;
                        self.play();
                    }
                }

                if response.clicked() && !response.dragged_by(egui::PointerButton::Secondary) {
                    if let Some(pos) = hover_pos {
                        if pos.x >= grid_rect.min.x {
                            let click_norm = ((pos.x - grid_rect.min.x) / grid_rect.width()).clamp(0.0, 1.0);
                            let target_sec = self.view_start_sec + click_norm * self.view_duration_sec;
                            let was_playing = self.is_playing;
                            if was_playing {
                                self.stop();
                            }
                            self.playhead_sec = target_sec.clamp(0.0, self.data.duration_sec);
                            self.user_panned_during_playback = false;
                            if was_playing {
                                self.play();
                            }
                        }
                    }
                }

                // Draw Beat / Time Vertical Grid lines based on zoom
                let view_dur = self.view_duration_sec;
                let view_start = self.view_start_sec;
                let view_end = view_start + view_dur;

                let step = if view_dur > 60.0 {
                    10.0
                } else if view_dur > 25.0 {
                    5.0
                } else if view_dur > 10.0 {
                    2.0
                } else if view_dur > 4.0 {
                    1.0
                } else if view_dur > 1.5 {
                    0.5
                } else {
                    0.2
                };

                let first_step = (view_start / step).floor() * step;
                let last_step = (view_end / step).ceil() * step;

                let mut s = first_step;
                while s <= last_step {
                    if s >= view_start && s <= view_end {
                        let norm_x = (s - view_start) / view_dur;
                        let x = grid_rect.min.x + norm_x * grid_rect.width();
                        let is_major = (s / (step * 5.0)).fract().abs() < 0.001 || ((s / step).round() as i64 % 5 == 0);
                        let stroke = if is_major {
                            Stroke::new(1.0, f_low)
                        } else {
                            Stroke::new(0.5, b_med)
                        };
                        painter.line_segment([Pos2::new(x, grid_rect.min.y), Pos2::new(x, grid_rect.max.y)], stroke);

                        let label_text = if step < 1.0 {
                            format!("{:.1}s", s)
                        } else {
                            format!("{:.0}s", s)
                        };
                        painter.text(
                            Pos2::new(x + 2.0, grid_rect.min.y + 4.0),
                            egui::Align2::LEFT_TOP,
                            label_text,
                            egui::FontId::monospace(9.0),
                            f_low,
                        );
                    }
                    s += step;
                }

                // Draw Notes (colored per track)
                let mut hovered: Option<MidiNote> = None;
                let mouse_pos = response.hover_pos();

                for note in &self.data.notes {
                    let note_end = note.start_sec + note.duration_sec;
                    if note_end < view_start || note.start_sec > view_end {
                        continue;
                    }

                    let norm_start = (note.start_sec - view_start) / view_dur;
                    let norm_dur = note.duration_sec / view_dur;
                    let norm_y = 1.0 - (note.key as f32 - min_k) / key_range;

                    let nx = grid_rect.min.x + norm_start * grid_rect.width();
                    let nw = (norm_dur * grid_rect.width()).max(3.0);
                    let ny = grid_rect.min.y + norm_y * (grid_rect.height() - row_height) + 1.0;
                    let nh = (row_height - 2.0).max(3.0);

                    let note_min_x = nx.max(grid_rect.min.x);
                    let note_max_x = (nx + nw).min(grid_rect.max.x);
                    if note_max_x <= note_min_x {
                        continue;
                    }

                    let note_rect = Rect::from_min_max(Pos2::new(note_min_x, ny), Pos2::new(note_max_x, ny + nh));

                    let is_note_hovered = mouse_pos.map(|p| note_rect.contains(p)).unwrap_or(false);
                    if is_note_hovered {
                        hovered = Some(note.clone());
                    }

                    // Colored per track using palette harmonies
                    let base_track_color = get_track_color(note.track, note.channel, self.data.track_count, palette);
                    let fill_color = if is_note_hovered {
                        f_high
                    } else {
                        let vel_factor = (note.velocity as f32 / 127.0).clamp(0.5, 1.0);
                        Color32::from_rgb(
                            (base_track_color.r() as f32 * vel_factor) as u8,
                            (base_track_color.g() as f32 * vel_factor) as u8,
                            (base_track_color.b() as f32 * vel_factor) as u8,
                        )
                    };

                    painter.rect_filled(note_rect, Rounding::same(2.0), fill_color);
                    painter.rect_stroke(note_rect, Rounding::same(2.0), Stroke::new(0.5, b_low));
                }

                self.hovered_note = hovered;

                // Draw Playhead line if in visible window
                if self.playhead_sec >= view_start && self.playhead_sec <= view_end {
                    let playhead_norm = (self.playhead_sec - view_start) / view_dur;
                    let px = grid_rect.min.x + playhead_norm * grid_rect.width();
                    painter.line_segment(
                        [Pos2::new(px, grid_rect.min.y), Pos2::new(px, grid_rect.max.y)],
                        Stroke::new(2.0, playhead_color),
                    );

                    // Playhead triangle handle
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            Pos2::new(px - 6.0, grid_rect.min.y),
                            Pos2::new(px + 6.0, grid_rect.min.y),
                            Pos2::new(px, grid_rect.min.y + 10.0),
                        ],
                        playhead_color,
                        Stroke::NONE,
                    ));
                } else if self.is_playing {
                    // Draw edge indicators when playhead is offscreen
                    if self.playhead_sec < view_start {
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                Pos2::new(grid_rect.min.x + 8.0, grid_rect.min.y + 4.0),
                                Pos2::new(grid_rect.min.x + 2.0, grid_rect.min.y + 10.0),
                                Pos2::new(grid_rect.min.x + 8.0, grid_rect.min.y + 16.0),
                            ],
                            playhead_color,
                            Stroke::NONE,
                        ));
                    } else if self.playhead_sec > view_end {
                        painter.add(egui::Shape::convex_polygon(
                            vec![
                                Pos2::new(grid_rect.max.x - 8.0, grid_rect.min.y + 4.0),
                                Pos2::new(grid_rect.max.x - 2.0, grid_rect.min.y + 10.0),
                                Pos2::new(grid_rect.max.x - 8.0, grid_rect.min.y + 16.0),
                            ],
                            playhead_color,
                            Stroke::NONE,
                        ));
                    }
                }

                // Hover tooltip with track and instrument info
                if let Some(note) = &self.hovered_note {
                    let track_name = self.data.track_names.get(note.track).cloned().unwrap_or_else(|| format!("Track {}", note.track + 1));
                    let channel_str = if note.channel == 9 {
                        "Ch 10 (Percussion)".to_string()
                    } else {
                        format!("Ch {}", note.channel + 1)
                    };
                    let tip = format!(
                        "Track: {} ({})\nPitch: {} (MIDI {})\nStart: {:.2}s\nDuration: {:.2}s\nVelocity: {}\nFreq: {:.1} Hz",
                        track_name,
                        channel_str,
                        note.full_name(),
                        note.key,
                        note.start_sec,
                        note.duration_sec,
                        note.velocity,
                        note.frequency()
                    );
                    response.on_hover_text(tip);
                }
            });
    }
}
