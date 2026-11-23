//! Timestamped, calibrated analog channels read from saved oscilloscope captures.

mod binary;
mod rigol;
mod rigol_csv;
mod siglent;
mod tektronix;

use binary::Lane;

use std::{
    fs::File,
    ops::Range,
    os::unix::fs::FileExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Maximum header prefix needed to recognize a supported scope capture.
pub const HEADER_BYTES: usize = tektronix::HEADER_BYTES;

/// Maximum number of samples returned by a single ranged read.
pub const MAX_SAMPLES_PER_READ: u64 = 1 << 20;

/// Returns whether a pathname is a candidate for oscilloscope recognition.
pub fn is_candidate_path(path: &Path) -> bool {
    path.extension().is_some_and(|value| {
        ["wfm", "csv", "bin"]
            .iter()
            .any(|extension| value.eq_ignore_ascii_case(extension))
    })
}

/// Recognizes a supported waveform signature in a bounded prefix.
///
/// This covers WFM signatures and the two specific Rigol CSV header rows.
/// Use [matches_header_with_len] to identify Siglent binary captures as well.
pub fn matches_header(prefix: &[u8]) -> bool {
    rigol::matches_header(prefix)
        || tektronix::matches_header(prefix)
        || rigol_csv::Header::recognize(prefix).is_some()
}

/// Recognizes a waveform header, including length-checked Siglent exports.
pub fn matches_header_with_len(prefix: &[u8], file_len: u64) -> bool {
    matches_header(prefix) || siglent::matches_header(prefix, file_len)
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
    /// Time coordinate of the first sample in the recorded time grid, in seconds.
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
enum Samples {
    Binary(Vec<Lane>),
    Csv(rigol_csv::Rows),
}

#[derive(Debug)]
struct Layout {
    model: String,
    channels: Vec<Channel>,
    samples: Samples,
}

impl Layout {
    fn binary(model: String, channels: Vec<Channel>, lanes: Vec<Lane>) -> Self {
        Self {
            model,
            channels,
            samples: Samples::Binary(lanes),
        }
    }

    fn read(file: &File, prefix: &[u8], file_len: u64) -> Result<Self, String> {
        if rigol::matches_header(prefix) {
            return rigol::parse(prefix, file_len);
        }
        if tektronix::matches_header(prefix) {
            let (channel, lane) = tektronix::parse(prefix, file_len)?;
            return Ok(Self::binary(
                "Tektronix WFM".into(),
                vec![channel],
                vec![lane],
            ));
        }
        if let Some(header) = rigol_csv::Header::recognize(prefix) {
            let (channels, rows) = header.open(file, file_len)?;
            return Ok(Self {
                model: "Rigol CSV".into(),
                channels,
                samples: Samples::Csv(rows),
            });
        }
        if let Ok((channels, lanes)) = siglent::open(prefix, file_len) {
            return Ok(Self::binary("Siglent V1".into(), channels, lanes));
        }
        Err("This oscilloscope waveform layout is unsupported.".into())
    }
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
    /// The binary readers only read the header. CSV also reads a bounded part
    /// of its first and last data rows to establish the sample extent. The
    /// caller should retain [crate::file_source::ReadCheck] for deferred jobs.
    ///
    /// # Errors
    /// Returns an error for an unreadable, truncated, or unsupported capture.
    pub fn open(file: File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Scope capture needs a regular file.".into());
        }
        let mut header = [0; HEADER_BYTES];
        let prefix = &mut header[..metadata.len().min(HEADER_BYTES as u64) as usize];
        file.read_exact_at(prefix, 0)
            .map_err(|error| format!("Reading scope header. {error}"))?;
        let layout = Layout::read(&file, prefix, metadata.len())?;
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
        let metadata = self
            .layout
            .channels
            .get(channel)
            .ok_or("Scope channel is unavailable.")?;
        if range.end > metadata.sample_count || range.start > range.end {
            return Err("Sample range is outside this capture.".into());
        }
        let count = range.end - range.start;
        if count > MAX_SAMPLES_PER_READ {
            return Err("Sample range exceeds the per-read limit.".into());
        }
        check_cancelled(cancel)?;

        match &self.layout.samples {
            Samples::Binary(lanes) => lanes[channel].read_samples(&self.file, range, cancel),
            Samples::Csv(rows) => rows.read_samples(&self.file, channel, range, cancel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    const RIGOL_HEADER_BYTES: usize = rigol::HEADER_BYTES;

    fn sample_capture(mask: u8, firmware: &[u8], values: &[u8]) -> File {
        let mut header = [0; RIGOL_HEADER_BYTES];
        header[..8].copy_from_slice(&[1, 0xff, 0xff, 0xff, 0xa6, 0xa5, 0x38, 0]);
        header[8..15].copy_from_slice(b"DS1054Z");
        header[28..28 + firmware.len()].copy_from_slice(firmware);
        header[48..50].copy_from_slice(&[1, 0]);
        header[72..80].copy_from_slice(&100_000_i64.to_le_bytes());
        header[84..86].copy_from_slice(&[0xd8, 0]);
        header[88] = mask;
        header[116..120].copy_from_slice(&(values.len() as u32).to_le_bytes());
        header[120..124].copy_from_slice(&0.25_f32.to_le_bytes());
        header[260..264].copy_from_slice(&(RIGOL_HEADER_BYTES as u32).to_le_bytes());

        for n in 0..4 {
            let offset = 92 + n * 4;
            header[offset..offset + 4].copy_from_slice(&(RIGOL_HEADER_BYTES as u32).to_le_bytes());
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
        file.set_len((RIGOL_HEADER_BYTES + 4) as u64).unwrap();
        assert!(Capture::open(file).is_err());
    }

    #[test]
    fn only_content_recognition_admits_overloaded_extensions() {
        assert!(is_candidate_path(Path::new("capture.BIN")));
        assert!(is_candidate_path(Path::new("capture.CSV")));
        assert!(!is_candidate_path(Path::new("capture.zip")));
        for prefix in [b"Time,Amplitude\n0,4\n".as_slice(), &[0; HEADER_BYTES]] {
            let len = prefix.len() as u64;
            assert!(!matches_header_with_len(prefix, len));
            let mut file = tempfile::tempfile().unwrap();
            file.write_all(prefix).unwrap();
            assert!(Capture::open(file).unwrap_err().contains("unsupported"));
        }
    }

    #[test]
    fn dispatches_rigol_csv_through_the_shared_range_reader() {
        let bytes =
            b"X,CH2,CH4,Start,Increment,\nSequence,Volt,Volt,-2.0e-3,1.0e-3,\n0,-1,3,,,\n1,1,-3,,,";
        assert!(matches_header_with_len(bytes, bytes.len() as u64));
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        let capture = Capture::open(file).unwrap();
        assert_eq!(capture.model(), "Rigol CSV");
        assert_eq!(capture.channels()[1].name, "CH4");
        assert_eq!(capture.channels()[1].time_at(1), -1e-3);
        assert_eq!(
            capture
                .read_samples(1, 0..2, &AtomicBool::new(false))
                .unwrap(),
            [3.0, -3.0]
        );
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
