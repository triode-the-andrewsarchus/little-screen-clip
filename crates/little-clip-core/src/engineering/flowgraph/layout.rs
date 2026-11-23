use std::collections::VecDeque;

use super::{Direction, Graph};

/// Saved or derived canvas position of a block's origin.
pub type Position = [f32; 2];

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
    Layout {
        bounds: bounds(positions.iter().copied()),
        positions,
        saved_positions,
    }
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
