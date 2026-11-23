//! Saved logic timing captures and bounded transition reads.
mod archive;
mod native;
mod packed;
mod rle;
mod saleae;
mod session;
mod vcd;

use std::{
    fs::File,
    ops::Range,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_LANES: usize = 256;
const MAX_EDGES: usize = 1 << 20;
const MAX_WINDOW_SAMPLES: u64 = 1 << 22;
const MAX_INFLATE_BYTES: u64 = 16 << 20;
const MAX_COLUMNS: usize = 4096;

/// Recognized capture formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Sigrok,
    DsView,
    SaleaeExport,
    Saleae,
    Vcd,
}

/// Digital state at a sample or an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Low,
    High,
    Unknown,
    HighImpedance,
}

/// Information for a channel in the saved capture.
#[derive(Clone, Debug)]
pub struct Lane {
    pub name: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub sample_rate: Option<f64>,
}

/// A state change on the capture's time axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub time_seconds: f64,
    pub level: Level,
}

/// Bounded state changes in a time window.
#[derive(Debug)]
pub struct Trace {
    /// Signal state at the left edge, if data was available there.
    pub initial: Option<Level>,
    pub edges: Vec<Edge>,
    /// End of the prefix searched for exact transitions.
    pub inspected_until: f64,
}

/// Signal activity in one half-open display interval.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bin {
    pub first: Option<Level>,
    pub last: Option<Level>,
    pub low: bool,
    pub high: bool,
    pub unknown: bool,
    pub high_impedance: bool,
    pub transitions: u64,
    pub exact: bool,
}

impl Bin {
    fn observe(&mut self, level: Level) {
        self.first.get_or_insert(level);
        if self.last.is_some_and(|last| last != level) {
            self.transitions += 1;
        }
        self.last = Some(level);
        match level {
            Level::Low => self.low = true,
            Level::High => self.high = true,
            Level::Unknown => self.unknown = true,
            Level::HighImpedance => self.high_impedance = true,
        }
    }
}

/// Display bins in order across the returned interval.
#[derive(Debug)]
pub struct Envelope {
    pub time: Range<f64>,
    pub bins: Vec<Bin>,
}

impl Envelope {
    pub fn sampled(&self) -> bool {
        self.bins.iter().any(|bin| !bin.exact)
    }
}

/// A saved capture backed by the pinned file and its metadata.
pub struct Capture {
    format: Format,
    lanes: Vec<Lane>,
    streams: Vec<Stream>,
    note: Option<String>,
}

struct Opened<T> {
    channels: Vec<(Lane, T)>,
    note: Option<String>,
}

enum Stream {
    Packed(packed::Stream),
    Saleae(saleae::Export),
    Native(native::Native),
    Vcd(vcd::Channel),
}

impl Capture {
    /// Opens an already guarded ordinary file. The stream payload is left lazy.
    ///
    /// # Errors
    /// Returns an error for malformed archives or unsupported capture formats.
    pub fn open(file: File, path: &Path) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("A logic capture must be a regular file.".into());
        }
        if has_extension(path, "bin") {
            let (source, lane) = saleae::Export::open(file, path, metadata.len())?;
            return Ok(Self {
                format: Format::SaleaeExport,
                lanes: vec![lane],
                streams: vec![Stream::Saleae(source)],
                note: None,
            });
        }
        if has_extension(path, "vcd") {
            let channels = vcd::open(file, metadata.len())?;
            return Ok(Self::from_opened(Format::Vcd, channels, Stream::Vcd));
        }
        let file = Arc::new(file);
        let mut archive = archive::Archive::open(file, metadata.len())?;
        if has_extension(path, "sal") {
            let channels = native::open(&mut archive)?;
            return Ok(Self::from_opened(Format::Saleae, channels, Stream::Native));
        }
        let (format, session) = if has_extension(path, "sr") {
            (Format::Sigrok, session::sigrok(&mut archive)?)
        } else if has_extension(path, "dsl") {
            (Format::DsView, session::dsview(&mut archive)?)
        } else {
            return Err("Unrecognized logic capture format.".into());
        };
        Ok(Self {
            format,
            lanes: session.lanes,
            streams: session.streams.into_iter().map(Stream::Packed).collect(),
            note: session.note,
        })
    }

    fn from_opened<T>(format: Format, opened: Opened<T>, wrap: impl Fn(T) -> Stream) -> Self {
        let (lanes, streams) = opened
            .channels
            .into_iter()
            .map(|(lane, source)| (lane, wrap(source)))
            .unzip();
        Self {
            format,
            lanes,
            streams,
            note: opened.note,
        }
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn lanes(&self) -> &[Lane] {
        &self.lanes
    }

    /// Best-known extent of a channel on the capture time axis.
    pub fn time_bounds(&self, lane: usize) -> Option<Range<f64>> {
        let metadata = self.lanes.get(lane)?;
        Some(match &self.streams[lane] {
            Stream::Native(stream) => stream.bounds(),
            Stream::Vcd(stream) => stream.bounds(),
            _ => metadata.start_seconds..metadata.end_seconds,
        })
    }

    /// Advances bounded indexing where the format leaves duration implicit.
    ///
    /// Returns true when all extents are known. Read [Self::time_bounds] after
    /// each call to display or navigate the newly available range.
    /// # Errors
    /// Returns an error for invalid input, cancellation, or read failure.
    pub fn refine(&self, cancel: &AtomicBool) -> Result<bool, String> {
        let mut budget = MAX_INFLATE_BYTES;
        let mut complete = true;
        for stream in &self.streams {
            complete &= match stream {
                Stream::Native(stream) => stream.refine(&mut budget, cancel)?,
                Stream::Vcd(stream) => stream.refine(&mut budget, cancel)?,
                _ => true,
            };
        }
        Ok(complete)
    }

    /// Reports channel data the reader cannot display.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Reads exact edges in the available prefix of a half-open time window.
    ///
    /// An edge, sample, or archive-inflation budget can shorten the prefix.
    /// Retrying a seek may advance an archive member's decompression.
    pub fn read_edges(
        &self,
        lane: usize,
        time: Range<f64>,
        max_edges: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        let (metadata, stream, time) = self.window(lane, time)?;
        let max_edges = max_edges.min(MAX_EDGES);
        match stream {
            Stream::Packed(stream) => Self::packed_edges(metadata, stream, time, max_edges, cancel),
            Stream::Saleae(stream) => stream.read_edges(time, max_edges, cancel),
            Stream::Native(stream) => stream.read_edges(time, max_edges, cancel),
            Stream::Vcd(stream) => stream.read_edges(time, max_edges, cancel),
        }
    }

    fn packed_edges(
        metadata: &Lane,
        stream: &packed::Stream,
        time: Range<f64>,
        max_edges: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        let mut trace = Trace {
            initial: None,
            edges: Vec::new(),
            inspected_until: time.start,
        };
        let mut budget = MAX_INFLATE_BYTES;
        let mut last = None;
        let samples = stream.sample_range(metadata, &time);
        let end = samples
            .end
            .min(samples.start.saturating_add(MAX_WINDOW_SAMPLES));
        let visited = stream.scan(samples.start..end, &mut budget, cancel, |sample, level| {
            if let Some(previous) = last {
                if level != previous {
                    if trace.edges.len() == max_edges {
                        return false;
                    }
                    trace.edges.push(Edge {
                        time_seconds: stream.time_at(metadata, sample),
                        level,
                    });
                }
            } else {
                trace.initial = Some(level);
            }
            last = Some(level);
            true
        })?;
        trace.inspected_until = stream
            .time_at(metadata, visited)
            .clamp(time.start, time.end);
        Ok(trace)
    }

    /// Preserves all observed states and counts transitions in each display bin.
    ///
    /// When a requested bin is larger than its share of the sample budget, it
    /// is marked approximate. Passing smaller windows allows exact refinement.
    pub fn summarize(
        &self,
        lane: usize,
        time: Range<f64>,
        columns: usize,
        sample_budget: u64,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let (metadata, stream, time) = self.window(lane, time)?;
        let columns = columns.clamp(1, MAX_COLUMNS);
        match stream {
            Stream::Packed(stream) => {
                Self::packed_envelope(metadata, stream, time, columns, sample_budget, cancel)
            }
            Stream::Saleae(stream) => stream.summarize(time, columns, cancel),
            Stream::Native(stream) => stream.summarize(time, columns, sample_budget, cancel),
            Stream::Vcd(stream) => stream.summarize(time, columns, sample_budget, cancel),
        }
    }

    fn packed_envelope(
        metadata: &Lane,
        stream: &packed::Stream,
        time: Range<f64>,
        columns: usize,
        sample_budget: u64,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        let mut bins = Vec::with_capacity(columns);
        let mut inflate_budget = MAX_INFLATE_BYTES;
        let sample_budget = sample_budget.min(MAX_WINDOW_SAMPLES);
        for col in 0..columns {
            check_cancel(cancel)?;
            let whole = stream.sample_range(metadata, &time_bin(&time, col, columns));
            let sample_limit = sample_budget / columns as u64
                + u64::from((col as u64) < sample_budget % columns as u64);
            let sampled = stream.preview_range(&whole, sample_limit);
            let mut bin = Bin::default();
            let allowance = if sampled == whole {
                inflate_budget
            } else {
                let spent = MAX_INFLATE_BYTES - inflate_budget;
                let available = MAX_INFLATE_BYTES * (col + 1) as u64 / columns as u64;
                available.saturating_sub(spent)
            };
            let mut budget = allowance;
            let inspected = stream.scan(sampled.clone(), &mut budget, cancel, |_, level| {
                bin.observe(level);
                true
            })?;
            inflate_budget -= allowance - budget;
            bin.exact = inspected == whole.end && sampled.start == whole.start;
            bins.push(bin);
        }
        Ok(Envelope { time, bins })
    }

    fn window(
        &self,
        lane: usize,
        time: Range<f64>,
    ) -> Result<(&Lane, &Stream, Range<f64>), String> {
        let metadata = self
            .lanes
            .get(lane)
            .ok_or("Logic channel is unavailable.")?;
        if !time.start.is_finite() || !time.end.is_finite() || time.start >= time.end {
            return Err("Invalid logic time window.".into());
        }
        let bounds = self
            .time_bounds(lane)
            .ok_or("Logic channel is unavailable.")?;
        let start = time.start.max(bounds.start);
        let end = time.end.min(bounds.end);
        if end <= start {
            return Err("Window is outside this logic channel.".into());
        }
        Ok((metadata, &self.streams[lane], start..end))
    }
}

/// Returns whether a pathname can name a supported logic capture.
pub fn is_candidate_path(path: &Path) -> bool {
    ["sr", "dsl", "bin", "sal", "vcd"]
        .iter()
        .any(|extension| has_extension(path, extension))
}

/// Verifies a candidate using only bounded capture metadata.
///
/// The caller supplies an already guarded file. The descriptor's seek
/// position is left unchanged.
pub fn probe(file: &File, path: &Path) -> bool {
    is_candidate_path(path)
        && file
            .try_clone()
            .ok()
            .is_some_and(|clone| Capture::open(clone, path).is_ok())
}

/// Recognizes a documented Saleae digital binary export signature.
pub fn matches_binary_header(prefix: &[u8]) -> bool {
    saleae::matches_header(prefix)
}

fn time_bin(time: &Range<f64>, column: usize, total: usize) -> Range<f64> {
    let width = (time.end - time.start) / total as f64;
    let left = time.start + column as f64 * width;
    let right = if column + 1 == total {
        time.end
    } else {
        time.start + (column + 1) as f64 * width
    };
    left..right
}

// The source yields increasing, non-overlapping signal spans. A column
// becomes exact only after their combined coverage reaches its right edge.
struct EnvelopeWriter {
    time: Range<f64>,
    bins: Vec<Bin>,
    column: usize,
}

impl EnvelopeWriter {
    fn new(time: Range<f64>, columns: usize) -> Self {
        Self {
            time,
            bins: vec![Bin::default(); columns],
            column: 0,
        }
    }

    fn observe(&mut self, span: Range<f64>, level: Level) {
        while self.column < self.bins.len() {
            let window = time_bin(&self.time, self.column, self.bins.len());
            if window.start >= span.end {
                break;
            }
            let bin = &mut self.bins[self.column];
            if window.end > span.start {
                bin.observe(level);
            }
            if span.end < window.end {
                break;
            }
            bin.exact = bin.first.is_some();
            self.column += 1;
        }
    }

    fn finish(self) -> Envelope {
        Envelope {
            time: self.time,
            bins: self.bins,
        }
    }
}

fn push_state(
    trace: &mut Trace,
    last: &mut Option<Level>,
    max_edges: usize,
    time_seconds: f64,
    level: Level,
) -> bool {
    trace.inspected_until = time_seconds;
    match *last {
        None => trace.initial = Some(level),
        Some(previous) if previous == level => return true,
        Some(_) if trace.edges.len() == max_edges => return false,
        Some(_) => trace.edges.push(Edge {
            time_seconds,
            level,
        }),
    }
    *last = Some(level);
    true
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Logic capture read cancelled.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
