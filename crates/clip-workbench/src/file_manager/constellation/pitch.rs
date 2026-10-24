//! Worker-side phase-vocoder transposition. Playback only borrows prepared PCM.
#[cfg(test)]
use super::sample_pack::PitchMode;
use super::sample_pack::SlotSample;
use realfft::{RealFftPlanner, num_complex::Complex32};
use std::sync::Arc;

impl SlotSample {
    /// Runs only on a bounded preparation/export worker, never on the UI or mixer thread.
    pub(super) fn prepare_pitch(&mut self) -> Result<(), String> {
        if !self.pitch_pending() {
            return Ok(());
        }
        let original = self.cached_audio.as_ref().ok_or("PCM is not prepared")?;
        let values = shift(
            &original.samples,
            original.channels as usize,
            self.pitch_semitones,
        )?;
        let mut processed = (**original).clone();
        processed.samples = Arc::new(values);
        self.prepared_pitch = Some((self.pitch_semitones.to_bits(), Arc::new(processed)));
        Ok(())
    }
}

/// Transposes interleaved PCM by ±24 semitones without changing its frame count.
///
/// Centered Hann windows and overlap normalization cover even short samples and
/// both endpoints. Independent channel phases preserve aligned stereo timing.
fn shift(samples: &[f32], channels: usize, semitones: f32) -> Result<Vec<f32>, String> {
    if channels == 0 || !semitones.is_finite() || !(-24.0..=24.0).contains(&semitones) {
        return Err("Pitch must be within ±24 semitones".into());
    }
    if semitones == 0.0 {
        return Ok(samples.to_vec());
    }
    const N: usize = 2048;
    const HOP: usize = N / 4;
    let frames = samples.len() / channels;
    let ratio = 2.0_f32.powf(semitones / 12.0);
    let tau = std::f32::consts::TAU;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let inverse = planner.plan_fft_inverse(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut shifted = inverse.make_input_vec();
    let mut time = inverse.make_output_vec();
    let mut forward_scratch = fft.make_scratch_vec();
    let mut inverse_scratch = inverse.make_scratch_vec();
    let window: Vec<_> = (0..N)
        .map(|i| (std::f32::consts::PI * i as f32 / N as f32).sin().powi(2))
        .collect();
    let bins = spectrum.len();
    let mut output = vec![0.0; samples.len()];
    let mut weights = vec![0.0; frames];
    for channel in 0..channels {
        let mut previous = vec![0.0; bins];
        let mut phases = vec![0.0; bins];
        let mut magnitudes = vec![0.0; bins];
        let mut frequencies = vec![0.0; bins];
        for center in (0..frames + N / 2).step_by(HOP) {
            let start = center as isize - N as isize / 2;
            for i in 0..N {
                let frame = start + i as isize;
                input[i] = if frame >= 0 && (frame as usize) < frames {
                    samples[frame as usize * channels + channel] * window[i]
                } else {
                    0.0
                };
            }
            fft.process_with_scratch(&mut input, &mut spectrum, &mut forward_scratch)
                .map_err(|e| e.to_string())?;
            magnitudes.fill(0.0);
            frequencies.fill(0.0);
            for (bin, value) in spectrum.iter().enumerate() {
                let phase = value.arg();
                let expected = tau * bin as f32 * HOP as f32 / N as f32;
                let delta = (phase - previous[bin] - expected + std::f32::consts::PI)
                    .rem_euclid(tau)
                    - std::f32::consts::PI;
                previous[bin] = phase;
                let target = (bin as f32 * ratio).round() as usize;
                if target >= bins {
                    continue;
                }
                let magnitude = value.norm();
                magnitudes[target] += magnitude;
                frequencies[target] +=
                    magnitude * (bin as f32 + delta * N as f32 / (tau * HOP as f32)) * ratio;
                if center == 0 {
                    // Keep the first window centered on the original onset when
                    // moving energy between bins (especially important for short hits).
                    phases[target] = phase + std::f32::consts::PI * (bin as f32 - target as f32);
                }
            }
            for bin in 0..bins {
                if magnitudes[bin] > 1e-12 && center != 0 {
                    phases[bin] += tau * frequencies[bin] / magnitudes[bin] * HOP as f32 / N as f32;
                    phases[bin] = phases[bin].rem_euclid(tau);
                }
                shifted[bin] = Complex32::from_polar(magnitudes[bin], phases[bin]);
            }
            shifted[0].im = 0.0;
            shifted[bins - 1].im = 0.0;
            inverse
                .process_with_scratch(&mut shifted, &mut time, &mut inverse_scratch)
                .map_err(|e| e.to_string())?;
            for i in 0..N {
                let frame = start + i as isize;
                if frame < 0 || frame as usize >= frames {
                    continue;
                }
                let frame = frame as usize;
                output[frame * channels + channel] += time[i] / N as f32 * window[i];
                if channel == 0 {
                    weights[frame] += window[i] * window[i];
                }
            }
        }
    }
    for (frame, values) in output.chunks_exact_mut(channels).enumerate() {
        for value in values {
            *value /= weights[frame].max(1e-6);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transposition_preserves_duration_and_moves_frequency_through_two_octaves() {
        let rate = 48000.0;
        let source: Vec<_> = (0..24000)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / rate).sin() * 0.3)
            .collect();
        for semitones in [-24.0, -12.0, 12.0, 24.0] {
            let shifted = shift(&source, 1, semitones).unwrap();
            assert_eq!(shifted.len(), source.len());
            assert!(shifted.iter().all(|x| x.is_finite()));
            let mut planner = RealFftPlanner::<f32>::new();
            let fft = planner.plan_fft_forward(8192);
            let mut input = shifted[6000..6000 + 8192].to_vec();
            let mut spectrum = fft.make_output_vec();
            fft.process(&mut input, &mut spectrum).unwrap();
            let peak = spectrum
                .iter()
                .enumerate()
                .skip(1)
                .max_by(|(_, a), (_, b)| a.norm_sqr().total_cmp(&b.norm_sqr()))
                .unwrap()
                .0;
            let actual = peak as f32 * rate / 8192.0;
            let expected = 440.0 * 2.0_f32.powf(semitones / 12.0);
            assert!(
                (actual - expected).abs() < 12.0,
                "{semitones}: expected {expected}, found {actual}"
            );
        }
        assert_eq!(shift(&source, 1, 0.0).unwrap(), source);
    }
    #[test]
    fn stereo_alignment_short_hits_and_stale_preparation_are_safe() {
        let stereo: Vec<_> = (0..8000)
            .flat_map(|i| {
                let value = (i as f32 * 0.05).sin() * 0.3;
                [value, -value]
            })
            .collect();
        let shifted = shift(&stereo, 2, 7.0).unwrap();
        let error = shifted
            .chunks_exact(2)
            .map(|x| (x[0] + x[1]).powi(2))
            .sum::<f32>()
            / 8000.0;
        assert!(error.sqrt() < 0.01, "stereo phase error {error}");
        let short = shift(&[1.0; 16], 1, 12.0).unwrap();
        assert_eq!(short.len(), 16);
        assert!(short.iter().all(|x| x.is_finite()));
        assert!(short.iter().any(|x| x.abs() > 0.001));
        let mut sample = SlotSample::new("x.wav".into(), "x.wav".into(), None, 1.0);
        sample.cached_audio = Some(Arc::new(super::super::sample_pack::CachedAudio {
            channels: 2,
            sample_rate: 8000,
            samples: Arc::new(stereo),
            mono_samples: Arc::new(vec![]),
            waveform_peaks: vec![],
            waveform_colors: vec![],
        }));
        sample.pitch_semitones = 12.0;
        sample.prepare_pitch().unwrap();
        assert!(!sample.pitch_pending());
        sample.pitch_semitones = -12.0;
        assert!(sample.pitch_pending());
        assert!(super::super::sample_pack::SharedPcm::trimmed(&sample).is_none());
        sample.pitch_mode = PitchMode::Time;
        assert!(!sample.pitch_pending());
        assert_eq!(sample.speed(), 0.5);
    }
}
