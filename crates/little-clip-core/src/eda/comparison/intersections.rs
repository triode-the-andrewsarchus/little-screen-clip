//! Bound possible edge crossings before handing a batch to the Boolean engine.

use super::geometry::Coverage;
use crate::eda::check_cancelled;
use std::{cmp::Reverse, collections::BinaryHeap, sync::atomic::AtomicBool};

const MAX_CANDIDATE_PAIRS: usize = 1_000_000;

#[derive(Clone, Copy)]
struct Edge {
    left: i32,
    right: i32,
    low: i32,
    high: i32,
}

pub(super) fn check(
    a: &Coverage,
    b: &Coverage,
    vertices: usize,
    padding: i32,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if vertices <= 1024 {
        return Ok(());
    }
    let mut edges = Vec::with_capacity(vertices);
    for (index, contour) in a.iter().chain(b).flatten().enumerate() {
        if index.is_multiple_of(128) {
            check_cancelled(cancel, "Artwork comparison cancelled")?;
        }
        edges.extend(
            contour
                .iter()
                .zip(contour.iter().cycle().skip(1))
                .map(|(a, b)| Edge {
                    left: a.x.min(b.x) - padding,
                    right: a.x.max(b.x) + padding,
                    low: a.y.min(b.y) - padding,
                    high: a.y.max(b.y) + padding,
                }),
        );
    }
    edges.sort_unstable_by_key(|edge| edge.left);
    let mut ys: Vec<i32> = edges
        .iter()
        .flat_map(|edge| [edge.low, edge.high])
        .collect();
    ys.sort_unstable();
    ys.dedup();

    let mut ends = BinaryHeap::<Reverse<(i32, usize, usize)>>::new();
    let mut lows = PrefixCounts::new(ys.len());
    let mut highs = PrefixCounts::new(ys.len());
    let mut candidates = 0;
    for (index, edge) in edges.into_iter().enumerate() {
        if index.is_multiple_of(512) {
            check_cancelled(cancel, "Artwork comparison cancelled")?;
        }
        while let Some(&Reverse((x, lo, hi))) = ends.peek() {
            if x >= edge.left {
                break;
            }
            ends.pop();
            lows.add(lo, -1);
            highs.add(hi, -1);
        }
        let lo = ys.partition_point(|y| *y < edge.low);
        let hi = ys.partition_point(|y| *y < edge.high);
        // Active X intervals already overlap. Subtract Y intervals ending
        // below this edge from those beginning at or below its upper edge.
        candidates += lows.prefix(hi + 1) - highs.prefix(lo);
        if candidates > MAX_CANDIDATE_PAIRS {
            return Err(
                "Artwork has too many potentially intersecting edges for geometric comparison"
                    .into(),
            );
        }
        lows.add(lo, 1);
        highs.add(hi, 1);
        ends.push(Reverse((edge.right, lo, hi)));
    }
    Ok(())
}

/// Fenwick tree over compressed Y coordinates.
struct PrefixCounts(Vec<i32>);

impl PrefixCounts {
    fn new(size: usize) -> Self {
        Self(vec![0; size + 1])
    }

    fn add(&mut self, index: usize, delta: i32) {
        let mut index = index + 1;
        while index < self.0.len() {
            self.0[index] += delta;
            index += index & index.wrapping_neg();
        }
    }

    fn prefix(&self, mut end: usize) -> usize {
        let mut sum = 0;
        while end != 0 {
            sum += self.0[end];
            end &= end - 1;
        }
        sum as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use i_overlay::i_float::int::point::IntPoint;

    #[test]
    fn edge_budget_rejects_dense_crossings_and_accepts_separated_geometry() {
        let polygon = |x| {
            vec![vec![
                IntPoint::new(x, 0),
                IntPoint::new(x + 1000, 1000),
                IntPoint::new(x + 1001, 1000),
            ]]
        };
        let overlaps: Coverage = (0..700).map(polygon).collect();
        let apart: Coverage = (0..700).map(|x| polygon(x * 2000)).collect();
        let cancel = AtomicBool::new(false);
        assert!(check(&overlaps, &vec![], 2100, 0, &cancel).is_err());
        assert!(check(&apart, &vec![], 2100, 0, &cancel).is_ok());
    }
}
