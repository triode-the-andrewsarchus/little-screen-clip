//! Standard Value Change Dump traces. Timestamp and state checkpoints avoid a
//! complete eager import of pre-existing captures.
//! https://sigrok.org/wiki/File_format:Vcd

mod header;
mod parser;

use self::{
    header::{Header, MAX_HEADER},
    parser::{Event, Parser},
};
use super::{
    Envelope, EnvelopeWriter, Lane, Level, MAX_INFLATE_BYTES, MAX_WINDOW_SAMPLES, Opened, Trace,
    check_cancel, push_state,
};
use std::{
    fs::File,
    ops::Range,
    os::unix::fs::FileExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const FIRST_WINDOW: u64 = 64 * 1024;
const TAIL_WINDOW: u64 = 256 * 1024;
const MAX_CHECKPOINTS: u64 = 32_768;
const MIN_CHECKPOINT_BYTES: u64 = 256 * 1024;
const OVERVIEW_PREFIX: u64 = 256 * 1024;
const OVERVIEW_PROBES: u64 = 24;
const PROBE_BYTES: u64 = 8192;
const PROBE_EVENTS: usize = 512;

pub(super) struct Channel {
    capture: Arc<Capture>,
    bit: usize,
}

struct Capture {
    file: Arc<File>,
    length: u64,
    header: Header,
    known_end: Option<u64>,
    indexed_end: AtomicU64,
    complete: AtomicBool,
    index: Mutex<Index>,
}

struct Index {
    parser: Parser,
    values: Vec<Level>,
    checkpoints: Vec<Checkpoint>,
    tick: u64,
}

struct Checkpoint {
    offset: u64,
    values: Box<[Level]>,
    tick: u64,
}

pub(super) fn open(file: File, length: u64) -> Result<Opened<Channel>, String> {
    let mut bytes = vec![0; length.min(MAX_HEADER) as usize];
    file.read_exact_at(&mut bytes, 0).map_err(io_error)?;
    let (header, bits) = Header::read(&bytes)?;
    let known_end = last_timestamp(&file, length, &header)?;
    let file = Arc::new(file);
    let initial = vec![Level::Unknown; header.values];
    let index = Index {
        parser: Parser::open(Arc::clone(&file), header.data_start..length),
        values: initial.clone(),
        checkpoints: vec![Checkpoint {
            offset: header.data_start,
            values: initial.into_boxed_slice(),
            tick: 0,
        }],
        tick: 0,
    };
    let capture = Arc::new(Capture {
        file,
        length,
        header,
        index: Mutex::new(index),
        known_end,
        indexed_end: AtomicU64::new(0),
        complete: AtomicBool::new(false),
    });
    if known_end.is_none() {
        let mut budget = FIRST_WINDOW;
        capture.prepare(u64::MAX, &mut budget, &AtomicBool::new(false))?;
    }
    let bounds = capture.bounds();
    if !bounds.end.is_finite() || bounds.end <= bounds.start {
        return Err("VCD contains no readable timing interval.".into());
    }
    let channels = capture
        .header
        .names
        .iter()
        .zip(bits)
        .map(|(name, bit)| {
            let lane = Lane {
                name: name.clone(),
                start_seconds: bounds.start,
                end_seconds: bounds.end,
                sample_rate: None,
            };
            (
                lane,
                Channel {
                    capture: Arc::clone(&capture),
                    bit,
                },
            )
        })
        .collect();
    let omitted = capture.header.omitted;
    let note = (omitted > 0).then(|| {
        format!("{omitted} non-digital or over-limit VCD declaration(s) are unavailable.")
    });
    Ok(Opened { channels, note })
}

impl Channel {
    pub(super) fn bounds(&self) -> Range<f64> {
        self.capture.bounds()
    }

    pub(super) fn refine(&self, budget: &mut u64, cancel: &AtomicBool) -> Result<bool, String> {
        self.capture.prepare(u64::MAX, budget, cancel)?;
        Ok(self.capture.complete.load(Ordering::Acquire))
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
        let inspected = self.visit(
            &time,
            MAX_WINDOW_SAMPLES,
            MAX_INFLATE_BYTES,
            cancel,
            |span, level| push_state(&mut trace, &mut last, max_edges, span.start, level),
        )?;
        trace.inspected_until = inspected;
        Ok(trace)
    }

    pub(super) fn summarize(
        &self,
        time: Range<f64>,
        columns: usize,
        budget: u64,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let coarse = self.capture.use_sparse_overview(&time, budget);
        let mut writer = EnvelopeWriter::new(time.clone(), columns);
        let prepare_bytes = if coarse {
            OVERVIEW_PREFIX
        } else {
            MAX_INFLATE_BYTES
        };
        self.visit(
            &time,
            budget.clamp(1, MAX_WINDOW_SAMPLES),
            prepare_bytes,
            cancel,
            |span, level| {
                writer.observe(span, level);
                true
            },
        )?;
        let mut envelope = writer.finish();
        if coarse && envelope.sampled() {
            self.sample_overview(&mut envelope, cancel)?;
        }
        Ok(envelope)
    }

    fn sample_overview(&self, envelope: &mut Envelope, cancel: &AtomicBool) -> Result<(), String> {
        let first = self.capture.header.data_start;
        let length = self.capture.length - first;
        let mut previous = None;
        for index in 0..OVERVIEW_PROBES {
            check_cancel(cancel)?;
            let offset = first + length / OVERVIEW_PROBES * index;
            let end = (offset + PROBE_BYTES).min(self.capture.length);
            let Some(anchor) = self.capture.probe_anchor(offset..end)? else {
                continue;
            };
            let Ok(samples) = self.probe_states(anchor..end) else {
                // Sparse samples have no preceding parser state. Any ambiguity
                // in a local sample is resolved during exact indexing instead.
                continue;
            };
            for (tick, level) in samples {
                if previous.is_some_and(|previous| previous >= tick) {
                    continue;
                }
                previous = Some(tick);
                let time = self.capture.time_at(tick);
                observe_approximate(envelope, time, level);
            }
        }
        Ok(())
    }

    fn probe_states(&self, bytes: Range<u64>) -> Result<Vec<(u64, Level)>, String> {
        let mut parser = Parser::open(Arc::clone(&self.capture.file), bytes);
        let Some(Event::Time(mut tick)) = parser.next(&self.capture.header)? else {
            return Ok(Vec::new());
        };
        let mut samples = Vec::new();
        let mut state = None;
        for _ in 0..PROBE_EVENTS {
            match parser.next(&self.capture.header) {
                // An incomplete final token beyond the sample boundary does
                // not invalidate earlier, timestamped observations.
                Ok(None) | Err(_) => break,
                Ok(Some(Event::Time(next))) if next > tick => {
                    if let Some(level) = state {
                        samples.push((tick, level));
                    }
                    tick = next;
                }
                Ok(Some(Event::Time(next))) if next < tick => return Ok(Vec::new()),
                Ok(Some(Event::Change(signal, values)))
                    if (signal.start..signal.start + signal.width).contains(&self.bit) =>
                {
                    state = Some(values[self.bit - signal.start]);
                }
                Ok(Some(Event::Reset)) => state = Some(Level::Unknown),
                _ => {}
            }
        }
        Ok(samples)
    }

    fn visit(
        &self,
        time: &Range<f64>,
        sample_budget: u64,
        prepare_bytes: u64,
        cancel: &AtomicBool,
        mut consume: impl FnMut(Range<f64>, Level) -> bool,
    ) -> Result<f64, String> {
        let end_tick = (time.end / self.capture.header.interval).ceil() as u64;
        let first_tick = (time.start / self.capture.header.interval).floor() as u64;
        let mut budget = prepare_bytes;
        self.capture.prepare(end_tick, &mut budget, cancel)?;
        let (offset, tick, level) = self.capture.checkpoint(first_tick, self.bit)?;
        let available = self
            .capture
            .time_at(self.capture.indexed_end.load(Ordering::Acquire))
            .min(time.end);
        if available <= time.start {
            return Ok(time.start);
        }
        let mut parser = Parser::open(Arc::clone(&self.capture.file), offset..self.capture.length);
        let mut visit = Visit {
            level,
            tick,
            time: time.start..available,
            remaining: sample_budget,
        };
        for index in 0..MAX_WINDOW_SAMPLES {
            if index.is_multiple_of(4096) {
                check_cancel(cancel)?;
                if parser.offset - offset >= prepare_bytes {
                    break;
                }
            }
            let event = parser.next(&self.capture.header)?;
            let stop = match event {
                None => visit.publish(available, self.capture.header.interval, &mut consume),
                Some(event) => {
                    match visit.event(event, self.bit, self.capture.header.interval, &mut consume) {
                        Some(stop) => stop,
                        None => continue,
                    }
                }
            };
            return Ok(stop);
        }
        Ok(self
            .capture
            .time_at(visit.tick)
            .clamp(time.start, available))
    }
}

struct Visit {
    level: Level,
    tick: u64,
    time: Range<f64>,
    remaining: u64,
}

impl Visit {
    fn event(
        &mut self,
        event: Event,
        bit: usize,
        interval: f64,
        consume: &mut impl FnMut(Range<f64>, Level) -> bool,
    ) -> Option<f64> {
        match event {
            Event::Change(signal, values)
                if (signal.start..signal.start + signal.width).contains(&bit) =>
            {
                self.level = values[bit - signal.start];
            }
            Event::Reset => self.level = Level::Unknown,
            Event::Time(tick) if tick > self.tick => {
                let until = tick as f64 * interval;
                let visited = self.publish(until, interval, consume);
                if visited < until || visited >= self.time.end {
                    return Some(visited);
                }
                self.tick = tick;
            }
            _ => {}
        }
        None
    }

    fn publish(
        &mut self,
        until: f64,
        interval: f64,
        consume: &mut impl FnMut(Range<f64>, Level) -> bool,
    ) -> f64 {
        let start = (self.tick as f64 * interval).max(self.time.start);
        let end = until.min(self.time.end);
        if start < end {
            if self.remaining == 0 || !consume(start..end, self.level) {
                return start;
            }
            self.remaining -= 1;
        }
        until.min(self.time.end)
    }
}

impl Capture {
    fn use_sparse_overview(&self, time: &Range<f64>, event_budget: u64) -> bool {
        let extent = self.bounds();
        self.length > MAX_INFLATE_BYTES
            && !self.complete.load(Ordering::Acquire)
            && time.end - time.start >= (extent.end - extent.start) * 0.25
            && event_budget <= 32_768
    }

    fn probe_anchor(&self, bytes: Range<u64>) -> Result<Option<u64>, String> {
        let mut block = [0; PROBE_BYTES as usize];
        let block = &mut block[..(bytes.end - bytes.start) as usize];
        self.file
            .read_exact_at(block, bytes.start)
            .map_err(io_error)?;
        let mut offset = bytes.start;
        for (index, line) in block.split_inclusive(|&byte| byte == b'\n').enumerate() {
            let token = line
                .trim_ascii_start()
                .split(u8::is_ascii_whitespace)
                .next()
                .unwrap_or_default();
            if index != 0
                && line.last() == Some(&b'\n')
                && let Some(time) = token.strip_prefix(b"#")
                && !self.header.symbols.contains_key(token)
                && parser::tick(time).is_ok()
            {
                return Ok(Some(offset));
            }
            offset += line.len() as u64;
        }
        Ok(None)
    }

    fn bounds(&self) -> Range<f64> {
        let indexed = self.indexed_end.load(Ordering::Acquire);
        // The fully verified timestamp supersedes any tail preview.
        let end = if self.complete.load(Ordering::Acquire) {
            indexed
        } else {
            self.known_end.unwrap_or(indexed).max(indexed)
        };
        0.0..self.time_at(end)
    }

    fn time_at(&self, ticks: u64) -> f64 {
        ticks as f64 * self.header.interval
    }

    fn checkpoint(&self, tick: u64, bit: usize) -> Result<(u64, u64, Level), String> {
        let index = self
            .index
            .lock()
            .map_err(|_| "VCD timestamp index is unavailable.")?;
        let n = index
            .checkpoints
            .partition_point(|point| point.tick <= tick)
            .saturating_sub(1);
        let point = &index.checkpoints[n];
        Ok((point.offset, point.tick, point.values[bit]))
    }

    fn prepare(&self, target: u64, budget: &mut u64, cancel: &AtomicBool) -> Result<(), String> {
        let mut index = self
            .index
            .lock()
            .map_err(|_| "VCD timestamp index is unavailable.")?;
        let start = index.parser.offset;
        let limit = start.saturating_add(*budget);
        let stride = self
            .length
            .div_ceil(MAX_CHECKPOINTS)
            .max(MIN_CHECKPOINT_BYTES);
        while !self.complete.load(Ordering::Acquire)
            && index.tick < target
            && index.parser.offset < limit
        {
            check_cancel(cancel)?;
            match index.parser.next(&self.header)? {
                None => {
                    self.indexed_end.store(index.tick + 1, Ordering::Release);
                    self.complete.store(true, Ordering::Release);
                }
                Some(event) => index.apply(event, stride)?,
            }
        }
        if !self.complete.load(Ordering::Acquire) {
            self.indexed_end.store(index.tick, Ordering::Release);
        }
        *budget = budget.saturating_sub(index.parser.offset - start);
        Ok(())
    }
}

impl Index {
    fn apply(&mut self, event: Event, interval: u64) -> Result<(), String> {
        match event {
            Event::Time(tick) => {
                if tick < self.tick {
                    return Err("VCD timestamps go backwards.".into());
                }
                let offset = self.parser.offset;
                if tick > self.tick && offset - self.checkpoints.last().unwrap().offset >= interval
                {
                    self.checkpoints.push(Checkpoint {
                        offset,
                        values: self.values.clone().into_boxed_slice(),
                        tick,
                    });
                }
                self.tick = tick;
            }
            Event::Change(signal, levels) => self.values[signal.start..signal.start + signal.width]
                .copy_from_slice(&levels[..signal.width]),
            Event::Reset => self.values.fill(Level::Unknown),
            Event::Ignore => {}
        }
        Ok(())
    }
}

fn observe_approximate(envelope: &mut Envelope, at: f64, level: Level) {
    if !envelope.time.contains(&at) {
        return;
    }
    let fraction = (at - envelope.time.start) / (envelope.time.end - envelope.time.start);
    let index = (fraction * envelope.bins.len() as f64) as usize;
    if let Some(bin) = envelope.bins.get_mut(index)
        && !bin.exact
    {
        bin.observe(level);
    }
}

fn last_timestamp(file: &File, length: u64, header: &Header) -> Result<Option<u64>, String> {
    let start = length.saturating_sub(TAIL_WINDOW).max(header.data_start);
    let mut bytes = vec![0; (length - start) as usize];
    file.read_exact_at(&mut bytes, start).map_err(io_error)?;
    for line in bytes.rsplit(|&byte| byte == b'\n') {
        let token = line
            .trim_ascii_start()
            .split(u8::is_ascii_whitespace)
            .next()
            .unwrap_or_default();
        if let Some(time) = token.strip_prefix(b"#")
            && !header.symbols.contains_key(token)
            && let Ok(tick) = parser::tick(time)
        {
            return Ok(Some(tick + 1));
        }
    }
    Ok(None)
}

fn io_error(error: std::io::Error) -> String {
    format!("Reading VCD. {error}")
}
