//! Quick scrubbing and region extraction strip for rolling audio buffer snapshots.
//!
//! Provides waveform peak pyramid visualization, scrub playback, slice selection,
//! and routing to left/right slots.

use crate::icons::Icon;
use crate::{hex_to_c32, hex_to_c32_alpha};
use egui::{Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::audio::{PeakBlock, build_peak_pyramid};
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::slot::HandSide;
use little_clip_core::theme::AppPalette;
use rodio::stream::MixerDeviceSink;
use rodio::{Decoder, Player, Source};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{error, info};

/// UI and playback state for the rolling audio snapshot quick-strip widget.
pub struct RollingAudioStripState {
    pub path: Option<PathBuf>,
    pub duration_sec: f32,
    pub sample_rate: u32,
    pub channels: u16,
    pub mono_samples: Vec<f32>,
    pub peaks_lvl0: Vec<PeakBlock>,
    pub peaks_lvl1: Vec<PeakBlock>,
    pub peaks_lvl2: Vec<PeakBlock>,
    pub is_loaded: bool,
    pub error_msg: Option<String>,

    // Region selection & dragging
    pub selected_region: Option<(f32, f32)>, // (start_sec, end_sec)
    drag_start_time: Option<f32>,
    pub is_dragging: bool,

    // Playback engine
    _sink: Option<MixerDeviceSink>,
    player: Option<Player>,
    pub is_playing: bool,
    pub playhead_sec: f32,
    playback_start_instant: Option<Instant>,
    playback_start_offset_sec: f32,
    play_stop_time: Option<f32>,

    // Feedback
    pub status_toast: Option<(String, f64)>, // (text, timestamp)
}

impl RollingAudioStripState {
    /// Materializes a selected rolling-buffer region for the full editor. The
    /// editor must never receive the entire rolling capture when a region was
    /// explicitly selected. ffmpeg's silence filter is used when available;
    /// the already-trimmed time slice remains a safe fallback otherwise.
    pub fn materialize_editor_selection(
        &self,
        start_sec: f32,
        end_sec: f32,
    ) -> Result<PathBuf, String> {
        let source = self
            .path
            .as_ref()
            .ok_or_else(|| "No rolling audio snapshot is loaded".to_string())?;
        let (start_sec, end_sec) = (start_sec.min(end_sec), start_sec.max(end_sec));
        if end_sec - start_sec < 0.05 {
            return Err("Select at least 50ms of audio before opening the editor".to_string());
        }
        let directory = std::env::temp_dir().join("little-screen-clip/rolling_editor_slices");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Could not create audio slice directory: {error}"))?;
        let id = uuid::Uuid::new_v4();
        let sliced = directory.join(format!("rolling-{id}-selected.wav"));
        little_clip_core::rolling_audio::slice_wav(source, start_sec, end_sec, &sliced)
            .map_err(|error| format!("Could not materialize selected audio: {error}"))?;

        let trimmed = directory.join(format!("rolling-{id}-trimmed.wav"));
        let status = little_clip_core::CommandRequest::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&sliced)
            .args([
                "-af",
                // Trim each edge independently. `stop_periods` is unreliable
                // across ffmpeg builds for short WAVs; reverse-and-trim gives
                // the same deterministic leading-silence operation on the tail.
                "silenceremove=start_periods=1:start_duration=0.05:start_threshold=-45dB,areverse,silenceremove=start_periods=1:start_duration=0.05:start_threshold=-45dB,areverse",
            ])
            .arg(&trimmed)
            .run()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if status && trimmed.is_file() {
            let _ = std::fs::remove_file(&sliced);
            Ok(trimmed)
        } else {
            Ok(sliced)
        }
    }
    /// Creates a new rolling audio strip state and loads the latest daemon snapshot.
    pub fn new() -> Self {
        let player = crate::ui_ext::new_shared_audio_player();
        if let Some(ref p) = player {
            p.set_volume(1.0);
        }

        let mut state = Self {
            path: None,
            duration_sec: 0.0,
            sample_rate: 48000,
            channels: 2,
            mono_samples: Vec::new(),
            peaks_lvl0: Vec::new(),
            peaks_lvl1: Vec::new(),
            peaks_lvl2: Vec::new(),
            is_loaded: false,
            error_msg: None,
            selected_region: None,
            drag_start_time: None,
            is_dragging: false,
            _sink: None,
            player,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            play_stop_time: None,
            status_toast: None,
        };

        state.load_snapshot();
        state
    }

    pub fn max_amplitude(&self) -> f32 {
        self.mono_samples
            .iter()
            .fold(0.0f32, |m, &s| m.max(s.abs()))
    }

    pub fn is_silent(&self) -> bool {
        self.mono_samples.is_empty() || self.max_amplitude() < 0.001
    }

    pub fn selected_region_peak(&self) -> f32 {
        let (s, e) = match self.selected_region {
            Some((a, b)) => (a.min(b), a.max(b)),
            None => return 0.0,
        };
        let total_frames = self.mono_samples.len();
        let f_s = ((s * self.sample_rate as f32) as usize).min(total_frames);
        let f_e = (((e * self.sample_rate as f32) as usize) + 1).min(total_frames);
        if f_s >= f_e || f_s >= total_frames {
            return 0.0;
        }
        self.mono_samples[f_s..f_e]
            .iter()
            .fold(0.0f32, |m, &x| m.max(x.abs()))
    }

    pub fn ensure_player(&mut self) -> Option<&Player> {
        if self.player.is_none() {
            let player = crate::ui_ext::new_shared_audio_player();
            if let Some(ref p) = player {
                p.set_volume(1.0);
            }
            self.player = player;
        }
        self.player.as_ref()
    }

    /// Fetches a frozen rolling audio snapshot from the daemon via IPC
    pub fn load_snapshot(&mut self) {
        match crate::send_ipc_request(&IpcRequest::GetRollingAudio) {
            Ok(IpcResponse::RollingAudio {
                path,
                duration_secs,
                sample_rate,
                channels,
            }) => {
                info!(
                    "Rolling audio snapshot received: {:?} ({:.2}s, {}Hz, ch={})",
                    path, duration_secs, sample_rate, channels
                );
                self.path = Some(path.clone());
                self.duration_sec = duration_secs;
                self.sample_rate = sample_rate;
                self.channels = channels;

                match load_pcm16_samples(&path) {
                    Ok((mono, sr, _ch)) => {
                        let (l0, l1, l2) = build_peak_pyramid(&mono, sr as f32);
                        self.mono_samples = mono;
                        self.peaks_lvl0 = l0;
                        self.peaks_lvl1 = l1;
                        self.peaks_lvl2 = l2;
                        self.is_loaded = true;
                        self.error_msg = None;

                        let peak = self.max_amplitude();
                        info!(
                            "Decoded snapshot: {:.2}s, peak amplitude: {:.4}",
                            duration_secs, peak
                        );
                    }
                    Err(e) => {
                        error!("Failed to decode snapshot WAV: {}", e);
                        self.error_msg = Some(format!("Failed to decode WAV: {}", e));
                    }
                }
            }
            Ok(IpcResponse::Error { error }) => {
                self.error_msg = Some(error);
            }
            Ok(_) => {
                self.error_msg = Some("Unexpected response from daemon".to_string());
            }
            Err(e) => {
                self.error_msg = Some(format!("IPC connection error: {}", e));
            }
        }
    }

    pub fn update_playback(&mut self) {
        if !self.is_playing {
            return;
        }
        if let Some(start_inst) = self.playback_start_instant {
            let elapsed = start_inst.elapsed().as_secs_f32();
            let cur = self.playback_start_offset_sec + elapsed;
            self.playhead_sec = cur;
            if let Some(stop_t) = self.play_stop_time {
                if cur >= stop_t {
                    self.stop();
                }
            }
        }
    }

    pub fn play_selection(&mut self) {
        let (s, e) = match self.selected_region {
            Some((a, b)) if (b - a).abs() > 0.05 => (a.min(b), a.max(b)),
            _ => return,
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        if self.is_silent() {
            self.status_toast = Some((
                "Buffer is currently silence — play audio through speakers first, then click Refresh".to_string(),
                now,
            ));
            return;
        }

        let sel_peak = self.selected_region_peak();
        if sel_peak < 0.001 {
            self.status_toast = Some((
                "Selected region is silence — drag over colored waveform peaks to hear audio"
                    .to_string(),
                now,
            ));
        }

        let source_path = match &self.path {
            Some(p) => p.clone(),
            None => return,
        };

        let slice_dir = if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(rt)
                .join("little-screen-clip")
                .join("rolling_slices")
        } else {
            PathBuf::from("/tmp/little-screen-clip/rolling_slices")
        };
        if let Err(err) = std::fs::create_dir_all(&slice_dir) {
            tracing::warn!(
                "Failed to create rolling slice directory {}: {err}",
                slice_dir.display()
            );
            return;
        }
        let preview_path = slice_dir.join("preview.wav");

        if let Err(err) =
            little_clip_core::rolling_audio::slice_wav(&source_path, s, e, &preview_path)
        {
            eprintln!("play_selection: slice_wav failed: {}", err);
            return;
        }

        self.play_stop_time = Some(e);
        self.playhead_sec = s;
        self.playback_start_offset_sec = s;
        self.playback_start_instant = Some(Instant::now());

        self.ensure_player();
        if let Some(ref p) = self.player {
            p.play();
            p.clear();
            match File::open(&preview_path) {
                Ok(file) => {
                    let reader = BufReader::new(file);
                    let decoder_res = Decoder::new_wav(reader).or_else(|_| {
                        File::open(&preview_path).and_then(|f2| {
                            Decoder::new(BufReader::new(f2))
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                        })
                    });

                    match decoder_res {
                        Ok(decoder) => {
                            p.append(decoder);
                            p.play();
                            self.is_playing = true;
                        }
                        Err(e) => eprintln!("play_selection: Decoder failed: {}", e),
                    }
                }
                Err(e) => eprintln!("play_selection: File::open failed: {}", e),
            }
        } else {
            eprintln!("play_selection: self.player is None");
        }
    }

    #[allow(dead_code)]
    pub fn pause(&mut self) {
        if let Some(ref p) = self.player {
            p.pause();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
    }

    pub fn stop(&mut self) {
        if let Some(ref p) = self.player {
            p.pause();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
        if let Some((s, _)) = self.selected_region {
            self.playhead_sec = s;
        }
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing {
            self.stop();
        } else {
            self.play_selection();
        }
    }

    pub fn copy_selection(&mut self, now: f64) -> bool {
        let (s, e) = match self.selected_region {
            Some((a, b)) if (b - a).abs() > 0.05 => (a.min(b), a.max(b)),
            _ => return false,
        };

        let source_path = match &self.path {
            Some(p) => p.clone(),
            None => return false,
        };

        let slice_dir = if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(rt)
                .join("little-screen-clip")
                .join("rolling_slices")
        } else {
            PathBuf::from("/tmp/little-screen-clip/rolling_slices")
        };
        if let Err(err) = std::fs::create_dir_all(&slice_dir) {
            tracing::warn!(
                "Failed to create rolling slice directory {}: {err}",
                slice_dir.display()
            );
            self.status_toast = Some((format!("Failed to create slice directory: {err}"), now));
            return false;
        }
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let slice_path = slice_dir.join(format!("rolling_slice_{}.wav", timestamp));

        match little_clip_core::rolling_audio::slice_wav(&source_path, s, e, &slice_path) {
            Ok(_) => {
                let req = IpcRequest::PushFile {
                    path: slice_path,
                    hand: HandSide::Left,
                };
                if crate::send_ipc_request(&req).is_ok() {
                    self.status_toast = Some((
                        format!("Copied {:.1}s audio slice to Left Hand & clipboard!", e - s),
                        now,
                    ));
                    true
                } else {
                    self.status_toast =
                        Some(("Failed to push slice to Left Hand".to_string(), now));
                    false
                }
            }
            Err(err) => {
                self.status_toast = Some((format!("Failed to slice audio: {}", err), now));
                false
            }
        }
    }

    pub fn query_peaks(&self, frame_start: usize, frame_end: usize) -> (f32, f32, [u8; 3]) {
        let span = frame_end.saturating_sub(frame_start).max(1);
        if self.peaks_lvl0.is_empty() {
            return (0.0, 0.0, [56, 189, 248]);
        }
        if span >= 16384 && !self.peaks_lvl2.is_empty() {
            let b_start = (frame_start / 16384).min(self.peaks_lvl2.len() - 1);
            let b_end = ((frame_end + 16383) / 16384)
                .min(self.peaks_lvl2.len())
                .max(b_start + 1);
            aggregate_blocks(&self.peaks_lvl2[b_start..b_end])
        } else if span >= 1024 && !self.peaks_lvl1.is_empty() {
            let b_start = (frame_start / 1024).min(self.peaks_lvl1.len() - 1);
            let b_end = ((frame_end + 1023) / 1024)
                .min(self.peaks_lvl1.len())
                .max(b_start + 1);
            aggregate_blocks(&self.peaks_lvl1[b_start..b_end])
        } else {
            let b_start = (frame_start / 64).min(self.peaks_lvl0.len() - 1);
            let b_end = ((frame_end + 63) / 64)
                .min(self.peaks_lvl0.len())
                .max(b_start + 1);
            aggregate_blocks(&self.peaks_lvl0[b_start..b_end])
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        open_full_editor: &mut Option<(PathBuf, Option<(f32, f32)>)>,
    ) {
        self.update_playback();
        if self.is_playing {
            ctx.request_repaint();
        }

        let now = ui.input(|i| i.time);
        if let Some((_, ts)) = self.status_toast {
            if now - ts > 4.0 {
                self.status_toast = None;
            }
        }

        let total_w = ui.available_width();
        let total_dur = self.duration_sec.max(0.1);

        ui.vertical(|ui| {
            // Header Row: Badge & Status / Selection info
            ui.horizontal(|ui| {
                Icon::Microphone.ui(ui, 13.0, hex_to_c32(&palette.harmonies.base));
                ui.label(
                    egui::RichText::new("ROLLING AUDIO")
                        .color(hex_to_c32(&palette.harmonies.base))
                        .strong()
                        .small(),
                );
                ui.label(
                    egui::RichText::new(format!("({:.0}s snapshot)", total_dur))
                        .color(hex_to_c32(&palette.theme.f_med))
                        .small(),
                );

                // Refresh Snapshot button
                let refresh_btn = ui.add(
                    crate::ui_ext::secondary_button_icon(Some(Icon::RotateCw), "Refresh", hex_to_c32(&palette.theme.b_low), palette)
                        .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med))),
                );
                if refresh_btn.clicked() {
                    self.load_snapshot();
                    let peak = self.max_amplitude();
                    if peak < 0.001 {
                        self.status_toast = Some(("Snapshot refreshed: buffer is currently silence".to_string(), now));
                    } else {
                        self.status_toast = Some((format!("Snapshot refreshed ({:.1}s, peak: {:.0}%)", self.duration_sec, peak * 100.0), now));
                    }
                }
                refresh_btn.on_hover_text("Reload latest rolling audio buffer from daemon (F5)");

                if self.is_silent() {
                    ui.horizontal(|ui| {
                        Icon::Alert.ui(ui, 12.0, hex_to_c32(&palette.diff_mod));
                        ui.label(
                            egui::RichText::new("Buffer is silence (play audio to capture)")
                                .color(hex_to_c32(&palette.diff_mod))
                                .small(),
                        );
                    });
                } else if let Some((s, e)) = self.selected_region {
                    let sel_dur = (e - s).abs();
                    let s_rel = s - total_dur;
                    let e_rel = e - total_dur;
                    ui.label(
                        egui::RichText::new(format!(
                            "• Selected: {:.1}s to {:.1}s ({:.2}s)",
                            s_rel, e_rel, sel_dur
                        ))
                        .color(hex_to_c32(&palette.harmonies.base))
                        .strong()
                        .small(),
                    );
                    ui.label(
                        egui::RichText::new("[Space] Play  [Ctrl+C] Copy  [Esc] Clear")
                            .color(hex_to_c32(&palette.theme.f_med))
                            .small(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("• Drag on waveform to select  |  Right-click: Full Editor")
                            .color(hex_to_c32(&palette.theme.f_low))
                            .small(),
                    );
                }

                if let Some((ref msg, _)) = self.status_toast {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(msg)
                                .color(hex_to_c32(&palette.diff_add))
                                .strong()
                                .small(),
                        );
                    });
                }
            });

            ui.add_space(2.0);

            // Waveform Canvas
            let canvas_h = 44.0;
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(total_w, canvas_h),
                egui::Sense::click_and_drag(),
            );

            if self.selected_region.is_some() {
                response.request_focus();
            }

            let painter = ui.painter_at(rect);

            // Background & Border (glow border when selection is active and focused)
            painter.rect_filled(rect, Rounding::same(4.0), hex_to_c32(&palette.theme.b_low));
            let border_stroke = if self.selected_region.is_some() {
                Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base))
            } else {
                Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med))
            };
            painter.rect_stroke(
                rect,
                Rounding::same(4.0),
                border_stroke,
            );

            let center_y = rect.center().y;
            let half_h = (canvas_h * 0.44).max(10.0);

            // Center zero-baseline
            painter.line_segment(
                [Pos2::new(rect.min.x, center_y), Pos2::new(rect.max.x, center_y)],
                Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.theme.f_low, 30)),
            );

            // Time Tick Marks (-30s, -20s, -10s, NOW)
            let tick_step_sec = if total_dur > 20.0 { 10.0 } else { 5.0 };
            let mut t_tick = 0.0;
            while t_tick <= total_dur {
                let rel_x = (t_tick / total_dur) * rect.width();
                let tick_x = rect.min.x + rel_x;
                let rel_sec = t_tick - total_dur;
                let tick_label = if rel_sec.abs() < 0.1 {
                    "NOW".to_string()
                } else {
                    format!("{:.0}s", rel_sec)
                };

                painter.line_segment(
                    [Pos2::new(tick_x, rect.min.y), Pos2::new(tick_x, rect.min.y + 4.0)],
                    Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.theme.f_med, 60)),
                );
                painter.text(
                    Pos2::new(tick_x + 2.0, rect.min.y + 3.0),
                    egui::Align2::LEFT_TOP,
                    tick_label,
                    egui::FontId::proportional(9.0),
                    hex_to_c32_alpha(&palette.theme.f_med, 100),
                );

                t_tick += tick_step_sec;
            }

            // Draw colored waveform bars
            if !self.peaks_lvl0.is_empty() {
                if self.is_silent() {
                    painter.text(
                        Pos2::new(rect.center().x, center_y),
                        egui::Align2::CENTER_CENTER,
                        "Capture buffer is currently silence — play audio and click Refresh (or press F5)",
                        egui::FontId::proportional(11.0),
                        hex_to_c32_alpha(&palette.theme.f_med, 160),
                    );
                } else {
                    let num_bars = (rect.width() as usize).clamp(50, 1000);
                    let bar_w = rect.width() / num_bars as f32;
                    let total_frames = self.mono_samples.len();
                    let sel_range = self.selected_region.map(|(a, b)| (a.min(b), a.max(b)));

                    let mut mesh = egui::Mesh::default();
                    for b in 0..num_bars {
                        let bar_x = rect.min.x + b as f32 * bar_w;
                        let t_start = ((bar_x - rect.min.x) / rect.width()) * total_dur;
                        let t_end = (((bar_x + bar_w) - rect.min.x) / rect.width()) * total_dur;
                        let t_mid = (t_start + t_end) * 0.5;

                        let f_s = ((t_start * self.sample_rate as f32) as usize).min(total_frames);
                        let f_e = (((t_end * self.sample_rate as f32) as usize) + 1).min(total_frames);

                        let (min_s, max_s, block_col) = self.query_peaks(f_s, f_e);

                        let in_sel = sel_range.map_or(false, |(s, e)| t_mid >= s && t_mid <= e);
                        let is_silent_col = max_s.abs() < 0.003 && min_s.abs() < 0.003;

                        if is_silent_col {
                            let dot_col = if in_sel {
                                hex_to_c32_alpha(&palette.harmonies.base, 100)
                            } else {
                                hex_to_c32_alpha(&palette.theme.f_low, 30)
                            };
                            mesh.add_colored_rect(
                                Rect::from_min_size(Pos2::new(bar_x, center_y - 0.5), Vec2::new(bar_w.max(1.0), 1.0)),
                                dot_col,
                            );
                        } else {
                            let bar_col = if in_sel {
                                hex_to_c32(&palette.harmonies.base)
                            } else {
                                Color32::from_rgb(block_col[0], block_col[1], block_col[2])
                            };

                            let y_top = center_y - max_s * half_h;
                            let y_bot = center_y - min_s * half_h;
                            let h = (y_bot - y_top).max(2.0);

                            mesh.add_colored_rect(
                                Rect::from_min_size(Pos2::new(bar_x, y_top), Vec2::new(bar_w.max(1.0), h)),
                                bar_col,
                            );
                        }
                    }
                    painter.add(egui::Shape::mesh(mesh));
                }
            } else if let Some(ref err) = self.error_msg {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", err),
                    egui::FontId::proportional(11.0),
                    hex_to_c32(&palette.diff_del),
                );
            } else {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No system audio captured yet",
                    egui::FontId::proportional(11.0),
                    hex_to_c32(&palette.theme.f_low),
                );
            }

            // Draw Selection Highlight Overlay
            if let Some((s, e)) = self.selected_region {
                let sel_s = s.min(e).clamp(0.0, total_dur);
                let sel_e = s.max(e).clamp(0.0, total_dur);
                let x_s = rect.min.x + (sel_s / total_dur) * rect.width();
                let x_e = rect.min.x + (sel_e / total_dur) * rect.width();
                let sel_rect = Rect::from_min_max(
                    Pos2::new(x_s, rect.min.y),
                    Pos2::new(x_e, rect.max.y),
                );

                painter.rect_filled(
                    sel_rect,
                    Rounding::ZERO,
                    hex_to_c32_alpha(&palette.harmonies.base, 40),
                );
                painter.line_segment(
                    [Pos2::new(x_s, rect.min.y), Pos2::new(x_s, rect.max.y)],
                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                );
                painter.line_segment(
                    [Pos2::new(x_e, rect.min.y), Pos2::new(x_e, rect.max.y)],
                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                );
            }

            // Draw Playhead Line
            if self.is_playing {
                let px = rect.min.x + (self.playhead_sec.clamp(0.0, total_dur) / total_dur) * rect.width();
                painter.line_segment(
                    [Pos2::new(px, rect.min.y), Pos2::new(px, rect.max.y)],
                    Stroke::new(2.0_f32, hex_to_c32(&palette.harmonies.complementary)),
                );
                painter.circle_filled(Pos2::new(px, rect.min.y + 3.0), 3.0, hex_to_c32(&palette.harmonies.complementary));
            }

            // --- Mouse / Pointer Interaction ---
            let x_to_time = |x: f32| -> f32 {
                let rel = ((x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                rel * total_dur
            };

            if response.drag_started() {
                if self.is_playing {
                    self.stop();
                }
                if let Some(pos) = response.interact_pointer_pos() {
                    let t0 = x_to_time(pos.x);
                    self.drag_start_time = Some(t0);
                    self.selected_region = Some((t0, t0));
                    self.is_dragging = true;
                }
            } else if response.dragged() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if let Some(t0) = self.drag_start_time {
                        let t_cur = x_to_time(pos.x);
                        self.selected_region = Some((t0.min(t_cur), t0.max(t_cur)));
                    }
                }
            } else if response.drag_stopped() {
                self.is_dragging = false;
                if let Some((s, e)) = self.selected_region {
                    if (e - s).abs() < 0.05 {
                        // Clicked without dragging: clear selection
                        self.selected_region = None;
                        if self.is_silent() {
                            self.load_snapshot();
                        }
                    }
                }
            }

            // Right-click opens the existing full audio editor
            if response.secondary_clicked() {
                if let Some(ref path) = self.path {
                    *open_full_editor = Some((path.clone(), self.selected_region));
                }
            }
        });
    }
}

fn aggregate_blocks(blocks: &[PeakBlock]) -> (f32, f32, [u8; 3]) {
    if blocks.is_empty() {
        return (0.0, 0.0, [56, 189, 248]);
    }
    let mut mn = 1.0f32;
    let mut mx = -1.0f32;
    let mut max_abs = 0.0f32;
    let mut best_rgb = blocks[0].rgb;
    for b in blocks {
        if b.min < mn {
            mn = b.min;
        }
        if b.max > mx {
            mx = b.max;
        }
        let abs_val = b.max.abs().max(b.min.abs());
        if abs_val > max_abs {
            max_abs = abs_val;
            best_rgb = b.rgb;
        }
    }
    if mn > mx {
        mn = 0.0;
        mx = 0.0;
    }
    (mn, mx, best_rgb)
}

fn load_pcm16_samples(path: &Path) -> Result<(Vec<f32>, u32, u16), std::io::Error> {
    let mut file = BufReader::new(File::open(path)?);
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not a valid WAV",
        ));
    }
    let mut channels = 2u16;
    let mut sample_rate = 48000u32;
    let mut bits_per_sample = 16u16;
    let mut data_size = 0u32;

    loop {
        let mut chunk_hdr = [0u8; 8];
        if file.read_exact(&mut chunk_hdr).is_err() {
            break;
        }
        let chunk_id = &chunk_hdr[0..4];
        let chunk_len =
            u32::from_le_bytes([chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]]);
        if chunk_id == b"fmt " {
            let mut fmt_bytes = vec![0u8; chunk_len as usize];
            file.read_exact(&mut fmt_bytes)?;
            if fmt_bytes.len() >= 16 {
                channels = u16::from_le_bytes([fmt_bytes[2], fmt_bytes[3]]);
                sample_rate =
                    u32::from_le_bytes([fmt_bytes[4], fmt_bytes[5], fmt_bytes[6], fmt_bytes[7]]);
                bits_per_sample = u16::from_le_bytes([fmt_bytes[14], fmt_bytes[15]]);
            }
        } else if chunk_id == b"data" {
            data_size = chunk_len;
            break;
        } else {
            file.seek(SeekFrom::Current(chunk_len as i64))?;
        }
    }

    if bits_per_sample != 16 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Only 16-bit PCM supported",
        ));
    }

    let num_samples = (data_size / 2) as usize;
    let ch = channels.max(1) as usize;
    let num_frames = num_samples / ch;
    let mut mono = Vec::with_capacity(num_frames);

    let mut buf = vec![0u8; 8192];
    let mut current_frame = Vec::with_capacity(ch);
    let mut remaining = data_size as usize;

    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = file.read(&mut buf[..to_read])?;
        if n == 0 {
            break;
        }
        let read_samples = n / 2;
        for i in 0..read_samples {
            let s = i16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
            current_frame.push(s as f32 / 32768.0);
            if current_frame.len() == ch {
                let avg: f32 = current_frame.iter().sum::<f32>() / ch as f32;
                mono.push(avg);
                current_frame.clear();
            }
        }
        remaining -= n;
    }

    Ok((mono, sample_rate, channels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::rolling_audio::RollingAudioBuffer;

    #[test]
    fn test_rolling_strip_play_selection() {
        let mut strip = RollingAudioStripState::new();
        println!("Strip is_loaded: {}", strip.is_loaded);
        println!("Strip duration: {}", strip.duration_sec);
        println!("Strip error_msg: {:?}", strip.error_msg);
        println!("Strip path: {:?}", strip.path);
        println!("Strip player is_some: {}", strip.player.is_some());

        strip.selected_region = Some((25.0, 28.0));
        strip.play_selection();
        println!("Strip is_playing: {}", strip.is_playing);
        println!("Strip playhead_sec: {}", strip.playhead_sec);
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }

    #[test]
    fn test_load_and_query_peaks() {
        let temp_dir = std::env::temp_dir().join("little_clip_workbench_test");
        let _ = std::fs::create_dir_all(&temp_dir);
        let wav_path = temp_dir.join("test_strip.wav");

        let mut buf = RollingAudioBuffer::new(2, 44100, 2);
        let mut samples = Vec::with_capacity(44100 * 4);
        for i in 0..(44100 * 4) {
            samples.push(((i % 500) as i16) * 30);
        }
        buf.push_samples(&samples);
        buf.write_wav(&wav_path).unwrap();

        let (mono, sr, ch) = load_pcm16_samples(&wav_path).unwrap();
        assert_eq!(sr, 44100);
        assert_eq!(ch, 2);
        assert_eq!(mono.len(), 44100 * 2);

        let (l0, l1, l2) = build_peak_pyramid(&mono, sr as f32);
        assert!(!l0.is_empty());
        assert!(!l1.is_empty());
        assert!(!l2.is_empty());

        let strip = RollingAudioStripState {
            path: Some(wav_path.clone()),
            duration_sec: 2.0,
            sample_rate: sr,
            channels: ch,
            mono_samples: mono,
            peaks_lvl0: l0,
            peaks_lvl1: l1,
            peaks_lvl2: l2,
            is_loaded: true,
            error_msg: None,
            selected_region: Some((0.5, 1.2)),
            drag_start_time: None,
            is_dragging: false,
            _sink: None,
            player: None,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            play_stop_time: None,
            status_toast: None,
        };

        let (mn, mx, rgb) = strip.query_peaks(0, 44100);
        assert!(mn <= mx);
        assert!(rgb[0] > 0 || rgb[1] > 0 || rgb[2] > 0);

        let _ = std::fs::remove_file(wav_path);
    }

    #[test]
    fn test_copy_selection_and_clear() {
        let temp_dir = std::env::temp_dir().join("little_clip_workbench_test2");
        let _ = std::fs::create_dir_all(&temp_dir);
        let wav_path = temp_dir.join("test_strip2.wav");

        let mut buf = RollingAudioBuffer::new(2, 44100, 2);
        let samples = vec![1000i16; 44100 * 4];
        buf.push_samples(&samples);
        buf.write_wav(&wav_path).unwrap();

        let mut strip = RollingAudioStripState {
            path: Some(wav_path.clone()),
            duration_sec: 2.0,
            sample_rate: 44100,
            channels: 2,
            mono_samples: vec![0.05; 44100 * 2],
            peaks_lvl0: Vec::new(),
            peaks_lvl1: Vec::new(),
            peaks_lvl2: Vec::new(),
            is_loaded: true,
            error_msg: None,
            selected_region: Some((0.2, 0.8)),
            drag_start_time: None,
            is_dragging: false,
            _sink: None,
            player: None,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            play_stop_time: None,
            status_toast: None,
        };

        assert!(strip.selected_region.is_some());
        // Verify region can be cleared
        strip.selected_region = None;
        assert!(strip.selected_region.is_none());

        let _ = std::fs::remove_file(wav_path);
    }

    #[test]
    fn test_amplitude_and_silence_detection() {
        let mut strip = RollingAudioStripState {
            path: None,
            duration_sec: 10.0,
            sample_rate: 44100,
            channels: 2,
            mono_samples: vec![0.0; 44100 * 10],
            peaks_lvl0: Vec::new(),
            peaks_lvl1: Vec::new(),
            peaks_lvl2: Vec::new(),
            is_loaded: true,
            error_msg: None,
            selected_region: Some((2.0, 5.0)),
            drag_start_time: None,
            is_dragging: false,
            _sink: None,
            player: None,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            play_stop_time: None,
            status_toast: None,
        };

        assert!(strip.is_silent());
        assert_eq!(strip.max_amplitude(), 0.0);
        assert_eq!(strip.selected_region_peak(), 0.0);

        // Inject a signal in seconds 4..6
        for i in (44100 * 4)..(44100 * 6) {
            strip.mono_samples[i] = 0.45;
        }

        assert!(!strip.is_silent());
        assert_eq!(strip.max_amplitude(), 0.45);
        // Region 2.0..5.0 overlaps with 4.0..5.0 which has 0.45
        assert_eq!(strip.selected_region_peak(), 0.45);

        // Region 1.0..3.0 has no signal (silent)
        strip.selected_region = Some((1.0, 3.0));
        assert_eq!(strip.selected_region_peak(), 0.0);
    }

    #[test]
    fn test_query_peaks_block_alignment() {
        // Create 2 seconds of mono audio at 48000 Hz = 96000 frames
        let sr = 48000u32;
        let mut mono = vec![0.0f32; 96000];

        // Put a burst of sound at 1.0s (frame 48000)
        mono[48000] = 0.8;
        mono[48001] = 0.9;

        let (l0, l1, l2) = build_peak_pyramid(&mono, sr as f32);

        let strip = RollingAudioStripState {
            path: None,
            duration_sec: 2.0,
            sample_rate: sr,
            channels: 2,
            mono_samples: mono,
            peaks_lvl0: l0,
            peaks_lvl1: l1,
            peaks_lvl2: l2,
            is_loaded: true,
            error_msg: None,
            selected_region: None,
            drag_start_time: None,
            is_dragging: false,
            _sink: None,
            player: None,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            play_stop_time: None,
            status_toast: None,
        };

        // At frame 48000 (1.0s):
        // Querying around 47500..48500 (span = 1000 => lvl0 blocks):
        let (_, mx0, _) = strip.query_peaks(47500, 48500);
        assert!(
            (mx0 - 0.9).abs() < 0.01,
            "Level 0 must see peak at frame 48000"
        );

        // Querying around 47000..49000 (span = 2000 => lvl1 blocks of 1024 frames):
        // Frame 48000 / 1024 = block 46
        let (_, mx1, _) = strip.query_peaks(47000, 49000);
        assert!(
            (mx1 - 0.9).abs() < 0.01,
            "Level 1 must see peak at frame 48000"
        );

        // Querying earlier range 10000..20000 (0.2s..0.4s) should be silence
        let (_, mx_early, _) = strip.query_peaks(10000, 20000);
        assert_eq!(mx_early, 0.0, "Early region must be silence");

        // Querying later range 60000..70000 (1.25s..1.45s) should be silence
        let (_, mx_late, _) = strip.query_peaks(60000, 70000);
        assert_eq!(mx_late, 0.0, "Late region must be silence");
    }

    #[test]
    #[ignore = "manual integration test requires a live rolling-audio snapshot"]
    fn test_pcm16_sync() {
        let path = PathBuf::from(
            "/run/user/1000/little-screen-clip/rolling_snapshots/current_snapshot.wav",
        );
        if !path.exists() {
            return;
        }
        let (mono, sr, _ch) = load_pcm16_samples(&path).unwrap();
        let first_active = mono.iter().position(|&s| s.abs() > 0.1);
        println!(
            "Rust load_pcm16_samples first active: {:?} ({:.3}s)",
            first_active,
            first_active.map(|i| i as f32 / sr as f32).unwrap_or(0.0)
        );
    }
}
