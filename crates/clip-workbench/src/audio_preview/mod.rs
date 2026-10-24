//! Rich interactive audio inspection workbench view.
//!
//! Provides spectral waterfall analysis, zero-crossing snapping, multi-slice region selection,
//! audio playback with Rodio, and export/split capabilities.

use crate::shell::{CommonShortcut, EscapeHatch, ShortcutDispatcher, StudioShell};
use crate::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::audio::{
    AudioAnalysisProgress, AudioFilterChain, AudioMetadata, AudioRegionEdit,
    DEFAULT_TRANSIENT_SENSITIVITY, FullAudioAnalysis, MAX_TRANSIENT_SENSITIVITY,
    MIN_TRANSIENT_SENSITIVITY, apply_audio_gain, compute_audio_analysis_progressive_cancellable,
    compute_audio_analysis_progressive_with_gain_and_edits_cancellable,
    compute_audio_spectral_preview_with_gain_and_edits_cancellable, read_audio_metadata,
};
use little_clip_core::ipc::IpcRequest;
use little_clip_core::palette::AppPalette;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use rodio::stream::MixerDeviceSink;
use rodio::{Decoder, Player, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

/// Top-level audio workbench modes. Future audio tools should be added here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioToolMode {
    #[default]
    Inspect,
    ChannelTools,
    StemSeparation,
    DigitalDecode,
    HfAnalysis,
}

impl AudioToolMode {
    /// Order of tools in the shared Audio Studio tab bar.
    const ALL: &'static [Self] = &[
        Self::Inspect,
        Self::ChannelTools,
        Self::StemSeparation,
        Self::DigitalDecode,
        Self::HfAnalysis,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Inspect => "[I]nspect & Slice",
            Self::ChannelTools => "[C]hannel Tools",
            Self::StemSeparation => "[S]tem Separation",
            Self::DigitalDecode => "[D]igital Decode",
            Self::HfAnalysis => "[H]F Analysis",
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        match self {
            Self::Inspect => crate::icons::Icon::Scissors,
            Self::ChannelTools => crate::icons::Icon::Waves,
            Self::StemSeparation => crate::icons::Icon::Layers,
            Self::DigitalDecode => crate::icons::Icon::BinaryDiff,
            Self::HfAnalysis => crate::icons::Icon::Bat,
        }
    }
}

mod analysis;
mod edits;
mod export;
mod playback;
mod selection;
mod studio;

pub use export::{copy_file_audio_segment, run_audio_command_cancellable};

mod layout;
mod tools;
use tools::*;

fn transient_sensitivity_from_slider(position: f32) -> f32 {
    let midpoint = (DEFAULT_TRANSIENT_SENSITIVITY - MIN_TRANSIENT_SENSITIVITY)
        / (MAX_TRANSIENT_SENSITIVITY - MIN_TRANSIENT_SENSITIVITY);
    let exponent = midpoint.ln() / 0.5_f32.ln();
    MIN_TRANSIENT_SENSITIVITY
        + (MAX_TRANSIENT_SENSITIVITY - MIN_TRANSIENT_SENSITIVITY)
            * position.clamp(0.0, 1.0).powf(exponent)
}

fn transient_sensitivity_to_slider(sensitivity: f32) -> f32 {
    let midpoint = (DEFAULT_TRANSIENT_SENSITIVITY - MIN_TRANSIENT_SENSITIVITY)
        / (MAX_TRANSIENT_SENSITIVITY - MIN_TRANSIENT_SENSITIVITY);
    let exponent = midpoint.ln() / 0.5_f32.ln();
    let normalized = (sensitivity.clamp(MIN_TRANSIENT_SENSITIVITY, MAX_TRANSIENT_SENSITIVITY)
        - MIN_TRANSIENT_SENSITIVITY)
        / (MAX_TRANSIENT_SENSITIVITY - MIN_TRANSIENT_SENSITIVITY);
    normalized.powf(1.0 / exponent)
}

/// Channel routing options when extracting audio slices to clipboard or disk.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChannelCopyMode {
    /// Preserves original multi-channel audio format.
    Native,
    /// Extracts only the left channel (mono).
    LeftOnly,
    /// Extracts only the right channel (mono).
    RightOnly,
}

/// Region selection mode for slicing waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// Single continuous range selection.
    #[default]
    Single,
    /// Multiple independent slice regions across the timeline.
    Multi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpectrumFilterKind {
    LowPass,
    HighPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollEditKind {
    Gain,
    Filter(SpectrumFilterKind),
    Crossfade,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrollEditKey {
    kind: ScrollEditKind,
    mode: SelectionMode,
    index: Option<usize>,
    range: (f32, f32),
}

/// Pure, testable viewport state machine managing timeline zoom and pan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioViewport {
    /// Start offset in seconds of the visible waveform window.
    pub start_sec: f32,
    /// Duration in seconds of the visible waveform window.
    pub duration_sec: f32,
}

impl AudioViewport {
    /// Creates a new viewport showing the full audio duration.
    pub fn new(total_duration: f32) -> Self {
        Self {
            start_sec: 0.0,
            duration_sec: total_duration.max(0.01),
        }
    }

    /// Applies a zoom factor centered around an anchor time point, clamped within bounds.
    pub fn zoom(&mut self, factor: f32, anchor_time: f32, total_duration: f32, sample_rate: f32) {
        let dur = total_duration.max(0.01);
        let min_dur = (12.0 / sample_rate.max(1.0)).max(0.0002);
        let max_dur = dur;

        let old_dur = self.duration_sec.clamp(min_dur, max_dur);
        let new_dur = (old_dur * factor).clamp(min_dur, max_dur);

        let anchor = anchor_time.clamp(0.0, dur);
        let anchor_ratio = if old_dur > 0.0 {
            ((anchor - self.start_sec) / old_dur).clamp(0.0, 1.0)
        } else {
            0.5
        };

        let mut new_start = anchor - anchor_ratio * new_dur;
        if new_start + new_dur > dur {
            new_start = dur - new_dur;
        }
        if new_start < 0.0 {
            new_start = 0.0;
        }

        self.start_sec = new_start;
        self.duration_sec = new_dur;
    }

    /// Pans the visible window by a time delta in seconds, clamped between 0 and duration.
    pub fn pan(&mut self, delta_sec: f32, total_duration: f32) {
        let dur = total_duration.max(0.01);
        let max_start = (dur - self.duration_sec).max(0.0);
        self.start_sec = (self.start_sec + delta_sec).clamp(0.0, max_start);
    }

    /// Resets the visible window to display the entire audio file.
    pub fn fit_to_window(&mut self, total_duration: f32) {
        self.start_sec = 0.0;
        self.duration_sec = total_duration.max(0.01);
    }
}

/// Pure, testable helper for region selection boundaries and containment.
pub struct AudioSelectionModel;

impl AudioSelectionModel {
    /// Returns true if `time_sec` falls within the inclusive `[start, end]` range (order independent).
    #[inline]
    pub fn contains(range: (f32, f32), time_sec: f32) -> bool {
        let (s, e) = if range.0 <= range.1 {
            (range.0, range.1)
        } else {
            (range.1, range.0)
        };
        time_sec >= s && time_sec <= e
    }

    /// Normalizes a `(start, end)` tuple so start <= end.
    #[inline]
    pub fn normalize(range: (f32, f32)) -> (f32, f32) {
        (range.0.min(range.1), range.0.max(range.1))
    }

    /// Clamps a range within `[0.0, total_duration]`.
    #[inline]
    pub fn clamp_to_duration(range: (f32, f32), total_duration: f32) -> (f32, f32) {
        let max_dur = total_duration.max(0.0);
        (range.0.clamp(0.0, max_dur), range.1.clamp(0.0, max_dur))
    }
}

/// Formats a time in seconds as `[hh]m:ss.mmm`.
pub fn format_sample_timestamp(sec: f32) -> String {
    let total_ms = (sec.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 {
        format!("{:02}h{:02}m{:02}s{:03}ms", h, m, s, ms)
    } else {
        format!("{:02}m{:02}s{:03}ms", m, s, ms)
    }
}

/// Formats an audio slice duration in seconds into human-readable minutes/seconds/milliseconds.
pub fn format_sample_duration(sec: f32) -> String {
    let total_ms = (sec.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 {
        format!("{:02}h{:02}m{:02}s{:03}ms", h, m, s, ms)
    } else if m > 0 {
        format!("{:02}m{:02}s{:03}ms", m, s, ms)
    } else {
        format!("{:02}s{:03}ms", s, ms)
    }
}

/// Generates a descriptive filename for an extracted audio slice containing timestamp and duration metadata.
pub fn format_sample_filename(
    original_stem: &str,
    start_sec: f32,
    duration_sec: f32,
    suffix: &str,
    ext: &str,
) -> String {
    let ts_str = format_sample_timestamp(start_sec);
    let dur_str = format_sample_duration(duration_sec);
    if suffix.is_empty() {
        format!("{}_{}_{}.{}", original_stem, ts_str, dur_str, ext)
    } else {
        format!("{}_{}_{}{}.{}", original_stem, ts_str, dur_str, suffix, ext)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragAction {
    ScrubRuler,
    DragLeftHandle {
        other_anchor: f32,
    },
    DragRightHandle {
        other_anchor: f32,
    },
    CreateSelection {
        start_time: f32,
        start_pos_x: f32,
        has_moved: bool,
    },
    PanScrollbar {
        start_x: f32,
        initial_start_sec: f32,
    },
    PanMiddle {
        start_x: f32,
        initial_start_sec: f32,
    },
    CreateMultiSelection {
        start_time: f32,
        start_pos_x: f32,
        has_moved: bool,
        index: usize,
    },
    DragMultiLeftHandle {
        index: usize,
        other_anchor: f32,
    },
    DragMultiRightHandle {
        index: usize,
        other_anchor: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RightClickHoldState {
    pub start_instant: Instant,
    pub target_range: (f32, f32),
    pub slice_index: Option<usize>,
    pub started_looping: bool,
}

pub const RIGHT_CLICK_HOLD_TIMEOUT: Duration = Duration::from_millis(300);

const AUDIO_NUMBER_KEYS: [egui::Key; 10] = [
    egui::Key::Num0,
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];

#[derive(Debug, Clone, Default, PartialEq)]
struct SessionAudioEdits {
    track_gain_db: f32,
    regions: Vec<AudioRegionEdit>,
}

#[derive(Default)]
struct PlaybackEdits {
    generation: AtomicU64,
    edits: RwLock<SessionAudioEdits>,
}

struct EditedPreviewSource<S> {
    inner: S,
    shared: Arc<PlaybackEdits>,
    generation: u64,
    track_gain_db: f32,
    regions: Vec<AudioRegionEdit>,
    frame: u64,
    channel: u16,
    gain_db: f32,
    soft_clip: bool,
    effect_mix: f32,
    duration_sec: f32,
    filters: Vec<AudioFilterChain>,
    active_region: Option<usize>,
}

impl<S: Source> EditedPreviewSource<S> {
    fn new(inner: S, shared: Arc<PlaybackEdits>) -> Self {
        let duration_sec = inner
            .total_duration()
            .map(|duration| duration.as_secs_f32())
            .unwrap_or(f32::MAX);
        Self {
            inner,
            shared,
            generation: u64::MAX,
            track_gain_db: 0.0,
            regions: Vec::new(),
            frame: 0,
            channel: 0,
            gain_db: 0.0,
            soft_clip: false,
            effect_mix: 0.0,
            duration_sec,
            filters: Vec::new(),
            active_region: None,
        }
    }
}

impl<S: Source> Iterator for EditedPreviewSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.channel == 0 {
            let generation = self.shared.generation.load(Ordering::Acquire);
            let changed = generation != self.generation;
            if changed {
                if let Ok(edits) = self.shared.edits.read() {
                    self.track_gain_db = edits.track_gain_db;
                    self.regions.clone_from(&edits.regions);
                }
                self.generation = generation;
            }
            let time = self.frame as f32 / self.inner.sample_rate().get() as f32;
            let active_index = self
                .regions
                .iter()
                .position(|region| region.effect_mix(time, self.duration_sec) > 0.0);
            if active_index != self.active_region || changed {
                self.active_region = active_index;
                self.filters = active_index
                    .map(|index| {
                        (0..self.inner.channels().get())
                            .map(|_| {
                                AudioFilterChain::new(
                                    self.inner.sample_rate().get() as f32,
                                    &self.regions[index],
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            let active = active_index.map(|index| &self.regions[index]);
            self.gain_db = active.map(|region| region.gain_db).unwrap_or(0.0);
            self.soft_clip = active.is_some_and(|region| region.soft_clip);
            self.effect_mix = active
                .map(|region| region.effect_mix(time, self.duration_sec))
                .unwrap_or(0.0);
        }
        let channel = self.channel as usize;
        self.channel += 1;
        if self.channel >= self.inner.channels().get() {
            self.channel = 0;
            self.frame += 1;
        }
        let dry = apply_audio_gain(sample, self.track_gain_db, false);
        let gained = apply_audio_gain(dry, self.gain_db, self.soft_clip);
        let wet = self
            .filters
            .get_mut(channel)
            .map(|filter| filter.process(gained))
            .unwrap_or(gained);
        Some(dry + (wet - dry) * self.effect_mix)
    }
}

impl<S: Source> Source for EditedPreviewSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)?;
        self.frame = (pos.as_secs_f64() * self.inner.sample_rate().get() as f64) as u64;
        self.channel = 0;
        self.active_region = None;
        self.filters.clear();
        Ok(())
    }
}

/// Read-only inline waveform view over a completed shared audio analysis.
#[derive(Clone)]
pub struct InlineAudioWaveform {
    source_analysis: Arc<FullAudioAnalysis>,
}

const MAX_INLINE_WAVEFORM_COLUMNS: usize = 16_384;

impl InlineAudioWaveform {
    pub fn new(source_analysis: Arc<FullAudioAnalysis>) -> Self {
        Self { source_analysis }
    }

    #[cfg(test)]
    pub fn source_analysis(&self) -> &Arc<FullAudioAnalysis> {
        &self.source_analysis
    }

    /// Draws one bar per visible pixel, querying the existing multi-resolution peak pyramid.
    pub fn render(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        let clip_rect = ui.clip_rect();
        let available_width = ui.available_width();
        let width = if available_width.is_finite() {
            available_width
                .max(1.0)
                .min(MAX_INLINE_WAVEFORM_COLUMNS as f32)
        } else {
            clip_rect
                .width()
                .max(1.0)
                .min(MAX_INLINE_WAVEFORM_COLUMNS as f32)
        };
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 44.0), egui::Sense::hover());
        let visible_rect = rect.intersect(clip_rect);
        if visible_rect.is_negative() || visible_rect.width() <= 0.0 || visible_rect.height() <= 0.0
        {
            return;
        }

        let canvas = hex_to_c32(&palette.colors.surface_recessed);
        let accent = hex_to_c32(&palette.harmonies.base);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, Rounding::same(4.0), canvas);

        let frames = self.source_analysis.total_frames;
        if frames == 0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No waveform data",
                egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                hex_to_c32(&palette.colors.text_secondary),
            );
            return;
        }

        let columns = Self::visible_column_count(rect.width());
        let Some(visible_columns) = Self::visible_column_range(rect, visible_rect, columns) else {
            return;
        };
        let center_y = rect.center().y;
        let amplitude = rect.height() * 0.42;
        for column in visible_columns {
            let frame_start = Self::frame_for_column(column, frames, columns);
            let frame_end = Self::frame_for_column(column + 1, frames, columns)
                .max(frame_start + 1)
                .min(frames);
            let (mut min, mut max, _) = self.source_analysis.query_peaks(0, frame_start, frame_end);
            if self.source_analysis.channels > 1 {
                let (right_min, right_max, _) =
                    self.source_analysis.query_peaks(1, frame_start, frame_end);
                min = (min + right_min) * 0.5;
                max = (max + right_max) * 0.5;
            }
            let x = rect.min.x + (column as f32 + 0.5) * rect.width() / columns as f32;
            painter.line_segment(
                [
                    Pos2::new(x, center_y - max.clamp(-1.0, 1.0) * amplitude),
                    Pos2::new(x, center_y - min.clamp(-1.0, 1.0) * amplitude),
                ],
                Stroke::new(1.0_f32, accent),
            );
        }
    }

    fn visible_column_count(width: f32) -> usize {
        if !width.is_finite() {
            return MAX_INLINE_WAVEFORM_COLUMNS;
        }
        width.ceil().clamp(1.0, MAX_INLINE_WAVEFORM_COLUMNS as f32) as usize
    }

    fn visible_column_range(
        rect: Rect,
        visible_rect: Rect,
        columns: usize,
    ) -> Option<std::ops::Range<usize>> {
        let width = rect.width();
        if columns == 0 || !width.is_finite() || width <= 0.0 {
            return None;
        }
        let first = (((visible_rect.min.x - rect.min.x) / width) * columns as f32)
            .floor()
            .clamp(0.0, columns as f32) as usize;
        let end = (((visible_rect.max.x - rect.min.x) / width) * columns as f32)
            .ceil()
            .clamp(0.0, columns as f32) as usize;
        (first < end).then_some(first..end)
    }

    fn frame_for_column(column: usize, frames: usize, columns: usize) -> usize {
        ((column as u128 * frames as u128) / columns as u128) as usize
    }
}

#[derive(Clone, PartialEq)]
struct AudioEditSnapshot {
    path: PathBuf,
    duration_sec: f32,
    track_gain_db: f32,
    selection_mode: SelectionMode,
    selection: Option<(f32, f32)>,
    multi_selections: Vec<(f32, f32)>,
    active_multi_index: Option<usize>,
    region_gain_db: Vec<f32>,
    single_region_gain_db: f32,
    region_crossfade_sec: Vec<f32>,
    single_crossfade_sec: f32,
    single_low_pass_hz: Option<f32>,
    single_high_pass_hz: Option<f32>,
    region_low_pass_hz: Vec<Option<f32>>,
    region_high_pass_hz: Vec<Option<f32>>,
}

/// Interactive UI state for audio waveform preview, spectrogram analysis, and multi-slice editing.
pub struct AudioPreviewState {
    pub path: PathBuf,
    pub title: String,
    /// Optional backing-file owner for completed-source previews created from temporary fixtures.
    _source_file_guard: Option<tempfile::TempPath>,
    pub active_mode: AudioToolMode,
    pub metadata: Option<AudioMetadata>,
    /// Progressive analysis stays session-owned until all stages have completed.
    analysis: Option<Arc<FullAudioAnalysis>>,
    /// Immutable source snapshot published only after the progressive job completes.
    completed_source_analysis: Option<Arc<FullAudioAnalysis>>,
    edited_waveform: Option<Arc<FullAudioAnalysis>>,
    /// Session-specific transient markers recomputed from sensitivity controls.
    session_transients: Option<Arc<[f32]>>,
    pub analysis_rx: Option<Receiver<AudioAnalysisProgress>>,
    analysis_job_rx: Option<Receiver<Result<(), JobError>>>,
    analysis_ticket: Option<JobTicket>,
    analysis_cancel: Option<Arc<AtomicBool>>,
    analysis_retry_at: Option<Instant>,
    analysis_progress_disconnected: bool,
    pub is_loading_analysis: bool,
    pub loading_start_time: Instant,
    pub duration_sec: f32,
    edit_analysis_rx: Option<Receiver<AudioAnalysisProgress>>,
    edit_analysis_job_rx: Option<Receiver<Result<(), JobError>>>,
    edit_analysis_ticket: Option<JobTicket>,
    edit_analysis_cancel: Option<Arc<AtomicBool>>,
    edit_analysis_retry_at: Option<Instant>,
    edit_analysis_progress_disconnected: bool,
    edit_analysis_observed: SessionAudioEdits,
    edit_analysis_inflight: SessionAudioEdits,
    edit_analysis_displayed: SessionAudioEdits,
    edit_change_at: Instant,
    edit_analysis_debounce: Duration,
    playback_edits: Arc<PlaybackEdits>,

    // Viewport Zoom & Pan (Audacity Style)
    pub view_start_sec: f32,
    pub view_duration_sec: f32,

    // GPU Textures for Spectrogram & ColorChord Ribbon
    spectrogram_texture: Option<egui::TextureHandle>,
    note_strip_texture: Option<egui::TextureHandle>,
    pub inspect_viewport_cache: ViewportSpectrogramCache,
    channel_tools: ChannelToolsState,

    // Audio Playback Engine (rodio)
    player: Option<Player>,
    player_factory: fn() -> Option<Player>,
    _sink: Option<MixerDeviceSink>,
    pub is_playing: bool,
    playback_start_instant: Option<Instant>,
    playback_start_offset_sec: f32,
    pub volume: f32,

    // Playhead & Region Selection
    pub current_time_sec: f32,
    pub selection_mode: SelectionMode,
    pub selection: Option<(f32, f32)>, // (start_sec, end_sec) for Single mode
    pub multi_selections: Vec<(f32, f32)>, // (start_sec, end_sec) list for Multi mode
    pub active_multi_index: Option<usize>,
    undo_stack: Vec<AudioEditSnapshot>,
    redo_stack: Vec<AudioEditSnapshot>,
    last_scroll_edit: Option<(ScrollEditKey, Instant)>,
    /// Non-destructive gain applied to the entire track before region effects.
    pub track_gain_db: f32,
    /// Non-destructive gain offsets keyed to multi-region selection indices.
    /// Source samples remain untouched until an export/editor handoff.
    pub region_gain_db: Vec<f32>,
    /// Non-destructive gain for the single selected region.
    pub single_region_gain_db: f32,
    /// Symmetric outward crossfade duration for the single selected region.
    single_crossfade_sec: f32,
    /// Symmetric outward crossfade durations keyed to multi-region indices.
    region_crossfade_sec: Vec<f32>,
    single_low_pass_hz: Option<f32>,
    single_high_pass_hz: Option<f32>,
    region_low_pass_hz: Vec<Option<f32>>,
    region_high_pass_hz: Vec<Option<f32>>,
    pub snap_zero_crossings: bool,
    pub show_transients: bool,
    pub transient_sensitivity: f32,
    pub audio_editor: String,
    active_drag: Option<DragAction>,
    pub loop_selection: bool,
    last_seek_reload: Option<Instant>,

    // Toast Notification
    pub status_toast: Option<(crate::ui_ext::StatusMessage, f64)>,

    // Save to Disk Dialog
    pub save_dialog_open: bool,
    pub save_destination_dir: String,
    pub save_filenames: Vec<String>,
    pub open_folder_after_save: bool,
    browse_rx: Option<Receiver<Option<String>>>,
    save_job: Option<AudioSaveJob>,
    completed_saves: Vec<PathBuf>,
    file_source: Option<crate::file_source::FileSource>,
    slice_copy_job: Option<AudioSliceCopyJob>,

    // Slice preview limit and looping state (for right-click slice preview and hold-to-loop)
    pub preview_stop_time: Option<f32>,
    pub preview_slice_range: Option<(f32, f32)>,
    pub preview_is_looping: bool,
    pub right_click_hold: Option<RightClickHoldState>,

    stem_separation: StemSeparationState,
    digital_decode: DigitalDecodeState,
    pub hf_analysis: HfAnalysisState,
}

struct AudioSaveJob {
    ticket: JobTicket,
    receiver: Receiver<Result<Result<AudioSaveOutcome, String>, JobError>>,
    destination_name: String,
}

impl Drop for AudioSaveJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

struct AudioSaveRegion {
    start: f32,
    end: f32,
    filename: String,
    filter: Option<String>,
}

struct AudioSavePlan {
    versioned: bool,
    file_source: Option<crate::file_source::FileSource>,
    destination: PathBuf,
    source: PathBuf,
    regions: Vec<AudioSaveRegion>,
    open_folder: bool,
}

#[derive(Debug)]
struct AudioSaveOutcome {
    saved_count: usize,
    saved_paths: Vec<PathBuf>,
    failures: Vec<String>,
}

struct AudioSliceCopyJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<String, String>, JobError>>,
}

impl Drop for AudioSliceCopyJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

struct AudioSliceCopyPlan {
    source: PathBuf,
    output: PathBuf,
    start: f32,
    end: f32,
    filter: Option<String>,
}

fn submit_session_analysis(
    path: PathBuf,
    jobs: &BackgroundJobs,
) -> (
    Arc<AtomicBool>,
    JobTicket,
    Receiver<AudioAnalysisProgress>,
    Receiver<Result<(), JobError>>,
) {
    let cancelled = Arc::new(AtomicBool::new(false));
    let job_cancelled = Arc::clone(&cancelled);
    let (progress_tx, progress_rx) = channel();
    let (ticket, job_rx) = jobs.submit(Duration::MAX, move || {
        compute_audio_analysis_progressive_cancellable(&path, 1200, &job_cancelled, |progress| {
            let _ = progress_tx.send(progress);
        });
    });
    (cancelled, ticket, progress_rx, job_rx)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioSliceDelivery {
    Daemon,
    SystemClipboard,
}

impl AudioPreviewState {
    /// Returns the immutable source snapshot once all progressive analysis stages finish.
    #[cfg(test)]
    pub fn completed_source_analysis(&self) -> Option<&Arc<FullAudioAnalysis>> {
        self.completed_source_analysis.as_ref()
    }

    pub(super) fn current_transient_markers(&self) -> &[f32] {
        self.session_transients
            .as_deref()
            .or_else(|| {
                self.analysis
                    .as_deref()
                    .map(|analysis| analysis.transients.as_slice())
            })
            .unwrap_or_default()
    }

    pub(super) fn set_transient_sensitivity(&mut self, sensitivity: f32) {
        self.transient_sensitivity = sensitivity;
        if let Some(transients) = self
            .analysis
            .as_deref()
            .and_then(|analysis| analysis.transients_with_sensitivity(sensitivity))
        {
            self.session_transients = Some(transients.into());
        }
    }
    /// Initializes an audio workbench viewer for the given audio file path.
    pub fn new(path: PathBuf, title: String) -> Self {
        let metadata = read_audio_metadata(&path);
        Self::new_with_metadata(path, title, metadata)
    }

    pub fn new_with_metadata(
        path: PathBuf,
        title: String,
        metadata: Option<AudioMetadata>,
    ) -> Self {
        Self::new_with_metadata_on_jobs(path, title, metadata, BackgroundJobs::global())
    }

    fn new_with_metadata_on_jobs(
        path: PathBuf,
        title: String,
        metadata: Option<AudioMetadata>,
        jobs: &BackgroundJobs,
    ) -> Self {
        Self::with_source_analysis_on_jobs(path, title, metadata, jobs, None)
    }

    /// Creates an independent Audio Studio view over a completed source analysis.
    /// The source-analysis worker is skipped; playback, selection, edits, and later
    /// edited-analysis jobs remain owned by this new viewer state.
    pub fn from_completed_source(
        path: PathBuf,
        title: String,
        metadata: Option<AudioMetadata>,
        source_analysis: Arc<FullAudioAnalysis>,
    ) -> Self {
        Self::with_source_analysis_on_jobs(
            path,
            title,
            metadata,
            BackgroundJobs::global(),
            Some(source_analysis),
        )
    }

    /// Creates a completed-source view that also owns the backing file's temporary-path guard.
    /// This keeps file-based playback, export, and external-editor operations valid after the
    /// gallery that created the fixture has closed.
    pub fn from_completed_source_with_file_guard(
        path: PathBuf,
        title: String,
        metadata: Option<AudioMetadata>,
        source_analysis: Arc<FullAudioAnalysis>,
        file_guard: tempfile::TempPath,
    ) -> Self {
        let mut state = Self::from_completed_source(path, title, metadata, source_analysis);
        state._source_file_guard = Some(file_guard);
        state
    }

    fn with_source_analysis_on_jobs(
        path: PathBuf,
        title: String,
        metadata: Option<AudioMetadata>,
        jobs: &BackgroundJobs,
        source_analysis: Option<Arc<FullAudioAnalysis>>,
    ) -> Self {
        let default_save_dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let duration_sec = source_analysis
            .as_ref()
            .map(|analysis| analysis.duration_ms as f32 / 1000.0)
            .or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| metadata.duration_ms as f32 / 1000.0)
            })
            .unwrap_or(0.0);
        let is_loading_analysis = source_analysis.is_none();

        let (analysis_cancel, analysis_ticket, analysis_rx, analysis_job_rx) = if source_analysis
            .is_some()
        {
            (None, None, None, None)
        } else {
            let (cancel, ticket, progress_rx, job_rx) = submit_session_analysis(path.clone(), jobs);
            (Some(cancel), Some(ticket), Some(progress_rx), Some(job_rx))
        };

        let initial_viewport = AudioViewport::new(duration_sec);

        Self {
            path,
            title,
            _source_file_guard: None,
            active_mode: AudioToolMode::Inspect,
            metadata,
            analysis: source_analysis.clone(),
            completed_source_analysis: source_analysis,
            edited_waveform: None,
            session_transients: None,
            analysis_rx,
            analysis_job_rx,
            analysis_ticket,
            analysis_cancel,
            analysis_retry_at: None,
            analysis_progress_disconnected: false,
            is_loading_analysis,
            loading_start_time: Instant::now(),
            duration_sec,
            edit_analysis_rx: None,
            edit_analysis_job_rx: None,
            edit_analysis_ticket: None,
            edit_analysis_cancel: None,
            edit_analysis_retry_at: None,
            edit_analysis_progress_disconnected: false,
            edit_analysis_observed: SessionAudioEdits::default(),
            edit_analysis_inflight: SessionAudioEdits::default(),
            edit_analysis_displayed: SessionAudioEdits::default(),
            edit_change_at: Instant::now(),
            edit_analysis_debounce: Duration::from_millis(180),
            playback_edits: Arc::default(),
            view_start_sec: initial_viewport.start_sec,
            view_duration_sec: initial_viewport.duration_sec,
            spectrogram_texture: None,
            note_strip_texture: None,
            inspect_viewport_cache: ViewportSpectrogramCache::default(),
            channel_tools: ChannelToolsState::default(),
            player: None,
            player_factory: crate::ui_ext::new_shared_audio_player,
            _sink: None,
            is_playing: false,
            playback_start_instant: None,
            playback_start_offset_sec: 0.0,
            volume: 0.8,
            current_time_sec: 0.0,
            selection_mode: SelectionMode::Single,
            selection: None,
            multi_selections: Vec::new(),
            active_multi_index: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_scroll_edit: None,
            track_gain_db: 0.0,
            region_gain_db: Vec::new(),
            single_region_gain_db: 0.0,
            region_crossfade_sec: Vec::new(),
            single_crossfade_sec: 0.0,
            single_low_pass_hz: None,
            single_high_pass_hz: None,
            region_low_pass_hz: Vec::new(),
            region_high_pass_hz: Vec::new(),
            snap_zero_crossings: false,
            show_transients: false,
            transient_sensitivity: DEFAULT_TRANSIENT_SENSITIVITY,
            audio_editor: "audacity".to_string(),
            active_drag: None,
            loop_selection: false,
            last_seek_reload: None,
            status_toast: None,
            save_dialog_open: false,
            save_destination_dir: default_save_dir,
            save_filenames: Vec::new(),
            open_folder_after_save: true,
            browse_rx: None,
            save_job: None,
            completed_saves: Vec::new(),
            file_source: None,
            slice_copy_job: None,
            preview_stop_time: None,
            preview_slice_range: None,
            preview_is_looping: false,
            right_click_hold: None,
            stem_separation: StemSeparationState::default(),
            digital_decode: DigitalDecodeState::default(),
            hf_analysis: HfAnalysisState::default(),
        }
    }

    /// Enables source checks and non-overwriting staged exports in Files.
    pub fn with_file_source(mut self, source: crate::file_source::FileSource) -> Self {
        self.file_source = Some(source);
        self
    }
    /// Takes paths published by completed exports, never speculative output names.
    pub(crate) fn take_saved_files(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.completed_saves)
    }

    /// Reports an accepted export that must finish before the studio closes.
    pub fn file_write_pending(&self) -> bool {
        self.save_job.is_some()
    }
    /// Reports temporary channel/region edits, which persist only through export.
    pub fn has_session_edits(&self) -> bool {
        self.session_audio_edits() != SessionAudioEdits::default()
    }
    pub fn with_editor(mut self, editor: String) -> Self {
        if !editor.is_empty() {
            self.audio_editor = editor;
        }
        self
    }
}

impl Drop for AudioPreviewState {
    fn drop(&mut self) {
        self.cancel_session_analysis();
        self.cancel_edit_analysis();
        self.cancel_digital_decode();
        self.cancel_stem_separation();
    }
}

fn should_start_stem_separation_on_space(
    space_pressed: bool,
    shortcuts_enabled: bool,
    active_mode: AudioToolMode,
    has_no_stems: bool,
    in_progress: bool,
) -> bool {
    space_pressed
        && shortcuts_enabled
        && active_mode == AudioToolMode::StemSeparation
        && has_no_stems
        && !in_progress
}

#[cfg(test)]
pub(crate) mod tests;
