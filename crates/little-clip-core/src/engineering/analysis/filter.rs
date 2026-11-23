//! Causal biquad cascades for recorded time series.

use realfft::num_complex::Complex64;
use std::f64::consts::TAU;

/// The response produced by one second-order filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterKind {
    /// Passes frequencies below the cutoff.
    LowPass,
    /// Passes frequencies above the cutoff.
    HighPass,
    /// Passes frequencies near the center.
    BandPass,
    /// Rejects frequencies near the center.
    Notch,
}

/// A reusable filter stage in physical frequency units.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FilterStage {
    /// Shape of the frequency response.
    pub kind: FilterKind,
    /// Cutoff or center frequency in Hertz.
    pub frequency_hz: f64,
    /// Quality factor controlling bandwidth or resonance.
    pub q: f64,
}

impl FilterStage {
    /// Returns the half-power band edges for a band-pass or notch stage.
    ///
    /// Frequencies are in Hertz and account for bilinear frequency warping.
    pub fn band_edges(self, sample_rate: f64) -> Option<[f64; 2]> {
        if !matches!(self.kind, FilterKind::BandPass | FilterKind::Notch) {
            return None;
        }
        Biquad::new(&self, sample_rate).ok()?;
        let center = warp(self.frequency_hz, sample_rate);
        let width_ratio = 0.5 / self.q;
        let upper_ratio = width_ratio.hypot(1.0) + width_ratio;
        let to_hz = |warped: f64| warped.atan() / std::f64::consts::PI * sample_rate;
        Some([to_hz(center / upper_ratio), to_hz(center * upper_ratio)])
    }

    /// Returns the quality factor placing a half-power edge at a frequency.
    pub fn q_for_edge(self, sample_rate: f64, edge_hz: f64) -> Option<f64> {
        if !(0.0..sample_rate * 0.5).contains(&edge_hz) {
            return None;
        }
        self.band_edges(sample_rate)?;
        let center = warp(self.frequency_hz, sample_rate);
        let edge = warp(edge_hz, sample_rate);
        let ratio = edge / center;
        Some((ratio - ratio.recip()).abs().recip().clamp(0.1, 25.0))
    }
}

fn warp(frequency: f64, sample_rate: f64) -> f64 {
    (std::f64::consts::PI * (frequency / sample_rate)).tan()
}

/// Compiled coefficients and sample history for a chain of filters.
///
/// Feed successive blocks in sample order. State carries across calls so
/// splitting an input into blocks leaves the result unchanged.
pub struct FilterChain {
    sample_rate: f64,
    stages: Vec<Biquad>,
}

impl FilterChain {
    /// Creates a cascade on one uniformly sampled channel's native grid.
    ///
    /// # Errors
    /// Rejects invalid sample rates, cutoffs too close to DC or Nyquist, and
    /// quality factors outside the stable design range.
    pub fn new(sample_rate: f64, stages: &[FilterStage]) -> Result<Self, &'static str> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("A filter needs a finite, positive sample rate");
        }
        let stages = stages
            .iter()
            .map(|stage| Biquad::new(stage, sample_rate))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            sample_rate,
            stages,
        })
    }

    /// Applies the cascade in place, retaining its history for the next block.
    ///
    /// A missing sample remains missing and resets history at the gap.
    pub fn process(&mut self, samples: &mut [f32]) {
        for sample in samples {
            let mut value = f64::from(*sample);
            for stage in &mut self.stages {
                value = stage.next(value);
            }
            *sample = value as f32;
        }
    }

    /// Returns the cascade's amplitude gain relative to the input.
    ///
    /// A gain of 1 leaves amplitude unchanged. Frequencies outside the
    /// one-sided bandwidth return `None`.
    pub fn gain_at(&self, frequency_hz: f64) -> Option<f64> {
        if !(0.0..=self.sample_rate * 0.5).contains(&frequency_hz) {
            return None;
        }
        let omega = TAU * (frequency_hz / self.sample_rate);
        let first = Complex64::from_polar(1.0, -omega);
        let second = first * first;
        Some(
            self.stages
                .iter()
                .map(|stage| stage.gain_at(first, second))
                .product(),
        )
    }
}

struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    history: [f64; 2],
}

impl Biquad {
    fn new(stage: &FilterStage, sample_rate: f64) -> Result<Self, &'static str> {
        let ratio = stage.frequency_hz / sample_rate;
        if !(1e-7..=0.5 - 1e-7).contains(&ratio) {
            return Err("The filter frequency must be between DC and Nyquist");
        }
        if !(0.1..=25.0).contains(&stage.q) {
            return Err("The filter Q must be between 0.1 and 25");
        }
        // Stable branch of the biquad design in difference_equation_calc.
        // Half-angle form avoids subtracting nearly equal values at low cutoffs.
        let theta = TAU * ratio;
        let cosine = theta.cos();
        let alpha = theta.sin() / (2.0 * stage.q);
        let b = match stage.kind {
            FilterKind::LowPass => {
                let edge = (theta * 0.5).sin().powi(2);
                [edge, 2.0 * edge, edge]
            }
            FilterKind::HighPass => {
                let edge = (theta * 0.5).cos().powi(2);
                [edge, -2.0 * edge, edge]
            }
            FilterKind::BandPass => [alpha, 0.0, -alpha],
            FilterKind::Notch => [1.0, -2.0 * cosine, 1.0],
        };
        let a = [-2.0 * cosine, 1.0 - alpha];
        let scale = (1.0 + alpha).recip();
        Ok(Self {
            b: b.map(|coefficient| coefficient * scale),
            a: a.map(|coefficient| coefficient * scale),
            history: [0.0; 2],
        })
    }

    fn next(&mut self, input: f64) -> f64 {
        if !input.is_finite() {
            self.history = [0.0; 2];
            return f64::NAN;
        }
        let y = self.b[0] * input + self.history[0];
        self.history = [
            self.b[1] * input - self.a[0] * y + self.history[1],
            self.b[2] * input - self.a[1] * y,
        ];
        y
    }

    fn gain_at(&self, first: Complex64, second: Complex64) -> f64 {
        let numerator = self.b[0] + self.b[1] * first + self.b[2] * second;
        let denominator = 1.0 + self.a[0] * first + self.a[1] * second;
        numerator.norm() / denominator.norm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(kind: FilterKind, hz: f64) -> FilterStage {
        FilterStage {
            kind,
            frequency_hz: hz,
            q: std::f64::consts::FRAC_1_SQRT_2,
        }
    }

    #[test]
    fn four_shapes_respect_dc_center_and_nyquist() {
        for (kind, expected) in [
            (FilterKind::LowPass, [1.0, 0.70710678, 0.0]),
            (FilterKind::HighPass, [0.0, 0.70710678, 1.0]),
            (FilterKind::BandPass, [0.0, 1.0, 0.0]),
            (FilterKind::Notch, [1.0, 0.0, 1.0]),
        ] {
            let chain = FilterChain::new(1000.0, &[stage(kind, 100.0)]).unwrap();
            for (hz, amplitude) in [0.0, 100.0, 500.0].into_iter().zip(expected) {
                assert!((chain.gain_at(hz).unwrap() - amplitude).abs() < 1e-7);
            }
        }
    }

    #[test]
    fn dragged_band_edges_match_the_applied_filter_at_high_frequencies() {
        for kind in [FilterKind::BandPass, FilterKind::Notch] {
            let rate = 1_000.0;
            let mut stage = stage(kind, 380.0);
            stage.q = 4.2;
            let chain = FilterChain::new(rate, &[stage]).unwrap();
            for edge in stage.band_edges(rate).unwrap() {
                assert!(
                    (chain.gain_at(edge).unwrap() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-10
                );
                assert!((stage.q_for_edge(rate, edge).unwrap() - stage.q).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn processing_in_blocks_preserves_history_until_a_gap() {
        let stages = [
            stage(FilterKind::LowPass, 150.0),
            stage(FilterKind::Notch, 100.0),
        ];
        let mut whole = [1.0, 2.0, 3.0, f32::NAN, 1.0, 2.0, 3.0];
        let mut chunked = whole;
        FilterChain::new(1000.0, &stages)
            .unwrap()
            .process(&mut whole);
        let mut filter = FilterChain::new(1000.0, &stages).unwrap();
        for block in chunked.chunks_mut(2) {
            filter.process(block);
        }
        assert_eq!(whole[..3], whole[4..]);
        assert_eq!(whole.map(f32::to_bits), chunked.map(f32::to_bits));
    }

    #[test]
    fn measured_tone_amplitude_matches_plotted_gain() {
        let hz = 70.0;
        let rate = 1000.0;
        let stages = [
            stage(FilterKind::HighPass, 40.0),
            stage(FilterKind::Notch, 100.0),
        ];
        let mut chain = FilterChain::new(rate, &stages).unwrap();
        let gain = chain.gain_at(hz).unwrap();
        let mut samples: Vec<f32> = (0..4000)
            .map(|i| (TAU * i as f64 * hz / rate).sin() as f32)
            .collect();
        chain.process(&mut samples);
        let sum: f64 = samples[2000..]
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum();
        let measured = (2.0 * sum / 2000.0).sqrt();
        assert!((gain - measured).abs() < 1e-6);
    }
}
