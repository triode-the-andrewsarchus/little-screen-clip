//! Streaming frequency intervals meeting a magnitude cutoff.

use super::{Complex64, TracePoint};
use std::{cmp::Ordering, collections::BinaryHeap};

const RETAINED_BANDS: usize = 4096;

pub fn magnitude_db(value: Complex64) -> f64 {
    20.0 * value.norm().log10()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThresholdDirection {
    AtLeast,
    AtMost,
}

impl ThresholdDirection {
    fn matches(self, level: f64, threshold: f64) -> bool {
        match self {
            Self::AtLeast => level >= threshold,
            Self::AtMost => level <= threshold,
        }
    }

    fn prefers(self, level: f64, previous: f64) -> bool {
        match self {
            Self::AtLeast => level > previous,
            Self::AtMost => level < previous,
        }
    }
}

/// A threshold crossing or the end of an observed region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandEdge {
    pub frequency_hz: f64,
    /// The data ended before it showed a crossing.
    pub limited: bool,
    /// Exact records that establish the crossing or observed boundary.
    pub rows: [TracePoint; 2],
}

/// One contiguous region. The extreme is an actual sampled point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThresholdBand {
    pub edges: [BandEdge; 2],
    pub extreme: TracePoint,
    pub extreme_db: f64,
}

impl ThresholdBand {
    pub fn width_hz(self) -> f64 {
        self.edges[1].frequency_hz - self.edges[0].frequency_hz
    }

    pub fn band_hz(self) -> [f64; 2] {
        self.edges.map(|edge| edge.frequency_hz)
    }
}

pub struct BandSearch {
    pub threshold_db: f64,
    pub bands: Vec<ThresholdBand>,
    /// A dense result retains the widest regions and reports the total count.
    pub found: usize,
}

pub struct BandFinder {
    threshold: f64,
    direction: ThresholdDirection,
    previous: Option<Sample>,
    pending: Option<Pending>,
    bands: BinaryHeap<RankedBand>,
    found: usize,
}

#[derive(Clone, Copy)]
struct Sample {
    point: TracePoint,
    level: f64,
}

struct Pending {
    first: BandEdge,
    extreme: Sample,
}

impl BandFinder {
    pub fn new(threshold: f64, direction: ThresholdDirection) -> Option<Self> {
        threshold.is_finite().then(|| Self {
            threshold,
            direction,
            previous: None,
            pending: None,
            bands: BinaryHeap::new(),
            found: 0,
        })
    }

    pub fn push(&mut self, point: TracePoint) {
        let level = magnitude_db(point.value);
        if !point.frequency_hz.is_finite() || level.is_nan() || level == f64::INFINITY {
            self.end_observation();
            return;
        }
        let next = Sample { point, level };
        if self.previous.is_some_and(|last| {
            last.point.index.checked_add(1) != Some(point.index)
                || last.point.frequency_hz >= point.frequency_hz
        }) {
            self.end_observation();
        }
        if self.direction.matches(level, self.threshold) {
            if let Some(open) = &mut self.pending {
                if self.direction.prefers(level, open.extreme.level) {
                    open.extreme = next;
                }
            } else {
                let first = self.edge(self.previous, next);
                self.pending = Some(Pending {
                    first,
                    extreme: next,
                });
            }
        } else if self.pending.is_some() {
            let end = self.edge(self.previous, next);
            self.close(end);
        }
        self.previous = Some(next);
    }

    fn edge(&self, previous: Option<Sample>, next: Sample) -> BandEdge {
        previous.map_or_else(
            || next.limit(),
            |last| BandEdge {
                frequency_hz: crossing(last, next, self.threshold),
                limited: false,
                rows: [last.point, next.point],
            },
        )
    }

    fn close(&mut self, end: BandEdge) {
        let Some(open) = self.pending.take() else {
            return;
        };
        let band = RankedBand(ThresholdBand {
            edges: [open.first, end],
            extreme: open.extreme.point,
            extreme_db: open.extreme.level,
        });
        self.found += 1;
        if self.bands.len() < RETAINED_BANDS {
            self.bands.push(band);
        } else if let Some(mut narrowest) = self.bands.peek_mut()
            && band < *narrowest
        {
            *narrowest = band;
        }
    }

    fn end_observation(&mut self) {
        if let Some(last) = self.previous.take() {
            self.close(last.limit());
        }
    }

    pub fn finish(mut self) -> BandSearch {
        self.end_observation();
        let mut bands: Vec<_> = self.bands.into_iter().map(|band| band.0).collect();
        bands.sort_unstable_by(|a, b| a.edges[0].frequency_hz.total_cmp(&b.edges[0].frequency_hz));
        BandSearch {
            threshold_db: self.threshold,
            bands,
            found: self.found,
        }
    }
}

impl Sample {
    fn limit(self) -> BandEdge {
        BandEdge {
            frequency_hz: self.point.frequency_hz,
            limited: true,
            rows: [self.point; 2],
        }
    }
}

fn crossing(a: Sample, b: Sample, threshold: f64) -> f64 {
    let fraction = if a.level.is_finite() && b.level.is_finite() {
        (threshold - a.level) / (b.level - a.level)
    } else {
        // Zero magnitude has no finite dB coordinate.
        let left = a.point.value.norm();
        let right = b.point.value.norm();
        (10_f64.powf(threshold / 20.0) - left) / (right - left)
    };
    (b.point.frequency_hz - a.point.frequency_hz)
        .mul_add(fraction.clamp(0.0, 1.0), a.point.frequency_hz)
}

struct RankedBand(ThresholdBand);

impl Ord for RankedBand {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .width_hz()
            .total_cmp(&self.0.width_hz())
            .then_with(|| {
                self.0.edges[0]
                    .frequency_hz
                    .total_cmp(&other.0.edges[0].frequency_hz)
            })
    }
}

impl PartialOrd for RankedBand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for RankedBand {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RankedBand {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_both_edges_and_keeps_observed_limits_distinct() {
        let mut finder = BandFinder::new(-10.0, ThresholdDirection::AtMost).unwrap();
        for (index, db) in [-5.0, -15.0, -20.0, -5.0, -15.0, -15.0]
            .into_iter()
            .enumerate()
        {
            finder.push(point(index, 100.0 * index as f64, db));
        }
        let result = finder.finish();
        assert_eq!(result.found, 2);
        let first = result.bands[0];
        for (actual, target) in first.band_hz().into_iter().zip([50.0, 200.0 + 200.0 / 3.0]) {
            assert!((actual - target).abs() < 1e-12);
        }
        assert!(!first.edges[0].limited && !first.edges[1].limited);
        assert_eq!(first.extreme.index, 2);
        let second = result.bands[1];
        assert!((second.edges[0].frequency_hz - 350.0).abs() < 1e-12);
        assert_eq!(second.edges[1].frequency_hz, 500.0);
        assert!(second.edges[1].limited);
    }

    #[test]
    fn does_not_bridge_a_missing_source_row_or_divide_by_infinite_db() {
        let mut finder = BandFinder::new(-20.0, ThresholdDirection::AtLeast).unwrap();
        for (index, hz, level) in [
            (0, 0.0, f64::NEG_INFINITY),
            (1, 100.0, 0.0),
            (3, 300.0, 0.0),
        ] {
            finder.push(point(index, hz, level));
        }
        let result = finder.finish();
        assert_eq!(result.found, 2);
        assert_eq!(result.bands[0].band_hz(), [10.0, 100.0]);
        assert_eq!(result.bands[1].band_hz(), [300.0, 300.0]);
        assert!(result.bands[0].edges[1].limited && result.bands[1].edges[0].limited);
    }

    fn point(index: usize, hz: f64, db: f64) -> TracePoint {
        TracePoint {
            index: index as u64,
            frequency_hz: hz,
            value: Complex64::new(10_f64.powf(db / 20.0), 0.0),
        }
    }
}
