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
    siglip::{SIGLIP_BATCH_SIZE, SiglipEmbedder, SiglipTextEmbedder},
};
use crate::file_manager::filesystem::{Directory, Entry};

pub const DEFAULT_TILE_SIZE: f32 = 128.0;
pub const TILE_SPACING: f32 = 4.0;

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
}

impl CanvasGrid {
    pub fn refilter(
        &mut self,
        hashes: &HashMap<PathBuf, (FeatureVector, u64)>,
        entries: &[Entry],
        threshold: f32,
    ) {
        let Some(ref concept_vec) = self.concept_embedding else {
            return;
        };

        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(self.candidate_indices.len());
        for &idx in &self.candidate_indices {
            let Some(entry) = entries.get(idx) else {
                continue;
            };
            let score = if let Some((FeatureVector::Dense(img_vec), _)) = hashes.get(&entry.path) {
                img_vec
                    .iter()
                    .zip(concept_vec.iter())
                    .map(|(&a, &b)| a * b)
                    .sum::<f32>()
            } else {
                -1.0
            };
            scored.push((idx, score));
        }

        if self.is_inverted {
            // Inverted search: match items where score < threshold (and valid score >= 0.0)
            let mut matches: Vec<(usize, f32)> = scored
                .iter()
                .filter(|&&(_, s)| s >= 0.0 && s < threshold)
                .copied()
                .collect();
            // Sort ascending: lowest score (least similar to inverted concept) comes first
            matches.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            self.indices = matches.into_iter().map(|(idx, _)| idx).collect();
        } else {
            // Normal search: match items where score >= threshold
            let mut matches: Vec<(usize, f32)> = scored
                .iter()
                .filter(|&&(_, s)| s >= threshold)
                .copied()
                .collect();
            // Sort descending: highest score (most similar to concept) comes first
            matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            self.indices = matches.into_iter().map(|(idx, _)| idx).collect();
        }
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
    hash_job: Option<HashJob>,
    search_job: Option<PendingJob<Option<Vec<f32>>>>,
    initial_fit_done: bool,
    pub hashing_progress: (usize, usize), // (completed, total)
    pub search_query: String,
    pub pending_search: Option<PendingSearchMeta>,
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
            hashing_progress: (0, 0),
            search_query: String::new(),
            pending_search: None,
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

    pub fn is_scanning(&self) -> bool {
        self.scan_job.is_some()
    }

    pub fn is_hashing(&self) -> bool {
        self.hash_job.is_some()
    }

    pub fn is_searching(&self) -> bool {
        self.search_job.is_some()
    }

    /// Triggers an asynchronous concept search by embedding the search query.
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
        if !self.grids.iter().any(|g| g.selected_for_search) {
            if let Some(base) = self.grids.first_mut() {
                base.selected_for_search = true;
            }
        }
        self.recompute_layout();
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
        };
        grid.refilter(&self.hashes, &self.entries, self.similarity_threshold);
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
        }];
        self.next_grid_id = 1;

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

        let is_siglip_available =
            crate::onnx_runtime::is_model_cached(crate::onnx_runtime::SIGLIP2_GIANT_MODEL_NAME);

        let missing: Vec<(PathBuf, u64)> = self
            .entries
            .iter()
            .filter(|e| {
                self.hashes.get(&e.path).map_or(true, |(feat, rev)| {
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
            let mut siglip_embedder = SiglipEmbedder::try_new().ok();

            for chunk in missing.chunks(SIGLIP_BATCH_SIZE) {
                if c.load(Ordering::Relaxed) {
                    break;
                }
                let mut chunk_results = Vec::with_capacity(chunk.len());
                let mut used_siglip = false;

                if let Some(embedder) = siglip_embedder.as_mut() {
                    let chunk_paths: Vec<PathBuf> = chunk.iter().map(|(p, _)| p.clone()).collect();
                    if let Ok(embeddings) = embedder.embed_batch(&chunk_paths, &c) {
                        for ((path, rev), feat) in chunk.iter().zip(embeddings.into_iter()) {
                            chunk_results.push((path.clone(), *rev, feat));
                        }
                        used_siglip = true;
                    }
                }

                if !used_siglip {
                    for (path, rev) in chunk {
                        if c.load(Ordering::Relaxed) {
                            break;
                        }
                        if let Some(hash) = compute_dhash_from_path(path, &c) {
                            chunk_results.push((path.clone(), *rev, FeatureVector::DHash(hash)));
                        }
                    }
                }

                if !chunk_results.is_empty() && batch_tx.send(chunk_results).is_err() {
                    break;
                }
            }
        });
        self.hash_job = Some(HashJob {
            ticket,
            cancelled,
            batch_rx,
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

        if let Some(job) = &self.search_job {
            if let Ok(res) = job.receiver.try_recv() {
                self.search_job = None;
                if let Ok(Some(emb)) = res {
                    if let Some(meta) = self.pending_search.take() {
                        // Deselect other grids so consecutive searches chain to this new grid by default
                        for g in &mut self.grids {
                            g.selected_for_search = false;
                        }

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
                        };
                        self.next_grid_id += 1;
                        new_grid.refilter(&self.hashes, &self.entries, self.similarity_threshold);
                        let new_id = new_grid.id;
                        self.grids.push(new_grid);
                        self.recompute_layout();
                        self.pending_pan_to_grid = Some(new_id);
                    }
                }
                ctx.request_repaint();
            }
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
            });
        }

        // Keep base grid indices synced with all entries
        if let Some(base) = self.grids.first_mut() {
            base.candidate_indices = (0..n).collect();
            base.indices = (0..n).collect();
        }

        // Refilter any search grids with current threshold
        let threshold = self.similarity_threshold;
        for grid in self.grids.iter_mut().skip(1) {
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

        const COL_GAP: usize = 2;
        const ROW_GAP: usize = 1;

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
                col_start_x = c + COL_GAP;
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
                    col_used_height + ROW_GAP + eff_r
                } else {
                    eff_r
                };

                if col_used_height > 0 && needed_height > max_column_height {
                    // Start a new column to the right
                    col_start_x += col_width + COL_GAP;
                    col_used_height = 0;
                    col_width = 0;
                }

                let row_offset = if col_used_height > 0 {
                    col_used_height + ROW_GAP
                } else {
                    0
                };

                grid.col_offset = col_start_x;
                grid.row_offset = row_offset;

                col_used_height = row_offset + eff_r;
                col_width = col_width.max(eff_c);

                max_overall_rows = max_overall_rows.max(col_used_height);
                max_overall_cols = max_overall_cols.max(col_start_x + col_width);

                for (rank, &entry_idx) in grid.indices.iter().enumerate() {
                    new_placements.push(GridPlacement {
                        index: entry_idx,
                        col: col_start_x + if c > 0 { rank % c } else { 0 },
                        row: row_offset + if c > 0 { rank / c } else { 0 },
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
        let fit_zoom = scale_x.min(scale_y).clamp(0.05, 2.5);
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

        let target_x = if grid_w < viewport.width() - 80.0 {
            ((viewport.width() - grid_w) * 0.5) - (grid.col_offset as f32) * step
        } else {
            40.0 - (grid.col_offset as f32) * step
        };

        // Position the header bar cleanly below the top HUD (at ~76px from canvas top)
        let top_offset = 76.0;
        let header_spacing = 34.0;
        let target_y = top_offset + header_spacing - (grid.row_offset as f32) * step;

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
