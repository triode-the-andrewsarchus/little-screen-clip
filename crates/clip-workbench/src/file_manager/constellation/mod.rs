//! 2D sample constellation view and classification system inspired by 2D audio sample mapping tools.

pub mod classifier;
pub mod db;
pub mod generated_model;
pub mod indexer;
pub mod ui;

use db::{ConstellationDb, ConstellationSample};
use generated_model::SampleCategory;
use indexer::{IndexProgress, run_indexing};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

pub struct IndexingJob {
    pub folder: PathBuf,
    pub cancelled: Arc<AtomicBool>,
    pub receiver: mpsc::Receiver<IndexProgress>,
}

pub struct ConstellationState {
    pub samples: Vec<ConstellationSample>,
    pub current_indexed_folder: Option<PathBuf>,
    pub category_filter: Option<SampleCategory>,
    pub search_query: String,
    pub hover_to_play: bool,
    pub selected_path: Option<PathBuf>,
    pub last_hovered_path: Option<PathBuf>,
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub active_job: Option<IndexingJob>,
    pub indexing_status: Option<String>,
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
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            active_job: None,
            indexing_status: None,
        }
    }
}

impl ConstellationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the path of the folder currently being indexed, if an indexing job is active.
    pub fn indexing_folder(&self) -> Option<&Path> {
        self.active_job.as_ref().map(|j| j.folder.as_path())
    }

    /// Checks if the specified folder is currently being indexed.
    pub fn is_indexing_folder(&self, folder: &Path) -> bool {
        self.indexing_folder() == Some(folder)
    }

    /// Loads indexed samples from SQLite for the given folder if not already loaded.
    pub fn check_and_load_folder(&mut self, folder: &Path) {
        if self.current_indexed_folder.as_deref() == Some(folder) {
            return;
        }

        self.current_indexed_folder = Some(folder.to_path_buf());
        self.selected_path = None;
        self.last_hovered_path = None;

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

    /// Periodic tick called from the application update loop to receive indexing updates.
    pub fn tick(&mut self) {
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
