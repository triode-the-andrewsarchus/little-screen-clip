//! Circular ring-buffer audio capture and WAV extraction/slicing utilities.
//!
//! Stores continuous rolling PCM audio in a fixed-capacity ring buffer and supports
//! non-destructive slicing of sub-regions from standard 16-bit PCM WAV containers.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::Path;

/// A thread-safe, fixed-capacity circular buffer storing interleaved 16-bit PCM samples
/// for continuous rolling audio capture.
#[derive(Debug, Clone)]
pub struct RollingAudioBuffer {
    sample_rate: u32,
    channels: u16,
    capacity_samples: usize,
    buffer: Vec<i16>,
    write_pos: usize,
    is_full: bool,
}

impl RollingAudioBuffer {
    /// Creates a new rolling buffer for the given duration in seconds, sample rate, and channel count.
    pub fn new(duration_sec: u64, sample_rate: u32, channels: u16) -> Self {
        let dur = duration_sec.max(1);
        let ch = channels.max(1) as usize;
        let mut capacity = (dur as usize) * (sample_rate as usize) * ch;
        // Ensure capacity is aligned to channel frames
        capacity -= capacity % ch;

        Self {
            sample_rate,
            channels,
            capacity_samples: capacity,
            buffer: vec![0i16; capacity],
            write_pos: 0,
            is_full: false,
        }
    }

    /// Clears buffered audio and applies new capture dimensions.
    ///
    /// Callers should hold their synchronization guard while reconfiguring so capture
    /// cannot append samples using the previous channel layout.
    pub fn reconfigure(&mut self, duration_sec: u64, sample_rate: u32, channels: u16) {
        *self = Self::new(duration_sec, sample_rate, channels);
    }

    #[inline]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[inline]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    #[inline]
    pub fn capacity_samples(&self) -> usize {
        self.capacity_samples
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        !self.is_full && self.write_pos == 0
    }

    #[inline]
    pub fn len_samples(&self) -> usize {
        if self.is_full {
            self.capacity_samples
        } else {
            self.write_pos
        }
    }

    #[inline]
    pub fn duration_secs(&self) -> f32 {
        let samples = self.len_samples();
        let frame_rate = self.sample_rate as f32 * self.channels as f32;
        if frame_rate > 0.0 {
            samples as f32 / frame_rate
        } else {
            0.0
        }
    }

    /// Appends raw little-endian signed 16-bit PCM bytes (e.g. from stdout of pw-record / parec).
    pub fn push_pcm_s16le(&mut self, bytes: &[u8]) {
        let ch = self.channels.max(1) as usize;
        let bytes_per_frame = 2 * ch;
        let num_frames = bytes.len() / bytes_per_frame;
        if num_frames == 0 || self.capacity_samples == 0 {
            return;
        }

        let num_samples = num_frames * ch;
        for i in 0..num_samples {
            let sample = i16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]);
            self.buffer[self.write_pos] = sample;
            self.write_pos += 1;
            if self.write_pos >= self.capacity_samples {
                self.write_pos = 0;
                self.is_full = true;
            }
        }
    }

    /// Appends a slice of `i16` interleaved samples.
    pub fn push_samples(&mut self, samples: &[i16]) {
        let ch = self.channels.max(1) as usize;
        let num_frames = samples.len() / ch;
        if num_frames == 0 || self.capacity_samples == 0 {
            return;
        }

        let total_samples = num_frames * ch;
        for &sample in &samples[..total_samples] {
            self.buffer[self.write_pos] = sample;
            self.write_pos += 1;
            if self.write_pos >= self.capacity_samples {
                self.write_pos = 0;
                self.is_full = true;
            }
        }
    }

    /// Returns a linearized copy of the circular buffer ordered chronologically (oldest to newest).
    pub fn snapshot_samples(&self) -> Vec<i16> {
        if self.is_empty() {
            return Vec::new();
        }

        if !self.is_full {
            self.buffer[..self.write_pos].to_vec()
        } else {
            let mut result = Vec::with_capacity(self.capacity_samples);
            result.extend_from_slice(&self.buffer[self.write_pos..]);
            result.extend_from_slice(&self.buffer[..self.write_pos]);
            result
        }
    }

    /// Returns a linearized mono f32 representation (normalized to [-1.0, 1.0])
    /// ordered chronologically, averaging all channels.
    pub fn snapshot_mono_f32(&self) -> Vec<f32> {
        let raw = self.snapshot_samples();
        if raw.is_empty() {
            return Vec::new();
        }

        let ch = self.channels as usize;
        if ch <= 1 {
            raw.into_iter().map(|s| s as f32 / 32768.0).collect()
        } else {
            let num_frames = raw.len() / ch;
            let mut mono = Vec::with_capacity(num_frames);
            let inv_ch = 1.0 / (ch as f32 * 32768.0);
            for frame in raw.chunks_exact(ch) {
                let sum: f32 = frame.iter().map(|&s| s as f32).sum();
                mono.push(sum * inv_ch);
            }
            mono
        }
    }

    /// Writes the current snapshot to a standard 16-bit PCM WAV file at `path`.
    /// Returns the duration of the audio in seconds on success.
    pub fn write_wav(&self, path: &Path) -> Result<f32, std::io::Error> {
        let samples = self.snapshot_samples();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        let channels = self.channels;
        let sample_rate = self.sample_rate;
        let bits_per_sample: u16 = 16;
        let block_align = channels * (bits_per_sample / 8);
        let byte_rate = sample_rate * block_align as u32;
        let data_size = (samples.len() * 2) as u32;
        PcmFormat {
            channels,
            sample_rate,
            byte_rate,
            block_align,
            bits_per_sample,
        }
        .write_header(&mut writer, data_size)?;

        for sample in samples {
            writer.write_all(&sample.to_le_bytes())?;
        }

        writer.flush()?;

        let total_frames = if channels > 0 {
            (data_size / (block_align as u32)) as f32
        } else {
            0.0
        };
        Ok(total_frames / sample_rate as f32)
    }
}

struct PcmFormat {
    channels: u16,
    sample_rate: u32,
    byte_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
}

impl PcmFormat {
    fn write_header(&self, writer: &mut impl Write, data_size: u32) -> io::Result<()> {
        let riff_size = data_size.checked_add(36 + (data_size & 1)).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "WAV exceeds RIFF size limit")
        })?;
        writer.write_all(b"RIFF")?;
        writer.write_all(&riff_size.to_le_bytes())?;
        writer.write_all(b"WAVEfmt ")?;
        writer.write_all(&16u32.to_le_bytes())?;
        writer.write_all(&1u16.to_le_bytes())?;
        writer.write_all(&self.channels.to_le_bytes())?;
        writer.write_all(&self.sample_rate.to_le_bytes())?;
        writer.write_all(&self.byte_rate.to_le_bytes())?;
        writer.write_all(&self.block_align.to_le_bytes())?;
        writer.write_all(&self.bits_per_sample.to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&data_size.to_le_bytes())
    }

    fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0u8; 16];
        reader.read_exact(&mut bytes)?;
        let tag = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
        let format = Self {
            channels: u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
            sample_rate: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            byte_rate: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            block_align: u16::from_le_bytes(bytes[12..14].try_into().unwrap()),
            bits_per_sample: u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
        };
        if tag != 1
            || format.channels == 0
            || format.sample_rate == 0
            || !matches!(format.bits_per_sample, 8 | 16 | 24 | 32)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid or unsupported PCM WAV format",
            ));
        }
        if format.channels.checked_mul(format.bits_per_sample / 8) != Some(format.block_align)
            || format
                .sample_rate
                .checked_mul(u32::from(format.block_align))
                != Some(format.byte_rate)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Inconsistent PCM WAV frame layout",
            ));
        }
        Ok(format)
    }
}

fn read_wave_container_end(file: &mut BufReader<File>) -> io::Result<u64> {
    let mut header = [0u8; 12];
    file.read_exact(&mut header)?;
    let end = u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap())) + 8;
    if &header[..4] != b"RIFF"
        || &header[8..] != b"WAVE"
        || end < 12
        || end > file.get_ref().metadata()?.len()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid or truncated RIFF/WAVE container",
        ));
    }
    Ok(end)
}

fn read_pcm_header(file: &mut BufReader<File>) -> io::Result<(PcmFormat, u64, u32)> {
    let end = read_wave_container_end(file)?;
    let mut format = None;
    // Bound work even for a multi-gigabyte container full of empty metadata chunks.
    for _ in 0..4096 {
        if file.stream_position()? + 8 > end {
            break;
        }
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let offset = file.stream_position()?;
        let next = offset + u64::from(size) + u64::from(size & 1);
        if next > end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "WAV chunk exceeds its RIFF container",
            ));
        }
        match &chunk[..4] {
            b"fmt " => {
                if size < 16 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Short WAV format chunk",
                    ));
                }
                format = Some(PcmFormat::read(file)?);
            }
            b"data" => {
                let format = format.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Missing PCM format before WAV data",
                    )
                })?;
                if size % u32::from(format.block_align) != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Incomplete PCM frame in WAV data",
                    ));
                }
                return Ok((format, offset, size));
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(next))?;
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "Missing WAV data or excessive chunk count",
    ))
}

/// Slices a finite time range from an 8/16/24/32-bit integer PCM WAV file.
///
/// Reversed bounds are ordered and both endpoints are clamped to complete frames.
/// Completed output is installed atomically at an unused destination; existing
/// files and symlinks are preserved.
///
/// # Errors
/// Returns an error for malformed/truncated containers, unsupported formats,
/// non-finite endpoints, changed/protected sources, existing destinations,
/// protected output directories, or filesystem failures.
pub fn slice_wav(
    source: &Path,
    start_sec: f32,
    end_sec: f32,
    out_path: &Path,
) -> Result<(), std::io::Error> {
    if !start_sec.is_finite() || !end_sec.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "WAV slice endpoints must be finite",
        ));
    }
    let mut file = BufReader::new(crate::kernel_interface::open_content(source, true)?);
    let observed =
        crate::file_source::FileSource::capture_file(file.get_ref()).map_err(io::Error::other)?;
    let (format, data_offset, total_data_bytes) = read_pcm_header(&mut file)?;
    let bytes_per_frame = u64::from(format.block_align);
    let total_frames = u64::from(total_data_bytes) / bytes_per_frame;

    let (s, e) = if start_sec <= end_sec {
        (start_sec, end_sec)
    } else {
        (end_sec, start_sec)
    };

    let frame_start =
        ((f64::from(s.max(0.0)) * f64::from(format.sample_rate)).floor() as u64).min(total_frames);
    let frame_end =
        ((f64::from(e.max(0.0)) * f64::from(format.sample_rate)).floor() as u64).min(total_frames);
    let slice_byte_offset = data_offset + frame_start * bytes_per_frame;
    let slice_byte_len = u32::try_from((frame_end - frame_start) * bytes_per_frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let padding = slice_byte_len & 1;
    file.seek(SeekFrom::Start(slice_byte_offset))?;

    let name = out_path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Missing WAV output filename")
    })?;
    let directory = crate::kernel_interface::create_content_directory(
        out_path.parent().unwrap_or(Path::new(".")),
    )?;
    let parent = std::path::PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    let mut writer = BufWriter::new(temporary.as_file_mut());

    format.write_header(&mut writer, slice_byte_len)?;

    let copied = io::copy(
        &mut (&mut file).take(u64::from(slice_byte_len)),
        &mut writer,
    )?;
    if copied != u64::from(slice_byte_len) {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "WAV data changed while slicing",
        ));
    }
    if padding != 0 {
        writer.write_all(&[0])?;
    }

    writer.flush()?;
    drop(writer);
    temporary.as_file().sync_all()?;
    if crate::file_source::FileSource::capture_file(file.get_ref()).map_err(io::Error::other)?
        != observed
    {
        return Err(io::Error::other("WAV source changed while slicing"));
    }
    crate::kernel_interface::pin_content_directory(&parent)?;
    temporary.persist_noclobber(parent.join(name))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_fixture() -> (tempfile::TempDir, std::path::PathBuf, Vec<u8>) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("input.wav");
        let mut buffer = RollingAudioBuffer::new(1, 8, 1);
        buffer.push_samples(&[0, 1, 2, 3, 4, 5, 6, 7]);
        buffer.write_wav(&source).unwrap();
        let bytes = std::fs::read(&source).unwrap();
        (directory, source, bytes)
    }

    #[test]
    fn wav_slice_handles_odd_chunks_and_extended_format_padding() {
        for extended_format in [false, true] {
            let (directory, source, original) = slice_fixture();
            let mut bytes = original.clone();
            if extended_format {
                bytes[16..20].copy_from_slice(&17u32.to_le_bytes());
                bytes.splice(36..36, [0x42, 0]);
            } else {
                let mut metadata = b"JUNK".to_vec();
                metadata.extend_from_slice(&3u32.to_le_bytes());
                metadata.extend_from_slice(&[1, 2, 3, 0]);
                bytes.splice(12..12, metadata);
            }
            let length = (bytes.len() - 8) as u32;
            bytes[4..8].copy_from_slice(&length.to_le_bytes());
            std::fs::write(&source, bytes).unwrap();
            let output = directory.path().join("slice.wav");
            slice_wav(&source, 0.25, 0.75, &output).unwrap();
            assert_eq!(&std::fs::read(output).unwrap()[44..], &original[48..56]);
        }
    }

    #[test]
    fn wav_slice_preserves_pcm_widths_and_clamps_reversed_ranges() {
        for bits in [8u16, 16, 24, 32] {
            let (directory, source, mut bytes) = slice_fixture();
            let stride = bits / 8;
            let payload: Vec<u8> = (0..8 * stride as u8).collect();
            bytes.truncate(44);
            bytes[4..8].copy_from_slice(&(36 + payload.len() as u32).to_le_bytes());
            bytes[28..32].copy_from_slice(&(8 * u32::from(stride)).to_le_bytes());
            bytes[32..34].copy_from_slice(&stride.to_le_bytes());
            bytes[34..36].copy_from_slice(&bits.to_le_bytes());
            bytes[40..44].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&payload);
            std::fs::write(&source, &bytes).unwrap();
            let output = directory.path().join("slice.wav");
            slice_wav(&source, 0.5, 0.125, &output).unwrap();
            let sliced = std::fs::read(output).unwrap();
            let length = 3 * usize::from(stride);
            assert_eq!(
                &sliced[44..44 + length],
                &payload[usize::from(stride)..4 * usize::from(stride)]
            );
            assert_eq!(sliced.len(), 44 + length + (length & 1));
            assert_eq!(
                u32::from_le_bytes(sliced[4..8].try_into().unwrap()) as usize + 8,
                sliced.len()
            );
            let output = directory.path().join("clamped.wav");
            slice_wav(&source, f32::MAX, -10.0, &output).unwrap();
            assert_eq!(std::fs::read(output).unwrap(), bytes);
        }
    }

    #[test]
    fn wav_slice_rejects_invalid_format_fields_before_creating_output() {
        for (offset, value) in [
            (22, 0u16.to_le_bytes().to_vec()),
            (24, 0u32.to_le_bytes().to_vec()),
            (28, 0u32.to_le_bytes().to_vec()),
            (32, 0u16.to_le_bytes().to_vec()),
            (34, 0u16.to_le_bytes().to_vec()),
            (34, 7u16.to_le_bytes().to_vec()),
            (22, u16::MAX.to_le_bytes().to_vec()),
            (24, u32::MAX.to_le_bytes().to_vec()),
        ] {
            let (directory, source, mut bytes) = slice_fixture();
            bytes[offset..offset + value.len()].copy_from_slice(&value);
            std::fs::write(&source, bytes).unwrap();
            let output = directory.path().join("slice.wav");
            assert!(slice_wav(&source, 0.0, 1.0, &output).is_err());
            assert!(!output.exists());
        }
    }

    #[test]
    fn wav_slice_rejects_truncation_and_huge_chunk_claims_without_allocating_them() {
        for (offset, size) in [(4, u32::MAX), (16, u32::MAX), (40, u32::MAX), (40, 15)] {
            let (directory, source, mut bytes) = slice_fixture();
            bytes[offset..offset + 4].copy_from_slice(&size.to_le_bytes());
            std::fs::write(&source, bytes).unwrap();
            let output = directory.path().join("slice.wav");
            assert!(slice_wav(&source, 0.0, 1.0, &output).is_err());
            assert!(!output.exists());
        }
    }

    #[test]
    fn wav_slice_preserves_existing_outputs_and_source() {
        for destination_kind in ["ordinary", "source", "symlink"] {
            let (directory, source, original) = slice_fixture();
            let output = if destination_kind == "source" {
                source.clone()
            } else {
                directory.path().join("slice.wav")
            };
            match destination_kind {
                "ordinary" => std::fs::write(&output, b"keep existing bytes").unwrap(),
                "symlink" => std::os::unix::fs::symlink(&source, &output).unwrap(),
                _ => {}
            }
            let result = slice_wav(&source, 0.25, 0.75, &output);
            assert!(
                result.is_err(),
                "accepted existing {destination_kind} output"
            );
            assert_eq!(std::fs::read(&source).unwrap(), original);
            if destination_kind == "ordinary" {
                assert_eq!(std::fs::read(&output).unwrap(), b"keep existing bytes");
            }
            if destination_kind == "symlink" {
                assert_eq!(std::fs::read_link(&output).unwrap(), source);
            }
            assert_eq!(
                std::fs::read_dir(directory.path()).unwrap().count(),
                if destination_kind == "source" { 1 } else { 2 }
            );
        }
    }

    #[test]
    fn wav_slice_installs_only_completed_output_in_new_directories() {
        let (directory, source, original) = slice_fixture();
        let output = directory.path().join("exports/audio/slice.wav");
        slice_wav(&source, 0.0, 1.0, &output).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(output.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn wav_slice_rejects_nonfinite_time_ranges() {
        let (directory, source, _) = slice_fixture();
        for (start, end) in [
            (f32::NAN, 0.5),
            (0.0, f32::INFINITY),
            (f32::NEG_INFINITY, 0.5),
        ] {
            let output = directory.path().join("slice.wav");
            assert!(slice_wav(&source, start, end, &output).is_err());
            assert!(!output.exists());
        }
    }

    #[test]
    fn test_circular_audio_buffer_wrap() {
        // 2 channels, 10 samples per sec, 2 sec buffer => capacity = 2 * 10 * 2 = 40 samples
        let mut buf = RollingAudioBuffer::new(2, 10, 2);
        assert_eq!(buf.capacity_samples(), 40);
        assert!(buf.is_empty());

        // Push 30 samples (values 0..30)
        let first_samples: Vec<i16> = (0..30).collect();
        buf.push_samples(&first_samples);
        assert_eq!(buf.len_samples(), 30);
        assert_eq!(buf.snapshot_samples(), first_samples);

        // Push 20 more samples (values 30..50) -> total 50 pushed, wrapping around!
        let next_samples: Vec<i16> = (30..50).collect();
        buf.push_samples(&next_samples);

        assert_eq!(buf.len_samples(), 40);
        let snapshot = buf.snapshot_samples();
        assert_eq!(snapshot.len(), 40);
        // The oldest 10 samples (0..10) were overwritten, so snapshot should be 10..50
        let expected: Vec<i16> = (10..50).collect();
        assert_eq!(snapshot, expected);
    }

    #[test]
    fn reconfigure_clears_samples_and_uses_new_capacity() {
        let mut buffer = RollingAudioBuffer::new(2, 10, 2);
        buffer.push_samples(&[1, 2, 3, 4]);

        buffer.reconfigure(3, 8, 1);

        assert!(buffer.is_empty());
        assert_eq!(buffer.sample_rate(), 8);
        assert_eq!(buffer.channels(), 1);
        assert_eq!(buffer.capacity_samples(), 24);
    }

    #[test]
    fn test_wav_export_and_slice() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wav_path = temp_dir.path().join("full_test.wav");
        let slice_path = temp_dir.path().join("slice_test.wav");

        // 1 sec of 44100Hz stereo
        let mut buf = RollingAudioBuffer::new(1, 44100, 2);
        let mut samples = Vec::with_capacity(88200);
        for i in 0..88200 {
            samples.push((i % 1000) as i16);
        }
        buf.push_samples(&samples);

        let dur = buf.write_wav(&wav_path).expect("Failed to write WAV");
        assert!((dur - 1.0).abs() < 0.01);

        // Slice middle 0.2 to 0.6 seconds (0.4s duration)
        slice_wav(&wav_path, 0.2, 0.6, &slice_path).expect("Failed to slice WAV");
        assert!(slice_path.exists());

        let metadata = std::fs::metadata(&slice_path).unwrap();
        // 44100 * 0.4 frames = 17640 frames * 4 bytes/frame = 70560 data bytes + 44 header bytes = 70604 bytes
        assert_eq!(metadata.len(), 70604);

        let _ = std::fs::remove_file(wav_path);
        let _ = std::fs::remove_file(slice_path);
    }
}
