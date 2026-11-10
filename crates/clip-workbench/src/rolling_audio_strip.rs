//! Quick scrubbing and region extraction strip for rolling audio buffer snapshots.
//!
//! Provides waveform peak pyramid visualization, scrub playback, slice selection,
//! and routing to left/right slots.

use crate::icons::Icon;
use crate::ui_ext::{StatusMessage, StatusToast, feedback_time};
use crate::{hex_to_c32, hex_to_c32_alpha};
use egui::{Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::BackgroundJobs;
use little_clip_core::audio::{PeakBlock, build_peak_pyramid};
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::palette::AppPalette;
use little_clip_core::slot::HandSide;
use rodio::Player;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use tracing::{error, info};

mod playback;
mod snapshot;
use little_clip_core::file_source::FileSource;
use little_clip_core::jobs::task::Task;
use playback::PlaybackLoad;
use snapshot::{RollingSnapshot, read_snapshot};

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
    snapshot_job: Option<Task<Result<RollingSnapshot, String>>>,
    source: Option<FileSource>,
    playback_job: Option<PlaybackLoad>,
    copy_job: Option<crate::desktop_ipc::PendingRequest>,

    // Region selection & dragging
    pub selected_region: Option<(f32, f32)>, // (start_sec, end_sec)
    drag_start_time: Option<f32>,
    pub is_dragging: bool,

    // Playback engine
    player: Option<Player>,
    pub is_playing: bool,
    pub playhead_sec: f32,
    playback_start_offset_sec: f32,
    play_stop_time: Option<f32>,

    // Transport hints may expire or clear on stop; accepted-copy failures remain
    // independent until another copy is submitted. Progress belongs to copy_job.
    playback_notice: StatusToast,
    copy_notice: StatusToast,
}

/// Actions requested by the rolling strip that must be applied by the workbench host.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RollingAudioAction {
    OpenEditor {
        source: FileSource,
        duration_sec: f32,
        selected_region: Option<(f32, f32)>,
    },
}

impl RollingAudioStripState {
    /// Materializes a selected rolling-buffer region for the full editor. The
    /// editor must never receive the entire rolling capture when a region was
    /// explicitly selected. ffmpeg's silence filter is used when available;
    /// the already-trimmed time slice remains a safe fallback otherwise.
    pub(crate) fn materialize_editor_selection(
        source: &FileSource,
        start_sec: f32,
        end_sec: f32,
        cancelled: &AtomicBool,
    ) -> Result<crate::audio_preview::PreparedAudioSlice, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("Rolling audio editor handoff cancelled".into());
        }
        if !start_sec.is_finite()
            || !end_sec.is_finite()
            || start_sec.min(end_sec) < 0.0
            || (end_sec - start_sec).abs() < 0.05
        {
            return Err("Select at least 50ms of audio before opening the editor".into());
        }
        crate::audio_preview::prepare_audio_slice(source, start_sec, end_sec, true, cancelled)
    }
    /// Creates a new rolling audio strip state and loads the latest daemon snapshot.
    pub fn new() -> Self {
        let mut state = Self::unloaded();
        state.start_snapshot_load();
        state
    }

    #[cfg(test)]
    pub(crate) fn empty_test_strip() -> Self {
        Self::unloaded()
    }

    fn unloaded() -> Self {
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
            source: None,
            playback_job: None,
            copy_job: None,
            selected_region: None,
            drag_start_time: None,
            is_dragging: false,
            player: None,
            is_playing: false,
            playhead_sec: 0.0,
            playback_start_offset_sec: 0.0,
            play_stop_time: None,
            playback_notice: StatusToast::default(),
            copy_notice: StatusToast::default(),
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

    /// Starts fetching a frozen rolling audio snapshot without blocking the UI.
    pub fn load_snapshot(&mut self) {
        self.start_snapshot_load();
    }

    fn start_snapshot_load(&mut self) {
        self.stop();
        self.snapshot_job = Some(
            Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(20),
                fetch_snapshot,
            )
            .with_response_deadline(Duration::from_secs(20)),
        );
        self.error_msg = None;
    }

    fn poll_snapshot(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.snapshot_job else {
            return;
        };
        let Some(result) = job.poll() else {
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        };
        self.snapshot_job = None;
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
                self.source = Some(snapshot.source);
                self.mono_samples = snapshot.mono_samples;
                self.peaks_lvl0 = snapshot.peaks_lvl0;
                self.peaks_lvl1 = snapshot.peaks_lvl1;
                self.peaks_lvl2 = snapshot.peaks_lvl2;
                self.is_loaded = true;
                self.error_msg = None;
            }
            Ok(Err(message)) => {
                error!("Failed to refresh rolling audio snapshot: {message}");
                self.error_msg = Some(message);
            }
            Err(error) => {
                let message = format!("Rolling audio refresh failed: {error}");
                self.error_msg = Some(message);
            }
        }
    }

    pub fn copy_selection(&mut self, now: f64) -> bool {
        self.copy_selection_on(now, crate::desktop_ipc::mutation_jobs(), |request| {
            crate::send_ipc_request(&request).map_err(|error| error.to_string())
        })
    }

    fn copy_selection_on(
        &mut self,
        _now: f64,
        jobs: &BackgroundJobs,
        send: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) -> bool {
        if self.copy_pending() {
            return false;
        }
        let (start, end) = match self.selected_region {
            Some((a, b))
                if a.is_finite() && b.is_finite() && a.min(b) >= 0.0 && (b - a).abs() > 0.05 =>
            {
                (a.min(b), a.max(b))
            }
            _ => return false,
        };
        let (Some(path), Some(source)) = (self.path.clone(), self.source.clone()) else {
            self.copy_notice.set(
                StatusMessage::error("Refresh the rolling snapshot before copying it."),
                feedback_time(),
            );
            return false;
        };
        self.copy_job = Some(crate::desktop_ipc::PendingRequest::start(jobs, move || {
            let plan = crate::audio_preview::audio_slice_copy_plan(
                &path,
                start,
                end,
                crate::audio_preview::ChannelCopyMode::Native,
                None,
            )
            .with_file_source(source);
            let path =
                crate::audio_preview::extract_audio_slice_file(&plan, &AtomicBool::new(false))?;
            send(IpcRequest::PushFile {
                path,
                hand: HandSide::Left,
            })
        }));
        self.copy_notice.clear();
        true
    }

    pub(crate) fn copy_pending(&self) -> bool {
        self.copy_job.is_some()
    }

    pub(crate) fn poll_copy(&mut self, ctx: &egui::Context) {
        let Some(result) = self.copy_job.as_ref().and_then(|job| job.poll(ctx)) else {
            return;
        };
        self.copy_job = None;
        if let Err(error) = result.and_then(crate::desktop_ipc::action_result) {
            self.copy_notice.set(
                StatusMessage::error(format!("Failed to copy audio slice: {error}")),
                feedback_time(),
            );
        }
        ctx.request_repaint();
    }

    pub fn query_peaks(&self, frame_start: usize, frame_end: usize) -> (f32, f32, [u8; 3]) {
        if self.peaks_lvl0.is_empty() {
            return (0.0, 0.0, [56, 189, 248]);
        }
        aggregate_blocks(little_clip_core::audio::peak_blocks_for_range(
            [&self.peaks_lvl0, &self.peaks_lvl1, &self.peaks_lvl2],
            frame_start..frame_end,
        ))
    }

    fn render_header(&mut self, ui: &mut egui::Ui, palette: &AppPalette, total_dur: f32, now: f64) {
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

                if self.playback_job.is_some() {
                    ui.spinner();
                    ui.label("Preparing playback…");
                }
                if self.copy_pending() {
                    ui.spinner();
                    ui.label("Preparing audio slice…");
                }
                for notice in [&self.copy_notice, &self.playback_notice] {
                    StatusToast::render_opt(&notice.active(now).cloned(), 4.0, ui, palette, now);
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
        self.poll_playback(ctx);
        self.update_playback();
        if self.is_playing {
            ctx.request_repaint();
        }

        let total_w = ui.available_width();
        let total_dur = self.duration_sec.max(0.1);

        ui.vertical(|ui| {
            self.render_header(ui, palette, total_dur, feedback_time());

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
                self.stop();
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
                if let Some(source) = &self.source {
                    *host_action = Some(RollingAudioAction::OpenEditor {
                        source: source.clone(),
                        duration_sec: self.duration_sec,
                        selected_region: self.selected_region,
                    });
                }
            }
        });
    }
}

fn fetch_snapshot(cancel: &AtomicBool) -> Result<RollingSnapshot, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Rolling snapshot cancelled".into());
    }
    let (path, duration_sec, sample_rate, channels) = match crate::send_ipc_request_with_timeout(
        &IpcRequest::GetRollingAudio,
        Some(Duration::from_secs(5)),
    ) {
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
    read_snapshot(&path, cancel)
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

        let identity = FileSource::capture(&source).unwrap();
        let output = RollingAudioStripState::materialize_editor_selection(
            &identity,
            1.25,
            0.5,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(output.path().is_file());
        let snapshot = read_snapshot(output.path(), &AtomicBool::new(false)).unwrap();
        let duration = snapshot.duration_sec;
        assert!(
            (0.6..=0.78).contains(&duration),
            "captured range should remain bounded after optional edge trimming: {duration}s"
        );
    }

    #[test]
    fn editor_materialization_rejects_short_or_cancelled_selection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.wav");
        std::fs::write(&path, b"source identity").unwrap();
        let source = FileSource::capture(&path).unwrap();
        assert_eq!(
            RollingAudioStripState::materialize_editor_selection(
                &source,
                0.0,
                0.04,
                &AtomicBool::new(false),
            )
            .err(),
            Some("Select at least 50ms of audio before opening the editor".to_owned())
        );
        assert_eq!(
            RollingAudioStripState::materialize_editor_selection(
                &source,
                0.0,
                1.0,
                &AtomicBool::new(true),
            )
            .err(),
            Some("Rolling audio editor handoff cancelled".to_owned())
        );
    }

    fn complete_snapshot(
        strip: &mut RollingAudioStripState,
        result: Result<RollingSnapshot, String>,
    ) {
        strip.snapshot_job = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(5),
            move |_| result,
        ));
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while strip.snapshot_job.is_some() {
            strip.poll_snapshot(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
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
        strip.source = Some(FileSource::capture(&source).unwrap());
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
                source: FileSource::capture(&source).unwrap(),
                duration_sec: 2.0,
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
                        strip.render_header(ui, &palette, 120.0, feedback_time());
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
        let narrow_silent = rendered_header_text(&mut strip, 520.0);
        assert!(
            narrow_silent
                .iter()
                .any(|text| text.contains("Buffer is silence"))
        );
        strip.mono_samples = vec![0.5; 32];
        strip.selected_region = Some((8.0, 45.0));
        let narrow_selected = rendered_header_text(&mut strip, 520.0);
        assert!(
            narrow_selected
                .iter()
                .any(|text| text.contains("Selected:"))
        );
        assert!(
            narrow_selected
                .iter()
                .any(|text| text.contains("[Space] Play"))
        );
    }

    #[test]
    fn completed_snapshot_replaces_waveform_and_clears_old_selection() {
        let mut strip = empty_test_strip();
        strip.is_playing = true;
        strip.selected_region = Some((3.0, 7.0));
        strip.is_loaded = true;
        strip.mono_samples = vec![0.0; 16];

        strip
            .copy_notice
            .set(StatusMessage::error("earlier copy failed"), feedback_time());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("new-snapshot.wav");
        std::fs::write(&path, b"test snapshot identity").unwrap();
        let source = FileSource::capture(&path).unwrap();
        let samples = vec![0.0, 0.5, -0.75, 0.0];
        let (peaks_lvl0, peaks_lvl1, peaks_lvl2) = build_peak_pyramid(&samples, 48000.0);
        complete_snapshot(
            &mut strip,
            Ok(RollingSnapshot {
                path: path.clone(),
                source: source.clone(),
                duration_sec: 0.5,
                sample_rate: 48000,
                channels: 1,
                mono_samples: samples.clone(),
                peaks_lvl0,
                peaks_lvl1,
                peaks_lvl2,
            }),
        );

        assert_eq!(strip.path.as_deref(), Some(path.as_path()));
        assert_eq!(strip.source.as_ref(), Some(&source));
        assert_eq!(strip.mono_samples, samples);
        assert_eq!(strip.max_amplitude(), 0.75);
        assert_eq!(strip.sample_rate, 48000);
        assert_eq!(strip.channels, 1);
        assert!(strip.selected_region.is_none());
        assert!(!strip.is_playing);
        assert!(strip.snapshot_job.is_none());
        assert!(strip.error_msg.is_none());
        assert!(!strip.playback_notice.is_active(feedback_time()));
        assert_eq!(
            strip.copy_notice.message(feedback_time() + 60.0),
            Some("earlier copy failed")
        );
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
    fn silent_playback_hint_uses_render_clock_and_stop_preserves_copy_failure() {
        let mut strip = empty_test_strip();
        strip.duration_sec = 1.0;
        strip.selected_region = Some((0.0, 1.0));
        strip
            .copy_notice
            .set(StatusMessage::error("copy failed"), feedback_time());
        strip.play_selection();
        let now = feedback_time();
        assert!(
            strip
                .playback_notice
                .message(now)
                .unwrap()
                .contains("silence")
        );
        assert!(!strip.playback_notice.is_active(now + 4.0));
        assert_eq!(strip.copy_notice.message(now + 60.0), Some("copy failed"));
        strip.stop();
        assert!(!strip.playback_notice.is_active(now));
        assert_eq!(strip.copy_notice.message(now + 60.0), Some("copy failed"));
    }

    #[test]
    fn header_schedules_hint_expiry_and_retains_copy_error_without_extra_height() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut strip = empty_test_strip();
        strip.mono_samples = vec![0.5; 32];
        strip.playback_notice.set("Audition hint", 10.0);
        strip
            .copy_notice
            .set(StatusMessage::error("Copy failed"), 10.0);
        let mut heights = Vec::new();
        for now in [11.0, 15.0, 70.0] {
            let output = ctx.run(
                egui::RawInput {
                    // A distinct frame clock must not determine feedback age.
                    time: Some(1000.0 + now),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 160.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let top = ui.cursor().top();
                        strip.render_header(ui, &palette, 120.0, now);
                        heights.push(ui.cursor().top() - top);
                    });
                },
            );
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        Some(text.galley.job.text.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            assert!(texts.contains(&"Copy failed"));
            assert_eq!(texts.contains(&"Audition hint"), now < 14.0);
            if now == 11.0 {
                assert!(
                    output
                        .viewport_output
                        .values()
                        .any(|viewport| { viewport.repaint_delay <= Duration::from_secs(3) }),
                    "idle hosts must receive an expiry repaint"
                );
            }
        }
        assert!(
            heights
                .iter()
                .all(|height| (height - heights[0]).abs() < 0.5)
        );
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

        let snapshot = read_snapshot(&wav_path, &AtomicBool::new(false)).unwrap();
        let (mono, sr, ch) = (
            snapshot.mono_samples,
            snapshot.sample_rate,
            snapshot.channels,
        );
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
    fn copy_selection_retains_completion_after_stop_and_rejects_repeated_submission() {
        let jobs = BackgroundJobs::new(1, 2);
        let (release, wait) = std::sync::mpsc::channel();
        let mut strip = empty_test_strip();
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("snapshot.wav");
        let samples: Vec<i16> = (0..16000).flat_map(|n| [n as i16, -(n as i16)]).collect();
        let mut buffer = RollingAudioBuffer::new(2, 8000, 2);
        buffer.push_samples(&samples);
        buffer.write_wav(&source).unwrap();
        strip.source = Some(FileSource::capture(&source).unwrap());
        strip.path = Some(source);
        let (captured, artifact_rx) = std::sync::mpsc::channel();
        strip.selected_region = Some((1.5, 0.5));
        assert!(strip.copy_selection_on(7.0, &jobs, move |request| {
            let IpcRequest::PushFile { path, hand } = request else {
                panic!("unexpected handoff");
            };
            assert_eq!(hand, HandSide::Left);
            let mut reader = hound::WavReader::open(&path).unwrap();
            assert_eq!(reader.spec().channels, 2);
            assert_eq!(reader.spec().sample_rate, 8000);
            let actual: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
            assert_eq!(actual, samples[8000..24000]);
            captured.send(path).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(IpcResponse::Error {
                error: "controlled handoff failure".into(),
            })
        }));
        assert!(!strip.copy_selection_on(8.0, &jobs, |_| panic!("duplicate copy")));
        strip.stop();
        strip.selected_region = None;
        assert!(strip.copy_pending());
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| strip.poll_copy(ctx));
        assert!(output.viewport_output.values().any(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::CancelClose))
        }));
        let artifact = artifact_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                strip.render_header(ui, &palette, 2.0, feedback_time() + 60.0);
            });
        });
        assert!(
            output.shapes.iter().any(|shape| matches!(
                &shape.shape,
                egui::epaint::Shape::Text(text) if text.galley.job.text == "Preparing audio slice…"
            )),
            "accepted work owns progress even after a transient notice would expire"
        );
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while strip.copy_pending() {
            strip.poll_copy(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            strip
                .copy_notice
                .message(feedback_time() + 60.0)
                .unwrap()
                .contains("controlled handoff failure")
        );
        assert!(
            artifact.is_file(),
            "published artifact survives handoff failure"
        );
        std::fs::remove_dir_all(artifact.parent().unwrap()).unwrap();
    }

    #[test]
    fn queued_copy_rejects_a_snapshot_replaced_after_the_click() {
        let jobs = BackgroundJobs::new(1, 2);
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let (_, finished) = jobs.submit(Duration::from_secs(5), move || {
            started.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("snapshot.wav");
        let mut buffer = RollingAudioBuffer::new(1, 8000, 1);
        buffer.push_samples(&vec![1234; 8000]);
        buffer.write_wav(&path).unwrap();
        let mut strip = empty_test_strip();
        strip.source = Some(FileSource::capture(&path).unwrap());
        strip.path = Some(path.clone());
        strip.selected_region = Some((0.1, 0.5));
        assert!(
            strip.copy_selection_on(0.0, &jobs, |_| panic!("a replaced source reached handoff"))
        );
        buffer.push_samples(&vec![4321; 8000]);
        buffer.write_wav(&path).unwrap();
        release.send(()).unwrap();
        finished
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while strip.copy_pending() {
            strip.poll_copy(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            strip
                .copy_notice
                .message(feedback_time() + 60.0)
                .unwrap()
                .contains("changed")
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
        let snapshot = read_snapshot(&path, &AtomicBool::new(false)).unwrap();
        let (mono, sr) = (snapshot.mono_samples, snapshot.sample_rate);
        let first_active = mono.iter().position(|&s| s.abs() > 0.1);
        println!(
            "Rust load_pcm16_samples first active: {:?} ({:.3}s)",
            first_active,
            first_active.map(|i| i as f32 / sr as f32).unwrap_or(0.0)
        );
    }
}
