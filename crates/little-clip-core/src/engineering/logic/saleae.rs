//! Saleae Logic 2 digital channel exports, formats 0 and 1.
//!
//! https://www.saleae.com/support/logic-software/saving-and-exporting-data/binary-export-format-logic-2
//! https://new.saleae.com/support/logic-software/saving-and-exporting-data/binary-and-csv-export-formats-2025-update

use super::{Bin, Envelope, Lane, Level, Trace, check_cancel, push_state};
use std::{fs::File, ops::Range, os::unix::fs::FileExt, path::Path, sync::atomic::AtomicBool};

const LEGACY_BYTES: usize = 28;
const CHUNK_HEADER_BYTES: usize = 36;
const MAX_CHUNKS: u64 = 8192;
const READ_RECORDS: u64 = 1024;

pub(super) struct Export {
    file: File,
    segments: Vec<Segment>,
}

struct Segment {
    time: Range<f64>,
    rate: Option<f64>,
    initial: Level,
    offset: u64,
    count: u64,
}

pub fn matches_header(prefix: &[u8]) -> bool {
    matches!(
        prefix,
        [
            b'<',
            b'S',
            b'A',
            b'L',
            b'E',
            b'A',
            b'E',
            b'>',
            0 | 1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            ..
        ]
    )
}

impl Export {
    pub(super) fn open(file: File, path: &Path, len: u64) -> Result<(Self, Lane), String> {
        let mut header = [0; 24];
        file.read_exact_at(&mut header, 0).map_err(io_error)?;
        if !matches_header(&header) {
            return Err("Not a supported Saleae digital binary export.".into());
        }
        let version = header[8];
        let (count, mut offset) = if version == 0 {
            (1, 16)
        } else {
            (le_u64(&header, 16), 24)
        };
        if !(1..=MAX_CHUNKS).contains(&count) {
            return Err("Saleae export has an invalid segment count.".into());
        }
        let mut segments = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let (segment, end) = Segment::open(&file, offset, version, len)?;
            if segments
                .last()
                .is_some_and(|previous: &Segment| previous.time.end > segment.time.start)
            {
                return Err("Saleae digital segments overlap or are out of order.".into());
            }
            segments.push(segment);
            offset = end;
        }
        if offset != len {
            return Err("Saleae export contains unexpected trailing data.".into());
        }
        let first = &segments[0];
        let rate = first
            .rate
            .filter(|rate| segments.iter().all(|segment| segment.rate == Some(*rate)));
        let lane = Lane {
            name: path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Digital")
                .to_owned(),
            start_seconds: first.time.start,
            end_seconds: segments.last().unwrap().time.end,
            sample_rate: rate,
        };
        Ok((Self { file, segments }, lane))
    }

    pub(super) fn read_edges(
        &self,
        time: Range<f64>,
        max_edges: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        let mut trace = Trace {
            initial: None,
            edges: Vec::new(),
            inspected_until: time.start,
        };
        let mut last = None;
        let mut cursor = time.start;
        let mut record = |when, level| push_state(&mut trace, &mut last, max_edges, when, level);
        for segment in self.segments_in(&time) {
            check_cancel(cancel)?;
            let window = intersect(&segment.time, &time);
            if cursor < window.start && !record(cursor, Level::Unknown) {
                return Ok(trace);
            }
            if !segment.visit_edges(&self.file, &window, cancel, &mut record)? {
                return Ok(trace);
            }
            cursor = window.end;
        }
        if cursor < time.end && !record(cursor, Level::Unknown) {
            return Ok(trace);
        }
        trace.inspected_until = time.end;
        Ok(trace)
    }

    pub(super) fn summarize(
        &self,
        time: Range<f64>,
        columns: usize,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let mut bins = Vec::with_capacity(columns);
        for col in 0..columns {
            check_cancel(cancel)?;
            let window = super::time_bin(&time, col, columns);
            bins.push(self.bin(&window)?);
        }
        Ok(Envelope { time, bins })
    }

    fn bin(&self, time: &Range<f64>) -> Result<Bin, String> {
        let mut cursor = time.start;
        let mut bin = Bin::default();
        for segment in self.segments_in(time) {
            let window = intersect(&segment.time, time);
            if cursor < window.start {
                bin.observe(Level::Unknown);
            }
            let (first, end) = segment.bounds(&self.file, &window)?;
            bin.observe(segment.state(first));
            if end > first {
                bin.low = true;
                bin.high = true;
                bin.transitions += end - first;
                bin.last = Some(segment.state(end));
            }
            cursor = window.end;
        }
        if cursor < time.end {
            bin.observe(Level::Unknown);
        }
        bin.exact = true;
        Ok(bin)
    }

    fn segments_in(&self, time: &Range<f64>) -> impl Iterator<Item = &Segment> {
        let first = self
            .segments
            .partition_point(|segment| segment.time.end <= time.start);
        let end = self
            .segments
            .partition_point(|segment| segment.time.start < time.end);
        self.segments[first..end].iter()
    }
}

impl Segment {
    fn open(file: &File, offset: u64, version: u8, file_len: u64) -> Result<(Self, u64), String> {
        let mut buffer = [0; CHUNK_HEADER_BYTES];
        let (size, shift) = if version == 0 {
            (LEGACY_BYTES, 0)
        } else {
            (CHUNK_HEADER_BYTES, 8)
        };
        file.read_exact_at(&mut buffer[..size], offset)
            .map_err(io_error)?;
        let initial = match le_u32(&buffer, 0) {
            0 => Level::Low,
            1 => Level::High,
            _ => return Err("Saleae export has an invalid initial state.".into()),
        };
        let rate = (shift != 0).then(|| le_f64(&buffer, 4));
        let start = le_f64(&buffer, 4 + shift);
        let end = le_f64(&buffer, 12 + shift);
        let count = le_u64(&buffer, 20 + shift);
        let data = offset
            .checked_add(size as u64)
            .ok_or("Saleae export offset overflow.")?;
        let after = count
            .checked_mul(8)
            .and_then(|size| data.checked_add(size))
            .filter(|end| *end <= file_len)
            .ok_or("Saleae export transitions are truncated.")?;
        validate_segment_timing(start, end, rate)?;
        let segment = Self {
            time: start..end,
            rate,
            initial,
            offset: data,
            count,
        };
        if count > 0 && segment.transition(file, 0)? > segment.transition(file, count - 1)? {
            return Err("Saleae digital transitions are out of order.".into());
        }
        Ok((segment, after))
    }

    fn visit_edges(
        &self,
        file: &File,
        window: &Range<f64>,
        cancel: &AtomicBool,
        record: &mut impl FnMut(f64, Level) -> bool,
    ) -> Result<bool, String> {
        let (first, end) = self.bounds(file, window)?;
        if !record(window.start, self.state(first)) {
            return Ok(false);
        }
        let mut offset = first;
        let mut block = [0; (READ_RECORDS * 8) as usize];
        let mut last_time = window.start;
        while offset < end {
            check_cancel(cancel)?;
            let count = (end - offset).min(READ_RECORDS);
            let bytes = &mut block[..count as usize * 8];
            file.read_exact_at(bytes, self.offset + offset * 8)
                .map_err(io_error)?;
            for (index, bytes) in bytes.as_chunks::<8>().0.iter().enumerate() {
                let when = f64::from_le_bytes(*bytes);
                if !when.is_finite() || when <= last_time || when >= window.end {
                    return Err("Saleae digital transitions are out of order.".into());
                }
                if !record(when, self.state(offset + index as u64 + 1)) {
                    return Ok(false);
                }
                last_time = when;
            }
            offset += count;
        }
        Ok(true)
    }

    fn state(&self, edges: u64) -> Level {
        if edges.is_multiple_of(2) {
            self.initial
        } else if self.initial == Level::Low {
            Level::High
        } else {
            Level::Low
        }
    }

    fn bounds(&self, file: &File, time: &Range<f64>) -> Result<(u64, u64), String> {
        let first = self.search(file, |when| when <= time.start, 0)?;
        let end = self.search(file, |when| when < time.end, first)?;
        Ok((first, end))
    }

    fn search(
        &self,
        file: &File,
        before: impl Fn(f64) -> bool,
        mut low: u64,
    ) -> Result<u64, String> {
        let mut high = self.count;
        while low < high {
            let middle = low + (high - low) / 2;
            if before(self.transition(file, middle)?) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low)
    }

    fn transition(&self, file: &File, index: u64) -> Result<f64, String> {
        let mut bytes = [0; 8];
        file.read_exact_at(&mut bytes, self.offset + index * 8)
            .map_err(io_error)?;
        let value = f64::from_le_bytes(bytes);
        if !value.is_finite() || value < self.time.start || value > self.time.end {
            return Err("Saleae transition falls outside its segment.".into());
        }
        Ok(value)
    }
}

fn validate_segment_timing(start: f64, end: f64, rate: Option<f64>) -> Result<(), String> {
    if !start.is_finite()
        || !end.is_finite()
        || start >= end
        || rate.is_some_and(|rate| !rate.is_finite() || rate <= 0.0)
    {
        return Err("Saleae export has invalid time bounds.".into());
    }
    Ok(())
}

fn intersect(a: &Range<f64>, b: &Range<f64>) -> Range<f64> {
    a.start.max(b.start)..a.end.min(b.end)
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn le_f64(bytes: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

fn io_error(error: std::io::Error) -> String {
    format!("Reading Saleae digital export. {error}")
}
