//! Frozen waveform data and its identity come from the same guarded file handle.
use super::*;
use little_clip_core::file_source::ReadCheck;
use std::{fs::File, io::BufReader};

// Bound retained mono PCM and the encoded input independently. At 48 kHz this
// allows over twenty minutes, well beyond the default thirty-second capture.
const MAX_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

pub(super) struct RollingSnapshot {
    pub path: PathBuf,
    pub source: FileSource,
    pub duration_sec: f32,
    pub sample_rate: u32,
    pub channels: u16,
    pub mono_samples: Vec<f32>,
    pub peaks_lvl0: Vec<PeakBlock>,
    pub peaks_lvl1: Vec<PeakBlock>,
    pub peaks_lvl2: Vec<PeakBlock>,
}

pub(super) fn read_snapshot(path: &Path, cancel: &AtomicBool) -> Result<RollingSnapshot, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Rolling snapshot cancelled".into());
    }
    let (file, source) = ReadCheck::default().open(path, None, cancel)?;
    let held = file.try_clone().map_err(|error| error.to_string())?;
    let mut reader = pcm16_reader(file)?;
    let spec = reader.spec();
    let frames = reader.duration() as usize;
    let mut mono_samples = Vec::new();
    mono_samples
        .try_reserve_exact(frames)
        .map_err(|error| error.to_string())?;
    let mut sum = 0.0;
    for (index, sample) in reader.samples::<i16>().enumerate() {
        if index.is_multiple_of(8192) && cancel.load(Ordering::Acquire) {
            return Err("Rolling snapshot cancelled".into());
        }
        sum += sample.map_err(|error| error.to_string())? as f32 / 32768.0;
        if (index + 1).is_multiple_of(spec.channels as usize) {
            mono_samples.push(sum / spec.channels as f32);
            sum = 0.0;
        }
    }
    if FileSource::capture_file(&held)? != source {
        return Err("Rolling snapshot changed while decoding; refresh it.".into());
    }
    source.check()?;
    if cancel.load(Ordering::Acquire) {
        return Err("Rolling snapshot cancelled".into());
    }
    let (peaks_lvl0, peaks_lvl1, peaks_lvl2) =
        build_peak_pyramid(&mono_samples, spec.sample_rate as f32);
    if cancel.load(Ordering::Acquire) {
        return Err("Rolling snapshot cancelled".into());
    }
    Ok(RollingSnapshot {
        path: path.to_owned(),
        source,
        duration_sec: mono_samples.len() as f32 / spec.sample_rate as f32,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        mono_samples,
        peaks_lvl0,
        peaks_lvl1,
        peaks_lvl2,
    })
}

// Admit the encoded size, header layout and declared PCM budget before the
// caller reserves waveform memory or starts sample iteration.
fn pcm16_reader(file: File) -> Result<hound::WavReader<BufReader<File>>, String> {
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length > MAX_SNAPSHOT_BYTES {
        return Err("Rolling snapshot exceeds the 256 MiB input limit".into());
    }
    let reader = hound::WavReader::new(BufReader::new(file)).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int
        || spec.bits_per_sample != 16
        || spec.channels == 0
        || spec.sample_rate == 0
    {
        return Err(
            "Rolling snapshot must be 16-bit PCM with valid channels and sample rate".into(),
        );
    }
    if u64::from(reader.len()) * 2 > length {
        return Err("Rolling snapshot has a truncated data chunk".into());
    }
    if u64::from(reader.duration()) * 4 > MAX_SNAPSHOT_BYTES {
        return Err("Rolling snapshot exceeds the 256 MiB waveform limit".into());
    }
    Ok(reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(1, 8000, 2);
        buffer.push_samples(&[8192, -4096, 16384, -8192]);
        buffer.write_wav(&path).unwrap();
        (directory, path)
    }

    #[test]
    fn snapshot_duration_samples_and_identity_describe_the_decoded_file() {
        let (_directory, path) = fixture();
        let snapshot = read_snapshot(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(snapshot.source, FileSource::capture(&path).unwrap());
        assert_eq!(snapshot.mono_samples, vec![0.0625, 0.125]);
        assert_eq!(snapshot.channels, 2);
        assert_eq!(snapshot.sample_rate, 8000);
        assert_eq!(snapshot.duration_sec, 2.0 / 8000.0);
        assert!(!snapshot.peaks_lvl0.is_empty());
    }

    #[test]
    fn malformed_headers_and_truncated_data_fail_without_allocating_claimed_chunks() {
        let (_directory, path) = fixture();
        let original = std::fs::read(&path).unwrap();
        for (offset, replacement) in [
            (16, u32::MAX.to_le_bytes().to_vec()),
            (22, 0_u16.to_le_bytes().to_vec()),
            (24, 0_u32.to_le_bytes().to_vec()),
            (34, 0_u16.to_le_bytes().to_vec()),
            (40, 1_000_000_u32.to_le_bytes().to_vec()),
        ] {
            let mut bytes = original.clone();
            bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                read_snapshot(&path, &AtomicBool::new(false)).is_err(),
                "invalid field at {offset}"
            );
        }
    }

    #[test]
    fn protected_sources_oversized_files_and_cancelled_reads_are_rejected() {
        let (_directory, path) = fixture();
        assert!(read_snapshot(Path::new("/dev/null"), &AtomicBool::new(false)).is_err());
        let missing = path.with_extension("missing");
        assert!(
            read_snapshot(&missing, &AtomicBool::new(true))
                .err()
                .unwrap()
                .contains("cancelled")
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_SNAPSHOT_BYTES + 1)
            .unwrap();
        assert!(
            read_snapshot(&path, &AtomicBool::new(false))
                .err()
                .unwrap()
                .contains("input limit")
        );
    }

    #[test]
    fn declared_mono_waveform_budget_is_checked_before_reserving_pcm() {
        let (_directory, path) = fixture();
        let mut header = std::fs::read(&path).unwrap();
        header.truncate(44);
        let data_size = MAX_SNAPSHOT_BYTES as u32 / 2 + 2;
        header[4..8].copy_from_slice(&(36 + data_size).to_le_bytes());
        header[22..24].copy_from_slice(&1_u16.to_le_bytes());
        header[28..32].copy_from_slice(&16_000_u32.to_le_bytes());
        header[32..34].copy_from_slice(&2_u16.to_le_bytes());
        header[40..44].copy_from_slice(&data_size.to_le_bytes());
        std::fs::write(&path, header).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(44 + u64::from(data_size))
            .unwrap();
        assert!(
            read_snapshot(&path, &AtomicBool::new(false))
                .err()
                .unwrap()
                .contains("waveform limit")
        );
    }
}
