use super::{Complex64, Error, TracePoint};
use crate::engineering::analysis::SpectrumWindow;
use rustfft::FftPlanner;
use std::ops::Range;

const MAX_POINTS: usize = 16_384;

/// Selects which delay samples contribute to a reconstructed trace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GateMode {
    /// Retains the selected delay interval.
    #[default]
    Keep,
    /// Removes the selected delay interval.
    Reject,
}

/// Band-limited complex impulse response of a selected network trace.
///
/// Samples are arranged in signed delay order. Frequencies are treated as a
/// contiguous measured band, without extrapolation to DC.
pub struct ImpulseResponse {
    first_hz: f64,
    spacing_hz: f64,
    samples: Vec<Complex64>,
    interpolated: bool,
}

impl ImpulseResponse {
    /// Transforms exact network sweep points into a delay-domain response.
    ///
    /// A sweep with nonuniform frequency steps is linearly interpolated in
    /// rectangular coordinates. The taper is normalized for coherent gain.
    ///
    /// # Errors
    /// Rejects non-finite or non-increasing data and unsupported point counts.
    pub fn from_trace(points: &[TracePoint], window: SpectrumWindow) -> Result<Self, Error> {
        if !(2..=MAX_POINTS).contains(&points.len()) {
            return Err(Error::Limit(
                "impulse response requires 2 to 16,384 exact points",
            ));
        }
        let first_hz = points[0].frequency_hz;
        let last_hz = points[points.len() - 1].frequency_hz;
        let spacing_hz = (last_hz - first_hz) / (points.len() - 1) as f64;
        if !spacing_hz.is_finite()
            || spacing_hz <= 0.0
            || points.iter().any(|point| {
                !point.frequency_hz.is_finite()
                    || !point.value.re.is_finite()
                    || !point.value.im.is_finite()
            })
            || points
                .windows(2)
                .any(|pair| pair[1].frequency_hz <= pair[0].frequency_hz)
        {
            return Err(Error::Invalid(
                "impulse response requires finite, increasing frequencies",
            ));
        }
        let interpolated = points.iter().enumerate().any(|(index, point)| {
            let expected = (index as f64).mul_add(spacing_hz, first_hz);
            let roundoff = expected.abs() * f64::EPSILON * 8.0;
            (point.frequency_hz - expected).abs() > (spacing_hz * 1e-6).max(roundoff)
        });
        let mut samples = if interpolated {
            uniform_trace(points, first_hz, spacing_hz)
        } else {
            points.iter().map(|point| point.value).collect()
        };
        let count = samples.len();
        let normalization = (0..count)
            .map(|index| window.weight(index, count))
            .sum::<f64>();
        for (index, sample) in samples.iter_mut().enumerate() {
            *sample *= window.weight(index, count) / normalization;
        }
        FftPlanner::new()
            .plan_fft_inverse(count)
            .process(&mut samples);
        samples.rotate_right(count / 2);
        if samples
            .iter()
            .any(|sample| !sample.re.is_finite() || !sample.im.is_finite())
        {
            return Err(Error::Invalid("non-finite impulse response"));
        }
        Ok(Self {
            first_hz,
            spacing_hz,
            samples,
            interpolated,
        })
    }

    /// Returns complex amplitudes in signed delay order.
    pub fn samples(&self) -> &[Complex64] {
        &self.samples
    }

    /// Returns the delay of sample zero.
    pub fn start_seconds(&self) -> f64 {
        -((self.samples.len() / 2) as f64) * self.step_seconds()
    }

    /// Returns the delay sample interval.
    pub fn step_seconds(&self) -> f64 {
        (self.samples.len() as f64 * self.spacing_hz).recip()
    }

    /// Returns the measured frequency band contributing to the response.
    pub fn band_hz(&self) -> [f64; 2] {
        [
            self.first_hz,
            self.first_hz + (self.samples.len() - 1) as f64 * self.spacing_hz,
        ]
    }

    /// Returns whether the frequency grid needed interpolation.
    pub fn interpolated(&self) -> bool {
        self.interpolated
    }

    /// Reconstructs a frequency trace from the chosen half-open delay interval.
    ///
    /// The reconstructed trace retains the taper used for the transform.
    ///
    /// # Errors
    /// Rejects a gate that contains no delay samples.
    pub fn gate(&self, time: Range<f64>) -> Result<Vec<TracePoint>, Error> {
        self.apply_gate(time, GateMode::Keep)
    }

    /// Reconstructs the contribution inside or outside a delay interval.
    ///
    /// # Errors
    /// Rejects a gate that contains no delay samples.
    pub fn apply_gate(&self, time: Range<f64>, mode: GateMode) -> Result<Vec<TracePoint>, Error> {
        let dt = self.step_seconds();
        let start = self.start_seconds();
        let selected = |index| time.contains(&(index as f64).mul_add(dt, start));
        if !(0..self.samples.len()).any(selected) {
            return Err(Error::Invalid("the gate contains no delay samples"));
        }
        let mut samples: Vec<_> = self
            .samples
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                if selected(index) == (mode == GateMode::Keep) {
                    value
                } else {
                    Complex64::ZERO
                }
            })
            .collect();
        let count = samples.len();
        samples.rotate_left(count / 2);
        FftPlanner::new()
            .plan_fft_forward(count)
            .process(&mut samples);
        Ok(samples
            .into_iter()
            .enumerate()
            .map(|(index, value)| TracePoint {
                index: index as u64,
                frequency_hz: (index as f64).mul_add(self.spacing_hz, self.first_hz),
                value,
            })
            .collect())
    }
}

fn uniform_trace(points: &[TracePoint], start: f64, step: f64) -> Vec<Complex64> {
    let mut current = 0;
    (0..points.len())
        .map(|index| {
            let frequency = (index as f64).mul_add(step, start);
            while current + 2 < points.len() && points[current + 1].frequency_hz < frequency {
                current += 1;
            }
            let left = points[current];
            let right = points[current + 1];
            let fraction = ((frequency - left.frequency_hz)
                / (right.frequency_hz - left.frequency_hz))
                .clamp(0.0, 1.0);
            left.value * (1.0 - fraction) + right.value * fraction
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    #[test]
    fn a_reflection_gate_retains_one_delay_and_roundtrips_the_measured_band() {
        let n = 128;
        let df = 2.0e6;
        let dt = 1.0 / (n as f64 * df);
        let first = 100.0e6;
        let expected = |hz| Complex64::from_polar(0.8, -TAU * hz * 5.0 * dt);
        let points: Vec<_> = (0..n)
            .map(|index| {
                let hz = first + index as f64 * df;
                TracePoint {
                    index,
                    frequency_hz: hz,
                    value: expected(hz) + Complex64::from_polar(0.25, -TAU * hz * 17.0 * dt),
                }
            })
            .collect();
        let response = ImpulseResponse::from_trace(&points, SpectrumWindow::Rectangular).unwrap();
        assert!((response.step_seconds() - dt).abs() < 1e-18);
        let middle = n as usize / 2;
        assert!((response.samples()[middle + 5].norm() - 0.8).abs() < 1e-10);
        assert!((response.samples()[middle + 17].norm() - 0.25).abs() < 1e-10);
        let gated = response.gate(4.0 * dt..6.0 * dt).unwrap();
        for point in gated {
            assert!((point.value - expected(point.frequency_hz)).norm() < 1e-10);
        }
        let rejected = response
            .apply_gate(4.0 * dt..6.0 * dt, GateMode::Reject)
            .unwrap();
        for (point, measured) in rejected.iter().zip(&points) {
            assert!((point.value - (measured.value - expected(point.frequency_hz))).norm() < 1e-10);
        }
    }

    #[test]
    fn nonuniform_sweep_uses_and_reports_a_uniform_grid() {
        let points = [1e6, 1.3e6, 2.0e6, 4e6]
            .into_iter()
            .enumerate()
            .map(|(index, hz)| TracePoint {
                index: index as u64,
                frequency_hz: hz,
                value: Complex64::new(0.25, 0.0),
            })
            .collect::<Vec<_>>();
        let view = ImpulseResponse::from_trace(&points, SpectrumWindow::Rectangular).unwrap();
        assert!(view.interpolated());
        let gated = view.gate(-1.0..1.0).unwrap();
        assert_eq!(
            gated
                .iter()
                .map(|point| point.frequency_hz)
                .collect::<Vec<_>>(),
            vec![1e6, 2e6, 3e6, 4e6]
        );
        assert!(
            gated
                .iter()
                .all(|point| (point.value.re - 0.25).abs() < 1e-12)
        );
    }
}
