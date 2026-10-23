//! Audio format header inspection for constellation hover tooltips and indexer.
//! Extracts bit depth, sample rate, and channel count directly from container headers.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioFormatInfo {
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bit_depth: Option<u16>,
}

/// Reads bit depth, sample rate, and channels from an audio file.
/// Inspects container headers with a bounded chunk search, then falls back to core
/// audio metadata. Must run on a worker because ordinary files may live on slow mounts.
pub fn read_audio_format_info(path: &Path) -> AudioFormatInfo {
    if let Ok(mut f) = little_clip_core::kernel_interface::open_content(path, true) {
        if let Some(info) = read_wav_info(&mut f) {
            return info;
        }
        if let Some(info) = read_flac_info(&mut f) {
            return info;
        }
        if let Some(info) = read_aiff_info(&mut f) {
            return info;
        }
    }

    if let Some(meta) = little_clip_core::audio::read_audio_metadata(path) {
        return AudioFormatInfo {
            sample_rate: (meta.sample_rate > 0).then_some(meta.sample_rate),
            channels: (meta.channels > 0).then_some(meta.channels),
            bit_depth: None,
        };
    }

    AudioFormatInfo::default()
}

/// Parses RIFF/WAVE header and fmt chunk.
pub fn read_wav_info(file: &mut (impl Read + Seek)) -> Option<AudioFormatInfo> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return None;
    }

    let fmt_data = read_chunk(file, b"fmt ", false)?;
    if fmt_data.len() < 16 {
        return None;
    }
    let format_tag = u16::from_le_bytes([fmt_data[0], fmt_data[1]]);
    let channels = u16::from_le_bytes([fmt_data[2], fmt_data[3]]);
    let sample_rate = u32::from_le_bytes(fmt_data[4..8].try_into().ok()?);
    let mut bit_depth = u16::from_le_bytes([fmt_data[14], fmt_data[15]]);
    if format_tag == 0xFFFE && fmt_data.len() >= 20 {
        let valid_bits = u16::from_le_bytes([fmt_data[18], fmt_data[19]]);
        if valid_bits > 0 {
            bit_depth = valid_bits;
        }
    }
    Some(AudioFormatInfo {
        sample_rate: (sample_rate > 0).then_some(sample_rate),
        channels: (channels > 0).then_some(channels),
        bit_depth: (bit_depth > 0).then_some(bit_depth),
    })
}

/// Finds a bounded chunk after a RIFF/FORM header, respecting odd-length padding.
fn read_chunk(
    file: &mut (impl Read + Seek),
    wanted: &[u8; 4],
    big_endian: bool,
) -> Option<Vec<u8>> {
    let mut scanned = 12u64;
    while scanned < 65536 {
        let mut header = [0; 8];
        file.read_exact(&mut header).ok()?;
        let bytes = header[4..8].try_into().ok()?;
        let length = if big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        } as u64;
        if &header[..4] == wanted {
            let mut data = vec![0; length.min(64) as usize];
            file.read_exact(&mut data).ok()?;
            return Some(data);
        }
        let padded = length + length % 2;
        scanned += 8 + padded;
        file.seek(SeekFrom::Current(padded as i64)).ok()?;
    }
    None
}

/// Parses FLAC header and STREAMINFO block.
pub fn read_flac_info(file: &mut (impl Read + Seek)) -> Option<AudioFormatInfo> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; 4];
    file.read_exact(&mut header).ok()?;
    if &header != b"fLaC" {
        return None;
    }

    let mut block_hdr = [0u8; 4];
    file.read_exact(&mut block_hdr).ok()?;
    let block_type = block_hdr[0] & 0x7F;
    let block_len = u32::from_be_bytes([0, block_hdr[1], block_hdr[2], block_hdr[3]]);
    if block_type != 0 || block_len != 34 {
        return None;
    }

    let mut data = [0u8; 34];
    file.read_exact(&mut data).ok()?;
    let sample_rate =
        ((data[10] as u32) << 12) | ((data[11] as u32) << 4) | ((data[12] >> 4) as u32);
    let channels = (((data[12] >> 1) & 0x07) + 1) as u16;
    let bit_depth = ((((data[12] & 0x01) as u16) << 4) | ((data[13] >> 4) as u16)) + 1;

    Some(AudioFormatInfo {
        sample_rate: (sample_rate > 0).then_some(sample_rate),
        channels: (channels > 0).then_some(channels),
        bit_depth: (bit_depth > 0).then_some(bit_depth),
    })
}

/// Parses AIFF header and COMM chunk.
pub fn read_aiff_info(file: &mut (impl Read + Seek)) -> Option<AudioFormatInfo> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    if &header[0..4] != b"FORM" || (&header[8..12] != b"AIFF" && &header[8..12] != b"AIFC") {
        return None;
    }

    let comm_data = read_chunk(file, b"COMM", true)?;
    if comm_data.len() < 18 {
        return None;
    }
    let channels = u16::from_be_bytes([comm_data[0], comm_data[1]]);
    let bit_depth = u16::from_be_bytes([comm_data[6], comm_data[7]]);
    let rate = parse_extended_float(&comm_data[8..18])?;
    let sample_rate = (1.0..=u32::MAX as f64)
        .contains(&rate)
        .then_some(rate as u32)?;
    Some(AudioFormatInfo {
        sample_rate: Some(sample_rate),
        channels: (channels > 0).then_some(channels),
        bit_depth: (bit_depth > 0).then_some(bit_depth),
    })
}

/// Converts 80-bit IEEE 754 extended precision float to f64.
fn parse_extended_float(bytes: &[u8]) -> Option<f64> {
    if bytes.len() < 10 {
        return None;
    }
    let expon = ((bytes[0] as u16 & 0x7F) << 8) | (bytes[1] as u16);
    let mut mantissa: u64 = 0;
    for &b in &bytes[2..10] {
        mantissa = (mantissa << 8) | (b as u64);
    }
    if expon == 0 && mantissa == 0 {
        return Some(0.0);
    }
    let sign = if (bytes[0] & 0x80) != 0 { -1.0 } else { 1.0 };
    let shift = expon as i32 - 16383 - 63;
    let value = (mantissa as f64) * 2.0f64.powi(shift);
    Some(sign * value)
}

/// Formats a sample rate in Hz to a friendly string (e.g. "44.1 kHz", "48 kHz", "96 kHz").
pub fn format_sample_rate(sample_rate: u32) -> String {
    if sample_rate.is_multiple_of(1000) {
        format!("{} kHz", sample_rate / 1000)
    } else {
        format!("{:.1} kHz", sample_rate as f32 / 1000.0)
    }
}

/// Formats channel count to a friendly string (e.g. "Mono (1 ch)", "Stereo (2 ch)").
pub fn format_channels(channels: u16) -> String {
    match channels {
        1 => "Mono (1 ch)".to_string(),
        2 => "Stereo (2 ch)".to_string(),
        n => format!("{n} ch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn malformed_flac_block_length_is_rejected() {
        let mut data = b"fLaC".to_vec();
        data.extend_from_slice(&[0, 0, 0, 1]);
        data.extend_from_slice(&[0; 34]);
        assert!(read_flac_info(&mut Cursor::new(data)).is_none());
    }

    #[test]
    fn protected_inputs_are_not_probed() {
        assert_eq!(
            read_audio_format_info(Path::new("/dev/zero")),
            AudioFormatInfo::default()
        );
    }

    #[test]
    fn test_format_sample_rate() {
        assert_eq!(format_sample_rate(44100), "44.1 kHz");
        assert_eq!(format_sample_rate(48000), "48 kHz");
        assert_eq!(format_sample_rate(96000), "96 kHz");
        assert_eq!(format_sample_rate(88200), "88.2 kHz");
    }

    #[test]
    fn test_format_channels() {
        assert_eq!(format_channels(1), "Mono (1 ch)");
        assert_eq!(format_channels(2), "Stereo (2 ch)");
        assert_eq!(format_channels(6), "6 ch");
    }

    #[test]
    fn test_wav_header_parsing() {
        // Construct a minimal valid 16-bit 44.1kHz stereo WAV header
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&36u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes()); // Chunk size
        buf.extend_from_slice(&1u16.to_le_bytes()); // Format tag (PCM)
        buf.extend_from_slice(&2u16.to_le_bytes()); // Channels
        buf.extend_from_slice(&44100u32.to_le_bytes()); // Sample rate
        buf.extend_from_slice(&(44100u32 * 4).to_le_bytes()); // Byte rate
        buf.extend_from_slice(&4u16.to_le_bytes()); // Block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample

        let mut cursor = Cursor::new(buf);
        let info = read_wav_info(&mut cursor).expect("Failed to read WAV header");
        assert_eq!(info.sample_rate, Some(44100));
        assert_eq!(info.channels, Some(2));
        assert_eq!(info.bit_depth, Some(16));
    }

    #[test]
    fn test_wav_24bit_header_parsing() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&36u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // Mono
        buf.extend_from_slice(&48000u32.to_le_bytes()); // 48kHz
        buf.extend_from_slice(&(48000u32 * 3).to_le_bytes());
        buf.extend_from_slice(&3u16.to_le_bytes());
        buf.extend_from_slice(&24u16.to_le_bytes()); // 24-bit

        let mut cursor = Cursor::new(buf);
        let info = read_wav_info(&mut cursor).expect("Failed to read 24-bit WAV header");
        assert_eq!(info.sample_rate, Some(48000));
        assert_eq!(info.channels, Some(1));
        assert_eq!(info.bit_depth, Some(24));
    }
}
