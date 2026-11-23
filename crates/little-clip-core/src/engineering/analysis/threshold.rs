//! Convert analog samples into logic states on the capture's original sample grid.

use crate::engineering::logic::{Edge, Level, Trace};
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

/// Interpretation of voltages between the two input thresholds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MiddleBand {
    /// A voltage in the band does not establish a logic level.
    #[default]
    Unknown,
    /// Hold the most recent known state until the opposite threshold is crossed.
    Hold,
    /// Mark the user-defined band with the logic viewer's floating state.
    Floating,
}

/// Fixed cutoffs or cutoffs that follow the signal range of the analyzed window.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Cutoffs {
    Fixed {
        low: f32,
        high: f32,
    },
    Adaptive {
        low_fraction: f32,
        high_fraction: f32,
    },
}

impl Default for Cutoffs {
    fn default() -> Self {
        Self::Adaptive {
            low_fraction: 0.4,
            high_fraction: 0.6,
        }
    }
}

/// Rules for making a digital lane from one analog waveform.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Threshold {
    pub cutoffs: Cutoffs,
    pub middle: MiddleBand,
    pub invert: bool,
}

impl Threshold {
    /// Validate the numeric values before using a stored or edited threshold.
    pub fn valid(self) -> bool {
        match self.cutoffs {
            Cutoffs::Fixed { low, high } => low.is_finite() && high.is_finite() && low <= high,
            Cutoffs::Adaptive {
                low_fraction,
                high_fraction,
            } => 0.0 <= low_fraction && low_fraction <= high_fraction && high_fraction <= 1.0,
        }
    }

    /// Convert a contiguous sample window. If the result exceeds the edge
    /// budget, the trace describes the prefix through the next unreported edge.
    ///
    /// # Errors
    /// Returns an error if the threshold is invalid or the work is cancelled.
    pub fn trace(
        self,
        samples: &[f32],
        first: f64,
        step: f64,
        visible: &Range<f64>,
        edge_limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        if !self.valid() {
            return Err("Low threshold must be at or below the high threshold".into());
        }
        let bounds = self.bounds(samples);
        let mut trace = Trace {
            initial: None,
            edges: Vec::new(),
            inspected_until: visible.start,
        };
        let mut previous = None;
        for (index, &sample) in samples.iter().enumerate() {
            if index % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                return Err("Thresholding cancelled".into());
            }
            let time = (index as f64).mul_add(step, first);
            if time >= visible.end {
                break;
            }
            let level = self.classify(sample, bounds, previous);
            if time <= visible.start {
                trace.initial = Some(level);
            } else if previous != Some(level) {
                if trace.edges.len() >= edge_limit {
                    trace.inspected_until = time;
                    return Ok(trace);
                }
                trace.edges.push(Edge {
                    time_seconds: time,
                    level,
                });
            }
            previous = Some(level);
        }
        trace.inspected_until = (samples.len() as f64).mul_add(step, first).min(visible.end);
        Ok(trace)
    }

    fn bounds(self, samples: &[f32]) -> Option<(f32, f32)> {
        let (low_fraction, high_fraction) = match self.cutoffs {
            Cutoffs::Fixed { low, high } => return Some((low, high)),
            Cutoffs::Adaptive {
                low_fraction,
                high_fraction,
            } => (low_fraction, high_fraction),
        };
        let extrema = samples
            .iter()
            .copied()
            .filter(|sample| sample.is_finite())
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), value| {
                (min.min(value), max.max(value))
            });
        let width = f64::from(extrema.1) - f64::from(extrema.0);
        if !(width.is_finite() && width > 0.0) {
            return None;
        }
        let at = |fraction| width.mul_add(f64::from(fraction), f64::from(extrema.0)) as f32;
        Some((at(low_fraction), at(high_fraction)))
    }

    fn classify(self, sample: f32, bounds: Option<(f32, f32)>, previous: Option<Level>) -> Level {
        let Some((low, high)) = bounds.filter(|_| sample.is_finite()) else {
            return Level::Unknown;
        };
        if sample <= low {
            return if self.invert { Level::High } else { Level::Low };
        }
        if sample >= high {
            return if self.invert { Level::Low } else { Level::High };
        }
        match self.middle {
            MiddleBand::Unknown => Level::Unknown,
            MiddleBand::Hold => previous.unwrap_or(Level::Unknown),
            MiddleBand::Floating => Level::HighImpedance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_rails_invert_and_keep_floating_distinct_from_missing_data() {
        let rule = Threshold {
            middle: MiddleBand::Floating,
            invert: true,
            ..Default::default()
        };
        let trace = rule
            .trace(
                &[10.0, 15.0, 20.0, f32::NAN],
                0.0,
                1.0,
                &(0.0..4.0),
                10,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(trace.initial, Some(Level::High));
        assert_eq!(
            trace
                .edges
                .iter()
                .map(|edge| edge.level)
                .collect::<Vec<_>>(),
            [Level::HighImpedance, Level::Low, Level::Unknown]
        );
    }

    #[test]
    fn hold_ignores_between_cutoff_noise_and_stops_at_the_next_omitted_edge() {
        let rule = Threshold {
            cutoffs: Cutoffs::Fixed {
                low: 1.0,
                high: 2.0,
            },
            middle: MiddleBand::Hold,
            invert: false,
        };
        let trace = rule
            .trace(
                &[0.0, 1.5, 2.2, 1.5, 0.1],
                0.0,
                1.0,
                &(0.0..5.0),
                1,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            trace.edges,
            [Edge {
                time_seconds: 2.0,
                level: Level::High
            }]
        );
        assert_eq!(trace.inspected_until, 4.0);
    }
}
