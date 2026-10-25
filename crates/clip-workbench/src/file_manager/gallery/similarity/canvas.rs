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
    siglip::SiglipTextEmbedder,
};
use crate::file_manager::filesystem::{Directory, Entry};

/// Default tile edge size in logical points at 1.0 zoom.
pub const DEFAULT_TILE_SIZE: f32 = 128.0;
/// Spacing between adjacent tiles in logical points.
pub const TILE_SPACING: f32 = 4.0;
/// Minimum search match allowance (0.05 = 5%).
pub const MIN_ALLOWANCE: f32 = 0.05;
/// Maximum search match allowance (0.95 = 95%).
pub const MAX_ALLOWANCE: f32 = 0.95;
/// Minimum canvas zoom factor.
pub const MIN_ZOOM: f32 = 0.05;
/// Maximum canvas zoom factor.
pub const MAX_ZOOM: f32 = 8.0;
/// Keyboard zoom multiplier per step.
pub const KEYBOARD_ZOOM_STEP: f32 = 1.25;
const MIN_FIT_ZOOM: f32 = 0.05;
const MAX_FIT_ZOOM: f32 = 2.5;
const SCROLL_ZOOM_SENSITIVITY: f32 = 0.002;
const MIN_SCROLL_ZOOM_FACTOR: f32 = 0.75;
const MAX_SCROLL_ZOOM_FACTOR: f32 = 1.35;

// Grid layout spacing
const GRID_COL_GAP: usize = 2;
const GRID_ROW_GAP: usize = 1;

// Pan-to-grid margins
const GRID_PAN_TOP_MARGIN: f32 = 76.0;
const GRID_PAN_HEADER_SPACING: f32 = 46.0;
const GRID_PAN_SIDE_PADDING: f32 = 80.0;
const GRID_PAN_LEFT_MARGIN: f32 = 40.0;

/// A sub-grid of image tiles on the canvas representing a search result or base set.
#[derive(Clone, Debug)]
pub struct CanvasGrid {
    pub id: usize,
    pub title: String,
    #[allow(dead_code)]
    pub query: Option<String>,
    pub is_inverted: bool,
    pub candidate_indices: Vec<usize>,
    pub concept_embedding: Option<Vec<f32>>,
    pub indices: Vec<usize>,
    pub selected_for_search: bool,
    pub grid_cols: usize,
    pub grid_rows: usize,
    pub col_offset: usize,
    pub row_offset: usize,
    pub allowance: f32,
    pub threshold: f32,
}

impl CanvasGrid {
    /// Converts a user-facing match allowance `[0.05, 0.95]` to an internal cosine threshold.
    pub fn allowance_to_threshold(allowance: f32, is_inverted: bool) -> f32 {
        let a = allowance.clamp(0.0, 1.0);
        if is_inverted {
            0.01 + 0.39 * a.powf(2.5)
        } else {
            0.01 + 0.39 * (1.0 - a).powf(2.5)
        }
    }

    /// Converts an internal cosine threshold to a user-facing match allowance `[0.05, 0.95]`.
    #[allow(dead_code)]
    pub fn threshold_to_allowance(threshold: f32, is_inverted: bool) -> f32 {
        let t = threshold.clamp(0.01, 0.40);
        let ratio = ((t - 0.01) / 0.39).clamp(0.0, 1.0);
        let p = 1.0 / 2.5;
        if is_inverted {
            ratio.powf(p).clamp(0.0, 1.0)
        } else {
            (1.0 - ratio.powf(p)).clamp(0.0, 1.0)
        }
    }

    /// Incremental step size for match allowance adjustments.
    pub const ALLOWANCE_STEP: f32 = 0.05;

    /// Adjusts the match allowance by `delta` and updates the internal similarity threshold.
    pub fn adjust_allowance(&mut self, delta: f32) {
        let new_val = ((self.allowance + delta) * 100.0).round() / 100.0;
        self.allowance = new_val.clamp(MIN_ALLOWANCE, MAX_ALLOWANCE);
        self.threshold = Self::allowance_to_threshold(self.allowance, self.is_inverted);
    }

    /// Sets the match allowance and updates the internal similarity threshold.
    pub fn set_allowance(&mut self, allowance: f32) {
        let new_val = (allowance * 100.0).round() / 100.0;
        self.allowance = new_val.clamp(MIN_ALLOWANCE, MAX_ALLOWANCE);
        self.threshold = Self::allowance_to_threshold(self.allowance, self.is_inverted);
    }

    fn compute_candidate_scores(
        &self,
        hashes: &HashMap<PathBuf, (FeatureVector, u64)>,
        entries: &[Entry],
    ) -> Option<Vec<(usize, f32)>> {
        let concept_vec = self.concept_embedding.as_ref()?;
        let mut scored = Vec::with_capacity(self.candidate_indices.len());
        for &idx in &self.candidate_indices {
            let Some(entry) = entries.get(idx) else {
                continue;
            };
            let score = if let Some((feat, _)) = hashes.get(&entry.path) {
                match feat {
                    FeatureVector::Dense(img_vec) if img_vec.len() == concept_vec.len() => img_vec
                        .iter()
                        .zip(concept_vec.iter())
                        .map(|(&a, &b)| a * b)
                        .sum::<f32>(),
                    FeatureVector::DHash(_) if concept_vec.len() == 64 => {
                        let img_vec = feat.to_vec();
                        img_vec
                            .iter()
                            .zip(concept_vec.iter())
                            .map(|(&a, &b)| (a * 0.25) * b)
                            .sum::<f32>()
                    }
                    _ => -1.0,
                }
            } else {
                -1.0
            };
            scored.push((idx, score));
        }
        Some(scored)
    }

    /// Filters candidate entries according to cosine similarity against the concept embedding.
    pub fn refilter(
        &mut self,
        hashes: &HashMap<PathBuf, (FeatureVector, u64)>,
        entries: &[Entry],
        threshold: f32,
    ) {
        let Some(scored) = self.compute_candidate_scores(hashes, entries) else {
            return;
        };

        if self.is_inverted {
            // Inverted search: match items where score < threshold (and valid score >= 0.0)
            let mut matches: Vec<(usize, f32)> = scored
                .into_iter()
                .filter(|&(_, s)| s >= 0.0 && s < threshold)
                .collect();
            // Sort ascending: lowest score (least similar to inverted concept) comes first
            matches.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            self.indices = matches.into_iter().map(|(idx, _)| idx).collect();
        } else {
            // Normal search: match items where score >= threshold
            let mut matches: Vec<(usize, f32)> = scored
                .into_iter()
                .filter(|&(_, s)| s >= threshold)
                .collect();
            // Sort descending: highest score (most similar to concept) comes first
            matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            self.indices = matches.into_iter().map(|(idx, _)| idx).collect();
        }
    }

    pub fn count_matches_at_allowance(
        &self,
        hashes: &HashMap<PathBuf, (FeatureVector, u64)>,
        entries: &[Entry],
        allowance: f32,
    ) -> usize {
        let Some(scored) = self.compute_candidate_scores(hashes, entries) else {
            return self.indices.len();
        };
        let threshold = Self::allowance_to_threshold(allowance, self.is_inverted);
        if self.is_inverted {
            scored
                .iter()
                .filter(|&&(_, s)| s >= 0.0 && s < threshold)
                .count()
        } else {
            scored.iter().filter(|&&(_, s)| s >= threshold).count()
        }
    }

    /// Returns `true` if decreasing the allowance would still leave at least one matching photo.
    pub fn can_decrease_allowance(
        &self,
        hashes: &HashMap<PathBuf, (FeatureVector, u64)>,
        entries: &[Entry],
    ) -> bool {
        if self.concept_embedding.is_none() || self.indices.is_empty() {
            return false;
        }
        if self.allowance <= MIN_ALLOWANCE + 1e-4 {
            return false;
        }
        let next_allowance = (self.allowance - Self::ALLOWANCE_STEP).max(MIN_ALLOWANCE);
        let next_count = self.count_matches_at_allowance(hashes, entries, next_allowance);
        // Gray out and prevent pressing (-) right before pressing it would mean 0 results
        next_count > 0
    }

    /// Returns `true` if increasing the allowance would yield more matching photos.
    pub fn can_increase_allowance(
        &self,
        hashes: &HashMap<PathBuf, (FeatureVector, u64)>,
        entries: &[Entry],
    ) -> bool {
        if self.concept_embedding.is_none() {
            return false;
        }
        // 1. When it's now so broad that it includes all of the original search
        if !self.candidate_indices.is_empty() && self.indices.len() >= self.candidate_indices.len()
        {
            return false;
        }
        if self.allowance >= MAX_ALLOWANCE - 1e-4 {
            return false;
        }
        // 2. Or if no matter how high the value goes it wouldn't include more photos
        let max_count = self.count_matches_at_allowance(hashes, entries, 1.0);
        if max_count <= self.indices.len() {
            return false;
        }
        true
    }
}

pub struct PendingSearchMeta {
    pub query: String,
    #[allow(dead_code)]
    pub prompt: String,
    pub is_inverted: bool,
    pub candidate_indices: Vec<usize>,
}

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

struct HashJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    batch_rx: mpsc::Receiver<Vec<(PathBuf, u64, FeatureVector)>>,
}

impl Drop for HashJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}

/// Interactive 2D canvas displaying clustered photo similarity grids.
pub(in crate::file_manager) struct SimilarityCanvas {
    pub pan: egui::Vec2,
    pub zoom: f32,
    pub tile_size: f32,
    pub spacing: f32,
    pub(in crate::file_manager) entries: Vec<Entry>,
    pub placements: Vec<GridPlacement>,
    pub grid_cols: usize,
    pub grid_rows: usize,
    pub(in crate::file_manager) hashes: HashMap<PathBuf, (FeatureVector, u64)>, // path -> (feature, revision)
    current_signature: Option<(PathBuf, u64, bool, usize)>,
    scan_job: Option<PendingJob<Vec<Entry>>>,
    hash_job: Option<HashJob>,
    search_job: Option<PendingJob<Option<Vec<f32>>>>,
    pub initial_fit_done: bool,
    pub is_middle_panning: bool,
    pub hashing_progress: (usize, usize), // (completed, total)
    pub search_query: String,
    pub pending_search: Option<PendingSearchMeta>,
    pub allowance: f32,
    pub similarity_threshold: f32,
    pub grids: Vec<CanvasGrid>,
    pub next_grid_id: usize,
    pub pending_pan_to_grid: Option<usize>,
    base_placements: Vec<GridPlacement>,
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
            search_job: None,
            initial_fit_done: false,
            is_middle_panning: false,
            hashing_progress: (0, 0),
            search_query: String::new(),
            pending_search: None,
            allowance: 0.50,
            similarity_threshold: 0.08,
            grids: Vec::new(),
            next_grid_id: 1,
            pending_pan_to_grid: None,
            base_placements: Vec::new(),
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
        self.search_job = None;
    }

    /// Stores a feature vector for a path (used in tests and internal cache updates).
    #[allow(dead_code)]
    pub fn insert_hash(&mut self, path: PathBuf, feat: FeatureVector, rev: u64) {
        self.hashes.insert(path, (feat, rev));
    }

    /// Returns `true` if a background recursive photo scan is in progress.
    pub fn is_scanning(&self) -> bool {
        self.scan_job.is_some()
    }

    /// Returns `true` if background feature hashing/embedding is in progress.
    pub fn is_hashing(&self) -> bool {
        self.hash_job.is_some()
    }

    /// Returns `true` if a background text search embedding query is in flight.
    pub fn is_searching(&self) -> bool {
        self.search_job.is_some()
    }

    /// Dispatches an asynchronous concept search by embedding the search query.
    pub fn trigger_search(&mut self) {
        let q = self.search_query.trim().to_string();
        if q.is_empty() {
            return;
        }

        let is_inverted = q.starts_with('!');
        let prompt = if is_inverted {
            q[1..].trim().to_string()
        } else {
            q.clone()
        };
        if prompt.is_empty() {
            return;
        }

        // Collect candidate indices from all grids that have selected_for_search == true
        let mut candidate_set = std::collections::BTreeSet::new();
        for grid in &self.grids {
            if grid.selected_for_search {
                for &idx in &grid.indices {
                    candidate_set.insert(idx);
                }
            }
        }

        // Fallback to base grid (all photos) if nothing is selected
        if candidate_set.is_empty() {
            if let Some(base) = self.grids.first() {
                candidate_set.extend(&base.indices);
            } else {
                candidate_set.extend(0..self.entries.len());
            }
        }

        let candidate_indices: Vec<usize> = candidate_set.into_iter().collect();

        self.pending_search = Some(PendingSearchMeta {
            query: q,
            prompt: prompt.clone(),
            is_inverted,
            candidate_indices,
        });

        self.schedule_hashing();
        let cancelled = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&cancelled);
        let pool = worker_pool();
        let (ticket, receiver) = pool.submit(Duration::from_secs(60), move || {
            if c.load(Ordering::Relaxed) {
                return None;
            }
            let mut embedder = SiglipTextEmbedder::try_new().ok()?;
            if c.load(Ordering::Relaxed) {
                return None;
            }
            embedder.embed_text(&prompt).ok()
        });
        self.search_job = Some(PendingJob {
            ticket,
            cancelled,
            receiver,
        });
    }

    /// Clears all searches and restores base grid.
    pub fn clear_all_searches(&mut self) {
        self.search_query.clear();
        self.pending_search = None;
        self.search_job = None;
        self.grids.retain(|g| g.id == 0);
        if let Some(base) = self.grids.first_mut() {
            base.selected_for_search = true;
        }
        self.recompute_layout();
    }

    /// Alias for clear_all_searches.
    #[allow(dead_code)]
    pub fn clear_search(&mut self) {
        self.clear_all_searches();
    }

    /// Removes a search grid by ID. Base grid (id 0) cannot be removed.
    pub fn remove_grid(&mut self, id: usize) {
        if id == 0 {
            return;
        }
        self.grids.retain(|g| g.id != id);
        if !self.grids.iter().any(|g| g.selected_for_search)
            && let Some(base) = self.grids.first_mut()
        {
            base.selected_for_search = true;
        }
        self.recompute_layout();
    }

    #[allow(dead_code)]
    pub fn adjust_global_allowance(&mut self, delta: f32) {
        self.allowance = (self.allowance + delta).clamp(MIN_ALLOWANCE, MAX_ALLOWANCE);
        self.similarity_threshold = CanvasGrid::allowance_to_threshold(self.allowance, false);
        for g in self.grids.iter_mut().skip(1) {
            g.set_allowance(self.allowance);
        }
    }

    pub fn can_decrease_grid_allowance(&self, grid_id: usize) -> bool {
        let Some(grid) = self.grids.iter().find(|g| g.id == grid_id) else {
            return false;
        };
        grid.can_decrease_allowance(&self.hashes, &self.entries)
    }

    pub fn can_increase_grid_allowance(&self, grid_id: usize) -> bool {
        let Some(grid) = self.grids.iter().find(|g| g.id == grid_id) else {
            return false;
        };
        grid.can_increase_allowance(&self.hashes, &self.entries)
    }

    pub fn adjust_grid_allowance(&mut self, grid_id: usize, delta: f32) -> bool {
        if delta < 0.0 && !self.can_decrease_grid_allowance(grid_id) {
            return false;
        }
        if delta > 0.0 && !self.can_increase_grid_allowance(grid_id) {
            return false;
        }
        if let Some(grid) = self.grids.iter_mut().find(|g| g.id == grid_id) {
            grid.adjust_allowance(delta);
            true
        } else {
            false
        }
    }

    pub fn result_grid_at_pos(&self, canvas_origin: Pos2, pointer_pos: Pos2) -> Option<usize> {
        let step = (self.tile_size + self.spacing) * self.zoom;
        // Search result grids have id > 0
        // Check tile areas first for exact hit testing
        for grid in self.grids.iter().filter(|g| g.id > 0) {
            let top_left = self.tile_screen_rect(canvas_origin, grid.col_offset, grid.row_offset);
            let w = (grid.grid_cols.max(1) as f32) * step;
            let h = (grid.grid_rows.max(1) as f32) * step;
            let tiles_rect = Rect::from_min_size(top_left.min, vec2(w, h));
            if tiles_rect.contains(pointer_pos) {
                return Some(grid.id);
            }
        }
        // If not directly over tiles, check header bars
        for grid in self.grids.iter().filter(|g| g.id > 0) {
            let top_left = self.tile_screen_rect(canvas_origin, grid.col_offset, grid.row_offset);
            let w = (grid.grid_cols.max(1) as f32) * step;
            let bar_w = w.max(300.0 * self.zoom.clamp(0.5, 1.0));
            let header_rect = Rect::from_min_size(
                egui::pos2(top_left.min.x, top_left.min.y - 48.0),
                vec2(bar_w, 48.0),
            );
            if header_rect.contains(pointer_pos) {
                return Some(grid.id);
            }
        }
        None
    }

    /// Creates a new search grid searching for photos visually similar to the reference photo.
    pub fn search_from_photo(&mut self, entry_path: &std::path::Path) -> Option<usize> {
        let feature = if let Some((feat, _)) = self.hashes.get(entry_path) {
            feat.clone()
        } else if let Some(hash) = compute_dhash_from_path(entry_path, &AtomicBool::new(false)) {
            FeatureVector::DHash(hash)
        } else {
            return None;
        };

        let mut vec = feature.to_vec();
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for x in &mut vec {
                *x /= norm;
            }
        }

        let selected: Vec<_> = self
            .grids
            .iter()
            .filter(|g| g.selected_for_search)
            .collect();
        let candidate_indices: Vec<usize> = if selected.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            let mut set = std::collections::BTreeSet::new();
            for g in selected {
                set.extend(&g.indices);
            }
            set.into_iter().collect()
        };

        let file_name = entry_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "photo".to_string());

        let id = self.next_grid_id;
        self.next_grid_id += 1;
        let allowance = self.allowance;
        let threshold = CanvasGrid::allowance_to_threshold(allowance, false);
        let mut grid = CanvasGrid {
            id,
            title: format!("Similar to {file_name}"),
            query: None,
            is_inverted: false,
            candidate_indices,
            concept_embedding: Some(vec),
            indices: Vec::new(),
            selected_for_search: true,
            grid_cols: 0,
            grid_rows: 0,
            col_offset: 0,
            row_offset: 0,
            allowance,
            threshold,
        };
        grid.refilter(&self.hashes, &self.entries, threshold);
        for g in &mut self.grids {
            g.selected_for_search = false;
        }
        self.grids.push(grid);
        self.recompute_layout();
        self.pending_pan_to_grid = Some(id);
        Some(id)
    }

    #[cfg(test)]
    pub fn add_search_grid(
        &mut self,
        query: &str,
        is_inverted: bool,
        candidate_indices: Vec<usize>,
        concept_embedding: Vec<f32>,
    ) -> usize {
        let id = self.next_grid_id;
        self.next_grid_id += 1;
        let allowance = self.allowance;
        let threshold = CanvasGrid::allowance_to_threshold(allowance, is_inverted);
        let mut grid = CanvasGrid {
            id,
            title: query.to_string(),
            query: Some(query.to_string()),
            is_inverted,
            candidate_indices,
            concept_embedding: Some(concept_embedding),
            indices: Vec::new(),
            selected_for_search: true,
            grid_cols: 0,
            grid_rows: 0,
            col_offset: 0,
            row_offset: 0,
            allowance,
            threshold,
        };
        grid.refilter(&self.hashes, &self.entries, threshold);
        for g in &mut self.grids {
            g.selected_for_search = false;
        }
        self.grids.push(grid);
        self.recompute_layout();
        self.pending_pan_to_grid = Some(id);
        id
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

    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        let n = self.entries.len();
        let (cols, rows) = grid_dimensions(n);
        self.grid_cols = cols;
        self.grid_rows = rows;
        self.base_placements.clear();

        self.grids = vec![CanvasGrid {
            id: 0,
            title: "All photos".to_string(),
            query: None,
            is_inverted: false,
            candidate_indices: (0..n).collect(),
            concept_embedding: None,
            indices: (0..n).collect(),
            selected_for_search: true,
            grid_cols: cols,
            grid_rows: rows,
            col_offset: 0,
            row_offset: 0,
            allowance: self.allowance,
            threshold: self.similarity_threshold,
        }];
        self.next_grid_id = 1;

        let safe_cols = cols.max(1);
        self.placements = (0..n)
            .map(|i| GridPlacement {
                index: i,
                col: i % safe_cols,
                row: i / safe_cols,
            })
            .collect();

        self.schedule_hashing();
    }

    fn schedule_hashing(&mut self) {
        if self.entries.is_empty() {
            self.hashing_progress = (0, 0);
            return;
        }

        let is_siglip_available =
            crate::onnx_runtime::is_model_cached(crate::onnx_runtime::SIGLIP2_GIANT_MODEL_NAME);

        let missing: Vec<(PathBuf, u64)> = self
            .entries
            .iter()
            .filter(|e| {
                self.hashes.get(&e.path).is_none_or(|(feat, rev)| {
                    *rev != e.revision
                        || (is_siglip_available && matches!(feat, FeatureVector::DHash(_)))
                })
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
        let (batch_tx, batch_rx) = mpsc::channel();

        let (ticket, _completion_rx) = pool.submit(Duration::from_secs(3600), move || {
            super::index::index_images(missing, &c, batch_tx);
        });
        self.hash_job = Some(HashJob {
            ticket,
            cancelled,
            batch_rx,
        });
    }

    /// Polls background jobs and requests repaints.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.scan_job
            && let Ok(Ok(entries)) = job.receiver.try_recv()
        {
            self.scan_job = None;
            self.set_entries(entries);
            ctx.request_repaint();
        }

        if let Some(job) = &mut self.hash_job {
            let mut received_any = false;
            let mut finished = false;
            loop {
                match job.batch_rx.try_recv() {
                    Ok(batch) => {
                        for (path, rev, feat) in batch {
                            self.hashes.insert(path, (feat, rev));
                        }
                        self.hashing_progress.0 = self.hashes.len().min(self.entries.len());
                        received_any = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
            if finished {
                self.hash_job = None;
                self.base_placements.clear();
                self.recompute_layout();
                ctx.request_repaint();
            } else if received_any {
                if self.grids.len() > 1 {
                    self.recompute_layout();
                }
                ctx.request_repaint();
            }
        }

        if let Some(job) = &self.search_job
            && let Ok(res) = job.receiver.try_recv()
        {
            self.search_job = None;
            if let Ok(Some(emb)) = res
                && let Some(meta) = self.pending_search.take()
            {
                // Deselect other grids so consecutive searches chain to this new grid by default
                for g in &mut self.grids {
                    g.selected_for_search = false;
                }

                let allowance = self.allowance;
                let threshold = CanvasGrid::allowance_to_threshold(allowance, meta.is_inverted);
                let mut new_grid = CanvasGrid {
                    id: self.next_grid_id,
                    title: meta.query.clone(),
                    query: Some(meta.query),
                    is_inverted: meta.is_inverted,
                    candidate_indices: meta.candidate_indices,
                    concept_embedding: Some(emb),
                    indices: Vec::new(),
                    selected_for_search: true, // Selected for next search
                    grid_cols: 0,
                    grid_rows: 0,
                    col_offset: 0,
                    row_offset: 0,
                    allowance,
                    threshold,
                };
                self.next_grid_id += 1;
                new_grid.refilter(&self.hashes, &self.entries, threshold);
                let new_id = new_grid.id;
                self.grids.push(new_grid);
                self.recompute_layout();
                self.pending_pan_to_grid = Some(new_id);
            }
            ctx.request_repaint();
        }

        if self.scan_job.is_some() || self.hash_job.is_some() || self.search_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(32));
        }
    }

    pub fn recompute_layout(&mut self) {
        let n = self.entries.len();
        if n == 0 {
            self.placements.clear();
            self.grid_cols = 0;
            self.grid_rows = 0;
            return;
        }

        if self.grids.is_empty() {
            let (c, r) = grid_dimensions(n);
            self.grids.push(CanvasGrid {
                id: 0,
                title: "All photos".to_string(),
                query: None,
                is_inverted: false,
                candidate_indices: (0..n).collect(),
                concept_embedding: None,
                indices: (0..n).collect(),
                selected_for_search: true,
                grid_cols: c,
                grid_rows: r,
                col_offset: 0,
                row_offset: 0,
                allowance: self.allowance,
                threshold: self.similarity_threshold,
            });
        }

        // Keep base grid indices synced with all entries
        if let Some(base) = self.grids.first_mut() {
            base.candidate_indices = (0..n).collect();
            base.indices = (0..n).collect();
        }

        // Refilter any search grids with their own threshold
        for grid in self.grids.iter_mut().skip(1) {
            let threshold = grid.threshold;
            grid.refilter(&self.hashes, &self.entries, threshold);
        }

        // Compute base placements if needed
        if self.base_placements.len() != n {
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
            self.base_placements = snap_to_grid(&coords_2d);
        }

        let max_column_height = self.grids.first().map_or(10, |base| base.grid_rows.max(8));

        let mut max_overall_cols = 0;
        let mut max_overall_rows = 0;
        let mut new_placements = Vec::new();

        let mut col_start_x = 0;
        let mut col_used_height = 0;
        let mut col_width = 0;

        for (grid_idx, grid) in self.grids.iter_mut().enumerate() {
            if grid_idx == 0 {
                let (c, r) = grid_dimensions(n);
                grid.grid_cols = c;
                grid.grid_rows = r;
                grid.col_offset = 0;
                grid.row_offset = 0;
                max_overall_rows = max_overall_rows.max(r);
                max_overall_cols = max_overall_cols.max(c);

                for p in &self.base_placements {
                    new_placements.push(GridPlacement {
                        index: p.index,
                        col: p.col,
                        row: p.row,
                    });
                }
                col_start_x = c + GRID_COL_GAP;
                col_used_height = 0;
                col_width = 0;
            } else {
                let count = grid.indices.len();
                let (c, r) = grid_dimensions(count);
                let eff_r = r.max(1);
                let eff_c = c.max(1);
                grid.grid_cols = c;
                grid.grid_rows = r;

                // Check if this grid fits vertically in the current column
                let needed_height = if col_used_height > 0 {
                    col_used_height + GRID_ROW_GAP + eff_r
                } else {
                    eff_r
                };

                if col_used_height > 0 && needed_height > max_column_height {
                    // Start a new column to the right
                    col_start_x += col_width + GRID_COL_GAP;
                    col_used_height = 0;
                    col_width = 0;
                }

                let row_offset = if col_used_height > 0 {
                    col_used_height + GRID_ROW_GAP
                } else {
                    0
                };

                grid.col_offset = col_start_x;
                grid.row_offset = row_offset;

                col_used_height = row_offset + eff_r;
                col_width = col_width.max(eff_c);

                max_overall_rows = max_overall_rows.max(col_used_height);
                max_overall_cols = max_overall_cols.max(col_start_x + col_width);

                let safe_c = eff_c;
                for (rank, &entry_idx) in grid.indices.iter().enumerate() {
                    new_placements.push(GridPlacement {
                        index: entry_idx,
                        col: col_start_x + (rank % safe_c),
                        row: row_offset + (rank / safe_c),
                    });
                }
            }
        }

        self.placements = new_placements;
        self.grid_cols = max_overall_cols;
        self.grid_rows = max_overall_rows;
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
        let fit_zoom = scale_x.min(scale_y).clamp(MIN_FIT_ZOOM, MAX_FIT_ZOOM);
        self.zoom = fit_zoom;
        let scaled_w = total_w * self.zoom;
        let scaled_h = total_h * self.zoom;
        self.pan = egui::vec2(
            ((viewport.width() - scaled_w) * 0.5).max(16.0),
            ((viewport.height() - scaled_h) * 0.5).max(16.0),
        );
    }

    /// Pans the canvas so that the given grid and its header bar are positioned in view.
    pub fn pan_to_grid(&mut self, grid_id: usize, viewport: Rect) {
        let Some(grid) = self.grids.iter().find(|g| g.id == grid_id) else {
            return;
        };
        let step = (self.tile_size + self.spacing) * self.zoom;
        let grid_w = (grid.grid_cols as f32) * step;

        let target_x = if grid_w < viewport.width() - GRID_PAN_SIDE_PADDING {
            ((viewport.width() - grid_w) * 0.5) - (grid.col_offset as f32) * step
        } else {
            GRID_PAN_LEFT_MARGIN - (grid.col_offset as f32) * step
        };

        // Position the header bar cleanly below the top HUD
        let target_y =
            GRID_PAN_TOP_MARGIN + GRID_PAN_HEADER_SPACING - (grid.row_offset as f32) * step;

        self.pan = egui::vec2(target_x, target_y);
    }

    /// Handles pan dragging and zoom mouse wheel events.
    pub fn handle_input(&mut self, ui: &mut Ui, canvas_rect: Rect, response: &egui::Response) {
        if !self.initial_fit_done && canvas_rect.width() > 100.0 && canvas_rect.height() > 100.0 {
            self.fit_to_view(canvas_rect);
            self.initial_fit_done = true;
        }

        if let Some(grid_id) = self.pending_pan_to_grid.take() {
            self.pan_to_grid(grid_id, canvas_rect);
        }

        let pointer_pos_raw = ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()));
        let pointer_in_canvas = pointer_pos_raw.is_some_and(|p| canvas_rect.contains(p));

        let (plus_pressed, minus_pressed, fit, escape) = ui.input(|i| {
            let plus = i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals);
            let minus = i.key_pressed(egui::Key::Minus);
            let fit =
                i.key_pressed(egui::Key::F) || (i.modifiers.ctrl && i.key_pressed(egui::Key::Num0));
            let escape = i.key_pressed(egui::Key::Escape);
            (plus, minus, fit, escape)
        });

        let focused_id = ui.memory(|m| m.focused());
        let text_focused =
            focused_id.is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());

        let hovered_result_grid = pointer_pos_raw
            .filter(|&p| canvas_rect.contains(p))
            .and_then(|p| self.result_grid_at_pos(canvas_rect.min, p));

        // When '+' or '-' is pressed, in similarity view override find/text bars:
        // 1. If hovering over a result grid, always adjust that result grid's allowance.
        // 2. Otherwise (if not over a result grid), zoom the canvas unless actively typing in a non-empty text input.
        if plus_pressed {
            if let Some(grid_id) = hovered_result_grid {
                if self.adjust_grid_allowance(grid_id, CanvasGrid::ALLOWANCE_STEP) {
                    self.recompute_layout();
                    ui.ctx().request_repaint();
                }
            } else if !text_focused
                || self.search_query.trim().is_empty()
                || self.search_query == "+"
                || self.search_query == "="
            {
                let center = canvas_rect.center();
                let new_zoom = (self.zoom * KEYBOARD_ZOOM_STEP).clamp(MIN_ZOOM, MAX_ZOOM);
                let origin_offset = center - canvas_rect.min - self.pan;
                self.pan = center - canvas_rect.min - origin_offset * (new_zoom / self.zoom);
                self.zoom = new_zoom;
            }
        }
        if minus_pressed {
            if let Some(grid_id) = hovered_result_grid {
                if self.adjust_grid_allowance(grid_id, -CanvasGrid::ALLOWANCE_STEP) {
                    self.recompute_layout();
                    ui.ctx().request_repaint();
                }
            } else if !text_focused
                || self.search_query.trim().is_empty()
                || self.search_query == "-"
            {
                let center = canvas_rect.center();
                let new_zoom = (self.zoom / KEYBOARD_ZOOM_STEP).clamp(MIN_ZOOM, MAX_ZOOM);
                let origin_offset = center - canvas_rect.min - self.pan;
                self.pan = center - canvas_rect.min - origin_offset * (new_zoom / self.zoom);
                self.zoom = new_zoom;
            }
        }

        // Clean up accidental single +/-/= character in search query from overriding
        if self.search_query == "+" || self.search_query == "=" || self.search_query == "-" {
            self.search_query.clear();
        }

        if !text_focused {
            if fit {
                self.fit_to_view(canvas_rect);
            }
            if escape {
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                } else if self.grids.len() > 1 {
                    self.clear_all_searches();
                }
            }
        }

        let middle_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let pointer_delta = ui.input(|i| i.pointer.delta());

        if middle_down {
            if pointer_in_canvas || self.is_middle_panning {
                self.is_middle_panning = true;
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if pointer_delta != egui::Vec2::ZERO {
                    self.pan += pointer_delta;
                }
            }
        } else {
            self.is_middle_panning = false;
            if response.dragged() {
                self.pan += response.drag_delta();
            }
        }

        let pointer_pos = pointer_pos_raw.unwrap_or(canvas_rect.center());
        if canvas_rect.contains(pointer_pos) {
            let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
            if scroll_delta.abs() > 0.01 {
                let zoom_factor = (1.0 + scroll_delta * SCROLL_ZOOM_SENSITIVITY)
                    .clamp(MIN_SCROLL_ZOOM_FACTOR, MAX_SCROLL_ZOOM_FACTOR);
                let new_zoom = (self.zoom * zoom_factor).clamp(MIN_ZOOM, MAX_ZOOM);
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
