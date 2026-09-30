//! Piano roll visualization and MIDI playback workbench view.
//!
//! Renders multi-track MIDI piano rolls, measure grid lines, note inspection,
//! and live audio synthesis playback via Rodio.

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

/// Interactive UI state for piano roll inspection and MIDI playback.
pub struct MidiViewerState {
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub was_playing_before_drag: bool,
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

fn midi_export_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir)
            .join("little-screen-clip")
            .join("midi_slices")
    } else {
        PathBuf::from("/tmp/little-screen-clip/midi_slices")
    }
}

fn write_unique_midi_export(
    slice_dir: &std::path::Path,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    let base_path = slice_dir.join(filename);
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("midi_export");
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("mid");

    for suffix in 0u32.. {
        let path = if suffix == 0 {
            base_path.clone()
        } else {
            slice_dir.join(format!("{stem}_{suffix}.{extension}"))
        };
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Failed to write MIDI file: {error}")),
        };
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!("Failed to write MIDI file: {error}"));
        }
        return Ok(path);
    }
    unreachable!("u32 collision suffix space exhausted")
}

fn write_midi_export(
    data: &MidiData,
    start_sec: f32,
    end_sec: f32,
    is_slice: bool,
    solo_track: Option<usize>,
    timestamp: u128,
    slice_dir: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<MidiExportArtifact, String> {
    let sliced_data = data.slice(start_sec, end_sec, solo_track);
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    let midi_bytes = sliced_data
        .to_midi_bytes()
        .map_err(|error| format!("Failed to export MIDI: {error}"))?;
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }

    std::fs::create_dir_all(&slice_dir).map_err(|error| {
        tracing::warn!(
            "Failed to create midi slice directory {}: {error}",
            slice_dir.display()
        );
        format!("Failed to create slice directory: {error}")
    })?;

    let track_suffix = solo_track.map_or_else(String::new, |track| {
        let name = data
            .track_names
            .get(track)
            .map(|name| name.replace(|c: char| !c.is_alphanumeric() && c != '_', "_"))
            .unwrap_or_else(|| format!("track_{}", track + 1));
        format!("_{name}")
    });
    let filename = if is_slice {
        format!("midi_slice_{timestamp}_{start_sec:.1}s-{end_sec:.1}s{track_suffix}.mid")
    } else {
        format!("midi_export_{timestamp}{track_suffix}.mid")
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    let path = write_unique_midi_export(&slice_dir, &filename, &midi_bytes)?;

    let artifact = MidiExportArtifact {
        path,
        total_notes: sliced_data.total_notes,
        duration_sec: end_sec - start_sec,
        is_slice,
        solo_info: solo_track.map_or_else(String::new, |track| {
            let name = data
                .track_names
                .get(track)
                .map(String::as_str)
                .unwrap_or("track");
            format!(" (solo '{name}')")
        }),
        delivery: MidiExportDelivery::PushedToWorkbench,
        retain_file: false,
    };
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    Ok(artifact)
}

fn handoff_midi_export(
    mut artifact: MidiExportArtifact,
    cancelled: &AtomicBool,
    started: Instant,
) -> Result<MidiExportArtifact, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    // Keep enough of the job deadline for bounded IPC and the clipboard fallback.
    if started.elapsed() >= Duration::from_secs(45) {
        return Err("MIDI export timed out before handoff".to_string());
    }
    let request = IpcRequest::PushFile {
        path: artifact.path.clone(),
        hand: HandSide::Left,
    };
    if crate::send_ipc_request_with_timeout(&request, Some(Duration::from_secs(2))).is_ok() {
        artifact.retain();
        return Ok(artifact);
    }

    if cancelled.load(Ordering::Acquire) {
        return Err("MIDI export cancelled".to_string());
    }
    little_clip_core::copy_file_uris_to_system_clipboard(std::slice::from_ref(&artifact.path))
        .map_err(|error| format!("Failed to copy MIDI file to system clipboard: {error}"))?;
    artifact.delivery = MidiExportDelivery::ClipboardFallback;
    artifact.retain();
    Ok(artifact)
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

    #[allow(dead_code)]
    pub fn from_bytes(bytes: &[u8], title: String) -> Result<Self, String> {
        let data = MidiData::parse_bytes(bytes, title.clone())?;
        Ok(Self::from_data(data, None, title))
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
            was_playing_before_drag: false,
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

    pub fn copy_selection(&mut self, now: f64) -> bool {
        self.copy_selection_on(
            now,
            BackgroundJobs::global(),
            midi_export_dir(),
            handoff_midi_export,
        )
    }

    fn copy_selection_on(
        &mut self,
        now: f64,
        jobs: &BackgroundJobs,
        export_dir: PathBuf,
        handoff: impl FnOnce(
            MidiExportArtifact,
            &AtomicBool,
            Instant,
        ) -> Result<MidiExportArtifact, String>
        + Send
        + 'static,
    ) -> bool {
        let (s, e, is_slice) = match self.selected_region {
            Some((a, b)) if (b - a).abs() > 0.05 => (a.min(b), a.max(b), true),
            _ => (0.0, self.data.duration_sec.max(0.1), false),
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.cancel_export_job();
        let data = Arc::clone(&self.data);
        let solo_track = self.solo_track;
        let cancelled = Arc::new(AtomicBool::new(false));
        let job_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::from_secs(60), move || {
            let started = Instant::now();
            let artifact = write_midi_export(
                &data,
                s,
                e,
                is_slice,
                solo_track,
                timestamp,
                export_dir,
                Arc::clone(&job_cancelled),
            )?;
            handoff(artifact, &job_cancelled, started)
        });
        self.export_job = Some(MidiExportJob {
            ticket,
            cancelled,
            receiver,
        });
        self.status_toast.set("Preparing MIDI export…", now);
        true
    }

    fn cancel_export_job(&mut self) {
        if let Some(job) = self.export_job.take() {
            job.cancelled.store(true, Ordering::Release);
            job.ticket.cancel();
        }
    }

    fn poll_export(&mut self, now: f64, ctx: &egui::Context) {
        let Some(job) = self.export_job.as_ref() else {
            return;
        };
        match job.receiver.try_recv() {
            Ok(Ok(Ok(mut artifact))) => {
                let desc = if artifact.is_slice {
                    format!(
                        "Copied {:.1}s MIDI slice ({} notes{}) to Clipboard & Left Hand!",
                        artifact.duration_sec, artifact.total_notes, artifact.solo_info
                    )
                } else {
                    format!(
                        "Copied full MIDI ({} notes{}) to Clipboard & Left Hand!",
                        artifact.total_notes, artifact.solo_info
                    )
                };
                match artifact.delivery {
                    MidiExportDelivery::PushedToWorkbench => {}
                    MidiExportDelivery::ClipboardFallback => {
                        tracing::debug!("MIDI export delivered through system clipboard fallback");
                    }
                }
                artifact.retain();
                self.status_toast.set(desc, now);
                self.export_job = None;
            }
            Ok(Ok(Err(error))) => {
                if error == "MIDI export cancelled" {
                    self.export_job = None;
                    return;
                }
                self.status_toast.set(error, now);
                self.export_job = None;
            }
            Ok(Err(JobError::Cancelled)) => self.export_job = None,
            Ok(Err(error)) => {
                self.status_toast
                    .set(format!("Failed to export MIDI: {error}"), now);
                self.export_job = None;
            }
            Err(TryRecvError::Disconnected) => {
                self.status_toast
                    .set("MIDI export worker disconnected", now);
                self.export_job = None;
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
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

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
        close: &mut bool,
    ) {
        self.render_ui_with_copy_dispatch(ui, palette, close, |state, now| {
            state.copy_selection(now);
        });
    }

    fn render_ui_with_copy_dispatch(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
        close: &mut bool,
        mut copy_dispatch: impl FnMut(&mut Self, f64),
    ) {
        let now = ui.input(|input| input.time);
        self.poll_export(now, ui.ctx());
        self.update_playback();
        if self.is_playing {
            ui.ctx().request_repaint();
        } else if self.is_synthesizing {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
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
            match midi_space_action(self.is_playing) {
                MidiSpaceAction::Pause => self.pause(),
                MidiSpaceAction::Play => self.play_selection(),
            }
        }

        // Handle copy shortcut (Ctrl+C / Cmd+C / egui::Event::Copy)
        let copy_requested = ui.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || ((i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::C))
        });
        if copy_requested {
            copy_dispatch(self, now);
        }

        // Handle + / - for keyboard zoom
        if ui.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
            self.zoom(0.8, self.playhead_sec);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
            self.zoom(1.25, self.playhead_sec);
        }

        let bg = crate::hex_to_c32(&palette.colors.canvas);
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);
        let _f_med = crate::hex_to_c32(&palette.colors.text_secondary);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let playhead_color = crate::hex_to_c32(&palette.harmonies.complementary);
        let success_color = crate::hex_to_c32(&palette.diff_add);
        let pause_color = crate::hex_to_c32(&palette.harmonies.warm_attention);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
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

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 2: Transport & Zoom Bar
                egui::Frame::none()
                    .fill(b_low)
                    .stroke(Stroke::new(1.0_f32, b_med))
                    .inner_margin(egui::Margin::symmetric(
                        crate::ui_ext::PANEL_INSET,
                        crate::ui_ext::CONTROL_GROUP_INSET_Y,
                    ))
                    .show(ui, |ui| {
                        // Keep every transport and zoom action visible on compact popup widths.
                        // The wrapped layout preserves the one-click controls while allowing the
                        // rightmost zoom group to continue on the next row when needed.
                        ui.horizontal_wrapped(|ui| {
                            // Transport controls
                            if is_playing {
                                let pause_btn = crate::ui_ext::action_button_icon(Some(Icon::Pause), "Pause [Space]", pause_color, palette);
                                if ui.add(pause_btn).clicked() {
                                    pause_clicked = true;
                                }
                            } else {
                                let play_label = if self.is_synthesizing {
                                    "⏳ Synthesizing…"
                                } else {
                                    "Play [Space]"
                                };
                                let play_btn = crate::ui_ext::action_button_icon(Some(Icon::Play), play_label, success_color, palette);
                                if ui.add(play_btn).clicked() {
                                    play_clicked = true;
                                }
                            }

                            let stop_btn = crate::ui_ext::action_button_icon(Some(Icon::Stop), "Stop", b_med, palette)
                                .min_size(egui::Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT))
                                .stroke(Stroke::new(1.0_f32, b_low));
                            if ui.add(stop_btn).clicked() {
                                stop_clicked = true;
                            }

                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let time_text = format!("{:.1}s / {:.1}s", playhead_sec, duration_sec);
                            ui.label(
                                egui::RichText::new(time_text)
                                    .monospace()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(accent),
                            );

                            ui.add_space(crate::ui_ext::SPACE_SM);

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
                                .min_size(egui::Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                            if ui.add(copy_btn).on_hover_text("Export selected region (or full score) as .mid to Left Hand & clipboard").clicked() {
                                copy_clicked = true;
                            }

                            if let Some((s, e)) = self.selected_region {
                                if (e - s).abs() > 0.05 {
                                    ui.add_space(crate::ui_ext::SPACE_XS);
                                    let min_s = s.min(e);
                                    let max_s = s.max(e);
                                    ui.label(
                                        egui::RichText::new(format!("Sel: {:.2}s - {:.2}s", min_s, max_s))
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(accent),
                                    );
                                    let x_btn = egui::Button::new(egui::RichText::new("✕").size(crate::ui_ext::BODY_FONT_SIZE))
                                        .min_size(egui::Vec2::new(crate::ui_ext::CONTROL_HEIGHT, crate::ui_ext::CONTROL_HEIGHT));
                                    if ui.add(x_btn).on_hover_text("Clear selection [Esc]").clicked() {
                                        clear_sel_clicked = true;
                                    }
                                }
                            }

                            if let Some(st) = self.solo_track {
                                ui.add_space(crate::ui_ext::SPACE_XS);
                                let st_name = self.data.track_names.get(st).cloned().unwrap_or_else(|| format!("Track {}", st + 1));
                                ui.label(
                                    egui::RichText::new(format!("Solo: {st_name}"))
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(pause_color),
                                );
                                let unsolo_btn = egui::Button::new(egui::RichText::new("Unsolo").size(crate::ui_ext::BODY_FONT_SIZE))
                                    .min_size(egui::Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
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
                    copy_dispatch(self, now);
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
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Tracks:")
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .strong()
                                .color(f_low),
                        );
                        if self.solo_track.is_some() {
                            let all_btn = egui::Button::new(egui::RichText::new("All Tracks").size(crate::ui_ext::BODY_FONT_SIZE).strong())
                                .fill(b_med)
                                .min_size(egui::Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                            if ui.add(all_btn).on_hover_text("Clear solo and show/copy all tracks").clicked() {
                                clear_solo_clicked = true;
                            }
                            ui.add_space(crate::ui_ext::SPACE_XS);
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
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(track_label_col),
                                )
                                .sense(egui::Sense::click()),
                            );

                            let solo_btn = egui::Button::new(
                                egui::RichText::new(if is_solo { "SOLO" } else { "Solo" })
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(solo_text_col),
                            )
                            .fill(solo_fill)
                            .min_size(egui::Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

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

                            ui.add_space(crate::ui_ext::SPACE_SM);
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

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                ui.separator();
                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

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
                let pitch_font = egui::FontId::monospace(crate::ui_ext::MONO_FONT_SIZE);
                let pitch_label_height = painter
                    .layout_no_wrap("C4".to_owned(), pitch_font.clone(), f_high)
                    .size()
                    .y;

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
                    let key_text_color = crate::ui_ext::contrast_text_c32_on(
                        key_color,
                        b_low,
                        Some(key_text_color),
                        palette,
                    );
                    painter.rect_filled(key_rect, Rounding::same(2.0), key_color);
                    painter.rect_stroke(key_rect, Rounding::same(2.0), Stroke::new(0.5_f32, b_low));

                    let note_name = format!("{}{}", NOTE_NAMES[(k_int % 12) as usize], (k_int as i32 / 12) - 1);
                    // Keep every pitch name when there is room for a full label
                    // in each lane. At dense vertical zoom, octave labels remain
                    // spaced twelve lanes apart and stay readable without colliding.
                    if should_draw_pitch_label(k_int, row_height, pitch_label_height) {
                        let label_size = painter
                            .layout_no_wrap(note_name.clone(), pitch_font.clone(), key_text_color)
                            .size();
                        let label_y = clamp_label_center(
                            key_rect.center().y,
                            label_size.y,
                            canvas_rect.top(),
                            canvas_rect.bottom(),
                        );
                        if label_size.x <= key_rect.width() - 8.0
                            && let Some(label_y) = label_y
                        {
                            painter.text(
                                Pos2::new(key_rect.min.x + 4.0, label_y),
                                egui::Align2::LEFT_CENTER,
                                note_name,
                                pitch_font.clone(),
                                key_text_color,
                            );
                        }
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

                let time_font = egui::FontId::monospace(crate::ui_ext::MONO_FONT_SIZE);
                let mut previous_label_right = f32::NEG_INFINITY;
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
                        let label_width = painter
                            .layout_no_wrap(label_text.clone(), time_font.clone(), f_low)
                            .size()
                            .x;
                        let label_left = x + 2.0;
                        if time_label_fits(
                            label_left,
                            label_width,
                            previous_label_right,
                            grid_rect.max.x,
                        ) {
                            painter.text(
                                Pos2::new(label_left, grid_rect.min.y + 4.0),
                                egui::Align2::LEFT_TOP,
                                label_text,
                                time_font.clone(),
                                f_low,
                            );
                            previous_label_right = label_left + label_width;
                        }
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
mod tests {
    use super::*;
    use little_clip_core::config::WorkbenchAppearance;
    use little_clip_core::palette::AppPalette;

    #[test]
    fn independent_midi_views_share_score_and_keep_transport_separate() {
        let source = Arc::new(MidiData {
            title: "Shared score".to_owned(),
            track_count: 1,
            track_names: vec!["Track".to_owned()],
            total_notes: 1,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 60,
            notes: vec![MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 100,
                start_sec: 0.5,
                duration_sec: 1.0,
            }],
        });
        let mut first =
            MidiViewerState::from_shared(Arc::clone(&source), None, "First score view".to_owned());
        let second =
            MidiViewerState::from_shared(Arc::clone(&source), None, "Second score view".to_owned());

        assert!(Arc::ptr_eq(&first.data, &second.data));
        assert!(Arc::ptr_eq(&first.data, &source));
        first.is_playing = true;
        first.playhead_sec = 1.25;
        first.selected_region = Some((0.5, 1.5));
        first.solo_track = Some(0);

        assert!(!second.is_playing);
        assert_eq!(second.playhead_sec, 0.0);
        assert_eq!(second.selected_region, None);
        assert_eq!(second.solo_track, None);
    }

    #[test]
    fn midi_space_runs_the_render_handler_and_sets_selected_region_bounds() {
        let data = MidiData {
            title: "Space test".to_string(),
            track_count: 1,
            track_names: vec!["Track".to_string()],
            total_notes: 0,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 60,
            notes: Vec::new(),
        };
        let mut state = MidiViewerState::from_data(data, None, "Space test".to_string());
        // This empty cache is too short for the selected-region offset, so
        // play_selection updates transport bounds and play returns before
        // attempting to decode audio or open an output device.
        state.wav_cache = Some(vec![0; 44]);
        state.selected_region = Some((1.25, 2.75));
        let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
        let mut close_requested = false;
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, &palette, &mut close_requested);
                });
            },
        );

        assert_eq!(state.playhead_sec, 1.25);
        assert_eq!(state.play_stop_time, Some(2.75));
        assert!(!state.is_playing);
        assert!(!close_requested);
    }

    #[test]
    fn midi_space_pauses_active_transport_without_player_or_sink() {
        let data = MidiData {
            title: "Space pause test".to_string(),
            track_count: 1,
            track_names: vec!["Track".to_string()],
            total_notes: 0,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 60,
            notes: Vec::new(),
        };
        let mut state = MidiViewerState::from_data(data, None, "Space pause test".to_string());
        state.is_playing = true;
        state.playback_start_instant = Some(Instant::now());
        state.player = None;
        state._sink = None;
        let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
        let mut close_requested = false;
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, &palette, &mut close_requested);
                });
            },
        );

        assert!(!state.is_playing);
        assert!(state.playback_start_instant.is_none());
        assert!(state.player.is_none());
        assert!(state._sink.is_none());
        assert!(!close_requested);
    }

    #[test]
    fn midi_escape_clears_selected_region_before_closing_viewer() {
        let data = MidiData {
            title: "Escape selection test".to_string(),
            track_count: 1,
            track_names: vec!["Track".to_string()],
            total_notes: 0,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 60,
            notes: Vec::new(),
        };
        let mut state = MidiViewerState::from_data(data, None, "Escape selection test".to_string());
        state.selected_region = Some((0.5, 2.5));
        state.play_stop_time = Some(2.5);
        // Model an active transport without audio resources so an accidental stop
        // transition is observable, while this regression cannot emit audio.
        state.is_playing = true;
        state.playback_start_instant = None;
        state.player = None;
        state._sink = None;

        let palette = AppPalette::for_appearance(WorkbenchAppearance::Dark);
        let mut close_requested = false;
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, &palette, &mut close_requested);
                });
            },
        );

        assert!(state.selected_region.is_none());
        assert!(state.play_stop_time.is_none());
        assert!(
            !close_requested,
            "first Escape clears the region, not viewer"
        );
        assert!(
            state.is_playing,
            "clearing selection must not stop transport"
        );
        assert!(state.player.is_none());
        assert!(state._sink.is_none());
    }

    #[test]
    fn piano_roll_pitch_labels_thin_out_before_lanes_get_too_short() {
        let label_height = crate::ui_ext::MONO_FONT_SIZE;
        assert!(should_draw_pitch_label(
            61,
            label_height + 2.0,
            label_height
        ));
        assert!(should_draw_pitch_label(60, 8.0, label_height));
        assert!(!should_draw_pitch_label(61, 8.0, label_height));
        assert!(should_draw_pitch_label(72, 8.0, label_height));
    }

    #[test]
    fn piano_roll_octave_labels_stay_inside_canvas_edges() {
        assert_eq!(clamp_label_center(2.0, 16.0, 0.0, 100.0), Some(8.0));
        assert_eq!(clamp_label_center(98.0, 16.0, 0.0, 100.0), Some(92.0));
        assert_eq!(clamp_label_center(5.0, 16.0, 0.0, 12.0), None);
    }

    #[test]
    fn piano_roll_time_labels_respect_measured_spacing_and_canvas_edge() {
        assert!(time_label_fits(20.0, 40.0, 0.0, 100.0));
        assert!(!time_label_fits(43.0, 30.0, 40.0, 100.0));
        assert!(!time_label_fits(70.0, 40.0, 0.0, 100.0));
    }

    #[test]
    fn piano_roll_key_labels_keep_contrast_on_black_and_white_keys() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            for (key_fill, preferred) in [
                (colors.panel, colors.text_muted),
                (colors.text_primary, colors.canvas),
            ] {
                let foreground = crate::ui_ext::contrast_text_c32_on(
                    key_fill,
                    colors.recessed,
                    Some(preferred),
                    &palette,
                );
                assert!(
                    little_clip_core::palette::contrast_ratio(
                        (key_fill.r(), key_fill.g(), key_fill.b()),
                        (foreground.r(), foreground.g(), foreground.b()),
                    ) >= 4.5,
                    "{appearance:?} piano key label lacks contrast"
                );
            }
        }
    }

    #[test]
    fn test_midi_view_track_colors() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let t0 = get_track_color(0, 0, 4, &palette);
            let t1 = get_track_color(1, 0, 4, &palette);
            let drums = get_track_color(2, 9, 4, &palette);
            assert_ne!(t0, t1, "{appearance:?}: tracks should be distinct");
            assert_ne!(
                t0, drums,
                "{appearance:?}: melodic and drum tracks should differ"
            );
        }
    }

    #[test]
    fn fixed_appearance_midi_notes_are_vibrant_and_distinct() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let note_c = palette.midi_note_color(0, 0, 4, 60, 100);
            let note_d = palette.midi_note_color(1, 0, 4, 62, 100);
            assert_ne!(note_c, note_d, "{appearance:?}: track notes should differ");
            let chroma = little_clip_core::palette::Oklab::from_rgb(note_c.0, note_c.1, note_c.2)
                .to_oklch()
                .c;
            assert!(
                chroma >= 0.15,
                "{appearance:?}: note chroma {chroma:.3} is muted"
            );
            let (r, g, b, _) = palette.rgba(&palette.colors.surface_recessed);
            assert!(
                little_clip_core::palette::contrast_ratio((r, g, b), note_c) >= 2.5,
                "{appearance:?}: MIDI note lacks contrast against display surface"
            );
        }
    }

    #[test]
    fn test_midi_view_single_track_pitch_differentiation() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let c4 = palette.midi_note_color(0, 0, 1, 60, 100);
            let g4 = palette.midi_note_color(0, 0, 1, 67, 100);
            assert_ne!(
                c4, g4,
                "{appearance:?}: C4 and G4 should have distinct hues"
            );
        }
    }

    #[test]
    fn midi_viewer_tracks_selection_and_solo_for_export() {
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
            data: Arc::new(midi),
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
            is_synthesizing: false,
            pending_play: false,
            synth_rx: None,
            synth_ticket: None,
            export_job: None,
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

        // Selection and solo state are captured by the asynchronous export request.
        state.selected_region = Some((1.0, 3.0));
        state.solo_track = Some(0);
        assert_eq!(state.selected_region, Some((1.0, 3.0)));
        assert_eq!(state.solo_track, Some(0));
    }

    #[test]
    fn midi_export_worker_writes_selected_solo_slice_with_original_timing_and_name() {
        let data = MidiData {
            title: "Test Score".to_string(),
            track_count: 2,
            track_names: vec!["Melody".to_string(), "Bass Line".to_string()],
            total_notes: 2,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 40,
            max_key: 72,
            notes: vec![
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 60,
                    velocity: 90,
                    start_sec: 0.75,
                    duration_sec: 1.0,
                },
                MidiNote {
                    track: 1,
                    channel: 1,
                    program: 32,
                    key: 40,
                    velocity: 95,
                    start_sec: 1.0,
                    duration_sec: 1.25,
                },
            ],
        };
        let test_dir = std::env::temp_dir().join(format!(
            "little-screen-clip-midi-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let artifact = write_midi_export(
            &data,
            0.5,
            2.0,
            true,
            Some(1),
            123,
            test_dir.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(artifact.total_notes, 1);
        assert_eq!(artifact.duration_sec, 1.5);
        assert!(
            artifact
                .path
                .ends_with("midi_slice_123_0.5s-2.0s_Bass_Line.mid")
        );
        assert_eq!(artifact.solo_info, " (solo 'Bass Line')");
        let bytes = std::fs::read(&artifact.path).unwrap();
        let parsed = MidiData::parse_bytes(&bytes, "round trip".to_string()).unwrap();
        assert_eq!(parsed.total_notes, 1);
        assert_eq!(
            parsed.notes[0].track, 0,
            "solo track is remapped to track zero"
        );
        assert!((parsed.notes[0].start_sec - 0.5).abs() < 0.01);
        drop(artifact);
        std::fs::remove_dir(test_dir).unwrap();
    }

    #[test]
    fn midi_full_score_copy_uses_injected_job_directory_and_handoff() {
        let data = MidiData {
            title: "Injected full score".to_string(),
            track_count: 1,
            track_names: vec!["Piano".to_string()],
            total_notes: 1,
            duration_sec: 2.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 60,
            notes: vec![MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 96,
                start_sec: 0.25,
                duration_sec: 0.5,
            }],
        };
        let output_dir = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = MidiViewerState::from_data(data, None, "Injected full score".to_string());
        let (request_tx, request_rx) = std::sync::mpsc::channel();

        assert!(state.copy_selection_on(
            1.0,
            &jobs,
            output_dir.path().to_owned(),
            move |mut artifact, _cancelled, _started| {
                let request = IpcRequest::PushFile {
                    path: artifact.path.clone(),
                    hand: HandSide::Left,
                };
                let bytes = std::fs::read(&artifact.path).map_err(|error| error.to_string())?;
                request_tx
                    .send((request, bytes))
                    .map_err(|error| error.to_string())?;
                artifact.retain();
                Ok(artifact)
            },
        ));

        let (request, bytes) = request_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("injected handoff callback should receive export");
        let IpcRequest::PushFile { path, hand } = request else {
            panic!("MIDI full-score handoff should request a file");
        };
        assert_eq!(hand, HandSide::Left);
        assert_eq!(path.parent(), Some(output_dir.path()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("midi_export_")
        );
        let parsed = MidiData::parse_bytes(&bytes, "injected round trip".to_string())
            .expect("full-score export should be parseable MIDI");
        assert_eq!(parsed.total_notes, 1);
        assert_eq!(parsed.notes[0].key, 60);
        assert!((parsed.notes[0].start_sec - 0.25).abs() < 0.01);
        assert!((parsed.notes[0].duration_sec - 0.5).abs() < 0.01);

        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.export_job.is_some() && Instant::now() < deadline {
            state.poll_export(1.1, &ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            state.export_job.is_none(),
            "injected export job did not resolve"
        );
        assert_eq!(
            state.status_toast.message(1.1),
            Some("Copied full MIDI (1 notes) to Clipboard & Left Hand!")
        );
    }

    #[test]
    fn rendered_full_score_copy_shortcut_exports_parseable_midi_to_left_hand() {
        let data = MidiData {
            title: "Rendered full score".to_string(),
            track_count: 1,
            track_names: vec!["Piano".to_string()],
            total_notes: 1,
            duration_sec: 2.0,
            bpm: 120.0,
            min_key: 60,
            max_key: 60,
            notes: vec![MidiNote {
                track: 0,
                channel: 0,
                program: 0,
                key: 60,
                velocity: 96,
                start_sec: 0.25,
                duration_sec: 0.5,
            }],
        };
        let expected_bytes = data.to_midi_bytes().expect("serialize fixture score");
        let mut state = MidiViewerState::from_data(data, None, "Rendered full score".to_string());
        let output_dir = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let (request_tx, request_rx) = std::sync::mpsc::channel();
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let mut close = false;
        let mut time = 0.0;
        let output_dir_path = output_dir.path().to_owned();
        let mut dispatch_count = 0;
        let mut dispatch = |state: &mut MidiViewerState, now| {
            dispatch_count += 1;
            let request_tx = request_tx.clone();
            state.copy_selection_on(
                now,
                &jobs,
                output_dir_path.clone(),
                move |mut artifact, _cancelled, _started| {
                    let request = IpcRequest::PushFile {
                        path: artifact.path.clone(),
                        hand: HandSide::Left,
                    };
                    let bytes = std::fs::read(&artifact.path).map_err(|error| error.to_string())?;
                    request_tx
                        .send((request, bytes))
                        .map_err(|error| error.to_string())?;
                    artifact.retain();
                    Ok(artifact)
                },
            );
        };
        let mut render = |state: &mut MidiViewerState, events: Vec<egui::Event>| {
            time += 0.1;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440.0, 900.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui_with_copy_dispatch(ui, &palette, &mut close, &mut dispatch);
                    });
                },
            );
            output
        };

        let initial = render(&mut state, Vec::new());
        assert!(
            initial.shapes.iter().any(|clipped| matches!(
                &clipped.shape,
                egui::Shape::Text(text) if text.galley.text() == "Copy MIDI [Ctrl+C]"
            )),
            "rendered full-score Copy MIDI button label"
        );
        render(&mut state, vec![egui::Event::Copy]);
        drop(render);
        assert_eq!(dispatch_count, 1, "rendered copy shortcut dispatch count");

        let (request, bytes) = request_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("rendered copy action should dispatch one MIDI export");
        let IpcRequest::PushFile { path, hand } = request else {
            panic!("rendered MIDI copy should request a file");
        };
        assert_eq!(hand, HandSide::Left);
        assert_eq!(path.parent(), Some(output_dir.path()));
        assert_eq!(bytes, expected_bytes);
        let parsed = MidiData::parse_bytes(&bytes, "rendered round trip".to_string())
            .expect("rendered copy payload should be parseable MIDI");
        assert_eq!(parsed.total_notes, 1);
        assert_eq!(parsed.notes[0].key, 60);

        assert!(!close);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.export_job.is_some() && Instant::now() < deadline {
            state.poll_export(time + 0.1, &ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            state.export_job.is_none(),
            "rendered MIDI export did not finish"
        );
        assert_eq!(
            state.status_toast.message(time + 0.1),
            Some("Copied full MIDI (1 notes) to Clipboard & Left Hand!")
        );
    }

    #[test]
    fn midi_export_worker_honors_cancellation_before_writing() {
        let data = MidiData {
            title: "Cancelled".to_string(),
            track_count: 0,
            track_names: Vec::new(),
            total_notes: 0,
            duration_sec: 1.0,
            bpm: 120.0,
            min_key: 0,
            max_key: 127,
            notes: Vec::new(),
        };
        let test_dir = std::env::temp_dir().join(format!(
            "little-screen-clip-midi-cancel-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cancelled = Arc::new(AtomicBool::new(true));
        let result = write_midi_export(
            &data,
            0.0,
            1.0,
            false,
            None,
            456,
            test_dir.clone(),
            cancelled,
        );
        assert_eq!(result.unwrap_err(), "MIDI export cancelled");
        assert!(!test_dir.exists());
    }

    #[test]
    fn midi_export_file_creation_retries_collision_without_overwriting_existing_file() {
        let test_dir = std::env::temp_dir().join(format!(
            "little-screen-clip-midi-collision-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&test_dir).unwrap();
        let original_path = test_dir.join("midi_slice_123_0.5s-2.0s_Bass.mid");
        std::fs::write(&original_path, b"original export").unwrap();

        let second_path = write_unique_midi_export(
            &test_dir,
            "midi_slice_123_0.5s-2.0s_Bass.mid",
            b"new export",
        )
        .unwrap();

        assert_eq!(
            second_path,
            test_dir.join("midi_slice_123_0.5s-2.0s_Bass_1.mid")
        );
        assert_eq!(std::fs::read(&original_path).unwrap(), b"original export");
        assert_eq!(std::fs::read(&second_path).unwrap(), b"new export");
        std::fs::remove_file(original_path).unwrap();
        std::fs::remove_file(second_path).unwrap();
        std::fs::remove_dir(test_dir).unwrap();
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
            data: Arc::new(midi),
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
            is_synthesizing: false,
            pending_play: false,
            synth_rx: None,
            synth_ticket: None,
            export_job: None,
        };
        let palette = little_clip_core::palette::AppPalette::default();
        let ctx = egui::Context::default();

        state.toggle_solo(0);
        assert_eq!(state.solo_track, Some(0));
        state.selected_region = Some((0.5, 1.5));

        for width in [760.0, 1000.0, 1760.0] {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(width, 600.0),
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

            let required_labels = [
                "Copy Solo Slice (Melody)",
                "Sel: 0.50s - 1.50s",
                "Solo: Melody",
                "Unsolo",
                "Tracks:",
            ];
            let visible_text = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(shape) => Some(shape),
                    _ => None,
                })
                .collect::<Vec<_>>();

            for required in required_labels {
                assert!(
                    visible_text.iter().any(|shape| {
                        shape.galley.job.text.contains(required) && !shape.galley.elided
                    }),
                    "{required:?} missing or elided at {width} points"
                );
            }

            for shape in visible_text {
                let bounds = shape.galley.rect.translate(shape.pos.to_vec2());
                assert!(
                    bounds.left() >= 0.0 && bounds.right() <= width + 0.5,
                    "MIDI label {:?} exceeds {width}-point host: {bounds:?}",
                    shape.galley.job.text
                );
            }
        }

        state.clear_solo();
        assert_eq!(state.solo_track, None);
    }
}
