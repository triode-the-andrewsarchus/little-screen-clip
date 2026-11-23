//! Common-frequency readouts and apertures with measured row provenance.

use super::{
    Complex64, Metadata, PortMode, TracePoint,
    group_delay::{has_phase, unwrap_phase},
};
use std::f64::consts::TAU;

/// Physical identity of a response and stimulus axis within a parameter family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Axes {
    /// Response axis in the sweep's physical port numbering.
    pub response: PortMode,
    /// Stimulus axis in the sweep's physical port numbering.
    pub stimulus: PortMode,
}

impl Axes {
    /// Resolves physical axes from a row and column of this file's matrix.
    pub fn from_trace(metadata: &Metadata, [response, stimulus]: [usize; 2]) -> Option<Self> {
        Some(Self {
            response: axis(metadata, response)?,
            stimulus: axis(metadata, stimulus)?,
        })
    }

    /// Finds the matrix position of these axes in another sweep.
    pub fn trace(self, metadata: &Metadata) -> Option<[usize; 2]> {
        let locate = |mode| (0..metadata.ports).find(|&slot| axis(metadata, slot) == Some(mode));
        Some([locate(self.response)?, locate(self.stimulus)?])
    }

    /// Confirms equal normalization and reference impedances for these axes.
    pub fn same_basis(self, a: &Metadata, b: &Metadata) -> bool {
        a.parameter == b.parameter
            && a.normalized() == b.normalized()
            && (!a.normalized()
                || [self.response, self.stimulus].into_iter().all(|mode| {
                    let references = |meta: &Metadata| match mode {
                        PortMode::Single(port) => [reference(meta, port), None],
                        PortMode::Differential(a, b) | PortMode::Common(a, b) => {
                            [reference(meta, a), reference(meta, b)]
                        }
                    };
                    references(a) == references(b)
                }))
    }
}

fn axis(metadata: &Metadata, slot: usize) -> Option<PortMode> {
    if metadata.mixed_mode.is_empty() {
        (slot < metadata.ports).then(|| PortMode::Single(slot + 1))
    } else {
        metadata.mixed_mode.get(slot).copied()
    }
}

fn reference(metadata: &Metadata, port: usize) -> Option<f64> {
    metadata.reference_ohms.get(port.checked_sub(1)?).copied()
}

/// Exact or complex-linearly interpolated readout at a requested frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Frequency evaluated on both sweeps.
    pub frequency_hz: f64,
    /// Value in rectangular coordinates on the source's original basis.
    pub value: Complex64,
    /// The two measured records. Both entries coincide for exact readings.
    pub rows: [TracePoint; 2],
}

impl Sample {
    /// Returns true when the result is a recorded measurement.
    pub fn exact(self) -> bool {
        self.rows[0].index == self.rows[1].index
    }
}

/// Samples an increasing, contiguous window without extrapolation.
pub fn sample(points: &[TracePoint], hz: f64) -> Option<Sample> {
    let [lo, hi] = locate(points, hz)?;
    let rows = [points[lo], points[hi]];
    let value = if lo == hi {
        rows[0].value
    } else {
        mix(rows[0].value, rows[1].value, fraction(rows, hz))
    };
    Some(Sample {
        frequency_hz: hz,
        value,
        rows,
    })
}

/// Average group delay over the exact requested aperture and its supporting rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delay {
    /// Average negative phase slope divided by a full turn.
    pub seconds: f64,
    /// Frequency interval used to calculate the slope.
    pub aperture_hz: [f64; 2],
    /// Measured support for the lower and upper phase estimates.
    pub edges: [[TracePoint; 2]; 2],
}

/// Uses the same frequency aperture on either grid. Boundary phases are
/// linearly interpolated after unwrapping consecutive measured phases.
pub fn delay(points: &[TracePoint], [start, end]: [f64; 2]) -> Option<Delay> {
    if end <= start {
        return None;
    }
    let [a, b] = locate(points, start)?;
    let [c, d] = locate(points, end)?;
    let run = &points[a..=d];
    if !valid_phase_run(run) {
        return None;
    }
    let phase = unwrap_phase(run);
    let edges = [[points[a], points[b]], [points[c], points[d]]];
    let phase_at =
        |[lo, hi]: [usize; 2], rows, hz| mix(phase[lo - a], phase[hi - a], fraction(rows, hz));
    let slope = phase_at([c, d], edges[1], end) - phase_at([a, b], edges[0], start);
    let seconds = -slope / (end - start) / TAU;
    seconds.is_finite().then_some(Delay {
        seconds,
        aperture_hz: [start, end],
        edges,
    })
}

fn valid_phase_run(run: &[TracePoint]) -> bool {
    run.iter().all(has_phase)
        && run.windows(2).all(|pair| {
            pair[0].index.checked_add(1) == Some(pair[1].index)
                && pair[0].frequency_hz < pair[1].frequency_hz
        })
}

fn locate(points: &[TracePoint], hz: f64) -> Option<[usize; 2]> {
    if !hz.is_finite() {
        return None;
    }
    let upper = points.partition_point(|point| point.frequency_hz < hz);
    let after = points.get(upper)?;
    if after.frequency_hz == hz {
        return Some([upper; 2]);
    }
    let lower = upper.checked_sub(1)?;
    let before = points[lower];
    (before.index.checked_add(1) == Some(after.index)
        && before.frequency_hz < hz
        && hz < after.frequency_hz)
        .then_some([lower, upper])
}

fn fraction([left, right]: [TracePoint; 2], hz: f64) -> f64 {
    if left.index == right.index {
        0.0
    } else {
        (hz - left.frequency_hz) / (right.frequency_hz - left.frequency_hz)
    }
}

fn mix<T>(a: T, b: T, fraction: f64) -> T
where
    T: std::ops::Mul<f64, Output = T> + std::ops::Add<Output = T>,
{
    a * (1.0 - fraction) + b * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_physical_axes_across_matrix_order_and_checks_relevant_impedances() {
        use super::super::{Parameter, Version};
        let mut a = Metadata {
            version: Version::V2,
            parameter: Parameter::S,
            ports: 3,
            reference_ohms: vec![50.0, 50.0, 75.0],
            declared_points: None,
            mixed_mode: vec![
                PortMode::Differential(1, 2),
                PortMode::Common(1, 2),
                PortMode::Single(3),
            ],
            declared_noise_points: None,
        };
        let mut b = a.clone();
        b.mixed_mode.swap(0, 2);
        b.reference_ohms[2] = 100.0;
        let axes = Axes::from_trace(&a, [0, 1]).unwrap();
        assert_eq!(axes.trace(&b), Some([2, 1]));
        assert!(axes.same_basis(&a, &b));
        b.reference_ohms[0] = 40.0;
        assert!(!axes.same_basis(&a, &b));
        a.parameter = Parameter::Z;
        b.parameter = Parameter::Z;
        assert!(axes.same_basis(&a, &b));
        b.version = Version::V1;
        assert!(!axes.same_basis(&a, &b));
    }

    #[test]
    fn exact_and_off_grid_values_keep_original_rows_without_extrapolation() {
        let points = [
            TracePoint {
                index: 12,
                frequency_hz: 100e6,
                value: Complex64::new(0.6, 0.1),
            },
            TracePoint {
                index: 13,
                frequency_hz: 150e6,
                value: Complex64::new(0.1, -0.1),
            },
        ];
        assert!(sample(&points, 100e6).unwrap().exact());
        let between = sample(&points, 130e6).unwrap();
        assert_eq!(between.rows, points);
        assert!(!between.exact());
        assert!((between.value - Complex64::new(0.30, -0.02)).norm() < 1e-12);
        assert!(sample(&points, 151e6).is_none());
        assert!(
            sample(
                &[
                    points[0],
                    TracePoint {
                        index: 14,
                        ..points[1]
                    }
                ],
                130e6
            )
            .is_none()
        );
    }

    #[test]
    fn equal_apertures_compare_delays_on_different_frequency_grids() {
        let measured = |offset: f64, step: f64, delay: f64| {
            (0..16)
                .map(|index| {
                    let frequency_hz = offset + index as f64 * step;
                    TracePoint {
                        index,
                        frequency_hz,
                        value: Complex64::from_polar(0.4, -TAU * frequency_hz * delay),
                    }
                })
                .collect::<Vec<_>>()
        };
        let [a, mut b] = [measured(90e6, 4e6, 8e-9), measured(96e6, 5e6, 10e-9)];
        let aperture = [107e6, 133e6];
        let first = delay(&a, aperture).unwrap();
        let second = delay(&b, aperture).unwrap();
        assert!((second.seconds - first.seconds - 2e-9).abs() < 1e-20);
        assert_eq!(
            first.edges[0].map(|point| point.frequency_hz),
            [106e6, 110e6]
        );
        assert_eq!(
            second.edges[1].map(|point| point.frequency_hz),
            [131e6, 136e6]
        );
        b[5].value = Complex64::ZERO;
        assert!(delay(&b, aperture).is_none());
    }
}
