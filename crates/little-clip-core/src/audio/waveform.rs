//! Waveform computation, peak pyramid pyramids, and interactive analysis query methods.

use super::analysis::*;
use super::decode::*;
use super::types::*;
use crate::config::{AudioConfig, AudioSamplePosition};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

impl FullAudioAnalysis {
    #[inline]
    pub fn query_color_at_time(&self, time_sec: f32) -> [u8; 3] {
        if self.spectral_colors.is_empty() {
            return [56, 189, 248]; // default sky blue
        }
        let dur = (self.duration_ms as f32 / 1000.0).max(0.001);
        let ratio = (time_sec / dur).clamp(0.0, 1.0);
        let idx = (ratio * (self.spectral_colors.len() - 1) as f32).round() as usize;
        self.spectral_colors[idx.min(self.spectral_colors.len() - 1)]
    }

    #[inline]
    pub fn find_nearest_transient(&self, time_sec: f32, max_dist_sec: f32) -> Option<f32> {
        let mut best_t = None;
        let mut best_dist = max_dist_sec;
        for &t in &self.transients {
            let dist = (t - time_sec).abs();
            if dist <= best_dist {
                best_dist = dist;
                best_t = Some(t);
            }
        }
        best_t
    }

    /// Recompute transient detection points using an adjustable sensitivity factor.
    pub fn recompute_transients(&mut self, sensitivity: f32) {
        let Some(transients) = self.transients_with_sensitivity(sensitivity) else {
            return;
        };
        self.transients = transients;
        self.is_transients_ready = true;
    }

    /// Computes transient detection points without changing the immutable source analysis.
    pub fn transients_with_sensitivity(&self, sensitivity: f32) -> Option<Vec<f32>> {
        if self.ch1_samples.is_empty() || self.sample_rate == 0 {
            return None;
        }
        let mixed: Vec<f32> = match &self.ch2_samples {
            Some(ch2) => self
                .ch1_samples
                .iter()
                .zip(ch2.iter())
                .map(|(&a, &b)| (a + b) * 0.5)
                .collect(),
            None => self.ch1_samples.to_vec(),
        };
        Some(detect_transients_with_sensitivity(
            &mixed,
            self.sample_rate as f32,
            sensitivity,
        ))
    }

    /// Finds the non-silent start and end boundary times (in seconds) based on threshold dB (e.g. -45.0 dB).
    /// Returns Option<(start_sec, end_sec)>.
    pub fn detect_silence_bounds(&self, threshold_db: f32) -> Option<(f32, f32)> {
        if self.ch1_samples.is_empty() || self.sample_rate == 0 {
            return None;
        }
        // Convert threshold_db to linear amplitude: 10^(threshold_db / 20)
        let lin_thresh = 10.0f32.powf(threshold_db / 20.0);
        let n = self.ch1_samples.len();
        let mut first_frame = None;
        let mut last_frame = None;

        for i in 0..n {
            let s1 = self.ch1_samples[i].abs();
            let s2 = self
                .ch2_samples
                .as_ref()
                .map(|ch2| ch2[i].abs())
                .unwrap_or(0.0);
            if s1 >= lin_thresh || s2 >= lin_thresh {
                first_frame = Some(i);
                break;
            }
        }

        let first = match first_frame {
            Some(f) => f,
            None => return None, // completely silent
        };

        for i in (first..n).rev() {
            let s1 = self.ch1_samples[i].abs();
            let s2 = self
                .ch2_samples
                .as_ref()
                .map(|ch2| ch2[i].abs())
                .unwrap_or(0.0);
            if s1 >= lin_thresh || s2 >= lin_thresh {
                last_frame = Some(i);
                break;
            }
        }

        let last = last_frame.unwrap_or(first);
        let sr = self.sample_rate as f32;
        // Small 15ms padding so we don't chop abruptly into transients/release
        let pad_frames = (0.015 * sr) as usize;
        let start_frame = first.saturating_sub(pad_frames);
        let end_frame = (last + pad_frames).min(n);

        let start_sec = start_frame as f32 / sr;
        let end_sec = end_frame as f32 / sr;
        Some((start_sec, end_sec))
    }

    #[inline]
    pub fn query_peaks(
        &self,
        ch: usize,
        start_frame: usize,
        end_frame: usize,
    ) -> (f32, f32, [u8; 3]) {
        if start_frame >= end_frame || start_frame >= self.total_frames {
            return (0.0, 0.0, [56, 189, 248]);
        }
        let end = end_frame.min(self.total_frames);
        let span = end - start_frame;

        let (lvl0, lvl1, lvl2, raw) = if ch == 0 {
            (
                &self.peaks_lvl0_ch1,
                &self.peaks_lvl1_ch1,
                &self.peaks_lvl2_ch1,
                &self.ch1_samples,
            )
        } else if let (Some(l0), Some(l1), Some(l2), Some(r)) = (
            &self.peaks_lvl0_ch2,
            &self.peaks_lvl1_ch2,
            &self.peaks_lvl2_ch2,
            &self.ch2_samples,
        ) {
            (l0, l1, l2, r)
        } else {
            (
                &self.peaks_lvl0_ch1,
                &self.peaks_lvl1_ch1,
                &self.peaks_lvl2_ch1,
                &self.ch1_samples,
            )
        };

        if span >= 16384 && !lvl2.is_empty() {
            let b_start = (start_frame / 16384).min(lvl2.len().saturating_sub(1));
            let b_end = ((end + 16383) / 16384).min(lvl2.len());
            let mut mn = 0.0f32;
            let mut mx = 0.0f32;
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut count = 0u32;
            for b in &lvl2[b_start..b_end] {
                if b.min < mn {
                    mn = b.min;
                }
                if b.max > mx {
                    mx = b.max;
                }
                r_sum += b.rgb[0] as u32;
                g_sum += b.rgb[1] as u32;
                b_sum += b.rgb[2] as u32;
                count += 1;
            }
            let col = if count > 0 {
                [
                    (r_sum / count) as u8,
                    (g_sum / count) as u8,
                    (b_sum / count) as u8,
                ]
            } else {
                [56, 189, 248]
            };
            (mn, mx, col)
        } else if span >= 1024 && !lvl1.is_empty() {
            let b_start = (start_frame / 1024).min(lvl1.len().saturating_sub(1));
            let b_end = ((end + 1023) / 1024).min(lvl1.len());
            let mut mn = 0.0f32;
            let mut mx = 0.0f32;
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut count = 0u32;
            for b in &lvl1[b_start..b_end] {
                if b.min < mn {
                    mn = b.min;
                }
                if b.max > mx {
                    mx = b.max;
                }
                r_sum += b.rgb[0] as u32;
                g_sum += b.rgb[1] as u32;
                b_sum += b.rgb[2] as u32;
                count += 1;
            }
            let col = if count > 0 {
                [
                    (r_sum / count) as u8,
                    (g_sum / count) as u8,
                    (b_sum / count) as u8,
                ]
            } else {
                [56, 189, 248]
            };
            (mn, mx, col)
        } else if span >= 64 && !lvl0.is_empty() {
            let b_start = (start_frame / 64).min(lvl0.len().saturating_sub(1));
            let b_end = ((end + 63) / 64).min(lvl0.len());
            let mut mn = 0.0f32;
            let mut mx = 0.0f32;
            let mut r_sum = 0u32;
            let mut g_sum = 0u32;
            let mut b_sum = 0u32;
            let mut count = 0u32;
            for b in &lvl0[b_start..b_end] {
                if b.min < mn {
                    mn = b.min;
                }
                if b.max > mx {
                    mx = b.max;
                }
                r_sum += b.rgb[0] as u32;
                g_sum += b.rgb[1] as u32;
                b_sum += b.rgb[2] as u32;
                count += 1;
            }
            let col = if count > 0 {
                [
                    (r_sum / count) as u8,
                    (g_sum / count) as u8,
                    (b_sum / count) as u8,
                ]
            } else {
                [56, 189, 248]
            };
            (mn, mx, col)
        } else {
            let mut mn = 0.0f32;
            let mut mx = 0.0f32;
            for &s in &raw[start_frame..end] {
                if s < mn {
                    mn = s;
                }
                if s > mx {
                    mx = s;
                }
            }
            let col = if !lvl0.is_empty() {
                let b_idx = (start_frame / 64).min(lvl0.len().saturating_sub(1));
                lvl0[b_idx].rgb
            } else {
                [56, 189, 248]
            };
            (mn, mx, col)
        }
    }

    #[inline]
    pub fn query_sample_color(&self, ch: usize, frame: usize) -> [u8; 3] {
        let lvl0 = if ch == 0 {
            &self.peaks_lvl0_ch1
        } else if let Some(ref l0) = self.peaks_lvl0_ch2 {
            l0
        } else {
            &self.peaks_lvl0_ch1
        };
        if lvl0.is_empty() {
            return [56, 189, 248];
        }
        let b_idx = (frame / 64).min(lvl0.len().saturating_sub(1));
        lvl0[b_idx].rgb
    }

    /// Finds the nearest zero-crossing point in the audio around `time_sec`.
    /// Searches within `search_radius_frames` (e.g. 500 frames ≈ 11ms at 44.1kHz).
    /// If stereo, finds where the audio wave passes closest to zero baseline
    /// with preference for positive-going zero crossings to avoid clicks/pops.
    pub fn snap_to_zero_crossing(&self, time_sec: f32, search_radius_frames: usize) -> f32 {
        if self.total_frames == 0 || self.sample_rate == 0 || self.ch1_samples.is_empty() {
            return time_sec;
        }

        let target_frame = (time_sec * self.sample_rate as f32).round().max(0.0) as usize;
        let target_frame = target_frame.min(self.total_frames.saturating_sub(1));

        let start_frame = target_frame.saturating_sub(search_radius_frames);
        let end_frame =
            (target_frame + search_radius_frames).min(self.total_frames.saturating_sub(1));

        if start_frame >= end_frame {
            return target_frame as f32 / self.sample_rate as f32;
        }

        let ch2 = self.ch2_samples.as_deref();

        let mut best_crossing: Option<(usize, f32)> = None;
        let mut best_min_amp = (target_frame, f32::MAX);

        for i in start_frame..end_frame {
            let s1_cur = self.ch1_samples[i];
            let s1_next = self.ch1_samples[i + 1];

            let (s2_cur, _s2_next) = if let Some(c2) = ch2 {
                (c2[i], c2[i + 1])
            } else {
                (0.0, 0.0)
            };

            let amp_cur = s1_cur.abs() + s2_cur.abs();
            let dist = (i as isize - target_frame as isize).abs() as f32;

            if amp_cur < best_min_amp.1 {
                best_min_amp = (i, amp_cur);
            }

            // Zero-crossing check on ch1
            let is_crossing =
                (s1_cur <= 0.0 && s1_next >= 0.0) || (s1_cur >= 0.0 && s1_next <= 0.0);
            if is_crossing {
                let frame_cand = if s1_cur.abs() <= s1_next.abs() {
                    i
                } else {
                    i + 1
                };
                let is_pos_going = s1_cur <= 0.0 && s1_next >= 0.0;
                let pos_bonus = if is_pos_going { 0.0 } else { 5.0 };
                let cost = dist + pos_bonus;
                if let Some((_, best_cost)) = best_crossing {
                    if cost < best_cost {
                        best_crossing = Some((frame_cand, cost));
                    }
                } else {
                    best_crossing = Some((frame_cand, cost));
                }
            }
        }

        let chosen_frame = if let Some((frame, _)) = best_crossing {
            frame
        } else {
            best_min_amp.0
        };

        chosen_frame as f32 / self.sample_rate as f32
    }
}

pub fn compute_plain_waveform(
    path: &Path,
    num_bars: usize,
    audio_config: &AudioConfig,
) -> Option<AudioWaveform> {
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

    let file = std::fs::File::open(path).ok()?;
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
        .map(|c| c.count() as u16)
        .unwrap_or(2);
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);

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

    let mut ch1_chunk_peaks: Vec<f32> = Vec::new();
    let mut ch2_chunk_peaks: Vec<f32> = Vec::new();
    let is_stereo = channels >= 2;

    let chunk_size = 512;
    let mut current_chunk1 = 0.0f32;
    let mut current_chunk2 = 0.0f32;
    let mut chunk_count = 0;
    let mut total_frames = 0u64;

    let max_frames_to_decode = if is_snippet {
        (target_duration_ms as f64 / 1000.0 * sample_rate as f64) as u64
    } else {
        u64::MAX
    };

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
                let n_ch = channels as usize;
                if n_ch == 0 {
                    continue;
                }
                let frames = samples.len() / n_ch;
                total_frames += frames as u64;

                for f in 0..frames {
                    let s1 = samples[f * n_ch].abs();
                    if s1 > current_chunk1 {
                        current_chunk1 = s1;
                    }
                    if is_stereo && n_ch > 1 {
                        let s2 = samples[f * n_ch + 1].abs();
                        if s2 > current_chunk2 {
                            current_chunk2 = s2;
                        }
                    }

                    chunk_count += 1;
                    if chunk_count >= chunk_size {
                        ch1_chunk_peaks.push(current_chunk1);
                        if is_stereo {
                            ch2_chunk_peaks.push(current_chunk2);
                        }
                        current_chunk1 = 0.0;
                        current_chunk2 = 0.0;
                        chunk_count = 0;
                    }
                }

                if is_snippet && total_frames >= max_frames_to_decode {
                    break;
                }
            }
        }
    }

    if chunk_count > 0 {
        ch1_chunk_peaks.push(current_chunk1);
        if is_stereo {
            ch2_chunk_peaks.push(current_chunk2);
        }
    }

    if ch1_chunk_peaks.is_empty() {
        return None;
    }

    let ch1_peaks = resample_peaks(&ch1_chunk_peaks, num_bars);
    let ch2_peaks = if is_stereo {
        Some(resample_peaks(&ch2_chunk_peaks, num_bars))
    } else {
        None
    };

    Some(AudioWaveform {
        channels,
        sample_rate,
        duration_ms: meta.duration_ms,
        ch1_peaks,
        ch2_peaks,
        spectral_colors: None,
        is_snippet,
        snippet_start_ms,
        snippet_duration_ms: target_duration_ms,
    })
}

/// Decodes the entire audio file from start to finish without snippet truncation,
/// generating a full-duration peak waveform suitable for high-res waveform inspection and editing.
pub fn compute_full_waveform(path: &Path, num_bars: usize) -> Option<AudioWaveform> {
    let meta = read_audio_metadata(path)?;
    if num_bars == 0 {
        return None;
    }

    let file = std::fs::File::open(path).ok()?;
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
        .map(|c| c.count() as u16)
        .unwrap_or(2);
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);

    let mut ch1_chunk_peaks: Vec<f32> = Vec::new();
    let mut ch2_chunk_peaks: Vec<f32> = Vec::new();
    let is_stereo = channels >= 2;

    let chunk_size = 512;
    let mut current_chunk1 = 0.0f32;
    let mut current_chunk2 = 0.0f32;
    let mut chunk_count = 0;

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
                let n_ch = channels as usize;
                if n_ch == 0 {
                    continue;
                }
                let frames = samples.len() / n_ch;

                for f in 0..frames {
                    let s1 = samples[f * n_ch].abs();
                    if s1 > current_chunk1 {
                        current_chunk1 = s1;
                    }
                    if is_stereo && n_ch > 1 {
                        let s2 = samples[f * n_ch + 1].abs();
                        if s2 > current_chunk2 {
                            current_chunk2 = s2;
                        }
                    }

                    chunk_count += 1;
                    if chunk_count >= chunk_size {
                        ch1_chunk_peaks.push(current_chunk1);
                        if is_stereo {
                            ch2_chunk_peaks.push(current_chunk2);
                        }
                        current_chunk1 = 0.0;
                        current_chunk2 = 0.0;
                        chunk_count = 0;
                    }
                }
            }
        }
    }

    if chunk_count > 0 {
        ch1_chunk_peaks.push(current_chunk1);
        if is_stereo {
            ch2_chunk_peaks.push(current_chunk2);
        }
    }

    if ch1_chunk_peaks.is_empty() {
        return None;
    }

    let ch1_peaks = resample_peaks(&ch1_chunk_peaks, num_bars);
    let ch2_peaks = if is_stereo {
        Some(resample_peaks(&ch2_chunk_peaks, num_bars))
    } else {
        None
    };

    Some(AudioWaveform {
        channels,
        sample_rate,
        duration_ms: meta.duration_ms,
        ch1_peaks,
        ch2_peaks,
        spectral_colors: None,
        is_snippet: false,
        snippet_start_ms: 0,
        snippet_duration_ms: meta.duration_ms,
    })
}

/// Computes bounded full-track peaks and workbench colors without retaining PCM.
///
/// Decodes planar packets, checking cancellation and a ten-second budget between
/// packets. `fallback_duration` and `max_duration` are in seconds. The latter
/// limits excerpts; `None` summarizes the whole track.
pub fn compute_preview_waveform(
    file: std::fs::File,
    path: &Path,
    fallback_duration: f32,
    bars: usize,
    max_duration: Option<f32>,
    cancelled: &AtomicBool,
) -> Result<(Vec<f32>, Vec<[u8; 3]>), String> {
    use symphonia::core::errors::Error;
    let mut format = preview_audio_format(file, path)?;
    let track = format.default_track().ok_or("No audio track")?;
    let id = track.id;
    let n_frames = track.codec_params.n_frames;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut summary: Option<PreviewWaveform> = None;
    let mut converted: Option<SampleBuffer<f32>> = None;
    let started = std::time::Instant::now();
    loop {
        check_preview_budget(cancelled, started)?;
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(error) => return Err(error.to_string()),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = decoder.decode(&packet).map_err(|e| e.to_string())?;
        let summary = match &mut summary {
            Some(summary) => summary,
            None => summary.insert(PreviewWaveform::from_stream(
                n_frames,
                *decoded.spec(),
                fallback_duration,
                max_duration,
                bars,
            )?),
        };
        summary.push_decoded(decoded, &mut converted)?;
        if summary.complete() {
            break;
        }
    }

    let mut summary = summary.ok_or("No decoded audio samples")?;
    summary.finish_bucket();
    Ok((summary.peaks, summary.colors))
}

// Probing and regular-file validation precede the bounded decode loop.
fn preview_audio_format(
    file: std::fs::File,
    path: &Path,
) -> Result<Box<dyn symphonia::core::formats::FormatReader>, String> {
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Audio source is not a regular file".into());
    }
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let format = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(file), Default::default()),
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?
        .format;
    Ok(format)
}

fn check_preview_budget(cancelled: &AtomicBool, started: std::time::Instant) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Music preview cancelled".into());
    }
    if started.elapsed() > std::time::Duration::from_secs(10) {
        return Err("Waveform exceeded the ten-second decoding budget".into());
    }
    Ok(())
}

// Packet-sized borrowed channel slices keep crossover state hot in the CPU cache.
struct PreviewWaveform {
    peaks: Vec<f32>,
    colors: Vec<[u8; 3]>,
    filters: Vec<WaveformColor>,
    rate: u32,
    frames: usize,
    frame: usize,
    bucket: usize,
}
impl PreviewWaveform {
    fn from_stream(
        n_frames: Option<u64>,
        spec: symphonia::core::audio::SignalSpec,
        fallback_duration: f32,
        max_duration: Option<f32>,
        bars: usize,
    ) -> Result<Self, String> {
        let rate = spec.rate;
        let channels = spec.channels.count();
        let duration = n_frames.map_or(fallback_duration as f64, |frames| {
            frames as f64 / rate as f64
        });
        let duration = max_duration.map_or(duration, |maximum| duration.min(maximum as f64));
        if !duration.is_finite() || duration <= 0.0 || rate == 0 || channels == 0 {
            return Err("Unknown track duration or channel layout".into());
        }
        Ok(Self::new(
            (duration * rate as f64).ceil() as usize,
            channels,
            rate,
            bars,
        ))
    }
    fn new(frames: usize, channels: usize, rate: u32, bars: usize) -> Self {
        Self {
            peaks: vec![0.0; bars.max(1)],
            colors: vec![[56, 189, 248]; bars.max(1)],
            filters: (0..channels)
                .map(|_| WaveformColor::new(rate as f32))
                .collect(),
            rate,
            frames: frames.max(1),
            frame: 0,
            bucket: 0,
        }
    }
    // Conversion belongs at the packet boundary, never in the sample loop.
    fn push_decoded(
        &mut self,
        decoded: symphonia::core::audio::AudioBufferRef<'_>,
        converted: &mut Option<SampleBuffer<f32>>,
    ) -> Result<(), String> {
        use symphonia::core::audio::{AudioBufferRef, Signal};
        if decoded.spec().channels.count() != self.filters.len() || decoded.spec().rate != self.rate
        {
            return Err("Audio format changed during preview".into());
        }
        let frames = decoded.frames();
        if frames == 0 {
            return Ok(());
        }
        if let AudioBufferRef::F32(buffer) = decoded {
            self.push_block(frames, |channel| buffer.chan(channel));
        } else {
            let buffer = converted.get_or_insert_with(|| {
                SampleBuffer::new(decoded.capacity() as u64, *decoded.spec())
            });
            // Formats can vary packet capacity while retaining their channel layout.
            if buffer.capacity() < decoded.capacity() * self.filters.len() {
                *buffer = SampleBuffer::new(decoded.capacity() as u64, *decoded.spec());
            }
            buffer.copy_planar_ref(decoded);
            self.push_block(frames, |channel| {
                &buffer.samples()[channel * frames..(channel + 1) * frames]
            });
        }
        Ok(())
    }
    fn complete(&self) -> bool {
        self.frame >= self.frames
    }
    fn push_block<'a>(&mut self, frames: usize, channel: impl Fn(usize) -> &'a [f32]) {
        let mut offset = 0;
        while offset < frames && !self.complete() {
            let end = ((self.bucket + 1) as u128 * self.frames as u128)
                .div_ceil(self.peaks.len() as u128) as usize;
            let count = (end - self.frame).min(frames - offset);
            for (index, filter) in self.filters.iter_mut().enumerate() {
                let samples = &channel(index)[offset..offset + count];
                let peak = filter.push_samples(samples);
                self.peaks[self.bucket] = self.peaks[self.bucket].max(peak);
            }
            self.frame += count;
            offset += count;
            if self.frame == end {
                self.finish_bucket();
                self.bucket += 1;
            }
        }
    }
    fn finish_bucket(&mut self) {
        if self.bucket >= self.colors.len() {
            return;
        }
        let mut sum = [0_u32; 3];
        for filter in &mut self.filters {
            let color = filter.take_color();
            for i in 0..3 {
                sum[i] += color[i] as u32;
            }
        }
        self.colors[self.bucket] = sum.map(|value| (value / self.filters.len() as u32) as u8);
    }
}

/// Streaming three-band waveform color shared by studio peaks and file previews.
///
/// Retains crossover filter history between blocks, without retaining audio samples.
pub struct WaveformColor {
    alpha_bass: f32,
    alpha_mid: f32,
    lp_bass: f32,
    lp_mid: f32,
    energy: [f32; 3],
    count: usize,
}

impl WaveformColor {
    /// Creates a bass/mid/treble crossover for the sample rate in Hz.
    pub fn new(sample_rate: f32) -> Self {
        let alpha = |cutoff: f32| {
            (1.0 - (-2.0 * std::f32::consts::PI * cutoff / sample_rate.max(1.0)).exp())
                .clamp(0.001, 0.999)
        };
        Self {
            alpha_bass: alpha(250.0),
            alpha_mid: alpha(2500.0),
            lp_bass: 0.0,
            lp_mid: 0.0,
            energy: [0.0; 3],
            count: 0,
        }
    }
    /// Accumulates one sample; non-finite input is treated as silence.
    #[inline]
    pub fn push(&mut self, sample: f32) {
        let s = if sample.is_finite() { sample } else { 0.0 };
        self.lp_bass += self.alpha_bass * (s - self.lp_bass);
        self.lp_mid += self.alpha_mid * (s - self.lp_mid);
        let bands = [self.lp_bass, self.lp_mid - self.lp_bass, s - self.lp_mid];
        for (energy, band) in self.energy.iter_mut().zip(bands) {
            *energy += band * band;
        }
        self.count += 1;
    }
    /// Accumulates a contiguous channel block and returns its bounded absolute peak.
    pub fn push_samples(&mut self, samples: &[f32]) -> f32 {
        let mut peak = 0.0_f32;
        for &sample in samples {
            self.push(sample);
            if sample.is_finite() {
                peak = peak.max(sample.abs().min(1.0));
            }
        }
        peak
    }
    /// Returns this block's color and clears its energies, preserving filter history.
    pub fn take_color(&mut self) -> [u8; 3] {
        let chunk_len = self.count.max(1) as f32;
        let w_bass = (self.energy[0] / chunk_len).sqrt() * 1.2;
        let w_mid = (self.energy[1] / chunk_len).sqrt() * 1.8;
        let w_treble = (self.energy[2] / chunk_len).sqrt() * 2.6;
        let total_w = w_bass + w_mid + w_treble;

        let rgb = if total_w > 1e-5 {
            let rb = (w_bass / total_w).clamp(0.0, 1.0);
            let rm = (w_mid / total_w).clamp(0.0, 1.0);
            let rt = (w_treble / total_w).clamp(0.0, 1.0);
            let r = ((rb * 1.45 + rm * 0.20) * 255.0).clamp(25.0, 255.0) as u8;
            let g = ((rm * 1.45 + rb * 0.15) * 255.0).clamp(25.0, 255.0) as u8;
            let b = (rt * 1.65 * 255.0).clamp(30.0, 255.0) as u8;
            [r, g, b]
        } else {
            [56, 189, 248]
        };

        self.energy = [0.0; 3];
        self.count = 0;
        rgb
    }
}

pub fn build_peak_pyramid(
    samples: &[f32],
    sample_rate: f32,
) -> (Vec<PeakBlock>, Vec<PeakBlock>, Vec<PeakBlock>) {
    build_peak_pyramid_inner(samples, sample_rate, None).unwrap_or_default()
}

pub(crate) fn build_peak_pyramid_cancellable(
    samples: &[f32],
    sample_rate: f32,
    cancelled: &AtomicBool,
) -> Option<(Vec<PeakBlock>, Vec<PeakBlock>, Vec<PeakBlock>)> {
    build_peak_pyramid_inner(samples, sample_rate, Some(cancelled))
}

fn build_peak_pyramid_inner(
    samples: &[f32],
    sample_rate: f32,
    cancelled: Option<&AtomicBool>,
) -> Option<(Vec<PeakBlock>, Vec<PeakBlock>, Vec<PeakBlock>)> {
    let is_cancelled = || cancelled.is_some_and(|flag| flag.load(Ordering::Acquire));
    if samples.is_empty() {
        return Some((Vec::new(), Vec::new(), Vec::new()));
    }

    let mut color = WaveformColor::new(sample_rate);

    // Level 0: 64 samples per block (~1.4ms resolution at 44.1kHz)
    let lvl0_len = (samples.len() + 63) / 64;
    let mut lvl0 = Vec::with_capacity(lvl0_len);
    for (chunk_index, chunk) in samples.chunks(64).enumerate() {
        if chunk_index.is_multiple_of(1024) && is_cancelled() {
            return None;
        }
        let mut mn = 0.0f32;
        let mut mx = 0.0f32;

        for &s in chunk {
            if s < mn {
                mn = s;
            }
            if s > mx {
                mx = s;
            }
            color.push(s);
        }
        let rgb = color.take_color();

        lvl0.push(PeakBlock {
            min: mn,
            max: mx,
            rgb,
        });
    }

    // Level 1: 1024 samples per block (16 lvl0 blocks)
    let lvl1_len = (lvl0.len() + 15) / 16;
    let mut lvl1 = Vec::with_capacity(lvl1_len);
    for (chunk_index, chunk) in lvl0.chunks(16).enumerate() {
        if chunk_index.is_multiple_of(1024) && is_cancelled() {
            return None;
        }
        let mut mn = 0.0f32;
        let mut mx = 0.0f32;
        let mut r_sum = 0u32;
        let mut g_sum = 0u32;
        let mut b_sum = 0u32;
        for b in chunk {
            if b.min < mn {
                mn = b.min;
            }
            if b.max > mx {
                mx = b.max;
            }
            r_sum += b.rgb[0] as u32;
            g_sum += b.rgb[1] as u32;
            b_sum += b.rgb[2] as u32;
        }
        let count = chunk.len() as u32;
        let rgb = if count > 0 {
            [
                (r_sum / count) as u8,
                (g_sum / count) as u8,
                (b_sum / count) as u8,
            ]
        } else {
            [56, 189, 248]
        };
        lvl1.push(PeakBlock {
            min: mn,
            max: mx,
            rgb,
        });
    }

    // Level 2: 16384 samples per block (16 lvl1 blocks)
    let lvl2_len = (lvl1.len() + 15) / 16;
    let mut lvl2 = Vec::with_capacity(lvl2_len);
    for (chunk_index, chunk) in lvl1.chunks(16).enumerate() {
        if chunk_index.is_multiple_of(1024) && is_cancelled() {
            return None;
        }
        let mut mn = 0.0f32;
        let mut mx = 0.0f32;
        let mut r_sum = 0u32;
        let mut g_sum = 0u32;
        let mut b_sum = 0u32;
        for b in chunk {
            if b.min < mn {
                mn = b.min;
            }
            if b.max > mx {
                mx = b.max;
            }
            r_sum += b.rgb[0] as u32;
            g_sum += b.rgb[1] as u32;
            b_sum += b.rgb[2] as u32;
        }
        let count = chunk.len() as u32;
        let rgb = if count > 0 {
            [
                (r_sum / count) as u8,
                (g_sum / count) as u8,
                (b_sum / count) as u8,
            ]
        } else {
            [56, 189, 248]
        };
        lvl2.push(PeakBlock {
            min: mn,
            max: mx,
            rgb,
        });
    }

    Some((lvl0, lvl1, lvl2))
}

fn resample_peaks(source: &[f32], target_len: usize) -> Vec<f32> {
    if source.is_empty() || target_len == 0 {
        return vec![0.0; target_len];
    }
    if source.len() == target_len {
        return source.to_vec();
    }

    let mut result = Vec::with_capacity(target_len);
    let step = source.len() as f64 / target_len as f64;

    for i in 0..target_len {
        let start_idx = (i as f64 * step).floor() as usize;
        let end_idx = (((i + 1) as f64 * step).ceil() as usize).min(source.len());

        let mut peak = 0.0f32;
        for &s in &source[start_idx..end_idx] {
            if s > peak {
                peak = s;
            }
        }
        result.push(peak.clamp(0.02, 1.0));
    }

    // Normalize so highest peak is around 0.95
    let max_val = result.iter().copied().fold(0.0f32, f32::max);
    if max_val > 0.0 {
        for v in &mut result {
            *v = (*v / max_val * 0.95).clamp(0.04, 1.0);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_waveform_synthetic_stereo_matches_scalar_across_packet_boundaries() {
        let rate = 48_000;
        let frames = 384_017;
        let bars = 512;
        let mut left: Vec<f32> = (0..frames)
            .map(|frame| {
                let frequency = [100.0, 1000.0, 10_000.0][(frame / 48_000) % 3];
                (std::f32::consts::TAU * frequency * frame as f32 / rate as f32).sin() * 0.4
            })
            .collect();
        left[frames - 10] = 1.4; // An isolated late peak must not disappear.
        left[123] = f32::NAN;
        left[321] = f32::INFINITY;
        let right: Vec<_> = left.iter().map(|sample| -*sample).collect(); // Opposite phase.
        let mut expected_peaks = Vec::new();
        let mut expected_colors = Vec::new();
        let mut filters = [
            WaveformColor::new(rate as f32),
            WaveformColor::new(rate as f32),
        ];
        let mut start = 0;
        for bucket in 0..bars {
            let end = ((bucket + 1) * frames).div_ceil(bars);
            let mut peak = 0.0_f32;
            for frame in start..end {
                for (index, sample) in [left[frame], right[frame]].into_iter().enumerate() {
                    filters[index].push(sample);
                    if sample.is_finite() {
                        peak = peak.max(sample.abs().min(1.0));
                    }
                }
            }
            let colors = [filters[0].take_color(), filters[1].take_color()];
            expected_colors.push(std::array::from_fn(|i| {
                ((colors[0][i] as u16 + colors[1][i] as u16) / 2) as u8
            }));
            expected_peaks.push(peak);
            start = end;
        }
        for packet_size in [37, 1152, 4096] {
            let started = std::time::Instant::now();
            let mut summary = PreviewWaveform::new(frames, 2, rate, bars);
            for offset in (0..frames).step_by(packet_size) {
                let end = (offset + packet_size).min(frames);
                summary.push_block(end - offset, |channel| {
                    if channel == 0 {
                        &left[offset..end]
                    } else {
                        &right[offset..end]
                    }
                });
            }
            summary.finish_bucket();
            assert!(summary.complete());
            assert_eq!(summary.peaks, expected_peaks, "packet size {packet_size}");
            assert_eq!(summary.colors, expected_colors, "packet size {packet_size}");
            assert_eq!(summary.peaks[bars - 1], 1.0);
            println!(
                "Synthetic stereo preview, {} samples, packet {}: {:?}",
                frames * 2,
                packet_size,
                started.elapsed()
            );
        }
    }

    fn analysis_with_impulse() -> FullAudioAnalysis {
        let mut samples = vec![0.0; 44_100];
        for index in (4_000..samples.len()).step_by(8_000) {
            samples[index] = 0.95;
        }
        FullAudioAnalysis {
            channels: 1,
            sample_rate: 44_100,
            duration_ms: 1_000,
            total_frames: samples.len(),
            ch1_samples: samples.into(),
            ch2_samples: None,
            overview_peaks1: Vec::new(),
            overview_peaks2: None,
            peaks_lvl0_ch1: Vec::new(),
            peaks_lvl0_ch2: None,
            peaks_lvl1_ch1: Vec::new(),
            peaks_lvl1_ch2: None,
            peaks_lvl2_ch1: Vec::new(),
            peaks_lvl2_ch2: None,
            spectrogram_width: 0,
            spectrogram_height: 0,
            spectrogram_rgba: Vec::new(),
            note_strip_width: 0,
            note_strip_height: 0,
            note_strip_rgba: Vec::new(),
            note_strip_info: Vec::new(),
            spectral_colors: Vec::new(),
            transients: vec![0.25],
            is_transients_ready: true,
            is_spectrogram_ready: false,
            ultrasonic_roi: Vec::new(),
            ultrasonic_peak_times: Vec::new(),
            is_truncated: false,
            truncation_warning: None,
        }
    }

    #[test]
    fn transient_sensitivity_can_be_computed_without_mutating_source_analysis() {
        let mut analysis = analysis_with_impulse();
        let original_transients = analysis.transients.clone();
        let adjusted = analysis.transients_with_sensitivity(0.8).unwrap();

        assert_eq!(analysis.transients, original_transients);
        analysis.recompute_transients(0.8);
        assert_eq!(analysis.transients, adjusted);
        assert!(analysis.is_transients_ready);
    }
}
