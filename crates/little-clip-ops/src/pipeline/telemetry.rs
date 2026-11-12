//! Compact per-node execution telemetry (hold-spectrum, waveform peaks, image thumbnails, text previews).

use super::types::PortValue;
use little_clip_core::ipc::NodeTelemetry;
use realfft::RealFftPlanner;
use std::collections::HashMap;
use std::path::Path;

const SPECTRUM_BINS: usize = 32;
const WAVEFORM_BINS: usize = 32;
const FFT_SIZE: usize = 2048;

/// Collects compact telemetry from a node's inputs and outputs after execution.
pub(super) fn collect_node_telemetry(
    inputs: &HashMap<String, PortValue>,
    node_outputs: &[(String, PortValue)],
) -> Option<NodeTelemetry> {
    let mut telemetry = NodeTelemetry::default();

    // Input audio hold-spectrum (if an input is an audio WAV).
    if let Some(in_audio) = inputs.values().find_map(|v| match v {
        PortValue::AudioPath(p) => Some(p.as_path()),
        _ => None,
    }) && let Some((spec, _, _)) = analyze_wav(in_audio)
    {
        telemetry.before_spectrum_db = spec;
    }

    // Output telemetry (audio, image, or text).
    for (_, val) in node_outputs {
        match val {
            PortValue::AudioPath(p) => {
                if let Some((spec, peaks, peak_db)) = analyze_wav(p) {
                    telemetry.after_spectrum_db = spec;
                    telemetry.waveform_peaks = peaks;
                    telemetry.peak_db = Some(peak_db);
                    break;
                }
            }
            PortValue::ImagePath(p) => {
                if let Some(png) = build_image_thumbnail(p) {
                    telemetry.thumbnail_png = png;
                    break;
                }
            }
            PortValue::Text(s) => {
                let snippet: String = s.chars().take(120).collect();
                telemetry.text_preview = Some(snippet);
                break;
            }
            _ => {}
        }
    }

    if telemetry.before_spectrum_db.is_empty()
        && telemetry.after_spectrum_db.is_empty()
        && telemetry.waveform_peaks.is_empty()
        && telemetry.thumbnail_png.is_empty()
        && telemetry.text_preview.is_none()
    {
        None
    } else {
        Some(telemetry)
    }
}

fn analyze_wav(path: &Path) -> Option<(Vec<i8>, Vec<u8>, f32)> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut offset_cursor = 12usize;
    let mut audio_format = 1u16;
    let mut channels = 1usize;
    let mut sample_rate = 44100.0f32;
    let mut bits_per_sample = 16u16;
    let mut data_slice: Option<&[u8]> = None;

    while offset_cursor + 8 <= bytes.len() {
        let id = &bytes[offset_cursor..offset_cursor + 4];
        let size = u32::from_le_bytes(
            bytes[offset_cursor + 4..offset_cursor + 8]
                .try_into()
                .ok()?,
        ) as usize;
        let payload_start = offset_cursor + 8;
        let payload_end = payload_start.saturating_add(size).min(bytes.len());
        if id == b"fmt " && payload_end - payload_start >= 16 {
            audio_format = u16::from_le_bytes([bytes[payload_start], bytes[payload_start + 1]]);
            channels = u16::from_le_bytes([bytes[payload_start + 2], bytes[payload_start + 3]])
                .max(1) as usize;
            sample_rate = u32::from_le_bytes(
                bytes[payload_start + 4..payload_start + 8]
                    .try_into()
                    .ok()?,
            )
            .max(8000) as f32;
            bits_per_sample =
                u16::from_le_bytes([bytes[payload_start + 14], bytes[payload_start + 15]]);
        } else if id == b"data" {
            data_slice = Some(&bytes[payload_start..payload_end]);
            break;
        }
        offset_cursor = payload_start + size + (size % 2);
    }

    let data = data_slice?;
    let bytes_per_sample = (bits_per_sample / 8).max(1) as usize;
    let frame_stride = bytes_per_sample * channels;
    let max_frames = (sample_rate as usize) * 3;
    let total_frames = (data.len() / frame_stride).min(max_frames);
    let mut mono = Vec::with_capacity(total_frames);

    for f in 0..total_frames {
        let mut acc = 0.0_f32;
        for ch in 0..channels {
            let s_off = f * frame_stride + ch * bytes_per_sample;
            let sample = match (audio_format, bits_per_sample) {
                (1, 16) if s_off + 2 <= data.len() => {
                    i16::from_le_bytes([data[s_off], data[s_off + 1]]) as f32 / 32768.0
                }
                (1, 24) if s_off + 3 <= data.len() => {
                    let v = ((data[s_off] as i32)
                        | ((data[s_off + 1] as i32) << 8)
                        | ((data[s_off + 2] as i8 as i32) << 16))
                        as f32;
                    v / 8388608.0
                }
                (1, 32) if s_off + 4 <= data.len() => {
                    i32::from_le_bytes(data[s_off..s_off + 4].try_into().ok()?) as f32
                        / 2147483648.0
                }
                (3, 32) if s_off + 4 <= data.len() => {
                    f32::from_le_bytes(data[s_off..s_off + 4].try_into().ok()?)
                }
                _ => 0.0,
            };
            acc += sample;
        }
        mono.push(acc / channels as f32);
    }

    if mono.is_empty() {
        return None;
    }

    // Waveform peaks + peak dBFS
    let mut max_amp = 0.0_f32;
    for &s in &mono {
        max_amp = max_amp.max(s.abs());
    }
    let peak_db = if max_amp > 1e-6 {
        20.0 * max_amp.log10()
    } else {
        -96.0
    };

    let mut peaks = Vec::with_capacity(WAVEFORM_BINS);
    let chunk = (mono.len() / WAVEFORM_BINS).max(1);
    for bin in 0..WAVEFORM_BINS {
        let start = bin * chunk;
        if start >= mono.len() {
            peaks.push(0);
            continue;
        }
        let end = ((bin + 1) * chunk).min(mono.len());
        let pk = mono[start..end]
            .iter()
            .fold(0.0_f32, |acc, &v| acc.max(v.abs()));
        peaks.push((pk.clamp(0.0, 1.0) * 255.0).round() as u8);
    }

    // Log-spaced max-hold spectrum (20 Hz .. 20 kHz)
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(FFT_SIZE);
    let mut input_buf = vec![0.0_f32; FFT_SIZE];
    let mut spectrum_buf = r2c.make_output_vec();
    let hann: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
        })
        .collect();

    let mut hold_mag = vec![0.0_f32; FFT_SIZE / 2 + 1];
    let hop = FFT_SIZE / 2;
    let mut offset = 0;
    while offset < mono.len() {
        for i in 0..FFT_SIZE {
            let s = mono.get(offset + i).copied().unwrap_or(0.0);
            input_buf[i] = s * hann[i];
        }
        if r2c.process(&mut input_buf, &mut spectrum_buf).is_ok() {
            for (k, c) in spectrum_buf.iter().enumerate() {
                let mag = c.norm() / (FFT_SIZE as f32 * 0.5);
                if mag > hold_mag[k] {
                    hold_mag[k] = mag;
                }
            }
        }
        offset += hop;
    }

    let mut spec_db = Vec::with_capacity(SPECTRUM_BINS);
    for b in 0..SPECTRUM_BINS {
        let t0 = b as f32 / SPECTRUM_BINS as f32;
        let t1 = (b + 1) as f32 / SPECTRUM_BINS as f32;
        let f0 = 20.0 * (1000.0_f32).powf(t0);
        let f1 = 20.0 * (1000.0_f32).powf(t1);
        let k0 = ((f0 * FFT_SIZE as f32) / sample_rate)
            .floor()
            .clamp(1.0, (FFT_SIZE / 2) as f32) as usize;
        let k1 = ((f1 * FFT_SIZE as f32) / sample_rate)
            .ceil()
            .clamp(k0 as f32, (FFT_SIZE / 2) as f32) as usize;
        let peak_mag = hold_mag[k0..=k1].iter().fold(0.0_f32, |acc, &v| acc.max(v));
        let db = if peak_mag > 1e-5 {
            (20.0 * peak_mag.log10()).clamp(-72.0, 18.0)
        } else {
            -72.0
        };
        spec_db.push(db.round() as i8);
    }

    Some((spec_db, peaks, peak_db))
}

fn build_image_thumbnail(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 16 * 1024 * 1024 {
        return None;
    }
    let img = image::open(path).ok()?;
    let thumb = img.thumbnail(96, 64);
    let mut png_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png_bytes);
    thumb.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    Some(png_bytes)
}
