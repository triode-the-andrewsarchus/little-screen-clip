//! Rich interactive audio inspection workbench view.
//!
//! Provides spectral waterfall analysis, zero-crossing snapping, multi-slice region selection,
//! audio playback with Rodio, and export/split capabilities.

use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::theme::AppPalette;
use crate::{hex_to_c32, hex_to_c32_alpha};
use little_clip_core::audio::{
    compute_audio_analysis_progressive, read_audio_metadata,
    AudioAnalysisProgress, AudioMetadata, FullAudioAnalysis,
};
use little_clip_core::ipc::IpcRequest;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use rodio::{Decoder, Player};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

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
pub fn format_sample_filename(original_stem: &str, start_sec: f32, duration_sec: f32, suffix: &str, ext: &str) -> String {
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
    DragLeftHandle { other_anchor: f32 },
    DragRightHandle { other_anchor: f32 },
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

/// Interactive UI state for audio waveform preview, spectrogram analysis, and multi-slice editing.
pub struct AudioPreviewState {
    pub path: PathBuf,
    pub title: String,
    pub metadata: Option<AudioMetadata>,
    pub analysis: Option<FullAudioAnalysis>,
    pub analysis_rx: Option<Receiver<AudioAnalysisProgress>>,
    pub is_loading_analysis: bool,
    pub loading_start_time: Instant,
    pub duration_sec: f32,

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

    // Slice preview limit (for right-click slice preview)
    pub preview_stop_time: Option<f32>,
}

impl AudioPreviewState {
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

        Self {
            path,
            title,
            metadata: meta,
            analysis: None,
            analysis_rx: Some(rx),
            is_loading_analysis: true,
            loading_start_time: Instant::now(),
            duration_sec,
            view_start_sec: 0.0,
            view_duration_sec: duration_sec,
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
            snap_zero_crossings: false,
            show_transients: false,
            transient_sensitivity: 1.0,
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
            self.current_time_sec = start_sec;
            self.status_toast = Some((
                format!("✂ Trimmed silence: {:.2}s - {:.2}s ({:.2}s duration)", start_sec, end_sec, dur),
                now,
            ));
        } else {
            self.status_toast = Some((
                format!("⚠️ No audio signal detected above silence threshold ({:.0}dB)", threshold_db),
                now,
            ));
        }
    }

    pub fn with_editor(mut self, editor: String) -> Self {
        if !editor.is_empty() {
            self.audio_editor = editor;
        }
        self
    }

    /// Launches an external audio editor (e.g. Audacity) for the current file.
    pub fn launch_external_audio_editor(&mut self, ctx: &egui::Context, close_requested: &mut bool) {
        let editor = if self.audio_editor.is_empty() {
            "audacity"
        } else {
            &self.audio_editor
        };
        let mut cmd = std::process::Command::new(editor);
        cmd.arg(&self.path);
        match cmd.spawn() {
            Ok(_) => {
                let filename = self.path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                self.status_toast = Some((
                    format!("🚀 Opened {} in {}", filename, editor),
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64(),
                ));
                self.stop();
                *close_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.status_toast = Some((
                    format!("⚠️ Failed to launch '{}': {}", editor, e),
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64(),
                ));
            }
        }
    }

    #[inline]
    pub fn is_time_selected(&self, t: f32) -> bool {
        match self.selection_mode {
            SelectionMode::Single => self.selection.map(|(s, e)| t >= s && t <= e).unwrap_or(false),
            SelectionMode::Multi => self.multi_selections.iter().any(|(s, e)| t >= *s && t <= *e),
        }
    }

    #[inline]
    pub fn active_selection_range(&self) -> Option<(f32, f32)> {
        match self.selection_mode {
            SelectionMode::Single => self.selection,
            SelectionMode::Multi => self
                .active_multi_index
                .and_then(|idx| self.multi_selections.get(idx).copied())
                .or_else(|| self.multi_selections.first().copied()),
        }
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
                    return t_transient;
                }
            }
            if self.snap_zero_crossings {
                return analysis.snap_to_zero_crossing(time_sec, 500);
            }
        }
        time_sec
    }

    pub fn snap_active_selection_to_zero_crossings(&mut self, now: f64) {
        let Some(ref analysis) = self.analysis else { return; };
        match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    let s_snapped = analysis.snap_to_zero_crossing(s, 500);
                    let e_snapped = analysis.snap_to_zero_crossing(e, 500);
                    let (s_new, e_new) = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                    self.selection = Some((s_new, e_new));
                    self.status_toast = Some((
                        format!("∿ Snapped selection to 0-crossings: {:.4}s - {:.4}s", s_new, e_new),
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
                            format!("∿ Snapped region #{} to 0-crossings: {:.4}s - {:.4}s", idx + 1, s_new, e_new),
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
                        format!("∿ Snapped all {} regions to 0-crossings", self.multi_selections.len()),
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
                    if !self.multi_selections.contains(&sel) {
                        self.multi_selections.push(sel);
                    }
                    self.active_multi_index = Some(self.multi_selections.len() - 1);
                }
            }
            SelectionMode::Single => {
                if let Some(idx) = self.active_multi_index {
                    if idx < self.multi_selections.len() {
                        self.selection = Some(self.multi_selections[idx]);
                    }
                } else if let Some(first) = self.multi_selections.first() {
                    self.selection = Some(*first);
                }
            }
        }
        self.selection_mode = mode;
    }

    pub fn zoom(&mut self, factor: f32, anchor_time: f32) {
        let dur = self.duration_sec.max(0.01);
        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate as f32)
            .unwrap_or(44100.0);
        // Allow zooming down to ~12 samples!
        let min_dur = (12.0 / sample_rate).max(0.0002);
        let max_dur = dur;

        let old_dur = self.view_duration_sec.clamp(min_dur, max_dur);
        let new_dur = (old_dur * factor).clamp(min_dur, max_dur);

        let anchor = anchor_time.clamp(0.0, dur);
        let anchor_ratio = if old_dur > 0.0 {
            ((anchor - self.view_start_sec) / old_dur).clamp(0.0, 1.0)
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

        self.view_start_sec = new_start;
        self.view_duration_sec = new_dur;
    }

    pub fn pan(&mut self, delta_sec: f32) {
        let dur = self.duration_sec.max(0.01);
        let max_start = (dur - self.view_duration_sec).max(0.0);
        self.view_start_sec = (self.view_start_sec + delta_sec).clamp(0.0, max_start);
    }

    pub fn fit_to_window(&mut self) {
        self.view_start_sec = 0.0;
        self.view_duration_sec = self.duration_sec.max(0.01);
    }

    pub fn toggle_play(&mut self) {
        if self.is_playing {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn play_slice_preview(&mut self, start_sec: f32, end_sec: f32) {
        let (s, mut e) = if start_sec < end_sec {
            (start_sec, end_sec)
        } else {
            (end_sec, start_sec)
        };
        if e <= s {
            e = (s + 0.1).min(self.duration_sec);
        }
        self.preview_stop_time = Some(e);
        self.current_time_sec = s;
        self.playback_start_offset_sec = s;
        self.playback_start_instant = Some(Instant::now());

        if let Some(ref p) = self.player {
            p.clear();
            if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    p.append(decoder);
                    let target_pos = Duration::from_secs_f32(s);
                    let _ = p.try_seek(target_pos);
                    p.play();
                    self.is_playing = true;
                }
            }
        }
    }

    pub fn play(&mut self) {
        self.preview_stop_time = None;
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
                    p.append(decoder);
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
        if let Some(ref p) = self.player {
            p.pause();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
    }

    pub fn stop(&mut self) {
        self.preview_stop_time = None;
        if let Some(ref p) = self.player {
            p.stop();
        }
        self.is_playing = false;
        self.playback_start_instant = None;
        self.current_time_sec = self.active_selection_range().map(|(s, _)| s).unwrap_or(0.0);
    }

    pub fn seek_to(&mut self, target_sec: f32) {
        self.preview_stop_time = None;
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
                                p.append(decoder);
                                let _ = p.try_seek(target_pos);
                                p.play();
                            }
                        }
                    }
                }
            }
        }
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
        let _ = std::fs::create_dir_all(&staging_dir);

        let mode_suffix = match mode {
            ChannelCopyMode::Native => "",
            ChannelCopyMode::LeftOnly => "_left_mono",
            ChannelCopyMode::RightOnly => "_right_mono",
        };
        let out_filename = format_sample_filename(base_name, start, duration, mode_suffix, "wav");
        let out_path = staging_dir.join(out_filename);

        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-ss")
            .arg(format!("{:.4}", start))
            .arg("-to")
            .arg(format!("{:.4}", end))
            .arg("-i")
            .arg(&self.path);

        match mode {
            ChannelCopyMode::Native => {
                cmd.arg("-c:a").arg("pcm_s16le");
            }
            ChannelCopyMode::LeftOnly => {
                cmd.arg("-af").arg("pan=mono|c0=c0").arg("-c:a").arg("pcm_s16le");
            }
            ChannelCopyMode::RightOnly => {
                cmd.arg("-af").arg("pan=mono|c0=c1").arg("-c:a").arg("pcm_s16le");
            }
        }

        cmd.arg(&out_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        if let Ok(s) = cmd.status() {
            if s.success() && out_path.exists() {
                let req = IpcRequest::PushFile {
                    path: out_path.clone(),
                    hand: little_clip_core::HandSide::Left,
                };
                let ipc_sent = crate::send_ipc_request(&req).is_ok();

                // If daemon is not running, fall back to direct wl-copy
                if !ipc_sent {
                    let path_str = out_path.to_string_lossy().to_string();
                    let uri_list = format!("file://{}\n", path_str);
                    let _ = std::process::Command::new("wl-copy")
                        .arg("-t")
                        .arg("text/uri-list")
                        .arg(&uri_list)
                        .spawn();
                    let _ = std::process::Command::new("wl-copy").arg(&path_str).spawn();
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
                self.status_toast = Some(("⚠️ Failed to extract audio segment via ffmpeg".to_string(), now));
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
        let _ = std::fs::create_dir_all(&staging_dir);

        let mode_suffix = match mode {
            ChannelCopyMode::Native => "",
            ChannelCopyMode::LeftOnly => "_left_mono",
            ChannelCopyMode::RightOnly => "_right_mono",
        };

        let mut out_paths: Vec<PathBuf> = Vec::new();

        for (_i, &(start, end)) in self.multi_selections.iter().enumerate() {
            let (s, e) = if start < end { (start, end) } else { (end, start) };
            let duration = (e - s).max(0.001);
            let out_filename = format_sample_filename(base_name, s, duration, mode_suffix, "wav");
            let out_path = staging_dir.join(out_filename);

            let mut cmd = std::process::Command::new("ffmpeg");
            cmd.arg("-y")
                .arg("-ss")
                .arg(format!("{:.4}", s))
                .arg("-to")
                .arg(format!("{:.4}", e))
                .arg("-i")
                .arg(&self.path);

            match mode {
                ChannelCopyMode::Native => {
                    cmd.arg("-c:a").arg("pcm_s16le");
                }
                ChannelCopyMode::LeftOnly => {
                    cmd.arg("-af").arg("pan=mono|c0=c0").arg("-c:a").arg("pcm_s16le");
                }
                ChannelCopyMode::RightOnly => {
                    cmd.arg("-af").arg("pan=mono|c0=c1").arg("-c:a").arg("pcm_s16le");
                }
            }

            cmd.arg(&out_path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            if let Ok(s) = cmd.status() {
                if s.success() && out_path.exists() {
                    out_paths.push(out_path);
                }
            }
        }

        if !out_paths.is_empty() {
            let mut uri_list = String::new();
            for p in &out_paths {
                uri_list.push_str(&format!("file://{}\n", p.to_string_lossy()));
            }

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

            // If daemon is not running, fall back to direct wl-copy
            if !ipc_sent {
                let _ = std::process::Command::new("wl-copy")
                    .arg("-t")
                    .arg("text/uri-list")
                    .arg(&uri_list)
                    .spawn();
                let _ = std::process::Command::new("wl-copy")
                    .arg(&uri_list)
                    .spawn();
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
            self.status_toast = Some(("⚠️ Failed to extract multi-region slices via ffmpeg".to_string(), now));
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
                    let base_name = self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
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
            let dest_name = dest_dir.file_name().and_then(|s| s.to_str()).unwrap_or("folder");
            self.status_toast = Some((
                format!(
                    "💾 Saved {} file(s) to {}/",
                    count,
                    dest_name
                ),
                now,
            ));

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
                    if ui.button("Browse...").on_hover_text("Choose destination folder via file manager dialog").clicked() {
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
                    if ui.button("↺ Reset Names").on_hover_text("Reset filenames to default original_name + timestamp + duration").clicked() {
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
                    let save_btn = egui::Button::new(
                        egui::RichText::new(btn_text)
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_high)),
                    )
                    .fill(hex_to_c32(&palette.diff_add))
                    .rounding(Rounding::same(6.0));

                    let is_enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add(save_btn).clicked() || is_enter {
                        trigger_save = true;
                    }

                    if ui.button("Cancel [Esc]").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
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

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);

        // Check for progressive audio analysis background updates
        if let Some(ref rx) = self.analysis_rx {
            ctx.request_repaint();
            while let Ok(progress) = rx.try_recv() {
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
                    AudioAnalysisProgress::SpectrogramAndNotesReady {
                        spectrogram_width,
                        spectrogram_height,
                        spectrogram_rgba,
                        note_strip_width,
                        note_strip_height,
                        note_strip_rgba,
                        note_strip_info,
                        spectral_colors,
                    } => {
                        let spec_img = egui::ColorImage::from_rgba_unmultiplied(
                            [spectrogram_width, spectrogram_height],
                            &spectrogram_rgba,
                        );
                        self.spectrogram_texture = Some(ctx.load_texture(
                            "spectrogram",
                            spec_img,
                            egui::TextureOptions::LINEAR,
                        ));

                        let note_img = egui::ColorImage::from_rgba_unmultiplied(
                            [note_strip_width, note_strip_height],
                            &note_strip_rgba,
                        );
                        self.note_strip_texture = Some(ctx.load_texture(
                            "note_strip",
                            note_img,
                            egui::TextureOptions::NEAREST,
                        ));

                        if let Some(ref mut analysis) = self.analysis {
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
                    AudioAnalysisProgress::Complete => {
                        self.analysis_rx = None;
                        break;
                    }
                    AudioAnalysisProgress::Failed(err) => {
                        self.status_toast = Some((err, now));
                        self.is_loading_analysis = false;
                        self.analysis_rx = None;
                        break;
                    }
                }
            }
        }

        // Advance playhead if playing
        if self.is_playing {
            ctx.request_repaint();
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                let new_time = self.playback_start_offset_sec + elapsed;

                // Check if previewing a specific slice
                if let Some(stop_t) = self.preview_stop_time {
                    if new_time >= stop_t {
                        self.pause();
                        self.current_time_sec = stop_t;
                        self.preview_stop_time = None;
                    } else {
                        self.current_time_sec = new_time;
                    }
                } else {
                    // Handle looping within region selection
                    let loop_range = self.active_selection_range();
                    if let Some((sel_start, sel_end)) = loop_range {
                        if self.loop_selection && new_time >= sel_end {
                            self.seek_to(sel_start);
                        } else if new_time >= self.duration_sec {
                            self.pause();
                            self.current_time_sec = self.duration_sec;
                        } else {
                            self.current_time_sec = new_time;
                        }
                    } else if new_time >= self.duration_sec {
                        self.pause();
                        self.current_time_sec = self.duration_sec;
                    } else {
                        self.current_time_sec = new_time;
                    }
                }

                // Auto-follow playhead when zoomed in
                let v_end = self.view_start_sec + self.view_duration_sec;
                if self.current_time_sec >= v_end {
                    let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                    self.view_start_sec = (self.current_time_sec - self.view_duration_sec * 0.1).clamp(0.0, max_start);
                } else if self.current_time_sec < self.view_start_sec {
                    self.view_start_sec = (self.current_time_sec - self.view_duration_sec * 0.1).max(0.0);
                }
            }
        }

        // Clear focus from background palette inputs so they cannot swallow shortcuts
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });

        // Global Keybindings for Audio Preview
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop();
            *close_requested = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_play();
        }

        // Poll asynchronous folder browsing dialog if running
        if let Some(ref rx) = self.browse_rx {
            if let Ok(res) = rx.try_recv() {
                if let Some(dir) = res {
                    self.save_destination_dir = dir;
                }
                self.browse_rx = None;
            }
        }

        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || i.raw.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.key_pressed(egui::Key::C) && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
        });

        if copy_requested {
            match self.selection_mode {
                SelectionMode::Single => self.copy_selected_segment(now, ChannelCopyMode::Native),
                SelectionMode::Multi => self.copy_all_multi_segments(now, ChannelCopyMode::Native),
            }
        }

        let save_requested = ctx.input(|i| {
            i.key_pressed(egui::Key::S) && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd)
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
        });

        if save_requested {
            if ctx.input(|i| i.modifiers.shift) {
                self.quick_save(now);
            } else {
                self.open_save_dialog();
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            let step = if ctx.input(|i| i.modifiers.shift) { 5.0 } else { 1.0 };
            self.seek_to(self.current_time_sec - step);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            let step = if ctx.input(|i| i.modifiers.shift) { 5.0 } else { 1.0 };
            self.seek_to(self.current_time_sec + step);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Home) || i.key_pressed(egui::Key::Num0)) {
            self.seek_to(0.0);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)) {
            match self.selection_mode {
                SelectionMode::Single => {
                    self.selection = None;
                }
                SelectionMode::Multi => {
                    if let Some(idx) = self.active_multi_index {
                        if idx < self.multi_selections.len() {
                            self.multi_selections.remove(idx);
                            self.active_multi_index = None;
                            self.status_toast = Some((format!("Removed selection #{}", idx + 1), now));
                        }
                    }
                }
            }
        }

        // 'Z' key: Snap selection to zero-crossings
        if ctx.input(|i| i.key_pressed(egui::Key::Z) && !i.modifiers.ctrl && !i.modifiers.command) {
            self.snap_active_selection_to_zero_crossings(now);
        }

        // 'T' key: Toggle transient detection markers & snapping (when transients are ready)
        if ctx.input(|i| i.key_pressed(egui::Key::T) && !i.modifiers.ctrl && !i.modifiers.command && !i.modifiers.alt) {
            let transients_ready = self.analysis.as_ref().map(|a| a.is_transients_ready).unwrap_or(false);
            if transients_ready {
                self.show_transients = !self.show_transients;
                let status = if self.show_transients { "ON (Snapping Enabled)" } else { "OFF" };
                self.status_toast = Some((
                    format!("⚡ Transients: {}", status),
                    now,
                ));
            }
        }

        // Zoom keyboard shortcuts: Plus / Minus / Ctrl+0
        if ctx.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
            self.zoom(0.88, self.current_time_sec);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
            self.zoom(1.136, self.current_time_sec);
        }
        if ctx.input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::Num0)) {
            self.fit_to_window();
        }

        // Number keys 1..9: preview corresponding slice from start to stop
        if self.selection_mode == SelectionMode::Multi && !self.multi_selections.is_empty() {
            let num_keys = [
                egui::Key::Num1, egui::Key::Num2, egui::Key::Num3,
                egui::Key::Num4, egui::Key::Num5, egui::Key::Num6,
                egui::Key::Num7, egui::Key::Num8, egui::Key::Num9,
            ];
            for (idx, &k) in num_keys.iter().enumerate() {
                if ctx.input(|i| i.key_pressed(k) && !i.modifiers.ctrl && !i.modifiers.command && !i.modifiers.alt) {
                    if idx < self.multi_selections.len() {
                        let (s, e) = self.multi_selections[idx];
                        self.active_multi_index = Some(idx);
                        self.play_slice_preview(s, e);
                        self.status_toast = Some((
                            format!("▶ Previewing slice #{} ({:02}:{:04.1} - {:02}:{:04.1})", idx + 1, (s / 60.0) as u32, s % 60.0, (e / 60.0) as u32, e % 60.0),
                            now,
                        ));
                        break;
                    }
                }
            }
        }

        let full_rect = ui.available_rect_before_wrap();
        ui.allocate_ui_at_rect(full_rect, |ui| {
            // Dark studio background surface
            ui.painter().rect_filled(
                full_rect,
                Rounding::ZERO,
                hex_to_c32(&palette.theme.background),
            );

            ui.add_space(8.0);

            // 1. Top Control Header
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new("🎵 Audio Analysis & Slicer")
                        .heading()
                        .strong()
                        .color(hex_to_c32(&palette.harmonies.base)),
                );

                ui.add_space(10.0);

                // File metadata badge
                let mins = (self.duration_sec / 60.0) as u32;
                let secs = self.duration_sec % 60.0;
                let meta_str = if let Some(meta) = &self.metadata {
                    format!(
                        "{} • {:02}:{:04.1} • {:.1} kHz • {}",
                        self.title,
                        mins,
                        secs,
                        meta.sample_rate as f32 / 1000.0,
                        meta.format
                    )
                } else {
                    format!("{} • {:02}:{:04.1}", self.title, mins, secs)
                };

                let meta_badge = egui::Button::new(
                    egui::RichText::new(meta_str)
                        .small()
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_med)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .stroke(Stroke::NONE)
                .rounding(Rounding::same(4.0));
                ui.add(meta_badge);

                // Multi-channel warning badge if > 2 channels
                if let Some(ref an) = self.analysis {
                    if an.channels > 2 {
                        ui.add_space(6.0);
                        let warn_badge = egui::Button::new(
                            egui::RichText::new(format!("⚠️ Multi-channel ({} ch) — Displaying L & R only", an.channels))
                                .small()
                                .strong()
                                .color(hex_to_c32(&palette.diff_mod)),
                        )
                        .fill(hex_to_c32_alpha(&palette.diff_mod, 40))
                        .stroke(Stroke::NONE)
                        .rounding(Rounding::same(4.0));
                        ui.add(warn_badge).on_hover_text(format!(
                            "Audio contains {} channels. The first 2 channels (Left and Right) are visualized and can be isolated.",
                            an.channels
                        ));
                    }
                }

                // Right-aligned close button & toast
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(14.0);
                    let close_btn = egui::Button::new(
                        egui::RichText::new("✕ Close (Esc)")
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_high)),
                    )
                    .fill(hex_to_c32(&palette.diff_del))
                    .rounding(Rounding::same(6.0));

                    if ui.add(close_btn).clicked() {
                        self.stop();
                        *close_requested = true;
                    }

                    ui.add_space(6.0);

                    let audacity_label = format!("🔊 {}", if self.audio_editor.is_empty() { "Audacity" } else { &self.audio_editor });
                    let audacity_btn = egui::Button::new(
                        egui::RichText::new(audacity_label)
                            .small()
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_high)),
                    )
                    .fill(hex_to_c32(&palette.theme.b_low))
                    .stroke(Stroke::new(1.0, hex_to_c32(&palette.harmonies.base)))
                    .rounding(Rounding::same(6.0));

                    if ui.add(audacity_btn).on_hover_text(format!("Open audio file in external editor ({})", self.audio_editor)).clicked() {
                        self.launch_external_audio_editor(ctx, close_requested);
                    }

                    ui.add_space(8.0);

                    if let Some((msg, ts)) = &self.status_toast {
                        if now - ts < 4.5 {
                            ui.label(
                                egui::RichText::new(msg)
                                    .color(hex_to_c32(&palette.diff_add))
                                    .small()
                                    .strong(),
                            );
                        }
                    }
                });
            });

            ui.add_space(8.0);

            // 2. Transport, Zoom & Slicing Control Bar
            ui.horizontal(|ui| {
                ui.add_space(14.0);

                // Play / Pause Button
                let play_icon = if self.is_playing { "⏸ Pause [Space]" } else { "▶ Play [Space]" };
                let play_color = if self.is_playing {
                    hex_to_c32(&palette.diff_mod)
                } else {
                    hex_to_c32(&palette.diff_add)
                };

                let play_btn = egui::Button::new(
                    egui::RichText::new(play_icon)
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_high)),
                )
                .fill(play_color)
                .rounding(Rounding::same(6.0));

                if ui.add(play_btn).clicked() {
                    self.toggle_play();
                }

                ui.add_space(3.0);

                // Stop Button
                let stop_btn = egui::Button::new(
                    egui::RichText::new("⏹ Stop")
                        .color(hex_to_c32(&palette.theme.f_high)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .rounding(Rounding::same(6.0));
                if ui.add(stop_btn).clicked() {
                    self.stop();
                }

                // Replay Button
                let replay_btn = egui::Button::new(
                    egui::RichText::new("⟲ Start")
                        .color(hex_to_c32(&palette.theme.f_high)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .rounding(Rounding::same(6.0));
                if ui.add(replay_btn).clicked() {
                    self.seek_to(self.active_selection_range().map(|(s, _)| s).unwrap_or(0.0));
                }

                ui.add_space(10.0);

                // Time Display
                let cur_m = (self.current_time_sec / 60.0) as u32;
                let cur_s = self.current_time_sec % 60.0;
                let tot_m = (self.duration_sec / 60.0) as u32;
                let tot_s = self.duration_sec % 60.0;

                ui.label(
                    egui::RichText::new(format!("{:02}:{:05.2} / {:02}:{:05.2}", cur_m, cur_s, tot_m, tot_s))
                        .color(hex_to_c32(&palette.harmonies.base))
                        .strong()
                        .monospace(),
                );

                ui.add_space(10.0);

                // Volume slider
                ui.label(egui::RichText::new("🔊").color(hex_to_c32(&palette.theme.f_med)));
                let mut vol_pct = self.volume * 100.0;
                let slider = egui::Slider::new(&mut vol_pct, 0.0..=100.0)
                    .show_value(false);
                if ui.add_sized([60.0, 18.0], slider).changed() {
                    self.volume = (vol_pct / 100.0).clamp(0.0, 1.0);
                    if let Some(ref p) = self.player {
                        p.set_volume(self.volume);
                    }
                }

                ui.add_space(12.0);

                // Zoom Controls (Audacity Style)
                let zoom_in_btn = egui::Button::new(
                    egui::RichText::new("🔍+").color(hex_to_c32(&palette.theme.f_high)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .rounding(Rounding::same(4.0));
                if ui.add(zoom_in_btn).on_hover_text("Zoom In (+ / Ctrl+Wheel)").clicked() {
                    self.zoom(0.75, self.current_time_sec);
                }

                let zoom_out_btn = egui::Button::new(
                    egui::RichText::new("🔍-").color(hex_to_c32(&palette.theme.f_high)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .rounding(Rounding::same(4.0));
                if ui.add(zoom_out_btn).on_hover_text("Zoom Out (- / Ctrl+Wheel)").clicked() {
                    self.zoom(1.33, self.current_time_sec);
                }

                let fit_btn = egui::Button::new(
                    egui::RichText::new("⟲ Fit").small().color(hex_to_c32(&palette.theme.f_med)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .rounding(Rounding::same(4.0));
                if ui.add(fit_btn).on_hover_text("Fit Full Track (Ctrl+0)").clicked() {
                    self.fit_to_window();
                }

                let zoom_pct = ((self.duration_sec / self.view_duration_sec.max(0.0001)) * 100.0).round() as u32;
                ui.label(
                    egui::RichText::new(format!("{}%", zoom_pct))
                        .small()
                        .monospace()
                        .color(hex_to_c32(&palette.theme.f_med)),
                );

                ui.add_space(12.0);

                // Mode Toggle: Single Region (default) vs Multi-Region Mode
                let is_multi = self.selection_mode == SelectionMode::Multi;
                let (mode_icon, mode_col, mode_bg) = if is_multi {
                    ("☷ Multi-Region", hex_to_c32(&palette.diff_add), hex_to_c32_alpha(&palette.diff_add, 40))
                } else {
                    ("▢ Single Region", hex_to_c32(&palette.theme.f_med), hex_to_c32(&palette.theme.b_low))
                };
                let mode_btn = egui::Button::new(
                    egui::RichText::new(mode_icon).small().strong().color(mode_col),
                )
                .fill(mode_bg)
                .rounding(Rounding::same(4.0));
                if ui.add(mode_btn).on_hover_text("Toggle between Single Slice mode and Multi-Region extraction mode").clicked() {
                    let next_mode = if is_multi { SelectionMode::Single } else { SelectionMode::Multi };
                    self.set_selection_mode(next_mode);
                }

                // Snap to 0-Crossing Toggle
                let (snap_col, snap_bg) = if self.snap_zero_crossings {
                    (hex_to_c32(&palette.harmonies.base), hex_to_c32_alpha(&palette.harmonies.base, 40))
                } else {
                    (hex_to_c32(&palette.theme.f_med), hex_to_c32(&palette.theme.b_low))
                };
                let snap_btn = egui::Button::new(
                    egui::RichText::new("∿ Snap 0-Cross [Z]").small().strong().color(snap_col),
                )
                .fill(snap_bg)
                .rounding(Rounding::same(4.0));
                if ui.add(snap_btn).on_hover_text("Snap selection boundaries to zero-crossings to prevent audio clicks/pops. Press [Z] to snap now.").clicked() {
                    self.snap_zero_crossings = !self.snap_zero_crossings;
                    if self.snap_zero_crossings {
                        self.snap_active_selection_to_zero_crossings(now);
                    }
                }

                // Transient Detection & Snap Toggle
                let transients_ready = self.analysis.as_ref().map(|a| a.is_transients_ready).unwrap_or(false);
                let (trans_col, trans_bg) = if !transients_ready {
                    (hex_to_c32(&palette.theme.f_low), hex_to_c32(&palette.theme.b_low))
                } else if self.show_transients {
                    (hex_to_c32(&palette.diff_mod), hex_to_c32_alpha(&palette.diff_mod, 40))
                } else {
                    (hex_to_c32(&palette.theme.f_med), hex_to_c32(&palette.theme.b_low))
                };
                let btn_label = if !transients_ready {
                    "⏳ Transients...".to_string()
                } else {
                    "⚡ Transients [T]".to_string()
                };
                let trans_btn = egui::Button::new(
                    egui::RichText::new(btn_label).small().strong().color(trans_col),
                )
                .fill(trans_bg)
                .rounding(Rounding::same(4.0));
                if transients_ready {
                    if ui.add(trans_btn).on_hover_text("Toggle transient marks and magnetic snapping for selections. Press [T] to toggle.").clicked() {
                        self.show_transients = !self.show_transients;
                        let status = if self.show_transients { "ON (Snapping Enabled)" } else { "OFF" };
                        self.status_toast = Some((
                            format!("⚡ Transients: {}", status),
                            now,
                        ));
                    }

                    // Transient sensitivity slider
                    if self.show_transients {
                        ui.add_space(4.0);
                        let sens_slider = egui::Slider::new(&mut self.transient_sensitivity, 0.1..=3.0)
                            .step_by(0.1)
                            .text("Sens")
                            .show_value(true);
                        if ui.add(sens_slider).on_hover_text("Adjust transient onset sensitivity (0.1 = sharp beats only, 3.0 = all soft attacks)").changed() {
                            if let Some(ref mut analysis) = self.analysis {
                                analysis.recompute_transients(self.transient_sensitivity);
                                self.status_toast = Some((
                                    format!("⚡ Sensitivity {:.1}x ({} transients detected)", self.transient_sensitivity, analysis.transients.len()),
                                    now,
                                ));
                            }
                        }
                    }
                } else {
                    ui.add_enabled(false, trans_btn).on_hover_text("Audio transients are currently being analyzed in background...");
                }

                ui.add_space(8.0);

                // Trim Silence Button
                let trim_btn = egui::Button::new(
                    egui::RichText::new("✂ Trim Silence")
                        .small()
                        .strong()
                        .color(hex_to_c32(&palette.theme.f_high)),
                )
                .fill(hex_to_c32(&palette.theme.b_low))
                .stroke(Stroke::new(1.0, hex_to_c32(&palette.theme.b_med)))
                .rounding(Rounding::same(4.0));
                if ui.add(trim_btn).on_hover_text("Trim silence (< -45 dB) from both leading and trailing sides").clicked() {
                    self.trim_silence(-45.0, now);
                }

                ui.add_space(10.0);

                let channels_count = self.analysis.as_ref().map(|a| a.channels).unwrap_or(2);

                match self.selection_mode {
                    SelectionMode::Single => {
                        // Selection Pill & Actions for Single Mode
                        if let Some((start, end)) = self.selection {
                            let sel_dur = end - start;
                            let sel_str = format!(
                                "⚡ Slice: {:02}:{:04.1} - {:02}:{:04.1} ({:.2}s)",
                                (start / 60.0) as u32,
                                start % 60.0,
                                (end / 60.0) as u32,
                                end % 60.0,
                                sel_dur
                            );

                            let sel_badge = egui::Button::new(
                                egui::RichText::new(sel_str)
                                    .small()
                                    .strong()
                                    .color(hex_to_c32(&palette.harmonies.base)),
                            )
                            .fill(hex_to_c32_alpha(&palette.harmonies.base, 40))
                            .stroke(Stroke::NONE)
                            .rounding(Rounding::same(4.0));
                            ui.add(sel_badge);

                            let clear_btn = egui::Button::new(
                                egui::RichText::new("✕ Clear")
                                    .small()
                                    .color(hex_to_c32(&palette.theme.f_med)),
                            )
                            .fill(hex_to_c32(&palette.theme.b_low))
                            .rounding(Rounding::same(4.0));
                            if ui.add(clear_btn).clicked() {
                                self.selection = None;
                            }
                        }

                        // Copy Slice (Native) Button
                        let copy_label = if self.selection.is_some() {
                            "📋 Copy Slice [Ctrl+C]"
                        } else {
                            "📋 Copy Track [Ctrl+C]"
                        };

                        let copy_btn = egui::Button::new(
                            egui::RichText::new(copy_label)
                                .strong()
                                .color(hex_to_c32(&palette.theme.f_high)),
                        )
                        .fill(hex_to_c32(&palette.harmonies.base))
                        .rounding(Rounding::same(6.0));

                        if ui.add(copy_btn).on_hover_text("Copy selected segment with all original channels").clicked() {
                            self.copy_selected_segment(now, ChannelCopyMode::Native);
                        }

                        // Channel-Specific Copy Buttons (Left only mono, Right only mono)
                        if channels_count >= 2 {
                            ui.add_space(4.0);
                            let left_btn = egui::Button::new(
                                egui::RichText::new("◧ Left Only (Mono)")
                                    .small()
                                    .strong()
                                    .color(hex_to_c32(&palette.left_hand_border)),
                            )
                            .fill(hex_to_c32_alpha(&palette.left_hand_border, 40))
                            .rounding(Rounding::same(4.0));
                            if ui.add(left_btn).on_hover_text("Copy only the Left channel as a mono audio file").clicked() {
                                self.copy_selected_segment(now, ChannelCopyMode::LeftOnly);
                            }

                            let right_btn = egui::Button::new(
                                egui::RichText::new("◨ Right Only (Mono)")
                                    .small()
                                    .strong()
                                    .color(hex_to_c32(&palette.right_hand_border)),
                            )
                            .fill(hex_to_c32_alpha(&palette.right_hand_border, 40))
                            .rounding(Rounding::same(4.0));
                            if ui.add(right_btn).on_hover_text("Copy only the Right channel as a mono audio file").clicked() {
                                self.copy_selected_segment(now, ChannelCopyMode::RightOnly);
                            }
                        }

                        ui.add_space(4.0);
                        let save_label = if self.selection.is_some() {
                            "💾 Save File... [Ctrl+S]"
                        } else {
                            "💾 Save Track... [Ctrl+S]"
                        };
                        let save_btn = egui::Button::new(
                            egui::RichText::new(save_label)
                                .strong()
                                .color(hex_to_c32(&palette.theme.f_high)),
                        )
                        .fill(hex_to_c32(&palette.diff_add))
                        .rounding(Rounding::same(6.0));
                        if ui.add(save_btn).on_hover_text("Save slice to disk formatted as original_name + timestamp + duration [Ctrl+S]").clicked() {
                            self.open_save_dialog();
                        }
                    }
                    SelectionMode::Multi => {
                        let count = self.multi_selections.len();
                        let badge_text = if count == 0 {
                            "☷ 0 Slices (Drag to add)".to_string()
                        } else {
                            format!("☷ {} Slices", count)
                        };
                        let count_badge = egui::Button::new(
                            egui::RichText::new(badge_text)
                                .small()
                                .strong()
                                .color(hex_to_c32(&palette.harmonies.base)),
                        )
                        .fill(hex_to_c32_alpha(&palette.harmonies.base, 40))
                        .stroke(Stroke::NONE)
                        .rounding(Rounding::same(4.0));
                        ui.add(count_badge);

                        if count > 0 {
                            let clear_all_btn = egui::Button::new(
                                egui::RichText::new("🗑 Clear All")
                                    .small()
                                    .color(hex_to_c32(&palette.diff_del)),
                            )
                            .fill(hex_to_c32(&palette.theme.b_low))
                            .rounding(Rounding::same(4.0));
                            if ui.add(clear_all_btn).on_hover_text("Remove all selected regions").clicked() {
                                self.multi_selections.clear();
                                self.active_multi_index = None;
                            }
                        }

                        let copy_label = if count > 0 {
                            format!("📋 Copy All ({}) [Ctrl+C]", count)
                        } else {
                            "📋 Copy Track [Ctrl+C]".to_string()
                        };

                        let copy_btn = egui::Button::new(
                            egui::RichText::new(copy_label)
                                .strong()
                                .color(hex_to_c32(&palette.theme.f_high)),
                        )
                        .fill(hex_to_c32(&palette.harmonies.base))
                        .rounding(Rounding::same(6.0));

                        if ui.add(copy_btn).on_hover_text("Export and copy all selected regions to Clipboard & Left Hand").clicked() {
                            self.copy_all_multi_segments(now, ChannelCopyMode::Native);
                        }

                        if channels_count >= 2 {
                            ui.add_space(4.0);
                            let left_label = if count > 0 { format!("◧ All Left ({})", count) } else { "◧ Left Only".to_string() };
                            let left_btn = egui::Button::new(
                                egui::RichText::new(left_label)
                                    .small()
                                    .strong()
                                    .color(hex_to_c32(&palette.left_hand_border)),
                            )
                            .fill(hex_to_c32_alpha(&palette.left_hand_border, 40))
                            .rounding(Rounding::same(4.0));
                            if ui.add(left_btn).on_hover_text("Export all regions isolating only the Left channel as mono").clicked() {
                                self.copy_all_multi_segments(now, ChannelCopyMode::LeftOnly);
                            }

                            let right_label = if count > 0 { format!("◨ All Right ({})", count) } else { "◨ Right Only".to_string() };
                            let right_btn = egui::Button::new(
                                egui::RichText::new(right_label)
                                    .small()
                                    .strong()
                                    .color(hex_to_c32(&palette.right_hand_border)),
                            )
                            .fill(hex_to_c32_alpha(&palette.right_hand_border, 40))
                            .rounding(Rounding::same(4.0));
                            if ui.add(right_btn).on_hover_text("Export all regions isolating only the Right channel as mono").clicked() {
                                self.copy_all_multi_segments(now, ChannelCopyMode::RightOnly);
                            }
                        }

                        ui.add_space(4.0);
                        let save_label = if count > 0 {
                            format!("💾 Save All ({}) [Ctrl+S]", count)
                        } else {
                            "💾 Save Track [Ctrl+S]".to_string()
                        };
                        let save_btn = egui::Button::new(
                            egui::RichText::new(save_label)
                                .strong()
                                .color(hex_to_c32(&palette.theme.f_high)),
                        )
                        .fill(hex_to_c32(&palette.diff_add))
                        .rounding(Rounding::same(6.0));
                        if ui.add(save_btn).on_hover_text("Save all slices to disk formatted as original_name + timestamp + duration [Ctrl+S]").clicked() {
                            self.open_save_dialog();
                        }
                    }
                }
            });

            // Multi-region slice pill list strip when in Multi mode
            let has_multi_pills = self.selection_mode == SelectionMode::Multi && !self.multi_selections.is_empty();
            if has_multi_pills {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("Slices:")
                            .small()
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );
                    let mut seek_target = None;
                    let mut preview_target = None;
                    let mut remove_slice_idx = None;
                    let slices_copy = self.multi_selections.clone();
                    let active_idx = self.active_multi_index;

                    egui::ScrollArea::horizontal()
                        .id_source("multi_slices_scroll")
                        .show(ui, |ui| {
                            for (idx, &(s, e)) in slices_copy.iter().enumerate() {
                                let is_active = active_idx == Some(idx);
                                let (bg_col, border_col, text_col) = if is_active {
                                    (hex_to_c32_alpha(&palette.harmonies.base, 40), hex_to_c32(&palette.harmonies.base), hex_to_c32(&palette.harmonies.base))
                                } else {
                                    (hex_to_c32(&palette.theme.b_low), hex_to_c32(&palette.theme.b_med), hex_to_c32(&palette.theme.f_med))
                                };

                                let label = format!("#{}: {:02}:{:04.1}-{:02}:{:04.1} ({:.2}s)", idx + 1, (s/60.0) as u32, s%60.0, (e/60.0) as u32, e%60.0, e - s);
                                let btn = egui::Button::new(
                                    egui::RichText::new(label).small().strong().color(text_col),
                                )
                                .fill(bg_col)
                                .stroke(Stroke::new(1.0, border_col))
                                .rounding(Rounding::same(4.0));

                                let btn_resp = ui.add(btn).on_hover_text("Left-Click: focus & seek | Right-Click: preview slice");
                                if btn_resp.clicked() {
                                    seek_target = Some((idx, s));
                                }
                                if btn_resp.secondary_clicked() {
                                    preview_target = Some((idx, s, e));
                                }

                                let del_btn = egui::Button::new(
                                    egui::RichText::new("✕").small().color(hex_to_c32(&palette.diff_del)),
                                )
                                .fill(hex_to_c32(&palette.theme.b_low))
                                .rounding(Rounding::same(4.0));
                                if ui.add(del_btn).on_hover_text(format!("Remove slice #{}", idx + 1)).clicked() {
                                    remove_slice_idx = Some(idx);
                                }
                                ui.add_space(6.0);
                            }
                        });

                    if let Some((idx, s, e)) = preview_target {
                        self.active_multi_index = Some(idx);
                        self.play_slice_preview(s, e);
                        self.status_toast = Some((
                            format!("▶ Previewing slice #{} ({:02}:{:04.1} - {:02}:{:04.1})", idx + 1, (s / 60.0) as u32, s % 60.0, (e / 60.0) as u32, e % 60.0),
                            now,
                        ));
                    } else if let Some((idx, s)) = seek_target {
                        self.active_multi_index = Some(idx);
                        self.seek_to(s);
                    }
                    if let Some(idx) = remove_slice_idx {
                        self.multi_selections.remove(idx);
                        if self.active_multi_index == Some(idx) {
                            self.active_multi_index = None;
                        } else if let Some(ref mut act) = self.active_multi_index {
                            if *act > idx {
                                *act -= 1;
                            }
                        }
                    }
                });
            }

            ui.add_space(8.0);

            // 3. Main Synchronized 3-View Representation Canvas
            let avail_w = ui.available_width() - 28.0;
            let sub_h = if has_multi_pills { 62.0 } else { 36.0 };
            let avail_h = (ui.available_height() - sub_h).max(220.0);

            ui.horizontal(|ui| {
                ui.add_space(14.0);
                self.render_waveform_canvas(ui, ctx, palette, avail_w, avail_h);
            });

            // 4. Footer Help Bar
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Navigation:")
                            .small()
                            .strong()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );
                    ui.label(
                        egui::RichText::new("[Hold Middle-Mouse / Shift+Wheel] Pan • [Wheel / +/-] Zoom • [Drag] Select • [Z] Snap 0-Cross • [Space] Play • [Ctrl+C] Copy • [Ctrl+S] Save • [Right-Click / Del] Remove Slice")
                            .small()
                            .color(hex_to_c32(&palette.theme.f_low)),
                    );
                });
            });
        });

        // Render Save Dialog modal if open
        self.render_save_dialog(ctx, palette, now);
    }

    fn render_waveform_canvas(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        palette: &AppPalette,
        width: f32,
        height: f32,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        let bg_frame = crate::hex_to_c32(&palette.theme.b_low);
        let border_frame = crate::hex_to_c32(&palette.theme.b_med);
        let text_f_med = crate::hex_to_c32(&palette.theme.f_med);
        let text_f_low = crate::hex_to_c32(&palette.theme.f_low);
        let accent_base = crate::hex_to_c32(&palette.harmonies.base);
        let accent_playhead = crate::hex_to_c32(&palette.harmonies.complementary);
        let transient_col = crate::hex_to_c32(&palette.diff_mod);

        // Frame border & background
        painter.rect_filled(
            rect,
            Rounding::same(8.0),
            bg_frame,
        );
        painter.rect_stroke(
            rect,
            Rounding::same(8.0),
            Stroke::new(1.0, border_frame),
        );

        if self.is_loading_analysis || self.analysis.is_none() {
            let elapsed = self.loading_start_time.elapsed().as_secs_f32();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("⏳ Decoding audio track & building waveform... ({:.1}s)", elapsed),
                egui::FontId::proportional(15.0),
                text_f_med,
            );
            return;
        }

        // Layout Partitioning:
        let label_w = 48.0;
        let track_min_x = rect.min.x + label_w;
        let track_max_x = rect.max.x - 6.0;
        let track_width = (track_max_x - track_min_x).max(10.0);

        let ruler_h = 24.0;
        let strip_h = 72.0; // 3x taller to accommodate 2-D multi-note chord stripes
        let scrollbar_h = 14.0;
        let spacing = 4.0;

        let available_view_h =
            (rect.height() - ruler_h - strip_h - scrollbar_h - spacing * 4.0).max(100.0);
        // Equal 50/50 partition so room for the note strip is taken equally from waveform and spectrum views
        let wave_h = (available_view_h * 0.50).max(50.0);
        let spec_h = (available_view_h * 0.50).max(40.0);

        let ruler_rect = Rect::from_min_max(
            Pos2::new(track_min_x, rect.min.y),
            Pos2::new(track_max_x, rect.min.y + ruler_h),
        );
        let wave_rect = Rect::from_min_max(
            Pos2::new(track_min_x, ruler_rect.max.y + spacing),
            Pos2::new(track_max_x, ruler_rect.max.y + spacing + wave_h),
        );
        let spec_rect = Rect::from_min_max(
            Pos2::new(track_min_x, wave_rect.max.y + spacing),
            Pos2::new(track_max_x, wave_rect.max.y + spacing + spec_h),
        );
        let strip_rect = Rect::from_min_max(
            Pos2::new(track_min_x, spec_rect.max.y + spacing),
            Pos2::new(track_max_x, spec_rect.max.y + spacing + strip_h),
        );
        let scroll_rect = Rect::from_min_max(
            Pos2::new(track_min_x, strip_rect.max.y + spacing),
            Pos2::new(track_max_x, strip_rect.max.y + spacing + scrollbar_h),
        );

        // Transform helpers parameterized by (start, dur) to avoid closures holding &self
        let x_to_t = move |x: f32, start: f32, dur: f32| -> f32 {
            start + ((x - track_min_x) / track_width) * dur
        };
        let t_to_x = move |t: f32, start: f32, dur: f32| -> f32 {
            track_min_x + ((t - start) / dur) * track_width
        };

        // --- INTERACTIVE MOUSE / INPUT HANDLING (MUTATES SELF BEFORE PAINTING) ---
        if response.hovered() {
            if let Some(mouse_pos) = response.hover_pos() {
                let handle_grab_radius_px = 10.0;
                let mut near_handle = false;
                match self.selection_mode {
                    SelectionMode::Single => {
                        if let Some((s, e)) = self.selection {
                            let xs = t_to_x(s, self.view_start_sec, self.view_duration_sec);
                            let xe = t_to_x(e, self.view_start_sec, self.view_duration_sec);
                            if (mouse_pos.x - xs).abs() <= handle_grab_radius_px
                                || (mouse_pos.x - xe).abs() <= handle_grab_radius_px
                            {
                                near_handle = true;
                            }
                        }
                    }
                    SelectionMode::Multi => {
                        for &(s, e) in &self.multi_selections {
                            let xs = t_to_x(s, self.view_start_sec, self.view_duration_sec);
                            let xe = t_to_x(e, self.view_start_sec, self.view_duration_sec);
                            if (mouse_pos.x - xs).abs() <= handle_grab_radius_px
                                || (mouse_pos.x - xe).abs() <= handle_grab_radius_px
                            {
                                near_handle = true;
                                break;
                            }
                        }
                    }
                }
                if near_handle {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                }
            }

            // Read raw_scroll_delta directly to avoid smooth_scroll_delta momentum lingering over multiple frames
            let raw_scroll = ui.input(|i| i.raw_scroll_delta);

            if raw_scroll.y.abs() > 0.1 {
                if ui.input(|i| i.modifiers.shift) {
                    // Shift+Wheel pans horizontally
                    let dt = -(raw_scroll.y / 50.0) * self.view_duration_sec * 0.12;
                    self.pan(dt);
                } else {
                    // Consistent, fine-grained exponential zoom curve:
                    // Standard step is ~5.5% per wheel notch (50 pt in egui-winit).
                    // Holding Ctrl or Alt enables ultra-fine zoom (~2.5% per notch).
                    let is_fine = ui.input(|i| i.modifiers.alt || i.modifiers.ctrl);
                    let step_ratio: f32 = if is_fine { 1.025 } else { 1.055 };
                    let steps = (raw_scroll.y / 50.0).clamp(-4.0, 4.0);
                    let factor = step_ratio.powf(-steps);

                    let anchor = if let Some(pos) = response.hover_pos() {
                        x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                            .clamp(0.0, self.duration_sec)
                    } else {
                        self.current_time_sec
                    };
                    self.zoom(factor, anchor);
                }
            } else if raw_scroll.x.abs() > 0.1 {
                // Horizontal wheel pans horizontally
                let dt = -(raw_scroll.x / 50.0) * self.view_duration_sec * 0.12;
                self.pan(dt);
            }
        }

        let is_middle = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let is_primary = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));

        if response.drag_started() {
            if let Some(mouse_pos) = response.interact_pointer_pos() {
                if is_middle {
                    // Holding Middle Mouse Button pans the canvas!
                    self.active_drag = Some(DragAction::PanMiddle {
                        start_x: mouse_pos.x,
                        initial_start_sec: self.view_start_sec,
                    });
                } else if is_primary {
                    if scroll_rect.contains(mouse_pos) {
                        self.active_drag = Some(DragAction::PanScrollbar {
                            start_x: mouse_pos.x,
                            initial_start_sec: self.view_start_sec,
                        });
                    } else {
                        let mouse_t = x_to_t(mouse_pos.x, self.view_start_sec, self.view_duration_sec)
                            .clamp(0.0, self.duration_sec);
                        let is_in_ruler = mouse_pos.y <= ruler_rect.max.y;

                        if is_in_ruler {
                            self.active_drag = Some(DragAction::ScrubRuler);
                            self.seek_to(mouse_t);
                        } else {
                            let handle_grab_radius_px = 10.0;
                            let mut captured = false;

                            match self.selection_mode {
                                SelectionMode::Single => {
                                    if let Some((s, e)) = self.selection {
                                        let xs = t_to_x(s, self.view_start_sec, self.view_duration_sec);
                                        let xe = t_to_x(e, self.view_start_sec, self.view_duration_sec);
                                        if (mouse_pos.x - xs).abs() <= handle_grab_radius_px {
                                            self.active_drag = Some(DragAction::DragLeftHandle { other_anchor: e });
                                            captured = true;
                                        } else if (mouse_pos.x - xe).abs() <= handle_grab_radius_px {
                                            self.active_drag = Some(DragAction::DragRightHandle { other_anchor: s });
                                            captured = true;
                                        }
                                    }

                                    if !captured {
                                        self.active_drag = Some(DragAction::CreateSelection {
                                            start_time: mouse_t,
                                            start_pos_x: mouse_pos.x,
                                            has_moved: false,
                                        });
                                        self.seek_to(mouse_t);
                                    }
                                }
                                SelectionMode::Multi => {
                                    // Check handles of existing multi-selections
                                    for (idx, &(s, e)) in self.multi_selections.iter().enumerate() {
                                        let xs = t_to_x(s, self.view_start_sec, self.view_duration_sec);
                                        let xe = t_to_x(e, self.view_start_sec, self.view_duration_sec);
                                        if (mouse_pos.x - xs).abs() <= handle_grab_radius_px {
                                            self.active_drag = Some(DragAction::DragMultiLeftHandle {
                                                index: idx,
                                                other_anchor: e,
                                            });
                                            self.active_multi_index = Some(idx);
                                            captured = true;
                                            break;
                                        } else if (mouse_pos.x - xe).abs() <= handle_grab_radius_px {
                                            self.active_drag = Some(DragAction::DragMultiRightHandle {
                                                index: idx,
                                                other_anchor: s,
                                            });
                                            self.active_multi_index = Some(idx);
                                            captured = true;
                                            break;
                                        }
                                    }

                                    if !captured {
                                        // Check if clicking inside an existing multi-selection slice to select it
                                        for (idx, &(s, e)) in self.multi_selections.iter().enumerate() {
                                            if mouse_t >= s && mouse_t <= e {
                                                self.active_multi_index = Some(idx);
                                                captured = true;
                                                break;
                                            }
                                        }
                                    }

                                    if !captured {
                                        let new_idx = self.multi_selections.len();
                                        self.multi_selections.push((mouse_t, mouse_t));
                                        self.active_multi_index = Some(new_idx);
                                        self.active_drag = Some(DragAction::CreateMultiSelection {
                                            start_time: mouse_t,
                                            start_pos_x: mouse_pos.x,
                                            has_moved: false,
                                            index: new_idx,
                                        });
                                        self.seek_to(mouse_t);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if response.dragged() {
            if let Some(mut drag) = self.active_drag {
                if let Some(mouse_pos) = response.interact_pointer_pos() {
                    let cur_t = x_to_t(mouse_pos.x, self.view_start_sec, self.view_duration_sec)
                        .clamp(0.0, self.duration_sec);
                    let bypass = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command);
                    match drag {
                        DragAction::PanMiddle { start_x, initial_start_sec } => {
                            let dx = mouse_pos.x - start_x;
                            let dt = (dx / track_width) * self.view_duration_sec;
                            let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                            self.view_start_sec = (initial_start_sec - dt).clamp(0.0, max_start);
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
                        }
                        DragAction::ScrubRuler => {
                            self.seek_to(cur_t);
                        }
                        DragAction::PanScrollbar { start_x, initial_start_sec } => {
                            let dx = mouse_pos.x - start_x;
                            let dt = (dx / scroll_rect.width()) * self.duration_sec;
                            let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                            self.view_start_sec = (initial_start_sec + dt).clamp(0.0, max_start);
                        }
                        DragAction::DragLeftHandle { other_anchor } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            let snapped_t = self.snap_time(cur_t, bypass);
                            let t1 = snapped_t.min(other_anchor);
                            let t2 = snapped_t.max(other_anchor);
                            self.selection = Some((t1, t2));
                            self.seek_to(snapped_t);
                        }
                        DragAction::DragRightHandle { other_anchor } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            let snapped_t = self.snap_time(cur_t, bypass);
                            let t1 = snapped_t.min(other_anchor);
                            let t2 = snapped_t.max(other_anchor);
                            self.selection = Some((t1, t2));
                            self.seek_to(snapped_t);
                        }
                        DragAction::CreateSelection {
                            start_time,
                            start_pos_x,
                            ref mut has_moved,
                        } => {
                            let dx = (mouse_pos.x - start_pos_x).abs();
                            if *has_moved || dx >= 2.0 {
                                *has_moved = true;
                                let s_snapped = self.snap_time(start_time, bypass);
                                let cur_snapped = self.snap_time(cur_t, bypass);
                                let t1 = s_snapped.min(cur_snapped);
                                let t2 = s_snapped.max(cur_snapped);
                                self.selection = Some((t1, t2));
                                self.seek_to(cur_snapped);
                            }
                            self.active_drag = Some(drag);
                        }
                        DragAction::DragMultiLeftHandle { index, other_anchor } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            if index < self.multi_selections.len() {
                                let snapped_t = self.snap_time(cur_t, bypass);
                                let t1 = snapped_t.min(other_anchor);
                                let t2 = snapped_t.max(other_anchor);
                                self.multi_selections[index] = (t1, t2);
                                self.seek_to(snapped_t);
                            }
                        }
                        DragAction::DragMultiRightHandle { index, other_anchor } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            if index < self.multi_selections.len() {
                                let snapped_t = self.snap_time(cur_t, bypass);
                                let t1 = snapped_t.min(other_anchor);
                                let t2 = snapped_t.max(other_anchor);
                                self.multi_selections[index] = (t1, t2);
                                self.seek_to(snapped_t);
                            }
                        }
                        DragAction::CreateMultiSelection {
                            start_time,
                            start_pos_x,
                            ref mut has_moved,
                            index,
                        } => {
                            let dx = (mouse_pos.x - start_pos_x).abs();
                            if *has_moved || dx >= 2.0 {
                                *has_moved = true;
                                let s_snapped = self.snap_time(start_time, bypass);
                                let cur_snapped = self.snap_time(cur_t, bypass);
                                let t1 = s_snapped.min(cur_snapped);
                                let t2 = s_snapped.max(cur_snapped);
                                if index < self.multi_selections.len() {
                                    self.multi_selections[index] = (t1, t2);
                                }
                                self.seek_to(cur_snapped);
                            }
                            self.active_drag = Some(drag);
                        }
                    }
                }
            }
        } else if response.drag_stopped() {
            if let Some(drag) = self.active_drag {
                match drag {
                    DragAction::CreateSelection { has_moved, .. } => {
                        if !has_moved {
                            self.selection = None;
                        }
                    }
                    DragAction::CreateMultiSelection { has_moved, index, .. } => {
                        if !has_moved {
                            if index < self.multi_selections.len() {
                                self.multi_selections.remove(index);
                            }
                            self.active_multi_index = None;
                        } else if index < self.multi_selections.len() {
                            let (s, e) = self.multi_selections[index];
                            let (s_fix, e_fix) = (s.min(e), s.max(e));
                            self.multi_selections[index] = (s_fix, e_fix);
                            self.multi_selections.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                            self.active_multi_index = self.multi_selections.iter().position(|r| *r == (s_fix, e_fix));
                        }
                    }
                    _ => {}
                }
            }
            self.active_drag = None;
        } else if response.secondary_clicked() {
            // Right-click: preview selected slice from start to stop (Shift+Right-click removes/clears)
            if let Some(pos) = response.interact_pointer_pos() {
                let clicked_t = x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                    .clamp(0.0, self.duration_sec);
                let shift_down = ui.input(|i| i.modifiers.shift);

                match self.selection_mode {
                    SelectionMode::Multi => {
                        let clicked_slice = self
                            .multi_selections
                            .iter()
                            .enumerate()
                            .find(|(_, (s, e))| clicked_t >= *s && clicked_t <= *e)
                            .map(|(idx, &(s, e))| (idx, s, e));

                        if shift_down {
                            if let Some((idx, _, _)) = clicked_slice {
                                self.multi_selections.remove(idx);
                                self.active_multi_index = None;
                                self.status_toast = Some((format!("Removed slice #{}", idx + 1), ui.input(|i| i.time)));
                            }
                        } else if let Some((idx, s, e)) = clicked_slice {
                            self.active_multi_index = Some(idx);
                            self.play_slice_preview(s, e);
                            self.status_toast = Some((
                                format!("▶ Previewing slice #{} ({:02}:{:04.1} - {:02}:{:04.1})", idx + 1, (s / 60.0) as u32, s % 60.0, (e / 60.0) as u32, e % 60.0),
                                ui.input(|i| i.time),
                            ));
                        } else if let Some(idx) = self.active_multi_index {
                            if idx < self.multi_selections.len() {
                                let (s, e) = self.multi_selections[idx];
                                self.play_slice_preview(s, e);
                                self.status_toast = Some((
                                    format!("▶ Previewing slice #{} ({:02}:{:04.1} - {:02}:{:04.1})", idx + 1, (s / 60.0) as u32, s % 60.0, (e / 60.0) as u32, e % 60.0),
                                    ui.input(|i| i.time),
                                ));
                            }
                        }
                    }
                    SelectionMode::Single => {
                        if let Some((s, e)) = self.selection {
                            let (s_min, s_max) = (s.min(e), s.max(e));
                            let is_inside = clicked_t >= s_min && clicked_t <= s_max;
                            if shift_down && is_inside {
                                self.selection = None;
                                self.status_toast = Some(("Cleared selection".to_string(), ui.input(|i| i.time)));
                            } else {
                                self.play_slice_preview(s_min, s_max);
                                self.status_toast = Some((
                                    format!("▶ Previewing slice ({:02}:{:04.1} - {:02}:{:04.1})", (s_min / 60.0) as u32, s_min % 60.0, (s_max / 60.0) as u32, s_max % 60.0),
                                    ui.input(|i| i.time),
                                ));
                            }
                        }
                    }
                }
            }
        } else if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                if scroll_rect.contains(pos) {
                    let ratio = (pos.x - scroll_rect.min.x) / scroll_rect.width();
                    let target_start = ratio * self.duration_sec - self.view_duration_sec * 0.5;
                    let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                    self.view_start_sec = target_start.clamp(0.0, max_start);
                } else {
                    let clicked_t = x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                        .clamp(0.0, self.duration_sec);
                    let is_in_ruler = pos.y <= ruler_rect.max.y;
                    if !is_in_ruler {
                        match self.selection_mode {
                            SelectionMode::Single => {
                                self.selection = None;
                            }
                            SelectionMode::Multi => {
                                if let Some(idx) = self.multi_selections.iter().position(|(s, e)| clicked_t >= *s && clicked_t <= *e) {
                                    self.active_multi_index = Some(idx);
                                } else {
                                    self.active_multi_index = None;
                                }
                            }
                        }
                    }
                    self.seek_to(clicked_t);
                }
            }
        } else if response.double_clicked() {
            match self.selection_mode {
                SelectionMode::Single => {
                    if self.selection.is_some() {
                        self.selection = None;
                    } else {
                        self.selection = Some((0.0, self.duration_sec));
                    }
                }
                SelectionMode::Multi => {
                    if self.multi_selections.is_empty() {
                        self.multi_selections.push((0.0, self.duration_sec));
                        self.active_multi_index = Some(0);
                    } else {
                        self.multi_selections.clear();
                        self.active_multi_index = None;
                    }
                }
            }
        }

        // --- PAINTING PHASE: ALL MUTATIONS COMPLETED ---
        let v_start = self.view_start_sec;
        let v_dur = self.view_duration_sec.max(0.0001);
        let time_to_x = move |t: f32| -> f32 {
            track_min_x + ((t - v_start) / v_dur) * track_width
        };
        let x_to_time = move |x: f32| -> f32 {
            v_start + ((x - track_min_x) / track_width) * v_dur
        };

        let analysis = self.analysis.as_ref().unwrap();

        // 1. RULER
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y), Pos2::new(rect.max.x, ruler_rect.max.y)),
            Rounding { nw: 8.0, ne: 8.0, sw: 0.0, se: 0.0 },
            bg_frame,
        );
        painter.line_segment(
            [Pos2::new(rect.min.x, ruler_rect.max.y), Pos2::new(rect.max.x, ruler_rect.max.y)],
            Stroke::new(1.0, border_frame),
        );

        // Dynamic Time Ticks
        let interval_sec = if v_dur > 120.0 {
            30.0
        } else if v_dur > 30.0 {
            10.0
        } else if v_dur > 10.0 {
            2.0
        } else if v_dur > 2.0 {
            0.5
        } else if v_dur > 0.5 {
            0.1
        } else if v_dur > 0.1 {
            0.02
        } else if v_dur > 0.02 {
            0.005
        } else {
            0.001
        };

        let first_marker = (v_start / interval_sec).floor() * interval_sec;
        let mut t = first_marker;
        let max_t = v_start + v_dur;
        while t <= max_t + interval_sec * 0.5 {
            if t >= v_start {
                let x = time_to_x(t);
                if x >= track_min_x && x <= track_max_x {
                    // Grid line down through all views
                    painter.line_segment(
                        [Pos2::new(x, ruler_rect.max.y), Pos2::new(x, strip_rect.max.y)],
                        Stroke::new(1.0, crate::hex_to_c32_alpha(&palette.theme.f_low, 20)),
                    );
                    // Ruler tick
                    painter.line_segment(
                        [Pos2::new(x, ruler_rect.min.y + 14.0), Pos2::new(x, ruler_rect.max.y)],
                        Stroke::new(1.0, crate::hex_to_c32_alpha(&palette.theme.f_med, 80)),
                    );
                    // Label
                    let label = if interval_sec >= 1.0 {
                        let m = (t / 60.0) as u32;
                        let s = (t % 60.0) as u32;
                        format!("{:02}:{:02}", m, s)
                    } else if interval_sec >= 0.01 {
                        let m = (t / 60.0) as u32;
                        let s = t % 60.0;
                        format!("{:02}:{:05.2}", m, s)
                    } else {
                        let s = t % 60.0;
                        format!("{:06.3}s", s)
                    };
                    painter.text(
                        Pos2::new(x + 3.0, ruler_rect.min.y + 10.0),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(9.0),
                        text_f_med,
                    );
                }
            }
            t += interval_sec;
        }

        // 2. VIEW 1: WAVEFORM (Audacity-style Stems or Peak Bars)
        painter.rect_filled(wave_rect, Rounding::ZERO, bg_frame);
        painter.rect_stroke(wave_rect, Rounding::ZERO, Stroke::new(1.0, border_frame));

        let is_stereo = analysis.channels >= 2 && analysis.ch2_samples.is_some();
        let total_frames = analysis.total_frames;
        let sample_rate = analysis.sample_rate as f32;

        let start_frame = ((v_start * sample_rate).floor() as isize).max(0) as usize;
        let end_frame = (((v_start + v_dur) * sample_rate).ceil() as usize).min(total_frames);
        let visible_frames = end_frame.saturating_sub(start_frame);
        let px_per_frame = track_width / visible_frames.max(1) as f32;

        if is_stereo {
            let half = wave_h / 2.0;
            let y_c1 = wave_rect.min.y + half * 0.5;
            let y_c2 = wave_rect.min.y + half * 1.5;

            painter.line_segment(
                [Pos2::new(track_min_x, wave_rect.min.y + half), Pos2::new(track_max_x, wave_rect.min.y + half)],
                Stroke::new(1.0, border_frame),
            );

            painter.text(
                Pos2::new(track_min_x - 8.0, y_c1),
                egui::Align2::RIGHT_CENTER,
                "L",
                egui::FontId::proportional(11.0),
                accent_base,
            );
            painter.text(
                Pos2::new(track_min_x - 8.0, y_c2),
                egui::Align2::RIGHT_CENTER,
                "R",
                egui::FontId::proportional(11.0),
                accent_base,
            );
        } else {
            painter.text(
                Pos2::new(track_min_x - 8.0, wave_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "Mono",
                egui::FontId::proportional(11.0),
                accent_base,
            );
        }

        if px_per_frame >= 3.0 {
            // --- AUDACITY STEM MODE ---
            let ch_count = if is_stereo { 2 } else { 1 };
            for ch_idx in 0..ch_count {
                let (y_base, max_def, samples, _col) = if ch_idx == 0 {
                    let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                    let base = if is_stereo { wave_rect.min.y + wave_h / 4.0 } else { wave_rect.center().y };
                    (base, half * 0.45, &analysis.ch1_samples, accent_base)
                } else {
                    let half = wave_h / 2.0;
                    let base = wave_rect.min.y + 3.0 * wave_h / 4.0;
                    (base, half * 0.45, analysis.ch2_samples.as_ref().unwrap(), accent_base)
                };

                painter.line_segment(
                    [Pos2::new(track_min_x, y_base), Pos2::new(track_max_x, y_base)],
                    Stroke::new(1.0, border_frame),
                );

                let mut prev_pos: Option<Pos2> = None;
                for f in start_frame..=end_frame.min(total_frames.saturating_sub(1)) {
                    let t = f as f32 / sample_rate;
                    let x = time_to_x(t);
                    let val = samples[f].clamp(-1.0, 1.0);
                    let y = y_base - val * max_def;
                    let pos = Pos2::new(x, y);

                    let in_sel = self.is_time_selected(t);
                    let point_col = if in_sel {
                        accent_base
                    } else {
                        let c = analysis.query_sample_color(ch_idx, f);
                        Color32::from_rgb(c[0], c[1], c[2])
                    };

                    if let Some(p_pos) = prev_pos {
                        painter.line_segment([p_pos, pos], Stroke::new(1.5, point_col));
                    }

                    painter.line_segment(
                        [Pos2::new(x, y_base), pos],
                        Stroke::new(
                            1.0,
                            Color32::from_rgba_unmultiplied(point_col.r(), point_col.g(), point_col.b(), 120),
                        ),
                    );

                    painter.circle_filled(pos, 3.0, point_col);
                    painter.circle_stroke(pos, 3.0, Stroke::new(1.0, border_frame));

                    prev_pos = Some(pos);
                }
            }
        } else {
            // --- OVERVIEW / PEAK ENVELOPE MODE (INSTANTANEOUS VIA PYRAMID + MESH) ---
            let num_bars = (track_width as usize).clamp(100, 1400);
            let bar_w = (track_width / num_bars as f32).max(1.0);

            let ch_count = if is_stereo { 2 } else { 1 };
            for ch_idx in 0..ch_count {
                let (y_base, max_def, _col) = if ch_idx == 0 {
                    let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                    let base = if is_stereo { wave_rect.min.y + wave_h / 4.0 } else { wave_rect.center().y };
                    (base, half * 0.45, accent_base)
                } else {
                    let half = wave_h / 2.0;
                    let base = wave_rect.min.y + 3.0 * wave_h / 4.0;
                    (base, half * 0.45, accent_base)
                };

                painter.line_segment(
                    [Pos2::new(track_min_x, y_base), Pos2::new(track_max_x, y_base)],
                    Stroke::new(1.0, border_frame),
                );

                let mut mesh = egui::Mesh::default();
                for b in 0..num_bars {
                    let bar_x = track_min_x + b as f32 * bar_w;
                    let t_start = x_to_time(bar_x);
                    let t_end = x_to_time(bar_x + bar_w);
                    let t_mid = (t_start + t_end) * 0.5;

                    let f_s = ((t_start * sample_rate) as usize).min(total_frames);
                    let f_e = (((t_end * sample_rate) as usize) + 1).min(total_frames);

                    let (min_s, max_s, block_col) = analysis.query_peaks(ch_idx, f_s, f_e);

                    let in_sel = self.is_time_selected(t_mid);
                    let bar_col = if in_sel {
                        accent_base
                    } else {
                        Color32::from_rgb(block_col[0], block_col[1], block_col[2])
                    };

                    let y_top = y_base - max_s * max_def;
                    let y_bot = y_base - min_s * max_def;
                    let h = (y_bot - y_top).max(1.0);

                    mesh.add_colored_rect(
                        Rect::from_min_size(Pos2::new(bar_x, y_top), Vec2::new(bar_w.max(1.0), h)),
                        bar_col,
                    );
                }
                painter.add(egui::Shape::mesh(mesh));
            }
        }

        // Render transient marks across waveform when enabled
        if self.show_transients {
            for &t_trans in &analysis.transients {
                if t_trans >= v_start && t_trans <= v_start + v_dur {
                    let tx = time_to_x(t_trans);
                    // Draw vertical transient marker line
                    painter.line_segment(
                        [Pos2::new(tx, wave_rect.min.y), Pos2::new(tx, wave_rect.max.y)],
                        Stroke::new(1.0, transient_col),
                    );
                    // Draw transient marker pip/diamond at top of waveform
                    let pip_top = Pos2::new(tx, wave_rect.min.y + 1.0);
                    let pip_left = Pos2::new(tx - 3.5, wave_rect.min.y + 5.5);
                    let pip_right = Pos2::new(tx + 3.5, wave_rect.min.y + 5.5);
                    let pip_bot = Pos2::new(tx, wave_rect.min.y + 10.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![pip_top, pip_right, pip_bot, pip_left],
                        transient_col,
                        Stroke::new(0.5, border_frame),
                    ));
                }
            }
        }

        // 3. VIEW 2: SPECTROGRAM (Spectral Energy Heatmap Over Time)
        let freq_labels = [
            (0.05, "20k"),
            (0.25, "10k"),
            (0.50, "5k"),
            (0.75, "1k"),
            (0.95, "100"),
        ];
        for (ratio, lbl) in freq_labels {
            let y = spec_rect.min.y + ratio * spec_h;
            painter.text(
                Pos2::new(track_min_x - 8.0, y),
                egui::Align2::RIGHT_CENTER,
                lbl,
                egui::FontId::monospace(9.0),
                text_f_med,
            );
            painter.line_segment(
                [Pos2::new(track_min_x, y), Pos2::new(track_max_x, y)],
                Stroke::new(1.0, border_frame),
            );
        }

        if let Some(ref tex) = self.spectrogram_texture {
            let total_dur = self.duration_sec.max(0.01);
            let u_start = (v_start / total_dur).clamp(0.0, 1.0);
            let u_end = ((v_start + v_dur) / total_dur).clamp(0.0, 1.0);
            let uv = Rect::from_min_max(Pos2::new(u_start, 0.0), Pos2::new(u_end, 1.0));
            painter.image(tex.id(), spec_rect, uv, Color32::WHITE);
        } else {
            painter.rect_filled(spec_rect, Rounding::ZERO, bg_frame);
            painter.text(
                spec_rect.center(),
                egui::Align2::CENTER_CENTER,
                "⏳ Generating Spectrogram Heatmap...",
                egui::FontId::proportional(12.0),
                text_f_low,
            );
        }
        painter.rect_stroke(spec_rect, Rounding::ZERO, Stroke::new(1.0, border_frame));

        // 4. VIEW 3: COLORCHORD 2-D MULTI-NOTE CHROMATIC RIBBON
        painter.text(
            Pos2::new(track_min_x - 8.0, strip_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "♫ Chord",
            egui::FontId::proportional(10.0),
            text_f_med,
        );

        if let Some(ref tex) = self.note_strip_texture {
            let total_dur = self.duration_sec.max(0.01);
            let u_start = (v_start / total_dur).clamp(0.0, 1.0);
            let u_end = ((v_start + v_dur) / total_dur).clamp(0.0, 1.0);
            let uv = Rect::from_min_max(Pos2::new(u_start, 0.0), Pos2::new(u_end, 1.0));
            // Paint theme background under texture so silent/empty note regions match active theme
            painter.rect_filled(strip_rect, Rounding::ZERO, bg_frame);
            painter.image(tex.id(), strip_rect, uv, Color32::WHITE);
        } else {
            painter.rect_filled(strip_rect, Rounding::ZERO, bg_frame);
            painter.text(
                strip_rect.center(),
                egui::Align2::CENTER_CENTER,
                "⏳ Analyzing Multi-Note Chords...",
                egui::FontId::proportional(12.0),
                text_f_low,
            );
        }
        painter.rect_stroke(strip_rect, Rounding::ZERO, Stroke::new(1.0, border_frame));

        // Hover tooltip over ColorChord note strip (multi-note chord display)
        if let Some(mouse_pos) = response.hover_pos() {
            if strip_rect.contains(mouse_pos) {
                let mouse_t = x_to_time(mouse_pos.x);
                let total_dur = self.duration_sec.max(0.01);
                let col_idx = (((mouse_t / total_dur) * (analysis.note_strip_width as f32)).floor() as usize)
                    .min(analysis.note_strip_info.len().saturating_sub(1));
                if let Some(info) = analysis.note_strip_info.get(col_idx) {
                    let text = if !info.chord_notes.is_empty() {
                        if info.chord_notes.len() > 1 {
                            let names = info.chord_notes.iter().map(|n| n.note_name.as_str()).collect::<Vec<_>>().join(" + ");
                            format!("🎵 Detected Chord: {} • Energy: {:.0}%", names, info.amplitude * 100.0)
                        } else {
                            format!("🎵 Musical Note: {} ({:.1} Hz) • Energy: {:.0}%", info.note_name, info.frequency_hz, info.amplitude * 100.0)
                        }
                    } else if info.active {
                        format!("🎵 Musical Note: {} ({:.1} Hz) • Energy: {:.0}%", info.note_name, info.frequency_hz, info.amplitude * 100.0)
                    } else {
                        "🎵 Musical Note: — (Silence / Below Threshold)".to_string()
                    };
                    egui::show_tooltip(ui.ctx(), response.id.with("note_tt"), |ui| {
                        ui.label(egui::RichText::new(text).strong().color(crate::hex_to_c32(&palette.theme.f_high)));
                    });
                }
            }
        }

        // 5. HORIZONTAL VIEWPORT SCROLLBAR / PAN TRACK
        painter.rect_filled(scroll_rect, Rounding::same(3.0), bg_frame);
        painter.rect_stroke(scroll_rect, Rounding::same(3.0), Stroke::new(1.0, border_frame));

        let total_dur = self.duration_sec.max(0.01);
        let thumb_x = scroll_rect.min.x + (v_start / total_dur) * scroll_rect.width();
        let thumb_w = ((v_dur / total_dur) * scroll_rect.width()).clamp(16.0, scroll_rect.width());
        let thumb_rect = Rect::from_min_size(Pos2::new(thumb_x, scroll_rect.min.y), Vec2::new(thumb_w, scroll_rect.height()));

        painter.rect_filled(thumb_rect, Rounding::same(3.0), border_frame);
        painter.rect_stroke(thumb_rect, Rounding::same(3.0), Stroke::new(1.0, text_f_med));

        // 6. UNIFIED SELECTION OVERLAY ACROSS ALL 3 VIEWS
        let view_top_y = ruler_rect.max.y;
        let view_bot_y = strip_rect.max.y;

        match self.selection_mode {
            SelectionMode::Single => {
                if let Some((start, end)) = self.selection {
                    let x_start = time_to_x(start);
                    let x_end = time_to_x(end);

                    if x_end > x_start {
                        let sel_rect = Rect::from_min_max(
                            Pos2::new(x_start.max(track_min_x), view_top_y),
                            Pos2::new(x_end.min(track_max_x), view_bot_y),
                        );
                        painter.rect_filled(
                            sel_rect,
                            Rounding::ZERO,
                            crate::hex_to_c32_alpha(&palette.harmonies.base, 40),
                        );

                        if x_start >= track_min_x && x_start <= track_max_x {
                            painter.line_segment(
                                [Pos2::new(x_start, ruler_rect.min.y), Pos2::new(x_start, view_bot_y)],
                                Stroke::new(2.5, accent_base),
                            );
                            painter.circle_filled(
                                Pos2::new(x_start, ruler_rect.center().y),
                                5.0,
                                accent_base,
                            );
                        }
                        if x_end >= track_min_x && x_end <= track_max_x {
                            painter.line_segment(
                                [Pos2::new(x_end, ruler_rect.min.y), Pos2::new(x_end, view_bot_y)],
                                Stroke::new(2.5, accent_base),
                            );
                            painter.circle_filled(
                                Pos2::new(x_end, ruler_rect.center().y),
                                5.0,
                                accent_base,
                            );
                        }
                    }
                }
            }
            SelectionMode::Multi => {
                for (idx, &(start, end)) in self.multi_selections.iter().enumerate() {
                    let is_active = self.active_multi_index == Some(idx);
                    let x_start = time_to_x(start);
                    let x_end = time_to_x(end);

                    if x_end > x_start {
                        let sel_rect = Rect::from_min_max(
                            Pos2::new(x_start.max(track_min_x), view_top_y),
                            Pos2::new(x_end.min(track_max_x), view_bot_y),
                        );
                        let alpha = if is_active { 55 } else { 30 };
                        painter.rect_filled(
                            sel_rect,
                            Rounding::ZERO,
                            crate::hex_to_c32_alpha(&palette.harmonies.base, alpha),
                        );

                        let handle_color = accent_base;
                        let stroke_width = if is_active { 2.5 } else { 1.5 };

                        if x_start >= track_min_x && x_start <= track_max_x {
                            painter.line_segment(
                                [Pos2::new(x_start, ruler_rect.min.y), Pos2::new(x_start, view_bot_y)],
                                Stroke::new(stroke_width, handle_color),
                            );
                            painter.circle_filled(
                                Pos2::new(x_start, ruler_rect.center().y),
                                if is_active { 5.0 } else { 3.5 },
                                handle_color,
                            );
                        }
                        if x_end >= track_min_x && x_end <= track_max_x {
                            painter.line_segment(
                                [Pos2::new(x_end, ruler_rect.min.y), Pos2::new(x_end, view_bot_y)],
                                Stroke::new(stroke_width, handle_color),
                            );
                            painter.circle_filled(
                                Pos2::new(x_end, ruler_rect.center().y),
                                if is_active { 5.0 } else { 3.5 },
                                handle_color,
                            );
                        }

                        // Badge on ruler top showing slice index "#1", "#2"
                        if x_start <= track_max_x && x_end >= track_min_x {
                            let badge_x = (x_start + 4.0).max(track_min_x + 2.0).min(track_max_x - 30.0);
                            let badge_rect = Rect::from_min_size(
                                Pos2::new(badge_x, ruler_rect.min.y + 3.0),
                                Vec2::new(26.0, 14.0),
                            );
                            let badge_bg = if is_active {
                                border_frame
                            } else {
                                bg_frame
                            };
                            painter.rect_filled(badge_rect, Rounding::same(3.0), badge_bg);
                            painter.text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("#{}", idx + 1),
                                egui::FontId::monospace(9.0),
                                if is_active { accent_base } else { text_f_med },
                            );
                        }
                    }
                }
            }
        }

        // 7. UNIFIED PLAYHEAD CURSOR ACROSS ALL 3 VIEWS
        let playhead_x = time_to_x(self.current_time_sec);
        if playhead_x >= track_min_x && playhead_x <= track_max_x {
            painter.line_segment(
                [Pos2::new(playhead_x, ruler_rect.min.y), Pos2::new(playhead_x, view_bot_y)],
                Stroke::new(2.0, accent_playhead),
            );
            let head_top = Pos2::new(playhead_x, ruler_rect.min.y);
            let diamond = [
                Pos2::new(head_top.x - 5.0, head_top.y),
                Pos2::new(head_top.x, head_top.y + 12.0),
                Pos2::new(head_top.x + 5.0, head_top.y),
                Pos2::new(head_top.x, head_top.y - 4.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                diamond.to_vec(),
                accent_playhead,
                Stroke::NONE,
            ));
        }

        // Precise Scrub Time Badge during ruler drag
        if matches!(self.active_drag, Some(DragAction::ScrubRuler)) {
            let badge_pos = Pos2::new((playhead_x + 8.0).min(track_max_x - 65.0), ruler_rect.min.y + 26.0);
            let cur_m = (self.current_time_sec / 60.0) as u32;
            let cur_s = self.current_time_sec % 60.0;
            let badge_text = format!("{:02}:{:05.2}", cur_m, cur_s);
            painter.rect_filled(
                Rect::from_min_size(badge_pos, Vec2::new(65.0, 18.0)),
                Rounding::same(4.0),
                crate::hex_to_c32_alpha(&palette.theme.b_low, 220),
            );
            painter.text(
                Pos2::new(badge_pos.x + 4.0, badge_pos.y + 9.0),
                egui::Align2::LEFT_CENTER,
                badge_text,
                egui::FontId::monospace(10.0),
                crate::hex_to_c32(&palette.diff_del),
            );
        }

        // Dynamic Cursor Feedback
        let is_middle_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        if is_middle_down || matches!(self.active_drag, Some(DragAction::PanMiddle { .. })) {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
        } else if let Some(hover_pos) = response.hover_pos() {
            let is_in_ruler = hover_pos.y <= ruler_rect.max.y;
            let is_in_scroll = scroll_rect.contains(hover_pos);
            if is_in_ruler || is_in_scroll {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            } else {
                let mut near_handle = false;
                match self.selection_mode {
                    SelectionMode::Single => {
                        if let Some((s, e)) = self.selection {
                            let xs = time_to_x(s);
                            let xe = time_to_x(e);
                            if (hover_pos.x - xs).abs() <= 6.0 || (hover_pos.x - xe).abs() <= 6.0 {
                                near_handle = true;
                            }
                        }
                    }
                    SelectionMode::Multi => {
                        for &(s, e) in &self.multi_selections {
                            let xs = time_to_x(s);
                            let xe = time_to_x(e);
                            if (hover_pos.x - xs).abs() <= 6.0 || (hover_pos.x - xe).abs() <= 6.0 {
                                near_handle = true;
                                break;
                            }
                        }
                    }
                }

                if near_handle {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                } else {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::audio::compute_full_audio_analysis;

    #[test]
    fn test_audio_preview_state_creation_and_seek() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
            assert!(state.duration_sec > 4.0);
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
    }

    #[test]
    fn test_zoom_and_pan_calculations() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
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
    }

    #[test]
    fn test_channel_slicing_modes() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
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
    }

    #[test]
    fn test_small_sub_50ms_slice_selection() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
            state.selection = Some((1.000, 1.020));
            state.copy_selected_segment(1.0, ChannelCopyMode::Native);
            assert!(state.status_toast.is_some());
            let (toast, _) = state.status_toast.unwrap();
            assert!(toast.contains("Copied slice"));
            assert!(toast.contains("0.020s"));
        }
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
            f.write_all(&(sample_rate * num_channels as u32 * 2).to_le_bytes()).unwrap();
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

        let surround_analysis = compute_full_audio_analysis(&surround_path, 50).expect("analyze 5.1");
        assert_eq!(surround_analysis.channels, 6);
        assert!(surround_analysis.channels > 2); // Triggers warning badge in UI
        assert!(surround_analysis.ch2_samples.is_some()); // Displays L and R
        let _ = std::fs::remove_file(&surround_path);
    }

    #[test]
    fn test_multi_region_mode_and_removal() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
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
    }

    #[test]
    fn test_zero_crossing_snapping_integration() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
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
    }

    #[test]
    fn test_transient_toggle_and_snapping_integration() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
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

            // Holding Shift or Ctrl bypasses snapping even when show_transients is ON
            let bypassed = state.snap_time(test_t, true);
            assert_eq!(bypassed, test_t);

            // Toggle transients OFF -> should not snap to transient
            state.show_transients = false;
            let not_snapped = state.snap_time(test_t, false);
            assert_eq!(not_snapped, test_t);
        }
    }

    #[test]
    fn test_middle_mouse_pan_calculation() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
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
        assert_eq!(mono_left_name, "DrumBeat_01m15s000ms_02s000ms_left_mono.wav");
    }

    #[test]
    fn test_save_to_file_single_and_multi() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
        if p.is_file() {
            let mut state = AudioPreviewState::new(p.clone(), "TAPE_NOISE_04.wav".to_string());
            let temp_dest = std::env::temp_dir().join("little_clip_save_test");
            let _ = std::fs::create_dir_all(&temp_dest);
            state.save_destination_dir = temp_dest.to_string_lossy().to_string();
            state.open_folder_after_save = false; // don't open file manager in test

            // 1. Single Selection Save
            state.set_selection_mode(SelectionMode::Single);
            state.selection = Some((1.0, 2.5)); // start 1.0s, duration 1.5s
            state.open_save_dialog();
            assert!(state.save_dialog_open);
            assert_eq!(state.save_filenames.len(), 1);
            assert_eq!(state.save_filenames[0], "TAPE_NOISE_04_00m01s000ms_01s500ms.wav");

            let res = state.execute_save(1.0);
            assert!(res.is_ok());
            assert_eq!(res.unwrap(), 1);
            let saved_single = temp_dest.join("TAPE_NOISE_04_00m01s000ms_01s500ms.wav");
            assert!(saved_single.exists());
            assert!(std::fs::metadata(&saved_single).unwrap().len() > 0);

            // 2. Multi-Selection Save
            state.set_selection_mode(SelectionMode::Multi);
            state.multi_selections = vec![(0.5, 1.2), (2.0, 2.8)];
            state.open_save_dialog();
            assert!(state.save_dialog_open);
            assert_eq!(state.save_filenames.len(), 2);
            assert_eq!(state.save_filenames[0], "TAPE_NOISE_04_00m00s500ms_00s700ms.wav");
            assert_eq!(state.save_filenames[1], "TAPE_NOISE_04_00m02s000ms_00s800ms.wav");

            let res_multi = state.execute_save(2.0);
            assert!(res_multi.is_ok());
            assert_eq!(res_multi.unwrap(), 2);

            let saved_multi_1 = temp_dest.join("TAPE_NOISE_04_00m00s500ms_00s700ms.wav");
            let saved_multi_2 = temp_dest.join("TAPE_NOISE_04_00m02s000ms_00s800ms.wav");
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
    }

    #[test]
    fn test_slice_preview_start_to_stop() {
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
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
        let p = PathBuf::from("/home/vega/Documents/type_testing/TAPE_NOISE_04.wav");
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
}
