use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
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
            raw.into_iter()
                .map(|s| s as f32 / 32768.0)
                .collect()
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
        let riff_size = 36 + data_size;

        // RIFF header
        writer.write_all(b"RIFF")?;
        writer.write_all(&riff_size.to_le_bytes())?;
        writer.write_all(b"WAVE")?;

        // "fmt " chunk
        writer.write_all(b"fmt ")?;
        writer.write_all(&16u32.to_le_bytes())?; // Subchunk1Size for PCM
        writer.write_all(&1u16.to_le_bytes())?;  // AudioFormat 1 = PCM
        writer.write_all(&channels.to_le_bytes())?;
        writer.write_all(&sample_rate.to_le_bytes())?;
        writer.write_all(&byte_rate.to_le_bytes())?;
        writer.write_all(&block_align.to_le_bytes())?;
        writer.write_all(&bits_per_sample.to_le_bytes())?;

        // "data" chunk
        writer.write_all(b"data")?;
        writer.write_all(&data_size.to_le_bytes())?;

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

/// Slices a sub-region `[start_sec, end_sec]` from a 16-bit PCM WAV file and writes it to `out_path`.
pub fn slice_wav(source: &Path, start_sec: f32, end_sec: f32, out_path: &Path) -> Result<(), std::io::Error> {
    let mut file = BufReader::new(File::open(source)?);

    let mut riff_header = [0u8; 12];
    file.read_exact(&mut riff_header)?;
    if &riff_header[0..4] != b"RIFF" || &riff_header[8..12] != b"WAVE" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not a valid RIFF/WAVE file",
        ));
    }

    let mut channels: Option<u16> = None;
    let mut sample_rate: Option<u32> = None;
    let mut bits_per_sample: Option<u16> = None;
    let mut data_start_offset: Option<u64> = None;
    let mut data_bytes_len: Option<u32> = None;

    // Scan chunks
    loop {
        let mut chunk_header = [0u8; 8];
        if file.read_exact(&mut chunk_header).is_err() {
            break;
        }

        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes([
            chunk_header[4],
            chunk_header[5],
            chunk_header[6],
            chunk_header[7],
        ]);

        if chunk_id == b"fmt " {
            let mut fmt_bytes = vec![0u8; chunk_size as usize];
            file.read_exact(&mut fmt_bytes)?;
            if fmt_bytes.len() >= 16 {
                let format_tag = u16::from_le_bytes([fmt_bytes[0], fmt_bytes[1]]);
                if format_tag != 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Only uncompressed PCM WAV is supported for slicing",
                    ));
                }
                channels = Some(u16::from_le_bytes([fmt_bytes[2], fmt_bytes[3]]));
                sample_rate = Some(u32::from_le_bytes([
                    fmt_bytes[4],
                    fmt_bytes[5],
                    fmt_bytes[6],
                    fmt_bytes[7],
                ]));
                bits_per_sample = Some(u16::from_le_bytes([fmt_bytes[14], fmt_bytes[15]]));
            }
        } else if chunk_id == b"data" {
            let offset = file.stream_position()?;
            data_start_offset = Some(offset);
            data_bytes_len = Some(chunk_size);
            break;
        } else {
            file.seek(SeekFrom::Current(chunk_size as i64))?;
        }
    }

    let channels = channels.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing fmt chunk in WAV")
    })?;
    let sample_rate = sample_rate.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing sample rate in WAV")
    })?;
    let bits_per_sample = bits_per_sample.unwrap_or(16);
    let bytes_per_sample = (bits_per_sample / 8) as u32;
    let bytes_per_frame = channels as u32 * bytes_per_sample;

    let data_offset = data_start_offset.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing data chunk in WAV")
    })?;
    let total_data_bytes = data_bytes_len.unwrap_or(0);
    let total_frames = total_data_bytes / bytes_per_frame;
    let total_duration = total_frames as f32 / sample_rate as f32;

    let (s, e) = if start_sec <= end_sec {
        (start_sec, end_sec)
    } else {
        (end_sec, start_sec)
    };

    let start_clamped = s.clamp(0.0, total_duration);
    let end_clamped = e.clamp(0.0, total_duration);
    let frame_start = (start_clamped * sample_rate as f32) as u32;
    let frame_end = (end_clamped * sample_rate as f32).max(frame_start as f32) as u32;
    let frames_to_copy = (frame_end - frame_start).min(total_frames.saturating_sub(frame_start));

    let slice_byte_offset = data_offset + (frame_start * bytes_per_frame) as u64;
    let slice_byte_len = frames_to_copy * bytes_per_frame;

    file.seek(SeekFrom::Start(slice_byte_offset))?;

    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let out_file = File::create(out_path)?;
    let mut writer = BufWriter::new(out_file);

    let riff_size = 36 + slice_byte_len;
    let byte_rate = sample_rate * bytes_per_frame;

    // RIFF header
    writer.write_all(b"RIFF")?;
    writer.write_all(&riff_size.to_le_bytes())?;
    writer.write_all(b"WAVE")?;

    // "fmt " chunk
    writer.write_all(b"fmt ")?;
    writer.write_all(&16u32.to_le_bytes())?;
    writer.write_all(&1u16.to_le_bytes())?; // PCM
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&(bytes_per_frame as u16).to_le_bytes())?;
    writer.write_all(&bits_per_sample.to_le_bytes())?;

    // "data" chunk
    writer.write_all(b"data")?;
    writer.write_all(&slice_byte_len.to_le_bytes())?;

    let mut remaining = slice_byte_len as usize;
    let mut buffer = [0u8; 8192];
    while remaining > 0 {
        let to_read = remaining.min(buffer.len());
        let n = file.read(&mut buffer[..to_read])?;
        if n == 0 {
            break;
        }
        writer.write_all(&buffer[..n])?;
        remaining -= n;
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_wav_export_and_slice() {
        let temp_dir = std::env::temp_dir().join("little_clip_test_audio");
        let _ = std::fs::create_dir_all(&temp_dir);
        let wav_path = temp_dir.join("full_test.wav");
        let slice_path = temp_dir.join("slice_test.wav");

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
