//! Digital audio decoding for amateur radio, signaling, and telecommunication protocols.
//!
//! This module provides streaming and batch demodulators for extracting data symbols
//! from raw PCM audio buffers across multiple digital modes:
//! - **Morse / CW**: Continuous-wave telegraphy via tone envelope detection and adaptive element classification.
//! - **DTMF**: Dual-tone multi-frequency telephony signalling (ITU-T Q.23 keys 0-9, *, #, A-D) via Goertzel filters.
//! - **Bell 202 AFSK / AX.25 / APRS**: 1200 baud amateur packet radio and APRS beacon frames.
//! - **Bell 103**: 300 baud frequency-shift keying.
//! - **FT8**: Weak-signal 8-FSK protocol across 15-second cycles.
//! - **WSPR**: Weak Signal Propagation Reporter 4-FSK beacon frames.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::warn;

/// Supported digital audio protocol decoder variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DecoderProtocol {
    /// Morse code / Continuous Wave telegraphy.
    CwMorse,
    /// Dual-Tone Multi-Frequency telephony signalling.
    Dtmf,
    /// Bell 202 AFSK amateur packet radio (AX.25 / APRS).
    Bell202Aprs,
    /// Bell 103 300-baud frequency-shift keying.
    Bell103,
    /// FT8 weak-signal 8-FSK protocol.
    Ft8,
    /// WSPR weak-signal 4-FSK beacon mode.
    Wspr,
    /// M17 4-FSK digital radio mode.
    M17,
}

impl DecoderProtocol {
    /// Returns the short user-facing label for this protocol.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::CwMorse => "Morse / CW",
            Self::Dtmf => "DTMF",
            Self::Bell202Aprs => "Bell 202 / APRS",
            Self::Bell103 => "Bell 103",
            Self::Ft8 => "FT8",
            Self::Wspr => "WSPR",
            Self::M17 => "M17",
        }
    }

    /// Returns the recommended UI theme color for logic analyzer chips (`[R, G, B]`).
    #[must_use]
    pub const fn chip_color(&self) -> [u8; 3] {
        match self {
            Self::CwMorse => [245, 158, 11],     // Warm Amber
            Self::Dtmf => [59, 130, 246],        // Vibrant Blue
            Self::Bell202Aprs => [16, 185, 129], // Emerald Green
            Self::Bell103 => [14, 165, 233],     // Sky Blue
            Self::Ft8 => [168, 85, 247],         // Purple
            Self::Wspr => [236, 72, 153],        // Pink
            Self::M17 => [20, 184, 166],         // Teal
        }
    }

    /// Returns all supported protocols in default scanning sequence.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::CwMorse,
            Self::Dtmf,
            Self::Bell202Aprs,
            Self::Bell103,
            Self::Ft8,
            Self::Wspr,
            Self::M17,
        ]
    }
}

impl fmt::Display for DecoderProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// A decoded symbol or packet frame positioned in time.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecodedSymbol {
    /// Protocol used to decode this symbol.
    pub protocol: DecoderProtocol,
    /// Concise text label for display on the timeline bar (e.g. `"E"`, `"4"`, `"APRS [N0CALL]"`).
    pub symbol: String,
    /// Start timestamp within the audio buffer in seconds.
    pub start_sec: f32,
    /// End timestamp within the audio buffer in seconds.
    pub end_sec: f32,
    /// Confidence or signal metric in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Extended metadata or packet payload details.
    pub details: Option<String>,
}

/// Downmixes multi-channel interleaved audio to mono `f32` samples.
#[must_use]
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let ch = channels as usize;
    let frames = interleaved.len() / ch;
    let mut mono = Vec::with_capacity(frames);
    let inv_ch = 1.0 / (channels as f32);

    for frame in interleaved.chunks_exact(ch) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum * inv_ch);
    }
    mono
}

/// Resamples a single-channel audio slice from `src_rate` to `dst_rate` using linear interpolation.
#[must_use]
pub fn resample_linear(mono_samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if mono_samples.is_empty() || src_rate == dst_rate {
        return mono_samples.to_vec();
    }
    let ratio = (src_rate as f64) / (dst_rate as f64);
    let src_len = mono_samples.len();
    let dst_len = ((src_len as f64) / ratio).round() as usize;
    let mut resampled = Vec::with_capacity(dst_len);

    for i in 0..dst_len {
        let src_pos = (i as f64) * ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - (idx as f64)) as f32;

        if idx + 1 < src_len {
            let s0 = mono_samples[idx];
            let s1 = mono_samples[idx + 1];
            resampled.push(s0 + frac * (s1 - s0));
        } else if idx < src_len {
            resampled.push(mono_samples[idx]);
        } else {
            resampled.push(0.0);
        }
    }

    resampled
}

/// Converts normalized `[-1.0, 1.0]` `f32` PCM samples into signed 16-bit integers.
#[must_use]
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * 32767.0).round() as i16
        })
        .collect()
}

// ----------------------------------------------------------------------------
// DTMF Decoder (Goertzel Filter Bank)
// ----------------------------------------------------------------------------

/// Standard DTMF low and high tone frequency groups in Hz.
const DTMF_LOW_FREQS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];
const DTMF_HIGH_FREQS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];

/// Standard DTMF keypad matrix mapped by row and column indices.
const DTMF_KEYS: [[char; 4]; 4] = [
    ['1', '2', '3', 'A'],
    ['4', '5', '6', 'B'],
    ['7', '8', '9', 'C'],
    ['*', '0', '#', 'D'],
];

/// Computes the Goertzel filter power at a target frequency for a block of samples.
fn goertzel_power(samples: &[f32], target_freq: f32, sample_rate: f32) -> f32 {
    let n = samples.len() as f32;
    let k = (0.5 + ((n * target_freq) / sample_rate)).floor();
    let omega = (2.0 * std::f32::consts::PI * k) / n;
    let coeff = 2.0 * omega.cos();

    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;

    for &sample in samples {
        let s0 = sample + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }

    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Decodes DTMF telephony tones from an 8000 Hz mono audio stream.
///
/// # Arguments
/// * `samples` - Mono audio samples at 8000 Hz.
/// * `sample_rate` - Sample rate in Hz (should be 8000).
#[must_use]
pub fn decode_dtmf(samples: &[f32], sample_rate: u32) -> Vec<DecodedSymbol> {
    if samples.is_empty() {
        return Vec::new();
    }

    let rate = sample_rate as f32;
    let window_size = 205; // ~25.6 ms at 8000 Hz
    let hop_size = 80; // 10 ms step

    let mut detected_frames = Vec::new();
    let num_windows = if samples.len() >= window_size {
        (samples.len() - window_size) / hop_size + 1
    } else {
        0
    };

    for w in 0..num_windows {
        let start = w * hop_size;
        let window = &samples[start..start + window_size];

        // Total signal energy in this window
        let total_energy: f32 = window.iter().map(|&s| s * s).sum();
        if total_energy < 0.02 {
            detected_frames.push(None);
            continue;
        }

        // Evaluate Goertzel power across the 4 low and 4 high tones
        let mut low_powers = [0.0f32; 4];
        let mut high_powers = [0.0f32; 4];

        for (i, &f) in DTMF_LOW_FREQS.iter().enumerate() {
            low_powers[i] = goertzel_power(window, f, rate);
        }
        for (i, &f) in DTMF_HIGH_FREQS.iter().enumerate() {
            high_powers[i] = goertzel_power(window, f, rate);
        }

        let mut sorted_low = low_powers;
        sorted_low.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let max_low_power = sorted_low[0];
        let second_low_power = sorted_low[1];
        let max_low_idx = low_powers
            .iter()
            .position(|&p| p == max_low_power)
            .unwrap_or(0);

        let mut sorted_high = high_powers;
        sorted_high.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let max_high_power = sorted_high[0];
        let second_high_power = sorted_high[1];
        let max_high_idx = high_powers
            .iter()
            .position(|&p| p == max_high_power)
            .unwrap_or(0);

        // Standard validation criteria:
        // 1. Dominance: Peak power must exceed second peak by at least 4x (~6 dB)
        let low_dominance = max_low_power > second_low_power * 3.0;
        let high_dominance = max_high_power > second_high_power * 3.0;

        // 2. Twist: High-to-low power ratio must be between -8 dB (0.16) and +4 dB (2.5)
        let twist = max_high_power / max_low_power.max(1e-6);
        let valid_twist = (0.12..=4.0).contains(&twist);

        // 3. Combined energy ratio
        let combined_tone_energy = (max_low_power + max_high_power) / (window_size as f32);
        let snr_valid = combined_tone_energy / total_energy.max(1e-6) > 0.08;

        if low_dominance && high_dominance && valid_twist && snr_valid {
            let key = DTMF_KEYS[max_low_idx][max_high_idx];
            let low_freq = DTMF_LOW_FREQS[max_low_idx];
            let high_freq = DTMF_HIGH_FREQS[max_high_idx];
            let confidence =
                ((combined_tone_energy / total_energy.max(1e-6)).clamp(0.0, 1.0) * 0.7 + 0.3)
                    .min(1.0);
            detected_frames.push(Some((key, low_freq, high_freq, confidence)));
        } else {
            detected_frames.push(None);
        }
    }

    // Debounce and merge consecutive detected frames into distinct key bursts
    let mut symbols = Vec::new();
    let min_frames_per_symbol = 3; // ~30 ms minimum burst duration
    let min_inter_symbol_frames = 3; // ~30 ms minimum silence interval

    let mut current_key: Option<char> = None;
    let mut current_start_frame = 0;
    let mut current_frame_count = 0;
    let mut current_low_freq = 0.0;
    let mut current_high_freq = 0.0;
    let mut current_confidence_sum = 0.0;
    let mut silent_frames = 0;

    for (w, frame) in detected_frames.iter().enumerate() {
        match frame {
            Some((key, l_freq, h_freq, conf)) => {
                silent_frames = 0;
                if let Some(active_k) = current_key {
                    if active_k == *key {
                        current_frame_count += 1;
                        current_confidence_sum += conf;
                    } else {
                        // Key changed without silence
                        if current_frame_count >= min_frames_per_symbol {
                            let start_sec = (current_start_frame * hop_size) as f32 / rate;
                            let end_sec = ((current_start_frame + current_frame_count) * hop_size
                                + window_size) as f32
                                / rate;
                            let avg_conf = current_confidence_sum / (current_frame_count as f32);
                            symbols.push(DecodedSymbol {
                                protocol: DecoderProtocol::Dtmf,
                                symbol: active_k.to_string(),
                                start_sec,
                                end_sec,
                                confidence: avg_conf,
                                details: Some(format!(
                                    "Tone: {:.0} Hz + {:.0} Hz, Duration: {:.0} ms",
                                    current_low_freq,
                                    current_high_freq,
                                    (end_sec - start_sec) * 1000.0
                                )),
                            });
                        }
                        current_key = Some(*key);
                        current_start_frame = w;
                        current_frame_count = 1;
                        current_low_freq = *l_freq;
                        current_high_freq = *h_freq;
                        current_confidence_sum = *conf;
                    }
                } else {
                    current_key = Some(*key);
                    current_start_frame = w;
                    current_frame_count = 1;
                    current_low_freq = *l_freq;
                    current_high_freq = *h_freq;
                    current_confidence_sum = *conf;
                }
            }
            None => {
                silent_frames += 1;
                if silent_frames >= min_inter_symbol_frames
                    && let Some(active_k) = current_key
                {
                    if current_frame_count >= min_frames_per_symbol {
                        let start_sec = (current_start_frame * hop_size) as f32 / rate;
                        let end_sec = ((current_start_frame + current_frame_count) * hop_size
                            + window_size) as f32
                            / rate;
                        let avg_conf = current_confidence_sum / (current_frame_count as f32);
                        symbols.push(DecodedSymbol {
                            protocol: DecoderProtocol::Dtmf,
                            symbol: active_k.to_string(),
                            start_sec,
                            end_sec,
                            confidence: avg_conf,
                            details: Some(format!(
                                "Tone: {:.0} Hz + {:.0} Hz, Duration: {:.0} ms",
                                current_low_freq,
                                current_high_freq,
                                (end_sec - start_sec) * 1000.0
                            )),
                        });
                    }
                    current_key = None;
                    current_frame_count = 0;
                }
            }
        }
    }

    if let Some(active_k) = current_key
        && current_frame_count >= min_frames_per_symbol
    {
        let start_sec = (current_start_frame * hop_size) as f32 / rate;
        let end_sec =
            ((current_start_frame + current_frame_count) * hop_size + window_size) as f32 / rate;
        let avg_conf = current_confidence_sum / (current_frame_count as f32);
        symbols.push(DecodedSymbol {
            protocol: DecoderProtocol::Dtmf,
            symbol: active_k.to_string(),
            start_sec,
            end_sec,
            confidence: avg_conf,
            details: Some(format!(
                "Tone: {:.0} Hz + {:.0} Hz, Duration: {:.0} ms",
                current_low_freq,
                current_high_freq,
                (end_sec - start_sec) * 1000.0
            )),
        });
    }

    symbols
}

// ----------------------------------------------------------------------------
// CW / Morse Code Decoder
// ----------------------------------------------------------------------------

/// Detects the dominant audio tone frequency in the Morse telegraphy band (400..1200 Hz).
#[must_use]
pub fn detect_cw_peak_frequency(samples: &[f32], sample_rate: u32) -> f32 {
    if samples.len() < 256 {
        return 700.0;
    }
    let rate = sample_rate as f32;
    let search_window = &samples[..samples.len().min(8000)]; // up to 1 second

    let mut best_freq = 700.0f32;
    let mut best_power = 0.0f32;

    // Search in 20 Hz steps across 400..1200 Hz
    let mut freq = 400.0f32;
    while freq <= 1200.0 {
        let p = goertzel_power(search_window, freq, rate);
        if p > best_power {
            best_power = p;
            best_freq = freq;
        }
        freq += 20.0;
    }

    best_freq
}

/// Returns the International Morse code elements (dots and dashes) for an alphanumeric character.
#[must_use]
pub fn char_to_morse(c: char) -> Option<&'static str> {
    match c.to_ascii_uppercase() {
        'A' => Some(".-"),
        'B' => Some("-..."),
        'C' => Some("-.-."),
        'D' => Some("-.."),
        'E' => Some("."),
        'F' => Some("..-."),
        'G' => Some("--."),
        'H' => Some("...."),
        'I' => Some(".."),
        'J' => Some(".---"),
        'K' => Some("-.-"),
        'L' => Some(".-.."),
        'M' => Some("--"),
        'N' => Some("-."),
        'O' => Some("---"),
        'P' => Some(".--."),
        'Q' => Some("--.-"),
        'R' => Some(".-."),
        'S' => Some("..."),
        'T' => Some("-"),
        'U' => Some("..-"),
        'V' => Some("...-"),
        'W' => Some(".--"),
        'X' => Some("-..-"),
        'Y' => Some("-.--"),
        'Z' => Some("--.."),
        '1' => Some(".----"),
        '2' => Some("..---"),
        '3' => Some("...--"),
        '4' => Some("....-"),
        '5' => Some("....."),
        '6' => Some("-...."),
        '7' => Some("--..."),
        '8' => Some("---.."),
        '9' => Some("----."),
        '0' => Some("-----"),
        '/' => Some("-..-."),
        '?' => Some("..--.."),
        '.' => Some(".-.-.-"),
        ',' => Some("--..--"),
        '=' => Some("-...-"),
        '+' => Some(".-.-."),
        '-' => Some("-....-"),
        _ => None,
    }
}

/// Formats a decoded Morse character together with its dot-dash representation.
fn format_morse_symbol(c: char) -> String {
    if let Some(code) = char_to_morse(c) {
        format!("{} ({})", c, code)
    } else {
        c.to_string()
    }
}

/// Decodes Morse telegraphy characters from audio using `cwdit-dsp` envelope detection
/// and `cwdit-morse` timing classification.
#[must_use]
pub fn decode_morse_cw(samples: &[f32], sample_rate: u32) -> Vec<DecodedSymbol> {
    if samples.is_empty() {
        return Vec::new();
    }

    let resampled = resample_linear(samples, sample_rate, 8000);
    let target_rate = 8000.0f32;
    let tone_freq = detect_cw_peak_frequency(&resampled, 8000);

    // Compute envelope using sliding Goertzel blocks of 80 samples (10 ms)
    let block_size = 80;
    let blocks = resampled.len() / block_size;
    if blocks < 10 {
        return Vec::new();
    }

    let mut envelope = Vec::with_capacity(blocks);
    let mut max_env = 0.0f32;
    let mut avg_env = 0.0f32;

    for b in 0..blocks {
        let chunk = &resampled[b * block_size..(b + 1) * block_size];
        let p = goertzel_power(chunk, tone_freq, target_rate).sqrt();
        envelope.push(p);
        if p > max_env {
            max_env = p;
        }
        avg_env += p;
    }
    avg_env /= blocks as f32;

    // Slicing threshold between noise floor and signal peak
    let threshold = avg_env + (max_env - avg_env) * 0.35;
    if max_env < 0.005 || max_env / avg_env.max(1e-6) < 1.5 {
        return Vec::new(); // Insufficient tone contrast
    }

    // Convert envelope to runs of (is_mark: bool, duration_blocks: usize)
    let mut runs: Vec<(bool, usize, usize)> = Vec::new(); // (mark, length_blocks, start_block)
    let mut current_mark = envelope[0] > threshold;
    let mut run_start = 0;
    let mut run_len = 0;

    for (b, &val) in envelope.iter().enumerate() {
        let mark = val > threshold;
        if mark == current_mark {
            run_len += 1;
        } else {
            runs.push((current_mark, run_len, run_start));
            current_mark = mark;
            run_start = b;
            run_len = 1;
        }
    }
    if run_len > 0 {
        runs.push((current_mark, run_len, run_start));
    }

    // Initialize cwdit_morse streaming decoder with adaptive timing
    let mut decoder = cwdit_morse::Decoder::new(cwdit_morse::TimingEstimator::from_unit(5));
    let mut symbols = Vec::new();

    let mut char_start_block: Option<usize> = None;
    let mut char_end_block = 0;

    for (mark, length, start_b) in runs {
        if mark {
            if char_start_block.is_none() {
                char_start_block = Some(start_b);
            }
            char_end_block = start_b + length;
        }

        let batch = decoder.push(mark, length as u32);
        for item in batch {
            match item {
                cwdit_morse::Decoded::Char(c) => {
                    let s_b = char_start_block.unwrap_or(start_b);
                    let e_b = char_end_block.max(s_b + 1);
                    let start_sec = (s_b * block_size) as f32 / target_rate;
                    let end_sec = (e_b * block_size) as f32 / target_rate;
                    let symbol = format_morse_symbol(c);

                    symbols.push(DecodedSymbol {
                        protocol: DecoderProtocol::CwMorse,
                        symbol: symbol.clone(),
                        start_sec,
                        end_sec,
                        confidence: 0.90,
                        details: Some(format!(
                            "Morse: '{}', Frequency: {:.0} Hz",
                            symbol, tone_freq
                        )),
                    });
                    char_start_block = None;
                }
                cwdit_morse::Decoded::WordBreak | cwdit_morse::Decoded::Unknown => {
                    char_start_block = None;
                }
            }
        }
    }

    for item in decoder.finish() {
        if let cwdit_morse::Decoded::Char(c) = item {
            let s_b = char_start_block.unwrap_or(0);
            let e_b = char_end_block.max(s_b + 1);
            let start_sec = (s_b * block_size) as f32 / target_rate;
            let end_sec = (e_b * block_size) as f32 / target_rate;
            let symbol = format_morse_symbol(c);

            symbols.push(DecodedSymbol {
                protocol: DecoderProtocol::CwMorse,
                symbol: symbol.clone(),
                start_sec,
                end_sec,
                confidence: 0.90,
                details: Some(format!(
                    "Morse: '{}', Frequency: {:.0} Hz",
                    symbol, tone_freq
                )),
            });
        }
    }

    symbols
}

// ----------------------------------------------------------------------------
// Yodel Protocol Decoders (Bell 202, APRS, FT8, WSPR)
// ----------------------------------------------------------------------------

/// Decodes Bell 202 AFSK / AX.25 / APRS frames using `yodel::tnc::TncReceiver`.
#[must_use]
pub fn decode_bell202_aprs(samples: &[f32], sample_rate: u32) -> Vec<DecodedSymbol> {
    if samples.is_empty() {
        return Vec::new();
    }

    // Bell 202 in yodel operates cleanly at 48000 Hz or 44100 Hz
    let target_rate = 48000u32;
    let resampled = resample_linear(samples, sample_rate, target_rate);
    let i16_samples = f32_to_i16(&resampled);

    let sample_rate_val = match yodel::SampleRate::new(target_rate) {
        Ok(sr) => sr,
        Err(err) => {
            warn!("Invalid sample rate for Bell 202: {err}");
            return Vec::new();
        }
    };

    let cfg = match yodel::tnc::TncConfig::bell_202(sample_rate_val) {
        Ok(c) => c,
        Err(err) => {
            warn!("Failed to configure TNC: {err}");
            return Vec::new();
        }
    };

    let mut rx: yodel::tnc::DefaultTncReceiver = match yodel::tnc::TncReceiver::new(cfg) {
        Ok(r) => r,
        Err(err) => {
            warn!("Failed to instantiate TNC receiver: {err}");
            return Vec::new();
        }
    };

    let mut symbols = Vec::new();
    let rate_f = target_rate as f32;

    for (sample_idx, &s) in i16_samples.iter().enumerate() {
        if let Some(frame) = rx.push_i16(s) {
            let end_sec = sample_idx as f32 / rate_f;
            let info_len = frame.info().len();
            // Estimate frame duration based on 1200 baud (approx 8.33 ms per byte)
            let frame_duration = (((frame.info().len() + 18) as f32) * 8.0 / 1200.0).max(0.05);
            let start_sec = (end_sec - frame_duration).max(0.0);

            let format_addr = |addr: &yodel::ax25::Address| -> String {
                let call = String::from_utf8_lossy(addr.callsign.as_bytes())
                    .trim()
                    .to_string();
                if addr.ssid.value() > 0 {
                    format!("{}-{}", call, addr.ssid.value())
                } else {
                    call
                }
            };
            let src_call = format_addr(&frame.src());
            let dst_call = format_addr(&frame.dest());

            let (label, details) = if let Ok(aprs_pkt) = frame.aprs() {
                match aprs_pkt {
                    yodel::aprs::AprsPacket::Position(pos) => {
                        let comment = String::from_utf8_lossy(pos.comment).to_string();
                        (
                            format!("{}>{} [Pos]", src_call, dst_call),
                            format!(
                                "Position beacon: {:.4}°, {:.4}°. Comment: {}",
                                pos.latitude.to_degrees(),
                                pos.longitude.to_degrees(),
                                comment
                            ),
                        )
                    }
                    yodel::aprs::AprsPacket::Message(msg) => match msg.content {
                        yodel::aprs::message::MessageContent::Text { text, .. } => {
                            let body = String::from_utf8_lossy(text).to_string();
                            let to = String::from_utf8_lossy(msg.addressee.as_bytes()).to_string();
                            (
                                format!("{}>{} [Msg: {}]", src_call, to, body),
                                format!("Message to {}: {}", to, body),
                            )
                        }
                        yodel::aprs::message::MessageContent::Ack { id } => {
                            let id_str = String::from_utf8_lossy(id);
                            (
                                format!("{}>{} [Ack {}]", src_call, dst_call, id_str),
                                format!("Ack message id: {}", id_str),
                            )
                        }
                        yodel::aprs::message::MessageContent::Reject { id } => {
                            let id_str = String::from_utf8_lossy(id);
                            (
                                format!("{}>{} [Rej {}]", src_call, dst_call, id_str),
                                format!("Reject message id: {}", id_str),
                            )
                        }
                    },
                    yodel::aprs::AprsPacket::Status(status) => {
                        let text = String::from_utf8_lossy(status.text).to_string();
                        (
                            format!("{}>{} [Status]", src_call, dst_call),
                            format!("Status report: {}", text),
                        )
                    }
                    yodel::aprs::AprsPacket::Weather(_) => (
                        format!("{}>{} [Wx]", src_call, dst_call),
                        "Weather report".to_string(),
                    ),
                    _ => {
                        let info = String::from_utf8_lossy(frame.info()).to_string();
                        (format!("{}>{}", src_call, dst_call), info)
                    }
                }
            } else {
                let info = String::from_utf8_lossy(frame.info()).to_string();
                (format!("{}>{}", src_call, dst_call), info)
            };

            symbols.push(DecodedSymbol {
                protocol: DecoderProtocol::Bell202Aprs,
                symbol: label,
                start_sec,
                end_sec,
                confidence: 0.98,
                details: Some(format!("Payload ({} bytes): {}", info_len, details)),
            });
        }
    }

    symbols
}

/// Decodes FT8 transmissions if the audio buffer contains at least 12.64 seconds of audio.
#[must_use]
pub fn decode_ft8(samples: &[f32], sample_rate: u32) -> Vec<DecodedSymbol> {
    let min_duration_sec = 12.64;
    let total_duration = samples.len() as f32 / sample_rate as f32;
    if total_duration < min_duration_sec {
        return Vec::new();
    }

    // FT8 receive engine operates strictly on 12 kHz mono audio
    let resampled = resample_linear(samples, sample_rate, 12000);
    let i16_samples = f32_to_i16(&resampled);

    let config = match yodel::ft8::Ft8DecoderConfig::new(1500, 2000) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let decoder = yodel::ft8::Ft8Decoder::new(config);
    let mut symbols = Vec::new();

    if let Ok(decodes) = decoder.decode(&i16_samples) {
        for dec in decodes {
            let start_sec = dec.dt_seconds.max(0.0);
            let end_sec = (start_sec + 12.64).min(total_duration);
            let msg_text = dec.message.as_str().to_string();

            symbols.push(DecodedSymbol {
                protocol: DecoderProtocol::Ft8,
                symbol: msg_text.clone(),
                start_sec,
                end_sec,
                confidence: (dec.sync_score.clamp(0.0, 1.0) * 0.5 + 0.5).min(1.0),
                details: Some(format!(
                    "FT8: '{}', Freq: {:.1} Hz, SNR: {:.1} dB, Sync: {:.2}",
                    msg_text, dec.freq_hz, dec.snr_db, dec.sync_score
                )),
            });
        }
    }

    symbols
}

/// Decodes WSPR transmissions if the audio buffer contains at least 110 seconds of audio.
#[must_use]
pub fn decode_wspr(samples: &[f32], sample_rate: u32) -> Vec<DecodedSymbol> {
    let min_duration_sec = 110.0;
    let total_duration = samples.len() as f32 / sample_rate as f32;
    if total_duration < min_duration_sec {
        return Vec::new();
    }

    let resampled = resample_linear(samples, sample_rate, 12000);
    let i16_samples = f32_to_i16(&resampled);

    let config = match yodel::wspr::WsprDecoderConfig::new(1500, 200) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let decoder = yodel::wspr::WsprDecoder::new(config);
    let mut symbols = Vec::new();

    if let Ok(decodes) = decoder.decode(&i16_samples) {
        for dec in decodes {
            let start_sec = dec.dt_seconds.max(0.0);
            let end_sec = (start_sec + 110.6).min(total_duration);
            let call = String::from_utf8_lossy(dec.message.callsign())
                .trim()
                .to_string();
            let grid = String::from_utf8_lossy(dec.message.grid().as_bytes()).to_string();
            let power = dec.message.power_dbm();

            symbols.push(DecodedSymbol {
                protocol: DecoderProtocol::Wspr,
                symbol: format!("{} {} {}dBm", call, grid, power),
                start_sec,
                end_sec,
                confidence: 0.95,
                details: Some(format!(
                    "WSPR: Station {}, Grid {}, Power {} dBm, Freq: {:.1} Hz, SNR: {:.1} dB",
                    call, grid, power, dec.freq_hz, dec.snr_db
                )),
            });
        }
    }

    symbols
}

// ----------------------------------------------------------------------------
// Full Parallel / Streaming Scanner
// ----------------------------------------------------------------------------

/// Scans the entire audio buffer across all supported decoders in parallel,
/// invoking `on_batch` whenever a decoder yields symbols.
pub fn scan_digital_decode_streaming<F>(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    cancel_flag: Option<Arc<AtomicBool>>,
    mut on_batch: F,
) where
    F: FnMut(Vec<DecodedSymbol>),
{
    let mono = downmix_to_mono(samples, channels);
    if mono.is_empty() {
        return;
    }

    let is_cancelled = || {
        cancel_flag
            .as_ref()
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    };

    // 1. DTMF Decoder
    if is_cancelled() {
        return;
    }
    let dtmf_mono = resample_linear(&mono, sample_rate, 8000);
    let dtmf_symbols = decode_dtmf(&dtmf_mono, 8000);
    if !dtmf_symbols.is_empty() {
        on_batch(dtmf_symbols);
    }

    // 2. Morse / CW Decoder
    if is_cancelled() {
        return;
    }
    let cw_symbols = decode_morse_cw(&mono, sample_rate);
    if !cw_symbols.is_empty() {
        on_batch(cw_symbols);
    }

    // 3. Bell 202 / APRS Decoder
    if is_cancelled() {
        return;
    }
    let aprs_symbols = decode_bell202_aprs(&mono, sample_rate);
    if !aprs_symbols.is_empty() {
        on_batch(aprs_symbols);
    }

    // 4. FT8 Decoder (runs if buffer >= 12.64s)
    if is_cancelled() {
        return;
    }
    let ft8_symbols = decode_ft8(&mono, sample_rate);
    if !ft8_symbols.is_empty() {
        on_batch(ft8_symbols);
    }

    // 5. WSPR Decoder (runs if buffer >= 110s)
    if is_cancelled() {
        return;
    }
    let wspr_symbols = decode_wspr(&mono, sample_rate);
    if !wspr_symbols.is_empty() {
        on_batch(wspr_symbols);
    }
}

/// Scans the entire audio buffer across all supported decoders synchronously,
/// returning all detected symbols sorted by start timestamp.
#[must_use]
pub fn scan_digital_decode_all(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Vec<DecodedSymbol> {
    let mut all_symbols = Vec::new();
    scan_digital_decode_streaming(samples, channels, sample_rate, None, |batch| {
        all_symbols.extend(batch);
    });
    all_symbols.sort_by(|a, b| {
        a.start_sec
            .partial_cmp(&b.start_sec)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all_symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_stereo_averages_channels() {
        let stereo = vec![1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        let mono = downmix_to_mono(&stereo, 2);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.5).abs() < 1e-6);
        assert!((mono[1] - 0.5).abs() < 1e-6);
        assert!((mono[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn linear_resampling_preserves_constant() {
        let const_samples = vec![0.75; 100];
        let resampled = resample_linear(&const_samples, 44100, 8000);
        assert!(!resampled.is_empty());
        for &s in &resampled {
            assert!((s - 0.75).abs() < 1e-5);
        }
    }

    #[test]
    fn decodes_synthesized_dtmf_digit_5() {
        // Digit '5' = 770 Hz (row 1) + 1336 Hz (col 1)
        let rate = 8000.0f32;
        let num_samples = 800; // 100 ms of tone
        let mut audio = Vec::with_capacity(num_samples + 800);

        // Leading silence
        audio.extend(vec![0.0f32; 200]);

        for i in 0..num_samples {
            let t = i as f32 / rate;
            let s = 0.5 * (2.0 * std::f32::consts::PI * 770.0 * t).sin()
                + 0.5 * (2.0 * std::f32::consts::PI * 1336.0 * t).sin();
            audio.push(s);
        }

        // Trailing silence
        audio.extend(vec![0.0f32; 200]);

        let decoded = decode_dtmf(&audio, 8000);
        assert!(!decoded.is_empty(), "Expected DTMF detection for digit '5'");
        assert_eq!(decoded[0].symbol, "5");
        assert_eq!(decoded[0].protocol, DecoderProtocol::Dtmf);
        assert!(decoded[0].start_sec >= 0.01 && decoded[0].start_sec <= 0.08);
    }

    #[test]
    fn decodes_synthesized_morse_letter_e() {
        // Letter 'E' is a single dit followed by gap
        let rate = 8000.0f32;
        let dit_samples = 400; // 50 ms dit at 700 Hz
        let mut audio = Vec::new();

        // 50ms tone
        for i in 0..dit_samples {
            let t = i as f32 / rate;
            audio.push(0.8 * (2.0 * std::f32::consts::PI * 700.0 * t).sin());
        }
        // 200ms space (character/word gap)
        audio.extend(vec![0.0f32; 1600]);

        let decoded = decode_morse_cw(&audio, 8000);
        assert!(!decoded.is_empty(), "Expected CW detection for letter 'E'");
        assert_eq!(decoded[0].symbol, "E (.)");
        assert_eq!(decoded[0].protocol, DecoderProtocol::CwMorse);
    }

    #[test]
    fn decodes_synthesized_aprs_packet() {
        use yodel::SampleRate;
        use yodel::aprs::{AprsPacket, Status};
        use yodel::ax25::Address;
        use yodel::tnc::{TncConfig, TncTransmitter};

        let rate = 48000;
        let cfg = TncConfig::bell_202(SampleRate::new(rate).unwrap()).unwrap();
        let tx = TncTransmitter::new(cfg);

        let packet = AprsPacket::Status(Status {
            text: b"little-clip digital decode test",
        });
        let mut info_buf = [0u8; 64];
        let mut frame_buf = [0u8; 330];
        let samples = tx
            .transmit_i16(
                &packet,
                Address::new(b"APRS", 0).unwrap(),
                Address::new(b"N0CALL", 7).unwrap(),
                &[],
                &mut info_buf,
                &mut frame_buf,
            )
            .unwrap();

        let mut f32_samples: Vec<f32> = samples.map(|s| s as f32 / 32768.0).collect();
        let mut padded = vec![0.0f32; 4800];
        padded.append(&mut f32_samples);
        padded.extend(vec![0.0f32; 4800]);

        let decoded = decode_bell202_aprs(&padded, rate);
        assert!(!decoded.is_empty(), "Expected APRS frame detection");
        assert_eq!(decoded[0].protocol, DecoderProtocol::Bell202Aprs);
        assert!(decoded[0].symbol.contains("N0CALL-7"));
    }
}
