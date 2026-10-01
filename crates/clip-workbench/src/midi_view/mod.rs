//! Piano roll visualization and MIDI playback workbench view.
//!
//! Renders multi-track MIDI piano rolls, measure grid lines, note inspection,
//! and live audio synthesis playback via Rodio.

mod export;
mod studio;
mod transport;

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::{self, StatusToast, ZoomAction};
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::ipc::IpcRequest;
use little_clip_core::presentation::midi::{MidiData, MidiNote};
use little_clip_core::{BackgroundJobs, HandSide, JobError, JobTicket};
use rodio::stream::MixerDeviceSink;
use rodio::{Decoder, Player};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

const INLINE_MIDI_MAX_NOTES_SCANNED: usize = 512;
const INLINE_MIDI_MAX_NOTES_DRAWN: usize = 128;
static NEXT_INLINE_MIDI_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Compact score overview for embedding in another view.
///
/// Playback is intentionally local to the preview: seeking only moves its
/// playhead and never starts the full viewer's audio synthesis pipeline.
pub struct InlineMidiPreview {
    data: Arc<MidiData>,
    host_size: Vec2,
    id: egui::Id,
    playhead_sec: f32,
    selected_note: Option<usize>,
}

impl InlineMidiPreview {
    /// Creates a compact score preview with a positive finite host size.
    pub fn new(data: Arc<MidiData>, host_size: Vec2) -> Self {
        assert!(
            host_size.x.is_finite()
                && host_size.y.is_finite()
                && host_size.x > 0.0
                && host_size.y > 0.0,
            "inline MIDI preview requires a positive finite host size"
        );
        Self {
            data,
            host_size,
            id: egui::Id::new((
                "inline-midi-preview",
                NEXT_INLINE_MIDI_ID.fetch_add(1, Ordering::Relaxed),
            )),
            playhead_sec: 0.0,
            selected_note: None,
        }
    }

    /// Draws a bounded overview and supports per-instance note selection and seeking.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) -> egui::Response {
        let (rect, _) = ui.allocate_exact_size(self.host_size, egui::Sense::hover());
        let response = ui.interact(rect, self.id.with("host"), egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let clip_rect = rect.intersect(ui.clip_rect());
        let painter = ui.painter().with_clip_rect(clip_rect);
        let panel = crate::hex_to_c32(&palette.colors.surface_recessed);
        let border = crate::hex_to_c32(&palette.colors.surface_panel);
        painter.rect_filled(
            rect,
            Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
            panel,
        );
        painter.rect_stroke(
            rect,
            Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
            Stroke::new(1.0_f32, border),
        );

        let header_height = 22.0_f32.min(rect.height() * 0.35);
        painter.text(
            Pos2::new(rect.left() + 8.0, rect.top() + header_height * 0.5),
            egui::Align2::LEFT_CENTER,
            format!(
                "{} notes · {:.1}s",
                self.data.total_notes, self.data.duration_sec
            ),
            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
            crate::hex_to_c32(&palette.colors.text_primary),
        );

        let canvas_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 6.0, rect.top() + header_height + 3.0),
            Pos2::new(rect.right() - 6.0, rect.bottom() - 6.0),
        );
        if canvas_rect.width() <= 0.0 || canvas_rect.height() <= 0.0 {
            return response;
        }
        let canvas_painter = painter.with_clip_rect(canvas_rect);
        let duration = self.data.duration_sec.max(f32::EPSILON);
        let key_min = f32::from(self.data.min_key);
        let key_range = (f32::from(self.data.max_key) - key_min + 1.0).max(1.0);
        let note_height = (canvas_rect.height() / key_range).clamp(2.0, 9.0);
        let mut note_rects = Vec::with_capacity(INLINE_MIDI_MAX_NOTES_DRAWN);

        let note_count = self.data.notes.len();
        let sample_count = note_count
            .min(INLINE_MIDI_MAX_NOTES_SCANNED)
            .min(INLINE_MIDI_MAX_NOTES_DRAWN);
        for sample_index in 0..sample_count {
            let index = if sample_count <= 1 {
                0
            } else {
                sample_index * (note_count - 1) / (sample_count - 1)
            };
            let Some(note) = self.data.notes.get(index) else {
                continue;
            };
            if note_rects.len() >= INLINE_MIDI_MAX_NOTES_DRAWN
                || !note.start_sec.is_finite()
                || !note.duration_sec.is_finite()
                || note.duration_sec <= 0.0
            {
                continue;
            }
            let start = note.start_sec.clamp(0.0, duration);
            let end = (note.start_sec + note.duration_sec).clamp(0.0, duration);
            if end <= start || note.key < self.data.min_key || note.key > self.data.max_key {
                continue;
            }
            let x_start = canvas_rect.left() + (start / duration) * canvas_rect.width();
            let x_end = canvas_rect.left() + (end / duration) * canvas_rect.width();
            let pitch = f32::from(note.key) - key_min;
            let y = canvas_rect.bottom() - ((pitch + 1.0) / key_range) * canvas_rect.height();
            let note_rect = Rect::from_min_max(
                Pos2::new(x_start, y),
                Pos2::new(x_end.max(x_start + 2.0), y + note_height),
            )
            .intersect(canvas_rect);
            if note_rect.is_positive() {
                let (r, g, b) = palette.midi_note_color(
                    note.track,
                    note.channel,
                    self.data.track_count,
                    note.key,
                    note.velocity,
                );
                let color = Color32::from_rgb(r, g, b);
                canvas_painter.rect_filled(note_rect, Rounding::same(2.0), color);
                if self.selected_note == Some(index) {
                    canvas_painter.rect_stroke(
                        note_rect,
                        Rounding::same(2.0),
                        Stroke::new(1.5_f32, crate::hex_to_c32(&palette.colors.text_primary)),
                    );
                }
                note_rects.push((index, note_rect));
            }
        }

        let playhead_x = canvas_rect.left()
            + (self.playhead_sec.clamp(0.0, duration) / duration) * canvas_rect.width();
        canvas_painter.line_segment(
            [
                Pos2::new(playhead_x, canvas_rect.top()),
                Pos2::new(playhead_x, canvas_rect.bottom()),
            ],
            Stroke::new(1.5_f32, crate::hex_to_c32(&palette.harmonies.complementary)),
        );

        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
            && canvas_rect.contains(pointer)
        {
            if let Some((index, _)) = note_rects
                .iter()
                .find(|(_, note_rect)| note_rect.contains(pointer))
            {
                self.selected_note = Some(*index);
                self.playhead_sec = self.data.notes[*index].start_sec.clamp(0.0, duration);
            } else {
                self.selected_note = None;
                self.playhead_sec = ((pointer.x - canvas_rect.left()) / canvas_rect.width())
                    .clamp(0.0, 1.0)
                    * duration;
            }
        }
        response
    }
}

/// Interactive UI state for piano roll inspection and MIDI playback.
pub struct MidiViewerState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub data: Arc<MidiData>,
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
    pub hovered_note: Option<MidiNote>,
    pub status_toast: StatusToast,
    pub selected_region: Option<(f32, f32)>,
    pub solo_track: Option<usize>,
    pub drag_start_time: Option<f32>,
    pub is_dragging_selection: bool,
    pub play_stop_time: Option<f32>,
    pub is_synthesizing: bool,
    pub pending_play: bool,
    synth_rx: Option<Receiver<Result<(Option<usize>, Vec<u8>), JobError>>>,
    synth_ticket: Option<JobTicket>,
    export_job: Option<MidiExportJob>,
}

struct MidiExportJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<MidiExportArtifact, String>, JobError>>,
}

#[derive(Debug)]
struct MidiExportArtifact {
    path: PathBuf,
    total_notes: usize,
    duration_sec: f32,
    is_slice: bool,
    solo_info: String,
    delivery: MidiExportDelivery,
    retain_file: bool,
}

#[derive(Debug)]
enum MidiExportDelivery {
    PushedToWorkbench,
    ClipboardFallback,
}

impl MidiExportArtifact {
    fn retain(&mut self) {
        self.retain_file = true;
    }
}

impl Drop for MidiExportArtifact {
    fn drop(&mut self) {
        if !self.retain_file {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Computes a distinct palette color for notes in a given MIDI track and channel using theme harmonies.
pub fn get_track_color(
    track_idx: usize,
    channel: u8,
    total_tracks: usize,
    palette: &little_clip_core::palette::AppPalette,
) -> Color32 {
    let (r, g, b) = palette.midi_track_color(track_idx, channel, total_tracks);
    Color32::from_rgb(r, g, b)
}

fn should_draw_pitch_label(key: u8, row_height: f32, label_height: f32) -> bool {
    row_height >= label_height + 2.0 || key % 12 == 0
}

fn time_label_fits(left: f32, width: f32, previous_right: f32, clip_right: f32) -> bool {
    left >= previous_right + 4.0 && left + width <= clip_right
}

fn clamp_label_center(center: f32, height: f32, clip_top: f32, clip_bottom: f32) -> Option<f32> {
    if height > clip_bottom - clip_top {
        return None;
    }
    Some(center.clamp(clip_top + height * 0.5, clip_bottom - height * 0.5))
}

impl MidiViewerState {
    /// Builds the viewer after a background file parse has completed.
    pub(crate) fn from_parsed_file(path: PathBuf, data: MidiData) -> Self {
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("MIDI")
            .to_owned();
        Self::from_data(data, Some(path), title)
    }

    fn from_data(data: MidiData, path: Option<PathBuf>, title: String) -> Self {
        Self::from_shared(Arc::new(data), path, title)
    }

    /// Creates an independent MIDI viewer state from shared parsed score data.
    pub fn from_shared(data: Arc<MidiData>, path: Option<PathBuf>, title: String) -> Self {
        let total_dur = data.duration_sec.max(0.5);
        Self {
            path,
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
            hovered_note: None,
            status_toast: StatusToast::new(),
            selected_region: None,
            solo_track: None,
            drag_start_time: None,
            is_dragging_selection: false,
            play_stop_time: None,
            is_synthesizing: false,
            pending_play: false,
            synth_rx: None,
            synth_ticket: None,
            export_job: None,
            data,
        }
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
}

impl Drop for MidiViewerState {
    fn drop(&mut self) {
        if let Some(ticket) = &self.synth_ticket {
            ticket.cancel();
        }
        self.cancel_export_job();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MidiSpaceAction {
    Play,
    Pause,
}

fn midi_space_action(is_playing: bool) -> MidiSpaceAction {
    if is_playing {
        MidiSpaceAction::Pause
    } else {
        MidiSpaceAction::Play
    }
}

#[cfg(test)]
mod tests;
