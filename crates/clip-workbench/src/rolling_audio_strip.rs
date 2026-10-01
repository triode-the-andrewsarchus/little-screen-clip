//! Quick scrubbing and region extraction strip for rolling audio buffer snapshots.
//!
//! Provides waveform peak pyramid visualization, scrub playback, slice selection,
//! and routing to left/right slots.

use crate::icons::Icon;
use crate::{hex_to_c32, hex_to_c32_alpha};
use egui::{Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::audio::{PeakBlock, build_peak_pyramid};
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::palette::AppPalette;
use little_clip_core::slot::HandSide;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use rodio::stream::MixerDeviceSink;
use rodio::{Decoder, Player};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tracing::{error, info};

struct RollingSnapshot {
    path: PathBuf,
    duration_sec: f32,
    sample_rate: u32,
    channels: u16,
    mono_samples: Vec<f32>,
    peaks_lvl0: Vec<PeakBlock>,
    peaks_lvl1: Vec<PeakBlock>,
    peaks_lvl2: Vec<PeakBlock>,
}

type SnapshotReceiver = Receiver<Result<Result<RollingSnapshot, String>, JobError>>;

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
    snapshot_job: Option<(JobTicket, SnapshotReceiver)>,
    snapshot_feedback: bool,

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

/// Actions requested by the rolling strip that must be applied by the workbench host.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RollingAudioAction {
    OpenEditor {
        snapshot_path: PathBuf,
        selected_region: Option<(f32, f32)>,
    },
}

impl RollingAudioStripState {
    /// Materializes a selected rolling-buffer region for the full editor. The
    /// editor must never receive the entire rolling capture when a region was
    /// explicitly selected. ffmpeg's silence filter is used when available;
    /// the already-trimmed time slice remains a safe fallback otherwise.
    pub(crate) fn materialize_editor_selection(
        source: &Path,
        start_sec: f32,
        end_sec: f32,
        cancelled: &AtomicBool,
    ) -> Result<PathBuf, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("Rolling audio editor handoff cancelled".to_owned());
        }
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
        if cancelled.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&sliced);
            return Err("Rolling audio editor handoff cancelled".to_owned());
        }

        let trimmed = directory.join(format!("rolling-{id}-trimmed.wav"));
        let mut trim = std::process::Command::new("ffmpeg");
        trim.args(["-y", "-i"])
            .arg(&sliced)
            .args([
                "-af",
                // Trim each edge independently. `stop_periods` is unreliable
                // across ffmpeg builds for short WAVs; reverse-and-trim gives
                // the same deterministic leading-silence operation on the tail.
                "silenceremove=start_periods=1:start_duration=0.05:start_threshold=-45dB,areverse,silenceremove=start_periods=1:start_duration=0.05:start_threshold=-45dB,areverse",
            ])
            .arg(&trimmed)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let trim_status = crate::audio_preview::run_audio_command_cancellable(&mut trim, cancelled);
        if cancelled.load(Ordering::Acquire) {
            let _ = std::fs::remove_file(&sliced);
            let _ = std::fs::remove_file(&trimmed);
            return Err("Rolling audio editor handoff cancelled".to_owned());
        }
        let status = trim_status.is_ok_and(|status| status.success());
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
            snapshot_job: None,
            snapshot_feedback: false,
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

        state.start_snapshot_load(false);
        state
    }

    #[cfg(test)]
    pub(crate) fn empty_test_strip() -> Self {
        Self {
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
            snapshot_job: None,
            snapshot_feedback: false,
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
        }
    }

    pub fn max_amplitude(&self) -> f32 {
        let blocks = if !self.peaks_lvl2.is_empty() {
            &self.peaks_lvl2
        } else {
            &self.peaks_lvl0
        };
        if !blocks.is_empty() {
            return blocks.iter().fold(0.0f32, |peak, block| {
                peak.max(block.min.abs()).max(block.max.abs())
            });
        }
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

    /// Starts fetching a frozen rolling audio snapshot without blocking the UI.
    pub fn load_snapshot(&mut self) {
        self.start_snapshot_load(true);
    }

    fn start_snapshot_load(&mut self, feedback: bool) {
        if let Some((ticket, _)) = self.snapshot_job.take() {
            ticket.cancel();
        }
        let job = BackgroundJobs::global().submit(Duration::from_secs(20), fetch_snapshot);
        self.snapshot_job = Some(job);
        self.snapshot_feedback = feedback;
        self.error_msg = None;
    }

    fn poll_snapshot(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.snapshot_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.snapshot_job = None;
        let feedback = std::mem::take(&mut self.snapshot_feedback);
        let now = ctx.input(|input| input.time);
        match result {
            Ok(Ok(snapshot)) => {
                self.stop();
                self.selected_region = None;
                self.drag_start_time = None;
                self.is_dragging = false;
                self.playhead_sec = 0.0;
                self.duration_sec = snapshot.duration_sec;
                self.sample_rate = snapshot.sample_rate;
                self.channels = snapshot.channels;
                self.path = Some(snapshot.path);
                self.mono_samples = snapshot.mono_samples;
                self.peaks_lvl0 = snapshot.peaks_lvl0;
                self.peaks_lvl1 = snapshot.peaks_lvl1;
                self.peaks_lvl2 = snapshot.peaks_lvl2;
                self.is_loaded = true;
                self.error_msg = None;
                if feedback {
                    let peak = self.max_amplitude();
                    let message = if peak < 0.001 {
                        "Snapshot refreshed: buffer is currently silence".to_string()
                    } else {
                        format!(
                            "Snapshot refreshed ({:.1}s, peak: {:.0}%)",
                            self.duration_sec,
                            peak * 100.0
                        )
                    };
                    self.status_toast = Some((message, now));
                }
            }
            Ok(Err(message)) => {
                error!("Failed to refresh rolling audio snapshot: {message}");
                self.error_msg = Some(message.clone());
                self.status_toast = Some((message, now));
            }
            Err(error) => {
                let message = format!("Rolling audio refresh failed: {error}");
                self.error_msg = Some(message.clone());
                self.status_toast = Some((message, now));
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
        let slice_dir = if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(rt)
                .join("little-screen-clip")
                .join("rolling_slices")
        } else {
            PathBuf::from("/tmp/little-screen-clip/rolling_slices")
        };
        self.copy_selection_to(now, &slice_dir, |request| {
            crate::send_ipc_request(request).is_ok()
        })
    }

    fn copy_selection_to(
        &mut self,
        now: f64,
        slice_dir: &Path,
        mut push: impl FnMut(&IpcRequest) -> bool,
    ) -> bool {
        let (s, e) = match self.selected_region {
            Some((a, b)) if (b - a).abs() > 0.05 => (a.min(b), a.max(b)),
            _ => return false,
        };
        let source_path = match &self.path {
            Some(path) => path.clone(),
            None => return false,
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
                if push(&req) {
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

    fn render_header(&mut self, ui: &mut egui::Ui, palette: &AppPalette, total_dur: f32) {
        // Header Row: Badge & Status / Selection info
        let canvas = hex_to_c32(&palette.colors.canvas);
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = crate::ui_ext::CONTROL_HEIGHT;
            // Treat each label as one wrapped-row item. egui's flowing-label
            // layout can place its glyphs above the row after a short icon.
            ui.style_mut().wrap = Some(false);
            ui.horizontal_wrapped(|ui| {
                let accent_text = crate::ui_ext::contrast_text_c32_on(
                    canvas,
                    canvas,
                    Some(hex_to_c32(&palette.harmonies.base)),
                    palette,
                );
                Icon::Microphone.ui(ui, 13.0, accent_text);
                ui.label(
                    egui::RichText::new("ROLLING AUDIO")
                        .color(accent_text)
                        .strong()
                        .size(crate::ui_ext::BODY_FONT_SIZE),
                );
                ui.label(
                    egui::RichText::new(format!("({:.0}s snapshot)", total_dur))
                        .color(hex_to_c32(&palette.colors.text_secondary))
                        .size(crate::ui_ext::BODY_FONT_SIZE),
                );

                // Refresh Snapshot button
                let refresh_label = if self.snapshot_job.is_some() {
                    "Refreshing…"
                } else {
                    "Refresh"
                };
                let refresh_btn = ui.add_enabled(
                    self.snapshot_job.is_none(),
                    crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCw),
                        refresh_label,
                        hex_to_c32(&palette.colors.surface_recessed),
                        palette,
                    )
                    .stroke(Stroke::new(
                        1.0_f32,
                        hex_to_c32(&palette.colors.surface_panel),
                    )),
                );
                if refresh_btn.clicked() {
                    self.load_snapshot();
                }
                refresh_btn.on_hover_text("Reload latest rolling audio buffer from daemon (F5)");

                if self.snapshot_job.is_some() && !self.is_loaded {
                    ui.label(
                        egui::RichText::new("Loading audio snapshot…")
                            .color(hex_to_c32(&palette.colors.text_secondary))
                            .size(crate::ui_ext::BODY_FONT_SIZE),
                    );
                } else if let Some(error) = &self.error_msg {
                    ui.label(
                        egui::RichText::new("Refresh failed — retry")
                            .color(crate::ui_ext::contrast_text_c32_on(
                                canvas,
                                canvas,
                                Some(hex_to_c32(&palette.diff_del)),
                                palette,
                            ))
                            .size(crate::ui_ext::BODY_FONT_SIZE),
                    )
                    .on_hover_text(error);
                } else if self.is_silent() {
                    Icon::Alert.ui(ui, 12.0, hex_to_c32(&palette.diff_mod));
                    ui.label(
                        egui::RichText::new("Buffer is silence (play audio to capture)")
                            .color(crate::ui_ext::contrast_text_c32_on(
                                canvas,
                                canvas,
                                Some(hex_to_c32(&palette.diff_mod)),
                                palette,
                            ))
                            .size(crate::ui_ext::BODY_FONT_SIZE),
                    );
                } else if let Some((s, e)) = self.selected_region {
                    let sel_dur = (e - s).abs();
                    let s_rel = s - total_dur;
                    let e_rel = e - total_dur;
                    ui.label(
                        egui::RichText::new(format!(
                            "• Selected: {:.1}s to {:.1}s ({:.2}s)",
                            s_rel, e_rel, sel_dur
                        ))
                        .color(accent_text)
                        .strong()
                        .size(crate::ui_ext::BODY_FONT_SIZE),
                    );
                    ui.label(
                        egui::RichText::new("[Space] Play  [Ctrl+C] Copy  [Esc] Clear")
                            .color(hex_to_c32(&palette.colors.text_secondary))
                            .size(crate::ui_ext::BODY_FONT_SIZE),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(
                            "• Drag on waveform to select  |  Right-click: Full Editor",
                        )
                        .color(hex_to_c32(&palette.colors.text_muted))
                        .size(crate::ui_ext::BODY_FONT_SIZE),
                    );
                }

                if let Some((ref msg, _)) = self.status_toast {
                    if self.error_msg.as_deref() != Some(msg.as_str()) {
                        ui.label(
                            egui::RichText::new(msg)
                                .color(crate::ui_ext::contrast_text_c32_on(
                                    canvas,
                                    canvas,
                                    Some(hex_to_c32(&palette.diff_add)),
                                    palette,
                                ))
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE),
                        );
                    }
                }
            });
        });
        ui.add_space(2.0);
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        host_action: &mut Option<RollingAudioAction>,
    ) {
        self.poll_snapshot(ctx);
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
            self.render_header(ui, palette, total_dur);

            // Waveform Canvas
            let canvas_h = 44.0;
            let (rect, response) = ui.allocate_exact_size(
                Vec2::new(total_w, canvas_h),
                egui::Sense::click_and_drag(),
            );

            let painter = ui.painter_at(rect);

            // Background & Border (glow border when selection is active and focused)
            painter.rect_filled(rect, Rounding::same(4.0), hex_to_c32(&palette.colors.surface_recessed));
            let border_stroke = if self.selected_region.is_some() {
                Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base))
            } else {
                Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel))
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
                Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.colors.text_muted, 30)),
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
                    Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.colors.text_secondary, 60)),
                );
                painter.text(
                    Pos2::new(tick_x + 2.0, rect.min.y + 3.0),
                    egui::Align2::LEFT_TOP,
                    tick_label,
                    egui::FontId::proportional(9.0),
                    hex_to_c32(&palette.colors.text_secondary),
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
                        egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                        hex_to_c32(&palette.colors.text_secondary),
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
                                hex_to_c32_alpha(&palette.colors.text_muted, 30)
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
            } else if self.snapshot_job.is_some() {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Loading rolling audio…",
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    hex_to_c32(&palette.colors.text_secondary),
                );
            } else if let Some(ref err) = self.error_msg {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", err),
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    crate::ui_ext::contrast_text_c32_on(
                        hex_to_c32(&palette.colors.surface_recessed),
                        hex_to_c32(&palette.colors.surface_recessed),
                        Some(hex_to_c32(&palette.diff_del)),
                        palette,
                    ),
                );
            } else {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No system audio captured yet",
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    hex_to_c32(&palette.colors.text_muted),
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
                response.request_focus();
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
                    *host_action = Some(RollingAudioAction::OpenEditor {
                        snapshot_path: path.clone(),
                        selected_region: self.selected_region,
                    });
                }
            }
        });
    }
}

impl Drop for RollingAudioStripState {
    fn drop(&mut self) {
        if let Some((ticket, _)) = &self.snapshot_job {
            ticket.cancel();
        }
    }
}

fn fetch_snapshot() -> Result<RollingSnapshot, String> {
    let (path, duration_sec, sample_rate, channels) =
        match crate::send_ipc_request(&IpcRequest::GetRollingAudio) {
            Ok(IpcResponse::RollingAudio {
                path,
                duration_secs,
                sample_rate,
                channels,
            }) => (path, duration_secs, sample_rate, channels),
            Ok(IpcResponse::Error { error }) => return Err(error),
            Ok(_) => return Err("Unexpected response from daemon".to_string()),
            Err(error) => return Err(format!("IPC connection error: {error}")),
        };
    info!(
        "Rolling audio snapshot received: {:?} ({:.2}s, {}Hz, ch={})",
        path, duration_sec, sample_rate, channels
    );
    let (mono_samples, decoded_rate, decoded_channels) =
        load_pcm16_samples(&path).map_err(|error| format!("Failed to decode WAV: {error}"))?;
    let (peaks_lvl0, peaks_lvl1, peaks_lvl2) =
        build_peak_pyramid(&mono_samples, decoded_rate as f32);
    Ok(RollingSnapshot {
        path,
        duration_sec,
        sample_rate: decoded_rate,
        channels: decoded_channels,
        mono_samples,
        peaks_lvl0,
        peaks_lvl1,
        peaks_lvl2,
    })
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
    fn header_glyphs_are_fully_visible_in_bottom_panel() {
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            for width in [960.0, 1184.0] {
                let ctx = egui::Context::default();
                crate::ui_ext::configure_workbench_style(&ctx);
                let palette = crate::ui_appearance::palette(appearance);
                let mut strip = empty_test_strip();
                for _ in 0..3 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(width, 662.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::TopBottomPanel::bottom("audio-test")
                                .min_height(100.0)
                                .show(ctx, |ui| {
                                    strip.render_ui(ui, ctx, &palette, &mut None);
                                });
                        },
                    );
                    let mut header_found = false;
                    for shape in output.shapes {
                        if let egui::epaint::Shape::Text(text) = shape.shape {
                            if text.galley.job.text.contains("ROLLING AUDIO") {
                                header_found = true;
                            }
                            // All header labels must retain their complete glyphs.
                            if text.pos.y < 610.0 {
                                let bounds = text.galley.mesh_bounds.translate(text.pos.to_vec2());
                                assert!(
                                    shape.clip_rect.expand(0.5).contains_rect(bounds),
                                    "clipped {:?}: {bounds:?} outside {:?}",
                                    text.galley.job.text,
                                    shape.clip_rect
                                );
                            }
                        }
                    }
                    assert!(header_found);
                }
            }
        }
    }

    #[test]
    fn rolling_audio_status_labels_meet_contrast_in_both_appearances() {
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = crate::ui_appearance::palette(appearance);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            for preferred in [
                hex_to_c32(&palette.harmonies.base),
                hex_to_c32(&palette.diff_del),
                hex_to_c32(&palette.diff_mod),
                hex_to_c32(&palette.diff_add),
            ] {
                let foreground = crate::ui_ext::contrast_text_c32_on(
                    colors.canvas,
                    colors.canvas,
                    Some(preferred),
                    &palette,
                );
                assert!(
                    little_clip_core::palette::contrast_ratio(
                        (colors.canvas.r(), colors.canvas.g(), colors.canvas.b()),
                        (foreground.r(), foreground.g(), foreground.b()),
                    ) >= 4.5,
                    "{appearance:?} rolling-audio status label lacks contrast"
                );
            }

            assert!(
                little_clip_core::palette::contrast_ratio(
                    (
                        colors.recessed.r(),
                        colors.recessed.g(),
                        colors.recessed.b()
                    ),
                    (
                        colors.text_secondary.r(),
                        colors.text_secondary.g(),
                        colors.text_secondary.b()
                    ),
                ) >= 4.5
            );
        }
    }

    #[test]
    fn editor_materialization_uses_captured_range_and_optional_trim() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("capture.wav");
        let mut buffer = RollingAudioBuffer::new(2, 44_100, 2);
        buffer.push_samples(&vec![1000i16; 44_100 * 2 * 2]);
        buffer.write_wav(&source).unwrap();

        let output = RollingAudioStripState::materialize_editor_selection(
            &source,
            1.25,
            0.5,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(output.is_file());
        let (samples, sample_rate, _) = load_pcm16_samples(&output).unwrap();
        let duration = samples.len() as f32 / sample_rate as f32;
        assert!(
            (0.6..=0.78).contains(&duration),
            "captured range should remain bounded after optional edge trimming: {duration}s"
        );
        assert!(
            output
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("trimmed")
        );
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn editor_materialization_rejects_short_or_cancelled_selection() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("missing.wav");
        assert_eq!(
            RollingAudioStripState::materialize_editor_selection(
                &source,
                0.0,
                0.04,
                &AtomicBool::new(false),
            ),
            Err("Select at least 50ms of audio before opening the editor".to_owned())
        );
        assert_eq!(
            RollingAudioStripState::materialize_editor_selection(
                &source,
                0.0,
                1.0,
                &AtomicBool::new(true),
            ),
            Err("Rolling audio editor handoff cancelled".to_owned())
        );
    }

    fn complete_snapshot(
        strip: &mut RollingAudioStripState,
        result: Result<RollingSnapshot, String>,
    ) {
        let (ticket, _) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        ticket.cancel();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        strip.snapshot_job = Some((ticket, receiver));
        sender.send(Ok(result)).unwrap();
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| strip.poll_snapshot(ctx));
    }

    fn empty_test_strip() -> RollingAudioStripState {
        RollingAudioStripState::empty_test_strip()
    }

    #[test]
    fn rendered_waveform_right_click_opens_editor_for_the_captured_source_range() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("rolling-source.wav");
        let mut buffer = RollingAudioBuffer::new(2, 44_100, 2);
        buffer.push_samples(&vec![1000i16; 44_100 * 4]);
        buffer.write_wav(&source).unwrap();

        let mut strip = empty_test_strip();
        strip.path = Some(source.clone());
        strip.duration_sec = 2.0;
        strip.sample_rate = 44_100;
        strip.channels = 2;
        strip.mono_samples = vec![0.05; 44_100 * 2];
        strip.is_loaded = true;
        strip.selected_region = Some((0.25, 1.25));

        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let pointer = Pos2::new(450.0, 75.0);
        let mut action = None;
        let mut frame = |time: f64, events: Vec<egui::Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 160.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        strip.render_ui(ui, ctx, &palette, &mut action);
                    });
                },
            );
        };

        frame(1.0, Vec::new());
        frame(1.1, vec![egui::Event::PointerMoved(pointer)]);
        frame(
            1.2,
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            1.3,
            vec![egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        assert_eq!(
            action,
            Some(RollingAudioAction::OpenEditor {
                snapshot_path: source,
                selected_region: Some((0.25, 1.25)),
            })
        );
    }

    #[test]
    fn header_wraps_at_narrow_width_without_extra_rows_when_wide() {
        fn rendered_header_text(strip: &mut RollingAudioStripState, width: f32) -> Vec<String> {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 160.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        strip.render_header(ui, &palette, 120.0);
                    });
                },
            );
            let mut text = Vec::new();
            for clipped_shape in output.shapes {
                let egui::epaint::Shape::Text(shape) = clipped_shape.shape else {
                    continue;
                };
                let bounds = shape.galley.rect.translate(shape.pos.to_vec2());
                assert!(
                    bounds.left() >= clipped_shape.clip_rect.left() - 0.5,
                    "header text starts outside its clip rect: {bounds:?} clipped to {:?}",
                    clipped_shape.clip_rect
                );
                assert!(
                    bounds.right() <= clipped_shape.clip_rect.right() + 0.5,
                    "header text exceeds available width: {bounds:?} clipped to {:?}",
                    clipped_shape.clip_rect
                );
                assert!(!shape.galley.elided, "header text was elided");
                text.push(shape.galley.job.text.clone());
            }
            text
        }

        let mut strip = empty_test_strip();
        strip.error_msg = Some("daemon unavailable".into());
        let narrow_error = rendered_header_text(&mut strip, 520.0);
        let wide_error = rendered_header_text(&mut strip, 1200.0);
        assert!(
            narrow_error
                .iter()
                .any(|text| text.contains("Refresh failed"))
        );
        assert!(
            wide_error
                .iter()
                .any(|text| text.contains("Refresh failed"))
        );

        let connection_error = "IPC connection error: No such file or directory (os error 2)";
        strip.error_msg = Some(connection_error.to_owned());
        strip.status_toast = Some((connection_error.to_owned(), 0.0));
        let error_toast = rendered_header_text(&mut strip, 520.0);
        assert!(
            error_toast
                .iter()
                .any(|text| text.contains("Refresh failed"))
        );
        assert!(
            error_toast
                .iter()
                .all(|text| !text.contains("IPC connection error")),
            "the current error is already available from the failure tooltip and canvas"
        );

        strip.error_msg = None;
        strip.status_toast = None;
        strip.status_toast = Some(("Snapshot refreshed (120.0s, peak: 75%)".into(), 0.0));
        let narrow_silent_toast = rendered_header_text(&mut strip, 520.0);
        assert!(
            narrow_silent_toast
                .iter()
                .any(|text| text.contains("Buffer is silence"))
        );
        assert!(
            narrow_silent_toast
                .iter()
                .any(|text| text.contains("Snapshot refreshed"))
        );
        strip.mono_samples = vec![0.5; 32];
        strip.selected_region = Some((8.0, 45.0));
        let narrow_selected_toast = rendered_header_text(&mut strip, 520.0);
        assert!(
            narrow_selected_toast
                .iter()
                .any(|text| text.contains("Selected:"))
        );
        assert!(
            narrow_selected_toast
                .iter()
                .any(|text| text.contains("[Space] Play"))
        );
        assert!(
            narrow_selected_toast
                .iter()
                .any(|text| text.contains("Snapshot refreshed"))
        );
    }

    #[test]
    fn completed_snapshot_replaces_waveform_and_clears_old_selection() {
        let mut strip = empty_test_strip();
        strip.selected_region = Some((3.0, 7.0));
        strip.is_loaded = true;
        strip.mono_samples = vec![0.0; 16];

        let samples = vec![0.0, 0.5, -0.75, 0.0];
        let (peaks_lvl0, peaks_lvl1, peaks_lvl2) = build_peak_pyramid(&samples, 48000.0);
        strip.snapshot_feedback = true;
        complete_snapshot(
            &mut strip,
            Ok(RollingSnapshot {
                path: PathBuf::from("new-snapshot.wav"),
                duration_sec: 0.5,
                sample_rate: 48000,
                channels: 1,
                mono_samples: samples.clone(),
                peaks_lvl0,
                peaks_lvl1,
                peaks_lvl2,
            }),
        );

        assert_eq!(strip.path.as_deref(), Some(Path::new("new-snapshot.wav")));
        assert_eq!(strip.mono_samples, samples);
        assert_eq!(strip.max_amplitude(), 0.75);
        assert_eq!(strip.sample_rate, 48000);
        assert_eq!(strip.channels, 1);
        assert!(strip.selected_region.is_none());
        assert!(strip.snapshot_job.is_none());
        assert!(strip.error_msg.is_none());
        assert!(strip.status_toast.as_ref().unwrap().0.contains("75%"));
    }

    #[test]
    fn failed_snapshot_keeps_existing_waveform_and_reports_error() {
        let mut strip = empty_test_strip();
        strip.path = Some(PathBuf::from("previous.wav"));
        strip.is_loaded = true;
        strip.mono_samples = vec![0.25, -0.5];
        strip.selected_region = Some((0.1, 0.2));
        complete_snapshot(&mut strip, Err("daemon unavailable".into()));

        assert_eq!(strip.path.as_deref(), Some(Path::new("previous.wav")));
        assert_eq!(strip.mono_samples, vec![0.25, -0.5]);
        assert_eq!(strip.selected_region, Some((0.1, 0.2)));
        assert_eq!(strip.error_msg.as_deref(), Some("daemon unavailable"));
        assert!(strip.snapshot_job.is_none());
    }

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

        let mut strip = empty_test_strip();
        strip.path = Some(wav_path.clone());
        strip.duration_sec = 2.0;
        strip.sample_rate = sr;
        strip.channels = ch;
        strip.mono_samples = mono;
        strip.peaks_lvl0 = l0;
        strip.peaks_lvl1 = l1;
        strip.peaks_lvl2 = l2;
        strip.is_loaded = true;
        strip.selected_region = Some((0.5, 1.2));

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

        let mut strip = empty_test_strip();
        strip.path = Some(wav_path.clone());
        strip.duration_sec = 2.0;
        strip.sample_rate = 44100;
        strip.mono_samples = vec![0.05; 44100 * 2];
        strip.is_loaded = true;
        strip.selected_region = Some((0.2, 0.8));

        assert!(strip.selected_region.is_some());
        // Verify region can be cleared
        strip.selected_region = None;
        assert!(strip.selected_region.is_none());

        let _ = std::fs::remove_file(wav_path);
    }

    #[test]
    fn copy_selection_slices_selected_wav_and_requests_left_hand_delivery() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.wav");
        let output_dir = directory.path().join("rolling-slices");
        let interleaved = [
            10, 100, 20, 200, 30, 300, 40, 400, 50, 500, 60, 600, 70, 700, 80, 800,
        ];
        let mut buffer = RollingAudioBuffer::new(2, 4, 2);
        buffer.push_samples(&interleaved);
        assert_eq!(buffer.write_wav(&source).unwrap(), 2.0);

        let mut strip = empty_test_strip();
        strip.path = Some(source);
        strip.duration_sec = 2.0;
        strip.sample_rate = 4;
        strip.channels = 2;
        strip.selected_region = Some((0.5, 1.5));
        let mut captured = None;
        assert!(strip.copy_selection_to(7.0, &output_dir, |request| {
            match request {
                IpcRequest::PushFile { path, hand } => {
                    captured = Some((path.clone(), *hand));
                    true
                }
                _ => false,
            }
        }));

        let (output_path, hand) = captured.expect("PushFile request captured");
        assert_eq!(hand, HandSide::Left);
        assert_eq!(output_path.parent(), Some(output_dir.as_path()));
        assert!(
            output_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rolling_slice_")
        );
        assert_eq!(
            output_path.extension().and_then(|ext| ext.to_str()),
            Some("wav")
        );

        let output = std::fs::read(output_path).unwrap();
        assert_eq!(&output[0..4], b"RIFF");
        assert_eq!(&output[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([output[22], output[23]]), 2);
        assert_eq!(u32::from_le_bytes(output[24..28].try_into().unwrap()), 4);
        assert_eq!(u16::from_le_bytes([output[34], output[35]]), 16);
        let data_size = u32::from_le_bytes(output[40..44].try_into().unwrap());
        assert_eq!(data_size, 16);
        let duration_sec = data_size as f32 / (4.0 * 2.0 * 2.0);
        assert_eq!(duration_sec, 1.0);
        let sliced_samples = output[44..]
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();
        assert_eq!(sliced_samples, interleaved[4..12]);
        assert_eq!(
            strip
                .status_toast
                .as_ref()
                .map(|(message, _)| message.as_str()),
            Some("Copied 1.0s audio slice to Left Hand & clipboard!")
        );
    }

    #[test]
    fn test_amplitude_and_silence_detection() {
        let mut strip = empty_test_strip();
        strip.duration_sec = 10.0;
        strip.sample_rate = 44100;
        strip.mono_samples = vec![0.0; 44100 * 10];
        strip.is_loaded = true;
        strip.selected_region = Some((2.0, 5.0));

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

        let mut strip = empty_test_strip();
        strip.duration_sec = 2.0;
        strip.sample_rate = sr;
        strip.mono_samples = mono;
        strip.peaks_lvl0 = l0;
        strip.peaks_lvl1 = l1;
        strip.peaks_lvl2 = l2;
        strip.is_loaded = true;

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
