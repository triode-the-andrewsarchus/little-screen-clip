//! Core audio data structures, metadata, waveforms, and DSP filter chains.

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
    pub ultrasonic_roi: Vec<f32>,
    pub ultrasonic_peak_times: Vec<f32>,
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
        ultrasonic_roi: Vec<f32>,
        ultrasonic_peak_times: Vec<f32>,
    },
    /// All processing stages complete.
    Complete,
    /// An error occurred during audio processing.
    Failed(String),
}

/// Session-only gain and filter settings for one selected timeline region.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioRegionEdit {
    pub start_sec: f32,
    pub end_sec: f32,
    /// Duration of the dry/wet transition extending outward from each region edge.
    /// A region touching the beginning or end of the track has no transition at that edge.
    pub crossfade_sec: f32,
    /// `NEG_INFINITY` represents silence and can be raised again later.
    pub gain_db: f32,
    /// Apply soft clipping only when the selected signal would exceed full scale.
    pub soft_clip: bool,
    pub low_pass_hz: Option<f32>,
    pub high_pass_hz: Option<f32>,
}

impl AudioRegionEdit {
    /// Bounds over which this edit can affect the track, including outward crossfades.
    pub fn effect_bounds(self, duration_sec: f32) -> (f32, f32) {
        let start = self.start_sec.min(self.end_sec).clamp(0.0, duration_sec);
        let end = self.start_sec.max(self.end_sec).clamp(0.0, duration_sec);
        let fade = self.crossfade_sec.max(0.0);
        let fade_start = if start > 0.0 {
            (start - fade).max(0.0)
        } else {
            start
        };
        let fade_end = if end < duration_sec {
            (end + fade).min(duration_sec)
        } else {
            end
        };
        (fade_start, fade_end)
    }

    /// Linear wet-signal mix at a point in the track.
    pub fn effect_mix(self, time_sec: f32, duration_sec: f32) -> f32 {
        let start = self.start_sec.min(self.end_sec).clamp(0.0, duration_sec);
        let end = self.start_sec.max(self.end_sec).clamp(0.0, duration_sec);
        if time_sec >= start && time_sec <= end {
            return 1.0;
        }
        let fade = self.crossfade_sec.max(0.0);
        if fade <= 0.0 {
            return 0.0;
        }
        if start > 0.0 && time_sec < start {
            return (1.0 - (start - time_sec) / fade).clamp(0.0, 1.0);
        }
        if end < duration_sec && time_sec > end {
            return (1.0 - (time_sec - end) / fade).clamp(0.0, 1.0);
        }
        0.0
    }
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
