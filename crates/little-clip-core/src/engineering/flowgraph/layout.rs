use std::collections::{BTreeMap, VecDeque};
use std::ops::Bound::Excluded;

use super::{Direction, Graph};

/// Saved or derived canvas position of a block's origin.
pub type Position = [f32; 2];

/// Footprint of a block in full-size canvas coordinates.
pub const BLOCK_SIZE: Position = [178.0, 52.0];

const COLUMN: f32 = 200.0;
const ROW: f32 = 90.0;
const MAX_COORDINATE: f32 = 1_000_000.0;

/// Stable positions shared by compact and full topology views.
#[derive(Clone, Debug)]
pub struct Layout {
    pub positions: Vec<Position>,
    /// Minimum and maximum block origins.
    pub bounds: [Position; 2],
    pub saved_positions: usize,
}

pub(super) fn valid(position: Position) -> Option<Position> {
    position
        .iter()
        .all(|value| value.is_finite() && value.abs() <= MAX_COORDINATE)
        .then_some(position)
}

pub(super) fn make(graph: &Graph) -> Layout {
    let saved_positions = graph
        .blocks
        .iter()
        .filter(|block| block.position.is_some())
        .count();
    let mut positions = Vec::with_capacity(graph.blocks.len());
    if saved_positions == graph.blocks.len() {
        positions.extend(graph.blocks.iter().filter_map(|block| block.position));
    } else {
        let depth = layers(graph);
        let columns = depth.iter().copied().max().unwrap_or(0) + 1;
        let mut rows = vec![0usize; columns];
        let saved_bounds = bounds(graph.blocks.iter().filter_map(|block| block.position));
        let base = if saved_positions == 0 {
            [0.0, 0.0]
        } else {
            [saved_bounds[1][0] + COLUMN, saved_bounds[0][1]]
        };
        for (block, column) in graph.blocks.iter().zip(depth) {
            let row = rows[column];
            let [x, y] = base;
            let position = block.position.unwrap_or_else(|| {
                rows[column] += 1;
                [x + column as f32 * COLUMN, y + row as f32 * ROW]
            });
            positions.push(position);
        }
    }
    separate_blocks(&mut positions);
    Layout {
        bounds: bounds(positions.iter().copied()),
        positions,
        saved_positions,
    }
}

fn separate_blocks(positions: &mut [Position]) {
    // In top-to-bottom order, raise the occupied bottom edge over each block's
    // X interval. Each skyline breakpoint is inserted and removed at most once.
    const GAP: f32 = 8.0;
    let mut order = (0..positions.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| positions[a][1].total_cmp(&positions[b][1]));
    let mut edges = positions
        .iter()
        .flat_map(|&[x, _]| [x, x + BLOCK_SIZE[0] + GAP])
        .collect::<Vec<_>>();
    edges.sort_by(f32::total_cmp);
    edges.dedup_by(|a, b| a.total_cmp(b).is_eq());
    let mut bottom = BTreeMap::new();
    for index in order {
        let [x, y] = positions[index];
        let left = edges.binary_search_by(|edge| edge.total_cmp(&x)).unwrap();
        let right = edges
            .binary_search_by(|edge| edge.total_cmp(&(x + BLOCK_SIZE[0] + GAP)))
            .unwrap();
        let right_y = skyline_at(&bottom, right);
        let mut next_y = y.max(skyline_at(&bottom, left));
        let inside = bottom
            .range((Excluded(left), Excluded(right)))
            .map(|(&key, &edge)| {
                next_y = next_y.max(edge);
                key
            })
            .collect::<Vec<_>>();
        for key in inside {
            bottom.remove(&key);
        }
        positions[index][1] = next_y;
        bottom.insert(left, next_y + BLOCK_SIZE[1] + GAP);
        bottom.insert(right, right_y);
    }
}

fn skyline_at(edges: &BTreeMap<usize, f32>, at: usize) -> f32 {
    edges
        .range(..=at)
        .next_back()
        .map_or(f32::NEG_INFINITY, |(_, &y)| y)
}

fn bounds(positions: impl Iterator<Item = Position>) -> [Position; 2] {
    let mut positions = positions;
    let Some(first) = positions.next() else {
        return [[0.0, 0.0]; 2];
    };
    positions.fold(
        [first, first],
        |[[min_x, min_y], [max_x, max_y]], [x, y]| {
            [[min_x.min(x), min_y.min(y)], [max_x.max(x), max_y.max(y)]]
        },
    )
}

fn layers(graph: &Graph) -> Vec<usize> {
    let count = graph.blocks.len();
    let mut depth = vec![0; count];
    let mut incoming = (0..count)
        .map(|block| graph.adjacent(block, Direction::Upstream).len())
        .collect::<Vec<_>>();
    let mut queue = incoming
        .iter()
        .enumerate()
        .filter_map(|(block, count)| (*count == 0).then_some(block))
        .collect::<VecDeque<_>>();
    while let Some(block) = queue.pop_front() {
        for &edge in graph.adjacent(block, Direction::Downstream) {
            let next = graph.connections[edge].sink.block;
            depth[next] = depth[next].max(depth[block] + 1);
            incoming[next] -= 1;
            if incoming[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    // Feedback paths stay visible even when a topological ordering is impossible.
    let after = depth.iter().copied().max().unwrap_or(0) + 1;
    for (block, column) in depth.iter_mut().enumerate() {
        if incoming[block] > 0 {
            *column = after + block % 6;
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_origins_are_separated_and_disjoint_origins_stay_put() {
        let mut positions = [
            [0.0, 0.0],
            [140.0, 0.0],
            [250.0, 15.0],
            [500.0, 10.0],
            [0.0, 200.0],
        ];
        separate_blocks(&mut positions);
        assert_eq!(
            positions,
            [
                [0.0, 0.0],
                [140.0, 60.0],
                [250.0, 120.0],
                [500.0, 10.0],
                [0.0, 200.0]
            ]
        );
    }
}
