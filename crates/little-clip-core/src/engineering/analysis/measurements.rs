//! Duration-weighted statistics on an aligned sample grid.

use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

const BANDS: usize = 384;

/// Synchronized values and an optional digital condition on their common grid.
pub struct MeasurementGrid<'a> {
    pub start: f64,
    pub step: f64,
    pub signal: &'a [f32],
    pub condition: Option<&'a [f32]>,
}

/// Contributions from one condition state.
#[derive(Clone, Copy, Debug)]
pub struct Moments {
    /// Time during which the condition had this value.
    pub state_seconds: f64,
    /// Time covered by finite signal samples while in this state.
    pub measured_seconds: f64,
    integral: f64,
    squares: f64,
    min: f64,
    max: f64,
}

impl Default for Moments {
    fn default() -> Self {
        Self {
            state_seconds: 0.0,
            measured_seconds: 0.0,
            integral: 0.0,
            squares: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }
}

impl Moments {
    fn contribute(&mut self, seconds: f64, value: f64) {
        self.measured_seconds += seconds;
        self.integral = value.mul_add(seconds, self.integral);
        self.squares = (value * value).mul_add(seconds, self.squares);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    pub fn mean(self) -> Option<f64> {
        self.integral()
            .map(|integral| integral / self.measured_seconds)
    }

    pub fn rms(self) -> Option<f64> {
        (self.measured_seconds > 0.0).then(|| (self.squares / self.measured_seconds).sqrt())
    }

    pub fn extent(self) -> Option<Range<f64>> {
        (self.measured_seconds > 0.0).then_some(self.min..self.max)
    }

    pub fn integral(self) -> Option<f64> {
        (self.measured_seconds > 0.0).then_some(self.integral)
    }
}

/// Measurements grouped by a condition and mapped onto the selected time range.
pub struct Measurement {
    pub span: Range<f64>,
    /// Low and high respectively. Ungated measurements use the high slot.
    pub groups: [Moments; 2],
    pub unknown_condition_seconds: f64,
    pub unrecorded_seconds: f64,
    /// Fraction of each display interval covered by actual low/high measurements.
    pub bands: Vec<[f64; 2]>,
}

impl MeasurementGrid<'_> {
    /// Uses each value over its sample cell, clipped at the requested endpoints.
    /// Digital gaps and non-finite samples contribute no numerical statistics.
    pub fn measure(&self, span: Range<f64>, cancel: &AtomicBool) -> Result<Measurement, String> {
        let span_width = span.end - span.start;
        let points = self.signal.len();
        if !self.start.is_finite()
            || !self.step.is_finite()
            || self.step <= 0.0
            || !span_width.is_finite()
            || span_width <= 0.0
            || self
                .condition
                .is_some_and(|condition| condition.len() != points)
        {
            return Err("Measurement needs a consistent sample grid and time range.".into());
        }
        let time_at = |index: usize| (index as f64).mul_add(self.step, self.start);
        let materialized = span.start.max(self.start)..span.end.min(time_at(points));
        let mut measurement = Measurement {
            span: span.clone(),
            groups: [Moments::default(); 2],
            unknown_condition_seconds: 0.0,
            unrecorded_seconds: (span_width - (materialized.end - materialized.start).max(0.0))
                .max(0.0),
            bands: vec![[0.0; 2]; BANDS],
        };
        if materialized.is_empty() {
            return Ok(measurement);
        }
        let index_at = |time: f64| (time - self.start) / self.step;
        let first = (index_at(materialized.start).floor() as usize).min(points);
        let last = (index_at(materialized.end).ceil() as usize).min(points);
        for index in first..last {
            if index % 1024 == 0 && cancel.load(Ordering::Relaxed) {
                return Err("Measurement cancelled.".into());
            }
            let left = time_at(index).max(materialized.start);
            let right = time_at(index + 1).min(materialized.end);
            if right > left {
                let condition = self.condition.map_or(1.0, |mask| mask[index]);
                measurement.add(left..right, condition, f64::from(self.signal[index]));
            }
        }
        Ok(measurement)
    }
}

impl Measurement {
    fn add(&mut self, cell: Range<f64>, condition: f32, value: f64) {
        let seconds = cell.end - cell.start;
        let group = match condition {
            0.0 => 0,
            1.0 => 1,
            _ => {
                self.unknown_condition_seconds += seconds;
                return;
            }
        };
        self.groups[group].state_seconds += seconds;
        if !value.is_finite() {
            return;
        }
        self.groups[group].contribute(seconds, value);
        let spacing = (self.span.end - self.span.start) / BANDS as f64;
        let index = |time: f64| ((time - self.span.start) / spacing).clamp(0.0, BANDS as f64);
        let start = index(cell.start);
        let end = index(cell.end);
        for bin in (start.floor() as usize)..(end.ceil() as usize) {
            let left = (bin as f64).max(start);
            let right = (bin as f64 + 1.0).min(end);
            self.bands[bin][group] = (self.bands[bin][group] + (right - left)).min(1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_each_condition_state_without_inventing_missing_samples() {
        let grid = MeasurementGrid {
            start: 0.0,
            step: 1.0,
            signal: &[2.0, 4.0, f32::NAN, 8.0, 10.0],
            condition: Some(&[0.0, 1.0, 1.0, f32::NAN, 0.0]),
        };
        let result = grid.measure(0.25..4.5, &AtomicBool::new(false)).unwrap();
        let [low, high] = result.groups;
        assert_eq!(low.measured_seconds, 1.25);
        assert_eq!(low.integral(), Some(6.5));
        assert_eq!(low.mean(), Some(5.2));
        assert_eq!(low.extent(), Some(2.0..10.0));
        assert_eq!(high.state_seconds, 2.0);
        assert_eq!(high.measured_seconds, 1.0);
        assert_eq!(high.integral(), Some(4.0));
        assert_eq!(high.rms(), Some(4.0));
        assert_eq!(result.unknown_condition_seconds, 1.0);
        assert_eq!(result.unrecorded_seconds, 0.0);
        assert!(result.bands[0][0] > 0.99 && result.bands[0][1] == 0.0);
        assert!(
            result.bands[BANDS / 2]
                .iter()
                .all(|fraction| *fraction == 0.0)
        );
    }

    #[test]
    fn reports_a_selection_beyond_the_materialized_grid() {
        let grid = MeasurementGrid {
            start: 1.0,
            step: 0.5,
            signal: &[2.0, 2.0],
            condition: None,
        };
        let result = grid.measure(0.5..2.25, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.unrecorded_seconds, 0.75);
        assert_eq!(result.groups[0].integral(), None);
        assert_eq!(result.groups[1].integral(), Some(2.0));
    }
}
