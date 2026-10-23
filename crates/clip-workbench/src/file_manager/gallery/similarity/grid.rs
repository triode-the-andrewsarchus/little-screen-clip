//! Discrete 2D grid snapping via recursive median bipartition and local relaxation.

const REFINE_PASSES: usize = 2;
const SCALE_EPSILON: f32 = 1e-5;
const COST_IMPROVEMENT_EPSILON: f32 = 1e-4;

/// A placed tile in the discrete 2D grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridPlacement {
    pub index: usize,
    pub col: usize,
    pub row: usize,
}

/// Computes the compact grid dimensions (columns, rows) for `n` items.
pub fn grid_dimensions(n: usize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = (n + cols - 1) / cols;
    (cols.max(1), rows.max(1))
}

/// Assigns `n` 2D points to discrete `(col, row)` grid cells with no overlaps and no holes.
///
/// Returns a vector of `GridPlacement` corresponding to each input point index.
pub fn snap_to_grid(points: &[(f32, f32)]) -> Vec<GridPlacement> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![GridPlacement {
            index: 0,
            col: 0,
            row: 0,
        }];
    }

    let (cols, rows) = grid_dimensions(n);

    // Collect all valid grid cell slots (row-major, exactly N cells)
    let mut valid_cells = Vec::with_capacity(n);
    for r in 0..rows {
        for c in 0..cols {
            if valid_cells.len() < n {
                valid_cells.push((c, r));
            }
        }
    }

    // Indices into `points`
    let mut item_indices: Vec<usize> = (0..n).collect();
    let mut assignments = vec![(0usize, 0usize); n];

    // Recursive bipartition
    partition(points, &mut item_indices, &valid_cells, &mut assignments);

    // Local 2-opt refinement pass to optimize metric smoothness
    refine_grid(points, &mut assignments, cols, rows);

    assignments
        .into_iter()
        .enumerate()
        .map(|(index, (col, row))| GridPlacement { index, col, row })
        .collect()
}

fn partition(
    points: &[(f32, f32)],
    items: &mut [usize],
    cells: &[(usize, usize)],
    assignments: &mut [(usize, usize)],
) {
    let count = items.len();
    if count == 0 || cells.is_empty() {
        return;
    }
    if count == 1 {
        assignments[items[0]] = cells[0];
        return;
    }

    // Determine bounding box of current cells
    let min_c = cells.iter().map(|&(c, _)| c).min().unwrap_or(0);
    let max_c = cells.iter().map(|&(c, _)| c).max().unwrap_or(0);
    let min_r = cells.iter().map(|&(_, r)| r).min().unwrap_or(0);
    let max_r = cells.iter().map(|&(_, r)| r).max().unwrap_or(0);

    let span_c = max_c - min_c + 1;
    let span_r = max_r - min_r + 1;

    if span_c >= span_r {
        // Split horizontally along column
        let mid_c = min_c + span_c / 2;
        let mut left_cells = Vec::new();
        let mut right_cells = Vec::new();
        for &cell in cells {
            if cell.0 < mid_c {
                left_cells.push(cell);
            } else {
                right_cells.push(cell);
            }
        }
        if left_cells.is_empty() || right_cells.is_empty() {
            // Cannot split further along column; fallback split in half
            let mid = count / 2;
            let (left_c, right_c) = cells.split_at(mid);
            items.sort_by(|&a, &b| points[a].0.total_cmp(&points[b].0));
            let (left_items, right_items) = items.split_at_mut(mid);
            partition(points, left_items, left_c, assignments);
            partition(points, right_items, right_c, assignments);
            return;
        }

        let k = left_cells.len().min(count);
        // Sort items by continuous X coordinate (tie-break by Y)
        items.sort_by(|&a, &b| {
            points[a]
                .0
                .total_cmp(&points[b].0)
                .then_with(|| points[a].1.total_cmp(&points[b].1))
        });
        let (left_items, right_items) = items.split_at_mut(k);
        partition(points, left_items, &left_cells, assignments);
        partition(points, right_items, &right_cells, assignments);
    } else {
        // Split vertically along row
        let mid_r = min_r + span_r / 2;
        let mut top_cells = Vec::new();
        let mut bottom_cells = Vec::new();
        for &cell in cells {
            if cell.1 < mid_r {
                top_cells.push(cell);
            } else {
                bottom_cells.push(cell);
            }
        }
        if top_cells.is_empty() || bottom_cells.is_empty() {
            let mid = count / 2;
            let (top_c, bottom_c) = cells.split_at(mid);
            items.sort_by(|&a, &b| points[a].1.total_cmp(&points[b].1));
            let (top_items, bottom_items) = items.split_at_mut(mid);
            partition(points, top_items, top_c, assignments);
            partition(points, bottom_items, bottom_c, assignments);
            return;
        }

        let k = top_cells.len().min(count);
        // Sort items by continuous Y coordinate (tie-break by X)
        items.sort_by(|&a, &b| {
            points[a]
                .1
                .total_cmp(&points[b].1)
                .then_with(|| points[a].0.total_cmp(&points[b].0))
        });
        let (top_items, bottom_items) = items.split_at_mut(k);
        partition(points, top_items, &top_cells, assignments);
        partition(points, bottom_items, &bottom_cells, assignments);
    }
}

/// Refines grid assignments using iterative local swaps to minimize continuous distance distortion.
fn refine_grid(
    points: &[(f32, f32)],
    assignments: &mut [(usize, usize)],
    cols: usize,
    rows: usize,
) {
    if points.len() <= 2 {
        return;
    }

    // Normalize continuous points to the grid coordinate space [0, cols-1] x [0, rows-1]
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &(x, y) in points {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let scale_x = if (max_x - min_x).abs() > SCALE_EPSILON {
        (cols.saturating_sub(1) as f32) / (max_x - min_x)
    } else {
        1.0
    };
    let scale_y = if (max_y - min_y).abs() > SCALE_EPSILON {
        (rows.saturating_sub(1) as f32) / (max_y - min_y)
    } else {
        1.0
    };

    let target_coords: Vec<(f32, f32)> = points
        .iter()
        .map(|&(x, y)| ((x - min_x) * scale_x, (y - min_y) * scale_y))
        .collect();

    // Map grid slot -> item index
    let mut grid_to_item = std::collections::HashMap::new();
    for (item, &slot) in assignments.iter().enumerate() {
        grid_to_item.insert(slot, item);
    }

    // Adjacent swap optimization passes
    for _ in 0..REFINE_PASSES {
        let mut improved = false;
        for item_a in 0..points.len() {
            let (ca, ra) = assignments[item_a];
            let neighbors = [
                (ca + 1, ra),
                (ca.saturating_sub(1), ra),
                (ca, ra + 1),
                (ca, ra.saturating_sub(1)),
            ];

            for &(cb, rb) in &neighbors {
                if cb >= cols || rb >= rows || (ca == cb && ra == rb) {
                    continue;
                }
                if let Some(&item_b) = grid_to_item.get(&(cb, rb)) {
                    if item_a == item_b {
                        continue;
                    }
                    // Current cost
                    let (ta_x, ta_y) = target_coords[item_a];
                    let (tb_x, tb_y) = target_coords[item_b];

                    let d_curr_a = (ta_x - ca as f32).powi(2) + (ta_y - ra as f32).powi(2);
                    let d_curr_b = (tb_x - cb as f32).powi(2) + (tb_y - rb as f32).powi(2);
                    let cost_curr = d_curr_a + d_curr_b;

                    // Swapped cost
                    let d_swap_a = (ta_x - cb as f32).powi(2) + (ta_y - rb as f32).powi(2);
                    let d_swap_b = (tb_x - ca as f32).powi(2) + (tb_y - ra as f32).powi(2);
                    let cost_swap = d_swap_a + d_swap_b;

                    if cost_swap + COST_IMPROVEMENT_EPSILON < cost_curr {
                        assignments[item_a] = (cb, rb);
                        assignments[item_b] = (ca, ra);
                        grid_to_item.insert((cb, rb), item_a);
                        grid_to_item.insert((ca, ra), item_b);
                        improved = true;
                    }
                }
            }
        }
        if !improved {
            break;
        }
    }
}
