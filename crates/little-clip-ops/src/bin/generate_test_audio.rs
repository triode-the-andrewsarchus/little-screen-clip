//! Standalone utility to synthesize clean, noisy, and very noisy digital test audio files.
//!
//! Generates DTMF, Morse/CW, and Bell 202 / APRS WAV files into the target testing folder
//! for manual inspection and verification in the Little Screen Clip workbench.

use little_clip_ops::digital_decode::{
    char_to_morse, decode_bell202_aprs, decode_dtmf, decode_morse_cw,
};
use std::fs::{File, create_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use yodel::SampleRate;
use yodel::aprs::message::{Addressee, Message, MessageContent};
use yodel::aprs::{AprsPacket, Status};
use yodel::ax25::Address;
use yodel::tnc::{TncConfig, TncTransmitter};

/// Deterministic pseudo-random number generator for Gaussian white noise generation.
struct SimpleRng(u64);

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 32) as u32 as f32) / (u32::MAX as f32)
    }

    /// Generates standard normal Gaussian white noise using the Box-Muller transform.
    fn next_gaussian(&mut self) -> f32 {
        let u1 = self.next_f32().max(1e-7);
        let u2 = self.next_f32();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

/// Adds additive white Gaussian noise (AWGN) with the specified standard deviation.
fn add_noise(samples: &[f32], noise_std: f32, seed: u64) -> Vec<f32> {
    if noise_std <= 0.0 {
        return samples.to_vec();
    }
    let mut rng = SimpleRng::new(seed);
    samples
        .iter()
        .map(|&s| {
            let noise = rng.next_gaussian() * noise_std;
            (s + noise).clamp(-1.0, 1.0)
        })
        .collect()
}

/// Writes standard mono 16-bit PCM WAV audio file with RIFF headers.
fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    let i16_samples: Vec<i16> = samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect();

    let data_len = (i16_samples.len() * 2) as u32;
    let file_len = 36 + data_len;

    file.write_all(b"RIFF")?;
    file.write_all(&file_len.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?; // Subchunk1Size (16 for PCM)
    file.write_all(&1u16.to_le_bytes())?; // AudioFormat (1 = PCM)
    file.write_all(&1u16.to_le_bytes())?; // NumChannels (1 = Mono)
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * 2).to_le_bytes())?; // ByteRate
    file.write_all(&2u16.to_le_bytes())?; // BlockAlign (NumChannels * BitsPerSample / 8)
    file.write_all(&16u16.to_le_bytes())?; // BitsPerSample
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;

    for &sample in &i16_samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

/// Standard DTMF frequencies.
fn dtmf_frequencies(key: char) -> Option<(f32, f32)> {
    match key {
        '1' => Some((697.0, 1209.0)),
        '2' => Some((697.0, 1336.0)),
        '3' => Some((697.0, 1477.0)),
        'A' | 'a' => Some((697.0, 1633.0)),
        '4' => Some((770.0, 1209.0)),
        '5' => Some((770.0, 1336.0)),
        '6' => Some((770.0, 1477.0)),
        'B' | 'b' => Some((770.0, 1633.0)),
        '7' => Some((852.0, 1209.0)),
        '8' => Some((852.0, 1336.0)),
        '9' => Some((852.0, 1477.0)),
        'C' | 'c' => Some((852.0, 1633.0)),
        '*' => Some((941.0, 1209.0)),
        '0' => Some((941.0, 1336.0)),
        '#' => Some((941.0, 1477.0)),
        'D' | 'd' => Some((941.0, 1633.0)),
        _ => None,
    }
}

/// Synthesizes a DTMF sequence into audio samples.
fn generate_dtmf(keys: &str, sample_rate: u32, tone_dur_s: f32, silence_dur_s: f32) -> Vec<f32> {
    let mut samples = Vec::new();
    let tone_frames = (sample_rate as f32 * tone_dur_s) as usize;
    let silence_frames = (sample_rate as f32 * silence_dur_s) as usize;
    let rate = sample_rate as f32;

    // Initial 100ms padding
    samples.resize((sample_rate as f32 * 0.1) as usize, 0.0);

    for ch in keys.chars() {
        if let Some((f_low, f_high)) = dtmf_frequencies(ch) {
            for i in 0..tone_frames {
                let t = i as f32 / rate;
                // Raised cosine ramp at edges (5ms) to prevent spectral splatter clicks
                let ramp_frames = (rate * 0.005) as usize;
                let envelope = if i < ramp_frames {
                    i as f32 / ramp_frames as f32
                } else if i >= tone_frames - ramp_frames {
                    (tone_frames - i) as f32 / ramp_frames as f32
                } else {
                    1.0
                };

                let val = (0.5 * (2.0 * std::f32::consts::PI * f_low * t).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * f_high * t).sin())
                    * envelope
                    * 0.75;
                samples.push(val);
            }
            samples.resize(samples.len() + silence_frames, 0.0);
        }
    }

    // Trailing 100ms padding
    samples.resize(samples.len() + (sample_rate as f32 * 0.1) as usize, 0.0);
    samples
}

/// Synthesizes Morse / CW audio using standard PARIS timing.
fn generate_morse(text: &str, sample_rate: u32, wpm: f32, freq: f32) -> Vec<f32> {
    let dot_dur_s = 1.2 / wpm;
    let rate = sample_rate as f32;
    let mut samples = Vec::new();

    // Lead-in padding (150 ms)
    samples.resize((rate * 0.15) as usize, 0.0);

    let append_tone = |samples: &mut Vec<f32>, duration_s: f32| {
        let count = (rate * duration_s) as usize;
        let ramp = (rate * 0.005) as usize; // 5ms soft edge
        for i in 0..count {
            let t = i as f32 / rate;
            let env = if i < ramp {
                i as f32 / ramp as f32
            } else if i >= count - ramp {
                (count - i) as f32 / ramp as f32
            } else {
                1.0
            };
            samples.push((2.0 * std::f32::consts::PI * freq * t).sin() * env * 0.8);
        }
    };

    let append_silence = |samples: &mut Vec<f32>, duration_s: f32| {
        let count = (rate * duration_s) as usize;
        samples.resize(samples.len() + count, 0.0);
    };

    let words: Vec<&str> = text.split_whitespace().collect();
    for (w_idx, word) in words.iter().enumerate() {
        if w_idx > 0 {
            append_silence(&mut samples, dot_dur_s * 7.0); // Word space
        }
        for (c_idx, ch) in word.chars().enumerate() {
            if c_idx > 0 {
                append_silence(&mut samples, dot_dur_s * 3.0); // Char space
            }
            if let Some(pattern) = char_to_morse(ch) {
                for (elem_idx, elem) in pattern.chars().enumerate() {
                    if elem_idx > 0 {
                        append_silence(&mut samples, dot_dur_s); // Element space
                    }
                    match elem {
                        '.' => append_tone(&mut samples, dot_dur_s),
                        '-' => append_tone(&mut samples, dot_dur_s * 3.0),
                        _ => {}
                    }
                }
            }
        }
    }

    // Trailing padding (150 ms)
    append_silence(&mut samples, 0.15);
    samples
}

/// Synthesizes a standard Bell 202 AFSK APRS packet (Status beacon).
fn generate_aprs_status(text: &[u8], src_call: &[u8], ssid: u8, rate: u32) -> Vec<f32> {
    let cfg = TncConfig::bell_202(SampleRate::new(rate).unwrap()).unwrap();
    let tx = TncTransmitter::new(cfg);

    let packet = AprsPacket::Status(Status { text });
    let mut info_buf = [0u8; 128];
    let mut frame_buf = [0u8; 512];

    let samples = tx
        .transmit_i16(
            &packet,
            Address::new(b"APRS", 0).unwrap(),
            Address::new(src_call, ssid).unwrap(),
            &[],
            &mut info_buf,
            &mut frame_buf,
        )
        .unwrap();

    let mut f32_samples: Vec<f32> = samples.map(|s| s as f32 / 32768.0).collect();
    let mut padded = vec![0.0f32; (rate as f32 * 0.1) as usize];
    padded.append(&mut f32_samples);
    padded.resize(padded.len() + (rate as f32 * 0.1) as usize, 0.0);
    padded
}

/// Synthesizes a standard Bell 202 AFSK APRS packet (Direct Message).
fn generate_aprs_message(
    addressee: &[u8],
    body: &[u8],
    src_call: &[u8],
    ssid: u8,
    rate: u32,
) -> Vec<f32> {
    let cfg = TncConfig::bell_202(SampleRate::new(rate).unwrap()).unwrap();
    let tx = TncTransmitter::new(cfg);

    let packet = AprsPacket::Message(Message {
        addressee: Addressee::new(addressee).unwrap(),
        content: MessageContent::Text {
            text: body,
            id: None,
        },
    });
    let mut info_buf = [0u8; 128];
    let mut frame_buf = [0u8; 512];

    let samples = tx
        .transmit_i16(
            &packet,
            Address::new(b"APRS", 0).unwrap(),
            Address::new(src_call, ssid).unwrap(),
            &[],
            &mut info_buf,
            &mut frame_buf,
        )
        .unwrap();

    let mut f32_samples: Vec<f32> = samples.map(|s| s as f32 / 32768.0).collect();
    let mut padded = vec![0.0f32; (rate as f32 * 0.1) as usize];
    padded.append(&mut f32_samples);
    padded.resize(padded.len() + (rate as f32 * 0.1) as usize, 0.0);
    padded
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("decode_testing"));
    create_dir_all(&output_dir)?;
    println!(
        "Generating digital decode test suite in: {}",
        output_dir.display()
    );

    struct TestCase {
        name: &'static str,
        samples: Vec<f32>,
        sample_rate: u32,
    }

    let mut cases = Vec::new();

    // 1. DTMF Sequences
    cases.push(TestCase {
        name: "dtmf_8675309",
        samples: generate_dtmf("8675309", 8000, 0.16, 0.10),
        sample_rate: 8000,
    });
    cases.push(TestCase {
        name: "dtmf_full_keypad",
        samples: generate_dtmf("123A456B789C*0#D", 8000, 0.12, 0.08),
        sample_rate: 8000,
    });

    // 2. Morse / CW
    cases.push(TestCase {
        name: "morse_sos",
        samples: generate_morse("SOS", 8000, 18.0, 750.0),
        sample_rate: 8000,
    });
    cases.push(TestCase {
        name: "morse_cq_de_k1abc",
        samples: generate_morse("CQ CQ DE K1ABC", 8000, 20.0, 700.0),
        sample_rate: 8000,
    });

    // 3. Bell 202 / APRS
    cases.push(TestCase {
        name: "aprs_status_beacon",
        samples: generate_aprs_status(b"little-clip digital decode test", b"N0CALL", 7, 48000),
        sample_rate: 48000,
    });
    cases.push(TestCase {
        name: "aprs_emergency_msg",
        samples: generate_aprs_message(
            b"SAR-01",
            b"TEAM EN ROUTE TO CHECKPOINT 4",
            b"W1AW",
            9,
            48000,
        ),
        sample_rate: 48000,
    });

    // 4. Mixed Composite (DTMF + Morse)
    let mut mixed = generate_dtmf("911", 8000, 0.15, 0.10);
    mixed.extend(vec![0.0f32; 800]); // 100ms pause
    mixed.extend(generate_morse("TEST", 8000, 20.0, 800.0));
    cases.push(TestCase {
        name: "mixed_dtmf_morse",
        samples: mixed,
        sample_rate: 8000,
    });

    // Noise levels
    let variants = [
        ("clean", 0.00f32, "SNR > 50 dB (Pure)"),
        ("noisy", 0.10f32, "SNR ~15 dB (Moderate Static)"),
        ("very_noisy", 0.32f32, "SNR ~3 dB (Heavy Static)"),
    ];

    println!("{:=<80}", "");
    println!(
        "{:<28} | {:<12} | {:<20} | {:<12}",
        "Filename", "Variant", "Demodulator Result", "Status"
    );
    println!("{:-<80}", "");

    let mut file_count = 0;

    for case in &cases {
        for (variant_name, noise_std, _desc) in &variants {
            let filename = format!("{}_{}.wav", case.name, variant_name);
            let filepath = output_dir.join(&filename);

            let noisy_samples = add_noise(&case.samples, *noise_std, 42 + file_count as u64 * 997);
            write_wav(&filepath, &noisy_samples, case.sample_rate)?;
            file_count += 1;

            // Self-verify decode detection on each variant
            let decode_summary = if case.name.starts_with("dtmf") {
                let syms = decode_dtmf(&noisy_samples, case.sample_rate);
                let digits: String = syms.iter().map(|s| s.symbol.as_str()).collect();
                if digits.is_empty() {
                    "(no digits)".to_string()
                } else {
                    format!("DTMF: \"{}\"", digits)
                }
            } else if case.name.starts_with("morse") {
                let syms = decode_morse_cw(&noisy_samples, case.sample_rate);
                let text = syms
                    .iter()
                    .map(|s| s.symbol.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if text.is_empty() {
                    "(no morse)".to_string()
                } else {
                    format!("CW: \"{}\"", text)
                }
            } else if case.name.starts_with("aprs") {
                let syms = decode_bell202_aprs(&noisy_samples, case.sample_rate);
                if syms.is_empty() {
                    "(no frame)".to_string()
                } else {
                    syms[0].symbol.clone()
                }
            } else {
                let dtmf_syms = decode_dtmf(&noisy_samples, case.sample_rate);
                let morse_syms = decode_morse_cw(&noisy_samples, case.sample_rate);
                format!("DTMF: {}, CW: {}", dtmf_syms.len(), morse_syms.len())
            };

            let status = if !decode_summary.contains("(no") {
                "✔ DECODED"
            } else if *variant_name == "very_noisy" {
                "⚠ NOISE FLOOR"
            } else {
                "✖ MISSED"
            };

            println!(
                "{:<28} | {:<12} | {:<20} | {:<12}",
                filename, variant_name, decode_summary, status
            );
        }
    }

    println!("{:=<80}", "");
    println!(
        "Successfully wrote {} WAV test files to {}",
        file_count,
        output_dir.display()
    );

    Ok(())
}
