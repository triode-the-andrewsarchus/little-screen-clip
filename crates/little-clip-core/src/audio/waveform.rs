//! Waveform computation, peak pyramid pyramids, and interactive analysis query methods.

use super::analysis::*;
use super::decode::*;
use super::types::*;
use crate::config::{AudioConfig, AudioSamplePosition};
use std::path::Path;
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
        if self.ch1_samples.is_empty() || self.sample_rate == 0 {
            return;
        }
        let mixed: Vec<f32> = match &self.ch2_samples {
            Some(ch2) => self
                .ch1_samples
                .iter()
                .zip(ch2.iter())
                .map(|(&a, &b)| (a + b) * 0.5)
                .collect(),
            None => self.ch1_samples.clone(),
        };
        self.transients =
            detect_transients_with_sensitivity(&mixed, self.sample_rate as f32, sensitivity);
        self.is_transients_ready = true;
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

pub fn build_peak_pyramid(
    samples: &[f32],
    sample_rate: f32,
) -> (Vec<PeakBlock>, Vec<PeakBlock>, Vec<PeakBlock>) {
    if samples.is_empty() {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    // 3-Band Serato DJ Crossover Filter:
    // Lowpass cutoff: 250 Hz (Red/Orange bass)
    // Highpass cutoff: 2500 Hz (Cyan/Blue highs)
    // Bandpass 250-2500 Hz (Green/Yellow mids)
    let alpha_bass = (1.0 - (-2.0 * std::f32::consts::PI * 250.0 / sample_rate.max(1.0)).exp())
        .clamp(0.001, 0.999);
    let alpha_mid = (1.0 - (-2.0 * std::f32::consts::PI * 2500.0 / sample_rate.max(1.0)).exp())
        .clamp(0.001, 0.999);
    let mut lp_bass = 0.0f32;
    let mut lp_mid = 0.0f32;

    // Level 0: 64 samples per block (~1.4ms resolution at 44.1kHz)
    let lvl0_len = (samples.len() + 63) / 64;
    let mut lvl0 = Vec::with_capacity(lvl0_len);
    for chunk in samples.chunks(64) {
        let mut mn = 0.0f32;
        let mut mx = 0.0f32;
        let mut e_bass = 0.0f32;
        let mut e_mid = 0.0f32;
        let mut e_treble = 0.0f32;

        for &s in chunk {
            if s < mn {
                mn = s;
            }
            if s > mx {
                mx = s;
            }
            lp_bass += alpha_bass * (s - lp_bass);
            lp_mid += alpha_mid * (s - lp_mid);
            let bass = lp_bass;
            let mid = lp_mid - lp_bass;
            let treble = s - lp_mid;
            e_bass += bass * bass;
            e_mid += mid * mid;
            e_treble += treble * treble;
        }

        let chunk_len = chunk.len() as f32;
        let w_bass = (e_bass / chunk_len).sqrt() * 1.2;
        let w_mid = (e_mid / chunk_len).sqrt() * 1.8;
        let w_treble = (e_treble / chunk_len).sqrt() * 2.6;
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

        lvl0.push(PeakBlock {
            min: mn,
            max: mx,
            rgb,
        });
    }

    // Level 1: 1024 samples per block (16 lvl0 blocks)
    let lvl1_len = (lvl0.len() + 15) / 16;
    let mut lvl1 = Vec::with_capacity(lvl1_len);
    for chunk in lvl0.chunks(16) {
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
    for chunk in lvl1.chunks(16) {
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

    (lvl0, lvl1, lvl2)
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
