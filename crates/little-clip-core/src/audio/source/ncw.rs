//! Native NCW preparation, shared by analysis, studios and multisample audition.
use super::{AudioMetadata, File, pcm_wav};
use std::{
    io::{Seek, SeekFrom},
    os::unix::fs::FileExt,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) fn recognizes(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(..8),
        Some(b"\x01\xa8\x9e\xd6\x31\x01\x00\x00" | b"\x01\xa8\x9e\xd6\x30\x01\x00\x00")
    )
}
pub(super) fn metadata(file: &File) -> Result<AudioMetadata, String> {
    let header = header(file)?;
    Ok(AudioMetadata {
        duration_ms: u64::from(header.num_samples) * 1000 / u64::from(header.sample_rate),
        sample_rate: header.sample_rate,
        channels: header.channels,
        format: "NCW".into(),
    })
}
fn header(file: &File) -> Result<::ncw::NcwHeader, String> {
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length > 512 * 1024 * 1024 {
        return Err("NCW source exceeds the 512 MiB input budget".into());
    }
    let mut bytes = [0; 120];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|e| e.to_string())?;
    let header = ::ncw::NcwHeader::read(&mut &bytes[..]).map_err(|e| e.to_string())?;
    if !(1..=8).contains(&header.channels)
        || !(1..=384000).contains(&header.sample_rate)
        || !matches!(header.bits_per_sample, 8 | 16 | 24 | 32)
        || u64::from(header.data_offset) + u64::from(header.data_size) > length
    {
        return Err("Invalid or unsupported NCW audio header".into());
    }
    Ok(header)
}
pub(super) fn prepare(file: &File, token: &AtomicBool) -> Result<Vec<u8>, String> {
    cancelled(token)?;
    let expected = header(file)?;
    // The decoder exposes whole-file decoding. Bound its temporary allocation
    // before invoking it; retained playback remains the shared bounded WAV.
    if u64::from(expected.num_samples) * u64::from(expected.channels) > 16_000_000 {
        return Err("NCW decoded audio exceeds the 16 million sample budget".into());
    }
    let mut source = file.try_clone().map_err(|e| e.to_string())?;
    source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut decoder = ::ncw::NcwReader::read(source).map_err(|e| e.to_string())?;
    if decoder.header != expected {
        return Err("NCW changed while opening".into());
    }
    cancelled(token)?;
    let format = decoder.sample_format;
    let values = decoder
        .decode_samples()
        .map_err(|e| format!("NCW sample decode: {e}"))?;
    let pcm = float_pcm(values, format, expected.bits_per_sample, token)?;
    pcm_wav(pcm, expected.sample_rate, expected.channels)
}
fn float_pcm(
    values: Vec<i32>,
    format: ::ncw::SampleFormat,
    bits: u16,
    token: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let scale = 2.0f32.powi(i32::from(bits) - 1);
    let mut pcm = Vec::with_capacity(values.len() * 4 + 56);
    for (index, value) in values.into_iter().enumerate() {
        if index % 8192 == 0 {
            cancelled(token)?;
        }
        let sample = match format {
            ::ncw::SampleFormat::Pcm => value as f32 / scale,
            ::ncw::SampleFormat::Float => f32::from_bits(value as u32),
        };
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    cancelled(token)?;
    Ok(pcm)
}
fn cancelled(token: &AtomicBool) -> Result<(), String> {
    if token.load(Ordering::Relaxed) {
        Err("NCW audio preparation cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture(bits: u16, channels: u16) -> File {
        let scale = 1i32 << (bits - 2);
        let values: Vec<i32> = (0..4096 * channels as usize)
            .map(|i| if i % 2 == 0 { scale } else { -scale })
            .collect();
        let bytes = ::ncw::encode_pcm(
            &values,
            ::ncw::PcmSpec {
                channels,
                bits_per_sample: bits,
                sample_rate: 22050,
            },
            ::ncw::StereoMode::Direct,
        )
        .unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        file
    }
    #[test]
    fn native_ncw_metadata_and_pcm_preparation_cover_depths_and_channel_counts() {
        for bits in [16, 24] {
            for channels in [1, 2] {
                let file = fixture(bits, channels);
                let metadata = metadata(&file).unwrap();
                assert_eq!(metadata.format, "NCW");
                assert_eq!(metadata.channels, channels);
                assert_eq!(metadata.duration_ms, 4096 * 1000 / 22050);
                let wave = prepare(&file, &AtomicBool::new(false)).unwrap();
                assert_eq!(&wave[..4], b"RIFF");
                assert_eq!(wave.len(), 56 + 4096 * channels as usize * 4);
                assert!(
                    wave[56..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|b| (f32::from_le_bytes(*b).abs() - 0.5).abs() < 0.00001)
                );
                assert!(prepare(&file, &AtomicBool::new(true)).is_err());
            }
        }
    }
    #[test]
    fn malformed_and_excessively_large_decoded_headers_are_rejected() {
        let file = fixture(16, 1);
        file.write_all_at(&32_000_000u32.to_le_bytes(), 16).unwrap();
        assert!(
            prepare(&file, &AtomicBool::new(false))
                .unwrap_err()
                .contains("16 million")
        );
        file.write_all_at(&0u32.to_le_bytes(), 12).unwrap();
        assert!(metadata(&file).is_err());
    }
}
