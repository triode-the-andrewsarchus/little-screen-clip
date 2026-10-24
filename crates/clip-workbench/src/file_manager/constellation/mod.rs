//! Maps recursively indexed audio samples and assembles reference-based sample packs.
//!
//! [`indexer`] performs bounded acoustic analysis and persists snapshots in [`db`].
//! Folder loads and pack operations use core's bounded background executor; the UI
//! accepts folder results only for the current folder generation. Indexing may
//! finish after navigation and still update its own database without changing the
//! visible map. Dropping the state cancels its read-only index/hover work.
//!
//! [`sample_pack`] owns sixteen layered pads. PCM is prepared off the UI thread, shared
//! by playback sources, and limited to the first 15 seconds/two million values per
//! layer. Bank metadata and symlinks preserve source references and trim settings;
//! reference saves retain edits as metadata; portable exports render WAV layers.
//! Accepted save/export/delete jobs
//! have no timeout and keep application close deferred until their results arrive.
//!
//! [`generated_model`] contains a fixed heuristic ensemble. Its scores and naming
//! priors group similar samples; they are not a calibrated instrument classifier.

pub mod audio_probe;
mod background;
pub mod classifier;
pub mod db;
pub mod generated_model;
mod history;
pub mod indexer;
mod layer_source;
mod midi;
mod output;
mod pack_export;
pub mod pack_shelf_ui;
mod pad_drop;
pub(crate) mod pad_interaction;
pub mod sample_pack;
pub mod ui;

use db::{ConstellationDb, ConstellationSample};
use generated_model::SampleCategory;
use indexer::{IndexProgress, run_indexing};
use sample_pack::SamplePackBank;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

// Shared decoder limits must be checked before sizes are derived from audio headers.
pub(super) const MAX_SAMPLE_RATE: u32 = 384_000;
pub(super) const MAX_SAMPLE_CHANNELS: u16 = 8;

pub struct IndexingJob {
    pub folder: PathBuf,
    pub cancelled: Arc<AtomicBool>,
    pub receiver: mpsc::Receiver<IndexProgress>,
}

#[derive(Clone, Debug)]
pub struct HistoryTrailPoint {
    pub file_path: PathBuf,
    pub x: f32,
    pub y: f32,
    pub category: SampleCategory,
}

#[derive(Clone, Debug, Default)]
pub struct ConstellationHoverPreview {
    pub file_path: PathBuf,
    pub waveform: Vec<f32>,
    pub waveform_colors: Vec<[u8; 3]>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bit_depth: Option<u16>,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct HoverPreviewRequest {
    generation: u64,
    cancelled: Arc<AtomicBool>,
    pub file_path: PathBuf,
    pub duration: f32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bit_depth: Option<u16>,
}

/// Owns map snapshots, cooperative read workers, and the editable sample bank.
///
/// Call [`Self::tick`] even while the view is hidden so accepted mutations finish.
/// UI entry points only submit work; decoded PCM and SQLite reads stay on workers.
pub struct ConstellationState {
    pub samples: Vec<ConstellationSample>,
    pub current_indexed_folder: Option<PathBuf>,
    pub category_filter: Option<SampleCategory>,
    min_duration: f32,
    max_duration: f32,
    channel_filter: u16,
    pub search_query: String,
    pub hover_to_play: bool,
    pub selected_path: Option<PathBuf>,
    /// Samples auditioned since the most recent explicit map click.
    visited_samples: std::collections::HashSet<PathBuf>,
    pub last_hovered_path: Option<PathBuf>,
    pub history_trail: std::collections::VecDeque<HistoryTrailPoint>,
    /// Full canvas before docks obscure it; projection must not depend on dock visibility.
    map_viewport: Option<egui::Rect>,
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub active_job: Option<IndexingJob>,
    pub indexing_status: Option<String>,
    indexing_progress: Option<f32>,
    indexing_started: Option<Instant>,
    pub hover_previews: HashMap<PathBuf, Arc<ConstellationHoverPreview>>,
    pub hover_req_tx: Option<mpsc::SyncSender<HoverPreviewRequest>>,
    pub hover_res_rx: Option<mpsc::Receiver<ConstellationHoverPreview>>,
    pub pending_hover_path: Option<PathBuf>,
    hover_cancelled: Arc<AtomicBool>,
    pub pack_bank: SamplePackBank,
    history: history::BankHistory,
    pending_bank_action: Option<history::BankAction>,
    pending_saved_bank: Option<SamplePackBank>,
    pending_load_bank: Option<SamplePackBank>,
    guard_saving: bool,
    load_approved: bool,
    pub(crate) close_approved: bool,
    pub(crate) pad_drop_hover: Option<egui::Pos2>,
    pub(crate) pad_drop_targets: Vec<(egui::Rect, usize)>,
    pub(crate) native_drop_results: Vec<(u64, bool)>,
    drop_sequence: u64,
    pub pack_shelf_open: bool,
    pub slot_flash: [Option<Instant>; sample_pack::NUM_SLOTS],
    selected_slot: usize,
    choke_anchor: Option<(u64, usize)>,
    pub(crate) studio_request: Option<pad_interaction::StudioRequest>,
    pad_editor_open: bool,
    pad_keys: [char; sample_pack::NUM_SLOTS],
    midi: Option<midi::Controller>,
    playback: Arc<std::sync::Mutex<output::Rack>>,
    /// The browser owns the map stream/loader; internal rack stops must retire it too.
    pub(crate) stop_map_audition: bool,
    background: Vec<background::Pending>,
    folder_generation: u64,
    bank_generation: u64,
    browser_generation: u64,
    pub browser_preview: Option<(PathBuf, SamplePackBank)>,
    failed_audio: std::collections::HashSet<PathBuf>,
    pub focus_search: bool,
    pub browser_open: bool,
    pub browser_selected_idx: Option<usize>,
    pub browser_packs: Vec<sample_pack::SamplePackSummary>,
    pub browser_status: Option<(String, Instant)>,
}

impl Default for ConstellationState {
    fn default() -> Self {
        let pack_bank = SamplePackBank::default();
        let history = history::BankHistory::new(&pack_bank);
        Self {
            samples: Vec::new(),
            current_indexed_folder: None,
            category_filter: None,
            min_duration: 0.0,
            max_duration: 0.0,
            channel_filter: 0,
            search_query: String::new(),
            hover_to_play: true,
            selected_path: None,
            visited_samples: std::collections::HashSet::new(),
            last_hovered_path: None,
            history_trail: std::collections::VecDeque::new(),
            map_viewport: None,
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            active_job: None,
            indexing_status: None,
            indexing_progress: None,
            indexing_started: None,
            hover_previews: HashMap::new(),
            hover_req_tx: None,
            hover_res_rx: None,
            pending_hover_path: None,
            hover_cancelled: Arc::new(AtomicBool::new(false)),
            pack_bank,
            history,
            pending_bank_action: None,
            pending_saved_bank: None,
            pending_load_bank: None,
            guard_saving: false,
            load_approved: false,
            close_approved: false,
            pad_drop_hover: None,
            pad_drop_targets: Vec::new(),
            native_drop_results: Vec::new(),
            drop_sequence: 0,
            pack_shelf_open: true,
            slot_flash: [None; sample_pack::NUM_SLOTS],
            selected_slot: 0,
            choke_anchor: None,
            studio_request: None,
            pad_editor_open: true,
            pad_keys: [
                'z', 'x', 'c', 'v', 'a', 's', 'd', 'f', 'q', 'w', 'e', 'r', '1', '2', '3', '4',
            ],
            midi: None,
            playback: Default::default(),
            stop_map_audition: false,
            background: Vec::new(),
            folder_generation: 0,
            bank_generation: 0,
            browser_generation: 0,
            browser_preview: None,
            failed_audio: Default::default(),
            focus_search: false,
            browser_open: false,
            browser_selected_idx: None,
            browser_packs: Vec::new(),
            browser_status: None,
        }
    }
}

impl ConstellationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an audition and excludes its sample from subsequent neighbor steps.
    ///
    /// The fading trail keeps at most 64 waypoints and omits consecutive duplicates.
    /// The exploration set resets on a map click or a folder/index reload.
    pub fn record_played_sample(&mut self, sample: &ConstellationSample) {
        self.visited_samples.insert(sample.file_path.clone());
        if let Some(last) = self.history_trail.back() {
            if last.file_path == sample.file_path {
                return;
            }
        }
        self.history_trail.push_back(HistoryTrailPoint {
            file_path: sample.file_path.clone(),
            x: sample.x,
            y: sample.y,
            category: sample.category,
        });
        if self.history_trail.len() > 64 {
            self.history_trail.pop_front();
        }
    }

    /// Computes the adaptive count of recent sample waypoints to connect with a fading trajectory line.
    ///
    /// When tightly exploring a small localized clump at high zoom, returns a compact history (around 8-12)
    /// to avoid visual clutter. When zoomed out or making large sweeping traverses across distant clumps,
    /// returns a broader history (up to 45-50) so the user's journey across the constellation is clearly visible.
    pub fn compute_trail_display_count(&self) -> usize {
        let trail_len = self.history_trail.len();
        if trail_len <= 2 {
            return trail_len;
        }

        // 1. Zoom baseline: tightly bounded at high zoom, expansive when zoomed out
        let zoom_baseline = 10.0 + (28.0 / self.zoom.sqrt()).clamp(0.0, 32.0);

        // 2. Spatial distance factor based on recent consecutive steps
        let recent_points: Vec<&HistoryTrailPoint> =
            self.history_trail.iter().rev().take(15).collect();
        let mut total_dist = 0.0f32;
        let mut pair_count = 0;
        for window in recent_points.windows(2) {
            let dx = window[0].x - window[1].x;
            let dy = window[0].y - window[1].y;
            total_dist += (dx * dx + dy * dy).sqrt();
            pair_count += 1;
        }
        let avg_dist = if pair_count > 0 {
            total_dist / pair_count as f32
        } else {
            0.15
        };

        let dist_adjustment = ((avg_dist - 0.15) * 35.0).clamp(-4.0, 15.0);
        let target_count = (zoom_baseline + dist_adjustment).round() as usize;
        target_count.clamp(8, 50).min(trail_len)
    }

    /// Returns the path of the folder currently being indexed, if an indexing job is active.
    pub fn indexing_folder(&self) -> Option<&Path> {
        self.active_job.as_ref().map(|j| j.folder.as_path())
    }

    /// Checks if the specified folder is currently being indexed.
    pub fn is_indexing_folder(&self, folder: &Path) -> bool {
        self.indexing_folder() == Some(folder)
    }

    /// Reports whether the displayed folder has a nonempty loaded index snapshot.
    ///
    /// This is a cached menu hint and never opens SQLite on the UI thread.
    pub fn is_folder_already_indexed(&self, folder: &Path) -> bool {
        if self.current_indexed_folder.as_deref() == Some(folder) && !self.samples.is_empty() {
            return true;
        }
        false
    }

    /// Requests a background SQLite snapshot when the displayed folder changes.
    ///
    /// Clears view-specific selection, preview, and camera state immediately;
    /// generation checks prevent older completions from replacing the new view.
    pub fn check_and_load_folder(&mut self, folder: &Path) {
        if self.current_indexed_folder.as_deref() == Some(folder) {
            return;
        }

        self.current_indexed_folder = Some(folder.to_path_buf());
        self.selected_path = None;
        self.last_hovered_path = None;
        self.history_trail.clear();
        self.visited_samples.clear();

        self.samples.clear();
        self.hover_cancelled.store(true, Ordering::Relaxed);
        self.hover_previews.clear();
        self.pending_hover_path = None;
        self.pan = egui::Vec2::ZERO;
        self.zoom = 1.0;
        self.folder_generation = self.folder_generation.wrapping_add(1);
        let path = folder.to_path_buf();
        self.submit_background(
            background::Task::Folder(path.clone(), self.folder_generation),
            move || {
                ConstellationDb::new()?
                    .load_samples(&path)
                    .map(background::Output::Samples)
            },
        );
    }

    /// Installs validated logical character rows, ordered from the bottom of the rack.
    pub fn configure_pad_keys(&mut self, rows: &[String; 4]) {
        if let Ok(keys) = rows
            .iter()
            .flat_map(|row| row.chars().map(|key| key.to_ascii_lowercase()))
            .collect::<Vec<_>>()
            .try_into()
        {
            self.pad_keys = keys;
        }
    }

    /// Reports whether a printable character is reserved by the configured rack.
    pub fn uses_pad_key(&self, key: char) -> bool {
        self.pad_keys.contains(&key.to_ascii_lowercase())
    }

    /// Starts recursive indexing of the target folder in a background worker thread.
    pub fn start_indexing(&mut self, folder: &Path) {
        self.cancel_indexing();

        let folder_buf = folder.to_path_buf();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();

        let cancel_clone = Arc::clone(&cancelled);
        let folder_clone = folder_buf.clone();

        let worker = thread::Builder::new()
            .name("constellation-indexer".to_string())
            .spawn(move || {
                run_indexing(folder_clone, cancel_clone, sender);
            });
        if let Err(error) = worker {
            self.indexing_status = Some(format!("Cannot start indexer: {error}"));
            return;
        }

        self.active_job = Some(IndexingJob {
            folder: folder_buf,
            cancelled,
            receiver,
        });

        self.indexing_started = Some(Instant::now());
        self.indexing_progress = None;
        self.indexing_status = Some("Scanning directory tree for audio samples...".to_string());
    }

    /// Cancels any currently active indexing job.
    pub fn cancel_indexing(&mut self) {
        if let Some(job) = self.active_job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        self.indexing_status = None;
        self.indexing_progress = None;
        self.indexing_started = None;
    }

    /// Starts a single hover worker with one queued request and one queued result.
    ///
    /// Returns `None` and records a status message if thread creation fails.
    pub fn ensure_hover_worker(&mut self) -> Option<&mpsc::SyncSender<HoverPreviewRequest>> {
        if self.hover_req_tx.is_none() {
            let (req_tx, req_rx) = mpsc::sync_channel::<HoverPreviewRequest>(1);
            let (res_tx, res_rx) = mpsc::sync_channel::<ConstellationHoverPreview>(1);

            let worker = thread::Builder::new()
                .name("constellation-hover-preview".into())
                .spawn(move || {
                    while let Ok(req) = req_rx.recv() {
                        // Coalesce / drain stale requests so we only compute the newest hovered sample
                        let mut latest = req;
                        while let Ok(newer) = req_rx.try_recv() {
                            latest = newer;
                        }

                        if latest.cancelled.load(Ordering::Relaxed) {
                            continue;
                        }
                        let path = &latest.file_path;
                        let source = crate::file_source::FileSource::capture(path).ok();
                        let info = audio_probe::read_audio_format_info(path);
                        let sample_rate = info.sample_rate.or(latest.sample_rate);
                        let channels = info.channels.or(latest.channels);
                        let bit_depth = info.bit_depth.or(latest.bit_depth);

                        let (waveform, colors) = super::music::preview::waveform(
                            path,
                            latest.duration,
                            64,
                            Some(15.0),
                            &latest.cancelled,
                        )
                        .unwrap_or_default();

                        if latest.cancelled.load(Ordering::Relaxed) {
                            continue;
                        }
                        if source.is_none_or(|source| source.check().is_err()) {
                            let _ = res_tx.send(ConstellationHoverPreview {
                                file_path: latest.file_path,
                                generation: latest.generation,
                                ..Default::default()
                            });
                            continue;
                        }
                        let preview = ConstellationHoverPreview {
                            generation: latest.generation,
                            file_path: latest.file_path,
                            waveform,
                            waveform_colors: colors,
                            sample_rate,
                            channels,
                            bit_depth,
                        };
                        let _ = res_tx.send(preview);
                    }
                });
            if let Err(error) = worker {
                self.browser_status = Some((
                    format!("Error starting preview worker: {error}"),
                    Instant::now(),
                ));
                return None;
            }

            self.hover_req_tx = Some(req_tx);
            self.hover_res_rx = Some(res_rx);
        }
        self.hover_req_tx.as_ref()
    }

    /// Drains any completed hover previews from the background worker.
    pub fn poll_hover_previews(&mut self) {
        if let Some(ref rx) = self.hover_res_rx {
            while let Ok(preview) = rx.try_recv() {
                if preview.generation != self.folder_generation {
                    continue;
                }
                if self.pending_hover_path.as_ref() == Some(&preview.file_path) {
                    self.pending_hover_path = None;
                }
                if self.hover_previews.len() > 256 {
                    self.hover_previews.clear();
                }
                self.hover_previews
                    .insert(preview.file_path.clone(), Arc::new(preview));
            }
        }
    }

    /// Stops slot voices and retires hover work through the shared preview lifecycle.
    ///
    /// Requests retirement of the browser-owned map stream and its pending load.
    /// The browser consumes that request before accepting loader results and after
    /// rack UI actions. Retains the hovered path so Escape cannot retrigger it.
    pub fn stop_playback(&mut self) {
        self.stop_map_audition = true;
        if let Some(midi) = &self.midi {
            midi.suspend();
        }
        self.playback
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stop();
        self.hover_cancelled.store(true, Ordering::Relaxed);
        self.pending_hover_path = None;
    }

    /// Routes a prepared map stream through the rack without truncating long samples.
    pub(crate) fn play_map_source(
        &mut self,
        source: Box<dyn rodio::Source<Item = f32> + Send>,
    ) -> Option<rodio::Player> {
        let mut rack = self.playback.lock().unwrap();
        rack.sync(&self.pack_bank);
        if rack.output.is_none() {
            rack.output = output::PadOutput::new(self.pack_bank.compressor_bypass);
        }
        let output = rack.output.as_ref()?;
        output.set_dynamics(&self.pack_bank);
        let player = output.play_source(source);
        rack.map_playing = true;
        Some(player)
    }
    pub(crate) fn set_map_playing(&mut self, playing: bool) {
        self.playback.lock().unwrap().map_playing = playing;
    }
    fn audition_layers(&mut self, samples: &[sample_pack::SlotSample]) {
        let mut rack = self.playback.lock().unwrap();
        rack.sync(&self.pack_bank);
        rack.audition(samples);
    }

    /// Restarts all ready layers in one slot without interrupting other slots.
    ///
    /// Stops the previous voices even if a replacement layer is still loading.
    pub fn trigger_slot(&mut self, slot_idx: usize) {
        self.trigger_slot_velocity(slot_idx, 127);
    }

    /// Plays a one-shot pad at MIDI velocity, applying the bank's velocity override.
    ///
    /// Retriggers replace that pad's voices; nonzero choke groups also retire peers.
    /// Velocity zero is a note-off and never chokes or restarts a pad.
    fn trigger_slot_velocity(&mut self, slot_idx: usize, velocity: u8) {
        let mut playback = self
            .playback
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        playback.sync(&self.pack_bank);
        if playback.trigger(slot_idx, velocity) && velocity != 0 {
            self.slot_flash[slot_idx] = Some(Instant::now());
        }
    }

    /// Adds a sample from the constellation map into a specific slot as a layer.
    pub fn add_sample_to_slot(&mut self, slot_idx: usize, sample: &ConstellationSample) {
        self.checkpoint_bank(false);
        if slot_idx >= self.pack_bank.slots.len() {
            return;
        }
        self.selected_slot = slot_idx;
        let slot_sample = sample_pack::SlotSample::new(
            sample.file_path.clone(),
            sample.filename.clone(),
            Some(sample.category),
            sample.duration_seconds,
        );
        if self.pack_bank.slots[slot_idx].samples.len() < 8 {
            self.failed_audio.remove(&sample.file_path);
            self.pack_bank.slots[slot_idx].samples.push(slot_sample);
        } else {
            self.browser_status = Some((
                "A slot supports at most eight layers".into(),
                Instant::now(),
            ));
        }
    }

    /// Clears all assigned samples from a specific slot.
    pub fn clear_slot(&mut self, slot_idx: usize) {
        self.checkpoint_bank(false);
        if slot_idx < self.pack_bank.slots.len() {
            self.pack_bank.slots[slot_idx].samples.clear();
            for player in self.playback.lock().unwrap().voices[slot_idx].drain(..) {
                player.stop();
            }
        }
    }

    /// Scans `<root>/little-screen-clip-sample-packs` for all saved packs.
    pub fn refresh_browser_packs(&mut self, current_folder: &Path) {
        let root = self
            .current_indexed_folder
            .as_deref()
            .unwrap_or(current_folder)
            .to_path_buf();
        self.browser_selected_idx = None;
        self.browser_preview = None;
        self.browser_generation = self.browser_generation.wrapping_add(1);
        self.submit_background(
            background::Task::List(root.clone(), self.browser_generation),
            move || {
                Ok(background::Output::Packs(sample_pack::list_saved_packs(
                    &root,
                )))
            },
        );
    }

    /// Accepts worker results, schedules bounded PCM loads, and retires ended voices.
    ///
    /// Must run while the view is hidden to preserve accepted mutation completions.
    pub fn tick(&mut self) {
        self.poll_background(little_clip_core::jobs::BackgroundJobs::global());
        self.queue_audio_loads();
        self.poll_hover_previews();
        {
            let mut playback = self
                .playback
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            playback.sync(&self.pack_bank);
            playback.retire();
        }
        let mut job_done = false;

        if let Some(job) = &self.active_job {
            loop {
                let msg = match job.receiver.try_recv() {
                    Ok(msg) => msg,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.indexing_status = Some("Indexer stopped before completion".into());
                        job_done = true;
                        break;
                    }
                };
                match msg {
                    IndexProgress::Scanning { files_found } => {
                        self.indexing_status =
                            Some(format!("Discovered {} audio files...", files_found));
                    }
                    IndexProgress::Indexing {
                        processed,
                        total,
                        current_file,
                        reused,
                    } => {
                        self.indexing_progress = Some(processed as f32 / total.max(1) as f32);
                        let pct = (processed as f32 / total.max(1) as f32 * 100.0) as u32;
                        self.indexing_status = Some(format!(
                            "Indexing [{}/{} - {}%] · {} reused · {} new: {}",
                            processed,
                            total,
                            pct,
                            reused,
                            processed.saturating_sub(reused),
                            current_file
                        ));
                    }
                    IndexProgress::Complete {
                        samples,
                        reused,
                        analyzed,
                    } => {
                        if self.current_indexed_folder.as_ref() == Some(&job.folder) {
                            self.folder_generation = self.folder_generation.wrapping_add(1);
                            self.samples = samples;
                            self.selected_path = None;
                            self.last_hovered_path = None;
                            self.history_trail.clear();
                            self.visited_samples.clear();
                            self.hover_previews.clear();
                            self.pending_hover_path = None;
                        }
                        self.indexing_status = Some(format!(
                            "Index complete · {reused} reused · {analyzed} analyzed"
                        ));
                        job_done = true;
                        break;
                    }
                    IndexProgress::Error(err) => {
                        self.indexing_status = Some(format!("Error: {}", err));
                        job_done = true;
                        break;
                    }
                }
            }
        }

        if job_done {
            self.active_job = None;
        }
    }
}

impl Drop for ConstellationState {
    fn drop(&mut self) {
        self.cancel_indexing();
        self.stop_playback();
        for job in &self.background {
            job.cancel_read();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrigger_stops_only_its_slot_and_navigation_stops_every_voice() {
        let (mixer, mut output) =
            rodio::mixer::mixer(1.try_into().unwrap(), 44100.try_into().unwrap());
        let mut state = ConstellationState::new();
        for (slot, amplitude) in [(0, 0.1), (0, 0.15), (1, 0.25)] {
            let player = rodio::Player::connect_new(&mixer);
            player.append(rodio::buffer::SamplesBuffer::new(
                1.try_into().unwrap(),
                44100.try_into().unwrap(),
                vec![amplitude; 44100],
            ));
            state.playback.lock().unwrap().voices[slot].push(player);
        }
        // Player queues start with a short silent span before their first source.
        for _ in 0..1000 {
            output.next();
        }
        assert!((output.next().unwrap() - 0.5).abs() < 0.0001);
        // An empty/cold slot must retire all its old layers as well.
        state.trigger_slot(0);
        assert!(state.playback.lock().unwrap().voices[0].is_empty());
        assert_eq!(state.playback.lock().unwrap().voices[1].len(), 1);
        assert!(state.busy());
        // Rodio applies stop controls at bounded periodic source checkpoints.
        for _ in 0..1000 {
            output.next();
        }
        assert!((output.next().unwrap() - 0.25).abs() < 0.0001);
        state.trigger_slot(0);
        assert_eq!(state.playback.lock().unwrap().voices[1].len(), 1);
        state.stop_playback();
        assert!(
            state
                .playback
                .lock()
                .unwrap()
                .voices
                .iter()
                .all(Vec::is_empty)
        );
        assert!(!state.busy());
        for _ in 0..1000 {
            output.next();
        }
        assert!(output.next().is_none());
    }

    #[test]
    fn velocity_override_and_retriggers_use_one_voice_per_pad() {
        let (player, mut output) = rodio::Player::new();
        let mut state = ConstellationState::new();
        state.playback.lock().unwrap().output = Some(output::PadOutput::with_player(player, true));
        let mut sample =
            sample_pack::SlotSample::new("kick.wav".into(), "kick.wav".into(), None, 1.0);
        let pcm = Arc::new(vec![0.5; 48000]);
        sample.cached_audio = Some(Arc::new(sample_pack::CachedAudio {
            channels: 1,
            sample_rate: 48000,
            samples: pcm.clone(),
            mono_samples: pcm,
            waveform_peaks: vec![],
            waveform_colors: vec![],
        }));
        state.pack_bank.slots[0].samples.push(sample);
        let full = 0.5 * 10.0_f32.powf(-2.0 / 20.0);
        for (velocity, override_velocity, expected) in [
            (64, false, full * 64.0 / 127.0),
            (127, false, full),
            (1, true, full),
        ] {
            state.pack_bank.override_velocity = override_velocity;
            state.trigger_slot_velocity(0, velocity);
            assert_eq!(state.playback.lock().unwrap().voices[0].len(), 1);
            for _ in 0..4000 {
                output.next();
            }
            assert!((output.next().unwrap() - expected).abs() < 0.00001);
        }
    }

    #[test]
    fn mute_and_solo_change_active_voice_gates_without_retriggering() {
        let (mixer, _audio) = rodio::mixer::mixer(2.try_into().unwrap(), 48000.try_into().unwrap());
        let mut state = ConstellationState::default();
        state.playback.lock().unwrap().voices[0].push(rodio::Player::connect_new(&mixer));
        state.pack_bank.slots[0].mute = true;
        state.playback.lock().unwrap().sync(&state.pack_bank);
        assert_eq!(state.playback.lock().unwrap().voices[0][0].volume(), 0.0);
        state.pack_bank.slots[0].mute = false;
        state.pack_bank.slots[1].solo = true;
        state.playback.lock().unwrap().sync(&state.pack_bank);
        assert_eq!(state.playback.lock().unwrap().voices[0][0].volume(), 0.0);
        state.pack_bank.slots[0].solo = true;
        state.playback.lock().unwrap().sync(&state.pack_bank);
        assert_eq!(state.playback.lock().unwrap().voices[0][0].volume(), 1.0);
        assert_eq!(state.playback.lock().unwrap().voices[0].len(), 1);
    }

    #[test]
    fn choke_groups_retire_peers_but_not_other_groups_and_ignore_note_off() {
        let (mixer, _output) =
            rodio::mixer::mixer(1.try_into().unwrap(), 44100.try_into().unwrap());
        let mut state = ConstellationState::new();
        for index in 0..4 {
            state.pack_bank.slots[index].choke_group = if index < 3 { 1 } else { 2 };
            state.playback.lock().unwrap().voices[index].push(rodio::Player::connect_new(&mixer));
        }
        state.trigger_slot_velocity(0, 0);
        assert!(
            state.playback.lock().unwrap().voices[..4]
                .iter()
                .all(|voices| voices.len() == 1)
        );
        state.trigger_slot_velocity(0, 64);
        assert!(
            state.playback.lock().unwrap().voices[..3]
                .iter()
                .all(Vec::is_empty)
        );
        assert_eq!(state.playback.lock().unwrap().voices[3].len(), 1);
        state.trigger_slot_velocity(15, 127);
        assert_eq!(state.playback.lock().unwrap().voices[3].len(), 1);
    }

    #[test]
    fn indexing_completion_does_not_replace_another_folder() {
        let mut state = ConstellationState::default();
        state.current_indexed_folder = Some(PathBuf::from("/current"));
        state.selected_path = Some(PathBuf::from("/current/selected.wav"));
        let (sender, receiver) = mpsc::channel();
        state.active_job = Some(IndexingJob {
            folder: PathBuf::from("/old"),
            cancelled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        sender
            .send(IndexProgress::Complete {
                samples: vec![],
                reused: 0,
                analyzed: 0,
            })
            .unwrap();
        state.tick();
        assert_eq!(
            state.selected_path,
            Some(PathBuf::from("/current/selected.wav"))
        );
        assert!(state.active_job.is_none());
    }

    #[test]
    fn disconnected_indexer_reports_failure() {
        let mut state = ConstellationState::default();
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        state.active_job = Some(IndexingJob {
            folder: PathBuf::from("/old"),
            cancelled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        state.tick();
        assert!(state.active_job.is_none());
        assert!(state.indexing_status.as_ref().unwrap().contains("stopped"));
    }

    #[test]
    fn test_history_trail_recording_and_deduplication() {
        let mut state = ConstellationState::new();

        let s1 = ConstellationSample {
            file_path: PathBuf::from("kick_01.wav"),
            folder_path: PathBuf::from("/samples"),
            filename: "kick_01.wav".to_string(),
            category: SampleCategory::Kick,
            confidence: 0.9,
            duration_seconds: 0.3,
            pitch_hz: 55.0,
            pitch_clarity: 0.8,
            brightness: 250.0,
            x: -0.8,
            y: -0.15,
            file_size: 1000,
            mtime: 0,
            sample_rate: 44100,
            channels: 2,
            bit_depth: 16,
            source_revision: None,
        };

        let s2 = ConstellationSample {
            file_path: PathBuf::from("snare_01.wav"),
            folder_path: PathBuf::from("/samples"),
            filename: "snare_01.wav".to_string(),
            category: SampleCategory::Snare,
            confidence: 0.95,
            duration_seconds: 0.25,
            pitch_hz: 200.0,
            pitch_clarity: 0.6,
            brightness: 4500.0,
            x: -0.2,
            y: -0.08,
            file_size: 1200,
            mtime: 0,
            sample_rate: 44100,
            channels: 2,
            bit_depth: 16,
            source_revision: None,
        };

        // Record first sample
        state.record_played_sample(&s1);
        assert_eq!(state.history_trail.len(), 1);

        // Record consecutive duplicate: should NOT increase trail length
        state.record_played_sample(&s1);
        assert_eq!(state.history_trail.len(), 1);

        // Record different sample
        state.record_played_sample(&s2);
        assert_eq!(state.history_trail.len(), 2);

        // Record s1 again (non-consecutive): should be added
        state.record_played_sample(&s1);
        assert_eq!(state.history_trail.len(), 3);
    }

    #[test]
    fn test_adaptive_trail_display_count() {
        let mut state = ConstellationState::new();

        // Populate 40 trail points
        for i in 0..40 {
            let sample = ConstellationSample {
                file_path: PathBuf::from(format!("sample_{:03}.wav", i)),
                folder_path: PathBuf::from("/samples"),
                filename: format!("sample_{:03}.wav", i),
                category: SampleCategory::Kick,
                confidence: 0.9,
                duration_seconds: 0.2,
                pitch_hz: 100.0,
                pitch_clarity: 0.5,
                brightness: 1000.0,
                // Micro steps within tight cluster
                x: -0.8 + (i as f32 * 0.005),
                y: -0.15 + (i as f32 * 0.005),
                file_size: 1000,
                mtime: 0,
                sample_rate: 44100,
                channels: 2,
                bit_depth: 16,
                source_revision: None,
            };
            state.record_played_sample(&sample);
        }

        // At high zoom with tight clustering, count should be small (~10-14)
        state.zoom = 20.0;
        let tight_count = state.compute_trail_display_count();
        assert!(
            (8..=16).contains(&tight_count),
            "Expected tight count around 10-14 at high zoom, got {}",
            tight_count
        );

        // Clear and populate with wide sweeping jumps across clumps
        state.history_trail.clear();
        for i in 0..40 {
            let angle = i as f32 * 0.25;
            let sample = ConstellationSample {
                file_path: PathBuf::from(format!("orbit_{:03}.wav", i)),
                folder_path: PathBuf::from("/samples"),
                filename: format!("orbit_{:03}.wav", i),
                category: SampleCategory::Kick,
                confidence: 0.9,
                duration_seconds: 0.2,
                pitch_hz: 100.0,
                pitch_clarity: 0.5,
                brightness: 1000.0,
                x: angle.cos() * 0.9,
                y: angle.sin() * 0.7,
                file_size: 1000,
                mtime: 0,
                sample_rate: 44100,
                channels: 2,
                bit_depth: 16,
                source_revision: None,
            };
            state.record_played_sample(&sample);
        }

        // At low zoom with sweeping steps, count should be large (~35-45)
        state.zoom = 1.0;
        let wide_count = state.compute_trail_display_count();
        assert!(
            wide_count >= 35,
            "Expected wide sweeping trail >= 35, got {}",
            wide_count
        );
        assert!(wide_count > tight_count);
    }
}
