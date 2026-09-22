//! High-frequency and ultrasonic audio DSP operations.
//!
//! Provides time-domain Overlap-Add (OLA) pitch shifting for transposing ultrasonic
//! recordings down to human hearing range, ultrasonic high-pass filtering, and
//! energy-based Region of Interest (ROI) detection for fast navigation to ultrasonic bursts.

use little_clip_core::audio::analysis::inferno_colormap;
use rayon::prelude::*;
use realfft::RealFftPlanner;
use std::f32::consts::PI;

/// Configuration settings for rendering an ultrasonic spectrogram.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HfSpectrogramConfig {
    /// FFT window size (e.g. 256, 512, 1024, 2048, 4096).
    pub fft_size: usize,
    /// Gain in decibels applied before colormap mapping (e.g. -20.0..+40.0 dB).
    pub gain_db: f32,
    /// Dynamic range in decibels (e.g. 40.0..120.0 dB).
    pub range_db: f32,
    /// Contrast exponent applied to normalized intensity (e.g. 0.2..3.0).
    pub contrast: f32,
}

impl Default for HfSpectrogramConfig {
    fn default() -> Self {
        Self {
            fft_size: 1024,
            gain_db: 0.0,
            range_db: 80.0,
            contrast: 1.0,
        }
    }
}

/// Renders a high-resolution spectrogram image of an audio segment for viewport display.
///
/// Computes an STFT across the requested time window using the specified FFT size,
/// Hann windowing, and intensity parameters (gain, dynamic range, contrast),
/// outputting an RGBA buffer formatted for direct upload to a texture.
///
/// # Arguments
///
/// * `samples` - Full audio channel samples slice.
/// * `sample_rate` - Audio sample rate in Hz.
/// * `start_sec` - Viewport start timestamp in seconds.
/// * `duration_sec` - Viewport duration in seconds.
/// * `num_cols` - Target horizontal resolution (width in pixels).
/// * `config` - Spectrogram rendering configuration parameters.
///
/// # Returns
///
/// A tuple `(width, height, rgba_pixels)` where:
/// - `width` is the clamped number of time columns.
/// - `height` is the number of frequency bins (`config.fft_size / 2`).
/// - `rgba_pixels` is a tightly packed RGBA byte vector of size `width * height * 4`.
pub fn render_hf_viewport_spectrogram(
    samples: &[f32],
    sample_rate: u32,
    start_sec: f32,
    duration_sec: f32,
    num_cols: usize,
    config: HfSpectrogramConfig,
) -> (usize, usize, Vec<u8>) {
    let fft_size = match config.fft_size {
        256 | 512 | 1024 | 2048 | 4096 => config.fft_size,
        other if other > 0 && other.is_power_of_two() => other,
        _ => 1024,
    };
    let num_freq_bins = fft_size / 2;
    let cols = num_cols.max(1);

    if samples.is_empty() || sample_rate == 0 {
        return (cols, num_freq_bins, vec![0u8; cols * num_freq_bins * 4]);
    }

    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_size);

    let hann: Vec<f32> = (0..fft_size)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / (fft_size - 1) as f32).cos()))
        .collect();

    let total_frames = samples.len();
    let start_frame = (start_sec.max(0.0) * sample_rate as f32) as isize;
    let end_frame = ((start_sec + duration_sec.max(0.001)) * sample_rate as f32) as isize;
    let view_frames = (end_frame - start_frame).max(1) as f64;

    let col_data: Vec<Vec<u8>> = (0..cols)
        .into_par_iter()
        .map(|c| {
            let mut in_data = r2c.make_input_vec();
            let mut out_data = r2c.make_output_vec();

            let center_frame = start_frame + (c as f64 / cols as f64 * view_frames) as isize;
            for k in 0..fft_size {
                let f_idx = center_frame - (fft_size as isize / 2) + k as isize;
                if f_idx >= 0 && (f_idx as usize) < total_frames {
                    in_data[k] = samples[f_idx as usize] * hann[k];
                } else {
                    in_data[k] = 0.0;
                }
            }

            let _ = r2c.process(&mut in_data, &mut out_data);

            let mut col_rgba = vec![0u8; num_freq_bins * 4];
            let ref_scale = fft_size as f32 * 0.5;
            let range_db = config.range_db.max(10.0);
            let contrast = config.contrast.clamp(0.1, 5.0);

            for (b, c) in out_data[..num_freq_bins].iter().enumerate() {
                let mag = (c.re * c.re + c.im * c.im).sqrt() / ref_scale;
                let db = 20.0 * (mag + 1e-6).log10() + config.gain_db;
                let norm_db = ((db + range_db) / range_db).clamp(0.0, 1.0);
                let shaped = if (contrast - 1.0).abs() > 0.01 {
                    norm_db.powf(contrast)
                } else {
                    norm_db
                };
                let col = inferno_colormap(shaped);

                // Row 0 is highest frequency bin (Nyquist), bottom row is 0 Hz
                let row = num_freq_bins - 1 - b;
                let idx = row * 4;
                col_rgba[idx..idx + 4].copy_from_slice(&col);
            }

            col_rgba
        })
        .collect();

    let mut rgba = vec![0u8; cols * num_freq_bins * 4];
    for (c, col_bytes) in col_data.into_iter().enumerate() {
        for row in 0..num_freq_bins {
            let src_idx = row * 4;
            let dst_idx = (row * cols + c) * 4;
            rgba[dst_idx..dst_idx + 4].copy_from_slice(&col_bytes[src_idx..src_idx + 4]);
        }
    }

    (cols, num_freq_bins, rgba)
}

/// Pitch-shift audio downwards by `factor` while preserving original duration.
///
/// Uses linear-interpolation resampling followed by Hann-windowed Overlap-Add (OLA)
/// synthesis to reconstruct the signal at its original length.
pub fn pitch_shift_realtime(samples: &[f32], ratio: f64) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }
    if ratio <= 1.0001 || !ratio.is_finite() {
        return samples.to_vec();
    }

    let abs_factor = ratio.max(1.0);

    // Step 1: Resample stretch (makes signal longer in samples, shifting frequencies down)
    let resampled = resample_stretch(samples, abs_factor);

    // Step 2: Overlap-Add (OLA) to reconstruct the original duration
    let window_size: usize = 2048;
    if resampled.len() < window_size || samples.len() < window_size {
        // Fallback for very short buffers
        return samples.to_vec();
    }

    let synthesis_hop = window_size / 2;
    let analysis_hop = ((synthesis_hop as f64 * abs_factor).round() as usize).max(1);

    let out_len = samples.len();
    let mut output = vec![0.0f32; out_len];
    let mut window_sum = vec![0.0f32; out_len];

    // Hann window
    let hann: Vec<f32> = (0..window_size)
        .map(|i| {
            let x = PI * i as f32 / window_size as f32;
            x.sin().powi(2)
        })
        .collect();

    let mut read_pos = 0usize;
    let mut write_pos = 0usize;

    while read_pos + window_size <= resampled.len() && write_pos + window_size <= out_len {
        for i in 0..window_size {
            output[write_pos + i] += resampled[read_pos + i] * hann[i];
            window_sum[write_pos + i] += hann[i];
        }
        read_pos += analysis_hop;
        write_pos += synthesis_hop;
    }

    // Normalize overlapping window gains
    for i in 0..out_len {
        if window_sum[i] > 0.001 {
            output[i] /= window_sum[i];
        }
    }

    output
}

/// Resample by stretching: output is longer, frequencies lower.
pub fn resample_stretch(samples: &[f32], factor: f64) -> Vec<f32> {
    if samples.is_empty() || factor <= 0.0 {
        return Vec::new();
    }
    let out_len = (samples.len() as f64 * factor).round() as usize;
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 / factor;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let s0 = samples[idx.min(samples.len() - 1)];
        let s1 = samples[(idx + 1).min(samples.len() - 1)];
        output.push(s0 + frac * (s1 - s0));
    }

    output
}

/// 2nd-order Biquad High-Pass filter for isolating ultrasonic frequencies.
pub struct BiquadHighPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadHighPass {
    pub fn new(sample_rate: f32, cutoff_hz: f32) -> Self {
        let nyquist = sample_rate * 0.5;
        let cutoff = cutoff_hz.clamp(10.0, nyquist * 0.99);
        let omega = 2.0 * PI * cutoff / sample_rate;
        let cos_omega = omega.cos();
        let alpha = omega.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2); // Q = 0.707

        let a0 = 1.0 + alpha;
        let b0 = ((1.0 + cos_omega) / 2.0) / a0;
        let b1 = (-(1.0 + cos_omega)) / a0;
        let b2 = ((1.0 + cos_omega) / 2.0) / a0;
        let a1 = (-2.0 * cos_omega) / a0;
        let a2 = (1.0 - alpha) / a0;

        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        samples.iter().map(|&s| self.process_sample(s)).collect()
    }
}

/// Applies a 4th-order cascaded high-pass filter (24 dB/octave) at `cutoff_hz`
/// (typically 20,000 Hz) to isolate ultrasonic content.
pub fn apply_ultrasonic_highpass(samples: &[f32], sample_rate: f32, cutoff_hz: f32) -> Vec<f32> {
    let mut stage1 = BiquadHighPass::new(sample_rate, cutoff_hz);
    let mut stage2 = BiquadHighPass::new(sample_rate, cutoff_hz);
    let mid = stage1.process(samples);
    stage2.process(&mid)
}

/// Computes an ultrasonic energy envelope (0.0..1.0 normalized) across `num_cols`
/// time slices, and detects timestamps of ultrasonic activity bursts (peaks).
pub fn compute_ultrasonic_roi(
    samples: &[f32],
    sample_rate: u32,
    num_cols: usize,
) -> (Vec<f32>, Vec<f32>) {
    if samples.is_empty() || num_cols == 0 || sample_rate < 40_000 {
        return (vec![0.0; num_cols], Vec::new());
    }

    // Highpass filter above 20 kHz to isolate ultrasonic energy
    let filtered = apply_ultrasonic_highpass(samples, sample_rate as f32, 20_000.0);

    let total_samples = samples.len();
    let samples_per_col = (total_samples as f64 / num_cols as f64).max(1.0);

    let mut energies = Vec::with_capacity(num_cols);
    let mut max_energy = 0.0f32;

    for col in 0..num_cols {
        let start = (col as f64 * samples_per_col) as usize;
        let end = (((col + 1) as f64 * samples_per_col) as usize).min(total_samples);
        if start >= end {
            energies.push(0.0);
            continue;
        }

        // Compute RMS energy in slice
        let slice = &filtered[start..end];
        let mut sum_sq = 0.0f32;
        for &s in slice {
            sum_sq += s * s;
        }
        let rms = (sum_sq / slice.len() as f32).sqrt();
        if rms > max_energy {
            max_energy = rms;
        }
        energies.push(rms);
    }

    // Normalize 0.0..1.0
    let mut normalized_roi = Vec::with_capacity(num_cols);
    if max_energy > 1e-6 {
        for &e in &energies {
            normalized_roi.push((e / max_energy).clamp(0.0, 1.0));
        }
    } else {
        normalized_roi.resize(num_cols, 0.0);
    }

    // Detect distinct peaks exceeding a meaningful ultrasonic threshold
    let peak_times = detect_burst_peaks(&energies, max_energy, samples_per_col, sample_rate);

    (normalized_roi, peak_times)
}

fn detect_burst_peaks(
    energies: &[f32],
    max_energy: f32,
    samples_per_col: f64,
    sample_rate: u32,
) -> Vec<f32> {
    if max_energy < 0.005 || energies.is_empty() || sample_rate == 0 {
        return Vec::new();
    }

    let threshold = (max_energy * 0.25).max(0.005);
    let mut peak_times = Vec::new();
    let mut in_burst = false;
    let mut burst_peak_val = 0.0f32;
    let mut burst_peak_col = 0usize;

    for (col, &e) in energies.iter().enumerate() {
        if e >= threshold {
            if !in_burst || e > burst_peak_val {
                burst_peak_val = e;
                burst_peak_col = col;
            }
            in_burst = true;
        } else if in_burst {
            let peak_time = (burst_peak_col as f64 + 0.5) * samples_per_col / sample_rate as f64;
            peak_times.push(peak_time as f32);
            in_burst = false;
        }
    }
    if in_burst {
        let peak_time = (burst_peak_col as f64 + 0.5) * samples_per_col / sample_rate as f64;
        peak_times.push(peak_time as f32);
    }

    peak_times
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resample_stretch_factor() {
        let samples = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let stretched = resample_stretch(&samples, 2.0);
        assert_eq!(stretched.len(), 10);
        assert!((stretched[0] - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_pitch_shift_preserves_length() {
        let sample_rate = 384_000;
        let num_samples = 4096;
        let input: Vec<f32> = (0..num_samples)
            .map(|i| (2.0 * PI * 27_000.0 * i as f32 / sample_rate as f32).sin())
            .collect();

        let shifted = pitch_shift_realtime(&input, 10.0);
        assert_eq!(shifted.len(), input.len());
        // Verify output is non-zero
        let energy: f32 = shifted.iter().map(|s| s * s).sum();
        assert!(energy > 0.0);
    }

    #[test]
    fn test_pitch_shift_bypass() {
        let input = vec![0.1, 0.2, -0.3, 0.4];
        let shifted = pitch_shift_realtime(&input, 1.0);
        assert_eq!(shifted, input);
    }

    #[test]
    fn test_highpass_attenuates_low_frequencies() {
        let sample_rate = 384_000.0;
        // 1 kHz tone (low frequency)
        let low_freq: Vec<f32> = (0..4096)
            .map(|i| (2.0 * PI * 1_000.0 * i as f32 / sample_rate).sin())
            .collect();
        // 40 kHz tone (ultrasonic)
        let high_freq: Vec<f32> = (0..4096)
            .map(|i| (2.0 * PI * 40_000.0 * i as f32 / sample_rate).sin())
            .collect();

        let filtered_low = apply_ultrasonic_highpass(&low_freq, sample_rate, 20_000.0);
        let filtered_high = apply_ultrasonic_highpass(&high_freq, sample_rate, 20_000.0);

        let rms_low: f32 = (filtered_low.iter().map(|s| s * s).sum::<f32>() / 4096.0).sqrt();
        let rms_high: f32 = (filtered_high.iter().map(|s| s * s).sum::<f32>() / 4096.0).sqrt();

        assert!(
            rms_low < 0.05,
            "1 kHz tone should be heavily attenuated: {rms_low}"
        );
        assert!(
            rms_high > 0.5,
            "40 kHz tone should pass through: {rms_high}"
        );
    }

    #[test]
    fn test_compute_ultrasonic_roi_detects_chirp() {
        let sample_rate = 384_000;
        let total_samples = sample_rate * 2; // 2 seconds
        let mut samples = vec![0.0f32; total_samples as usize];

        // Place a 30 kHz burst at 1.0s to 1.1s (amplitude 0.8)
        let burst_start = sample_rate;
        let burst_end = sample_rate + sample_rate / 10;
        for i in burst_start..burst_end {
            samples[i as usize] =
                0.8 * (2.0 * PI * 30_000.0 * (i - burst_start) as f32 / sample_rate as f32).sin();
        }

        let num_cols = 100;
        let (roi, peak_times) = compute_ultrasonic_roi(&samples, sample_rate, num_cols);

        assert_eq!(roi.len(), num_cols);
        assert!(!peak_times.is_empty(), "Should detect peak time");
        let peak = peak_times[0];
        assert!(
            (peak - 1.05).abs() < 0.08,
            "Peak should be around 1.05s, got {peak}"
        );

        // Column around 50 (at 1.0s) should have max ROI
        let col_52_energy = roi[52];
        let col_10_energy = roi[10];
        assert!(col_52_energy > 0.8);
        assert!(col_10_energy < 0.05);
    }

    #[test]
    fn test_render_hf_viewport_spectrogram_dimensions_and_frequency() {
        let sample_rate = 384_000;
        let total_samples = sample_rate; // 1 second
        let mut samples = vec![0.0f32; total_samples as usize];

        // 48 kHz tone (ultrasonic)
        let tone_freq = 48_000.0f32;
        for i in 0..total_samples {
            samples[i as usize] =
                0.5 * (2.0 * PI * tone_freq * i as f32 / sample_rate as f32).sin();
        }

        let config = HfSpectrogramConfig {
            fft_size: 1024,
            gain_db: 0.0,
            range_db: 80.0,
            contrast: 1.0,
        };

        let (width, height, rgba) =
            render_hf_viewport_spectrogram(&samples, sample_rate, 0.0, 0.5, 200, config);

        assert_eq!(width, 200);
        assert_eq!(height, 512); // 1024 / 2
        assert_eq!(rgba.len(), 200 * 512 * 4);

        // Nyquist = 192,000 Hz. 512 bins -> 375 Hz per bin.
        // 48,000 Hz / 375 Hz = bin 128.
        // Row 0 is highest freq (bin 511), bottom row is 0 Hz (bin 0).
        // Row for bin 128 is 511 - 128 = row 383.
        let expected_row =
            512 - 1 - (tone_freq / (sample_rate as f32 * 0.5) * 512.0).round() as usize;

        // Check center column has bright pixel around expected_row
        let col = 100;
        let mut max_row = 0;
        let mut max_brightness = 0u32;
        for row in 0..height {
            let idx = (row * width + col) * 4;
            let brightness = rgba[idx] as u32 + rgba[idx + 1] as u32 + rgba[idx + 2] as u32;
            if brightness > max_brightness {
                max_brightness = brightness;
                max_row = row;
            }
        }
        assert!(
            (max_row as isize - expected_row as isize).abs() <= 2,
            "Max brightness should be near row {expected_row}, got {max_row}"
        );
        assert!(max_brightness > 200, "Peak brightness should be intense");

        // Verify 4096 FFT window size (2048 frequency bins)
        let config_4096 = HfSpectrogramConfig {
            fft_size: 4096,
            ..config
        };
        let (w4, h4, rgba4) =
            render_hf_viewport_spectrogram(&samples, sample_rate, 0.0, 0.5, 150, config_4096);
        assert_eq!(w4, 150);
        assert_eq!(h4, 2048);
        assert_eq!(rgba4.len(), 150 * 2048 * 4);
    }

    fn generate_synthetic_ultrasonic_wav(
        sample_rate: u32,
        duration_sec: f32,
    ) -> std::path::PathBuf {
        use std::io::Write;
        let temp_path =
            std::env::temp_dir().join(format!("test_ultrasonic_{}.wav", uuid::Uuid::new_v4()));
        let mut file = std::fs::File::create(&temp_path).expect("failed to create temp wav");
        let num_samples = (sample_rate as f32 * duration_sec) as usize;
        let num_channels = 1u16;
        let bits_per_sample = 16u16;
        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_len.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&num_channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&block_align.to_le_bytes()).unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();

        // 30 kHz chirp between 0.1s and 0.2s
        let chirp_start = (sample_rate as f32 * 0.1) as usize;
        let chirp_end = (sample_rate as f32 * 0.2) as usize;
        for i in 0..num_samples {
            let s = if i >= chirp_start && i < chirp_end {
                0.8 * (2.0 * PI * 30_000.0 * (i - chirp_start) as f32 / sample_rate as f32).sin()
            } else {
                0.0
            };
            let val = (s * 30000.0) as i16;
            file.write_all(&val.to_le_bytes()).unwrap();
        }
        file.flush().unwrap();
        temp_path
    }

    #[test]
    fn test_render_hf_viewport_spectrogram_on_bat_file() {
        let bat_path = generate_synthetic_ultrasonic_wav(384_000, 0.5);
        let analysis =
            little_clip_core::audio::compute_audio_analysis_progressive(&bat_path, 100, |_| {})
                .expect("failed to decode synthetic ultrasonic wav");
        assert_eq!(analysis.sample_rate, 384_000);

        let config = HfSpectrogramConfig {
            fft_size: 1024,
            gain_db: 5.0,
            range_db: 75.0,
            contrast: 1.1,
        };

        let (w, h, rgba) = render_hf_viewport_spectrogram(
            &analysis.ch1_samples,
            analysis.sample_rate,
            0.0,
            0.4,
            400,
            config,
        );

        assert_eq!(w, 400);
        assert_eq!(h, 512);
        assert_eq!(rgba.len(), 400 * 512 * 4);

        // Bat chirp is at 25-35 kHz. Verify significant high-frequency energy in rows corresponding to 25k-35k.
        let nyquist = analysis.sample_rate as f32 * 0.5;
        let row_35k = (h as f32 * (1.0 - 35_000.0 / nyquist)) as usize;
        let row_25k = (h as f32 * (1.0 - 25_000.0 / nyquist)) as usize;

        let mut has_chirp_energy = false;
        for c in 0..w {
            for row in row_35k..=row_25k {
                let idx = (row * w + c) * 4;
                if rgba[idx] > 180 || rgba[idx + 1] > 100 {
                    has_chirp_energy = true;
                    break;
                }
            }
            if has_chirp_energy {
                break;
            }
        }
        assert!(
            has_chirp_energy,
            "Should detect high-energy bat chirp in 25-35 kHz band"
        );
        let _ = std::fs::remove_file(&bat_path);
    }
}
