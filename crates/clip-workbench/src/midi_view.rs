//! Piano roll visualization and MIDI playback workbench view.
//!
//! Renders multi-track MIDI piano rolls, measure grid lines, note inspection,
//! and live audio synthesis playback via Rodio.

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::{self, StatusToast, ZoomAction};
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::HandSide;
use little_clip_core::ipc::IpcRequest;
use little_clip_core::presentation::midi::{MidiData, MidiNote};
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
    #[allow(dead_code)]
    pub was_playing_before_drag: bool,
    pub hovered_note: Option<MidiNote>,
    pub status_toast: StatusToast,
    pub selected_region: Option<(f32, f32)>,
    pub solo_track: Option<usize>,
    pub drag_start_time: Option<f32>,
    pub is_dragging_selection: bool,
    pub play_stop_time: Option<f32>,
}

/// Computes a distinct palette color for notes in a given MIDI track and channel using theme harmonies.
pub fn get_track_color(
    track_idx: usize,
    channel: u8,
    total_tracks: usize,
    palette: &little_clip_core::theme::AppPalette,
) -> Color32 {
    let (r, g, b) = palette.midi_track_color(track_idx, channel, total_tracks);
    Color32::from_rgb(r, g, b)
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
            status_toast: StatusToast::new(),
            selected_region: None,
            solo_track: None,
            drag_start_time: None,
            is_dragging_selection: false,
            play_stop_time: None,
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
            status_toast: StatusToast::new(),
            selected_region: None,
            solo_track: None,
            drag_start_time: None,
            is_dragging_selection: false,
            play_stop_time: None,
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
            self.wav_cache = Some(self.data.synthesize_tracks_to_wav(self.solo_track));
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
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64();
                        self.status_toast
                            .set(format!("Audio decode error: {}", e), now);
                    }
                }
            }
            Err(e) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64();
                self.status_toast
                    .set(format!("Audio device error: {}", e), now);
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

    pub fn copy_selection(&mut self, now: f64) -> bool {
        let (s, e, is_slice) = match self.selected_region {
            Some((a, b)) if (b - a).abs() > 0.05 => (a.min(b), a.max(b), true),
            _ => (0.0, self.data.duration_sec.max(0.1), false),
        };

        let sliced_data = self.data.slice(s, e, self.solo_track);
        let midi_bytes = match sliced_data.to_midi_bytes() {
            Ok(b) => b,
            Err(err) => {
                self.status_toast
                    .set(format!("Failed to export MIDI: {err}"), now);
                return false;
            }
        };

        let slice_dir = if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(rt)
                .join("little-screen-clip")
                .join("midi_slices")
        } else {
            PathBuf::from("/tmp/little-screen-clip/midi_slices")
        };

        if let Err(err) = std::fs::create_dir_all(&slice_dir) {
            tracing::warn!(
                "Failed to create midi slice directory {}: {err}",
                slice_dir.display()
            );
            self.status_toast
                .set(format!("Failed to create slice directory: {err}"), now);
            return false;
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);

        let track_suffix = if let Some(st) = self.solo_track {
            let name = self
                .data
                .track_names
                .get(st)
                .map(|s| s.replace(|c: char| !c.is_alphanumeric() && c != '_', "_"))
                .unwrap_or_else(|| format!("track_{}", st + 1));
            format!("_{name}")
        } else {
            String::new()
        };

        let filename = if is_slice {
            format!(
                "midi_slice_{}_{:.1}s-{:.1}s{}.mid",
                timestamp, s, e, track_suffix
            )
        } else {
            format!("midi_export_{}{}.mid", timestamp, track_suffix)
        };

        let slice_path = slice_dir.join(filename);
        if let Err(err) = std::fs::write(&slice_path, &midi_bytes) {
            self.status_toast
                .set(format!("Failed to write MIDI file: {err}"), now);
            return false;
        }

        let req = IpcRequest::PushFile {
            path: slice_path.clone(),
            hand: HandSide::Left,
        };

        let ipc_sent = crate::send_ipc_request(&req).is_ok();
        if !ipc_sent {
            if let Err(error) = little_clip_core::copy_file_uris_to_system_clipboard(
                std::slice::from_ref(&slice_path),
            ) {
                tracing::debug!(%error, "failed to copy MIDI slice to system clipboard");
            }
        }

        let solo_info = if let Some(st) = self.solo_track {
            let name = self
                .data
                .track_names
                .get(st)
                .map(|s| s.as_str())
                .unwrap_or("track");
            format!(" (solo '{name}')")
        } else {
            String::new()
        };

        let desc = if is_slice {
            format!(
                "Copied {:.1}s MIDI slice ({} notes{}) to Clipboard & Left Hand!",
                e - s,
                sliced_data.total_notes,
                solo_info
            )
        } else {
            format!(
                "Copied full MIDI ({} notes{}) to Clipboard & Left Hand!",
                sliced_data.total_notes, solo_info
            )
        };

        self.status_toast.set(desc, now);
        true
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

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::theme::AppPalette,
        close: &mut bool,
    ) {
        self.update_playback();
        if self.is_playing {
            ui.ctx().request_repaint();
        }

        // 1. Hook up Escape key: clear selection first, or close if no selection
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.selected_region.is_some() {
                self.selected_region = None;
                self.play_stop_time = None;
            } else {
                self.stop();
                *close = true;
                return;
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        // Handle Space key for Play/Pause
        if ui.input(|i| i.key_pressed(egui::Key::Space)) {
            if self.is_playing {
                self.pause();
            } else {
                self.play_selection();
            }
        }

        // Handle copy shortcut (Ctrl+C / Cmd+C / egui::Event::Copy)
        let copy_requested = ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || ((i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::C))
        });
        if copy_requested {
            self.copy_selection(now);
        }

        // Handle + / - for keyboard zoom
        if ui.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
            self.zoom(0.8, self.playhead_sec);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
            self.zoom(1.25, self.playhead_sec);
        }

        let bg = crate::hex_to_c32(&palette.theme.background);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let _f_med = crate::hex_to_c32(&palette.theme.f_med);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let playhead_color = crate::hex_to_c32(&palette.harmonies.complementary);
        let success_color = crate::hex_to_c32(&palette.diff_add);
        let pause_color = crate::hex_to_c32(&palette.harmonies.warm_attention);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let total_notes = self.data.total_notes;
                let duration_sec = self.data.duration_sec;
                let bpm = self.data.bpm;
                let track_count = self.data.track_count;
                let playhead_sec = self.playhead_sec;
                let is_playing = self.is_playing;
                let zoom_pct = ((duration_sec / self.view_duration_sec.max(0.01)) * 100.0).round() as u32;

                let mut play_clicked = false;
                let mut pause_clicked = false;
                let mut stop_clicked = false;
                let mut copy_clicked = false;
                let mut clear_sel_clicked = false;
                let mut clear_solo_clicked = false;
                let mut toggled_solo_track = None;
                let mut zoom_action = None;

                let info_text = format!(
                    "{} notes • {:.1}s • {:.0} BPM • {} tracks",
                    total_notes, duration_sec, bpm, track_count
                );

                let mut shell = StudioShell::new(&self.title, Icon::Midi, palette, close)
                    .with_badge(info_text, None)
                    .with_escape_hatch(EscapeHatch::primary(
                        "MuseScore",
                        "mscore",
                        "Open MIDI composition in MuseScore",
                    ).with_icon(Icon::Midi))
                    .with_footer_hints("Click grid: Seek | Drag: Select region | [Ctrl+C]: Copy slice | Space: Play | Esc: Clear/Close");

                if let Some((s, e)) = self.selected_region {
                    let min_s = s.min(e);
                    let max_s = s.max(e);
                    let solo_suffix = if let Some(st) = self.solo_track {
                        let name = self.data.track_names.get(st).map(|s| s.as_str()).unwrap_or("Track");
                        format!(" • Solo: {name}")
                    } else {
                        String::new()
                    };
                    shell = shell.with_footer_readout(format!(
                        "Selection: {:.2}s - {:.2}s ({:.2}s{}) • [Ctrl+C] Copy • [Space] Play • [Esc] Clear",
                        min_s,
                        max_s,
                        max_s - min_s,
                        solo_suffix
                    ));
                } else if let Some(ref note) = self.hovered_note {
                    shell = shell.with_footer_readout(format!(
                        "Note: {} (MIDI {}) • Velocity: {} • Time: {:.2}s",
                        note.full_name(),
                        note.key,
                        note.velocity,
                        note.start_sec
                    ));
                }

                let clicked_hatch = shell.render_header(ui, |_ui| {});
                if let Some(0) = clicked_hatch {
                    if let Some(ref p) = self.path {
                        EscapeHatchLauncher::launch("MuseScore", "mscore", p, &mut self.status_toast, now);
                    } else {
                        self.status_toast.set("No on-disk file path for this MIDI stream", now);
                    }
                }

                ui.add_space(4.0);

                // Zone 2: Transport & Zoom Bar
                egui::Frame::none()
                    .fill(b_low)
                    .stroke(Stroke::new(1.0_f32, b_med))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Transport controls
                            if is_playing {
                                let pause_btn = crate::ui_ext::action_button_icon(Some(Icon::Pause), "Pause [Space]", pause_color, palette);
                                if ui.add(pause_btn).clicked() {
                                    pause_clicked = true;
                                }
                            } else {
                                let play_btn = crate::ui_ext::action_button_icon(Some(Icon::Play), "Play [Space]", success_color, palette);
                                if ui.add(play_btn).clicked() {
                                    play_clicked = true;
                                }
                            }

                            let stop_btn = crate::ui_ext::action_button_icon(Some(Icon::Stop), "Stop", b_med, palette)
                                .min_size(egui::Vec2::new(0.0, 26.0))
                                .stroke(Stroke::new(1.0_f32, b_low));
                            if ui.add(stop_btn).clicked() {
                                stop_clicked = true;
                            }

                            ui.add_space(8.0);
                            let time_text = format!("{:.1}s / {:.1}s", playhead_sec, duration_sec);
                            ui.label(
                                egui::RichText::new(time_text)
                                    .monospace()
                                    .small()
                                    .strong()
                                    .color(accent),
                            );

                            ui.add_space(8.0);

                            // Copy selection button
                            let has_sel = match self.selected_region {
                                Some((s, e)) => (e - s).abs() > 0.05,
                                None => false,
                            };
                            let copy_label = match (has_sel, self.solo_track) {
                                (true, Some(st)) => {
                                    let tname = self.data.track_names.get(st).map(|s| s.as_str()).unwrap_or("Track");
                                    format!("Copy Solo Slice ({tname}) [Ctrl+C]")
                                }
                                (true, None) => "Copy Slice [Ctrl+C]".to_string(),
                                (false, Some(st)) => {
                                    let tname = self.data.track_names.get(st).map(|s| s.as_str()).unwrap_or("Track");
                                    format!("Copy Solo ({tname}) [Ctrl+C]")
                                }
                                (false, None) => "Copy MIDI [Ctrl+C]".to_string(),
                            };
                            let copy_btn = crate::ui_ext::secondary_button_icon(Some(Icon::Copy), copy_label, b_med, palette)
                                .min_size(egui::Vec2::new(0.0, 26.0));
                            if ui.add(copy_btn).on_hover_text("Export selected region (or full score) as .mid to Left Hand & clipboard").clicked() {
                                copy_clicked = true;
                            }

                            if let Some((s, e)) = self.selected_region {
                                if (e - s).abs() > 0.05 {
                                    ui.add_space(4.0);
                                    let min_s = s.min(e);
                                    let max_s = s.max(e);
                                    ui.label(
                                        egui::RichText::new(format!("Sel: {:.2}s - {:.2}s", min_s, max_s))
                                            .small()
                                            .strong()
                                            .color(accent),
                                    );
                                    let x_btn = egui::Button::new(egui::RichText::new("✕").small())
                                        .min_size(egui::Vec2::new(18.0, 18.0));
                                    if ui.add(x_btn).on_hover_text("Clear selection [Esc]").clicked() {
                                        clear_sel_clicked = true;
                                    }
                                }
                            }

                            if let Some(st) = self.solo_track {
                                ui.add_space(4.0);
                                let st_name = self.data.track_names.get(st).cloned().unwrap_or_else(|| format!("Track {}", st + 1));
                                ui.label(
                                    egui::RichText::new(format!("Solo: {st_name}"))
                                        .small()
                                        .strong()
                                        .color(pause_color),
                                );
                                let unsolo_btn = egui::Button::new(egui::RichText::new("Unsolo").small())
                                    .min_size(egui::Vec2::new(0.0, 20.0));
                                if ui.add(unsolo_btn).on_hover_text("Unsolo and preview/copy all tracks").clicked() {
                                    clear_solo_clicked = true;
                                }
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                zoom_action = ui_ext::zoom_control_bar(ui, zoom_pct, palette);
                            });
                        });
                    });

                if shell.is_closed() {
                    self.stop();
                }
                if play_clicked {
                    self.play_selection();
                }
                if pause_clicked {
                    self.pause();
                }
                if stop_clicked {
                    self.stop();
                }
                if copy_clicked {
                    self.copy_selection(now);
                }
                if clear_sel_clicked {
                    self.selected_region = None;
                    self.play_stop_time = None;
                }
                if let Some(action) = zoom_action {
                    match action {
                        ZoomAction::ZoomIn => self.zoom(0.8, self.playhead_sec),
                        ZoomAction::ZoomOut => self.zoom(1.25, self.playhead_sec),
                        ZoomAction::Fit => self.reset_zoom(),
                    }
                }

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
                        if self.solo_track.is_some() {
                            let all_btn = egui::Button::new(egui::RichText::new("All Tracks").small().strong())
                                .fill(b_med);
                            if ui.add(all_btn).on_hover_text("Clear solo and show/copy all tracks").clicked() {
                                clear_solo_clicked = true;
                            }
                            ui.add_space(4.0);
                        }
                        for t_idx in 0..self.data.track_count {
                            let note_count = self.data.notes.iter().filter(|n| n.track == t_idx).count();
                            if note_count == 0 {
                                continue;
                            }
                            let first_note = self.data.notes.iter().find(|n| n.track == t_idx);
                            let channel = first_note.map(|n| n.channel).unwrap_or(0);
                            let t_color = get_track_color(t_idx, channel, self.data.track_count, palette);
                            let t_name = self.data.track_names.get(t_idx).cloned().unwrap_or_else(|| format!("Track {}", t_idx + 1));

                            let is_solo = self.solo_track == Some(t_idx);
                            let solo_fill = if is_solo {
                                pause_color
                            } else {
                                b_med
                            };
                            let solo_text_col = if is_solo {
                                bg
                            } else {
                                f_high
                            };

                            let track_label_col = if is_solo {
                                t_color
                            } else if self.solo_track.is_some() {
                                f_low
                            } else {
                                t_color
                            };

                            let track_label_resp = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!("● {} ({})", t_name, note_count))
                                        .small()
                                        .color(track_label_col),
                                )
                                .sense(egui::Sense::click()),
                            );

                            let solo_btn = egui::Button::new(
                                egui::RichText::new(if is_solo { "SOLO" } else { "Solo" })
                                    .small()
                                    .strong()
                                    .color(solo_text_col),
                            )
                            .fill(solo_fill)
                            .min_size(egui::Vec2::new(0.0, 18.0));

                            let solo_btn_resp = ui.add(solo_btn);

                            if track_label_resp
                                .on_hover_text(format!("Solo {} (isolates in playback and copy-out)", t_name))
                                .clicked()
                                || solo_btn_resp
                                    .on_hover_text(format!("Solo {} (isolates in playback and copy-out)", t_name))
                                    .clicked()
                            {
                                toggled_solo_track = Some(t_idx);
                            }

                            ui.add_space(8.0);
                        }
                    });
                }

                // Process solo actions AFTER track legend buttons have been rendered and evaluated
                if clear_solo_clicked {
                    self.clear_solo();
                }
                if let Some(t_idx) = toggled_solo_track {
                    self.toggle_solo(t_idx);
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);

                // 3. Interactive Piano Roll Area
                let mut available_size = ui.available_size();
                available_size.y = (available_size.y - 32.0).max(100.0);
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
                    painter.line_segment([lane_rect.left_bottom(), lane_rect.right_bottom()], Stroke::new(0.5_f32, b_med));

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
                    painter.rect_stroke(key_rect, Rounding::same(2.0), Stroke::new(0.5_f32, b_low));

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

                // Primary click and drag: selection & seeking
                let x_to_time = |x: f32| -> f32 {
                    let rel = ((x - grid_rect.min.x) / grid_rect.width()).clamp(0.0, 1.0);
                    self.view_start_sec + rel * self.view_duration_sec
                };

                if response.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(pos) = hover_pos {
                        if pos.x >= grid_rect.min.x {
                            let t0 = x_to_time(pos.x);
                            self.drag_start_time = Some(t0);
                            self.is_dragging_selection = true;
                        }
                    }
                }

                if response.dragged_by(egui::PointerButton::Primary) {
                    if self.is_dragging_selection {
                        if let (Some(pos), Some(t0)) = (hover_pos, self.drag_start_time) {
                            let t_cur = x_to_time(pos.x);
                            if (t_cur - t0).abs() > 0.03 {
                                self.selected_region = Some((t0.min(t_cur), t0.max(t_cur)));
                            }
                        }
                    }
                }

                if response.drag_stopped() {
                    if self.is_dragging_selection {
                        self.is_dragging_selection = false;
                        if let Some((s, e)) = self.selected_region {
                            if (e - s).abs() < 0.05 {
                                self.selected_region = None;
                            }
                        }
                    }
                }

                if response.clicked() && !response.dragged_by(egui::PointerButton::Secondary) {
                    if let Some(pos) = hover_pos {
                        if pos.x >= grid_rect.min.x {
                            let target_sec = x_to_time(pos.x);
                            self.selected_region = None;
                            self.play_stop_time = None;
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
                            Stroke::new(1.0_f32, f_low)
                        } else {
                            Stroke::new(0.5_f32, b_med)
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

                // Draw Selection Highlight Overlay
                if let Some((s, e)) = self.selected_region {
                    let sel_s = s.min(e).clamp(0.0, self.data.duration_sec);
                    let sel_e = s.max(e).clamp(0.0, self.data.duration_sec);

                    if sel_e >= view_start && sel_s <= view_end {
                        let norm_s = (sel_s - view_start) / view_dur;
                        let norm_e = (sel_e - view_start) / view_dur;
                        let x_s = grid_rect.min.x + norm_s * grid_rect.width();
                        let x_e = grid_rect.min.x + norm_e * grid_rect.width();
                        let x_min = x_s.max(grid_rect.min.x);
                        let x_max = x_e.min(grid_rect.max.x);

                        if x_max > x_min {
                            let sel_rect = Rect::from_min_max(
                                Pos2::new(x_min, grid_rect.min.y),
                                Pos2::new(x_max, grid_rect.max.y),
                            );
                            painter.rect_filled(
                                sel_rect,
                                Rounding::ZERO,
                                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 45),
                            );
                        }

                        if x_s >= grid_rect.min.x && x_s <= grid_rect.max.x {
                            painter.line_segment(
                                [Pos2::new(x_s, grid_rect.min.y), Pos2::new(x_s, grid_rect.max.y)],
                                Stroke::new(1.5_f32, accent),
                            );
                        }
                        if x_e >= grid_rect.min.x && x_e <= grid_rect.max.x {
                            painter.line_segment(
                                [Pos2::new(x_e, grid_rect.min.y), Pos2::new(x_e, grid_rect.max.y)],
                                Stroke::new(1.5_f32, accent),
                            );
                        }
                    }
                }

                // Draw Notes (colored per track, dimmed if another track is soloed)
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

                    // Colored per track/channel/pitch using palette theme harmonies
                    let fill_color = if is_note_hovered {
                        f_high
                    } else {
                        let (r, g, b) = palette.midi_note_color(
                            note.track,
                            note.channel,
                            self.data.track_count,
                            note.key,
                            note.velocity,
                        );
                        if let Some(st) = self.solo_track {
                            if note.track == st {
                                Color32::from_rgb(r, g, b)
                            } else {
                                Color32::from_rgba_unmultiplied(r / 3, g / 3, b / 3, 50)
                            }
                        } else {
                            Color32::from_rgb(r, g, b)
                        }
                    };

                    painter.rect_filled(note_rect, Rounding::same(2.0), fill_color);
                    painter.rect_stroke(note_rect, Rounding::same(2.0), Stroke::new(0.5_f32, b_low));
                }

                self.hovered_note = hovered;


                // Draw Playhead line if in visible window
                if self.playhead_sec >= view_start && self.playhead_sec <= view_end {
                    let playhead_norm = (self.playhead_sec - view_start) / view_dur;
                    let px = grid_rect.min.x + playhead_norm * grid_rect.width();
                    painter.line_segment(
                        [Pos2::new(px, grid_rect.min.y), Pos2::new(px, grid_rect.max.y)],
                        Stroke::new(2.0_f32, playhead_color),
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

                // 4. Footer
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::theme::{AppPalette, Theme, find_themes_dir};
    use std::collections::HashMap;

    #[test]
    fn test_midi_view_track_colors() {
        let theme = Theme::apollo();
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        let t0 = get_track_color(0, 0, 4, &palette);
        let t1 = get_track_color(1, 0, 4, &palette);
        let drums = get_track_color(2, 9, 4, &palette);

        assert_ne!(t0, t1, "Track 0 and Track 1 should be distinct colors");
        assert_ne!(t0, drums, "Melodic and drum tracks should be distinct");
    }

    #[test]
    fn test_midi_view_note_colors_vibrancy_zenburn() {
        let dir = find_themes_dir();
        let zenburn_path = dir.join("zenburn.svg");
        if !zenburn_path.exists() {
            return;
        }
        let theme = Theme::from_file(&zenburn_path).expect("failed to load zenburn");
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        let note_c = palette.midi_note_color(0, 0, 4, 60, 100);
        let note_d = palette.midi_note_color(1, 0, 4, 62, 100);

        assert_ne!(note_c, note_d);
        // Ensure note_c is not dark or muted (r + g + b should indicate good brightness)
        let brightness = note_c.0 as u32 + note_c.1 as u32 + note_c.2 as u32;
        assert!(
            brightness >= 350,
            "Note brightness should be vibrant in Zenburn, got {}",
            brightness
        );
    }

    #[test]
    fn test_midi_view_single_track_pitch_differentiation() {
        let theme = Theme::apollo();
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        let c4 = palette.midi_note_color(0, 0, 1, 60, 100);
        let g4 = palette.midi_note_color(0, 0, 1, 67, 100);
        assert_ne!(
            c4, g4,
            "Single-track C4 and G4 must have different chromatic hues"
        );
    }

    #[test]
    fn test_midi_viewer_selection_and_solo_copy() {
        let midi = MidiData {
            title: "Test Score".to_string(),
            track_count: 2,
            track_names: vec!["Melody".to_string(), "Bass".to_string()],
            total_notes: 4,
            duration_sec: 5.0,
            bpm: 120.0,
            min_key: 36,
            max_key: 72,
            notes: vec![
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 60,
                    velocity: 90,
                    start_sec: 1.0,
                    duration_sec: 1.0,
                },
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 64,
                    velocity: 90,
                    start_sec: 2.5,
                    duration_sec: 1.0,
                },
                MidiNote {
                    track: 1,
                    channel: 1,
                    program: 32,
                    key: 36,
                    velocity: 95,
                    start_sec: 0.5,
                    duration_sec: 2.0,
                },
                MidiNote {
                    track: 1,
                    channel: 1,
                    program: 32,
                    key: 40,
                    velocity: 95,
                    start_sec: 3.0,
                    duration_sec: 1.5,
                },
            ],
        };

        let mut state = MidiViewerState {
            path: None,
            title: "Test Score".to_string(),
            data: midi,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset: 0.0,
            player: None,
            _sink: None,
            wav_cache: None,
            view_start_sec: 0.0,
            view_duration_sec: 5.0,
            user_panned_during_playback: false,
            was_playing_before_drag: false,
            hovered_note: None,
            status_toast: StatusToast::new(),
            selected_region: None,
            solo_track: None,
            drag_start_time: None,
            is_dragging_selection: false,
            play_stop_time: None,
        };

        // Test solo toggling
        state.toggle_solo(1);
        assert_eq!(state.solo_track, Some(1));
        state.toggle_solo(1);
        assert_eq!(state.solo_track, None);

        state.toggle_solo(0);
        assert_eq!(state.solo_track, Some(0));
        state.clear_solo();
        assert_eq!(state.solo_track, None);

        // Test selection and copying
        state.selected_region = Some((1.0, 3.0));
        state.solo_track = Some(0);

        let copied = state.copy_selection(1.0);
        assert!(copied);
        let msg = state.status_toast.message(1.0).unwrap();
        assert!(msg.contains("Copied 2.0s MIDI slice"));
        assert!(msg.contains("solo 'Melody'"));
    }

    #[test]
    fn test_midi_viewer_render_ui_tracks_and_legend() {
        let midi = MidiData {
            title: "Multi-Track Score".to_string(),
            track_count: 2,
            track_names: vec!["Melody".to_string(), "Harmony".to_string()],
            total_notes: 2,
            duration_sec: 5.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 64,
            notes: vec![
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 60,
                    velocity: 80,
                    start_sec: 0.0,
                    duration_sec: 1.0,
                },
                MidiNote {
                    track: 1,
                    channel: 1,
                    program: 0,
                    key: 64,
                    velocity: 80,
                    start_sec: 1.0,
                    duration_sec: 1.0,
                },
            ],
        };

        let mut state = MidiViewerState {
            path: None,
            title: "Multi-Track Score".to_string(),
            data: midi,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset: 0.0,
            player: None,
            _sink: None,
            wav_cache: None,
            view_start_sec: 0.0,
            view_duration_sec: 5.0,
            user_panned_during_playback: false,
            was_playing_before_drag: false,
            hovered_note: None,
            status_toast: StatusToast::new(),
            selected_region: None,
            solo_track: None,
            drag_start_time: None,
            is_dragging_selection: false,
            play_stop_time: None,
        };
        let palette = little_clip_core::theme::AppPalette::default();
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1000.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut close = false;
                    state.render_ui(ui, &palette, &mut close);
                    assert!(!close);
                });
            },
        );

        // Verify solo toggle and render with solo active
        state.toggle_solo(0);
        assert_eq!(state.solo_track, Some(0));

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1000.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut close = false;
                    state.render_ui(ui, &palette, &mut close);
                    assert!(!close);
                });
            },
        );

        state.clear_solo();
        assert_eq!(state.solo_track, None);
    }
}
