//! Pitch detection (YIN), transient detection, harmonic chord analysis, and colormaps.

use super::types::SingleNoteInfo;
use fearless_simd::{Level, Simd, dispatch, prelude::*};
use fearless_simd_macros::simd;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn inferno_colormap(t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32, f: f32| a + (b - a) * f;
    let (r, g, b) = if t < 0.25 {
        let f = t / 0.25;
        (
            lerp(10.0, 50.0, f),
            lerp(12.0, 15.0, f),
            lerp(26.0, 95.0, f),
        )
    } else if t < 0.50 {
        let f = (t - 0.25) / 0.25;
        (
            lerp(50.0, 180.0, f),
            lerp(15.0, 30.0, f),
            lerp(95.0, 70.0, f),
        )
    } else if t < 0.75 {
        let f = (t - 0.50) / 0.25;
        (
            lerp(180.0, 245.0, f),
            lerp(30.0, 130.0, f),
            lerp(70.0, 15.0, f),
        )
    } else {
        let f = (t - 0.75) / 0.25;
        (
            lerp(245.0, 255.0, f),
            lerp(130.0, 255.0, f),
            lerp(15.0, 200.0, f),
        )
    };
    [r as u8, g as u8, b as u8, 255]
}

pub const CHROMATIC_NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn freq_to_note(freq: f32) -> (&'static str, i32, f32) {
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let rounded_midi = midi.round() as i32;
    let note_idx = rounded_midi.rem_euclid(12) as usize;
    let octave = (rounded_midi / 12) - 1;
    (CHROMATIC_NOTE_NAMES[note_idx], octave, midi)
}

/// YIN fundamental frequency pitch detector (De Cheveigné and Kawahara).
/// Accurately detects pitches from C1 (~32.7 Hz) up to C8 (>4186 Hz) with sub-cent accuracy.
pub fn detect_pitch_yin(samples: &[f32], sample_rate: f32) -> Option<(f32, f32)> {
    dispatch!(Level::new(), simd => detect_pitch_yin_simd(simd, samples, sample_rate))
}

#[simd]
fn detect_pitch_yin_simd<S: Simd>(
    simd: S,
    samples: &[f32],
    sample_rate: f32,
) -> Option<(f32, f32)> {
    let min_freq = 25.0f32; // Allow detecting down to 25 Hz so true fundamental is identified before thresholding
    let max_freq = 4200.0f32; // Up to C8

    let min_tau = (sample_rate / max_freq).floor() as usize;
    let max_tau = (sample_rate / min_freq).ceil() as usize;

    if samples.len() <= max_tau {
        return None;
    }

    let window_size = 1024.min(samples.len() - max_tau);
    if window_size < 128 {
        return None;
    }

    let mut d = vec![0.0f32; max_tau + 1];
    let mut d_prime = vec![0.0f32; max_tau + 1];
    d_prime[0] = 1.0;
    let mut running_sum = 0.0f32;

    let threshold = 0.20f32;
    let mut tau_found = 0;

    // The tau loop stays sequential because d'(tau) depends on the cumulative sum
    // and its early-exit condition depends on preceding lags. Vectorize the inner
    // sample-wise difference reduction instead.
    for tau in 1..=max_tau {
        let mut sum_lanes = S::f32s::splat(simd, 0.0);
        let s1 = &samples[0..window_size];
        let s2 = &samples[tau..tau + window_size];
        let lane_count = S::f32s::LEN;
        let mut chunks1 = s1.chunks_exact(lane_count);
        let mut chunks2 = s2.chunks_exact(lane_count);
        for (chunk1, chunk2) in chunks1.by_ref().zip(chunks2.by_ref()) {
            let diff = S::f32s::from_slice(simd, chunk1) - S::f32s::from_slice(simd, chunk2);
            sum_lanes += diff * diff;
        }
        let mut sum = sum_lanes.reduce_sum();
        for (&a, &b) in chunks1.remainder().iter().zip(chunks2.remainder()) {
            let diff = a - b;
            sum += diff * diff;
        }
        d[tau] = sum;
        running_sum += sum;

        if running_sum > 0.0 {
            d_prime[tau] = d[tau] * (tau as f32) / running_sum;
        } else {
            d_prime[tau] = 1.0;
        }

        if tau >= min_tau {
            if tau_found == 0 && d_prime[tau] < threshold {
                tau_found = tau;
            } else if tau_found > 0 {
                if d_prime[tau] < d_prime[tau_found] {
                    tau_found = tau;
                } else if d_prime[tau] > d_prime[tau_found] * 1.05 {
                    // Local minimum reached, break early!
                    break;
                }
            }
        }
    }

    // Fallback: global minimum if no dip below threshold
    if tau_found == 0 {
        let mut min_val = 1.0f32;
        let mut min_tau_idx = 0;
        for tau in min_tau..=max_tau {
            if d_prime[tau] < min_val {
                min_val = d_prime[tau];
                min_tau_idx = tau;
            }
        }
        if min_val < 0.40 && min_tau_idx > 0 {
            tau_found = min_tau_idx;
        } else {
            return None;
        }
    }

    // Parabolic interpolation for sub-sample accuracy
    let x0 = tau_found.saturating_sub(1);
    let x1 = tau_found;
    let x2 = (tau_found + 1).min(max_tau);
    let y0 = d_prime[x0];
    let y1 = d_prime[x1];
    let y2 = d_prime[x2];

    let denom = 2.0 * (2.0 * y1 - y0 - y2);
    let delta = if denom.abs() > 1e-6 {
        (y0 - y2) / denom
    } else {
        0.0
    };
    let refined_tau = tau_found as f32 + delta.clamp(-0.5, 0.5);

    if refined_tau <= 0.0 {
        return None;
    }

    let freq = sample_rate / refined_tau;
    let clarity = (1.0 - d_prime[tau_found]).clamp(0.0, 1.0);
    Some((freq, clarity))
}

/// Lowest sensitivity exposed by the audio editor control.
pub const MIN_TRANSIENT_SENSITIVITY: f32 = 0.2;
/// Initial sensitivity used by analysis and the audio editor control.
pub const DEFAULT_TRANSIENT_SENSITIVITY: f32 = 0.35;
/// Highest sensitivity exposed by the audio editor control.
pub const MAX_TRANSIENT_SENSITIVITY: f32 = 1.0;

/// Logarithmic spectral flux transient onset detector.
/// Detects drum hits, attacks, and percussive transients across loud and quiet passages.
pub fn detect_transients(samples: &[f32], sample_rate: f32) -> Vec<f32> {
    detect_transients_with_sensitivity(samples, sample_rate, DEFAULT_TRANSIENT_SENSITIVITY)
}

/// Logarithmic spectral flux transient onset detector with an adjustable sensitivity factor.
pub fn detect_transients_with_sensitivity(
    samples: &[f32],
    sample_rate: f32,
    sensitivity: f32,
) -> Vec<f32> {
    detect_transients_with_sensitivity_cancellable(samples, sample_rate, sensitivity, None)
        .unwrap_or_default()
}

pub(crate) fn detect_transients_with_sensitivity_cancellable(
    samples: &[f32],
    sample_rate: f32,
    sensitivity: f32,
    cancelled: Option<&AtomicBool>,
) -> Option<Vec<f32>> {
    let is_cancelled = || cancelled.is_some_and(|flag| flag.load(Ordering::Acquire));
    if samples.is_empty() || sample_rate <= 0.0 {
        return Some(Vec::new());
    }

    let hop_size = 256;
    let num_hops = samples.len() / hop_size;
    if num_hops < 4 {
        return Some(Vec::new());
    }

    let mut odf = Vec::with_capacity(num_hops);
    let mut prev_l_hf = 0.0f32;
    let mut prev_l_tot = 0.0f32;

    for k in 0..num_hops {
        if k.is_multiple_of(256) && is_cancelled() {
            return None;
        }
        let start = k * hop_size;
        let end = (start + hop_size).min(samples.len());
        let chunk = &samples[start..end];

        let mut total_e = 0.0f32;
        let mut hf_e = 0.0f32;
        let mut prev_s = if start > 0 { samples[start - 1] } else { 0.0 };

        for &s in chunk {
            total_e += s * s;
            let diff = s - prev_s;
            hf_e += diff * diff;
            prev_s = s;
        }

        let l_tot = (1.0 + 1000.0 * (total_e / chunk.len() as f32).sqrt()).ln();
        let l_hf = (1.0 + 1000.0 * (hf_e / chunk.len() as f32).sqrt()).ln();

        let delta_hf = (l_hf - prev_l_hf).max(0.0);
        let delta_tot = (l_tot - prev_l_tot).max(0.0);
        let flux = delta_hf + 0.6 * delta_tot;
        odf.push(flux);

        prev_l_hf = l_hf;
        prev_l_tot = l_tot;
    }

    let min_interval_hops = (0.035 * sample_rate / hop_size as f32).max(2.0) as usize;
    let window_radius = 8;
    let mut transients = Vec::new();
    let mut last_transient_hop = 0usize;
    let sens = sensitivity.clamp(0.05, 5.0);

    for k in 1..num_hops.saturating_sub(1) {
        if k.is_multiple_of(256) && is_cancelled() {
            return None;
        }
        let w_start = k.saturating_sub(window_radius);
        let w_end = (k + window_radius + 1).min(num_hops);
        let mut local_vals = odf[w_start..w_end].to_vec();
        local_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = local_vals[local_vals.len() / 2];
        let mean: f32 = local_vals.iter().sum::<f32>() / local_vals.len() as f32;

        let threshold = (median * 1.5 + mean * 0.4 + 0.15) / sens;

        if odf[k] > threshold && odf[k] > odf[k - 1] && odf[k] >= odf[k + 1] {
            if transients.is_empty() || k.saturating_sub(last_transient_hop) >= min_interval_hops {
                let s_start = k * hop_size;
                let s_end = (s_start + hop_size).min(samples.len());
                let mut peak_sample = s_start;
                let mut peak_val = 0.0f32;
                for i in s_start..s_end {
                    let v = samples[i].abs();
                    if v > peak_val {
                        peak_val = v;
                        peak_sample = i;
                    }
                }

                let t = peak_sample as f32 / sample_rate;
                transients.push(t);
                last_transient_hop = k;
            }
        }
    }

    Some(transients)
}

/// Extracts all active notes (chords and single notes) from STFT frequency bins
/// using ColorChord's BaseNoteFinder algorithms (24-bin chromatic octave,
/// frequency slope amplification, circular neighbor smoothing, DecomposeHistogram
/// sub-bin interpolation, and non-linear amplitude compression).
pub fn detect_chord_from_fft(
    out_data: &[realfft::num_complex::Complex<f32>],
    sample_rate: f32,
    window_size: usize,
) -> Vec<SingleNoteInfo> {
    const BINS_PER_OCTAVE: usize = 24;
    const OCTAVE_FILTER_ITERATIONS: usize = 2;
    const OCTAVE_FILTER_STRENGTH: f32 = 0.5;
    const PEAK_COMPRESS_COEFF: f32 = 1.0;
    const PEAK_COMPRESS_EXP: f32 = 0.5;
    const MIN_NOTE_COMBINE_DIST: f32 = 0.5; // in 24-bin units (0.25 semitones / 25 cents)
    const NOTE_OUTPUT_CHOP: f32 = 0.04;
    const MIN_NOTE_CREATION_AMP: f32 = 0.02;

    let num_bins = out_data.len();
    let mut chroma = [0.0f32; BINS_PER_OCTAVE];
    let mut chroma_octave_mag = [[0.0f32; 9]; BINS_PER_OCTAVE];
    let mut total_note_energy = 0.0f32;
    let mut total_full_energy = 0.0f32;

    for b in 1..num_bins {
        let freq = b as f32 * sample_rate / window_size as f32;
        let mag = (out_data[b].re * out_data[b].re + out_data[b].im * out_data[b].im).sqrt();
        let energy = mag * mag;
        total_full_energy += energy;

        if freq < 65.0 || freq > 8000.0 {
            continue;
        }
        total_note_energy += energy;

        // Apply DFTSlope frequency-dependent amplification to compensate for 1/f falloff
        // and balance higher melody/harmony frequencies with low bass
        let slope_gain = 1.0 + 0.12 * (freq / 100.0).ln_1p().max(0.0);
        let weighted_mag = mag * slope_gain;

        // Map frequency to ColorChord 24-bin chromatic octave (A4 = 69 = bin 0, yellow)
        let midi = 69.0 + 12.0 * (freq / 440.0).log2();
        let bin_float = ((midi - 69.0) * 2.0).rem_euclid(BINS_PER_OCTAVE as f32);
        let n0 = (bin_float.floor() as usize) % BINS_PER_OCTAVE;
        let n1 = (n0 + 1) % BINS_PER_OCTAVE;
        let frac = bin_float - bin_float.floor();

        chroma[n0] += weighted_mag * (1.0 - frac);
        chroma[n1] += weighted_mag * frac;

        let oct = ((midi / 12.0).floor() as i32 - 1).clamp(0, 8) as usize;
        chroma_octave_mag[n0][oct] += weighted_mag * (1.0 - frac);
        chroma_octave_mag[n1][oct] += weighted_mag * frac;
    }

    let max_chroma = chroma.iter().cloned().fold(0.0f32, f32::max);
    if max_chroma < 0.1
        || total_note_energy < 1e-4
        || (total_full_energy > 1e-4 && total_note_energy / total_full_energy < 0.25)
    {
        return Vec::new();
    }

    // ColorChord Step 1: Neighbor filtering (circular smoothing across the 24 folded bins)
    let mut filtered_chroma = chroma;
    for _ in 0..OCTAVE_FILTER_ITERATIONS {
        let pre = filtered_chroma;
        for c in 0..BINS_PER_OCTAVE {
            let right = pre[(c + 1) % BINS_PER_OCTAVE];
            let left = pre[(c + BINS_PER_OCTAVE - 1) % BINS_PER_OCTAVE];
            filtered_chroma[c] = pre[c] * (1.0 - OCTAVE_FILTER_STRENGTH)
                + 0.5 * (left + right) * OCTAVE_FILTER_STRENGTH;
        }
    }

    // ColorChord Step 2: Peak detection with sub-bin interpolation (DecomposeHistogram in Turbo mode)
    struct RawDist {
        mean: f32,
        amplitude: f32,
    }
    let mut dists = Vec::with_capacity(12);

    for c in 0..BINS_PER_OCTAVE {
        let left = filtered_chroma[(c + BINS_PER_OCTAVE - 1) % BINS_PER_OCTAVE];
        let here = filtered_chroma[c];
        let right = filtered_chroma[(c + 1) % BINS_PER_OCTAVE];

        if left >= here || right > here {
            continue;
        }
        if left == here && right == here {
            continue;
        }

        let total_adj_diff = (here - left) + (here - right);
        if total_adj_diff <= 1e-9 {
            continue;
        }
        let prop_diff_left = (here - left) / total_adj_diff;
        let prop_diff_right = (here - right) / total_adj_diff;
        let internal_offset = if prop_diff_left < prop_diff_right {
            -(0.5 - prop_diff_left)
        } else {
            0.5 - prop_diff_right
        };

        let mean = (c as f32 + internal_offset).rem_euclid(BINS_PER_OCTAVE as f32);
        dists.push(RawDist {
            mean,
            amplitude: here * 4.0,
        });
    }

    if dists.is_empty() {
        return Vec::new();
    }

    // ColorChord Step 3: Normalize and non-linearly compress peak amplitudes
    let amp_sum: f32 = dists.iter().map(|d| d.amplitude).sum();
    if amp_sum > 1e-6 {
        let coeff = PEAK_COMPRESS_COEFF / (amp_sum * PEAK_COMPRESS_COEFF).powf(PEAK_COMPRESS_EXP);
        for d in &mut dists {
            d.amplitude *= coeff;
        }
    }
    dists.sort_by(|a, b| {
        b.amplitude
            .partial_cmp(&a.amplitude)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ColorChord Step 4: Combine note peaks that are within MinNoteCombineDistance (0.5 bins)
    fn circular_dist(a: f32, b: f32, length: f32) -> f32 {
        let mut d = (a - b).abs() % length;
        if d > length / 2.0 {
            d = length - d;
        }
        d
    }

    fn circular_weighted_avg(
        pos_a: f32,
        weight_a: f32,
        pos_b: f32,
        weight_b: f32,
        length: f32,
    ) -> f32 {
        let weight_sum = weight_a + weight_b;
        if weight_sum <= 1e-9 {
            return pos_a % length;
        }
        let mut a = pos_a;
        let mut b = pos_b;
        if (a - b).abs() > length / 2.0 {
            if a < b {
                a += length;
            } else {
                b += length;
            }
        }
        let mid = (a * weight_a + b * weight_b) / weight_sum;
        mid.rem_euclid(length)
    }

    let mut combined: Vec<RawDist> = Vec::new();
    for d in dists {
        if let Some(existing) = combined
            .iter_mut()
            .find(|e| circular_dist(e.mean, d.mean, BINS_PER_OCTAVE as f32) < MIN_NOTE_COMBINE_DIST)
        {
            existing.mean = circular_weighted_avg(
                existing.mean,
                existing.amplitude,
                d.mean,
                d.amplitude,
                BINS_PER_OCTAVE as f32,
            );
            existing.amplitude += d.amplitude;
        } else {
            combined.push(d);
        }
    }

    // ColorChord Step 5: Output chop and thresholding
    let mut detected = Vec::new();
    for d in combined {
        let final_amp = d.amplitude - NOTE_OUTPUT_CHOP;
        if final_amp <= MIN_NOTE_CREATION_AMP {
            continue;
        }

        let mean_pos = d.mean.rem_euclid(BINS_PER_OCTAVE as f32);
        // In ColorChord: Mean = 0 is A (Yellow).
        let fine_semitone = mean_pos / 2.0; // 0.0 is A, 1.0 is A#, etc.
        let note_id = (mean_pos / BINS_PER_OCTAVE as f32).rem_euclid(1.0); // 0.0 is A (Yellow)

        // Semitone index 0 is A -> MIDI class 9 (A).
        let semitone_from_c = (fine_semitone.round() as i32 + 9).rem_euclid(12);

        // Find predominant octave from FFT energy across octaves 2..7 for this note class
        let mut best_oct = 4;
        let mut best_oct_mag = 0.0f32;
        for oct_idx in 2..=7 {
            let f = 440.0 * 2.0f32.powf(((oct_idx + 1) * 12 + semitone_from_c - 69) as f32 / 12.0);
            if f < 65.0 {
                continue;
            }
            let b_float = f * window_size as f32 / sample_rate;
            let b0 = (b_float.floor() as usize).min(num_bins - 1);
            let b1 = (b0 + 1).min(num_bins - 1);
            let frac = b_float - b_float.floor();
            let mag0 =
                (out_data[b0].re * out_data[b0].re + out_data[b0].im * out_data[b0].im).sqrt();
            let mag1 =
                (out_data[b1].re * out_data[b1].re + out_data[b1].im * out_data[b1].im).sqrt();
            let interp_mag = mag0 * (1.0 - frac) + mag1 * frac;
            if interp_mag > best_oct_mag {
                best_oct_mag = interp_mag;
                best_oct = oct_idx;
            }
        }

        // Base MIDI note for this note class in best_oct
        let base_midi = (best_oct + 1) * 12 + semitone_from_c;
        // Nominal ColorChord bin for this MIDI note
        let nominal_bin = (((base_midi - 69) as f32) * 2.0).rem_euclid(BINS_PER_OCTAVE as f32);
        // Sub-bin deviation from nominal bin (-0.5 .. +0.5 bins = -25 .. +25 cents)
        let mut bin_diff = (mean_pos - nominal_bin).rem_euclid(BINS_PER_OCTAVE as f32);
        if bin_diff > (BINS_PER_OCTAVE as f32 / 2.0) {
            bin_diff -= BINS_PER_OCTAVE as f32;
        }
        let full_midi = base_midi as f32 + bin_diff * 0.5;
        let approx_freq = 440.0 * 2.0f32.powf((full_midi - 69.0) / 12.0);

        if approx_freq < 65.0 {
            continue;
        }

        let (note_class, octave_num, _) = freq_to_note(approx_freq);
        let note_name = format!("{}{}", note_class, octave_num);

        let amp = final_amp.clamp(0.25, 1.0);
        let rgb_f = rustchord::cc_to_rgb(note_id, 1.0, amp);
        let rgb = [
            (rgb_f[0] * 255.0).clamp(0.0, 255.0) as u8,
            (rgb_f[1] * 255.0).clamp(0.0, 255.0) as u8,
            (rgb_f[2] * 255.0).clamp(0.0, 255.0) as u8,
        ];

        detected.push(SingleNoteInfo {
            note_name,
            frequency_hz: approx_freq,
            amplitude: amp,
            rgb,
        });
    }

    detected.sort_by(|a, b| {
        b.amplitude
            .partial_cmp(&a.amplitude)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    detected.truncate(6);
    detected
}

#[cfg(test)]
mod yin_tests {
    use super::detect_pitch_yin;

    fn harmonic_tone(frequency: f32) -> Vec<f32> {
        let sample_rate = 44_100.0;
        (0..4096)
            .map(|i| {
                let t = i as f32 / sample_rate;
                (std::f32::consts::TAU * frequency * t).sin() * 0.75
                    + (std::f32::consts::TAU * frequency * 2.0 * t).sin() * 0.25
            })
            .collect()
    }

    #[test]
    fn yin_detects_fundamentals_across_its_range() {
        for expected in [55.0, 110.0, 220.0, 440.0, 1000.0, 4000.0] {
            let samples = harmonic_tone(expected);
            let (detected, clarity) = detect_pitch_yin(&samples, 44_100.0)
                .unwrap_or_else(|| panic!("failed to detect {expected} Hz"));
            assert!(
                (detected - expected).abs() < (expected * 0.005).max(1.0),
                "expected {expected} Hz, got {detected} Hz"
            );
            assert!(
                clarity > 0.9,
                "unexpected clarity {clarity} at {expected} Hz"
            );
        }
    }

    #[test]
    fn yin_rejects_silence_noise_and_too_short_inputs() {
        let silence = vec![0.0; 4096];
        assert_eq!(detect_pitch_yin(&silence, 44_100.0), None);

        let mut state = 0x1234_5678_u32;
        let noise: Vec<f32> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0
            })
            .collect();
        assert_eq!(detect_pitch_yin(&noise, 44_100.0), None);
        assert_eq!(detect_pitch_yin(&[0.0; 128], 44_100.0), None);
    }
}
