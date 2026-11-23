use realfft::RealFftPlanner;
use std::f64::consts::TAU;

/// Largest contiguous window accepted by the spectrum and period tools.
pub const MAX_ANALYSIS_SAMPLES: usize = 65_536;

/// Apodization used when preparing a spectrum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpectrumWindow {
    /// A periodic Hann window for general-purpose spectral analysis.
    #[default]
    Hann,
    /// Retains the original samples for coherent measurements.
    Rectangular,
    /// Suppresses sidelobes when comparing tones of very different amplitudes.
    BlackmanHarris,
}

impl SpectrumWindow {
    fn weight(self, index: usize, len: usize) -> f64 {
        let phase = TAU * index as f64 / len as f64;
        match self {
            Self::Hann => 0.5 - 0.5 * phase.cos(),
            Self::Rectangular => 1.0,
            Self::BlackmanHarris => {
                0.35875 - 0.48829 * phase.cos() + 0.14128 * (2.0 * phase).cos()
                    - 0.01168 * (3.0 * phase).cos()
            }
        }
    }
}

/// One-sided, coherent-gain-normalized amplitude spectrum.
#[derive(Debug, Clone)]
pub struct Spectrum {
    /// Frequency in Hertz between successive FFT bins.
    pub bin_hz: f64,
    /// Peak amplitudes in the original input units from DC through Nyquist.
    pub peak_amplitude: Vec<f32>,
}

/// Returns a spectrum using a periodic Hann window.
///
/// See [`spectrum_with_window`] for the amplitude and input conventions.
pub fn spectrum(samples: &[f32], sample_rate_hz: f64) -> Option<Spectrum> {
    spectrum_with_window(samples, sample_rate_hz, SpectrumWindow::Hann)
}

/// Returns the one-sided amplitude spectrum of equally spaced samples.
///
/// A bin-centered sinusoid reports its peak amplitude in the input units.
/// DC and Nyquist are kept at their single-sided gain. Padding to a power of
/// two refines the bin grid while record length still limits resolution.
/// Missing, non-finite, or oversized windows return `None`.
pub fn spectrum_with_window(
    samples: &[f32],
    sample_rate_hz: f64,
    window: SpectrumWindow,
) -> Option<Spectrum> {
    if !valid_input(samples, sample_rate_hz, 2) {
        return None;
    }
    let size = samples.len().next_power_of_two();
    let bin_hz = sample_rate_hz / size as f64;
    let scale = max_absolute(samples).max(f64::from(f32::MIN_POSITIVE));
    let mut plan = RealFftPlanner::<f32>::new();
    let fft = plan.plan_fft_forward(size);
    let mut input = fft.make_input_vec();
    let mut coherent_gain = 0.0;
    for (index, (&sample, output)) in samples.iter().zip(&mut input).enumerate() {
        let weight = window.weight(index, samples.len());
        coherent_gain += weight;
        *output = (f64::from(sample) / scale * weight) as f32;
    }
    let mut bins = fft.make_output_vec();
    fft.process(&mut input, &mut bins).ok()?;
    let last = bins.len().checked_sub(1)?;
    let peak_amplitude: Vec<_> = bins
        .iter()
        .enumerate()
        .map(|(index, bin)| {
            let one_sided_gain = if index == 0 || index == last {
                1.0
            } else {
                2.0
            };
            (f64::from(bin.norm()) * scale * one_sided_gain / coherent_gain) as f32
        })
        .collect();
    peak_amplitude
        .iter()
        .all(|sample| sample.is_finite())
        .then_some(Spectrum {
            bin_hz,
            peak_amplitude,
        })
}

/// Estimates the repeat frequency of an equally spaced signal.
///
/// Returns `None` if the record is flat or lacks a sufficiently periodic
/// waveform. The detector considers lags with at least three periods in the
/// supplied window. It searches for a repeating waveform, including when
/// harmonics are stronger than the fundamental.
pub fn fundamental_hz(samples: &[f32], sample_rate_hz: f64) -> Option<f64> {
    if !valid_input(samples, sample_rate_hz, 32) {
        return None;
    }
    let (correlation, oscillation) = normalized_correlation(samples)?;
    let period = periodic_peak(&correlation, oscillation)?;
    Some(sample_rate_hz / period)
}

fn valid_input(samples: &[f32], sample_rate_hz: f64, min_samples: usize) -> bool {
    (min_samples..=MAX_ANALYSIS_SAMPLES).contains(&samples.len())
        && sample_rate_hz.is_finite()
        && sample_rate_hz >= f64::MIN_POSITIVE
        && samples.iter().all(|sample| sample.is_finite())
}

fn max_absolute(samples: &[f32]) -> f64 {
    f64::from(
        samples
            .iter()
            .fold(0.0_f32, |peak, value| peak.max(value.abs())),
    )
}

fn normalized_correlation(samples: &[f32]) -> Option<(Vec<f64>, Option<Oscillation>)> {
    let len = samples.len();
    let dc = samples.iter().map(|&value| f64::from(value)).sum::<f64>() / len as f64;
    let excursion = samples
        .iter()
        .map(|&value| (f64::from(value) - dc).abs())
        .fold(0.0, f64::max);
    if excursion <= max_absolute(samples) * f64::from(f32::EPSILON) {
        return None;
    }

    // Zero padding leaves the FFT autocorrelation linear across the real data.
    let fft_len = (len * 2).next_power_of_two();
    let mut plan = RealFftPlanner::<f32>::new();
    let forward = plan.plan_fft_forward(fft_len);
    let inverse = plan.plan_fft_inverse(fft_len);
    let mut signal = forward.make_input_vec();
    let mut power = forward.make_output_vec();
    let mut energy = Vec::with_capacity(len + 1);
    let mut total = 0.0;
    energy.push(total);
    for (&sample, output) in samples.iter().zip(&mut signal) {
        let value = (f64::from(sample) - dc) / excursion;
        total += value * value;
        energy.push(total);
        *output = value as f32;
    }
    forward.process(&mut signal, &mut power).ok()?;
    for bin in &mut power {
        *bin = realfft::num_complex::Complex::new(bin.norm_sqr() / fft_len as f32, 0.0);
    }
    let oscillation = dominant_oscillation(&power, fft_len);
    inverse.process(&mut power, &mut signal).ok()?;

    // Normalize by the energy actually participating in each lag. A repeated
    // waveform scores near 1, even when the overlap is shorter than one record.
    Some((
        signal[..=len / 3]
            .iter()
            .enumerate()
            .map(|(lag, &dot)| {
                let denominator = energy[len - lag] + total - energy[lag];
                (2.0 * f64::from(dot) / denominator).clamp(-1.0, 1.0)
            })
            .collect(),
        oscillation,
    ))
}

struct Oscillation {
    phase: f64,
    sin: f64,
    cos_minus_one: f64,
}

fn dominant_oscillation(
    power: &[realfft::num_complex::Complex32],
    fft_len: usize,
) -> Option<Oscillation> {
    let mut total = 0.0;
    let mut strongest = (0, 0.0);
    for (index, point) in power.iter().enumerate().skip(1) {
        let magnitude = f64::from(point.re);
        total += magnitude;
        if magnitude > strongest.1 {
            strongest = (index, magnitude);
        }
    }
    let (index, magnitude) = strongest;
    if magnitude < 0.18 * total || index == power.len() - 1 || index == 0 {
        return None;
    }
    let left = f64::from(power[index - 1].re).ln();
    let right = f64::from(power[index + 1].re).ln();
    let bin = if left.is_finite() && right.is_finite() {
        parabolic_peak(index, left, magnitude.ln(), right).0
    } else {
        index as f64
    };
    let phase = TAU * bin / fft_len as f64;
    let (sin, cos) = phase.sin_cos();
    if !(0.35..std::f64::consts::PI - 0.025).contains(&phase) {
        return None;
    }
    Some(Oscillation {
        phase,
        sin,
        cos_minus_one: cos - 1.0,
    })
}

fn periodic_peak(correlation: &[f64], oscillation: Option<Oscillation>) -> Option<f64> {
    // Skip the drop from lag zero, where a slowly changing signal trivially
    // resembles itself. Estimate each local positive peak at fractional lag.
    let mut peaks = correlation
        .windows(3)
        .enumerate()
        .skip(1)
        .filter_map(|(index, values)| {
            let [left, center, right] = *values else {
                return None;
            };
            if center <= left || center < right || center < 0.15 {
                return None;
            }
            Some(match &oscillation {
                Some(shape) => shape.peak(index + 1, left, center, right),
                None => parabolic_peak(index + 1, left, center, right),
            })
        });
    let highest = peaks.clone().map(|(_, height)| height).fold(0.0, f64::max);
    if highest < 0.85 {
        return None;
    }
    // Nearby periods may be repeated harmonics. Where the full period is
    // almost perfectly periodic, require that level of agreement to favor
    // the shorter period.
    let threshold = highest - (1.0 - highest).max(0.012);
    peaks.find_map(|(lag, height)| (height >= threshold).then_some(lag))
}

impl Oscillation {
    fn peak(&self, index: usize, left: f64, center: f64, right: f64) -> (f64, f64) {
        // At a few samples per cycle a quadratic fit can badly undershoot
        // a peak between samples. A local cosine fit uses the
        // strongest oscillation in the same FFT used for autocorrelation.
        let cosine = (left + right - 2.0 * center) / (2.0 * self.cos_minus_one);
        let sine = (right - left) / (2.0 * self.sin);
        let delta = (sine.atan2(cosine) / self.phase).clamp(-0.5, 0.5);
        let (sin, cos) = (self.phase * delta).sin_cos();
        let height = center + cosine * (cos - 1.0) + sine * sin;
        (index as f64 + delta, height.min(1.0))
    }
}

fn parabolic_peak(index: usize, left: f64, center: f64, right: f64) -> (f64, f64) {
    let curvature = left + right - 2.0 * center;
    let slope = (right - left) * 0.5;
    let delta = (-slope / curvature).clamp(-0.5, 0.5);
    (
        index as f64 + delta,
        (center + slope * delta + 0.5 * curvature * delta * delta).min(1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sampled(count: usize, sample: impl Fn(f64) -> f64) -> Vec<f32> {
        (0..count)
            .map(|index| sample(index as f64) as f32)
            .collect()
    }

    #[test]
    fn reports_calibrated_amplitude_and_does_not_double_dc_or_nyquist() {
        let len = 512;
        let signal = sampled(len, |index| {
            0.5 + 2.0 * (TAU * index * 23.0 / len as f64).sin() + 0.25 * (TAU * index / 2.0).cos()
        });
        for window in [
            SpectrumWindow::Hann,
            SpectrumWindow::Rectangular,
            SpectrumWindow::BlackmanHarris,
        ] {
            let spectrum = spectrum_with_window(&signal, 512.0, window).unwrap();
            assert_eq!(spectrum.bin_hz, 1.0);
            assert!((spectrum.peak_amplitude[0] - 0.5).abs() < 1e-5);
            assert!((spectrum.peak_amplitude[23] - 2.0).abs() < 1e-5);
            assert!((spectrum.peak_amplitude[256] - 0.25).abs() < 1e-5);
        }
    }

    #[test]
    fn period_detection_is_not_limited_to_audio_rates() {
        let rate = 8_000_000.0;
        let tone = 82_388.5;
        let signal = sampled(4096, |index| 2.7 + 1.5 * (TAU * tone * index / rate).sin());
        assert!((fundamental_hz(&signal, rate).unwrap() / tone - 1.0).abs() < 0.002);
    }

    #[test]
    fn resolves_a_fractional_period_near_nyquist() {
        let period = 2.4;
        let signal = sampled(8192, |index| (TAU * index / period).cos());
        let estimated_period = 1.0 / fundamental_hz(&signal, 1.0).unwrap();
        assert!((estimated_period - period).abs() < 0.01);
    }

    #[test]
    fn uses_the_repeat_period_when_the_second_harmonic_is_stronger() {
        let signal = sampled(8192, |index| {
            let phase = TAU * index / 96.19;
            0.13 * phase.sin() + (phase * 2.0 + 0.2).sin() + 0.15 * (phase * 4.0).sin()
        });
        assert!((fundamental_hz(&signal, 96_190.0).unwrap() - 1000.0).abs() < 1.0);
    }

    #[test]
    fn refuses_flat_or_aperiodic_records() {
        let mut state = 141u32;
        let noise = (0..2048)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 8) as f32 / (1 << 24) as f32
            })
            .collect::<Vec<_>>();
        assert_eq!(fundamental_hz(&noise, 48_000.0), None);
        assert_eq!(fundamental_hz(&[2.0; 256], 48_000.0), None);
        assert_eq!(fundamental_hz(&[f32::NAN; 256], 48_000.0), None);
        assert_eq!(fundamental_hz(&[0.0; 256], f64::INFINITY), None);
    }
}
