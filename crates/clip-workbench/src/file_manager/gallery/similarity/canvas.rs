//! Zoomable, pannable 1x1 image canvas model arranged by perceptual similarity.
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use egui::{Pos2, Rect, Ui, vec2};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};

use super::{
    dhash::compute_dhash_from_path,
    embedding::{FeatureVector, project_features_to_2d},
    grid::{GridPlacement, grid_dimensions, snap_to_grid},
    scanner::scan_recursive_photos,
};
use crate::file_manager::filesystem::{Directory, Entry};

pub const DEFAULT_TILE_SIZE: f32 = 128.0;
pub const TILE_SPACING: f32 = 4.0;

struct PendingJob<T> {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<T, JobError>>,
}

impl<T> Drop for PendingJob<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}

pub(in crate::file_manager) struct SimilarityCanvas {
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub tile_size: f32,
    pub spacing: f32,
    pub(in crate::file_manager) entries: Vec<Entry>,
    pub placements: Vec<GridPlacement>,
    pub grid_cols: usize,
    pub grid_rows: usize,
    hashes: HashMap<PathBuf, (FeatureVector, u64)>, // path -> (feature, revision)
    current_signature: Option<(PathBuf, u64, bool, usize)>,
    scan_job: Option<PendingJob<Vec<Entry>>>,
    hash_job: Option<PendingJob<Vec<(PathBuf, u64, FeatureVector)>>>,
    initial_fit_done: bool,
    pub hashing_progress: (usize, usize), // (completed, total)
}

impl Default for SimilarityCanvas {
    fn default() -> Self {
        Self {
            pan: egui::vec2(24.0, 24.0),
            zoom: 1.0,
            tile_size: DEFAULT_TILE_SIZE,
            spacing: TILE_SPACING,
            entries: Vec::new(),
            placements: Vec::new(),
            grid_cols: 0,
            grid_rows: 0,
            hashes: HashMap::new(),
            current_signature: None,
            scan_job: None,
            hash_job: None,
            initial_fit_done: false,
            hashing_progress: (0, 0),
        }
    }
}

fn worker_pool() -> &'static BackgroundJobs {
    static JOBS: std::sync::LazyLock<BackgroundJobs> =
        std::sync::LazyLock::new(|| BackgroundJobs::new(4, 16));
    &JOBS
}

impl SimilarityCanvas {
    /// Resets panning and zoom state.
    pub fn reset_view(&mut self) {
        self.initial_fit_done = false;
        self.pan = egui::vec2(24.0, 24.0);
        self.zoom = 1.0;
    }

    /// Retires pending background jobs.
    pub fn retire(&mut self) {
        self.scan_job = None;
        self.hash_job = None;
    }

    pub fn is_scanning(&self) -> bool {
        self.scan_job.is_some()
    }

    pub fn is_hashing(&self) -> bool {
        self.hash_job.is_some()
    }

    /// Synchronizes entries with the active directory or launches recursive scan.
    pub(in crate::file_manager) fn sync(
        &mut self,
        directory: &Directory,
        visible_indices: &[usize],
        recurse: bool,
        show_hidden: bool,
    ) {
        let sig = (
            directory.path.clone(),
            directory.signature,
            recurse,
            visible_indices.len(),
        );
        if self.current_signature.as_ref() == Some(&sig) {
            return;
        }
        self.current_signature = Some(sig);
        self.initial_fit_done = false;

        if recurse {
            let cancelled = Arc::new(AtomicBool::new(false));
            let root = directory.path.clone();
            let c = Arc::clone(&cancelled);
            let pool = worker_pool();
            let (ticket, receiver) = pool.submit(Duration::from_secs(120), move || {
                scan_recursive_photos(&root, show_hidden, &c)
            });
            self.scan_job = Some(PendingJob {
                ticket,
                cancelled,
                receiver,
            });
            let photos: Vec<Entry> = visible_indices
                .iter()
                .filter_map(|&idx| directory.entries.get(idx))
                .filter(|e| e.is_photo())
                .cloned()
                .collect();
            self.set_entries(photos);
        } else {
            self.scan_job = None;
            let photos: Vec<Entry> = visible_indices
                .iter()
                .filter_map(|&idx| directory.entries.get(idx))
                .filter(|e| e.is_photo())
                .cloned()
                .collect();
            self.set_entries(photos);
        }
    }

    fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        let n = self.entries.len();
        let (cols, rows) = grid_dimensions(n);
        self.grid_cols = cols;
        self.grid_rows = rows;

        self.placements = (0..n)
            .map(|i| GridPlacement {
                index: i,
                col: if cols > 0 { i % cols } else { 0 },
                row: if cols > 0 { i / cols } else { 0 },
            })
            .collect();

        self.schedule_hashing();
    }

    fn schedule_hashing(&mut self) {
        if self.entries.is_empty() {
            self.hashing_progress = (0, 0);
            return;
        }

        let missing: Vec<(PathBuf, u64)> = self
            .entries
            .iter()
            .filter(|e| {
                self.hashes
                    .get(&e.path)
                    .map_or(true, |&(_, rev)| rev != e.revision)
            })
            .map(|e| (e.path.clone(), e.revision))
            .collect();

        let total = self.entries.len();
        let already = total - missing.len();
        self.hashing_progress = (already, total);

        if missing.is_empty() {
            self.recompute_layout();
            return;
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&cancelled);
        let pool = worker_pool();
        let (ticket, receiver) = pool.submit(Duration::from_secs(120), move || {
            let mut results = Vec::new();
            for (path, rev) in missing {
                if c.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(hash) = compute_dhash_from_path(&path, &c) {
                    results.push((path, rev, FeatureVector::DHash(hash)));
                }
            }
            results
        });
        self.hash_job = Some(PendingJob {
            ticket,
            cancelled,
            receiver,
        });
    }

    /// Polls background jobs and requests repaints.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.scan_job {
            if let Ok(Ok(entries)) = job.receiver.try_recv() {
                self.scan_job = None;
                self.set_entries(entries);
                ctx.request_repaint();
            }
        }

        if let Some(job) = &self.hash_job {
            if let Ok(Ok(results)) = job.receiver.try_recv() {
                self.hash_job = None;
                for (path, rev, feat) in results {
                    self.hashes.insert(path, (feat, rev));
                }
                self.hashing_progress.0 = self.hashes.len().min(self.entries.len());
                self.recompute_layout();
                ctx.request_repaint();
            }
        }

        if self.scan_job.is_some() || self.hash_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(32));
        }
    }

    fn recompute_layout(&mut self) {
        let n = self.entries.len();
        if n == 0 {
            self.placements.clear();
            return;
        }

        let features: Vec<Vec<f32>> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                self.hashes
                    .get(&e.path)
                    .map(|(f, _)| f.to_vec())
                    .unwrap_or_else(|| FeatureVector::DHash(i as u64).to_vec())
            })
            .collect();

        let coords_2d = project_features_to_2d(&features);
        self.placements = snap_to_grid(&coords_2d);
        let (cols, rows) = grid_dimensions(n);
        self.grid_cols = cols;
        self.grid_rows = rows;
    }

    /// Fits the entire grid neatly inside the given viewport.
    pub fn fit_to_view(&mut self, viewport: Rect) {
        if self.grid_cols == 0 || self.grid_rows == 0 {
            return;
        }
        let total_w = self.grid_cols as f32 * (self.tile_size + self.spacing);
        let total_h = self.grid_rows as f32 * (self.tile_size + self.spacing);
        let pad = 32.0;
        let scale_x = (viewport.width() - pad) / total_w.max(1.0);
        let scale_y = (viewport.height() - pad) / total_h.max(1.0);
        let fit_zoom = scale_x.min(scale_y).clamp(0.05, 2.5);
        self.zoom = fit_zoom;
        let scaled_w = total_w * self.zoom;
        let scaled_h = total_h * self.zoom;
        self.pan = egui::vec2(
            ((viewport.width() - scaled_w) * 0.5).max(16.0),
            ((viewport.height() - scaled_h) * 0.5).max(16.0),
        );
    }

    /// Handles pan dragging and zoom mouse wheel events.
    pub fn handle_input(&mut self, ui: &mut Ui, canvas_rect: Rect, response: &egui::Response) {
        if !self.initial_fit_done && canvas_rect.width() > 100.0 && canvas_rect.height() > 100.0 {
            self.fit_to_view(canvas_rect);
            self.initial_fit_done = true;
        }

        if response.dragged() {
            self.pan += response.drag_delta();
        }

        let pointer_pos = ui
            .input(|i| i.pointer.hover_pos())
            .unwrap_or(canvas_rect.center());
        if canvas_rect.contains(pointer_pos) {
            let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
            if scroll_delta.abs() > 0.01 {
                let zoom_factor = (1.0 + scroll_delta * 0.002).clamp(0.75, 1.35);
                let new_zoom = (self.zoom * zoom_factor).clamp(0.05, 8.0);
                let origin_offset = pointer_pos - canvas_rect.min - self.pan;
                self.pan = pointer_pos - canvas_rect.min - origin_offset * (new_zoom / self.zoom);
                self.zoom = new_zoom;
            }
        }
    }

    /// Calculates screen rect for a tile at grid column `col` and row `row`.
    pub fn tile_screen_rect(&self, canvas_origin: Pos2, col: usize, row: usize) -> Rect {
        let unit_size = self.tile_size * self.zoom;
        let unit_spacing = self.spacing * self.zoom;
        let step = unit_size + unit_spacing;
        let pos = canvas_origin + self.pan + vec2(col as f32 * step, row as f32 * step);
        Rect::from_min_size(pos, vec2(unit_size, unit_size))
    }

    /// Returns the collection of image paths currently displayed on canvas.
    pub fn photo_collection(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|e| e.path.clone()).collect()
    }
}
