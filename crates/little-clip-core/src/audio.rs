//! Audio decoding, waveform peak pyramids, spectral analysis, pitch detection, and transient extraction.
//!
//! Integrates Symphonia for decoding diverse audio formats, RealFFT / Rayon for parallel STFT
//! spectrogram generation, logarithmic spectral flux for beat/transient onset detection,
//! YIN algorithm for pitch detection, and Serato/ColorChord harmonic color mapping.

use crate::config::{AudioConfig, AudioSamplePosition};
use rayon::prelude::*;
use realfft::RealFftPlanner;
use rustchord::cc_to_rgb;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Downsampled audio waveform peaks and optional spectral frequency coloring for UI display.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AudioWaveform {
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Total duration in milliseconds.
    pub duration_ms: u64,
    /// Peak amplitude values for channel 1 (values in `[0.0, 1.0]`).
    pub ch1_peaks: Vec<f32>,
    /// Optional peak amplitude values for channel 2.
    pub ch2_peaks: Option<Vec<f32>>,
    /// 3-band spectral frequency colors per bar (`[R, G, B]`).
    pub spectral_colors: Option<Vec<[u8; 3]>>,
    /// Whether this waveform represents a truncated snippet rather than the full file.
    #[serde(default)]
    pub is_snippet: bool,
    /// Millisecond start offset of the snippet within the file.
    #[serde(default)]
    pub snippet_start_ms: u64,
    /// Duration of the snippet in milliseconds.
    #[serde(default)]
    pub snippet_duration_ms: u64,
}

/// Basic audio file metadata extracted via container probing.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioMetadata {
    /// Total duration in milliseconds.
    pub duration_ms: u64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
    /// Container or codec format name (e.g. `"WAV"`, `"FLAC"`, `"MP3"`).
    pub format: String,
}

/// Identified musical pitch note information for a single detected harmonic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SingleNoteInfo {
    /// Note name with octave (e.g. `"C4"`, `"A#3"`).
    pub note_name: String,
    /// Fundamental frequency in Hz.
    pub frequency_hz: f32,
    /// Normalized amplitude in `[0.0, 1.0]`.
    pub amplitude: f32,
    /// Harmonic ColorChord RGB color.
    pub rgb: [u8; 3],
}

/// ColorChord pitch and chord estimation metadata for an audio time column.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ColorChordNoteInfo {
    /// Primary dominant note name (e.g. `"A4"`).
    pub note_name: String,
    /// Primary dominant frequency in Hz.
    pub frequency_hz: f32,
    /// Overall note activity amplitude.
    pub amplitude: f32,
    /// `true` if a musical pitch was detected above threshold.
    pub active: bool,
    /// Harmonic color corresponding to the dominant pitch.
    pub rgb: [u8; 3],
    /// All detected chord components in descending order of energy.
    #[serde(default)]
    pub chord_notes: Vec<SingleNoteInfo>,
}

/// Multi-scale min/max peak summary block with 3-band crossover color.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeakBlock {
    /// Minimum negative sample amplitude in block.
    pub min: f32,
    /// Maximum positive sample amplitude in block.
    pub max: f32,
    /// Crossover filter RGB color.
    #[serde(default)]
    pub rgb: [u8; 3],
}

/// Comprehensive audio analysis data containing raw PCM samples, peak pyramids,
/// spectrogram image, ColorChord note ribbon, spectral colors, and transient onset points.
#[derive(Debug, Clone)]
pub struct FullAudioAnalysis {
    pub channels: u16,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub total_frames: usize,
    pub ch1_samples: Vec<f32>,
    pub ch2_samples: Option<Vec<f32>>,
    pub overview_peaks1: Vec<f32>,
    pub overview_peaks2: Option<Vec<f32>>,
    pub peaks_lvl0_ch1: Vec<PeakBlock>,
    pub peaks_lvl0_ch2: Option<Vec<PeakBlock>>,
    pub peaks_lvl1_ch1: Vec<PeakBlock>,
    pub peaks_lvl1_ch2: Option<Vec<PeakBlock>>,
    pub peaks_lvl2_ch1: Vec<PeakBlock>,
    pub peaks_lvl2_ch2: Option<Vec<PeakBlock>>,
    pub spectrogram_width: usize,
    pub spectrogram_height: usize,
    pub spectrogram_rgba: Vec<u8>,
    pub note_strip_width: usize,
    pub note_strip_height: usize,
    pub note_strip_rgba: Vec<u8>,
    pub note_strip_info: Vec<ColorChordNoteInfo>,
    pub spectral_colors: Vec<[u8; 3]>,
    pub transients: Vec<f32>,
    pub is_transients_ready: bool,
    pub is_spectrogram_ready: bool,
    pub is_truncated: bool,
    pub truncation_warning: Option<String>,
}

/// Incremental progress stages emitted during progressive background audio decoding.
#[derive(Debug, Clone)]
pub enum AudioAnalysisProgress {
    /// Stage 1: Waveform ready! PCM samples, peak pyramids, overview peaks, duration, metadata.
    WaveformReady(Box<FullAudioAnalysis>),
    /// Stage 2: Transients ready!
    TransientsReady(Vec<f32>),
    /// Stage 3: Spectrogram, 3-band colors, and ColorChord note ribbon ready!
    SpectrogramAndNotesReady {
        spectrogram_width: usize,
        spectrogram_height: usize,
        spectrogram_rgba: Vec<u8>,
        note_strip_width: usize,
        note_strip_height: usize,
        note_strip_rgba: Vec<u8>,
        note_strip_info: Vec<ColorChordNoteInfo>,
        spectral_colors: Vec<[u8; 3]>,
    },
    /// All processing stages complete.
    Complete,
    /// An error occurred during audio processing.
    Failed(String),
}

/// Session-only gain and filter settings for one selected timeline region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioRegionEdit {
    pub start_sec: f32,
    pub end_sec: f32,
    /// `NEG_INFINITY` represents silence and can be raised again later.
    pub gain_db: f32,
    /// Apply soft clipping only when the selected signal would exceed full scale.
    pub soft_clip: bool,
    pub low_pass_hz: Option<f32>,
    pub high_pass_hz: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(sample_rate: f32, cutoff_hz: f32, high_pass: bool) -> Self {
        let frequency = cutoff_hz.clamp(10.0, sample_rate * 0.49);
        let omega = 2.0 * std::f32::consts::PI * frequency / sample_rate;
        let (sin, cos) = omega.sin_cos();
        let alpha = sin / std::f32::consts::SQRT_2;
        let (b0, b1, b2) = if high_pass {
            ((1.0 + cos) * 0.5, -(1.0 + cos), (1.0 + cos) * 0.5)
        } else {
            ((1.0 - cos) * 0.5, 1.0 - cos, (1.0 - cos) * 0.5)
        };
        let a0 = 1.0 + alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }
}

/// Stateful two-pole Butterworth filters for one audio channel.
#[derive(Debug, Clone, Default)]
pub struct AudioFilterChain {
    low_pass: Option<Biquad>,
    high_pass: Option<Biquad>,
}

impl AudioFilterChain {
    pub fn new(sample_rate: f32, region: &AudioRegionEdit) -> Self {
        Self {
            low_pass: region
                .low_pass_hz
                .map(|hz| Biquad::new(sample_rate, hz, false)),
            high_pass: region
                .high_pass_hz
                .map(|hz| Biquad::new(sample_rate, hz, true)),
        }
    }

    pub fn process(&mut self, mut sample: f32) -> f32 {
        if let Some(filter) = &mut self.low_pass {
            sample = filter.process(sample);
        }
        if let Some(filter) = &mut self.high_pass {
            sample = filter.process(sample);
        }
        sample
    }
}

/// Apply the same gain curve used by the audio editor preview.
#[inline]
pub fn apply_audio_gain(sample: f32, gain_db: f32, soft_clip: bool) -> f32 {
    if gain_db == 0.0 {
        sample
    } else if gain_db == f32::NEG_INFINITY {
        0.0
    } else {
        let amplified = sample * 10.0_f32.powf(gain_db / 20.0);
        if soft_clip {
            amplified.tanh()
        } else {
            amplified
        }
    }
}

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
    let file = std::fs::File::open(path).ok()?;
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

    // Incremental evaluation of d(tau) and d'(tau) with early minimum break
    for tau in 1..=max_tau {
        let mut sum = 0.0f32;
        let s1 = &samples[0..window_size];
        let s2 = &samples[tau..tau + window_size];
        for i in 0..window_size {
            let diff = s1[i] - s2[i];
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
    if samples.is_empty() || sample_rate <= 0.0 {
        return Vec::new();
    }

    let hop_size = 256;
    let num_hops = samples.len() / hop_size;
    if num_hops < 4 {
        return Vec::new();
    }

    let mut odf = Vec::with_capacity(num_hops);
    let mut prev_l_hf = 0.0f32;
    let mut prev_l_tot = 0.0f32;

    for k in 0..num_hops {
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

    transients
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
    compute_audio_analysis_progressive_with_edits(path, num_cols, &[], on_progress)
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
    compute_audio_analysis_inner(path, num_cols, region_edits, false, on_progress)
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
    compute_audio_analysis_inner(path, num_cols, regions, true, on_progress)
}

fn compute_audio_analysis_inner<F>(
    path: &Path,
    num_cols: usize,
    region_edits: &[AudioRegionEdit],
    spectral_only: bool,
    mut on_progress: F,
) -> Option<FullAudioAnalysis>
where
    F: FnMut(AudioAnalysisProgress),
{
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

    let file = match std::fs::File::open(path) {
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
    if total_frames == 0 {
        on_progress(AudioAnalysisProgress::Failed(
            "Decoded 0 audio frames".to_string(),
        ));
        return None;
    }

    if !region_edits.is_empty() {
        // Earlier selections take precedence when regions overlap, matching the editor view.
        let mut claimed = vec![false; total_frames];
        for region in region_edits {
            let start = ((region.start_sec.min(region.end_sec).max(0.0) * sample_rate as f32)
                as usize)
                .min(total_frames);
            let end = ((region.start_sec.max(region.end_sec).max(0.0) * sample_rate as f32)
                as usize)
                .min(total_frames);
            let mut ch1_filter = AudioFilterChain::new(sample_rate as f32, region);
            let mut ch2_filter = AudioFilterChain::new(sample_rate as f32, region);
            for frame in start..end {
                if !claimed[frame] {
                    claimed[frame] = true;
                    ch1_samples[frame] = ch1_filter.process(apply_audio_gain(
                        ch1_samples[frame],
                        region.gain_db,
                        region.soft_clip,
                    ));
                    if let Some(ch2) = &mut ch2_samples {
                        ch2[frame] = ch2_filter.process(apply_audio_gain(
                            ch2[frame],
                            region.gain_db,
                            region.soft_clip,
                        ));
                    }
                }
            }
        }
    }

    let duration_ms = (total_frames as f64 / sample_rate as f64 * 1000.0) as u64;

    // Concurrently build multi-scale min/max peak pyramids with Serato DJ 3-band crossover colors
    let t_pyramid_start = std::time::Instant::now();
    let (
        (peaks_lvl0_ch1, peaks_lvl1_ch1, peaks_lvl2_ch1),
        (peaks_lvl0_ch2, peaks_lvl1_ch2, peaks_lvl2_ch2),
    ) = if spectral_only {
        ((Vec::new(), Vec::new(), Vec::new()), (None, None, None))
    } else {
        rayon::join(
            || build_peak_pyramid(&ch1_samples, sample_rate as f32),
            || {
                if let Some(ref ch2) = ch2_samples {
                    let (l0, l1, l2) = build_peak_pyramid(ch2, sample_rate as f32);
                    (Some(l0), Some(l1), Some(l2))
                } else {
                    (None, None, None)
                }
            },
        )
    };
    let t_pyramid_elapsed = t_pyramid_start.elapsed();

    // 1. Overview Peaks
    let t_overview_start = std::time::Instant::now();
    let mut overview_peaks1 = Vec::with_capacity(num_cols);
    let mut overview_peaks2 = if is_stereo {
        Some(Vec::with_capacity(num_cols))
    } else {
        None
    };

    for c in 0..if spectral_only { 0 } else { num_cols } {
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

    // Emit Stage 1: Waveform is fully ready for immediate interactive rendering!
    let mut analysis = FullAudioAnalysis {
        channels,
        sample_rate,
        duration_ms,
        total_frames,
        ch1_samples,
        ch2_samples,
        overview_peaks1,
        overview_peaks2,
        peaks_lvl0_ch1,
        peaks_lvl0_ch2,
        peaks_lvl1_ch1,
        peaks_lvl1_ch2,
        peaks_lvl2_ch1,
        peaks_lvl2_ch2,
        spectrogram_width: num_cols,
        spectrogram_height: 256,
        spectrogram_rgba: Vec::new(),
        note_strip_width: num_cols,
        note_strip_height: 48,
        note_strip_rgba: Vec::new(),
        note_strip_info: Vec::new(),
        spectral_colors: Vec::new(),
        transients: Vec::new(),
        is_transients_ready: false,
        is_spectrogram_ready: false,
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
        detect_transients(&mixed_samples, sample_rate as f32)
    };
    let t_transients_elapsed = t_transients_start.elapsed();
    analysis.transients = transients.clone();
    analysis.is_transients_ready = true;
    if !spectral_only {
        on_progress(AudioAnalysisProgress::TransientsReady(transients));
    }

    // Emit Stage 3: Rayon parallel STFT Spectrogram & ColorChord 2-D note strip
    let t_fft_loop_start = std::time::Instant::now();
    let window_size = 512;
    let num_freq_bins = 256;
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
    }

    let col_results: Vec<ColumnAnalysisResult> = (0..num_cols)
        .into_par_iter()
        .map(|c| {
            let mut in_data = r2c.make_input_vec();
            let mut out_data = r2c.make_output_vec();

            let center_frame = (c as f64 / num_cols as f64 * total_frames as f64) as isize;
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
            let mut e_bass = 0.0f32;
            let mut e_mid = 0.0f32;
            let mut e_high = 0.0f32;
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

            ColumnAnalysisResult {
                spec_col_rgba,
                spectral_color,
                note_strip_col_rgba,
                note_info,
            }
        })
        .collect();

    let mut spectrogram_rgba = vec![0u8; spectrogram_width * spectrogram_height * 4];
    let mut note_strip_rgba = vec![0u8; note_strip_width * note_strip_height * 4];
    let mut spectral_colors = Vec::with_capacity(num_cols);
    let mut note_strip_info = Vec::with_capacity(num_cols);

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
    });
    analysis.spectrogram_width = spectrogram_width;
    analysis.spectrogram_height = spectrogram_height;
    analysis.spectrogram_rgba = spectrogram_rgba;
    analysis.note_strip_width = note_strip_width;
    analysis.note_strip_height = note_strip_height;
    analysis.note_strip_rgba = note_strip_rgba;
    analysis.note_strip_info = note_strip_info;
    analysis.spectral_colors = spectral_colors;
    analysis.is_spectrogram_ready = true;

    if spectral_only {
        let ((l0, l1, l2), right) = rayon::join(
            || build_peak_pyramid(&analysis.ch1_samples, sample_rate as f32),
            || {
                analysis
                    .ch2_samples
                    .as_ref()
                    .map(|ch2| build_peak_pyramid(ch2, sample_rate as f32))
            },
        );
        analysis.peaks_lvl0_ch1 = l0;
        analysis.peaks_lvl1_ch1 = l1;
        analysis.peaks_lvl2_ch1 = l2;
        if let Some((l0, l1, l2)) = right {
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

/// Decodes the full audio track and computes complete audio analysis synchronously.
pub fn compute_full_audio_analysis(path: &Path, num_cols: usize) -> Option<FullAudioAnalysis> {
    compute_audio_analysis_progressive(path, num_cols, |_| {})
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
    use std::io::Write;

    #[test]
    fn session_gain_soft_clips_and_can_recover_from_silence() {
        let sample = 0.5;
        assert_eq!(apply_audio_gain(sample, f32::NEG_INFINITY, false), 0.0);
        assert_eq!(apply_audio_gain(sample, 0.0, false), sample);
        let boosted = apply_audio_gain(0.05, 40.0, true);
        assert!(boosted > 0.9 && boosted < 1.0);
        assert_eq!(apply_audio_gain(-0.05, 40.0, true), -boosted);
        assert!(apply_audio_gain(sample, -12.0, false) > 0.0);
        assert!(apply_audio_gain(sample, 1.0, false) > sample);
    }

    #[test]
    fn session_gain_rebuilds_spectrum_and_notes_without_changing_source() {
        let path = generate_test_wav();
        let original_bytes = std::fs::read(&path).unwrap();
        let original = compute_audio_analysis_progressive(&path, 120, |_| {}).unwrap();
        let edited = compute_audio_analysis_progressive_with_edits(
            &path,
            120,
            &[AudioRegionEdit {
                start_sec: 0.25,
                end_sec: 0.75,
                gain_db: f32::NEG_INFINITY,
                soft_clip: false,
                low_pass_hz: None,
                high_pass_hz: None,
            }],
            |_| {},
        )
        .unwrap();
        assert_eq!(edited.ch1_samples[4000], original.ch1_samples[4000]);
        assert_ne!(original.ch1_samples[15000], 0.0);
        assert_eq!(edited.ch1_samples[15000], 0.0);
        let middle_col = 60;
        assert!((0..edited.spectrogram_height).any(|row| {
            let idx = (row * edited.spectrogram_width + middle_col) * 4;
            edited.spectrogram_rgba[idx..idx + 4] != original.spectrogram_rgba[idx..idx + 4]
        }));
        assert!(original.note_strip_info[middle_col].active);
        assert!(!edited.note_strip_info[middle_col].active);
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn spectral_preview_prioritizes_filters_and_attenuates_rejected_frequencies() {
        let path = generate_test_wav();
        let original = compute_audio_analysis_progressive(&path, 120, |_| {}).unwrap();
        let mut stages = Vec::new();
        let filtered = compute_audio_spectral_preview_with_edits(
            &path,
            120,
            &[AudioRegionEdit {
                start_sec: 0.0,
                end_sec: 1.0,
                gain_db: 0.0,
                soft_clip: false,
                low_pass_hz: Some(300.0),
                high_pass_hz: None,
            }],
            |progress| stages.push(progress),
        )
        .unwrap();
        let spectrum_stage = stages
            .iter()
            .position(|p| matches!(p, AudioAnalysisProgress::SpectrogramAndNotesReady { .. }))
            .expect("spectrum update");
        let waveform_stage = stages
            .iter()
            .position(|p| matches!(p, AudioAnalysisProgress::WaveformReady(_)))
            .expect("waveform update");
        assert!(spectrum_stage < waveform_stage);
        assert!(
            !stages
                .iter()
                .any(|p| matches!(p, AudioAnalysisProgress::TransientsReady(_)))
        );
        let raw_treble = original.ch2_samples.as_ref().unwrap();
        let lowpassed_treble = filtered.ch2_samples.as_ref().unwrap();
        let rms = |samples: &[f32]| {
            (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
        };
        assert!(rms(&lowpassed_treble[22_050..]) < rms(&raw_treble[22_050..]) * 0.1);
        assert_ne!(filtered.spectrogram_rgba, original.spectrogram_rgba);

        let highpassed = compute_audio_spectral_preview_with_edits(
            &path,
            120,
            &[AudioRegionEdit {
                start_sec: 0.0,
                end_sec: 1.0,
                gain_db: 0.0,
                soft_clip: false,
                low_pass_hz: None,
                high_pass_hz: Some(1_000.0),
            }],
            |_| {},
        )
        .unwrap();
        assert!(
            rms(&highpassed.ch1_samples[22_050..]) < rms(&original.ch1_samples[22_050..]) * 0.1
        );
        std::fs::remove_file(path).unwrap();
    }

    fn generate_test_wav() -> std::path::PathBuf {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join(format!("test_clip_{}.wav", uuid::Uuid::new_v4()));

        let mut file = std::fs::File::create(&path).expect("failed to create temp wav");
        let sample_rate = 44100u32;
        let num_samples = 44100usize; // 1 second
        let num_channels = 2u16;
        let bits_per_sample = 16u16;

        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        // RIFF header
        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_len.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();

        // fmt subchunk
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap(); // Subchunk1Size (16 for PCM)
        file.write_all(&1u16.to_le_bytes()).unwrap(); // AudioFormat (1 = PCM)
        file.write_all(&num_channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&block_align.to_le_bytes()).unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();

        // data subchunk
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();

        // Write stereo PCM samples: Ch1 = 100Hz bass, Ch2 = 4000Hz treble
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let s1 = (t * 100.0 * 2.0 * std::f32::consts::PI).sin();
            let s2 = (t * 4000.0 * 2.0 * std::f32::consts::PI).sin();

            let val1 = (s1 * 28000.0) as i16;
            let val2 = (s2 * 28000.0) as i16;

            file.write_all(&val1.to_le_bytes()).unwrap();
            file.write_all(&val2.to_le_bytes()).unwrap();
        }

        file.flush().unwrap();
        path
    }

    #[test]
    fn test_audio_metadata_and_waveform() {
        let wav_path = generate_test_wav();

        assert!(is_audio_path(&wav_path));

        let meta = read_audio_metadata(&wav_path).expect("Failed to read audio metadata");
        assert_eq!(meta.channels, 2);
        assert_eq!(meta.sample_rate, 44100);
        assert_eq!(meta.format, "WAV");

        let cfg = AudioConfig::default();
        let waveform =
            compute_plain_waveform(&wav_path, 60, &cfg).expect("Failed to compute plain waveform");
        assert_eq!(waveform.channels, 2);
        assert_eq!(waveform.ch1_peaks.len(), 60);
        assert!(waveform.ch2_peaks.is_some());
        assert_eq!(waveform.ch2_peaks.as_ref().unwrap().len(), 60);
        assert!(waveform.spectral_colors.is_none());
        assert!(!waveform.is_snippet);

        let colors = compute_spectral_colors(&wav_path, 60, &cfg)
            .expect("Failed to compute spectral colors");
        assert_eq!(colors.len(), 60);

        let sample_col = colors[10];
        println!("Sample spectral color: {:?}", sample_col);
        assert!(sample_col[0] > 50 || sample_col[1] > 50 || sample_col[2] > 50);

        // Test snippet extraction on a forced threshold (simulate > 5min file)
        let snippet_cfg = AudioConfig {
            max_full_waveform_sec: 0, // Force snippet preview
            snippet_duration_sec: 1,
            sample_position: AudioSamplePosition::Middle,
            ..Default::default()
        };
        let snip_wf = compute_plain_waveform(&wav_path, 40, &snippet_cfg)
            .expect("Failed to compute snippet waveform");
        assert!(snip_wf.is_snippet);
        assert_eq!(snip_wf.ch1_peaks.len(), 40);

        let snip_colors = compute_spectral_colors(&wav_path, 40, &snippet_cfg)
            .expect("Failed to compute snippet spectral colors");
        assert_eq!(snip_colors.len(), 40);

        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_compute_full_waveform() {
        let wav_path = generate_test_wav();
        let wf = compute_full_waveform(&wav_path, 80).expect("failed to compute full waveform");
        assert_eq!(wf.channels, 2);
        assert_eq!(wf.ch1_peaks.len(), 80);
        assert!(wf.ch2_peaks.is_some());
        assert_eq!(wf.ch2_peaks.as_ref().unwrap().len(), 80);
        assert!(!wf.is_snippet);
        assert_eq!(wf.snippet_start_ms, 0);
        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_rustchord_integration() {
        for i in 0..12 {
            let note_id = i as f32 / 12.0;
            let rgb = rustchord::cc_to_rgb(note_id, 1.0, 1.0);
            assert!(rgb[0] >= 0.0 && rgb[0] <= 1.0);
            assert!(rgb[1] >= 0.0 && rgb[1] <= 1.0);
            assert!(rgb[2] >= 0.0 && rgb[2] <= 1.0);
        }
        // Note 0 (A) is yellow [1.0, 1.0, 0.0]
        let yellow = rustchord::cc_to_rgb(0.0, 1.0, 1.0);
        assert!((yellow[0] - 1.0).abs() < 1e-3);
        assert!((yellow[1] - 1.0).abs() < 1e-3);
        assert!(yellow[2].abs() < 1e-3);

        // Note 6 (D# / F#) has magenta/violet hue
        let d_sharp = rustchord::cc_to_rgb(6.0 / 12.0, 1.0, 1.0);
        assert!((d_sharp[0] - 1.0).abs() < 1e-3);
        assert!((d_sharp[2] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn test_detect_chord_from_fft_chords() {
        let sample_rate = 44100.0f32;
        let window_size = 2048;
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(window_size);

        // 1. Test C Major chord: C4 (261.63 Hz) + E4 (329.63 Hz) + G4 (392.00 Hz)
        let c4 = 261.63f32;
        let e4 = 329.63f32;
        let g4 = 392.00f32;
        let mut in_data = r2c.make_input_vec();
        let mut out_data = r2c.make_output_vec();

        for i in 0..window_size {
            let t = i as f32 / sample_rate;
            let w = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos());
            let s = ((t * c4 * 2.0 * std::f32::consts::PI).sin()
                + (t * e4 * 2.0 * std::f32::consts::PI).sin()
                + (t * g4 * 2.0 * std::f32::consts::PI).sin())
                * 0.33;
            in_data[i] = s * w;
        }
        r2c.process(&mut in_data, &mut out_data).unwrap();

        let notes = detect_chord_from_fft(&out_data, sample_rate, window_size);
        println!("C Major detected notes: {:?}", notes);
        assert!(
            notes.len() >= 3,
            "Expected at least 3 chord components, got {}",
            notes.len()
        );

        let has_c = notes
            .iter()
            .any(|n| n.note_name.starts_with('C') && (n.frequency_hz - c4).abs() / c4 < 0.04);
        let has_e = notes
            .iter()
            .any(|n| n.note_name.starts_with('E') && (n.frequency_hz - e4).abs() / e4 < 0.04);
        let has_g = notes
            .iter()
            .any(|n| n.note_name.starts_with('G') && (n.frequency_hz - g4).abs() / g4 < 0.04);
        assert!(has_c, "C Major missing C note component");
        assert!(has_e, "C Major missing E note component");
        assert!(has_g, "C Major missing G note component");

        // 2. Test A Minor chord: A3 (220.00 Hz) + C4 (261.63 Hz) + E4 (329.63 Hz)
        let a3 = 220.00f32;
        for i in 0..window_size {
            let t = i as f32 / sample_rate;
            let w = 0.5
                * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (window_size - 1) as f32).cos());
            let s = ((t * a3 * 2.0 * std::f32::consts::PI).sin()
                + (t * c4 * 2.0 * std::f32::consts::PI).sin()
                + (t * e4 * 2.0 * std::f32::consts::PI).sin())
                * 0.33;
            in_data[i] = s * w;
        }
        r2c.process(&mut in_data, &mut out_data).unwrap();

        let aminor_notes = detect_chord_from_fft(&out_data, sample_rate, window_size);
        println!("A Minor detected notes: {:?}", aminor_notes);
        assert!(
            aminor_notes.len() >= 3,
            "Expected at least 3 chord components in A Minor"
        );
        let has_a = aminor_notes
            .iter()
            .any(|n| n.note_name.starts_with('A') && (n.frequency_hz - a3).abs() / a3 < 0.04);
        let has_am_c = aminor_notes
            .iter()
            .any(|n| n.note_name.starts_with('C') && (n.frequency_hz - c4).abs() / c4 < 0.04);
        let has_am_e = aminor_notes
            .iter()
            .any(|n| n.note_name.starts_with('E') && (n.frequency_hz - e4).abs() / e4 < 0.04);
        assert!(has_a, "A Minor missing A note component");
        assert!(has_am_c, "A Minor missing C note component");
        assert!(has_am_e, "A Minor missing E note component");
    }

    #[test]
    fn test_compute_full_audio_analysis() {
        let wav_path = generate_test_wav();
        let analysis = compute_full_audio_analysis(&wav_path, 150)
            .expect("failed to compute full audio analysis");

        assert_eq!(analysis.channels, 2);
        assert!(analysis.total_frames > 0);
        assert_eq!(analysis.ch1_samples.len(), analysis.total_frames);
        assert!(analysis.ch2_samples.is_some());
        assert_eq!(
            analysis.ch2_samples.as_ref().unwrap().len(),
            analysis.total_frames
        );

        assert_eq!(analysis.overview_peaks1.len(), 150);
        assert_eq!(analysis.overview_peaks2.as_ref().unwrap().len(), 150);

        assert_eq!(analysis.spectrogram_width, 150);
        assert_eq!(analysis.spectrogram_height, 256);
        assert_eq!(analysis.spectrogram_rgba.len(), 150 * 256 * 4);

        assert_eq!(analysis.note_strip_width, 150);
        assert_eq!(analysis.note_strip_height, 48);
        assert_eq!(analysis.note_strip_rgba.len(), 150 * 48 * 4);
        assert_eq!(analysis.note_strip_info.len(), 150);

        // Check that active notes were detected
        let active_notes: Vec<_> = analysis
            .note_strip_info
            .iter()
            .filter(|n| n.active)
            .collect();
        println!("Detected active notes count: {}", active_notes.len());
        assert!(!active_notes.is_empty());

        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_snap_to_zero_crossing() {
        let wav_path = generate_test_wav();
        let analysis =
            compute_full_audio_analysis(&wav_path, 100).expect("failed to analyze test wav");

        // The test wav has a 100Hz sine wave on Ch1, period = 441 samples (sample_rate = 44100).
        // Zero crossings occur at frames: 0, 220, 441, 661, 882, etc.
        // Let's test snapping from a frame near 225 (which is 225 / 44100 = 0.0051s):
        let test_time = 225.0 / 44100.0;
        let snapped_time = analysis.snap_to_zero_crossing(test_time, 100);
        let snapped_frame = (snapped_time * 44100.0).round() as usize;

        // Snapped frame should be very close to the zero crossing (around frame 220 or 221)
        assert!((snapped_frame as isize - 220).abs() <= 2);

        // Amplitude at the snapped frame should be virtually zero
        let amp_at_snap = analysis.ch1_samples[snapped_frame].abs();
        assert!(amp_at_snap < 0.05);

        let _ = std::fs::remove_file(&wav_path);
    }

    #[test]
    fn test_colorchord_note_estimation_stairstep_c1_to_c6() {
        use std::io::BufWriter;

        let temp_dir = std::env::temp_dir();
        let wav_path = temp_dir.join(format!("test_stairstep_{}.wav", uuid::Uuid::new_v4()));

        let sample_rate = 44100u32;
        let num_notes = 61usize; // C1 (MIDI 24) to C6 (MIDI 84)
        let samples_per_note = 44100usize; // 1.0 second per note
        let total_samples = num_notes * samples_per_note;
        let num_channels = 1u16;
        let bits_per_sample = 16u16;

        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (total_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        let file = std::fs::File::create(&wav_path).expect("failed to create stairstep wav");
        let mut writer = BufWriter::new(file);

        // RIFF header
        writer.write_all(b"RIFF").unwrap();
        writer.write_all(&riff_len.to_le_bytes()).unwrap();
        writer.write_all(b"WAVE").unwrap();

        // fmt subchunk
        writer.write_all(b"fmt ").unwrap();
        writer.write_all(&16u32.to_le_bytes()).unwrap();
        writer.write_all(&1u16.to_le_bytes()).unwrap();
        writer.write_all(&num_channels.to_le_bytes()).unwrap();
        writer.write_all(&sample_rate.to_le_bytes()).unwrap();
        writer.write_all(&byte_rate.to_le_bytes()).unwrap();
        writer.write_all(&block_align.to_le_bytes()).unwrap();
        writer.write_all(&bits_per_sample.to_le_bytes()).unwrap();

        // data subchunk
        writer.write_all(b"data").unwrap();
        writer.write_all(&data_len.to_le_bytes()).unwrap();

        // Generate 61 1-second notes from MIDI 24 (C1, ~32.7 Hz) to MIDI 84 (C6, ~1046.5 Hz)
        for note_idx in 0..num_notes {
            let midi = 24 + note_idx as i32;
            let freq = 440.0 * 2.0f32.powf((midi as f32 - 69.0) / 12.0);
            for s in 0..samples_per_note {
                let t = s as f32 / sample_rate as f32;
                let sample_val = (t * freq * 2.0 * std::f32::consts::PI).sin() * 0.8;
                let sample_i16 = (sample_val * 32000.0) as i16;
                writer.write_all(&sample_i16.to_le_bytes()).unwrap();
            }
        }
        writer.flush().unwrap();
        drop(writer);

        // Analyze audio with 1200 columns (high resolution across 61 seconds)
        let num_cols = 1200;
        let analysis = compute_full_audio_analysis(&wav_path, num_cols)
            .expect("failed to compute analysis for stairstep wav");

        assert_eq!(analysis.duration_ms, 61000);

        let note_names = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];

        // Check each note ~500ms into the note
        let mut all_correct = true;
        for note_idx in 0..num_notes {
            let midi = 24 + note_idx as i32;
            let expected_class = note_names[(midi % 12) as usize];
            let expected_octave = (midi / 12) - 1;
            let expected_name = format!("{}{}", expected_class, expected_octave);
            let expected_freq = 440.0 * 2.0f32.powf((midi as f32 - 69.0) / 12.0);

            let check_time = note_idx as f32 * 1.0 + 0.500;
            let col_idx = (((check_time / 61.0) * num_cols as f32).floor() as usize)
                .min(analysis.note_strip_info.len().saturating_sub(1));
            let info = &analysis.note_strip_info[col_idx];

            let is_match = if expected_freq < 65.0 {
                // Frequencies below 65 Hz are ignored as non-musical sub-bass rumble
                !info.active || info.note_name == "—"
            } else {
                let freq_err_ratio = (info.frequency_hz - expected_freq).abs() / expected_freq;
                info.active && info.note_name == expected_name && freq_err_ratio < 0.05
            };

            if !is_match {
                let freq_err_ratio = (info.frequency_hz - expected_freq).abs() / expected_freq;
                println!(
                    "MISMATCH: Note {} (MIDI {}, expected {} at {:.2} Hz) -> got active={}, name='{}', freq={:.2} Hz (err={:.1}%)",
                    note_idx,
                    midi,
                    expected_name,
                    expected_freq,
                    info.active,
                    info.note_name,
                    info.frequency_hz,
                    freq_err_ratio * 100.0
                );
                all_correct = false;
            }
        }

        let _ = std::fs::remove_file(&wav_path);
        assert!(
            all_correct,
            "Not all notes from C1 to C6 were correctly identified!"
        );
    }

    #[test]
    fn test_profile_deadmau5() {
        let p = std::path::Path::new(
            "/mnt/rust/data/MusicJelly/Deadmau5/Some Chords (Single)/Deadmau5 - Some Chords.mp3",
        );
        if !p.is_file() {
            println!("Deadmau5 file not found, skipping profile test.");
            return;
        }

        let meta = read_audio_metadata(p).expect("metadata");
        println!("\n=== PROFILING DEADMAU5 TRACK ===");
        println!("File: {:?}", p);
        println!(
            "Duration: {} ms ({:.2} min), Sample rate: {} Hz, Channels: {}",
            meta.duration_ms,
            meta.duration_ms as f64 / 60000.0,
            meta.sample_rate,
            meta.channels
        );

        let t_total_start = std::time::Instant::now();
        let analysis = compute_full_audio_analysis(p, 1200).expect("analysis failed");
        let total_elapsed = t_total_start.elapsed();

        println!("Total analysis time: {:?}", total_elapsed);
        println!("Total frames: {}", analysis.total_frames);
        println!("Transients: {}", analysis.transients.len());
        println!("Overview peaks: {}", analysis.overview_peaks1.len());
        println!(
            "Spectrogram RGBA bytes: {}",
            analysis.spectrogram_rgba.len()
        );
        println!("Note strip info count: {}", analysis.note_strip_info.len());
    }

    #[test]
    fn test_detect_silence_bounds() {
        let sample_rate = 1000u32;
        let mut samples = vec![0.0f32; 3000]; // 3 seconds total
        // Add signal from 1.0s to 2.0s (frames 1000 to 2000)
        for i in 1000..2000 {
            samples[i] = 0.5;
        }

        let mut analysis = FullAudioAnalysis {
            channels: 1,
            sample_rate,
            duration_ms: 3000,
            total_frames: 3000,
            ch1_samples: samples,
            ch2_samples: None,
            overview_peaks1: Vec::new(),
            overview_peaks2: None,
            peaks_lvl0_ch1: Vec::new(),
            peaks_lvl0_ch2: None,
            peaks_lvl1_ch1: Vec::new(),
            peaks_lvl1_ch2: None,
            peaks_lvl2_ch1: Vec::new(),
            peaks_lvl2_ch2: None,
            spectrogram_width: 10,
            spectrogram_height: 10,
            spectrogram_rgba: Vec::new(),
            note_strip_width: 10,
            note_strip_height: 10,
            note_strip_rgba: Vec::new(),
            note_strip_info: Vec::new(),
            spectral_colors: Vec::new(),
            transients: Vec::new(),
            is_transients_ready: false,
            is_spectrogram_ready: false,
            is_truncated: false,
            truncation_warning: None,
        };

        let bounds = analysis.detect_silence_bounds(-45.0);
        assert!(bounds.is_some());
        let (start, end) = bounds.unwrap();
        // Should detect start around 1.0s (accounting for 15ms padding = ~0.985s) and end around 2.0s (~2.015s)
        assert!(start <= 1.0 && start >= 0.95);
        assert!(end >= 1.99 && end <= 2.05);

        // Entirely silent track should return None
        analysis.ch1_samples = vec![0.0; 1000];
        assert_eq!(analysis.detect_silence_bounds(-45.0), None);
    }

    #[test]
    fn test_transient_sensitivity_scaling() {
        let sample_rate = 44100.0;
        let mut samples = vec![0.0f32; 44100 * 2]; // 2 seconds
        // Create 4 distinct burst impulses
        for idx in &[5000, 15000, 25000, 35000] {
            for offset in 0..100 {
                samples[idx + offset] = 0.9 * (1.0 - offset as f32 / 100.0);
            }
        }
        // Create a minor subtle attack
        for offset in 0..100 {
            samples[45000 + offset] = 0.15 * (1.0 - offset as f32 / 100.0);
        }

        let normal = detect_transients_with_sensitivity(&samples, sample_rate, 1.0);
        let highly_sensitive = detect_transients_with_sensitivity(&samples, sample_rate, 3.0);
        let low_sensitive = detect_transients_with_sensitivity(&samples, sample_rate, 0.2);

        // More sensitive should detect at least as many or more transients than lower sensitivity
        assert!(highly_sensitive.len() >= normal.len());
        assert!(normal.len() >= low_sensitive.len());
    }

    #[test]
    fn test_default_transient_sensitivity_matches_editor_control() {
        assert_eq!(MIN_TRANSIENT_SENSITIVITY, 0.2);
        assert_eq!(DEFAULT_TRANSIENT_SENSITIVITY, 0.35);
        assert_eq!(MAX_TRANSIENT_SENSITIVITY, 1.0);

        let sample_rate = 48_000.0;
        let mut samples = vec![0.0; 48_000];
        for sample in &mut samples[12_000..12_100] {
            *sample = 0.9;
        }
        assert_eq!(
            detect_transients(&samples, sample_rate),
            detect_transients_with_sensitivity(
                &samples,
                sample_rate,
                DEFAULT_TRANSIENT_SENSITIVITY,
            ),
        );
    }
}
