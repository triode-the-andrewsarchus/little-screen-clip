//! Hybrid sample classifier combining physical DSP features, token priors,
//! and compact decision-tree ensembles for 2D audio constellation mapping.

use super::generated_model::{SampleCategory, predict_acoustic_probabilities};
use realfft::RealFftPlanner;
use std::collections::hash_map::DefaultHasher;
use std::f32::consts::PI;
use std::hash::{Hash, Hasher};
use std::path::Path;

pub struct SampleClassification {
    pub category: SampleCategory,
    pub confidence: f32,
    pub brightness: f32,
    pub pitch_hz: f32,
    pub pitch_clarity: f32,
    pub duration: f32,
    pub x: f32,
    pub y: f32,
}

pub struct SampleFeatures {
    pub values: [f32; 32],
    pub duration: f32,
    pub spectral_centroid: f32,
    pub pitch_hz: f32,
    pub pitch_clarity: f32,
}

/// Extracts the 32 physical acoustic DSP features from mono PCM samples.
pub fn extract_features(samples: &[f32], sample_rate: f32) -> Option<SampleFeatures> {
    if samples.is_empty()
        || !sample_rate.is_finite()
        || sample_rate <= 0.0
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return None;
    }

    let duration = samples.len() as f32 / sample_rate;
    if duration < 0.005 {
        return None;
    }

    // Find peak and normalize
    let mut peak = 0.0f32;
    for &s in samples {
        let abs = s.abs();
        if abs > peak {
            peak = abs;
        }
    }
    if peak < 1e-4 {
        return None; // Silence
    }

    let inv_peak = 1.0 / peak;
    let norm: Vec<f32> = samples.iter().map(|&s| s * inv_peak).collect();

    // 1. Temporal Envelope
    let mut peak_idx = 0;
    for (i, &s) in norm.iter().enumerate() {
        if s.abs() >= 0.999 {
            peak_idx = i;
            break;
        }
    }

    let mut attack_idx = peak_idx;
    for (i, &s) in norm[..=peak_idx].iter().enumerate() {
        if s.abs() >= 0.90 {
            attack_idx = i;
            break;
        }
    }
    let attack_time = attack_idx as f32 / sample_rate;

    let mut decay_idx = norm.len() - peak_idx;
    for (i, &s) in norm[peak_idx..].iter().enumerate() {
        if s.abs() < 0.10 {
            decay_idx = i;
            break;
        }
    }
    let decay_time = decay_idx as f32 / sample_rate;

    // RMS Energy & Temporal Centroid
    let mut sum_sq = 0.0f32;
    let mut time_energy_sum = 0.0f32;
    for (i, &s) in norm.iter().enumerate() {
        let sq = s * s;
        sum_sq += sq;
        let t = i as f32 / sample_rate;
        time_energy_sum += t * sq;
    }
    let rms = (sum_sq / norm.len() as f32).sqrt();
    let temporal_centroid = if sum_sq > 1e-9 {
        time_energy_sum / sum_sq
    } else {
        0.0
    };

    // 2. Zero Crossing Rate (ZCR)
    let mut zc_count = 0usize;
    let mut prev_sign = norm[0] >= 0.0;
    for &s in &norm[1..] {
        let sign = s >= 0.0;
        if sign != prev_sign {
            zc_count += 1;
            prev_sign = sign;
        }
    }
    let zcr = zc_count as f32 / norm.len() as f32;

    let head_25ms = (0.025 * sample_rate) as usize;
    let head_slice = &norm[..norm.len().min(head_25ms)];
    let mut head_zc = 0usize;
    if !head_slice.is_empty() {
        let mut prev = head_slice[0] >= 0.0;
        for &s in &head_slice[1..] {
            let sign = s >= 0.0;
            if sign != prev {
                head_zc += 1;
                prev = sign;
            }
        }
    }
    let zcr_head = head_zc as f32 / head_slice.len().max(1) as f32;

    // 3. FFT Spectral Features
    const FFT_SIZE: usize = 2048;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let mut in_buf = vec![0.0f32; FFT_SIZE];
    let window_len = norm.len().min(FFT_SIZE);
    for i in 0..window_len {
        // Hanning window
        let w = 0.5 * (1.0 - (2.0 * PI * i as f32 / window_len as f32).cos());
        in_buf[i] = norm[i] * w;
    }

    let mut out_buf = fft.make_output_vec();
    let _ = fft.process(&mut in_buf, &mut out_buf);

    let num_bins = out_buf.len(); // 1025
    let mut mag = Vec::with_capacity(num_bins);
    let mut power = Vec::with_capacity(num_bins);
    let mut spec_sum = 0.0f32;
    let mut total_power = 0.0f32;
    let mut weighted_freq_sum = 0.0f32;

    let bin_hz = sample_rate / FFT_SIZE as f32;

    let mut sub_power = 0.0f32;
    let mut low_power = 0.0f32;
    let mut mid_power = 0.0f32;
    let mut high_power = 0.0f32;

    for (k, c) in out_buf.iter().enumerate() {
        let m = (c.re * c.re + c.im * c.im).sqrt();
        let p = m * m + 1e-12;
        mag.push(m);
        power.push(p);

        spec_sum += m;
        total_power += p;

        let freq = k as f32 * bin_hz;
        weighted_freq_sum += freq * m;

        if freq >= 20.0 && freq < 100.0 {
            sub_power += p;
        } else if freq >= 100.0 && freq < 300.0 {
            low_power += p;
        } else if freq >= 300.0 && freq < 2500.0 {
            mid_power += p;
        } else if freq >= 2500.0 && freq <= 11000.0 {
            high_power += p;
        }
    }

    let spectral_centroid = if spec_sum > 1e-9 {
        weighted_freq_sum / spec_sum
    } else {
        0.0
    };

    // Spectral spread
    let mut spread_sq_sum = 0.0f32;
    for (k, &m) in mag.iter().enumerate() {
        let freq = k as f32 * bin_hz;
        let diff = freq - spectral_centroid;
        spread_sq_sum += diff * diff * m;
    }
    let spectral_spread = if spec_sum > 1e-9 {
        (spread_sq_sum / spec_sum).sqrt()
    } else {
        0.0
    };

    // Spectral rolloff (85% energy frequency)
    let rolloff_threshold = 0.85 * total_power;
    let mut cum_power = 0.0f32;
    let mut spectral_rolloff = (num_bins - 1) as f32 * bin_hz;
    for (k, &p) in power.iter().enumerate() {
        cum_power += p;
        if cum_power >= rolloff_threshold {
            spectral_rolloff = k as f32 * bin_hz;
            break;
        }
    }

    // Spectral flatness (Wiener entropy)
    let mut log_power_sum = 0.0f32;
    for &p in &power {
        log_power_sum += p.ln();
    }
    let geom_mean = (log_power_sum / num_bins as f32).exp();
    let arith_mean = total_power / num_bins as f32;
    let spectral_flatness = if arith_mean > 1e-9 {
        (geom_mean / arith_mean).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let sub_ratio = sub_power / total_power.max(1e-9);
    let low_ratio = low_power / total_power.max(1e-9);
    let mid_ratio = mid_power / total_power.max(1e-9);
    let high_ratio = high_power / total_power.max(1e-9);

    // Initial peak frequency (head 512 samples)
    let head_peak_freq = {
        let head_len = norm.len().min(512);
        let mut max_head_mag = 0.0f32;
        let mut max_head_bin = 0;
        let step = (num_bins as f32 / (head_len as f32 / 2.0)).max(1.0) as usize;
        for (k, &m) in mag.iter().step_by(step).enumerate() {
            if m > max_head_mag {
                max_head_mag = m;
                max_head_bin = k;
            }
        }
        max_head_bin as f32 * bin_hz * step as f32
    };

    // 4. Pitch & Pitch Clarity via Autocorrelation
    let (pitch_hz, pitch_clarity) = estimate_pitch(&norm, sample_rate);

    // 5. Transient Count
    let hop_size = 256;
    let num_hops = norm.len() / hop_size;
    let mut transients = 0.0f32;
    if num_hops > 8 {
        let mut hop_energies = Vec::with_capacity(num_hops);
        for i in 0..num_hops {
            let start = i * hop_size;
            let end = (start + hop_size).min(norm.len());
            let e: f32 = norm[start..end].iter().map(|&s| s * s).sum();
            hop_energies.push(e);
        }

        let mut diffs = Vec::with_capacity(num_hops.saturating_sub(1));
        for i in 0..hop_energies.len().saturating_sub(1) {
            diffs.push((hop_energies[i + 1] - hop_energies[i]).max(0.0));
        }

        let mean_diff = diffs.iter().sum::<f32>() / diffs.len().max(1) as f32;
        let thresh = mean_diff * 2.5 + 0.05;
        let mut count = 0;
        for &d in &diffs {
            if d > thresh {
                count += 1;
            }
        }
        transients = count as f32;
    }

    // 6. 13 MFCCs
    let mut mfcc = [0.0f32; 13];
    const NUM_MEL: usize = 26;
    let max_mel = 2595.0 * (1.0 + (sample_rate / 2.0) / 700.0).log10();
    let mut mel_energies = [0.0f32; NUM_MEL];

    let mut mel_points = [0.0f32; NUM_MEL + 2];
    for i in 0..NUM_MEL + 2 {
        let mel = (i as f32 / (NUM_MEL + 1) as f32) * max_mel;
        mel_points[i] = 700.0 * (10.0f32.powf(mel / 2595.0) - 1.0);
    }

    let mut bin_points = [0usize; NUM_MEL + 2];
    for i in 0..NUM_MEL + 2 {
        bin_points[i] = ((FFT_SIZE as f32 + 1.0) * mel_points[i] / sample_rate).floor() as usize;
    }

    for m in 1..=NUM_MEL {
        let f_prev = bin_points[m - 1];
        let f_curr = bin_points[m];
        let f_next = bin_points[m + 1];

        let mut e = 0.0f32;
        if f_curr > f_prev {
            for k in f_prev..f_curr.min(num_bins) {
                let weight = (k - f_prev) as f32 / (f_curr - f_prev) as f32;
                e += mag[k] * weight;
            }
        }
        if f_next > f_curr {
            for k in f_curr..f_next.min(num_bins) {
                let weight = (f_next - k) as f32 / (f_next - f_curr) as f32;
                e += mag[k] * weight;
            }
        }
        mel_energies[m - 1] = e.max(1e-5).ln();
    }

    // DCT-II
    for i in 0..13 {
        let mut sum = 0.0f32;
        for j in 0..NUM_MEL {
            sum += mel_energies[j] * ((PI / NUM_MEL as f32) * (j as f32 + 0.5) * i as f32).cos();
        }
        let scale = if i == 0 {
            (1.0 / NUM_MEL as f32).sqrt()
        } else {
            (2.0 / NUM_MEL as f32).sqrt()
        };
        mfcc[i] = sum * scale;
    }

    // Pack strictly into alphabetical feature vector of 32 elements:
    let values = [
        attack_time,       //  0: attack_time
        decay_time,        //  1: decay_time
        duration,          //  2: duration
        head_peak_freq,    //  3: head_peak_freq
        high_ratio,        //  4: high_ratio
        low_ratio,         //  5: low_ratio
        mfcc[0],           //  6: mfcc_1
        mfcc[9],           //  7: mfcc_10
        mfcc[10],          //  8: mfcc_11
        mfcc[11],          //  9: mfcc_12
        mfcc[12],          // 10: mfcc_13
        mfcc[1],           // 11: mfcc_2
        mfcc[2],           // 12: mfcc_3
        mfcc[3],           // 13: mfcc_4
        mfcc[4],           // 14: mfcc_5
        mfcc[5],           // 15: mfcc_6
        mfcc[6],           // 16: mfcc_7
        mfcc[7],           // 17: mfcc_8
        mfcc[8],           // 18: mfcc_9
        mid_ratio,         // 19: mid_ratio
        pitch_clarity,     // 20: pitch_clarity
        pitch_hz,          // 21: pitch_hz
        rms,               // 22: rms
        spectral_centroid, // 23: spectral_centroid
        spectral_flatness, // 24: spectral_flatness
        spectral_rolloff,  // 25: spectral_rolloff
        spectral_spread,   // 26: spectral_spread
        sub_ratio,         // 27: sub_ratio
        temporal_centroid, // 28: temporal_centroid
        transients,        // 29: transients
        zcr,               // 30: zcr
        zcr_head,          // 31: zcr_head
    ];

    Some(SampleFeatures {
        values,
        duration,
        spectral_centroid,
        pitch_hz,
        pitch_clarity,
    })
}

/// Estimates fundamental frequency and normalized autocorrelation over the first 100 ms.
fn estimate_pitch(samples: &[f32], sample_rate: f32) -> (f32, f32) {
    let first_100ms = (0.10 * sample_rate) as usize;
    let ac_slice = &samples[..samples.len().min(first_100ms)];
    let mut pitch_hz = 0.0f32;
    let mut pitch_clarity = 0.0f32;

    if ac_slice.len() > 128 {
        let min_lag = ((sample_rate / 1000.0) as usize).max(1);
        let max_lag = (sample_rate / 40.0) as usize;
        let end_lag = max_lag.min(ac_slice.len() / 2);

        let mut r0 = 0.0f32;
        for &s in ac_slice {
            r0 += s * s;
        }

        if r0 > 1e-6 && min_lag < end_lag {
            let mut best_lag = min_lag;
            let mut best_r = 0.0f32;

            for lag in min_lag..end_lag {
                let mut sum = 0.0f32;
                let lim = ac_slice.len() - lag;
                for i in 0..lim {
                    sum += ac_slice[i] * ac_slice[i + lag];
                }
                if sum > best_r {
                    best_r = sum;
                    best_lag = lag;
                }
            }

            pitch_clarity = (best_r / r0).clamp(0.0, 1.0);
            if pitch_clarity > 0.25 {
                pitch_hz = sample_rate / best_lag as f32;
            }
        }
    }

    (pitch_hz, pitch_clarity)
}

/// Returns an ordered filename or directory prior for a sample.
///
/// Filename hints take precedence over ancestry. These are weak naming heuristics;
/// [`classify_sample`] checks physical plausibility before combining them with the
/// acoustic model. Short `bd`/`hat` abbreviations require word boundaries to avoid
/// treating names such as `subdivision` and `what` as drum labels.
pub fn match_path_token(path: &Path) -> Option<(SampleCategory, f32)> {
    use SampleCategory::*;
    const FILENAMES: &[(SampleCategory, f32, &[&str])] = &[
        (Kick, 1.0, &["kick", "bassdrum"]),
        (Snare, 1.0, &["snare", "snr"]),
        (Clap, 1.0, &["clap", "clp", "handclap"]),
        (
            HihatOpen,
            1.0,
            &["open_hat", "openhat", "open_hh", "open-hh", "oh_", "oh "],
        ),
        (
            HihatClosed,
            0.9,
            &[
                "closed_hat",
                "closedhat",
                "closed_hh",
                "closed-hh",
                "ch_",
                "ch ",
                "hihat",
                "hi-hat",
                "hh",
            ],
        ),
        (Cymbal, 1.0, &["crash", "ride", "cymbal", "splash", "china"]),
        (Tom, 1.0, &["tom", "rototom", "floortom"]),
        (
            Percussion,
            1.0,
            &[
                "shaker",
                "tambourine",
                "conga",
                "bongo",
                "cowbell",
                "perc",
                "rim",
            ],
        ),
        (Bass, 0.95, &["808", "sub", "bass", "reese"]),
        (Loop, 0.95, &["loop", "bpm"]),
        (Vocal, 0.95, &["vocal", "vox", "acapella", "chant"]),
        (
            Melodic,
            0.95,
            &["synth", "lead", "pad", "chord", "stab", "pluck"],
        ),
    ];
    const FOLDERS: &[(SampleCategory, f32, &[&str])] = &[
        (Kick, 0.85, &["bd - bass drum", "/bd/", "/kick"]),
        (Snare, 0.85, &["sn - snare drum", "/sn/", "/snare"]),
        (Clap, 0.85, &["hc - hand clap", "/clap"]),
        (HihatClosed, 0.80, &["ch - closed hat", "/hats"]),
        (HihatOpen, 0.85, &["oh - open hat"]),
        (
            Cymbal,
            0.85,
            &["cc - crash cymbal", "rc - ride cymbal", "/ride_cymbal"],
        ),
        (
            Tom,
            0.85,
            &["ht - hi tom", "lt - low tom", "mt - mid tom", "/toms"],
        ),
        (Percussion, 0.80, &["/percussion", "shaker"]),
    ];
    let filename = path.file_name()?.to_string_lossy().to_lowercase();
    let words: Vec<_> = filename.split(|c: char| !c.is_ascii_alphabetic()).collect();
    if words.contains(&"bd") {
        return Some((Kick, 1.0));
    }
    let special = |category| match category {
        Snare => filename.starts_with("sd_") || filename.starts_with("sd "),
        HihatOpen => {
            filename.contains("open") && (words.contains(&"hat") || filename.contains("hh"))
        }
        HihatClosed => words.contains(&"hat"),
        _ => false,
    };
    if let Some((category, weight, _)) = FILENAMES.iter().find(|(category, _, patterns)| {
        patterns.iter().any(|pattern| filename.contains(pattern)) || special(*category)
    }) {
        return Some((*category, *weight));
    }
    let lower = path.to_string_lossy().to_lowercase();
    FOLDERS
        .iter()
        .find(|(_, _, patterns)| patterns.iter().any(|pattern| lower.contains(pattern)))
        .map(|(category, weight, _)| (*category, *weight))
}

/// Classifies a sample using our full hybrid ensemble.
pub fn classify_sample(
    path: &Path,
    samples: &[f32],
    sample_rate: f32,
) -> Option<SampleClassification> {
    let feat = extract_features(samples, sample_rate)?;

    // 1. ML probability distribution across all 12 classes
    let ml_probs = predict_acoustic_probabilities(&feat.values);

    // 2. Token Prior
    let token_hint = match_path_token(path);

    // 3. Physical Guardrails & Score Combination
    let mut scores = [0.0f32; 12];
    for (i, &p) in ml_probs.iter().enumerate() {
        scores[i] = p;
    }

    // Apply token prior if physically plausible
    if let Some((hint_cat, hint_weight)) = token_hint {
        let idx = hint_cat as usize;
        let mut plausible = true;

        let sc = feat.spectral_centroid;
        let sub = feat.values[27]; // sub_ratio
        let duration = feat.duration;

        if hint_cat == SampleCategory::Kick && (sc > 3500.0 || (sub < 0.05 && sc > 2000.0)) {
            plausible = false;
        } else if hint_cat == SampleCategory::HihatClosed && duration > 1.2 {
            plausible = false;
        } else if hint_cat == SampleCategory::Bass && sub + feat.values[5] < 0.08 {
            plausible = false;
        }

        if plausible {
            scores[idx] += 0.85 * hint_weight;
        } else {
            scores[idx] -= 0.50;
        }
    }

    // Hard physical overrides
    // Extended duration with multiple beats is almost certainly a loop
    if feat.duration > 2.5 && feat.values[29] >= 3.0 {
        scores[SampleCategory::Loop as usize] += 1.5;
    }

    // Find best class
    let mut best_idx = 0;
    let mut best_score = -100.0f32;
    for (i, &s) in scores.iter().enumerate() {
        if s > best_score {
            best_score = s;
            best_idx = i;
        }
    }

    let category = SampleCategory::ALL[best_idx];
    let confidence =
        (best_score / (scores.iter().map(|&s| s.max(0.0)).sum::<f32>() + 1e-6)).clamp(0.1, 1.0);

    // 4. Compute 2D Constellation Coordinates (Organic Continuous Style)
    let (x, y) = calculate_organic_coordinates(
        category,
        feat.spectral_centroid,
        feat.duration,
        feat.pitch_clarity,
        &path.to_string_lossy(),
    );

    Some(SampleClassification {
        category,
        confidence,
        brightness: feat.spectral_centroid,
        pitch_hz: feat.pitch_hz,
        pitch_clarity: feat.pitch_clarity,
        duration: feat.duration,
        x,
        y,
    })
}

/// Computes continuous, organic 2D constellation coordinates.
/// Free of rectangular walls or hard boundary clamping.
pub fn calculate_organic_coordinates(
    category: SampleCategory,
    brightness: f32,
    duration: f32,
    pitch_clarity: f32,
    path_hint: &str,
) -> (f32, f32) {
    // Celestial clump archipelago configuration:
    // (center_x, center_y, radius_x, radius_y, tilt_radians)
    let (cx, cy, rx, ry, tilt): (f32, f32, f32, f32, f32) = match category {
        SampleCategory::Kick => (-0.85, -0.15, 0.36, 0.44, 0.10),
        SampleCategory::Bass => (-0.70, -0.65, 0.35, 0.34, -0.15),
        SampleCategory::Tom => (-0.48, 0.40, 0.34, 0.36, 0.25),
        SampleCategory::Percussion => (-0.10, 0.52, 0.36, 0.38, 0.15),
        SampleCategory::Snare => (-0.20, -0.08, 0.36, 0.40, 0.05),
        SampleCategory::Clap => (0.12, -0.18, 0.32, 0.34, -0.10),
        SampleCategory::HihatClosed => (0.38, 0.45, 0.34, 0.36, -0.20),
        SampleCategory::HihatOpen => (0.68, 0.25, 0.32, 0.34, 0.05),
        SampleCategory::Cymbal => (0.90, -0.05, 0.36, 0.38, 0.30),
        SampleCategory::Vocal => (0.02, -0.52, 0.32, 0.32, -0.05),
        SampleCategory::Melodic => (0.50, -0.58, 0.44, 0.38, 0.08),
        SampleCategory::Loop => (0.85, -0.42, 0.38, 0.36, -0.12),
    };

    // 1. Continuous smooth acoustic modifiers (logarithmic, hyperbolic tangent)
    let bright_clamped = brightness.max(80.0);
    let bright_log = (bright_clamped.ln() - (3000.0_f32).ln()) / 2.2;
    let u_bright = (bright_log * 0.7).tanh();

    let dur_clamped = duration.max(0.04);
    let dur_log = (dur_clamped.ln() - (0.45_f32).ln()) / 1.8;
    let u_dur = (dur_log * 0.7).tanh();

    let u_clarity = ((pitch_clarity - 0.5) * 1.5).tanh();

    // 2. Deterministic pseudo-random micro-diffusion (Box-Muller Gaussian)
    let mut hasher = DefaultHasher::new();
    path_hint.hash(&mut hasher);
    let h1 = hasher.finish();

    let mut hasher2 = DefaultHasher::new();
    (h1, 0x9e3779b97f4a7c15_u64).hash(&mut hasher2);
    let h2 = hasher2.finish();

    let f1 = ((h1 as f64) / (u64::MAX as f64)).abs().max(0.0001) as f32;
    let f2 = ((h2 as f64) / (u64::MAX as f64)).abs() as f32;

    let r_noise = (-2.0 * f1.ln()).sqrt() * 0.52;
    let theta_noise = f2 * 2.0 * PI;

    // Local elliptical displacement
    let lx = (u_bright * 0.52 + r_noise * theta_noise.cos()) * rx;
    let ly = (u_dur * 0.52 + u_clarity * 0.20 + r_noise * theta_noise.sin()) * ry;

    // Apply cloud tilt
    let (sin_t, cos_t) = tilt.sin_cos();
    let dx = lx * cos_t - ly * sin_t;
    let dy = lx * sin_t + ly * cos_t;

    // Organic unconstrained coordinates: natural Gaussian splatter with zero hard boundaries or arcs
    (cx + dx, cy + dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_audio_is_not_classified() {
        assert!(extract_features(&[f32::NAN; 256], 44100.0).is_none());
        assert!(extract_features(&[f32::INFINITY; 256], 44100.0).is_none());
        assert!(extract_features(&[0.5; 256], f32::NAN).is_none());
        let features = extract_features(&[0.5; 256], 500.0).unwrap();
        assert!(features.values.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn short_drum_abbreviations_do_not_match_unrelated_words() {
        assert_eq!(match_path_token(Path::new("what.wav")), None);
        assert_eq!(
            match_path_token(Path::new("subdivision.wav")).unwrap().0,
            SampleCategory::Bass
        );
        assert_eq!(
            match_path_token(Path::new("BD_909.wav")).unwrap().0,
            SampleCategory::Kick
        );
        assert_eq!(
            match_path_token(Path::new("hat02.wav")).unwrap().0,
            SampleCategory::HihatClosed
        );
    }

    #[test]
    fn test_token_hints() {
        assert_eq!(
            match_path_token(Path::new("deep_808_kick.wav")).map(|(cat, _)| cat),
            Some(SampleCategory::Kick)
        );
        assert_eq!(
            match_path_token(Path::new("crispy_snare_drum.wav")).map(|(cat, _)| cat),
            Some(SampleCategory::Snare)
        );
        assert_eq!(
            match_path_token(Path::new("tight_closed_hat.wav")).map(|(cat, _)| cat),
            Some(SampleCategory::HihatClosed)
        );
        assert_eq!(
            match_path_token(Path::new("open_hh_909.wav")).map(|(cat, _)| cat),
            Some(SampleCategory::HihatOpen)
        );
        assert_eq!(
            match_path_token(Path::new("sub_bass_c.wav")).map(|(cat, _)| cat),
            Some(SampleCategory::Bass)
        );
        assert_eq!(
            match_path_token(Path::new("vocal_chop_lead.wav")).map(|(cat, _)| cat),
            Some(SampleCategory::Vocal)
        );
    }

    #[test]
    fn test_dsp_feature_extraction_and_classification() {
        let sr = 44100.0f32;
        let num_samples = (sr * 0.4) as usize; // 400ms

        // Generate synthetic kick (sine wave with pitch drop and exponential decay)
        let mut kick_samples = Vec::with_capacity(num_samples);
        let mut phase = 0.0f32;
        for i in 0..num_samples {
            let t = i as f32 / sr;
            let freq = 120.0 * (-t * 15.0).exp() + 50.0;
            phase += 2.0 * PI * freq / sr;
            let env = (-t * 8.0).exp();
            kick_samples.push(phase.sin() * env);
        }

        let features = extract_features(&kick_samples, sr).expect("Failed to extract features");
        assert!(
            features.spectral_centroid < 800.0,
            "Centroid should be low: {}",
            features.spectral_centroid
        );
        assert!(features.duration >= 0.35);

        // Classify without name hint
        let classification = classify_sample(Path::new("sample_001.wav"), &kick_samples, sr)
            .expect("Classification failed");
        assert!(
            classification.category == SampleCategory::Kick
                || classification.category == SampleCategory::Bass
                || classification.category == SampleCategory::Tom,
            "Expected low percussive/bass category, got {:?}",
            classification.category
        );
        assert!(classification.x >= -1.75 && classification.x <= 1.75);
        assert!(classification.y >= -1.50 && classification.y <= 1.50);
    }
}
