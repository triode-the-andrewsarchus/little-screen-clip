//! Timestamped, calibrated analog channels read from saved oscilloscope captures.

mod rigol;

use std::{
    fs::File,
    ops::Range,
    os::unix::fs::FileExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Maximum header prefix needed to recognize a supported binary scope capture.
pub const HEADER_BYTES: usize = 280;

/// Maximum number of samples returned by a single ranged read.
pub const MAX_SAMPLES_PER_READ: u64 = 1 << 20;

/// Returns whether a pathname is a candidate for oscilloscope recognition.
pub fn is_candidate_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("wfm"))
}

/// Recognizes the Rigol DS1000Z-family file header in a bounded prefix.
pub fn matches_header(prefix: &[u8]) -> bool {
    rigol::matches_header(prefix)
}

/// Calibrated sample grid and display metadata for one saved channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// Channel identifier from the instrument layout.
    pub name: String,
    /// Measurement unit such as `V` or `A`.
    pub unit: String,
    /// Number of samples present in the capture.
    pub sample_count: u64,
    /// Time of the first sample relative to the capture trigger, in seconds.
    pub start_seconds: f64,
    /// Time between consecutive samples, in seconds.
    pub interval_seconds: f64,
}

impl Channel {
    /// Returns the time coordinate for a sample on this channel's grid.
    pub fn time_at(&self, index: u64) -> f64 {
        (index as f64).mul_add(self.interval_seconds, self.start_seconds)
    }
}

#[derive(Debug)]
struct ByteLane {
    offset: u64,
    stride: usize,
    column: usize,
    gain: f64,
    bias: f64,
}

#[derive(Debug)]
struct Layout {
    model: String,
    channels: Vec<Channel>,
    lanes: Vec<ByteLane>,
}

/// A capture backed by one retained file handle and a bounded header.
#[derive(Debug)]
pub struct Capture {
    file: File,
    layout: Layout,
}

impl Capture {
    /// Opens an already guarded, readable capture handle.
    ///
    /// Only the header is read. The caller should obtain the file through
    /// [crate::file_source::ReadCheck] and retain its observation checks for
    /// deferred jobs.
    ///
    /// # Errors
    /// Returns an error for an unreadable, truncated, or unsupported capture.
    pub fn open(file: File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Scope capture needs a regular file.".into());
        }
        let mut header = [0; HEADER_BYTES];
        file.read_exact_at(&mut header, 0)
            .map_err(|error| format!("Reading scope header. {error}"))?;
        let layout = rigol::parse(&header, metadata.len())?;
        Ok(Self { file, layout })
    }

    /// Returns the instrument model recorded in the capture.
    pub fn model(&self) -> &str {
        &self.layout.model
    }

    /// Returns the saved analog channels in instrument order.
    pub fn channels(&self) -> &[Channel] {
        &self.layout.channels
    }

    /// Reads a half-open sample interval in the channel's measurement unit.
    ///
    /// The positioned read leaves the handle's file position untouched. A
    /// caller can share one capture across independent range jobs.
    ///
    /// # Errors
    /// Returns an error for an invalid channel or range, an interval above the
    /// per-read limit, cancellation, or unavailable sample data.
    pub fn read_samples(
        &self,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        let lane = self
            .layout
            .lanes
            .get(channel)
            .ok_or("Scope channel is unavailable.")?;
        if range.end > self.layout.channels[channel].sample_count || range.start > range.end {
            return Err("Sample range is outside this capture.".into());
        }
        let count = range.end - range.start;
        if count > MAX_SAMPLES_PER_READ {
            return Err("Sample range exceeds the per-read limit.".into());
        }
        check_cancelled(cancel)?;

        // Parser validation bounds the entire payload by the file length.
        let offset = lane.offset + range.start * lane.stride as u64;
        let mut bytes = vec![0; count as usize * lane.stride];
        for (index, chunk) in bytes.chunks_mut(64 * 1024).enumerate() {
            check_cancelled(cancel)?;
            self.file
                .read_exact_at(chunk, offset + (index * 64 * 1024) as u64)
                .map_err(|error| format!("Reading scope samples. {error}"))?;
        }
        check_cancelled(cancel)?;
        let samples = bytes
            .chunks_exact(lane.stride)
            .map(|frame| ((f64::from(frame[lane.column]) - 127.0) * lane.gain + lane.bias) as f32)
            .collect();
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    fn sample_capture(mask: u8, firmware: &[u8], values: &[u8]) -> File {
        let mut header = [0; HEADER_BYTES];
        header[..8].copy_from_slice(&[1, 0xff, 0xff, 0xff, 0xa6, 0xa5, 0x38, 0]);
        header[8..15].copy_from_slice(b"DS1054Z");
        header[28..28 + firmware.len()].copy_from_slice(firmware);
        header[48..50].copy_from_slice(&[1, 0]);
        header[72..80].copy_from_slice(&100_000_i64.to_le_bytes());
        header[84..86].copy_from_slice(&[0xd8, 0]);
        header[88] = mask;
        header[116..120].copy_from_slice(&(values.len() as u32).to_le_bytes());
        header[120..124].copy_from_slice(&0.25_f32.to_le_bytes());
        header[260..264].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());

        for n in 0..4 {
            let offset = 92 + n * 4;
            header[offset..offset + 4].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
            let offset = 124 + 28 * n;
            header[offset] = u8::from(mask & (1 << n) != 0);
            header[offset + 8..offset + 12].copy_from_slice(&1.0_f32.to_le_bytes());
            header[offset + 17] = 2;
        }
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&header).unwrap();
        file.write_all(values).unwrap();
        file
    }

    #[test]
    fn reads_sparse_channel_layout_as_calibrated_framed_samples() {
        let file = sample_capture(0b1001, b"00.04.04.SP3", &[127, 167, 147, 107, 127, 127]);
        let capture = Capture::open(file).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(capture.model(), "DS1054Z");
        assert_eq!(capture.channels()[0].name, "CH1");
        assert_eq!(capture.channels()[1].name, "CH4");
        assert_eq!(capture.channels()[1].sample_count, 3);
        assert!((capture.channels()[0].time_at(2) - 102e-9).abs() < 1e-20);
        assert_eq!(
            capture.read_samples(0, 0..3, &cancel).unwrap(),
            [2.0, -1.0, 0.0]
        );
        assert_eq!(capture.read_samples(1, 1..2, &cancel).unwrap(), [1.0]);
    }

    #[test]
    fn reads_three_channels_with_a_four_byte_stride() {
        let file = sample_capture(0b1101, b"00.04.04.SP4", &[147, 127, 0, 107]);
        let capture = Capture::open(file).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            capture
                .channels()
                .iter()
                .map(|ch| ch.name.as_str())
                .collect::<Vec<_>>(),
            ["CH1", "CH3", "CH4"]
        );
        assert_eq!(capture.read_samples(0, 0..1, &cancel).unwrap(), [0.0]);
        assert_eq!(capture.read_samples(1, 0..1, &cancel).unwrap(), [1.0]);
        assert_eq!(capture.read_samples(2, 0..1, &cancel).unwrap(), [2.0]);
    }

    #[test]
    fn rejects_declared_samples_past_the_end_of_the_file() {
        let file = sample_capture(1, b"00.04.04.SP3", &[0; 5]);
        file.set_len((HEADER_BYTES + 4) as u64).unwrap();
        assert!(Capture::open(file).is_err());
    }

    #[test]
    fn validates_ranges_and_cancellation_without_changing_file_position() {
        let file = sample_capture(1, b"00.04.04.SP4", &[127; 10]);
        let mut retained = file.try_clone().unwrap();
        retained.seek(SeekFrom::Start(33)).unwrap();
        let capture = Capture::open(file).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(capture.read_samples(0, 7..11, &cancel).is_err());
        assert_eq!(capture.read_samples(0, 7..9, &cancel).unwrap(), [1.0; 2]);
        assert_eq!(retained.stream_position().unwrap(), 33);
        cancel.store(true, Ordering::Relaxed);
        assert!(capture.read_samples(0, 0..10, &cancel).is_err());
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Scope sample read cancelled.".into())
    } else {
        Ok(())
    }
}
