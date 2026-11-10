//! Background PCA, similarity scoring and grid packing from an owned immutable snapshot.
//! Dense feature buffers are shared, so snapshot capture never clones image embeddings.
use super::{
    canvas::{CanvasGrid, normalized_feature},
    embedding::{FeatureVector, project_features_to_2d},
    grid::{GridPlacement, grid_dimensions, snap_to_grid},
};
use crate::file_manager::filesystem::Entry;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
const GRID_COL_GAP: usize = 2;
const GRID_ROW_GAP: usize = 1;

pub(super) struct LayoutSnapshot {
    pub entries: std::sync::Arc<[Entry]>,
    pub hashes: HashMap<PathBuf, (FeatureVector, u64)>,
    pub grids: Vec<CanvasGrid>,
    pub base_placements: Vec<GridPlacement>,
    pub placements: Vec<GridPlacement>,
    pub grid_cols: usize,
    pub grid_rows: usize,
}

impl LayoutSnapshot {
    pub fn compute(mut self, cancelled: &AtomicBool) -> Self {
        if cancelled.load(Ordering::Relaxed) {
            return self;
        }
        self.refresh_searches(cancelled);
        self.project_base(cancelled);
        if !cancelled.load(Ordering::Relaxed) {
            self.pack_grids();
        }
        self
    }

    fn refresh_searches(&mut self, cancelled: &AtomicBool) {
        let n = self.entries.len();
        // Keep base grid indices synced with all entries
        if let Some(base) = self.grids.first_mut() {
            base.candidate_indices = (0..n).collect();
            base.indices = (0..n).collect();
        }

        // Reverse-image queries track upgrades from perceptual hashes to neural vectors.
        for grid in self.grids.iter_mut().skip(1) {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            if let Some(path) = &grid.reference_photo
                && let Some((feature, _)) = self.hashes.get(path)
            {
                grid.concept_embedding = Some(normalized_feature(feature));
            }
            grid.refresh_scores(&self.hashes, &self.entries);
            let threshold = grid.threshold;
            grid.refilter(&self.hashes, &self.entries, threshold);
        }
    }

    fn project_base(&mut self, cancelled: &AtomicBool) {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let n = self.entries.len();
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

            let coords_2d = project_features_to_2d(&features, cancelled);
            if !cancelled.load(Ordering::Relaxed) {
                self.base_placements = snap_to_grid(&coords_2d);
            }
        }
    }

    fn pack_grids(&mut self) {
        let n = self.entries.len();
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
}
