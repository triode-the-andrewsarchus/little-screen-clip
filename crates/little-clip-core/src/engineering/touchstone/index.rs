use super::{
    Complex64, Error, MAX_HEADER_BYTES, Metadata, Point, Reader,
    stream::{Record, is_line_break},
};
use std::{
    io::{BufRead, Cursor, Read, Seek},
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const CHECKPOINT_INTERVAL: u64 = 256;
const MAX_CHECKPOINTS: usize = 65_536;
const MAX_OVERVIEW_BUCKETS: usize = 1024;
const MAX_WINDOW_ROWS: usize = 16_384;
const MAX_WINDOW_VALUES: usize = 512 * 1024;
const MAX_PREVIEW_ROWS: usize = 1024;

/// A point retained by the magnitude envelope, with its exact source identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TracePoint {
    /// Source record ordinal.
    pub index: u64,
    /// Center frequency in hertz.
    pub frequency_hz: f64,
    /// Value of the plotted network parameter.
    pub value: Complex64,
}

/// Extrema and endpoints from a contiguous interval of a network parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    /// First point in the interval.
    pub first: TracePoint,
    /// Point with the smallest magnitude in the interval.
    pub minimum: TracePoint,
    /// Point with the greatest magnitude in the interval.
    pub maximum: TracePoint,
    /// Last point in the interval.
    pub last: TracePoint,
}

impl Envelope {
    fn new(point: TracePoint) -> Self {
        Self {
            first: point,
            minimum: point,
            maximum: point,
            last: point,
        }
    }

    fn join(&mut self, next: Self) {
        if next.minimum.value.norm() < self.minimum.value.norm() {
            self.minimum = next.minimum;
        }
        if next.maximum.value.norm() > self.maximum.value.norm() {
            self.maximum = next.maximum;
        }
        self.last = next.last;
    }
}

/// A bounded prefix preview, available before a full sweep is indexed.
#[derive(Clone, Debug)]
pub struct Summary {
    /// Network metadata from the bounded header.
    pub metadata: Metadata,
    /// Initial points in source order.
    pub points: Vec<Point>,
    /// Whether the entire network sweep is represented.
    pub complete: bool,
}

/// Reads a bounded preview of the beginning of a sweep.
///
/// The input may be arbitrarily large. The preview reads at most 256 KiB
/// plus a single byte to distinguish a full file from a sampled prefix.
///
/// # Errors
///
/// Returns an error if the header cannot be identified, no complete points
/// fit in the preview, cancellation was requested, or the read failed.
pub fn summary(
    input: impl Read,
    path: &Path,
    row_limit: usize,
    cancelled: &AtomicBool,
) -> Result<Summary, Error> {
    let mut prefix = read_prefix(input, cancelled)?;
    let truncated = prefix.len() as u64 > MAX_HEADER_BYTES;
    if truncated {
        prefix.truncate(MAX_HEADER_BYTES as usize);
        let newline = prefix
            .iter()
            .rposition(|&byte| is_line_break(byte))
            .ok_or(Error::Limit("preview line"))?;
        prefix.truncate(newline + 1);
    }
    let mut reader = Reader::open(Cursor::new(prefix), path)?;
    let metadata = reader.metadata().clone();
    let count = row_limit
        .clamp(1, MAX_PREVIEW_ROWS)
        .min(MAX_WINDOW_VALUES / (metadata.ports * metadata.ports));
    let mut points = Vec::new();
    let mut complete = !truncated;
    loop {
        check_cancelled(cancelled)?;
        match reader.next_point() {
            Ok(Some(point)) => {
                if points.len() == count {
                    complete = false;
                    break;
                }
                points.push(point);
            }
            Ok(None) => break,
            Err(Error::Truncated) if truncated => break,
            Err(error) => return Err(error),
        }
    }
    if points.is_empty() {
        return Err(Error::Truncated);
    }
    Ok(Summary {
        metadata,
        points,
        complete,
    })
}

fn read_prefix(input: impl Read, cancelled: &AtomicBool) -> Result<Vec<u8>, Error> {
    let mut input = input.take(MAX_HEADER_BYTES + 1);
    let mut prefix = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        check_cancelled(cancelled)?;
        match input.read(&mut buffer) {
            Ok(0) => return Ok(prefix),
            Ok(count) => prefix.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Sparse record checkpoints and a bounded full-sweep envelope.
///
/// The caller must tie the index to the exact observed revision of the source.
/// Matrix values are read directly from the source when a window is requested.
#[derive(Clone, Debug)]
pub struct Index {
    checkpoints: Vec<Record>,
    checkpoint_interval: u64,
    envelope: Vec<Envelope>,
    trace: [usize; 2],
    bucket_interval: u64,
    points: u64,
}

impl Index {
    /// Returns the number of indexed frequency records.
    pub fn points(&self) -> u64 {
        self.points
    }

    /// Returns the exact beginning and ending frequencies in hertz.
    pub fn band_hz(&self) -> [f64; 2] {
        // Build only publishes a nonempty index.
        [
            self.envelope[0].first.frequency_hz,
            self.envelope
                .last()
                .map_or(0.0, |bin| bin.last.frequency_hz),
        ]
    }

    /// Returns up to 1024 buckets for the indexed matrix element.
    pub fn overview(&self) -> &[Envelope] {
        &self.envelope
    }

    /// Returns the zero-based response and stimulus for the indexed overview.
    pub fn trace(&self) -> [usize; 2] {
        self.trace
    }

    fn push(&mut self, record: Record, value: Complex64) {
        self.push_checkpoint(record);
        let bin = Envelope::new(TracePoint {
            index: record.index,
            frequency_hz: record.frequency_hz,
            value,
        });
        let bucket = record.index / self.bucket_interval;
        if bucket as usize == self.envelope.len() {
            if self.envelope.len() == MAX_OVERVIEW_BUCKETS {
                self.coarsen();
            }
            self.envelope.push(bin);
        } else if let Some(last) = self.envelope.last_mut() {
            last.join(bin);
        }
        self.points = record.index + 1;
    }

    fn coarsen(&mut self) {
        self.bucket_interval *= 2;
        let count = self.envelope.len() / 2;
        for index in 0..count {
            let mut bin = self.envelope[2 * index];
            bin.join(self.envelope[2 * index + 1]);
            self.envelope[index] = bin;
        }
        self.envelope.truncate(count);
    }

    fn push_checkpoint(&mut self, record: Record) {
        if !record.index.is_multiple_of(self.checkpoint_interval) {
            return;
        }
        if self.checkpoints.len() == MAX_CHECKPOINTS {
            self.checkpoint_interval *= 2;
            let mut index = 0;
            self.checkpoints.retain(|_| {
                index += 1;
                index % 2 == 1
            });
        }
        if record.index.is_multiple_of(self.checkpoint_interval) {
            self.checkpoints.push(record);
        }
    }

    fn preceding_row(&self, row: u64) -> Record {
        let position = self
            .checkpoints
            .partition_point(|record| record.index <= row);
        self.checkpoints[position.saturating_sub(1)]
    }

    fn preceding_frequency(&self, frequency_hz: f64) -> Record {
        let position = self
            .checkpoints
            .partition_point(|record| record.frequency_hz < frequency_hz);
        self.checkpoints[position.saturating_sub(1)]
    }
}

impl<R: BufRead + Seek> Reader<R> {
    /// Scans the sweep once, retaining sparse offsets and magnitude extrema for the first matrix element.
    ///
    /// The scanner uses constant-size buffers and does not allocate a matrix
    /// for each record. Start this work after the bounded preview is visible.
    ///
    /// # Errors
    ///
    /// Returns an error if a record is invalid, if the read is cancelled,
    /// or if the input cannot be read and repositioned.
    pub fn build_index(&mut self, cancelled: &AtomicBool) -> Result<Index, Error> {
        self.build_trace_index([0, 0], cancelled)
    }

    /// Builds an overview for one matrix element, with zero-based response and stimulus indices.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown matrix element, invalid record,
    /// cancellation, or failure to read and reposition the input.
    pub fn build_trace_index(
        &mut self,
        trace: [usize; 2],
        cancelled: &AtomicBool,
    ) -> Result<Index, Error> {
        let [response, stimulus] = trace;
        let ports = self.metadata().ports;
        if response >= ports || stimulus >= ports {
            return Err(Error::Invalid("unknown network parameter"));
        }
        let position = response * ports + stimulus;
        self.restart()?;
        let mut index = Index {
            checkpoints: Vec::new(),
            checkpoint_interval: CHECKPOINT_INTERVAL,
            envelope: Vec::new(),
            trace,
            bucket_interval: 1,
            points: 0,
        };
        loop {
            check_cancelled(cancelled)?;
            let Some(record) = self.read_record()? else {
                break;
            };
            index.push(record, self.sample_value(position));
        }
        if index.points == 0 {
            Err(Error::Invalid("network data has no frequency points"))
        } else {
            Ok(index)
        }
    }

    /// Reads exact rows from the selected source interval using sparse checkpoints.
    ///
    /// Use an index built from the same observation of the capture. Bounds past
    /// the final row are clamped. An empty interval returns an empty vector.
    ///
    /// # Errors
    ///
    /// Returns an error if the window exceeds the row or matrix memory limit,
    /// the read is cancelled, or the source cannot be repositioned and decoded.
    pub fn read_window(
        &mut self,
        index: &Index,
        rows: Range<u64>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Point>, Error> {
        let rows = rows.start.min(index.points)..rows.end.min(index.points);
        let count = rows.end.saturating_sub(rows.start);
        let ports = self.metadata().ports;
        if count > MAX_WINDOW_ROWS as u64 || count > (MAX_WINDOW_VALUES / (ports * ports)) as u64 {
            return Err(Error::Limit("selected record window"));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let first = index.preceding_row(rows.start);
        self.position_at(first.offset, first.index)?;
        let mut points = Vec::with_capacity(count as usize);
        for row in first.index..rows.end {
            check_cancelled(cancelled)?;
            let record = self.read_record()?.ok_or(Error::Truncated)?;
            if row >= rows.start {
                points.push(self.point(record));
            }
        }
        Ok(points)
    }

    /// Finds the nearest exact record for a frequency marker.
    ///
    /// Use an index built from the same observation of the capture.
    ///
    /// # Errors
    ///
    /// Returns an error if the marker is non-finite or if the required
    /// records cannot be read or the request is cancelled.
    pub fn nearest(
        &mut self,
        index: &Index,
        frequency_hz: f64,
        cancelled: &AtomicBool,
    ) -> Result<Point, Error> {
        if !frequency_hz.is_finite() {
            return Err(Error::Invalid("marker frequency must be finite"));
        }
        let first = index.preceding_frequency(frequency_hz);
        self.position_at(first.offset, first.index)?;
        let mut previous: Option<Record> = None;
        for _ in first.index..index.points {
            check_cancelled(cancelled)?;
            let record = self.read_record()?.ok_or(Error::Truncated)?;
            if record.frequency_hz >= frequency_hz {
                if let Some(previous) = previous
                    && frequency_hz - previous.frequency_hz <= record.frequency_hz - frequency_hz
                {
                    self.position_at(previous.offset, previous.index)?;
                    return self.next_point()?.ok_or(Error::Truncated);
                }
                return Ok(self.point(record));
            }
            previous = Some(record);
        }
        previous
            .map(|record| self.point(record))
            .ok_or(Error::Truncated)
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
