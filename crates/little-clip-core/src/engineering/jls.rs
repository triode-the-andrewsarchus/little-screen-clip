//! File-backed Joulescope recordings and bounded waveform windows.

mod v1;
mod v2;

use std::{
    fs::File,
    ops::Range,
    os::unix::fs::FileExt,
    sync::atomic::{AtomicBool, Ordering},
};

use super::{
    scope::{Channel, MAX_SAMPLES_PER_READ},
    waveforms::{Envelope, Peak},
};

/// Number of bytes needed to identify a JLS format generation.
pub const HEADER_BYTES: usize = 16;

const V1_MAGIC: &[u8; HEADER_BYTES] = b"\xd3tagfmt \r\n \n  \x1a\x1c";
const V2_MAGIC: &[u8; HEADER_BYTES] = b"jlsfmt\r\n \n \x1a  \xb2\x1c";
const MAX_COLUMNS: usize = 16384;
const RAW_ENVELOPE_SAMPLES: u64 = 1 << 18;
const IO_WINDOW_SAMPLES: u64 = 16384;
const MAX_INTEGRAL_SAMPLES: u64 = 1 << 20;

/// Recognizes both published Joulescope file identifiers.
pub fn matches_header(prefix: &[u8]) -> bool {
    prefix.starts_with(V1_MAGIC) || prefix.starts_with(V2_MAGIC)
}

/// A recording backed by a retained regular-file handle and its on-disk index.
#[derive(Debug)]
pub struct Capture {
    reader: Reader,
    format: Format,
}

#[derive(Debug)]
enum Format {
    V1(v1::Recording),
    V2(v2::Recording),
}

impl Capture {
    /// Opens a guarded file handle without loading signal payloads.
    ///
    /// # Errors
    ///
    /// Returns an error when the format, metadata, or indexed signal is invalid.
    pub fn open(file: File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Joulescope recordings must be regular files".into());
        }
        let reader = Reader {
            file,
            len: metadata.len(),
        };
        let magic = reader.array::<HEADER_BYTES>(0)?;
        let format = match &magic {
            V1_MAGIC => Format::V1(v1::Recording::open(&reader)?),
            V2_MAGIC => Format::V2(v2::Recording::open(&reader)?),
            _ => return Err("Unrecognized Joulescope header".into()),
        };
        Ok(Self { reader, format })
    }

    /// Returns the recorded instrument model, or the format name when unspecified.
    pub fn model(&self) -> &str {
        match &self.format {
            Format::V1(recording) => recording.model(),
            Format::V2(recording) => &recording.model,
        }
    }

    /// Returns fixed-rate waveforms in display order.
    pub fn channels(&self) -> &[Channel] {
        match &self.format {
            Format::V1(recording) => &recording.channels,
            Format::V2(recording) => &recording.channels,
        }
    }

    /// Returns samples in physical units for a half-open, channel-relative range.
    ///
    /// Source bins that retain only extrema return a constant sample if their
    /// measured values agree, and NaN otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid range, cancellation, or damaged data.
    pub fn read_samples(
        &self,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        self.validate_range(channel, &range)?;
        if range.end - range.start > MAX_SAMPLES_PER_READ {
            return Err("Joulescope sample window exceeds its read limit".into());
        }
        match &self.format {
            Format::V1(recording) => recording.read_samples(&self.reader, channel, range, cancel),
            Format::V2(recording) => recording.read_samples(&self.reader, channel, range, cancel),
        }
    }

    /// Returns peak-preserving bins for the visible interval using file summaries.
    ///
    /// Bins may align outward to the stored summary grid. The returned envelope
    /// indicates when bounded sampling was necessary to cover the input.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid range, cancellation, or damaged data.
    pub fn envelope(
        &self,
        channel: usize,
        range: Range<u64>,
        columns: usize,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        self.validate_range(channel, &range)?;
        let columns = columns.clamp(1, MAX_COLUMNS) as u64;
        match &self.format {
            Format::V1(recording) => {
                recording.envelope(&self.reader, channel, range, columns, cancel)
            }
            Format::V2(recording) => {
                recording.envelope(&self.reader, channel, range, columns, cancel)
            }
        }
    }

    /// Integrates a window in physical units × seconds.
    ///
    /// Returns `None` when samples are missing or when the bounded read cannot
    /// verify completeness. Stored means have the precision saved in the file.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid range, cancellation, or damaged data.
    pub fn integral(
        &self,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Option<f64>, String> {
        self.validate_range(channel, &range)?;
        check_cancel(cancel)?;
        let sum = if range.end - range.start <= MAX_INTEGRAL_SAMPLES {
            raw_sum(&range, |range| self.read_samples(channel, range, cancel))?
        } else {
            match &self.format {
                Format::V1(recording) => {
                    recording.integral(&self.reader, channel, range, cancel)?
                }
                Format::V2(recording) => {
                    recording.integral(&self.reader, channel, range, cancel, true)?
                }
            }
        };
        Ok(sum.map(|sum| sum * self.channels()[channel].interval_seconds))
    }

    /// Estimates a large v2 window integral from stored means on available bins.
    ///
    /// The format does not record valid-sample counts for these means. The
    /// estimate assumes each finite bin mean applies to its whole bin and
    /// returns `None` for entirely missing bins or incomplete coverage.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid range, cancellation, or damaged data.
    pub fn summary_integral(
        &self,
        channel: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Option<f64>, String> {
        self.validate_range(channel, &range)?;
        check_cancel(cancel)?;
        let Format::V2(recording) = &self.format else {
            return Ok(None);
        };
        Ok(recording
            .integral(&self.reader, channel, range, cancel, false)?
            .map(|sum| sum * self.channels()[channel].interval_seconds))
    }

    fn validate_range(&self, channel: usize, range: &Range<u64>) -> Result<(), String> {
        let info = self
            .channels()
            .get(channel)
            .ok_or("Unknown Joulescope channel")?;
        if range.start > range.end || range.end > info.sample_count {
            return Err("Joulescope sample range is outside the channel".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Reader {
    file: File,
    len: u64,
}

impl Reader {
    fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<(), String> {
        if buffer.len() as u64 > self.len.saturating_sub(offset) || offset > self.len {
            return Err("Joulescope data extends beyond the recording".into());
        }
        self.file
            .read_exact_at(buffer, offset)
            .map_err(|error| format!("Read Joulescope file at {offset}: {error}"))
    }

    fn array<const N: usize>(&self, offset: u64) -> Result<[u8; N], String> {
        let mut bytes = [0; N];
        self.read(offset, &mut bytes)?;
        Ok(bytes)
    }

    fn bytes(&self, offset: u64, count: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; count];
        self.read(offset, &mut bytes)?;
        Ok(bytes)
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Joulescope read canceled".into());
    }
    Ok(())
}

fn new_envelope(start: u64, end: u64, step: u64) -> Envelope {
    Envelope {
        start,
        step,
        peaks: vec![Peak::default(); end.saturating_sub(start).div_ceil(step) as usize],
        exact: true,
    }
}

fn raw_envelope(
    envelope: &mut Envelope,
    range: Range<u64>,
    read: impl Fn(Range<u64>) -> Result<Vec<f32>, String>,
) -> Result<(), String> {
    let length = range.end - range.start;
    if length <= RAW_ENVELOPE_SAMPLES {
        let mut start = range.start;
        while start < range.end {
            let end = (start + IO_WINDOW_SAMPLES).min(range.end);
            let samples = read(start..end)?;
            add_samples(envelope, start, &samples);
            start = end;
        }
        return Ok(());
    }

    envelope.exact = false;
    // Bound both bytes and the number of seeks for an unindexed overview.
    let windows = envelope.peaks.len().clamp(1, 128) as u64;
    let step = length.div_ceil(windows);
    let mut start = range.start;
    while start < range.end {
        let end = start.saturating_add(128).min(range.end);
        let samples = read(start..end)?;
        add_samples(envelope, start, &samples);
        start = start.saturating_add(step);
    }
    Ok(())
}

fn raw_sum(
    range: &Range<u64>,
    read: impl Fn(Range<u64>) -> Result<Vec<f32>, String>,
) -> Result<Option<f64>, String> {
    if range.end - range.start > MAX_INTEGRAL_SAMPLES {
        return Ok(None);
    }
    let mut total = 0.0;
    let mut start = range.start;
    while start < range.end {
        let end = (start + IO_WINDOW_SAMPLES).min(range.end);
        for sample in read(start..end)? {
            if !sample.is_finite() {
                return Ok(None);
            }
            total += f64::from(sample);
        }
        start = end;
    }
    Ok(Some(total))
}

fn add_samples(envelope: &mut Envelope, start: u64, samples: &[f32]) {
    let mut used = 0;
    while used < samples.len() {
        let at = start - envelope.start + used as u64;
        let remaining = (envelope.step - (at % envelope.step)) as usize;
        let end = used + remaining.min(samples.len() - used);
        envelope.peaks[(at / envelope.step) as usize].add(&samples[used..end]);
        used = end;
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
