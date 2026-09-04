use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use crate::config::{AudioConfig, AudioSamplePosition};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AudioWaveform {
    pub channels: u16,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub ch1_peaks: Vec<f32>,
    pub ch2_peaks: Option<Vec<f32>>,
    pub spectral_colors: Option<Vec<[u8; 3]>>,
    #[serde(default)]
    pub is_snippet: bool,
    #[serde(default)]
    pub snippet_start_ms: u64,
    #[serde(default)]
    pub snippet_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioMetadata {
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub format: String,
}

pub fn is_audio_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("mp3" | "wav" | "wave" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "alac"
        | "aiff" | "aif" | "opus" | "wma") => true,
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
    let track = format
        .default_track()
        .or_else(|| format.tracks().first())?;

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
        file.write_all(&1u16.to_le_bytes()).unwrap();  // AudioFormat (1 = PCM)
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
        let waveform = compute_plain_waveform(&wav_path, 60, &cfg)
            .expect("Failed to compute plain waveform");
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
    fn test_shapeforms_sample() {
        let cfg = AudioConfig::default();
        let path = std::path::Path::new("/home/vega/Downloads/Shapeforms CASSETTE/Shapeforms CASSETTE/AUDIO/EMF AND NOISE/EMF_TAPE_RECORDING_05.wav");
        if path.exists() {
            let meta = read_audio_metadata(path).expect("failed to read metadata of 24-bit wav");
            println!("EMF_TAPE_RECORDING_05: {} ms, {} Hz, {} channels", meta.duration_ms, meta.sample_rate, meta.channels);
            assert_eq!(meta.channels, 2);
            assert_eq!(meta.sample_rate, 96000);

            let wf = compute_plain_waveform(path, 100, &cfg).expect("failed to compute plain waveform of 24-bit wav");
            assert_eq!(wf.ch1_peaks.len(), 100);
            assert!(wf.ch2_peaks.is_some());

            let colors = compute_spectral_colors(path, 100, &cfg).expect("failed to compute spectral colors of 24-bit wav");
            assert_eq!(colors.len(), 100);
            println!("First 5 spectral colors of EMF recording: {:?}", &colors[..5]);
        }

        let noise_path = std::path::Path::new("/home/vega/Downloads/Shapeforms CASSETTE/Shapeforms CASSETTE/AUDIO/EMF AND NOISE/TAPE_NOISE_01.wav");
        if noise_path.exists() {
            let colors = compute_spectral_colors(noise_path, 100, &cfg).expect("failed on noise");
            println!("First 5 spectral colors of TAPE NOISE: {:?}", &colors[..5]);
        }
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

        let mp3_path = std::path::Path::new("/home/vega/Documents/type_testing/Computer Error Song.mp3");
        if mp3_path.is_file() {
            let full_wf = compute_full_waveform(mp3_path, 200).expect("failed to decode mp3 full waveform");
            assert!(!full_wf.is_snippet);
            assert!(full_wf.duration_ms > 350_000);
            assert_eq!(full_wf.ch1_peaks.len(), 200);
        }
    }
}
