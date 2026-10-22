//! 2D sample constellation view and classification system inspired by 2D audio sample mapping tools.

pub mod audio_probe;
pub mod classifier;
pub mod db;
pub mod generated_model;
pub mod indexer;
pub mod pack_shelf_ui;
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
}

#[derive(Clone, Debug)]
pub struct HoverPreviewRequest {
    pub file_path: PathBuf,
    pub duration: f32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bit_depth: Option<u16>,
}

pub struct ConstellationState {
    pub samples: Vec<ConstellationSample>,
    pub current_indexed_folder: Option<PathBuf>,
    pub category_filter: Option<SampleCategory>,
    pub search_query: String,
    pub hover_to_play: bool,
    pub selected_path: Option<PathBuf>,
    pub last_hovered_path: Option<PathBuf>,
    pub history_trail: std::collections::VecDeque<HistoryTrailPoint>,
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub active_job: Option<IndexingJob>,
    pub indexing_status: Option<String>,
    pub hover_previews: HashMap<PathBuf, Arc<ConstellationHoverPreview>>,
    pub hover_req_tx: Option<mpsc::Sender<HoverPreviewRequest>>,
    pub hover_res_rx: Option<mpsc::Receiver<ConstellationHoverPreview>>,
    pub pending_hover_path: Option<PathBuf>,
    pub pack_bank: SamplePackBank,
    pub pack_shelf_open: bool,
    pub slot_flash: [Option<Instant>; 10],
    pub active_players: Vec<rodio::Player>,
    pub hovered_slot_idx: Option<usize>,
    pub pinned_slot_idx: Option<usize>,
    pub browser_open: bool,
    pub browser_selected_idx: Option<usize>,
    pub browser_packs: Vec<sample_pack::SamplePackSummary>,
    pub browser_status: Option<(String, Instant)>,
}

impl Default for ConstellationState {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            current_indexed_folder: None,
            category_filter: None,
            search_query: String::new(),
            hover_to_play: true,
            selected_path: None,
            last_hovered_path: None,
            history_trail: std::collections::VecDeque::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            active_job: None,
            indexing_status: None,
            hover_previews: HashMap::new(),
            hover_req_tx: None,
            hover_res_rx: None,
            pending_hover_path: None,
            pack_bank: SamplePackBank::default(),
            pack_shelf_open: true,
            slot_flash: [None; 10],
            active_players: Vec::new(),
            hovered_slot_idx: None,
            pinned_slot_idx: None,
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

    /// Records a played sample into the auditory history trail, preventing consecutive duplicates.
    pub fn record_played_sample(&mut self, sample: &ConstellationSample) {
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

    /// Checks if a folder has already been indexed in SQLite.
    pub fn is_folder_already_indexed(&self, folder: &Path) -> bool {
        if self.current_indexed_folder.as_deref() == Some(folder) && !self.samples.is_empty() {
            return true;
        }
        if let Ok(db) = ConstellationDb::new() {
            if let Ok(Some(count)) = db.is_folder_indexed(folder) {
                return count > 0;
            }
        }
        false
    }

    /// Loads indexed samples from SQLite for the given folder if not already loaded.
    pub fn check_and_load_folder(&mut self, folder: &Path) {
        if self.current_indexed_folder.as_deref() == Some(folder) {
            return;
        }

        self.current_indexed_folder = Some(folder.to_path_buf());
        self.selected_path = None;
        self.last_hovered_path = None;
        self.history_trail.clear();

        if let Ok(db) = ConstellationDb::new() {
            if let Ok(samples) = db.load_samples(folder) {
                self.samples = samples;
                return;
            }
        }

        self.samples.clear();
    }

    /// Starts recursive indexing of the target folder in a background worker thread.
    pub fn start_indexing(&mut self, folder: &Path) {
        self.cancel_indexing();

        let folder_buf = folder.to_path_buf();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();

        let cancel_clone = Arc::clone(&cancelled);
        let folder_clone = folder_buf.clone();

        thread::Builder::new()
            .name("constellation-indexer".to_string())
            .spawn(move || {
                run_indexing(folder_clone, cancel_clone, sender);
            })
            .ok();

        self.active_job = Some(IndexingJob {
            folder: folder_buf,
            cancelled,
            receiver,
        });

        self.indexing_status = Some("Scanning directory tree for audio samples...".to_string());
    }

    /// Cancels any currently active indexing job.
    pub fn cancel_indexing(&mut self) {
        if let Some(job) = self.active_job.take() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        self.indexing_status = None;
    }

    /// Ensures the dedicated background worker for computing hover previews is running.
    pub fn ensure_hover_worker(&mut self) -> &mpsc::Sender<HoverPreviewRequest> {
        if self.hover_req_tx.is_none() {
            let (req_tx, req_rx) = mpsc::channel::<HoverPreviewRequest>();
            let (res_tx, res_rx) = mpsc::channel::<ConstellationHoverPreview>();

            thread::Builder::new()
                .name("constellation-hover-preview".into())
                .spawn(move || {
                    while let Ok(req) = req_rx.recv() {
                        // Coalesce / drain stale requests so we only compute the newest hovered sample
                        let mut latest = req;
                        while let Ok(newer) = req_rx.try_recv() {
                            latest = newer;
                        }

                        let path = &latest.file_path;
                        let info = audio_probe::read_audio_format_info(path);
                        let sample_rate = info.sample_rate.or(latest.sample_rate);
                        let channels = info.channels.or(latest.channels);
                        let bit_depth = info.bit_depth.or(latest.bit_depth);

                        let (waveform, colors) = super::music::preview::waveform(
                            path,
                            latest.duration,
                            64,
                            None,
                            &AtomicBool::new(false),
                        )
                        .unwrap_or_default();

                        let preview = ConstellationHoverPreview {
                            file_path: latest.file_path,
                            waveform,
                            waveform_colors: colors,
                            sample_rate,
                            channels,
                            bit_depth,
                        };
                        let _ = res_tx.send(preview);
                    }
                })
                .expect("Failed to spawn constellation hover preview worker");

            self.hover_req_tx = Some(req_tx);
            self.hover_res_rx = Some(res_rx);
        }
        self.hover_req_tx.as_ref().unwrap()
    }

    /// Drains any completed hover previews from the background worker.
    pub fn poll_hover_previews(&mut self) {
        if let Some(ref rx) = self.hover_res_rx {
            while let Ok(preview) = rx.try_recv() {
                if self.hover_previews.len() > 256 {
                    self.hover_previews.clear();
                }
                self.hover_previews
                    .insert(preview.file_path.clone(), Arc::new(preview));
            }
        }
    }

    /// Triggers audio playback for a sample slot (all layered samples play simultaneously).
    pub fn trigger_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.pack_bank.slots.len() {
            return;
        }
        self.slot_flash[slot_idx] = Some(Instant::now());
        let slot = &self.pack_bank.slots[slot_idx];
        for s in &slot.samples {
            if let Some(player) = sample_pack::play_slot_sample(s) {
                self.active_players.push(player);
            }
        }
    }

    /// Adds a sample from the constellation map into a specific slot as a layer.
    pub fn add_sample_to_slot(&mut self, slot_idx: usize, sample: &ConstellationSample) {
        if slot_idx >= self.pack_bank.slots.len() {
            return;
        }
        let slot_sample = sample_pack::SlotSample::new(
            sample.file_path.clone(),
            sample.filename.clone(),
            Some(sample.category),
            sample.duration_seconds,
        );
        self.pack_bank.slots[slot_idx].samples.push(slot_sample);
    }

    /// Clears all assigned samples from a specific slot.
    pub fn clear_slot(&mut self, slot_idx: usize) {
        if slot_idx < self.pack_bank.slots.len() {
            self.pack_bank.slots[slot_idx].samples.clear();
        }
    }

    /// Scans `<root>/little-screen-clip-sample-packs` for all saved packs.
    pub fn refresh_browser_packs(&mut self, current_folder: &Path) {
        let root = self
            .current_indexed_folder
            .as_deref()
            .unwrap_or(current_folder);
        self.browser_packs = sample_pack::list_saved_packs(root);
    }

    /// Periodic tick called from the application update loop to receive indexing updates.
    pub fn tick(&mut self) {
        self.poll_hover_previews();
        self.active_players.retain(|p| !p.empty());
        let mut job_done = false;

        if let Some(job) = &self.active_job {
            while let Ok(msg) = job.receiver.try_recv() {
                match msg {
                    IndexProgress::Scanning { files_found } => {
                        self.indexing_status =
                            Some(format!("Discovered {} audio files...", files_found));
                    }
                    IndexProgress::Indexing {
                        processed,
                        total,
                        current_file,
                    } => {
                        let pct = (processed as f32 / total.max(1) as f32 * 100.0) as u32;
                        self.indexing_status = Some(format!(
                            "Indexing [{}/{} - {}%]: {}",
                            processed, total, pct, current_file
                        ));
                    }
                    IndexProgress::Complete { samples, .. } => {
                        self.samples = samples;
                        self.indexing_status = None;
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

#[cfg(test)]
mod tests {
    use super::*;

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
