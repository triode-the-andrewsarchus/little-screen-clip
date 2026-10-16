//! Audio file decoding, probing, progressive analysis, and 3-band spectral analysis using Symphonia.

use super::analysis::*;
use super::types::*;
use super::waveform::*;
use crate::config::{AudioConfig, AudioSamplePosition};
use rayon::prelude::*;
use realfft::RealFftPlanner;
use rustchord::cc_to_rgb;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

pub fn is_audio_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some(
            "mp3" | "wav" | "wave" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "alac" | "aiff"
            | "aif" | "opus" | "wma",
        ) => true,
        _ => false,
    }
}

pub fn read_audio_metadata(path: &Path) -> Option<AudioMetadata> {
    let file = crate::kernel_interface::open_content(path, true).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;

    let format = probed.format;
    let track = format.default_track().or_else(|| format.tracks().first())?;

    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let format_name = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("AUDIO")
        .to_uppercase();

    // Estimate duration
    let duration_ms = if let Some(n_frames) = track.codec_params.n_frames {
        (n_frames as f64 / sample_rate as f64 * 1000.0) as u64
    } else {
        // Fallback: estimate from file size (assuming ~192kbps average if unknown)
        let est_sec = (file_len as f64 * 8.0) / (192.0 * 1000.0);
        (est_sec * 1000.0) as u64
    };

    Some(AudioMetadata {
        duration_ms,
        sample_rate,
        channels,
        format: format_name,
    })
}

/// Decodes audio and computes downsampled peak waveform.
/// For files exceeding audio_config.max_full_waveform_sec, decodes a snippet
/// from either the middle (default) or start, as configured.
/// Progressively decodes audio and computes analysis stages:
/// 1. Waveform samples & peak pyramid (instant interactive waveform)
/// 2. Transients (instant snapping)
/// 3. Spectrogram & ColorChord note ribbon (Rayon multi-threaded FFT + chromagram)
pub fn compute_audio_analysis_progressive<F>(
    path: &Path,
    num_cols: usize,
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_progressive_with_gain_and_edits(path, num_cols, 0.0, &[], on_progress)
}

/// Cancellable progressive analysis for bounded background jobs.
pub fn compute_audio_analysis_progressive_cancellable<F>(
    path: &Path,
    num_cols: usize,
    cancelled: &AtomicBool,
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_inner(
        path,
        num_cols,
        0.0,
        &[],
        false,
        Some(cancelled),
        on_progress,
    )
}

/// Rebuilds analysis from the original file with session edits applied to decoded PCM.
/// The file itself is never modified.
pub fn compute_audio_analysis_progressive_with_edits<F>(
    path: &Path,
    num_cols: usize,
    region_edits: &[AudioRegionEdit],
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_progressive_with_gain_and_edits(
        path,
        num_cols,
        0.0,
        region_edits,
        on_progress,
    )
}

/// Rebuilds analysis with a track-wide gain applied before any region edits.
pub fn compute_audio_analysis_progressive_with_gain_and_edits<F>(
    path: &Path,
    num_cols: usize,
    track_gain_db: f32,
    region_edits: &[AudioRegionEdit],
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_inner(
        path,
        num_cols,
        track_gain_db,
        region_edits,
        false,
        None,
        on_progress,
    )
}

/// Cancellable variant of progressive analysis with track gain and region edits.
pub fn compute_audio_analysis_progressive_with_gain_and_edits_cancellable<F>(
    path: &Path,
    num_cols: usize,
    track_gain_db: f32,
    region_edits: &[AudioRegionEdit],
    cancelled: &AtomicBool,
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_inner(
        path,
        num_cols,
        track_gain_db,
        region_edits,
        false,
        Some(cancelled),
        on_progress,
    )
}

/// Prioritizes the spectrum and chord strip for interactive region effect edits.
/// Peak pyramids are built after the spectrum so filter edits show their
/// spectral result first. Transient detection is skipped; snapping uses the
/// retained source analysis.
pub fn compute_audio_spectral_preview_with_edits<F>(
    path: &Path,
    num_cols: usize,
    regions: &[AudioRegionEdit],
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_spectral_preview_with_gain_and_edits(path, num_cols, 0.0, regions, on_progress)
}

/// Rebuilds the spectrum-first preview with track-wide gain applied before region edits.
pub fn compute_audio_spectral_preview_with_gain_and_edits<F>(
    path: &Path,
    num_cols: usize,
    track_gain_db: f32,
    regions: &[AudioRegionEdit],
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_inner(
        path,
        num_cols,
        track_gain_db,
        regions,
        true,
        None,
        on_progress,
    )
}

/// Cancellable spectrum-first preview with track gain and region edits.
pub fn compute_audio_spectral_preview_with_gain_and_edits_cancellable<F>(
    path: &Path,
    num_cols: usize,
    track_gain_db: f32,
    regions: &[AudioRegionEdit],
    cancelled: &AtomicBool,
    on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    compute_audio_analysis_inner(
        path,
        num_cols,
        track_gain_db,
        regions,
        true,
        Some(cancelled),
        on_progress,
    )
}

fn compute_audio_analysis_inner<F>(
    path: &Path,
    num_cols: usize,
    track_gain_db: f32,
    region_edits: &[AudioRegionEdit],
    spectral_only: bool,
    cancelled: Option<&AtomicBool>,
    mut on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
    let fallback_cancel = AtomicBool::new(false);
    let cancelled = cancelled.unwrap_or(&fallback_cancel);
    let is_cancelled = || cancelled.load(Ordering::Acquire);
    if is_cancelled() {
        return None;
    }
    let meta = match read_audio_metadata(path) {
        Some(m) => m,
        None => {
            on_progress(AudioAnalysisProgress::Failed(
                "Could not read audio metadata".to_string(),
            ));
            return None;
        }
    };
    let num_cols = num_cols.max(120);

    let file = match crate::kernel_interface::open_content(path, true) {
        Ok(f) => f,
        Err(e) => {
            on_progress(AudioAnalysisProgress::Failed(format!(
                "Could not open file: {}",
                e
            )));
            return None;
        }
    };
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = match symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) {
        Ok(p) => p,
        Err(e) => {
            on_progress(AudioAnalysisProgress::Failed(format!(
                "Audio probe failed: {}",
                e
            )));
            return None;
        }
    };

    let mut format = probed.format;
    let track = match format.default_track().or_else(|| format.tracks().first()) {
        Some(t) => t.clone(),
        None => {
            on_progress(AudioAnalysisProgress::Failed(
                "No audio tracks found in file".to_string(),
            ));
            return None;
        }
    };

    let mut decoder = match symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
    {
        Ok(d) => d,
        Err(e) => {
            on_progress(AudioAnalysisProgress::Failed(format!(
                "Decoder init failed: {}",
                e
            )));
            return None;
        }
    };

    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let is_stereo = channels >= 2;

    // Estimate capacity to avoid reallocations
    let est_frames = ((meta.duration_ms as f64 / 1000.0) * sample_rate as f64) as usize;
    // Cap at 30 minutes to safeguard memory
    let max_frames = 86_400_000usize;
    let alloc_cap = est_frames.min(max_frames).max(44100);

    let mut ch1_samples: Vec<f32> = Vec::with_capacity(alloc_cap);
    let mut ch2_samples: Option<Vec<f32>> = if is_stereo {
        Some(Vec::with_capacity(alloc_cap))
    } else {
        None
    };

    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    let t_decode_start = std::time::Instant::now();
    let mut decoded_packets = 0usize;
    while let Ok(packet) = format.next_packet() {
        decoded_packets += 1;
        if decoded_packets.is_multiple_of(16) && is_cancelled() {
            return None;
        }
        if packet.track_id() != track.id {
            continue;
        }

        if let Ok(decoded) = decoder.decode(&packet) {
            if sample_buf.is_none() {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
            }

            if let Some(ref mut sb) = sample_buf {
                sb.copy_interleaved_ref(decoded);
                let samples = sb.samples();
                let n_ch = channels as usize;
                if n_ch == 0 {
                    continue;
                }
                let frames = samples.len() / n_ch;

                for f in 0..frames {
                    if f.is_multiple_of(4096) && is_cancelled() {
                        return None;
                    }
                    if ch1_samples.len() >= max_frames {
                        break;
                    }
                    let s1 = samples[f * n_ch];
                    ch1_samples.push(s1);
                    if is_stereo {
                        let s2 = if n_ch > 1 { samples[f * n_ch + 1] } else { s1 };
                        ch2_samples.as_mut().unwrap().push(s2);
                    }
                }
            }
        }
        if ch1_samples.len() >= max_frames {
            break;
        }
    }
    let is_truncated = ch1_samples.len() >= max_frames;
    if is_truncated {
        tracing::warn!(
            "Audio decoding truncated at max frame cap of {} frames",
            max_frames
        );
    }
    let t_decode_elapsed = t_decode_start.elapsed();

    let total_frames = ch1_samples.len();
    if is_cancelled() {
        return None;
    }
    if total_frames == 0 {
        on_progress(AudioAnalysisProgress::Failed(
            "Decoded 0 audio frames".to_string(),
        ));
        return None;
    }
    let duration_sec = total_frames as f64 / sample_rate as f64;
    // Default to higher-quality 1024-point FFT (512 frequency bins),
    // dropping down to standard 512-point FFT (256 frequency bins) if 10 minutes or longer.
    let (window_size, num_freq_bins) = if duration_sec >= 600.0 {
        (512, 256)
    } else {
        (1024, 512)
    };

    apply_track_gain(&mut ch1_samples, ch2_samples.as_deref_mut(), track_gain_db);
    if is_cancelled() {
        return None;
    }

    if !region_edits.is_empty() {
        // Earlier selections take precedence when regions overlap, matching the editor view.
        let mut claimed = vec![false; total_frames];
        for region in region_edits {
            let (effect_start, effect_end) = region.effect_bounds(duration_sec as f32);
            let start = ((effect_start * sample_rate as f32) as usize).min(total_frames);
            let end = ((effect_end * sample_rate as f32).ceil() as usize).min(total_frames);
            let mut ch1_filter = AudioFilterChain::new(sample_rate as f32, region);
            let mut ch2_filter = AudioFilterChain::new(sample_rate as f32, region);
            for frame in start..end {
                if frame.is_multiple_of(4096) && is_cancelled() {
                    return None;
                }
                let mix = region.effect_mix(frame as f32 / sample_rate as f32, duration_sec as f32);
                if !claimed[frame] {
                    claimed[frame] = mix > 0.0;
                    let dry_ch1 = ch1_samples[frame];
                    let wet_ch1 = ch1_filter.process(apply_audio_gain(
                        dry_ch1,
                        region.gain_db,
                        region.soft_clip,
                    ));
                    ch1_samples[frame] = dry_ch1 + (wet_ch1 - dry_ch1) * mix;
                    if let Some(ch2) = &mut ch2_samples {
                        let dry_ch2 = ch2[frame];
                        let wet_ch2 = ch2_filter.process(apply_audio_gain(
                            dry_ch2,
                            region.gain_db,
                            region.soft_clip,
                        ));
                        ch2[frame] = dry_ch2 + (wet_ch2 - dry_ch2) * mix;
                    }
                }
            }
        }
    }

    if is_cancelled() {
        return None;
    }

    let duration_ms = (total_frames as f64 / sample_rate as f64 * 1000.0) as u64;

    // Concurrently build multi-scale min/max peak pyramids with Serato DJ 3-band crossover colors
    let t_pyramid_start = std::time::Instant::now();
    let (ch1_pyramid, ch2_pyramid) = if spectral_only {
        (Some((Vec::new(), Vec::new(), Vec::new())), Some(None))
    } else {
        rayon::join(
            || build_peak_pyramid_cancellable(&ch1_samples, sample_rate as f32, cancelled),
            || {
                if let Some(ref ch2) = ch2_samples {
                    build_peak_pyramid_cancellable(ch2, sample_rate as f32, cancelled).map(Some)
                } else {
                    Some(None)
                }
            },
        )
    };
    let (peaks_lvl0_ch1, peaks_lvl1_ch1, peaks_lvl2_ch1) = match ch1_pyramid {
        Some(pyramid) => pyramid,
        None => return None,
    };
    let (peaks_lvl0_ch2, peaks_lvl1_ch2, peaks_lvl2_ch2) = match ch2_pyramid {
        Some(Some((l0, l1, l2))) => (Some(l0), Some(l1), Some(l2)),
        Some(None) => (None, None, None),
        None => return None,
    };
    let t_pyramid_elapsed = t_pyramid_start.elapsed();
    if is_cancelled() {
        return None;
    }

    // 1. Overview Peaks
    let num_cols = if sample_rate > 196_000 && num_cols == 1200 {
        3600
    } else {
        num_cols
    };
    let t_overview_start = std::time::Instant::now();
    let mut overview_peaks1 = Vec::with_capacity(num_cols);
    let mut overview_peaks2 = if is_stereo {
        Some(Vec::with_capacity(num_cols))
    } else {
        None
    };

    for c in 0..if spectral_only { 0 } else { num_cols } {
        if c.is_multiple_of(64) && is_cancelled() {
            return None;
        }
        let f_start = (c * total_frames) / num_cols;
        let f_end = (((c + 1) * total_frames) / num_cols).min(total_frames);
        let span = f_end.saturating_sub(f_start);

        let (p1, p2) = if span >= 1024 && !peaks_lvl1_ch1.is_empty() {
            let b_start = (f_start / 1024).min(peaks_lvl1_ch1.len().saturating_sub(1));
            let b_end = ((f_end + 1023) / 1024).min(peaks_lvl1_ch1.len());
            let mut mx1 = 0.0f32;
            for b in &peaks_lvl1_ch1[b_start..b_end] {
                let m = b.min.abs().max(b.max.abs());
                if m > mx1 {
                    mx1 = m;
                }
            }
            let mx2 = if let Some(ref l1) = peaks_lvl1_ch2 {
                let mut m2 = 0.0f32;
                for b in &l1[b_start..b_end] {
                    let m = b.min.abs().max(b.max.abs());
                    if m > m2 {
                        m2 = m;
                    }
                }
                m2
            } else {
                0.0
            };
            (mx1, mx2)
        } else {
            let mut mx1 = 0.0f32;
            let mut mx2 = 0.0f32;
            for f in f_start..f_end {
                let s1 = ch1_samples[f].abs();
                if s1 > mx1 {
                    mx1 = s1;
                }
                if is_stereo {
                    let s2 = ch2_samples.as_ref().unwrap()[f].abs();
                    if s2 > mx2 {
                        mx2 = s2;
                    }
                }
            }
            (mx1, mx2)
        };

        overview_peaks1.push(p1.min(1.0));
        if let Some(ref mut op2) = overview_peaks2 {
            op2.push(p2.min(1.0));
        }
    }
    let t_overview_elapsed = t_overview_start.elapsed();

    // Pre-calculate mixed audio samples for transients and YIN analysis
    let t_mix_start = std::time::Instant::now();
    let mixed_samples: Vec<f32> = if is_stereo {
        let ch2 = ch2_samples.as_ref().unwrap();
        ch1_samples
            .iter()
            .zip(ch2.iter())
            .map(|(&s1, &s2)| 0.5 * (s1 + s2))
            .collect()
    } else {
        ch1_samples.clone()
    };
    let t_mix_elapsed = t_mix_start.elapsed();
    if is_cancelled() {
        return None;
    }

    // Emit Stage 1: Waveform is fully ready for immediate interactive rendering!
    let mut analysis = FullAudioAnalysis {
        channels,
        sample_rate,
        duration_ms,
        total_frames,
        ch1_samples: ch1_samples.into(),
        ch2_samples: ch2_samples.map(Into::into),
        overview_peaks1,
        overview_peaks2,
        peaks_lvl0_ch1,
        peaks_lvl0_ch2,
        peaks_lvl1_ch1,
        peaks_lvl1_ch2,
        peaks_lvl2_ch1,
        peaks_lvl2_ch2,
        spectrogram_width: num_cols,
        spectrogram_height: num_freq_bins,
        spectrogram_rgba: Vec::new(),
        note_strip_width: num_cols,
        note_strip_height: 48,
        note_strip_rgba: Vec::new(),
        note_strip_info: Vec::new(),
        spectral_colors: Vec::new(),
        transients: Vec::new(),
        is_transients_ready: false,
        is_spectrogram_ready: false,
        ultrasonic_roi: Vec::new(),
        ultrasonic_peak_times: Vec::new(),
        is_truncated,
        truncation_warning: if is_truncated {
            Some(format!(
                "Audio track exceeded decoding frame limit of {} frames (~{:.1} min) and was truncated",
                max_frames,
                (max_frames as f64 / sample_rate as f64) / 60.0
            ))
        } else {
            None
        },
    };
    if !spectral_only {
        on_progress(AudioAnalysisProgress::WaveformReady(Box::new(
            analysis.clone(),
        )));
    }

    // Emit Stage 2: Transients detection
    let t_transients_start = std::time::Instant::now();
    let transients = if spectral_only {
        Vec::new()
    } else {
        detect_transients_with_sensitivity_cancellable(
            &mixed_samples,
            sample_rate as f32,
            DEFAULT_TRANSIENT_SENSITIVITY,
            Some(cancelled),
        )?
    };
    let t_transients_elapsed = t_transients_start.elapsed();
    analysis.transients = transients.clone();
    analysis.is_transients_ready = true;
    if !spectral_only {
        on_progress(AudioAnalysisProgress::TransientsReady(transients));
    }
    if is_cancelled() {
        return None;
    }

    // Emit Stage 3: Rayon parallel STFT Spectrogram & ColorChord 2-D note strip
    let t_fft_loop_start = std::time::Instant::now();
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(window_size);

    let hann: Vec<f32> = (0..window_size)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos())
        })
        .collect();

    let spectrogram_width = num_cols;
    let spectrogram_height = num_freq_bins;
    let note_strip_width = num_cols;
    let note_strip_height = 48usize;

    struct ColumnAnalysisResult {
        spec_col_rgba: Vec<u8>,
        spectral_color: [u8; 3],
        note_strip_col_rgba: Vec<u8>,
        note_info: ColorChordNoteInfo,
        ultrasonic_energy: f32,
    }

    let col_results: Vec<ColumnAnalysisResult> = (0..num_cols)
        .into_par_iter()
        .map(|c| {
            if is_cancelled() {
                return None;
            }
            let mut in_data = r2c.make_input_vec();
            let mut out_data = r2c.make_output_vec();

            let frame_start = (c as f64 / num_cols as f64 * total_frames as f64) as usize;
            let frame_end = (((c + 1) as f64 / num_cols as f64 * total_frames as f64) as usize)
                .min(total_frames);

            let center_frame = if sample_rate > 196_000 && frame_end > frame_start + window_size {
                let mut peak_val = 0.0f32;
                let mut peak_idx = (frame_start + frame_end) / 2;
                for idx in frame_start..frame_end {
                    let s_abs = mixed_samples[idx].abs();
                    if s_abs > peak_val {
                        peak_val = s_abs;
                        peak_idx = idx;
                    }
                }
                peak_idx as isize
            } else {
                ((frame_start + frame_end) / 2) as isize
            };

            for k in 0..window_size {
                let f_idx = center_frame - (window_size as isize / 2) + k as isize;
                if f_idx >= 0 && (f_idx as usize) < total_frames {
                    let s = mixed_samples[f_idx as usize];
                    in_data[k] = s * hann[k];
                } else {
                    in_data[k] = 0.0;
                }
            }

            let _ = r2c.process(&mut in_data, &mut out_data);

            // Spectrogram column (Audacity-style Inferno heatmap)
            let mut spec_col_rgba = vec![0u8; num_freq_bins * 4];
            for b in 0..num_freq_bins {
                let mag = (out_data[b].re * out_data[b].re + out_data[b].im * out_data[b].im)
                    .sqrt()
                    / (window_size as f32 * 0.5);
                let db = 20.0 * (mag + 1e-6).log10();
                let norm_db = ((db + 80.0) / 80.0).clamp(0.0, 1.0);
                let col = inferno_colormap(norm_db);

                // Row 0 is highest frequency bin (num_freq_bins - 1), bottom row is 0 Hz
                let row = num_freq_bins - 1 - b;
                let idx = row * 4;
                spec_col_rgba[idx..idx + 4].copy_from_slice(&col);
            }

            // 3-band spectral frequency distribution for waveform coloring (Bass / Mid / Treble)
            // + Ultrasonic energy tracking (> 20 kHz)
            let mut e_bass = 0.0f32;
            let mut e_mid = 0.0f32;
            let mut e_high = 0.0f32;
            let mut e_ultrasonic = 0.0f32;
            for b in 1..num_freq_bins {
                let freq = b as f32 * sample_rate as f32 / window_size as f32;
                let mag =
                    (out_data[b].re * out_data[b].re + out_data[b].im * out_data[b].im).sqrt();
                let energy = mag * mag;
                if freq < 250.0 {
                    e_bass += energy;
                } else if freq < 3200.0 {
                    e_mid += energy;
                } else {
                    e_high += energy;
                }
                if freq >= 20_000.0 {
                    e_ultrasonic += energy;
                }
            }
            let w_low = e_bass.sqrt() * 1.0;
            let w_mid = e_mid.sqrt() * 2.2;
            let w_high = e_high.sqrt() * 3.4;
            let sum = w_low + w_mid + w_high;
            let spectral_color = if sum < 0.0001 {
                [100, 116, 139]
            } else {
                let r_ratio = (w_low / sum).clamp(0.0, 1.0);
                let g_ratio = (w_mid / sum).clamp(0.0, 1.0);
                let b_ratio = (w_high / sum).clamp(0.0, 1.0);
                let r = ((r_ratio * 1.5 + g_ratio * 0.3) * 255.0).clamp(30.0, 255.0) as u8;
                let g = ((g_ratio * 1.5 + r_ratio * 0.3) * 255.0).clamp(30.0, 255.0) as u8;
                let b = (b_ratio * 1.6 * 255.0).clamp(30.0, 255.0) as u8;
                [r, g, b]
            };

            // ColorChord note extraction via STFT Chromagram & fast early-exit YIN
            let half_w = 2048;
            let start_idx = (center_frame - half_w).max(0) as usize;
            let end_idx = ((center_frame + half_w) as usize).min(total_frames);
            let win_slice = if end_idx > start_idx {
                &mixed_samples[start_idx..end_idx]
            } else {
                &[]
            };

            let mut sum_sq = 0.0f32;
            for &s in win_slice {
                sum_sq += s * s;
            }
            let rms = if !win_slice.is_empty() {
                (sum_sq / win_slice.len() as f32).sqrt()
            } else {
                0.0
            };

            let mut chord_notes = detect_chord_from_fft(&out_data, sample_rate as f32, window_size);

            // Sub-cent pitch refinement via fast early-exit YIN
            if rms > 0.005 && win_slice.len() >= 256 {
                if let Some((detected_freq, clarity)) =
                    detect_pitch_yin(win_slice, sample_rate as f32)
                {
                    if detected_freq < 65.0 && clarity >= 0.35 {
                        // Fundamental is below 65 Hz (sub-bass / rumble / ~40Hz). Ignore as non-musical.
                        chord_notes.clear();
                    } else if clarity >= 0.35 && detected_freq >= 65.0 && detected_freq <= 4200.0 {
                        let (note_class, octave, midi) = freq_to_note(detected_freq);
                        let note_name = format!("{}{}", note_class, octave);
                        let note_id = (midi - 69.0).rem_euclid(12.0) / 12.0;
                        let amp = (rms * 5.0).clamp(0.4, 1.0);
                        let rgb_f = cc_to_rgb(note_id, 1.0, amp);
                        let rgb = [
                            (rgb_f[0] * 255.0).clamp(0.0, 255.0) as u8,
                            (rgb_f[1] * 255.0).clamp(0.0, 255.0) as u8,
                            (rgb_f[2] * 255.0).clamp(0.0, 255.0) as u8,
                        ];
                        let yin_note = SingleNoteInfo {
                            note_name,
                            frequency_hz: detected_freq,
                            amplitude: amp,
                            rgb,
                        };

                        if chord_notes.is_empty() || chord_notes.len() == 1 {
                            chord_notes = vec![yin_note];
                        } else {
                            let class_idx = (midi.round() as i32).rem_euclid(12) as usize;
                            if let Some(pos) = chord_notes.iter().position(|n| {
                                n.note_name.starts_with(CHROMATIC_NOTE_NAMES[class_idx])
                            }) {
                                chord_notes[pos] = yin_note.clone();
                                if pos > 0 {
                                    chord_notes.swap(0, pos);
                                }
                            } else if clarity >= 0.5 {
                                chord_notes.insert(0, yin_note);
                            }
                        }
                    }
                }
            }

            let mut note_strip_col_rgba = vec![0u8; note_strip_height * 4];
            let note_info = if chord_notes.is_empty() {
                // Fully transparent so active theme background displays seamlessly behind the ribbon
                let bg_rgb = [0u8, 0u8, 0u8, 0u8];
                for row in 0..note_strip_height {
                    let idx = row * 4;
                    note_strip_col_rgba[idx..idx + 4].copy_from_slice(&bg_rgb);
                }
                ColorChordNoteInfo {
                    note_name: "—".to_string(),
                    frequency_hz: 0.0,
                    amplitude: 0.0,
                    active: false,
                    rgb: [0, 0, 0],
                    chord_notes: Vec::new(),
                }
            } else {
                chord_notes.truncate(6);
                let num_notes = chord_notes.len();
                for (k, n) in chord_notes.iter().enumerate() {
                    let row_start = k * note_strip_height / num_notes;
                    let row_end = ((k + 1) * note_strip_height / num_notes).min(note_strip_height);
                    for row in row_start..row_end {
                        let idx = row * 4;
                        if num_notes > 1 && row == row_start && k > 0 {
                            note_strip_col_rgba[idx..idx + 4].copy_from_slice(&[0, 0, 0, 160]);
                        } else {
                            note_strip_col_rgba[idx..idx + 4]
                                .copy_from_slice(&[n.rgb[0], n.rgb[1], n.rgb[2], 255]);
                        }
                    }
                }
                let primary = &chord_notes[0];
                ColorChordNoteInfo {
                    note_name: primary.note_name.clone(),
                    frequency_hz: primary.frequency_hz,
                    amplitude: primary.amplitude,
                    active: true,
                    rgb: primary.rgb,
                    chord_notes,
                }
            };

            Some(ColumnAnalysisResult {
                spec_col_rgba,
                spectral_color,
                note_strip_col_rgba,
                note_info,
                ultrasonic_energy: e_ultrasonic.sqrt(),
            })
        })
        .collect::<Option<Vec<_>>>()?;

    let mut spectrogram_rgba = vec![0u8; spectrogram_width * spectrogram_height * 4];
    let mut note_strip_rgba = vec![0u8; note_strip_width * note_strip_height * 4];
    let mut spectral_colors = Vec::with_capacity(num_cols);
    let mut note_strip_info = Vec::with_capacity(num_cols);

    let mut max_us = 0.0f32;
    for res in &col_results {
        if res.ultrasonic_energy > max_us {
            max_us = res.ultrasonic_energy;
        }
    }
    let mut ultrasonic_roi = Vec::with_capacity(num_cols);
    if max_us > 1e-6 {
        for res in &col_results {
            ultrasonic_roi.push((res.ultrasonic_energy / max_us).clamp(0.0, 1.0));
        }
    } else {
        ultrasonic_roi.resize(num_cols, 0.0);
    }

    let mut ultrasonic_peak_times = Vec::new();
    if max_us >= 1e-4 {
        let threshold = max_us * 0.25;
        let mut in_burst = false;
        let mut burst_peak_val = 0.0f32;
        let mut burst_peak_col = 0usize;

        for (c, res) in col_results.iter().enumerate() {
            if res.ultrasonic_energy >= threshold {
                if !in_burst {
                    in_burst = true;
                    burst_peak_val = res.ultrasonic_energy;
                    burst_peak_col = c;
                } else if res.ultrasonic_energy > burst_peak_val {
                    burst_peak_val = res.ultrasonic_energy;
                    burst_peak_col = c;
                }
            } else if in_burst {
                let t =
                    (burst_peak_col as f64 / num_cols as f64 * duration_ms as f64 / 1000.0) as f32;
                ultrasonic_peak_times.push(t);
                in_burst = false;
            }
        }
        if in_burst {
            let t = (burst_peak_col as f64 / num_cols as f64 * duration_ms as f64 / 1000.0) as f32;
            ultrasonic_peak_times.push(t);
        }
    }

    for (c, res) in col_results.into_iter().enumerate() {
        for row in 0..spectrogram_height {
            let src_idx = row * 4;
            let dst_idx = (row * spectrogram_width + c) * 4;
            spectrogram_rgba[dst_idx..dst_idx + 4]
                .copy_from_slice(&res.spec_col_rgba[src_idx..src_idx + 4]);
        }
        for row in 0..note_strip_height {
            let src_idx = row * 4;
            let dst_idx = (row * note_strip_width + c) * 4;
            note_strip_rgba[dst_idx..dst_idx + 4]
                .copy_from_slice(&res.note_strip_col_rgba[src_idx..src_idx + 4]);
        }
        spectral_colors.push(res.spectral_color);
        note_strip_info.push(res.note_info);
    }
    let t_fft_loop_elapsed = t_fft_loop_start.elapsed();

    println!(
        "[AUDIO ANALYSIS TIMING] Decode: {:?} | Pyramid: {:?} | Overview: {:?} | Mix: {:?} | Transients: {:?} | Spectrogram/Notes (Rayon): {:?}",
        t_decode_elapsed,
        t_pyramid_elapsed,
        t_overview_elapsed,
        t_mix_elapsed,
        t_transients_elapsed,
        t_fft_loop_elapsed
    );

    on_progress(AudioAnalysisProgress::SpectrogramAndNotesReady {
        spectrogram_width,
        spectrogram_height,
        spectrogram_rgba: spectrogram_rgba.clone(),
        note_strip_width,
        note_strip_height,
        note_strip_rgba: note_strip_rgba.clone(),
        note_strip_info: note_strip_info.clone(),
        spectral_colors: spectral_colors.clone(),
        ultrasonic_roi: ultrasonic_roi.clone(),
        ultrasonic_peak_times: ultrasonic_peak_times.clone(),
    });
    if is_cancelled() {
        return None;
    }
    analysis.spectrogram_width = spectrogram_width;
    analysis.spectrogram_height = spectrogram_height;
    analysis.spectrogram_rgba = spectrogram_rgba;
    analysis.note_strip_width = note_strip_width;
    analysis.note_strip_height = note_strip_height;
    analysis.note_strip_rgba = note_strip_rgba;
    analysis.note_strip_info = note_strip_info;
    analysis.spectral_colors = spectral_colors;
    analysis.ultrasonic_roi = ultrasonic_roi;
    analysis.ultrasonic_peak_times = ultrasonic_peak_times;
    analysis.is_spectrogram_ready = true;

    if spectral_only {
        let (ch1_pyramid, right) = rayon::join(
            || build_peak_pyramid_cancellable(&analysis.ch1_samples, sample_rate as f32, cancelled),
            || {
                analysis
                    .ch2_samples
                    .as_ref()
                    .map(|ch2| build_peak_pyramid_cancellable(ch2, sample_rate as f32, cancelled))
            },
        );
        let (l0, l1, l2) = match ch1_pyramid {
            Some(pyramid) => pyramid,
            None => return None,
        };
        analysis.peaks_lvl0_ch1 = l0;
        analysis.peaks_lvl1_ch1 = l1;
        analysis.peaks_lvl2_ch1 = l2;
        if let Some(pyramid) = right {
            let (l0, l1, l2) = match pyramid {
                Some(pyramid) => pyramid,
                None => return None,
            };
            analysis.peaks_lvl0_ch2 = Some(l0);
            analysis.peaks_lvl1_ch2 = Some(l1);
            analysis.peaks_lvl2_ch2 = Some(l2);
        }
        on_progress(AudioAnalysisProgress::WaveformReady(Box::new(
            analysis.clone(),
        )));
    }
    on_progress(AudioAnalysisProgress::Complete);

    Some(analysis)
}

fn apply_track_gain(ch1: &mut [f32], ch2: Option<&mut [f32]>, gain_db: f32) {
    if gain_db == 0.0 {
        return;
    }
    ch1.par_iter_mut()
        .for_each(|sample| *sample = apply_audio_gain(*sample, gain_db, false));
    if let Some(ch2) = ch2 {
        ch2.par_iter_mut()
            .for_each(|sample| *sample = apply_audio_gain(*sample, gain_db, false));
    }
}

/// Decodes the full audio track and computes complete audio analysis synchronously.
pub fn compute_full_audio_analysis(path: &Path, num_cols: usize) -> Option<FullAudioAnalysis> {
    compute_audio_analysis_progressive(path, num_cols, |_| {})
}

/// Analyzes spectral frequency distribution (Bass / Mids / Highs) using 3-band filtering
/// For files exceeding audio_config.max_full_waveform_sec, analyzes the snippet window.
pub fn compute_spectral_colors(
    path: &Path,
    num_bars: usize,
    audio_config: &AudioConfig,
) -> Option<Vec<[u8; 3]>> {
    let meta = read_audio_metadata(path)?;
    if num_bars == 0 {
        return None;
    }

    let max_full_ms = audio_config.max_full_waveform_sec * 1000;
    let (is_snippet, snippet_start_ms, target_duration_ms) = if meta.duration_ms > max_full_ms {
        let snippet_dur = (audio_config.snippet_duration_sec * 1000).min(meta.duration_ms);
        let start = match audio_config.sample_position {
            AudioSamplePosition::Middle => {
                if meta.duration_ms > snippet_dur {
                    (meta.duration_ms - snippet_dur) / 2
                } else {
                    0
                }
            }
            AudioSamplePosition::Start => 0,
        };
        (true, start, snippet_dur)
    } else {
        (false, 0, meta.duration_ms)
    };

    let file = crate::kernel_interface::open_content(path, true).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .or_else(|| format.tracks().first())?
        .clone();

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;

    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as usize)
        .unwrap_or(2);
    let sample_rate = (track.codec_params.sample_rate.unwrap_or(44100) as f32).max(8000.0);

    // If sampling a snippet from the middle, attempt coarse seek
    if is_snippet && snippet_start_ms > 0 {
        let start_sec = snippet_start_ms as f64 / 1000.0;
        let _ = format.seek(
            SeekMode::Coarse,
            SeekTo::Time {
                time: Time::from(start_sec),
                track_id: Some(track.id),
            },
        );
        decoder.reset();
    }

    // 3-Band Filter:
    let dt = 1.0 / sample_rate;
    let rc_low = 1.0 / (2.0 * std::f32::consts::PI * 250.0);
    let alpha_low = (dt / (rc_low + dt)).clamp(0.001, 0.999);

    let rc_high = 1.0 / (2.0 * std::f32::consts::PI * 3200.0);
    let alpha_high = (rc_high / (rc_high + dt)).clamp(0.001, 0.999);

    let mut y_low = 0.0f32;
    let mut y_high = 0.0f32;
    let mut prev_x = 0.0f32;

    let chunk_size = 512;
    let mut e_low = 0.0f32;
    let mut e_mid = 0.0f32;
    let mut e_high = 0.0f32;
    let mut chunk_samples = 0;
    let mut total_frames = 0u64;

    let max_frames_to_decode = if is_snippet {
        (target_duration_ms as f64 / 1000.0 * sample_rate as f64) as u64
    } else {
        u64::MAX
    };

    let mut chunk_energies: Vec<[f32; 3]> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }

        if let Ok(decoded) = decoder.decode(&packet) {
            if sample_buf.is_none() {
                let spec = *decoded.spec();
                let duration = decoded.capacity() as u64;
                sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
            }

            if let Some(ref mut sb) = sample_buf {
                sb.copy_interleaved_ref(decoded);
                let samples = sb.samples();
                let n_ch = channels;
                if n_ch == 0 {
                    continue;
                }
                let frames = samples.len() / n_ch;
                total_frames += frames as u64;

                for f in 0..frames {
                    let x = if n_ch > 1 {
                        (samples[f * n_ch] + samples[f * n_ch + 1]) * 0.5
                    } else {
                        samples[f * n_ch]
                    };

                    y_low += alpha_low * (x - y_low);
                    y_high = alpha_high * (y_high + x - prev_x);
                    prev_x = x;
                    let y_mid = x - y_low - y_high;

                    e_low += y_low * y_low;
                    e_mid += y_mid * y_mid;
                    e_high += y_high * y_high;
                    chunk_samples += 1;

                    if chunk_samples >= chunk_size {
                        chunk_energies.push([
                            (e_low / chunk_samples as f32).sqrt(),
                            (e_mid / chunk_samples as f32).sqrt(),
                            (e_high / chunk_samples as f32).sqrt(),
                        ]);
                        e_low = 0.0;
                        e_mid = 0.0;
                        e_high = 0.0;
                        chunk_samples = 0;
                    }
                }

                if is_snippet && total_frames >= max_frames_to_decode {
                    break;
                }
            }
        }
    }

    if chunk_samples > 0 {
        chunk_energies.push([
            (e_low / chunk_samples as f32).sqrt(),
            (e_mid / chunk_samples as f32).sqrt(),
            (e_high / chunk_samples as f32).sqrt(),
        ]);
    }

    if chunk_energies.is_empty() {
        return None;
    }

    let mut colors = Vec::with_capacity(num_bars);
    let step = chunk_energies.len() as f64 / num_bars as f64;

    for i in 0..num_bars {
        let start_idx = (i as f64 * step).floor() as usize;
        let end_idx = (((i + 1) as f64 * step).ceil() as usize).min(chunk_energies.len());

        let mut b_low = 0.0f32;
        let mut b_mid = 0.0f32;
        let mut b_high = 0.0f32;
        let mut count = 0;

        for e in &chunk_energies[start_idx..end_idx] {
            b_low += e[0];
            b_mid += e[1];
            b_high += e[2];
            count += 1;
        }

        if count > 0 {
            b_low /= count as f32;
            b_mid /= count as f32;
            b_high /= count as f32;
        }

        let w_low = b_low * 1.0;
        let w_mid = b_mid * 2.2;
        let w_high = b_high * 3.4;
        let sum = w_low + w_mid + w_high;

        if sum < 0.0001 {
            colors.push([100, 116, 139]);
        } else {
            let r_ratio = (w_low / sum).clamp(0.0, 1.0);
            let g_ratio = (w_mid / sum).clamp(0.0, 1.0);
            let b_ratio = (w_high / sum).clamp(0.0, 1.0);

            let r = ((r_ratio * 1.5 + g_ratio * 0.3) * 255.0).clamp(30.0, 255.0) as u8;
            let g = ((g_ratio * 1.5 + r_ratio * 0.3) * 255.0).clamp(30.0, 255.0) as u8;
            let b = (b_ratio * 1.6 * 255.0).clamp(30.0, 255.0) as u8;

            colors.push([r, g, b]);
        }
    }

    Some(colors)
}
