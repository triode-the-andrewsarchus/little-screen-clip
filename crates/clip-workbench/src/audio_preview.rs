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
    MIN_TRANSIENT_SENSITIVITY, apply_audio_gain, compute_audio_analysis_progressive,
    compute_audio_analysis_progressive_with_edits, compute_audio_spectral_preview_with_edits,
    read_audio_metadata,
};
use little_clip_core::ipc::IpcRequest;
use little_clip_core::theme::AppPalette;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use rodio::{Decoder, Player, Source};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

/// Top-level audio workbench modes. Future audio tools should be added here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioToolMode {
    #[default]
    Inspect,
    StemSeparation,
    DigitalDecode,
}

impl AudioToolMode {
    /// Order of tools in the shared Audio Studio tab bar.
    const ALL: &'static [Self] = &[Self::Inspect, Self::StemSeparation, Self::DigitalDecode];

    fn label(self) -> &'static str {
        match self {
            Self::Inspect => "[I]nspect & Slice",
            Self::StemSeparation => "[S]tem Separation",
            Self::DigitalDecode => "[D]igital Decode",
        }
    }
}

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

#[derive(Default)]
struct PlaybackEdits {
    generation: AtomicU64,
    regions: RwLock<Vec<AudioRegionEdit>>,
}

struct EditedPreviewSource<S> {
    inner: S,
    shared: Arc<PlaybackEdits>,
    generation: u64,
    regions: Vec<AudioRegionEdit>,
    frame: u64,
    channel: u16,
    gain_db: f32,
    soft_clip: bool,
    filters: Vec<AudioFilterChain>,
    active_region: Option<usize>,
}

impl<S: Source> EditedPreviewSource<S> {
    fn new(inner: S, shared: Arc<PlaybackEdits>) -> Self {
        Self {
            inner,
            shared,
            generation: u64::MAX,
            regions: Vec::new(),
            frame: 0,
            channel: 0,
            gain_db: 0.0,
            soft_clip: false,
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
                if let Ok(regions) = self.shared.regions.read() {
                    self.regions.clone_from(&regions);
                }
                self.generation = generation;
            }
            let time = self.frame as f32 / self.inner.sample_rate().get() as f32;
            let active_index = self
                .regions
                .iter()
                .position(|region| time >= region.start_sec && time < region.end_sec);
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
        }
        let channel = self.channel as usize;
        self.channel += 1;
        if self.channel >= self.inner.channels().get() {
            self.channel = 0;
            self.frame += 1;
        }
        let sample = apply_audio_gain(sample, self.gain_db, self.soft_clip);
        Some(
            self.filters
                .get_mut(channel)
                .map(|filter| filter.process(sample))
                .unwrap_or(sample),
        )
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

/// Interactive UI state for audio waveform preview, spectrogram analysis, and multi-slice editing.
#[derive(Clone, PartialEq)]
struct AudioEditSnapshot {
    path: PathBuf,
    duration_sec: f32,
    selection_mode: SelectionMode,
    selection: Option<(f32, f32)>,
    multi_selections: Vec<(f32, f32)>,
    active_multi_index: Option<usize>,
    region_gain_db: Vec<f32>,
    single_region_gain_db: f32,
    single_low_pass_hz: Option<f32>,
    single_high_pass_hz: Option<f32>,
    region_low_pass_hz: Vec<Option<f32>>,
    region_high_pass_hz: Vec<Option<f32>>,
}

pub struct AudioPreviewState {
    pub path: PathBuf,
    pub title: String,
    pub active_mode: AudioToolMode,
    pub metadata: Option<AudioMetadata>,
    pub analysis: Option<FullAudioAnalysis>,
    edited_waveform: Option<FullAudioAnalysis>,
    pub analysis_rx: Option<Receiver<AudioAnalysisProgress>>,
    pub is_loading_analysis: bool,
    pub loading_start_time: Instant,
    pub duration_sec: f32,
    edit_analysis_rx: Option<Receiver<AudioAnalysisProgress>>,
    edit_analysis_observed: Vec<AudioRegionEdit>,
    edit_analysis_inflight: Vec<AudioRegionEdit>,
    edit_analysis_displayed: Vec<AudioRegionEdit>,
    edit_change_at: Instant,
    edit_analysis_debounce: Duration,
    playback_edits: Arc<PlaybackEdits>,

    // Viewport Zoom & Pan (Audacity Style)
    pub view_start_sec: f32,
    pub view_duration_sec: f32,

    // GPU Textures for Spectrogram & ColorChord Ribbon
    spectrogram_texture: Option<egui::TextureHandle>,
    note_strip_texture: Option<egui::TextureHandle>,

    // Audio Playback Engine (rodio)
    player: Option<Player>,
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
    /// Non-destructive gain offsets keyed to multi-region selection indices.
    /// Source samples remain untouched until an export/editor handoff.
    pub region_gain_db: Vec<f32>,
    /// Non-destructive gain for the single selected region.
    pub single_region_gain_db: f32,
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
    pub status_toast: Option<(String, f64)>,

    // Save to Disk Dialog
    pub save_dialog_open: bool,
    pub save_destination_dir: String,
    pub save_filenames: Vec<String>,
    pub open_folder_after_save: bool,
    browse_rx: Option<Receiver<Option<String>>>,

    // Slice preview limit and looping state (for right-click slice preview and hold-to-loop)
    pub preview_stop_time: Option<f32>,
    pub preview_slice_range: Option<(f32, f32)>,
    pub preview_is_looping: bool,
    pub right_click_hold: Option<RightClickHoldState>,

    stem_separation: StemSeparationState,
    digital_decode: DigitalDecodeState,
}

impl AudioPreviewState {
    const MIN_GAIN_DB: f32 = -60.0;
    const MAX_GAIN_DB: f32 = 40.0; // 100x
    const MAX_UNDO_STEPS: usize = 50;
    const SCROLL_UNDO_WINDOW: Duration = Duration::from_millis(300);

    fn edit_snapshot(&self) -> AudioEditSnapshot {
        AudioEditSnapshot {
            path: self.path.clone(),
            duration_sec: self.duration_sec,
            selection_mode: self.selection_mode,
            selection: self.selection,
            multi_selections: self.multi_selections.clone(),
            active_multi_index: self.active_multi_index,
            region_gain_db: self.region_gain_db.clone(),
            single_region_gain_db: self.single_region_gain_db,
            single_low_pass_hz: self.single_low_pass_hz,
            single_high_pass_hz: self.single_high_pass_hz,
            region_low_pass_hz: self.region_low_pass_hz.clone(),
            region_high_pass_hz: self.region_high_pass_hz.clone(),
        }
    }

    fn record_edit(&mut self, before: AudioEditSnapshot) {
        if before == self.edit_snapshot() {
            return;
        }
        if self.undo_stack.len() == Self::MAX_UNDO_STEPS {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(before);
        self.redo_stack.clear();
        self.last_scroll_edit = None;
    }

    fn record_scroll_edit(
        &mut self,
        before: AudioEditSnapshot,
        kind: ScrollEditKind,
        now: Instant,
    ) {
        if before == self.edit_snapshot() {
            return;
        }
        let target = match self.selection_mode {
            SelectionMode::Single => self.selection.map(|range| (None, range)),
            SelectionMode::Multi => self.active_multi_index.and_then(|index| {
                self.multi_selections
                    .get(index)
                    .copied()
                    .map(|range| (Some(index), range))
            }),
        };
        let Some((index, range)) = target else {
            self.record_edit(before);
            return;
        };
        let key = ScrollEditKey {
            kind,
            mode: self.selection_mode,
            index,
            range,
        };
        let continues = self.last_scroll_edit.is_some_and(|(previous, at)| {
            previous == key
                && now
                    .checked_duration_since(at)
                    .is_some_and(|elapsed| elapsed <= Self::SCROLL_UNDO_WINDOW)
        });
        if !continues {
            self.record_edit(before);
        }
        self.last_scroll_edit = Some((key, now));
    }

    fn restore_edit(&mut self, snapshot: AudioEditSnapshot) {
        let session_changed = self.path != snapshot.path;
        if session_changed {
            self.stop();
        }
        self.path = snapshot.path;
        self.duration_sec = snapshot.duration_sec;
        self.selection_mode = snapshot.selection_mode;
        self.selection = snapshot.selection;
        self.multi_selections = snapshot.multi_selections;
        self.active_multi_index = snapshot.active_multi_index;
        self.region_gain_db = snapshot.region_gain_db;
        self.single_region_gain_db = snapshot.single_region_gain_db;
        self.single_low_pass_hz = snapshot.single_low_pass_hz;
        self.single_high_pass_hz = snapshot.single_high_pass_hz;
        self.region_low_pass_hz = snapshot.region_low_pass_hz;
        self.region_high_pass_hz = snapshot.region_high_pass_hz;
        if session_changed {
            self.current_time_sec = self.current_time_sec.min(self.duration_sec);
            self.view_start_sec = 0.0;
            self.view_duration_sec = self.duration_sec;
            self.restart_session_analysis();
        }
    }

    fn undo(&mut self) {
        self.last_scroll_edit = None;
        if let Some(previous) = self.undo_stack.pop() {
            self.redo_stack.push(self.edit_snapshot());
            self.restore_edit(previous);
        }
    }

    fn redo(&mut self) {
        self.last_scroll_edit = None;
        if let Some(next) = self.redo_stack.pop() {
            if self.undo_stack.len() == Self::MAX_UNDO_STEPS {
                self.undo_stack.remove(0);
            }
            self.undo_stack.push(self.edit_snapshot());
            self.restore_edit(next);
        }
    }

    fn handle_undo_shortcuts(&mut self, ctx: &egui::Context) {
        match ShortcutDispatcher::check_common(ctx) {
            Some(CommonShortcut::Undo) => {
                self.undo();
                ctx.request_repaint();
            }
            Some(CommonShortcut::Redo) => {
                self.redo();
                ctx.request_repaint();
            }
            _ => {}
        }
    }

    fn restart_session_analysis(&mut self) {
        self.analysis = None;
        self.edited_waveform = None;
        self.spectrogram_texture = None;
        self.note_strip_texture = None;
        self.is_loading_analysis = true;
        self.edit_analysis_rx = None;
        self.edit_analysis_observed.clear();
        self.edit_analysis_inflight.clear();
        self.edit_analysis_displayed.clear();
        if let Ok(mut regions) = self.playback_edits.regions.write() {
            regions.clear();
            self.playback_edits
                .generation
                .fetch_add(1, Ordering::Release);
        }
        let (tx, rx) = channel();
        let path = self.path.clone();
        std::thread::spawn(move || {
            compute_audio_analysis_progressive(&path, 1200, |p| {
                let _ = tx.send(p);
            })
        });
        self.analysis_rx = Some(rx);
    }

    fn step_gain_db(current: f32, delta_db: f32) -> f32 {
        if delta_db == 0.0 {
            current
        } else if current == f32::NEG_INFINITY {
            if delta_db > 0.0 {
                Self::MIN_GAIN_DB
            } else {
                current
            }
        } else {
            let next = (current + delta_db).min(Self::MAX_GAIN_DB);
            if next < Self::MIN_GAIN_DB {
                f32::NEG_INFINITY
            } else {
                next
            }
        }
    }

    pub fn adjust_active_region_gain(&mut self, delta_db: f32, now: Instant) {
        let before = self.edit_snapshot();
        match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                self.single_region_gain_db =
                    Self::step_gain_db(self.single_region_gain_db, delta_db);
            }
            SelectionMode::Multi => {
                if let Some(index) = self.active_multi_index {
                    if self.region_gain_db.len() < self.multi_selections.len() {
                        self.region_gain_db.resize(self.multi_selections.len(), 0.0);
                    }
                    if let Some(gain) = self.region_gain_db.get_mut(index) {
                        *gain = Self::step_gain_db(*gain, delta_db);
                    }
                }
            }
            _ => {}
        }
        self.record_scroll_edit(before, ScrollEditKind::Gain, now);
    }

    fn filter_cutoff_range(&self) -> (f32, f32) {
        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate)
            .or_else(|| self.metadata.as_ref().map(|m| m.sample_rate))
            .unwrap_or(44_100) as f32;
        (20.0, (sample_rate * 0.45).min(20_000.0).max(20.0))
    }

    fn step_filter_cutoff(
        current: Option<f32>,
        wheel_steps: f32,
        kind: SpectrumFilterKind,
        range: (f32, f32),
    ) -> Option<f32> {
        if wheel_steps == 0.0 {
            return current;
        }
        let start = match kind {
            SpectrumFilterKind::LowPass => range.1.min(12_000.0),
            SpectrumFilterKind::HighPass => 120.0_f32.clamp(range.0, range.1),
        };
        Some((current.unwrap_or(start) * 2.0_f32.powf(wheel_steps / 4.0)).clamp(range.0, range.1))
    }

    fn adjust_active_region_filter(
        &mut self,
        kind: SpectrumFilterKind,
        wheel_steps: f32,
        now: Instant,
    ) {
        let before = self.edit_snapshot();
        let range = self.filter_cutoff_range();
        match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                let cutoff = match kind {
                    SpectrumFilterKind::LowPass => &mut self.single_low_pass_hz,
                    SpectrumFilterKind::HighPass => &mut self.single_high_pass_hz,
                };
                *cutoff = Self::step_filter_cutoff(*cutoff, wheel_steps, kind, range);
            }
            SelectionMode::Multi => {
                if let Some(index) = self.active_multi_index {
                    let cutoffs = match kind {
                        SpectrumFilterKind::LowPass => &mut self.region_low_pass_hz,
                        SpectrumFilterKind::HighPass => &mut self.region_high_pass_hz,
                    };
                    cutoffs.resize(self.multi_selections.len(), None);
                    if let Some(cutoff) = cutoffs.get_mut(index) {
                        *cutoff = Self::step_filter_cutoff(*cutoff, wheel_steps, kind, range);
                    }
                }
            }
            _ => {}
        }
        self.record_scroll_edit(before, ScrollEditKind::Filter(kind), now);
    }

    fn active_filter_cutoff(&self, kind: SpectrumFilterKind) -> Option<f32> {
        match (self.selection_mode, kind) {
            (SelectionMode::Single, SpectrumFilterKind::LowPass) => self.single_low_pass_hz,
            (SelectionMode::Single, SpectrumFilterKind::HighPass) => self.single_high_pass_hz,
            (SelectionMode::Multi, SpectrumFilterKind::LowPass) => self
                .active_multi_index
                .and_then(|i| self.region_low_pass_hz.get(i).copied().flatten()),
            (SelectionMode::Multi, SpectrumFilterKind::HighPass) => self
                .active_multi_index
                .and_then(|i| self.region_high_pass_hz.get(i).copied().flatten()),
        }
    }

    fn reset_active_region_filter(&mut self, kind: SpectrumFilterKind) -> bool {
        let before = self.edit_snapshot();
        let cutoff = match (self.selection_mode, kind) {
            (SelectionMode::Single, SpectrumFilterKind::LowPass) if self.selection.is_some() => {
                Some(&mut self.single_low_pass_hz)
            }
            (SelectionMode::Single, SpectrumFilterKind::HighPass) if self.selection.is_some() => {
                Some(&mut self.single_high_pass_hz)
            }
            (SelectionMode::Multi, SpectrumFilterKind::LowPass) => self
                .active_multi_index
                .and_then(|i| self.region_low_pass_hz.get_mut(i)),
            (SelectionMode::Multi, SpectrumFilterKind::HighPass) => self
                .active_multi_index
                .and_then(|i| self.region_high_pass_hz.get_mut(i)),
            _ => None,
        };
        let changed = if let Some(cutoff) = cutoff {
            cutoff.take().is_some()
        } else {
            false
        };
        self.record_edit(before);
        changed
    }

    fn clear_single_region_effects(&mut self) {
        self.single_region_gain_db = 0.0;
        self.single_low_pass_hz = None;
        self.single_high_pass_hz = None;
    }

    fn clear_multi_regions(&mut self) {
        self.multi_selections.clear();
        self.region_gain_db.clear();
        self.region_low_pass_hz.clear();
        self.region_high_pass_hz.clear();
        self.active_multi_index = None;
    }

    fn remove_multi_region(&mut self, index: usize) {
        if index >= self.multi_selections.len() {
            return;
        }
        self.multi_selections.remove(index);
        if index < self.region_gain_db.len() {
            self.region_gain_db.remove(index);
        }
        if index < self.region_low_pass_hz.len() {
            self.region_low_pass_hz.remove(index);
        }
        if index < self.region_high_pass_hz.len() {
            self.region_high_pass_hz.remove(index);
        }
        self.active_multi_index = match self.active_multi_index {
            Some(active) if active == index => None,
            Some(active) if active > index => Some(active - 1),
            active => active,
        };
    }

    fn sort_multi_regions(&mut self) {
        let mut order: Vec<usize> = (0..self.multi_selections.len()).collect();
        order.sort_by(|&a, &b| {
            self.multi_selections[a]
                .0
                .total_cmp(&self.multi_selections[b].0)
        });
        let active = self.active_multi_index;
        let regions = self.multi_selections.clone();
        let gains = self.region_gain_db.clone();
        let low = self.region_low_pass_hz.clone();
        let high = self.region_high_pass_hz.clone();
        self.multi_selections = order.iter().map(|&i| regions[i]).collect();
        self.region_gain_db = order
            .iter()
            .map(|&i| gains.get(i).copied().unwrap_or(0.0))
            .collect();
        self.region_low_pass_hz = order
            .iter()
            .map(|&i| low.get(i).copied().flatten())
            .collect();
        self.region_high_pass_hz = order
            .iter()
            .map(|&i| high.get(i).copied().flatten())
            .collect();
        self.active_multi_index = active.and_then(|old| order.iter().position(|&i| i == old));
    }

    fn focus_region_at_time(&mut self, time: f32) -> bool {
        match self.selection_mode {
            SelectionMode::Single => self
                .selection
                .is_some_and(|(start, end)| time >= start.min(end) && time <= start.max(end)),
            SelectionMode::Multi => {
                self.active_multi_index = self
                    .multi_selections
                    .iter()
                    .position(|&(start, end)| time >= start.min(end) && time <= start.max(end));
                self.active_multi_index.is_some()
            }
        }
    }

    fn reset_active_region_gain(&mut self) -> bool {
        let before = self.edit_snapshot();
        let changed = match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                let changed = self.single_region_gain_db != 0.0;
                self.single_region_gain_db = 0.0;
                changed
            }
            SelectionMode::Multi => self
                .active_multi_index
                .and_then(|index| self.region_gain_db.get_mut(index))
                .map(|gain| {
                    let changed = *gain != 0.0;
                    *gain = 0.0;
                    changed
                })
                .unwrap_or(false),
            _ => false,
        };
        self.record_edit(before);
        changed
    }

    fn region_edit(
        &self,
        start: f32,
        end: f32,
        gain_db: f32,
        low_pass_hz: Option<f32>,
        high_pass_hz: Option<f32>,
    ) -> AudioRegionEdit {
        let soft_clip = if gain_db <= 0.0 {
            false
        } else if let Some(analysis) = &self.analysis {
            let sample_rate = analysis.sample_rate as f32;
            let first =
                ((start.min(end).max(0.0) * sample_rate) as usize).min(analysis.total_frames);
            let last =
                ((start.max(end).max(0.0) * sample_rate) as usize).min(analysis.total_frames);
            if first >= last || analysis.channels > 2 {
                true
            } else {
                let mut peak = 0.0_f32;
                for ch in 0..if analysis.ch2_samples.is_some() { 2 } else { 1 } {
                    let (min, max, _) = analysis.query_peaks(ch, first, last);
                    peak = peak.max(min.abs()).max(max.abs());
                }
                peak * 10.0_f32.powf(gain_db / 20.0) > 1.0
            }
        } else {
            true
        };
        AudioRegionEdit {
            start_sec: start.min(end),
            end_sec: start.max(end),
            gain_db,
            soft_clip,
            low_pass_hz,
            high_pass_hz,
        }
    }

    fn active_region_edit(&self) -> Option<AudioRegionEdit> {
        match self.selection_mode {
            SelectionMode::Single => self.selection.map(|(start, end)| {
                self.region_edit(
                    start,
                    end,
                    self.single_region_gain_db,
                    self.single_low_pass_hz,
                    self.single_high_pass_hz,
                )
            }),
            SelectionMode::Multi => self
                .active_multi_index
                .or_else(|| (!self.multi_selections.is_empty()).then_some(0))
                .and_then(|index| {
                    self.multi_selections.get(index).map(|&(start, end)| {
                        self.region_edit(
                            start,
                            end,
                            self.region_gain_db.get(index).copied().unwrap_or(0.0),
                            self.region_low_pass_hz.get(index).copied().flatten(),
                            self.region_high_pass_hz.get(index).copied().flatten(),
                        )
                    })
                }),
        }
    }

    fn region_edits(&self) -> Vec<AudioRegionEdit> {
        let regions: Vec<AudioRegionEdit> = match self.selection_mode {
            SelectionMode::Single => self
                .selection
                .into_iter()
                .map(|(start, end)| {
                    self.region_edit(
                        start,
                        end,
                        self.single_region_gain_db,
                        self.single_low_pass_hz,
                        self.single_high_pass_hz,
                    )
                })
                .collect(),
            SelectionMode::Multi => self
                .multi_selections
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| {
                    let gain_db = self.region_gain_db.get(index).copied().unwrap_or(0.0);
                    self.region_edit(
                        start,
                        end,
                        gain_db,
                        self.region_low_pass_hz.get(index).copied().flatten(),
                        self.region_high_pass_hz.get(index).copied().flatten(),
                    )
                })
                .collect(),
        };
        if regions.iter().any(|region: &AudioRegionEdit| {
            region.gain_db != 0.0 || region.low_pass_hz.is_some() || region.high_pass_hz.is_some()
        }) {
            regions
        } else {
            Vec::new()
        }
    }

    fn gain_filter(gain_db: f32, soft_clip: bool) -> Option<String> {
        if gain_db == 0.0 {
            None
        } else if gain_db == f32::NEG_INFINITY {
            Some("volume=0".to_string())
        } else if soft_clip {
            Some(format!("volume={gain_db}dB,asoftclip=type=tanh"))
        } else {
            Some(format!("volume={gain_db}dB"))
        }
    }

    fn region_filter(region: AudioRegionEdit) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(gain) = Self::gain_filter(region.gain_db, region.soft_clip) {
            filters.push(gain);
        }
        if let Some(cutoff) = region.low_pass_hz {
            filters.push(format!("lowpass=f={cutoff:.3}:p=2"));
        }
        if let Some(cutoff) = region.high_pass_hz {
            filters.push(format!("highpass=f={cutoff:.3}:p=2"));
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    fn channel_edit_filter(mode: ChannelCopyMode, region: AudioRegionEdit) -> Option<String> {
        let pan = match mode {
            ChannelCopyMode::Native => None,
            ChannelCopyMode::LeftOnly => Some("pan=mono|c0=c0"),
            ChannelCopyMode::RightOnly => Some("pan=mono|c0=c1"),
        };
        match (pan, Self::region_filter(region)) {
            (Some(pan), Some(gain)) => Some(format!("{pan},{gain}")),
            (Some(pan), None) => Some(pan.to_string()),
            (None, gain) => gain,
        }
    }
    /// Initializes an audio workbench viewer for the given audio file path.
    pub fn new(path: PathBuf, title: String) -> Self {
        let default_save_dir = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let meta = read_audio_metadata(&path);
        let duration_sec = meta
            .as_ref()
            .map(|m| m.duration_ms as f32 / 1000.0)
            .unwrap_or(0.0);

        // Spawn background worker for progressive audio analysis (Waveform -> Transients -> Spectrogram/ColorChord)
        let (tx, rx) = channel();
        let path_clone = path.clone();
        std::thread::spawn(move || {
            let num_cols = 1200;
            compute_audio_analysis_progressive(&path_clone, num_cols, |progress| {
                let _ = tx.send(progress);
            });
        });

        // Initialize rodio playback engine
        let mut sink = DeviceSinkBuilder::open_default_sink().ok();
        if let Some(ref mut s) = sink {
            s.log_on_drop(false);
        }
        let player = sink.as_ref().map(|s| Player::connect_new(s.mixer()));
        if let Some(ref p) = player {
            p.set_volume(0.8);
        }

        let initial_viewport = AudioViewport::new(duration_sec);

        Self {
            path,
            title,
            active_mode: AudioToolMode::Inspect,
            metadata: meta,
            analysis: None,
            edited_waveform: None,
            analysis_rx: Some(rx),
            is_loading_analysis: true,
            loading_start_time: Instant::now(),
            duration_sec,
            edit_analysis_rx: None,
            edit_analysis_observed: Vec::new(),
            edit_analysis_inflight: Vec::new(),
            edit_analysis_displayed: Vec::new(),
            edit_change_at: Instant::now(),
            edit_analysis_debounce: Duration::from_millis(180),
            playback_edits: Arc::default(),
            view_start_sec: initial_viewport.start_sec,
            view_duration_sec: initial_viewport.duration_sec,
            spectrogram_texture: None,
            note_strip_texture: None,
            player,
            _sink: sink,
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
            region_gain_db: Vec::new(),
            single_region_gain_db: 0.0,
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
            preview_stop_time: None,
            preview_slice_range: None,
            preview_is_looping: false,
            right_click_hold: None,
            stem_separation: StemSeparationState::default(),
            digital_decode: DigitalDecodeState::default(),
        }
    }

    /// Trims leading and trailing silence on both sides by detecting signal boundaries and setting the active selection.
    pub fn trim_silence(&mut self, threshold_db: f32, now: f64) {
        let Some(ref analysis) = self.analysis else {
            self.status_toast = Some(("⚠️ Audio analysis still loading...".to_string(), now));
            return;
        };
        if let Some((start_sec, end_sec)) = analysis.detect_silence_bounds(threshold_db) {
            let dur = end_sec - start_sec;
            self.selection_mode = SelectionMode::Single;
            self.selection = Some((start_sec, end_sec));
            self.clear_single_region_effects();
            self.current_time_sec = start_sec;
            self.status_toast = Some((
                format!(
                    "✂ Trimmed silence: {:.2}s - {:.2}s ({:.2}s duration)",
                    start_sec, end_sec, dur
                ),
                now,
            ));
        } else {
            self.status_toast = Some((
                format!(
                    "⚠️ No audio signal detected above silence threshold ({:.0}dB)",
                    threshold_db
                ),
                now,
            ));
        }
    }

    /// Removes the active slice into a private session WAV. The original file is
    /// never changed; subsequent playback, waveform analysis, export and the
    /// Audacity escape hatch all operate on the squished session timeline.
    fn delete_active_slice_from_session(&mut self, now: f64) {
        let Some((start, end)) = self.active_selection_range() else {
            self.status_toast = Some(("Hover/select a slice before deleting it".into(), now));
            return;
        };
        let start = start.min(end).max(0.0);
        let end = end.max(start).min(self.duration_sec);
        if end - start < 0.001 || end >= self.duration_sec && start <= 0.0 {
            self.status_toast = Some(("Cannot delete the entire session".into(), now));
            return;
        }
        let out = std::env::temp_dir()
            .join("little_clip_audio_sessions")
            .join(format!("edited-{}.wav", uuid::Uuid::new_v4()));
        if let Some(parent) = out.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    "Failed to create audio session directory {}: {err}",
                    parent.display()
                );
                self.status_toast = Some(("Failed to create session directory".into(), now));
                return;
            }
        }
        let mut filters = Vec::new();
        let mut labels = Vec::new();
        if start > 0.001 {
            filters.push(format!("[0:a]atrim=0:{start:.6},asetpts=PTS-STARTPTS[a0]"));
            labels.push("[a0]");
        }
        if end < self.duration_sec - 0.001 {
            filters.push(format!("[0:a]atrim={end:.6},asetpts=PTS-STARTPTS[a1]"));
            labels.push("[a1]");
        }
        let filter = if labels.len() == 2 {
            format!(
                "{};{}concat=n=2:v=0:a=1[out]",
                filters.join(";"),
                labels.join("")
            )
        } else {
            format!("{};{}anull[out]", filters.join(";"), labels[0])
        };
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&self.path)
            .args([
                "-filter_complex",
                &filter,
                "-map",
                "[out]",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok || !out.is_file() {
            self.status_toast = Some(("Failed to create edited audio session".into(), now));
            return;
        }
        let before = self.edit_snapshot();
        self.stop();
        self.path = out;
        self.duration_sec = (self.duration_sec - (end - start)).max(0.0);
        self.view_start_sec = 0.0;
        self.view_duration_sec = self.duration_sec;
        self.current_time_sec = start.min(self.duration_sec);
        self.selection = None;
        self.clear_multi_regions();
        self.clear_single_region_effects();
        self.restart_session_analysis();
        self.record_edit(before);
        self.status_toast = Some((
            format!("Deleted {:.2}s; waveform recomputing", end - start),
            now,
        ));
    }

    pub fn with_editor(mut self, editor: String) -> Self {
        if !editor.is_empty() {
            self.audio_editor = editor;
        }
        self
    }

    fn session_edit_filter(&self) -> Option<String> {
        let regions = self.region_edits();
        if regions.is_empty() {
            return None;
        }
        let mut cuts = vec![0.0, self.duration_sec];
        for region in &regions {
            cuts.push(region.start_sec.clamp(0.0, self.duration_sec));
            cuts.push(region.end_sec.clamp(0.0, self.duration_sec));
        }
        cuts.sort_by(f32::total_cmp);
        cuts.dedup();
        let mut filters = Vec::new();
        for pair in cuts.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if end - start < 0.00001 {
                continue;
            }
            let mid = (start + end) * 0.5;
            let region = regions
                .iter()
                .find(|r| mid >= r.start_sec && mid <= r.end_sec)
                .copied();
            if let Some(gain_filter) = region.and_then(Self::region_filter) {
                let enable = format!("enable='between(t,{start:.6},{end:.6})'");
                filters.extend(
                    gain_filter
                        .split(',')
                        .map(|part| format!("{part}:{enable}")),
                );
            }
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    fn editor_handoff_path(&self) -> Result<PathBuf, String> {
        let Some(filter) = self.session_edit_filter() else {
            return Ok(self.path.clone());
        };
        let out = std::env::temp_dir()
            .join("little_clip_audio_sessions")
            .join(format!("audacity-handoff-{}.wav", uuid::Uuid::new_v4()));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Could not prepare audio handoff: {e}"))?;
        }
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&self.path)
            .args(["-af", &filter, "-c:a", "pcm_s16le"])
            .arg(&out)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && out.is_file() {
            Ok(out)
        } else {
            Err("Could not render audio edits for editor handoff".to_string())
        }
    }

    /// Launches an external audio editor (e.g. Audacity) for the current file.
    pub fn launch_external_audio_editor(
        &mut self,
        _ctx: &egui::Context,
        _close_requested: &mut bool,
    ) {
        let editor = if self.audio_editor.is_empty() {
            "audacity"
        } else {
            &self.audio_editor
        };
        let handoff_path = match self.editor_handoff_path() {
            Ok(path) => path,
            Err(error) => {
                self.status_toast = Some((error, _ctx.input(|i| i.time)));
                return;
            }
        };
        let mut cmd = std::process::Command::new(editor);
        cmd.arg(&handoff_path);
        match cmd.spawn() {
            Ok(_) => {
                let filename = self
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                self.status_toast = Some((
                    format!("🚀 Opened {} in {}", filename, editor),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs_f64(),
                ));
                self.stop();
            }
            Err(e) => {
                // Try xdg-open fallback
                if std::process::Command::new("xdg-open")
                    .arg(&handoff_path)
                    .spawn()
                    .is_ok()
                {
                    self.status_toast = Some((
                        "🚀 Opened audio in default application via xdg-open".to_string(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64(),
                    ));
                    self.stop();
                } else {
                    self.status_toast = Some((
                        format!("⚠️ Failed to launch '{}': {}", editor, e),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64(),
                    ));
                }
            }
        }
    }

    #[inline]
    pub fn is_time_selected(&self, t: f32) -> bool {
        match self.selection_mode {
            SelectionMode::Single => self
                .selection
                .map(|r| AudioSelectionModel::contains(r, t))
                .unwrap_or(false),
            SelectionMode::Multi => self
                .multi_selections
                .iter()
                .any(|&r| AudioSelectionModel::contains(r, t)),
        }
    }

    #[inline]
    pub fn active_selection_range(&self) -> Option<(f32, f32)> {
        let raw = match self.selection_mode {
            SelectionMode::Single => self.selection,
            SelectionMode::Multi => self
                .active_multi_index
                .and_then(|idx| self.multi_selections.get(idx).copied())
                .or_else(|| self.multi_selections.first().copied()),
        }?;
        Some(AudioSelectionModel::clamp_to_duration(
            AudioSelectionModel::normalize(raw),
            self.duration_sec,
        ))
    }

    pub fn snap_time(&self, time_sec: f32, bypass: bool) -> f32 {
        if bypass {
            return time_sec;
        }
        if let Some(ref analysis) = self.analysis {
            if self.show_transients {
                // Magnetic snapping radius: ~12px in screen space converted to seconds
                let snap_radius = (self.view_duration_sec / 800.0) * 12.0;
                if let Some(t_transient) = analysis.find_nearest_transient(time_sec, snap_radius) {
                    return if self.snap_zero_crossings {
                        analysis.snap_to_zero_crossing(t_transient, 500)
                    } else {
                        t_transient
                    };
                }
            }
            if self.snap_zero_crossings {
                return analysis.snap_to_zero_crossing(time_sec, 500);
            }
        }
        time_sec
    }

    pub fn snap_active_selection_to_zero_crossings(&mut self, now: f64) {
        let Some(ref analysis) = self.analysis else {
            return;
        };
        match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    let s_snapped = analysis.snap_to_zero_crossing(s, 500);
                    let e_snapped = analysis.snap_to_zero_crossing(e, 500);
                    let (s_new, e_new) = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                    self.selection = Some((s_new, e_new));
                    self.status_toast = Some((
                        format!(
                            "∿ Snapped selection to 0-crossings: {:.4}s - {:.4}s",
                            s_new, e_new
                        ),
                        now,
                    ));
                }
            }
            SelectionMode::Multi => {
                if let Some(idx) = self.active_multi_index {
                    if idx < self.multi_selections.len() {
                        let (s, e) = self.multi_selections[idx];
                        let s_snapped = analysis.snap_to_zero_crossing(s, 500);
                        let e_snapped = analysis.snap_to_zero_crossing(e, 500);
                        let (s_new, e_new) = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                        self.multi_selections[idx] = (s_new, e_new);
                        self.status_toast = Some((
                            format!(
                                "∿ Snapped region #{} to 0-crossings: {:.4}s - {:.4}s",
                                idx + 1,
                                s_new,
                                e_new
                            ),
                            now,
                        ));
                    }
                } else if !self.multi_selections.is_empty() {
                    for region in &mut self.multi_selections {
                        let s_snapped = analysis.snap_to_zero_crossing(region.0, 500);
                        let e_snapped = analysis.snap_to_zero_crossing(region.1, 500);
                        *region = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                    }
                    self.status_toast = Some((
                        format!(
                            "∿ Snapped all {} regions to 0-crossings",
                            self.multi_selections.len()
                        ),
                        now,
                    ));
                }
            }
        }
    }

    pub fn set_selection_mode(&mut self, mode: SelectionMode) {
        if self.selection_mode == mode {
            return;
        }
        match mode {
            SelectionMode::Multi => {
                if let Some(sel) = self.selection {
                    let index = if let Some(index) =
                        self.multi_selections.iter().position(|range| *range == sel)
                    {
                        index
                    } else {
                        self.multi_selections.push(sel);
                        self.multi_selections.len() - 1
                    };
                    self.region_gain_db.resize(self.multi_selections.len(), 0.0);
                    self.region_gain_db[index] = self.single_region_gain_db;
                    self.region_low_pass_hz
                        .resize(self.multi_selections.len(), None);
                    self.region_high_pass_hz
                        .resize(self.multi_selections.len(), None);
                    self.region_low_pass_hz[index] = self.single_low_pass_hz;
                    self.region_high_pass_hz[index] = self.single_high_pass_hz;
                    self.active_multi_index = Some(index);
                }
            }
            SelectionMode::Single => {
                if let Some(idx) = self.active_multi_index {
                    if idx < self.multi_selections.len() {
                        self.selection = Some(self.multi_selections[idx]);
                        self.single_region_gain_db =
                            self.region_gain_db.get(idx).copied().unwrap_or(0.0);
                        self.single_low_pass_hz =
                            self.region_low_pass_hz.get(idx).copied().flatten();
                        self.single_high_pass_hz =
                            self.region_high_pass_hz.get(idx).copied().flatten();
                    }
                } else if let Some(first) = self.multi_selections.first() {
                    self.selection = Some(*first);
                    self.single_region_gain_db =
                        self.region_gain_db.first().copied().unwrap_or(0.0);
                    self.single_low_pass_hz = self.region_low_pass_hz.first().copied().flatten();
                    self.single_high_pass_hz = self.region_high_pass_hz.first().copied().flatten();
                }
            }
        }
        self.selection_mode = mode;
    }

    pub fn zoom(&mut self, factor: f32, anchor_time: f32) {
        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate as f32)
            .unwrap_or(44100.0);
        let mut vp = AudioViewport {
            start_sec: self.view_start_sec,
            duration_sec: self.view_duration_sec,
        };
        vp.zoom(factor, anchor_time, self.duration_sec, sample_rate);
        self.view_start_sec = vp.start_sec;
        self.view_duration_sec = vp.duration_sec;
    }

    pub fn pan(&mut self, delta_sec: f32) {
        let mut vp = AudioViewport {
            start_sec: self.view_start_sec,
            duration_sec: self.view_duration_sec,
        };
        vp.pan(delta_sec, self.duration_sec);
        self.view_start_sec = vp.start_sec;
    }

    pub fn fit_to_window(&mut self) {
        let mut vp = AudioViewport {
            start_sec: self.view_start_sec,
            duration_sec: self.view_duration_sec,
        };
        vp.fit_to_window(self.duration_sec);
        self.view_start_sec = vp.start_sec;
        self.view_duration_sec = vp.duration_sec;
    }

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

    pub fn play_slice_preview_ex(&mut self, start_sec: f32, end_sec: f32, looping: bool) {
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

        if let Some(ref p) = self.player {
            p.clear();
            if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    p.append(EditedPreviewSource::new(
                        decoder,
                        Arc::clone(&self.playback_edits),
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

        if let Some(ref p) = self.player {
            p.clear();
            if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    p.append(EditedPreviewSource::new(
                        decoder,
                        Arc::clone(&self.playback_edits),
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
                                p.append(EditedPreviewSource::new(
                                    decoder,
                                    Arc::clone(&self.playback_edits),
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

    pub fn copy_selected_segment(&mut self, now: f64, mode: ChannelCopyMode) {
        let (start, end) = self
            .active_selection_range()
            .unwrap_or((0.0, self.duration_sec.max(0.1)));

        let duration = (end - start).max(0.001);
        let base_name = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");

        let staging_dir = std::env::temp_dir().join("little_clip_audio_segments");
        if let Err(err) = std::fs::create_dir_all(&staging_dir) {
            tracing::warn!(
                "Failed to create audio staging directory {}: {err}",
                staging_dir.display()
            );
            self.status_toast = Some(("Failed to create audio staging directory".into(), now));
            return;
        }

        let mode_suffix = match mode {
            ChannelCopyMode::Native => "",
            ChannelCopyMode::LeftOnly => "_left_mono",
            ChannelCopyMode::RightOnly => "_right_mono",
        };
        let out_filename = format_sample_filename(base_name, start, duration, mode_suffix, "wav");
        let out_path = staging_dir.join(out_filename);
        let region = self
            .active_region_edit()
            .unwrap_or_else(|| self.region_edit(start, end, 0.0, None, None));

        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-ss")
            .arg(format!("{:.4}", start))
            .arg("-to")
            .arg(format!("{:.4}", end))
            .arg("-i")
            .arg(&self.path);

        if let Some(filter) = Self::channel_edit_filter(mode, region) {
            cmd.arg("-af").arg(filter);
        }
        cmd.arg("-c:a").arg("pcm_s16le");

        cmd.arg(&out_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Ok(s) = cmd.status() {
            if s.success() && out_path.exists() {
                let req = IpcRequest::PushFile {
                    path: out_path.clone(),
                    hand: little_clip_core::HandSide::Left,
                };
                let ipc_sent = crate::send_ipc_request(&req).is_ok();

                // If the daemon is unavailable, use the shared local clipboard adapter.
                if !ipc_sent {
                    if let Err(error) = little_clip_core::copy_file_uris_to_system_clipboard(
                        std::slice::from_ref(&out_path),
                    ) {
                        tracing::debug!(%error, "failed to copy audio slice to system clipboard");
                    }
                }

                let mode_desc = match mode {
                    ChannelCopyMode::Native => "slice",
                    ChannelCopyMode::LeftOnly => "LEFT (mono) slice",
                    ChannelCopyMode::RightOnly => "RIGHT (mono) slice",
                };

                self.status_toast = Some((
                    format!(
                        "📋 Copied {} ({:02}:{:04.1} - {:02}:{:04.1}, {:.3}s) to Clipboard & Left Hand!",
                        mode_desc,
                        (start / 60.0) as u32,
                        start % 60.0,
                        (end / 60.0) as u32,
                        end % 60.0,
                        duration
                    ),
                    now,
                ));
            } else {
                self.status_toast = Some((
                    "⚠️ Failed to extract audio segment via ffmpeg".to_string(),
                    now,
                ));
            }
        } else {
            self.status_toast = Some(("⚠️ ffmpeg process failed to execute".to_string(), now));
        }
    }

    pub fn copy_all_multi_segments(&mut self, now: f64, mode: ChannelCopyMode) {
        if self.multi_selections.is_empty() {
            self.copy_selected_segment(now, mode);
            return;
        }

        let base_name = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");

        let staging_dir = std::env::temp_dir().join("little_clip_audio_segments");
        if let Err(err) = std::fs::create_dir_all(&staging_dir) {
            tracing::warn!(
                "Failed to create audio staging directory {}: {err}",
                staging_dir.display()
            );
            self.status_toast = Some(("Failed to create audio staging directory".into(), now));
            return;
        }

        let mode_suffix = match mode {
            ChannelCopyMode::Native => "",
            ChannelCopyMode::LeftOnly => "_left_mono",
            ChannelCopyMode::RightOnly => "_right_mono",
        };

        let mut out_paths: Vec<PathBuf> = Vec::new();

        for (i, &(start, end)) in self.multi_selections.iter().enumerate() {
            let (s, e) = if start < end {
                (start, end)
            } else {
                (end, start)
            };
            let duration = (e - s).max(0.001);
            let out_filename = format_sample_filename(base_name, s, duration, mode_suffix, "wav");
            let out_path = staging_dir.join(out_filename);
            let region = self.region_edit(
                s,
                e,
                self.region_gain_db.get(i).copied().unwrap_or(0.0),
                self.region_low_pass_hz.get(i).copied().flatten(),
                self.region_high_pass_hz.get(i).copied().flatten(),
            );

            let mut cmd = std::process::Command::new("ffmpeg");
            cmd.arg("-y")
                .arg("-ss")
                .arg(format!("{:.4}", s))
                .arg("-to")
                .arg(format!("{:.4}", e))
                .arg("-i")
                .arg(&self.path);

            if let Some(filter) = Self::channel_edit_filter(mode, region) {
                cmd.arg("-af").arg(filter);
            }
            cmd.arg("-c:a").arg("pcm_s16le");

            cmd.arg(&out_path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            if let Ok(s) = cmd.status() {
                if s.success() && out_path.exists() {
                    out_paths.push(out_path);
                }
            }
        }

        if !out_paths.is_empty() {
            // Push all files as a single batch into Left Hand (1 item, 1 hand!)
            let req = if out_paths.len() == 1 {
                IpcRequest::PushFile {
                    path: out_paths[0].clone(),
                    hand: little_clip_core::HandSide::Left,
                }
            } else {
                IpcRequest::PushFiles {
                    paths: out_paths.clone(),
                    hand: little_clip_core::HandSide::Left,
                }
            };
            let ipc_sent = crate::send_ipc_request(&req).is_ok();

            // If the daemon is unavailable, use the shared local clipboard adapter.
            if !ipc_sent {
                if let Err(error) = little_clip_core::copy_file_uris_to_system_clipboard(&out_paths)
                {
                    tracing::debug!(%error, "failed to copy audio slices to system clipboard");
                }
            }

            let mode_desc = match mode {
                ChannelCopyMode::Native => "",
                ChannelCopyMode::LeftOnly => " LEFT (mono)",
                ChannelCopyMode::RightOnly => " RIGHT (mono)",
            };

            self.status_toast = Some((
                format!(
                    "📋 Copied {} audio{} slices to Left Hand (1 batch) & Clipboard!",
                    out_paths.len(),
                    mode_desc
                ),
                now,
            ));
        } else {
            self.status_toast = Some((
                "⚠️ Failed to extract multi-region slices via ffmpeg".to_string(),
                now,
            ));
        }
    }

    pub fn reset_save_filenames(&mut self) {
        let base_name = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("audio");

        let ext = self
            .path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| "wav".to_string());
        let export_ext = if matches!(ext.as_str(), "wav" | "flac" | "mp3" | "ogg") {
            ext.as_str()
        } else {
            "wav"
        };

        let slices: Vec<(f32, f32)> = match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
            SelectionMode::Multi => {
                if !self.multi_selections.is_empty() {
                    self.multi_selections
                        .iter()
                        .map(|&(s, e)| if s < e { (s, e) } else { (e, s) })
                        .collect()
                } else if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
        };

        self.save_filenames = slices
            .into_iter()
            .map(|(s, e)| {
                let dur = (e - s).max(0.001);
                format_sample_filename(base_name, s, dur, "", export_ext)
            })
            .collect();
    }

    pub fn open_save_dialog(&mut self) {
        if self.save_destination_dir.is_empty() {
            self.save_destination_dir = self
                .path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string());
        }
        self.reset_save_filenames();
        self.save_dialog_open = true;
    }

    pub fn start_browse_directory(&mut self) {
        let (tx, rx) = channel();
        self.browse_rx = Some(rx);
        let start_dir = self.save_destination_dir.clone();
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("zenity");
            cmd.arg("--file-selection")
                .arg("--directory")
                .arg("--title=Select Destination Folder");
            if !start_dir.is_empty() && std::path::Path::new(&start_dir).is_dir() {
                cmd.arg(format!("--filename={}/", start_dir));
            }
            if let Ok(output) = cmd.output() {
                if output.status.success() {
                    let picked = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !picked.is_empty() {
                        let _ = tx.send(Some(picked));
                        return;
                    }
                }
            }
            let _ = tx.send(None);
        });
    }

    pub fn execute_save(&mut self, now: f64) -> Result<usize, String> {
        let dest_dir = PathBuf::from(self.save_destination_dir.trim());
        if !dest_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&dest_dir) {
                let err_msg = format!("⚠️ Cannot create destination directory: {}", e);
                self.status_toast = Some((err_msg.clone(), now));
                return Err(err_msg);
            }
        }

        let slices: Vec<(f32, f32)> = match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
            SelectionMode::Multi => {
                if !self.multi_selections.is_empty() {
                    self.multi_selections
                        .iter()
                        .map(|&(s, e)| if s < e { (s, e) } else { (e, s) })
                        .collect()
                } else if let Some((s, e)) = self.selection {
                    vec![if s < e { (s, e) } else { (e, s) }]
                } else {
                    vec![(0.0, self.duration_sec)]
                }
            }
        };

        let mut saved_paths: Vec<PathBuf> = Vec::new();

        for (i, &(start, end)) in slices.iter().enumerate() {
            let mut filename = self
                .save_filenames
                .get(i)
                .cloned()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    let base_name = self
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("audio");
                    let dur = (end - start).max(0.001);
                    format_sample_filename(base_name, start, dur, "", "wav")
                });

            if !filename.contains('.') {
                filename.push_str(".wav");
            }

            let out_path = dest_dir.join(&filename);

            let mut cmd = std::process::Command::new("ffmpeg");
            cmd.arg("-y")
                .arg("-ss")
                .arg(format!("{:.4}", start))
                .arg("-to")
                .arg(format!("{:.4}", end))
                .arg("-i")
                .arg(&self.path);

            let gain_db = match self.selection_mode {
                SelectionMode::Single if self.selection.is_some() => self.single_region_gain_db,
                SelectionMode::Multi if !self.multi_selections.is_empty() => {
                    self.region_gain_db.get(i).copied().unwrap_or(0.0)
                }
                _ => 0.0,
            };
            let (low_pass_hz, high_pass_hz) = match self.selection_mode {
                SelectionMode::Single if self.selection.is_some() => {
                    (self.single_low_pass_hz, self.single_high_pass_hz)
                }
                SelectionMode::Multi if !self.multi_selections.is_empty() => (
                    self.region_low_pass_hz.get(i).copied().flatten(),
                    self.region_high_pass_hz.get(i).copied().flatten(),
                ),
                _ => (None, None),
            };
            let region = self.region_edit(start, end, gain_db, low_pass_hz, high_pass_hz);
            if let Some(filter) = Self::region_filter(region) {
                cmd.arg("-af").arg(filter);
            }

            let ext = out_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();

            if ext == "wav" {
                cmd.arg("-c:a").arg("pcm_s16le");
            } else if ext == "flac" {
                cmd.arg("-c:a").arg("flac");
            }

            cmd.arg(&out_path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            if let Ok(s) = cmd.status() {
                if s.success() && out_path.exists() {
                    saved_paths.push(out_path);
                }
            }
        }

        if !saved_paths.is_empty() {
            let count = saved_paths.len();
            let dest_name = dest_dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("folder");
            self.status_toast =
                Some((format!("💾 Saved {} file(s) to {}/", count, dest_name), now));

            if self.open_folder_after_save {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&dest_dir)
                    .spawn();
            }

            self.save_dialog_open = false;
            Ok(count)
        } else {
            let err_msg = "⚠️ Failed to save audio file(s) via ffmpeg".to_string();
            self.status_toast = Some((err_msg.clone(), now));
            Err(err_msg)
        }
    }

    pub fn quick_save(&mut self, now: f64) {
        if self.save_destination_dir.is_empty() {
            self.save_destination_dir = self
                .path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| ".".to_string());
        }
        self.reset_save_filenames();
        let _ = self.execute_save(now);
    }

    pub fn render_save_dialog(&mut self, ctx: &egui::Context, palette: &AppPalette, now: f64) {
        if !self.save_dialog_open {
            return;
        }

        let mut close_dialog = false;
        let mut trigger_save = false;

        egui::Window::new("💾 Save Audio Selection to File(s)")
            .collapsible(false)
            .resizable(true)
            .default_size(Vec2::new(580.0, 360.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.heading(
                    egui::RichText::new("Save Audio Slices to Disk")
                        .strong()
                        .color(hex_to_c32(&palette.harmonies.base)),
                );
                ui.label(
                    egui::RichText::new(
                        "Extract and save selections as individual audio files with sample timestamps & durations.",
                    )
                    .color(hex_to_c32(&palette.theme.f_med)),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // 1. Destination Directory
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📁 Destination:")
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_high)),
                    );
                    let text_width = (ui.available_width() - 90.0).max(100.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.save_destination_dir)
                            .desired_width(text_width),
                    );
                    if ui.add(layout::AudioWorkspace::secondary_button("Browse...", hex_to_c32(&palette.theme.b_low), palette))
                        .on_hover_text("Choose destination folder via file manager dialog").clicked() {
                        self.start_browse_directory();
                    }
                });

                ui.add_space(10.0);

                // 2. Filenames list
                let slice_count = self.save_filenames.len();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("📄 Files to Save ({}):", slice_count))
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_high)),
                    );
                    ui.add_space(8.0);
                    if ui.add(layout::AudioWorkspace::secondary_button("↺ Reset Names", hex_to_c32(&palette.theme.b_low), palette))
                        .on_hover_text("Reset filenames to default original_name + timestamp + duration").clicked() {
                        self.reset_save_filenames();
                    }
                });
                ui.label(
                    egui::RichText::new("Format: [OriginalName]_[SampleTimestamp]_[Duration]")
                        .small()
                        .color(hex_to_c32(&palette.theme.f_low)),
                );

                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for (idx, filename) in self.save_filenames.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("#{:02}:", idx + 1)).weak());
                                ui.add(
                                    egui::TextEdit::singleline(filename)
                                        .desired_width(ui.available_width() - 8.0),
                                );
                            });
                        }
                    });

                ui.add_space(10.0);
                ui.checkbox(
                    &mut self.open_folder_after_save,
                    "Open destination folder in file manager after saving",
                );

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let btn_text = if slice_count == 1 {
                        "💾 Save File [Enter]".to_string()
                    } else {
                        format!("💾 Save All {} Files [Enter]", slice_count)
                    };
                    let save_btn = layout::AudioWorkspace::primary_button(btn_text, hex_to_c32(&palette.diff_add), palette)
                        .rounding(Rounding::same(6.0));

                    let is_enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add(save_btn).clicked() || is_enter {
                        trigger_save = true;
                    }

                    if ui.add(layout::AudioWorkspace::secondary_button("Cancel [Esc]", hex_to_c32(&palette.theme.b_low), palette)).clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close_dialog = true;
                    }
                });
            });

        if trigger_save {
            let _ = self.execute_save(now);
        } else if close_dialog {
            self.save_dialog_open = false;
        }
    }

    fn poll_audio_analysis(&mut self, ctx: &egui::Context, now: f64) {
        if let Some(rx) = self.analysis_rx.take() {
            ctx.request_repaint_after(Duration::from_millis(50));
            let mut finished = false;
            loop {
                let progress = match rx.try_recv() {
                    Ok(progress) => progress,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        self.is_loading_analysis = false;
                        break;
                    }
                };
                match progress {
                    AudioAnalysisProgress::WaveformReady(box_analysis) => {
                        let analysis = *box_analysis;
                        self.duration_sec = analysis.duration_ms as f32 / 1000.0;
                        if self.view_duration_sec <= 0.05 {
                            self.view_duration_sec = self.duration_sec;
                        }
                        self.analysis = Some(analysis);
                        self.is_loading_analysis = false;
                    }
                    AudioAnalysisProgress::TransientsReady(transients) => {
                        if let Some(ref mut analysis) = self.analysis {
                            analysis.transients = transients;
                            analysis.is_transients_ready = true;
                        }
                    }
                    progress @ AudioAnalysisProgress::SpectrogramAndNotesReady { .. } => {
                        self.install_spectral_analysis(ctx, progress);
                    }
                    AudioAnalysisProgress::Complete => {
                        finished = true;
                        break;
                    }
                    AudioAnalysisProgress::Failed(error) => {
                        self.status_toast = Some((error, now));
                        self.is_loading_analysis = false;
                        finished = true;
                        break;
                    }
                }
            }
            if !finished {
                self.analysis_rx = Some(rx);
            }
        }
        self.poll_edit_analysis(ctx, now);
    }

    fn install_spectral_analysis(&mut self, ctx: &egui::Context, progress: AudioAnalysisProgress) {
        let AudioAnalysisProgress::SpectrogramAndNotesReady {
            spectrogram_width,
            spectrogram_height,
            spectrogram_rgba,
            note_strip_width,
            note_strip_height,
            note_strip_rgba,
            note_strip_info,
            spectral_colors,
        } = progress
        else {
            return;
        };
        let spec_img = egui::ColorImage::from_rgba_unmultiplied(
            [spectrogram_width, spectrogram_height],
            &spectrogram_rgba,
        );
        self.spectrogram_texture = Some(ctx.load_texture(
            format!("spectrogram:{}", self.path.display()),
            spec_img,
            egui::TextureOptions::LINEAR,
        ));
        let note_img = egui::ColorImage::from_rgba_unmultiplied(
            [note_strip_width, note_strip_height],
            &note_strip_rgba,
        );
        self.note_strip_texture = Some(ctx.load_texture(
            format!("note_strip:{}", self.path.display()),
            note_img,
            egui::TextureOptions::NEAREST,
        ));
        if let Some(analysis) = &mut self.analysis {
            analysis.spectrogram_width = spectrogram_width;
            analysis.spectrogram_height = spectrogram_height;
            analysis.spectrogram_rgba = spectrogram_rgba;
            analysis.note_strip_width = note_strip_width;
            analysis.note_strip_height = note_strip_height;
            analysis.note_strip_rgba = note_strip_rgba;
            analysis.note_strip_info = note_strip_info;
            analysis.spectral_colors = spectral_colors;
            analysis.is_spectrogram_ready = true;
        }
    }

    fn poll_edit_analysis(&mut self, ctx: &egui::Context, now: f64) {
        let regions = self.region_edits();
        if regions != self.edit_analysis_observed {
            self.edited_waveform = None;
            let old = &self.edit_analysis_observed;
            let filter_changed = old.len() != regions.len()
                && (old
                    .iter()
                    .any(|r| r.low_pass_hz.is_some() || r.high_pass_hz.is_some())
                    || regions
                        .iter()
                        .any(|r| r.low_pass_hz.is_some() || r.high_pass_hz.is_some()))
                || old.iter().zip(&regions).any(|(a, b)| {
                    a.low_pass_hz != b.low_pass_hz
                        || a.high_pass_hz != b.high_pass_hz
                        || ((a.start_sec != b.start_sec || a.end_sec != b.end_sec)
                            && (a.low_pass_hz.is_some()
                                || a.high_pass_hz.is_some()
                                || b.low_pass_hz.is_some()
                                || b.high_pass_hz.is_some()))
                });
            self.edit_analysis_debounce =
                Duration::from_millis(if filter_changed { 70 } else { 180 });
            if let Ok(mut playback_regions) = self.playback_edits.regions.write() {
                playback_regions.clone_from(&regions);
                self.playback_edits
                    .generation
                    .fetch_add(1, Ordering::Release);
            }
            self.edit_analysis_observed = regions;
            self.edit_change_at = Instant::now();
        }
        if let Some(rx) = self.edit_analysis_rx.take() {
            let mut finished = false;
            ctx.request_repaint_after(Duration::from_millis(50));
            loop {
                let progress = match rx.try_recv() {
                    Ok(progress) => progress,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        break;
                    }
                };
                match progress {
                    AudioAnalysisProgress::WaveformReady(analysis) => {
                        if self.edit_analysis_inflight == self.edit_analysis_observed {
                            self.edited_waveform = Some(*analysis);
                        }
                    }
                    progress @ AudioAnalysisProgress::SpectrogramAndNotesReady { .. } => {
                        if self.edit_analysis_inflight == self.edit_analysis_observed {
                            self.install_spectral_analysis(ctx, progress);
                            self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        }
                    }
                    AudioAnalysisProgress::Complete => {
                        finished = true;
                        break;
                    }
                    AudioAnalysisProgress::Failed(error) => {
                        self.status_toast = Some((error, now));
                        self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        finished = true;
                        break;
                    }
                    _ => {}
                }
            }
            if !finished {
                self.edit_analysis_rx = Some(rx);
            }
        }
        if self.edit_analysis_rx.is_none()
            && self.analysis_rx.is_none()
            && self.analysis.is_some()
            && self.edit_analysis_observed != self.edit_analysis_displayed
        {
            let debounce = self.edit_analysis_debounce;
            if self.edit_change_at.elapsed() < debounce {
                ctx.request_repaint_after(debounce - self.edit_change_at.elapsed());
            } else {
                let path = self.path.clone();
                let regions = self.edit_analysis_observed.clone();
                let has_filter = regions
                    .iter()
                    .any(|region| region.low_pass_hz.is_some() || region.high_pass_hz.is_some());
                self.edit_analysis_inflight = regions.clone();
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let send_progress = |progress| {
                        if matches!(
                            progress,
                            AudioAnalysisProgress::WaveformReady(_)
                                | AudioAnalysisProgress::SpectrogramAndNotesReady { .. }
                                | AudioAnalysisProgress::Complete
                                | AudioAnalysisProgress::Failed(_)
                        ) {
                            let _ = tx.send(progress);
                        }
                    };
                    if has_filter {
                        compute_audio_spectral_preview_with_edits(
                            &path,
                            1200,
                            &regions,
                            send_progress,
                        );
                    } else {
                        compute_audio_analysis_progressive_with_edits(
                            &path,
                            1200,
                            &regions,
                            send_progress,
                        );
                    }
                });
                self.edit_analysis_rx = Some(rx);
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    /// Renders shared studio chrome, then the selected audio tool workspace.
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        if !ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::I)) {
                self.active_mode = AudioToolMode::Inspect;
            }
            if ctx
                .input(|i| i.key_pressed(egui::Key::S) && !i.modifiers.command && !i.modifiers.ctrl)
            {
                self.active_mode = AudioToolMode::StemSeparation;
            }
            if ctx
                .input(|i| i.key_pressed(egui::Key::D) && !i.modifiers.command && !i.modifiers.ctrl)
            {
                self.active_mode = AudioToolMode::DigitalDecode;
            }
        }
        // Tool-level shortcuts must win over nested stem editors.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop();
            *close_requested = true;
            return;
        }
        if self.active_mode == AudioToolMode::StemSeparation
            && ctx.input(|i| i.key_pressed(egui::Key::Space))
            && self.stem_separation.stems.is_empty()
            && !self.stem_separation.in_progress
        {
            self.start_stem_separation();
        }
        layout::AudioWorkspace::show(ui, palette, |ui| {
            self.render_audio_header(ui, ctx, palette, close_requested);
            ui.add_space(4.0);
            self.render_audio_tool_bar(ui, palette);
            ui.add_space(8.0);
            match self.active_mode {
                AudioToolMode::Inspect => {
                    self.render_standard_ui(ui, ctx, palette, close_requested, None)
                }
                AudioToolMode::StemSeparation => {
                    self.render_stem_separation_ui(ui, ctx, palette, close_requested)
                }
                AudioToolMode::DigitalDecode => {
                    self.render_digital_decode_ui(ui, ctx, palette, close_requested)
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::audio::compute_full_audio_analysis;

    #[test]
    fn region_gain_reaches_silence_recovers_and_caps_at_100x() {
        assert_eq!(AudioPreviewState::step_gain_db(0.0, 100.0), 40.0);
        assert_eq!(
            AudioPreviewState::step_gain_db(-60.0, -1.0),
            f32::NEG_INFINITY
        );
        assert_eq!(
            AudioPreviewState::step_gain_db(f32::NEG_INFINITY, 1.0),
            -60.0
        );
        assert_eq!(
            AudioPreviewState::gain_filter(f32::NEG_INFINITY, false).as_deref(),
            Some("volume=0")
        );
        assert_eq!(
            AudioPreviewState::gain_filter(40.0, true).as_deref(),
            Some("volume=40dB,asoftclip=type=tanh")
        );
        assert_eq!(
            AudioPreviewState::gain_filter(1.0, false).as_deref(),
            Some("volume=1dB")
        );
    }

    #[test]
    fn middle_click_reset_targets_only_the_region_under_pointer() {
        let mut state = AudioPreviewState::new(
            PathBuf::from("/__little_clip_missing_middle_reset__.wav"),
            "test".to_string(),
        );
        state.selection = Some((1.0, 2.0));
        state.single_region_gain_db = f32::NEG_INFINITY;
        assert!(!state.focus_region_at_time(3.0));
        assert_eq!(state.single_region_gain_db, f32::NEG_INFINITY);
        assert!(state.focus_region_at_time(1.5));
        assert!(state.reset_active_region_gain());
        assert_eq!(state.single_region_gain_db, 0.0);

        state.selection_mode = SelectionMode::Multi;
        state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
        state.region_gain_db = vec![6.0, f32::NEG_INFINITY];
        state.active_multi_index = Some(0);
        assert!(state.focus_region_at_time(3.5));
        assert_eq!(state.active_multi_index, Some(1));
        assert!(state.reset_active_region_gain());
        assert_eq!(state.region_gain_db, vec![6.0, 0.0]);
    }

    #[test]
    fn spectrum_cutoffs_step_logarithmically_and_reset_independently() {
        let mut state = AudioPreviewState::new(
            PathBuf::from("/__little_clip_missing_filter_test__.wav"),
            "test".to_string(),
        );
        state.duration_sec = 10.0;
        state.selection = Some((1.0, 3.0));
        state.adjust_active_region_filter(SpectrumFilterKind::LowPass, -4.0, Instant::now());
        state.adjust_active_region_filter(SpectrumFilterKind::HighPass, 4.0, Instant::now());
        assert_eq!(state.single_low_pass_hz, Some(6_000.0));
        assert_eq!(state.single_high_pass_hz, Some(240.0));
        let filter = state.session_edit_filter().unwrap();
        assert!(filter.contains("lowpass=f=6000.000:p=2:enable='between(t,1.000000,3.000000)'"));
        assert!(filter.contains("highpass=f=240.000:p=2:enable='between(t,1.000000,3.000000)'"));
        assert!(state.reset_active_region_filter(SpectrumFilterKind::LowPass));
        assert_eq!(state.single_low_pass_hz, None);
        assert_eq!(state.single_high_pass_hz, Some(240.0));

        state.set_selection_mode(SelectionMode::Multi);
        state.multi_selections.push((4.0, 5.0));
        state.region_low_pass_hz.push(Some(2_000.0));
        state.region_high_pass_hz.push(None);
        state.region_gain_db.push(0.0);
        assert!(state.focus_region_at_time(4.5));
        assert!(state.reset_active_region_filter(SpectrumFilterKind::LowPass));
        assert_eq!(state.region_low_pass_hz, vec![None, None]);
        assert_eq!(state.region_high_pass_hz, vec![Some(240.0), None]);
    }

    #[test]
    fn gain_and_filter_edits_undo_and_redo_in_order() {
        let mut state = AudioPreviewState::new(
            PathBuf::from("/__little_clip_missing_undo_test__.wav"),
            "test".to_string(),
        );
        state.selection = Some((1.0, 2.0));
        state.adjust_active_region_gain(3.0, Instant::now());
        state.adjust_active_region_filter(SpectrumFilterKind::LowPass, -4.0, Instant::now());
        assert!(state.reset_active_region_gain());
        assert_eq!(state.undo_stack.len(), 3);

        state.undo();
        assert_eq!(state.single_region_gain_db, 3.0);
        state.undo();
        assert_eq!(state.single_low_pass_hz, None);
        state.undo();
        assert_eq!(state.single_region_gain_db, 0.0);
        state.redo();
        state.redo();
        assert_eq!(state.single_region_gain_db, 3.0);
        assert_eq!(state.single_low_pass_hz, Some(6_000.0));

        state.adjust_active_region_filter(SpectrumFilterKind::HighPass, 4.0, Instant::now());
        assert!(state.redo_stack.is_empty());
        state.undo();
        assert_eq!(state.single_high_pass_hz, None);
        assert_eq!(state.single_low_pass_hz, Some(6_000.0));
    }

    #[test]
    fn rapid_gain_scrolls_share_one_undo_step_until_the_gap_exceeds_300_ms() {
        let mut state = AudioPreviewState::new(
            PathBuf::from("/__little_clip_missing_scroll_undo__.wav"),
            "test".to_string(),
        );
        state.selection = Some((1.0, 2.0));
        let start = Instant::now();
        state.adjust_active_region_gain(1.0, start);
        state.adjust_active_region_gain(1.0, start + Duration::from_millis(200));
        state.adjust_active_region_gain(1.0, start + Duration::from_millis(500));
        assert_eq!(state.undo_stack.len(), 1);
        assert_eq!(state.single_region_gain_db, 3.0);

        state.adjust_active_region_gain(1.0, start + Duration::from_millis(801));
        assert_eq!(state.undo_stack.len(), 2);
        state.undo();
        assert_eq!(state.single_region_gain_db, 3.0);
        state.undo();
        assert_eq!(state.single_region_gain_db, 0.0);
        state.adjust_active_region_gain(2.0, start + Duration::from_millis(900));
        assert_eq!(state.undo_stack.len(), 1);
        assert!(state.redo_stack.is_empty());
        state.undo();
        assert_eq!(state.single_region_gain_db, 0.0);
    }

    #[test]
    fn scroll_history_separates_filter_types_and_slices() {
        let mut state = AudioPreviewState::new(
            PathBuf::from("/__little_clip_missing_scroll_targets__.wav"),
            "test".to_string(),
        );
        state.selection_mode = SelectionMode::Multi;
        state.multi_selections = vec![(1.0, 2.0), (3.0, 4.0)];
        state.active_multi_index = Some(0);
        let start = Instant::now();
        state.adjust_active_region_filter(SpectrumFilterKind::LowPass, -1.0, start);
        state.adjust_active_region_filter(
            SpectrumFilterKind::LowPass,
            -1.0,
            start + Duration::from_millis(100),
        );
        state.adjust_active_region_filter(
            SpectrumFilterKind::HighPass,
            1.0,
            start + Duration::from_millis(150),
        );
        state.active_multi_index = Some(1);
        state.adjust_active_region_filter(
            SpectrumFilterKind::HighPass,
            1.0,
            start + Duration::from_millis(200),
        );
        assert_eq!(state.undo_stack.len(), 3);
        state.undo();
        assert_eq!(state.active_multi_index, Some(1));
        assert_eq!(state.region_high_pass_hz[1], None);
        assert!(state.region_high_pass_hz[0].is_some());
        state.undo();
        assert_eq!(state.active_multi_index, Some(0));
        assert_eq!(state.region_high_pass_hz.first().copied().flatten(), None);
        assert!(state.region_low_pass_hz[0].is_some());
        state.undo();
        assert_eq!(state.region_low_pass_hz.first().copied().flatten(), None);
    }

    #[test]
    fn deleting_a_region_restores_the_previous_session_on_undo() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.wav");
        let samples = vec![0_u8; 16_000];
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36_u32 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&8_000_u32.to_le_bytes());
        wav.extend_from_slice(&16_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        std::fs::write(&original, wav).unwrap();

        let mut state = AudioPreviewState::new(original.clone(), "test".to_string());
        state.duration_sec = 1.0;
        state.selection = Some((0.25, 0.5));
        state.adjust_active_region_gain(6.0, Instant::now());
        state.delete_active_slice_from_session(0.0);
        let edited = state.path.clone();
        assert_ne!(edited, original);
        assert!(edited.is_file());
        assert_eq!(state.duration_sec, 0.75);
        state.undo();
        assert_eq!(state.path, original);
        assert_eq!(state.selection, Some((0.25, 0.5)));
        assert_eq!(state.single_region_gain_db, 6.0);
        assert_eq!(state.duration_sec, 1.0);
        state.redo();
        assert_eq!(state.path, edited);
        assert_eq!(state.selection, None);
        assert_eq!(state.duration_sec, 0.75);
        let _ = std::fs::remove_file(edited);
    }

    #[test]
    fn sorting_and_removing_multi_regions_keeps_their_effects_attached() {
        let mut state = AudioPreviewState::new(
            PathBuf::from("/__little_clip_missing_region_sort__.wav"),
            "test".to_string(),
        );
        state.multi_selections = vec![(4.0, 5.0), (1.0, 2.0)];
        state.region_gain_db = vec![6.0, -3.0];
        state.region_low_pass_hz = vec![Some(2_000.0), None];
        state.region_high_pass_hz = vec![None, Some(200.0)];
        state.active_multi_index = Some(0);
        state.sort_multi_regions();
        assert_eq!(state.multi_selections, vec![(1.0, 2.0), (4.0, 5.0)]);
        assert_eq!(state.region_gain_db, vec![-3.0, 6.0]);
        assert_eq!(state.region_low_pass_hz, vec![None, Some(2_000.0)]);
        assert_eq!(state.region_high_pass_hz, vec![Some(200.0), None]);
        assert_eq!(state.active_multi_index, Some(1));
        state.remove_multi_region(0);
        assert_eq!(state.multi_selections, vec![(4.0, 5.0)]);
        assert_eq!(state.region_gain_db, vec![6.0]);
        assert_eq!(state.region_low_pass_hz, vec![Some(2_000.0)]);
        assert_eq!(state.active_multi_index, Some(0));
    }

    #[test]
    fn playback_reads_updated_session_gain_without_restarting_source() {
        let shared = Arc::<PlaybackEdits>::default();
        *shared.regions.write().unwrap() = vec![AudioRegionEdit {
            start_sec: 0.0,
            end_sec: 1.0,
            gain_db: f32::NEG_INFINITY,
            soft_clip: false,
            low_pass_hz: None,
            high_pass_hz: None,
        }];
        shared.generation.fetch_add(1, Ordering::Release);
        let source = rodio::buffer::SamplesBuffer::new(
            std::num::NonZeroU16::new(1).unwrap(),
            std::num::NonZeroU32::new(4).unwrap(),
            vec![0.5; 8],
        );
        let mut preview = EditedPreviewSource::new(source, Arc::clone(&shared));
        assert_eq!(preview.next(), Some(0.0));
        shared.regions.write().unwrap()[0].gain_db = 0.0;
        shared.generation.fetch_add(1, Ordering::Release);
        assert_eq!(preview.next(), Some(0.5));
    }

    #[test]
    fn audio_tool_modes_have_stable_toolbar_labels() {
        assert_eq!(AudioToolMode::Inspect.label(), "[I]nspect & Slice");
        assert_eq!(AudioToolMode::StemSeparation.label(), "[S]tem Separation");
        assert_eq!(AudioToolMode::DigitalDecode.label(), "[D]igital Decode");
        assert_eq!(AudioToolMode::default(), AudioToolMode::Inspect);
    }

    #[test]
    fn stem_manifest_parser_ignores_worker_log_lines() {
        let output = br#"Downloading model...
Separated track
{"vocals":"/tmp/vocals.wav","drums":"/tmp/drums.wav","bass":"/tmp/bass.wav","other":"/tmp/other.wav"}
"#;
        let stems = parse_stem_manifest(output).expect("manifest should parse");
        assert_eq!(stems.len(), 4);
        assert_eq!(
            stems[0],
            ("vocals".to_string(), PathBuf::from("/tmp/vocals.wav"))
        );
    }

    #[test]
    fn stem_separation_state_starts_without_outputs() {
        let state = StemSeparationState::default();
        assert!(!state.in_progress);
        assert!(state.stems.is_empty());
        assert!(state.workspace.is_none());
        assert!(state.solo_stem.is_none());
    }

    #[test]
    fn grouped_stem_routing_respects_mute_and_solo() {
        assert!(stem_is_audible(0, false, None));
        assert!(!stem_is_audible(0, true, None));
        assert!(stem_is_audible(2, false, Some(2)));
        assert!(!stem_is_audible(1, false, Some(2)));
        assert!(!stem_is_audible(2, true, Some(2)));
    }

    #[test]
    fn switching_editor_stems_identifies_the_previous_player_to_stop() {
        assert_eq!(editor_stem_switch(0, 2, 4), Some((0, 2)));
        assert_eq!(editor_stem_switch(2, 2, 4), None);
        assert_eq!(editor_stem_switch(2, 4, 4), None);
    }

    #[test]
    fn grouped_waveform_scrubbing_maps_and_clamps_time() {
        assert_eq!(waveform_scrub_time(50.0, 0.0, 100.0, 240.0), 120.0);
        assert_eq!(waveform_scrub_time(-10.0, 0.0, 100.0, 240.0), 0.0);
        assert_eq!(waveform_scrub_time(150.0, 0.0, 100.0, 240.0), 240.0);
    }

    #[test]
    fn transient_sensitivity_slider_is_nonlinear_around_the_default() {
        let tolerance = 0.000_01;
        assert!((transient_sensitivity_from_slider(0.0) - 0.2).abs() < tolerance);
        assert!((transient_sensitivity_from_slider(0.5) - 0.35).abs() < tolerance);
        assert!((transient_sensitivity_from_slider(1.0) - 1.0).abs() < tolerance);

        for sensitivity in [0.2, 0.25, 0.35, 0.6, 1.0] {
            let round_trip =
                transient_sensitivity_from_slider(transient_sensitivity_to_slider(sensitivity));
            assert!((round_trip - sensitivity).abs() < tolerance);
        }
    }

    #[test]
    fn audacity_stem_list_imports_each_file_as_a_track() {
        let paths = vec![
            PathBuf::from("/tmp/stems/vocals.wav"),
            PathBuf::from("/tmp/stems/drums.wav"),
            PathBuf::from("/tmp/stems/bass.wav"),
            PathBuf::from("/tmp/stems/other.wav"),
        ];
        assert_eq!(
            audacity_lof_contents(&paths).unwrap(),
            "file \"/tmp/stems/vocals.wav\"\n\
             file \"/tmp/stems/drums.wav\"\n\
             file \"/tmp/stems/bass.wav\"\n\
             file \"/tmp/stems/other.wav\"\n"
        );
    }

    #[test]
    fn isolated_audacity_profile_is_writable_and_skips_effect_scanning() {
        let workspace = tempfile::tempdir().unwrap();
        let stems = vec![PathBuf::from("/tmp/stems/vocals.wav")];
        let mut command = std::process::Command::new("audacity");
        prepare_isolated_audacity_command(&mut command, workspace.path(), &stems).unwrap();

        let config = std::fs::read_to_string(
            workspace
                .path()
                .join("audacity-profile/config/audacity/audacity.cfg"),
        )
        .unwrap();
        assert!(config.contains("SkipEffectsScanAtStartup=1"));
        assert!(workspace.path().join("separated-stems.lof").is_file());

        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            environment.get("XDG_CONFIG_HOME"),
            Some(
                &workspace
                    .path()
                    .join("audacity-profile/config")
                    .to_string_lossy()
                    .into_owned()
            ),
        );
        assert_eq!(
            environment.get("VST_PATH"),
            Some(
                &workspace
                    .path()
                    .join("audacity-profile/empty-plugins")
                    .to_string_lossy()
                    .into_owned()
            ),
        );
    }

    fn generate_fixture_wav(name: &str, duration_secs: f32) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write;
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join(format!("{name}.wav"));
        let mut file = std::fs::File::create(&path).unwrap();
        let sample_rate = 44100u32;
        let num_samples = (sample_rate as f32 * duration_secs) as usize;
        let num_channels = 2u16;
        let bits_per_sample = 16u16;

        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_len.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&num_channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&block_align.to_le_bytes()).unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();

        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let s1 = ((t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 8000.0) as i16;
            let s2 = ((t * 880.0 * 2.0 * std::f32::consts::PI).sin() * 8000.0) as i16;
            file.write_all(&s1.to_le_bytes()).unwrap();
            file.write_all(&s2.to_le_bytes()).unwrap();
        }
        (temp_dir, path)
    }

    #[test]
    fn test_audio_preview_state_creation_and_seek() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        assert!(state.duration_sec >= 4.0);
        assert!(state.metadata.is_some());

        // Test seek
        state.seek_to(2.0);
        assert_eq!(state.current_time_sec, 2.0);

        // Test selection
        state.selection = Some((1.0, 3.0));
        assert_eq!(state.selection, Some((1.0, 3.0)));

        // Test segment copy extraction (native stereo)
        state.copy_selected_segment(1.0, ChannelCopyMode::Native);
        assert!(state.status_toast.is_some());
        let (toast, _) = state.status_toast.unwrap();
        assert!(toast.contains("Copied slice"));
    }

    #[test]
    fn test_zoom_and_pan_calculations() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        let init_dur = state.view_duration_sec;

        // Zoom In
        state.zoom(0.5, 2.0);
        assert!(state.view_duration_sec < init_dur);
        assert!(state.view_start_sec >= 0.0);

        // Pan right
        let s1 = state.view_start_sec;
        state.pan(0.5);
        assert!(state.view_start_sec >= s1);

        // Fit to window
        state.fit_to_window();
        assert_eq!(state.view_start_sec, 0.0);
        assert_eq!(state.view_duration_sec, state.duration_sec);
    }

    #[test]
    fn test_channel_slicing_modes() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        state.selection = Some((0.5, 1.5));

        // Test Left Only
        state.copy_selected_segment(1.0, ChannelCopyMode::LeftOnly);
        assert!(state.status_toast.is_some());
        let (toast, _) = state.status_toast.as_ref().unwrap();
        assert!(toast.contains("LEFT (mono)"));

        // Test Right Only
        state.copy_selected_segment(2.0, ChannelCopyMode::RightOnly);
        assert!(state.status_toast.is_some());
        let (toast, _) = state.status_toast.as_ref().unwrap();
        assert!(toast.contains("RIGHT (mono)"));
    }

    #[test]
    fn test_small_sub_50ms_slice_selection() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        state.selection = Some((1.000, 1.020));
        state.copy_selected_segment(1.0, ChannelCopyMode::Native);
        assert!(state.status_toast.is_some());
        let (toast, _) = state.status_toast.unwrap();
        assert!(toast.contains("Copied slice"));
        assert!(toast.contains("0.020s"));
    }

    #[test]
    fn test_mono_and_multichannel_handling() {
        use std::io::Write;
        let temp_dir = std::env::temp_dir();

        // 1. Generate Mono WAV
        let mono_path = temp_dir.join(format!("test_mono_{}.wav", uuid::Uuid::new_v4()));
        {
            let mut f = std::fs::File::create(&mono_path).unwrap();
            let sample_rate = 44100u32;
            let num_samples = 4410usize; // 0.1s
            let num_channels = 1u16;
            let bits = 16u16;
            let data_len = (num_samples * num_channels as usize * 2) as u32;
            let riff_len = 36 + data_len;
            f.write_all(b"RIFF").unwrap();
            f.write_all(&riff_len.to_le_bytes()).unwrap();
            f.write_all(b"WAVE").unwrap();
            f.write_all(b"fmt ").unwrap();
            f.write_all(&16u32.to_le_bytes()).unwrap();
            f.write_all(&1u16.to_le_bytes()).unwrap();
            f.write_all(&num_channels.to_le_bytes()).unwrap();
            f.write_all(&sample_rate.to_le_bytes()).unwrap();
            f.write_all(&(sample_rate * 2).to_le_bytes()).unwrap();
            f.write_all(&2u16.to_le_bytes()).unwrap();
            f.write_all(&bits.to_le_bytes()).unwrap();
            f.write_all(b"data").unwrap();
            f.write_all(&data_len.to_le_bytes()).unwrap();
            for _ in 0..num_samples {
                f.write_all(&1000i16.to_le_bytes()).unwrap();
            }
        }

        let mono_analysis = compute_full_audio_analysis(&mono_path, 50).expect("analyze mono");
        assert_eq!(mono_analysis.channels, 1);
        assert!(mono_analysis.ch2_samples.is_none());
        assert!(!mono_analysis.ch1_samples.is_empty());
        let _ = std::fs::remove_file(&mono_path);

        // 2. Generate 6-Channel (5.1 Surround) WAV
        let surround_path = temp_dir.join(format!("test_surround_{}.wav", uuid::Uuid::new_v4()));
        {
            let mut f = std::fs::File::create(&surround_path).unwrap();
            let sample_rate = 44100u32;
            let num_samples = 4410usize; // 0.1s
            let num_channels = 6u16;
            let bits = 16u16;
            let data_len = (num_samples * num_channels as usize * 2) as u32;
            let riff_len = 36 + data_len;
            f.write_all(b"RIFF").unwrap();
            f.write_all(&riff_len.to_le_bytes()).unwrap();
            f.write_all(b"WAVE").unwrap();
            f.write_all(b"fmt ").unwrap();
            f.write_all(&16u32.to_le_bytes()).unwrap();
            f.write_all(&1u16.to_le_bytes()).unwrap();
            f.write_all(&num_channels.to_le_bytes()).unwrap();
            f.write_all(&sample_rate.to_le_bytes()).unwrap();
            f.write_all(&(sample_rate * num_channels as u32 * 2).to_le_bytes())
                .unwrap();
            f.write_all(&(num_channels * 2).to_le_bytes()).unwrap();
            f.write_all(&bits.to_le_bytes()).unwrap();
            f.write_all(b"data").unwrap();
            f.write_all(&data_len.to_le_bytes()).unwrap();
            for _ in 0..num_samples {
                for _ch in 0..6 {
                    f.write_all(&1200i16.to_le_bytes()).unwrap();
                }
            }
        }

        let surround_analysis =
            compute_full_audio_analysis(&surround_path, 50).expect("analyze 5.1");
        assert_eq!(surround_analysis.channels, 6);
        assert!(surround_analysis.channels > 2); // Triggers warning badge in UI
        assert!(surround_analysis.ch2_samples.is_some()); // Displays L and R
        let _ = std::fs::remove_file(&surround_path);
    }

    #[test]
    fn test_multi_region_mode_and_removal() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        assert_eq!(state.selection_mode, SelectionMode::Single);
        assert!(state.multi_selections.is_empty());

        // Single selection default
        state.selection = Some((1.0, 2.0));
        assert_eq!(state.active_selection_range(), Some((1.0, 2.0)));
        assert!(state.is_time_selected(1.5));
        assert!(!state.is_time_selected(2.5));

        // Switch to Multi-Region Mode
        state.set_selection_mode(SelectionMode::Multi);
        assert_eq!(state.selection_mode, SelectionMode::Multi);
        // Preserves the existing selection into multi_selections
        assert_eq!(state.multi_selections.len(), 1);
        assert_eq!(state.multi_selections[0], (1.0, 2.0));

        // Add second region
        state.multi_selections.push((2.5, 3.5));
        assert_eq!(state.multi_selections.len(), 2);
        assert!(state.is_time_selected(1.5));
        assert!(state.is_time_selected(3.0));
        assert!(!state.is_time_selected(2.2));

        // Test multi-region batch extraction
        state.copy_all_multi_segments(1.0, ChannelCopyMode::Native);
        assert!(state.status_toast.is_some());
        let (toast, _) = state.status_toast.as_ref().unwrap();
        assert!(toast.contains("Copied 2 audio slices"));

        // Remove a region
        state.multi_selections.remove(0);
        assert_eq!(state.multi_selections.len(), 1);
        assert_eq!(state.multi_selections[0], (2.5, 3.5));
        assert!(!state.is_time_selected(1.5));
        assert!(state.is_time_selected(3.0));

        // Switch back to Single Mode: keeps first multi-region as single selection
        state.set_selection_mode(SelectionMode::Single);
        assert_eq!(state.selection_mode, SelectionMode::Single);
        assert_eq!(state.selection, Some((2.5, 3.5)));
    }

    #[test]
    fn test_zero_crossing_snapping_integration() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        assert!(!state.snap_zero_crossings);

        state.selection = Some((1.0123, 2.4567));
        // Toggle snap mode
        state.snap_zero_crossings = true;
        assert!(state.snap_zero_crossings);

        let analysis = compute_full_audio_analysis(&p, 50).expect("analysis");
        state.analysis = Some(analysis);

        let snapped_s = state.snap_time(1.0123, false);
        let snapped_e = state.snap_time(2.4567, false);
        // Snapped values should be valid timestamps
        assert!(snapped_s >= 0.0 && snapped_s <= state.duration_sec);
        assert!(snapped_e >= 0.0 && snapped_e <= state.duration_sec);

        // Trigger instant snap on active selection
        state.snap_active_selection_to_zero_crossings(1.0);
        assert!(state.status_toast.is_some());
        let (toast, _) = state.status_toast.as_ref().unwrap();
        assert!(toast.contains("Snapped selection to 0-crossings"));
    }

    #[test]
    fn test_transient_toggle_and_snapping_integration() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        assert!(!state.show_transients);

        // Toggle transients ON
        state.show_transients = true;
        assert!(state.show_transients);

        let mut analysis = compute_full_audio_analysis(&p, 50).expect("analysis");
        // Add a synthetic transient if empty to verify snapping
        if analysis.transients.is_empty() {
            analysis.transients.push(1.234);
        }
        state.analysis = Some(analysis.clone());

        let target_t = analysis.transients[0];
        // Test snapping near the transient (within ~5ms)
        let test_t = target_t + 0.005;
        let snapped = state.snap_time(test_t, false);
        assert_eq!(snapped, target_t);

        // With both modes enabled, choose the transient first and then refine it
        // to the nearest zero crossing rather than snapping around the pointer.
        state.snap_zero_crossings = true;
        let expected_crossing = analysis.snap_to_zero_crossing(target_t, 500);
        assert_eq!(state.snap_time(test_t, false), expected_crossing);

        // Holding Shift or Ctrl bypasses snapping even when both modes are ON
        let bypassed = state.snap_time(test_t, true);
        assert_eq!(bypassed, test_t);

        // Toggle transients OFF -> should not snap to transient
        state.show_transients = false;
        let zero_crossing_only = state.snap_time(test_t, false);
        assert_eq!(
            zero_crossing_only,
            analysis.snap_to_zero_crossing(test_t, 500),
        );
    }

    #[test]
    fn test_middle_mouse_pan_calculation() {
        let (_dir, p) = generate_fixture_wav("fixture", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "fixture.wav".to_string());
        state.view_duration_sec = 2.0;
        state.view_start_sec = 1.0;
        state.selection = Some((1.2, 1.8));

        // Simulate middle mouse pan drag:
        // start_x = 500.0, current_x = 400.0 (mouse moved 100px left, meaning viewing future audio)
        let track_width = 1000.0;
        let start_x = 500.0;
        let cur_x = 400.0;
        let initial_start_sec = 1.0;

        let dx = cur_x - start_x; // -100.0
        let dt = (dx / track_width) * state.view_duration_sec; // (-100/1000) * 2.0 = -0.2
        let max_start = (state.duration_sec - state.view_duration_sec).max(0.0);
        state.view_start_sec = (initial_start_sec - dt).clamp(0.0, max_start); // 1.0 - (-0.2) = 1.2

        assert!((state.view_start_sec - 1.2).abs() < 0.001);
        // Selection remains intact and unchanged
        assert_eq!(state.selection, Some((1.2, 1.8)));
    }

    #[test]
    fn test_sample_timestamp_and_duration_formatting() {
        // Sub-minute timestamps & durations
        assert_eq!(format_sample_timestamp(0.0), "00m00s000ms");
        assert_eq!(format_sample_timestamp(4.5), "00m04s500ms");
        assert_eq!(format_sample_timestamp(12.345), "00m12s345ms");
        assert_eq!(format_sample_duration(0.05), "00s050ms");
        assert_eq!(format_sample_duration(1.25), "01s250ms");

        // Over 1 minute
        assert_eq!(format_sample_timestamp(75.25), "01m15s250ms");
        assert_eq!(format_sample_duration(65.1), "01m05s100ms");

        // Over 1 hour
        assert_eq!(format_sample_timestamp(3665.2), "01h01m05s200ms");
        assert_eq!(format_sample_duration(3605.0), "01h00m05s000ms");

        // Filename formatting: original_name + timestamp + duration
        let filename = format_sample_filename("MyTrack", 4.5, 1.25, "", "wav");
        assert_eq!(filename, "MyTrack_00m04s500ms_01s250ms.wav");

        let mono_left_name = format_sample_filename("DrumBeat", 75.0, 2.0, "_left_mono", "wav");
        assert_eq!(
            mono_left_name,
            "DrumBeat_01m15s000ms_02s000ms_left_mono.wav"
        );
    }

    #[test]
    fn test_save_to_file_single_and_multi() {
        let (_dir, p) = generate_fixture_wav("test_track", 5.0);
        let mut state = AudioPreviewState::new(p.clone(), "test_track.wav".to_string());
        let temp_dest =
            std::env::temp_dir().join(format!("little_clip_save_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(&temp_dest);
        state.save_destination_dir = temp_dest.to_string_lossy().to_string();
        state.open_folder_after_save = false; // don't open file manager in test

        // 1. Single Selection Save
        state.set_selection_mode(SelectionMode::Single);
        state.selection = Some((1.0, 2.5)); // start 1.0s, duration 1.5s
        state.open_save_dialog();
        assert!(state.save_dialog_open);
        assert_eq!(state.save_filenames.len(), 1);
        assert_eq!(
            state.save_filenames[0],
            "test_track_00m01s000ms_01s500ms.wav"
        );

        let res = state.execute_save(1.0);
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), 1);
        let saved_single = temp_dest.join("test_track_00m01s000ms_01s500ms.wav");
        assert!(saved_single.exists());
        assert!(std::fs::metadata(&saved_single).unwrap().len() > 0);

        // 2. Multi-Selection Save
        state.set_selection_mode(SelectionMode::Multi);
        state.multi_selections = vec![(0.5, 1.2), (2.0, 2.8)];
        state.open_save_dialog();
        assert!(state.save_dialog_open);
        assert_eq!(state.save_filenames.len(), 2);
        assert_eq!(
            state.save_filenames[0],
            "test_track_00m00s500ms_00s700ms.wav"
        );
        assert_eq!(
            state.save_filenames[1],
            "test_track_00m02s000ms_00s800ms.wav"
        );

        let res_multi = state.execute_save(2.0);
        assert!(res_multi.is_ok());
        assert_eq!(res_multi.unwrap(), 2);

        let saved_multi_1 = temp_dest.join("test_track_00m00s500ms_00s700ms.wav");
        let saved_multi_2 = temp_dest.join("test_track_00m02s000ms_00s800ms.wav");
        assert!(saved_multi_1.exists());
        assert!(saved_multi_2.exists());
        assert!(std::fs::metadata(&saved_multi_1).unwrap().len() > 0);
        assert!(std::fs::metadata(&saved_multi_2).unwrap().len() > 0);

        // Cleanup test files
        let _ = std::fs::remove_file(saved_single);
        let _ = std::fs::remove_file(saved_multi_1);
        let _ = std::fs::remove_file(saved_multi_2);
        let _ = std::fs::remove_dir(temp_dest);
    }

    #[test]
    fn test_slice_preview_start_to_stop() {
        let (_dir, p) = generate_fixture_wav("test", 5.0);
        let mut state = AudioPreviewState::new(p, "test.wav".to_string());
        state.duration_sec = 10.0;

        assert_eq!(state.preview_stop_time, None);

        // 1. Play slice preview from 1.5s to 3.2s
        state.play_slice_preview(1.5, 3.2);
        assert_eq!(state.preview_stop_time, Some(3.2));
        assert!((state.current_time_sec - 1.5).abs() < 1e-4);

        // 2. Pause clears preview stop time
        state.pause();
        assert_eq!(state.preview_stop_time, None);

        // 3. Play slice preview with inverted bounds (4.0s down to 2.0s)
        state.play_slice_preview(4.0, 2.0);
        assert_eq!(state.preview_stop_time, Some(4.0));
        assert!((state.current_time_sec - 2.0).abs() < 1e-4);

        // 4. Seek clears preview stop time
        state.seek_to(0.5);
        assert_eq!(state.preview_stop_time, None);
        assert!((state.current_time_sec - 0.5).abs() < 1e-4);
    }

    #[test]
    fn test_fine_grained_consistent_zoom() {
        let (_dir, p) = generate_fixture_wav("test", 5.0);
        let mut state = AudioPreviewState::new(p, "test.wav".to_string());
        state.duration_sec = 20.0;
        state.view_start_sec = 2.0;
        state.view_duration_sec = 10.0;

        let initial_dur = state.view_duration_sec;

        // 1 Notch Zoom In (delta = +50.0 pt -> steps = +1.0)
        let step_ratio: f32 = 1.055;
        let factor_in = step_ratio.powf(-1.0); // ~0.9478
        state.zoom(factor_in, 5.0);

        // View duration should shrink by ~5.2% (fine-grained)
        assert!(state.view_duration_sec < initial_dur);
        assert!((state.view_duration_sec - (initial_dur * factor_in)).abs() < 1e-4);

        // 1 Notch Zoom Out (delta = -50.0 pt -> steps = -1.0)
        let factor_out = step_ratio.powf(1.0); // 1.055
        state.zoom(factor_out, 5.0);

        // View duration must return back to initial duration (perfect mathematical symmetry)
        assert!((state.view_duration_sec - initial_dur).abs() < 1e-4);

        // Ultra-fine mode (Ctrl/Alt held): ~2.4% zoom
        let ultra_fine_ratio: f32 = 1.025;
        let ultra_fine_factor = ultra_fine_ratio.powf(-1.0);
        state.zoom(ultra_fine_factor, 5.0);
        assert!((state.view_duration_sec - (initial_dur * ultra_fine_factor)).abs() < 1e-4);
    }

    #[test]
    fn test_audio_viewport_zoom_pan_and_fit() {
        let mut vp = AudioViewport::new(10.0);
        assert_eq!(vp.start_sec, 0.0);
        assert_eq!(vp.duration_sec, 10.0);

        // Zoom in by factor 0.5 centered at 4.0 seconds
        vp.zoom(0.5, 4.0, 10.0, 44100.0);
        assert!((vp.duration_sec - 5.0).abs() < 1e-4);
        assert!(vp.start_sec >= 0.0 && vp.start_sec + vp.duration_sec <= 10.0);

        // Pan right by 2 seconds
        let prev_start = vp.start_sec;
        vp.pan(2.0, 10.0);
        assert!((vp.start_sec - (prev_start + 2.0)).abs() < 1e-4);

        // Pan past right boundary clamps to 10.0 - duration
        vp.pan(100.0, 10.0);
        assert!((vp.start_sec - (10.0 - vp.duration_sec)).abs() < 1e-4);

        // Pan negative clamps to 0.0
        vp.pan(-100.0, 10.0);
        assert_eq!(vp.start_sec, 0.0);

        // Reset to full window
        vp.fit_to_window(10.0);
        assert_eq!(vp.start_sec, 0.0);
        assert_eq!(vp.duration_sec, 10.0);
    }

    #[test]
    fn test_audio_selection_model_contains_and_normalize() {
        // Normal range
        assert!(AudioSelectionModel::contains((2.0, 5.0), 3.0));
        assert!(AudioSelectionModel::contains((2.0, 5.0), 2.0));
        assert!(AudioSelectionModel::contains((2.0, 5.0), 5.0));
        assert!(!AudioSelectionModel::contains((2.0, 5.0), 1.9));
        assert!(!AudioSelectionModel::contains((2.0, 5.0), 5.1));

        // Inverted range (dragged backwards)
        assert!(AudioSelectionModel::contains((5.0, 2.0), 3.5));
        assert!(!AudioSelectionModel::contains((5.0, 2.0), 1.0));

        // Normalization
        assert_eq!(AudioSelectionModel::normalize((5.0, 2.0)), (2.0, 5.0));
        assert_eq!(AudioSelectionModel::normalize((2.0, 5.0)), (2.0, 5.0));

        // Clamp to duration
        assert_eq!(
            AudioSelectionModel::clamp_to_duration((-1.0, 15.0), 10.0),
            (0.0, 10.0)
        );
    }

    #[test]
    fn test_slice_preview_looping_and_loop_seek() {
        let (_dir, p) = generate_fixture_wav("test", 5.0);
        let mut state = AudioPreviewState::new(p, "test.wav".to_string());
        state.duration_sec = 10.0;

        assert_eq!(state.preview_stop_time, None);
        assert_eq!(state.preview_slice_range, None);
        assert!(!state.preview_is_looping);

        // 1. Play slice preview with looping enabled
        state.play_slice_preview_loop(1.2, 3.8);
        assert_eq!(state.preview_stop_time, Some(3.8));
        assert_eq!(state.preview_slice_range, Some((1.2, 3.8)));
        assert!(state.preview_is_looping);
        assert!((state.current_time_sec - 1.2).abs() < 1e-4);

        // 2. Loop seek preserves preview bounds and looping flag
        state.loop_seek_to(1.2);
        assert_eq!(state.preview_stop_time, Some(3.8));
        assert_eq!(state.preview_slice_range, Some((1.2, 3.8)));
        assert!(state.preview_is_looping);
        assert!((state.current_time_sec - 1.2).abs() < 1e-4);

        // 3. Normal seek clears preview state
        state.seek_to(0.5);
        assert_eq!(state.preview_stop_time, None);
        assert_eq!(state.preview_slice_range, None);
        assert!(!state.preview_is_looping);
        assert!((state.current_time_sec - 0.5).abs() < 1e-4);

        // 4. Play slice preview non-looping
        state.play_slice_preview(2.0, 4.0);
        assert_eq!(state.preview_stop_time, Some(4.0));
        assert_eq!(state.preview_slice_range, Some((2.0, 4.0)));
        assert!(!state.preview_is_looping);

        // 5. Pause clears preview state
        state.pause();
        assert_eq!(state.preview_stop_time, None);
        assert_eq!(state.preview_slice_range, None);
        assert!(!state.preview_is_looping);
    }

    #[test]
    fn test_right_click_hold_state_and_timeout() {
        assert_eq!(RIGHT_CLICK_HOLD_TIMEOUT, Duration::from_millis(300));

        let now = Instant::now();
        let hold = RightClickHoldState {
            start_instant: now,
            target_range: (1.0, 3.0),
            slice_index: Some(2),
            started_looping: false,
        };

        assert_eq!(hold.target_range, (1.0, 3.0));
        assert_eq!(hold.slice_index, Some(2));
        assert!(!hold.started_looping);
    }
}
