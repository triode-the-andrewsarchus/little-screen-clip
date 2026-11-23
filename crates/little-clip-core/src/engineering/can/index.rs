use std::{cmp::Reverse, collections::BTreeMap, ops::Range, sync::atomic::AtomicBool};

use super::{
    Capture, Channel, FileIdentity, Format, Frame, FrameKind, Identifier, Location,
    MAX_DETAIL_ROWS, Scanner,
};

const PREVIEW_BYTES: u64 = 2 * 1024 * 1024;
const PREVIEW_ROWS: u64 = 4096;
const PREVIEW_DETAILS: u64 = 48;
const MAX_BINS: i128 = 1024;
const MAX_IDS: usize = 4096;
const MAX_CHANNELS: usize = 256;
const MAX_CHECKPOINTS: usize = 32768;
const INITIAL_STRIDE: u64 = 256;
const MAX_RANGE_SCAN_ROWS: u64 = 1 << 20;
const MAX_FILTER_SCAN_ROWS: u64 = 4_000_000;
const MAX_FILTER_HITS: usize = 65_536;

/// Observed traffic for a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelSummary {
    pub channel: Channel,
    pub frames: u64,
    pub error_frames: u64,
    pub payload_bytes: u64,
}

/// Observed traffic for a channel and arbitration identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentifierSummary {
    pub channel: Channel,
    pub id: Identifier,
    pub frames: u64,
}

/// Counts and frame-row anchors of one timestamp interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityBin {
    pub time_ns: Range<i128>,
    pub first_row: u64,
    pub last_row: u64,
    pub frames: u64,
    pub error_frames: u64,
    pub payload_bytes: u64,
}

/// Bounded activity and identifier counts for a prefix or the entire recording.
#[derive(Clone, Debug)]
pub struct Summary {
    pub format: Format,
    pub frames: u64,
    pub errors: u64,
    pub time_bounds_ns: Option<[i128; 2]>,
    pub channels: Vec<ChannelSummary>,
    pub identifiers: Vec<IdentifierSummary>,
    pub activity: Vec<ActivityBin>,
    /// Initial exact rows retained for the preview while index construction runs.
    pub sample: Vec<Frame>,
    pub omitted_identifier_frames: u64,
    pub omitted_channel_frames: u64,
    pub covered_bytes: u64,
    pub file_bytes: u64,
    pub complete: bool,
}

/// Sparse frame checkpoints and a completed capture summary.
#[derive(Clone, Debug)]
pub struct Index {
    pub summary: Summary,
    checkpoints: Vec<Checkpoint>,
    identity: FileIdentity,
}

/// Optional criteria for matching frame headers in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub channel: Option<Channel>,
    pub id: Option<Identifier>,
    pub errors_only: bool,
}

/// Matching frame headers from a bounded search.
#[derive(Debug)]
pub struct Filtered {
    pub frames: Vec<Frame>,
    pub scanned: u64,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug)]
struct Checkpoint {
    location: Location,
    row: u64,
    bounds: [i128; 2],
}

pub(super) fn preview(capture: &Capture, cancel: &AtomicBool) -> Result<Summary, String> {
    let mut reader = Scanner::new(capture, None, PREVIEW_BYTES)?;
    let mut summary = Aggregate::default();
    let mut sample = Vec::new();
    while summary.frames < PREVIEW_ROWS {
        let retain = summary.frames < PREVIEW_DETAILS;
        let Some((_, mut frame)) = reader.next(retain, cancel)? else {
            break;
        };
        frame.row = summary.frames;
        summary.push(&frame);
        if retain {
            sample.push(frame);
        }
    }
    let covered = reader.covered_bytes();
    let complete = covered >= capture.byte_len && summary.frames < PREVIEW_ROWS;
    capture.validate_identity()?;
    let mut summary = summary.finish(capture.format(), covered, capture.byte_len, complete);
    summary.sample = sample;
    Ok(summary)
}

pub(super) fn build(capture: &Capture, cancel: &AtomicBool) -> Result<Index, String> {
    let mut reader = Scanner::new(capture, None, u64::MAX)?;
    let mut summary = Aggregate::default();
    let mut checkpoints = Vec::<Checkpoint>::new();
    let mut stride = INITIAL_STRIDE;
    while let Some((location, frame)) = reader.next(false, cancel)? {
        let row = summary.frames;
        if row.is_multiple_of(stride) {
            if checkpoints.len() == MAX_CHECKPOINTS {
                compact(&mut checkpoints);
                stride = stride.saturating_mul(2);
            }
            if row.is_multiple_of(stride) {
                checkpoints.push(Checkpoint {
                    location,
                    row,
                    bounds: [frame.timestamp_ns; 2],
                });
            }
        }
        if let Some(point) = checkpoints.last_mut() {
            point.bounds[0] = point.bounds[0].min(frame.timestamp_ns);
            point.bounds[1] = point.bounds[1].max(frame.timestamp_ns);
        }
        summary.push(&frame);
    }
    let summary = summary.finish(
        capture.format(),
        reader.covered_bytes(),
        capture.byte_len,
        true,
    );
    capture.validate_identity()?;
    Ok(Index {
        summary,
        checkpoints,
        identity: capture.identity,
    })
}

fn compact(checkpoints: &mut Vec<Checkpoint>) {
    for index in 0..checkpoints.len() / 2 {
        let mut left = checkpoints[2 * index];
        let right = checkpoints[2 * index + 1];
        left.bounds[0] = left.bounds[0].min(right.bounds[0]);
        left.bounds[1] = left.bounds[1].max(right.bounds[1]);
        checkpoints[index] = left;
    }
    checkpoints.truncate(checkpoints.len() / 2);
}

pub(super) fn rows(
    capture: &Capture,
    index: &Index,
    rows: Range<u64>,
    cancel: &AtomicBool,
) -> Result<Vec<Frame>, String> {
    valid_index(capture, index)?;
    if rows.start > rows.end || rows.end > index.summary.frames {
        return Err("CAN frame row range is outside the recording".into());
    }
    if rows.end - rows.start > MAX_DETAIL_ROWS as u64 {
        return Err("CAN frame page exceeds its row limit".into());
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let point = index
        .checkpoints
        .partition_point(|point| point.row <= rows.start)
        - 1;
    let checkpoint = index.checkpoints[point];
    let mut scanner = Scanner::new(capture, Some(checkpoint.location), u64::MAX)?;
    let mut output = Vec::with_capacity((rows.end - rows.start) as usize);
    for row in checkpoint.row..rows.end {
        let retain = row >= rows.start;
        let (_, mut frame) = scanner
            .next(retain, cancel)?
            .ok_or("CAN frame index is incomplete")?;
        if retain {
            frame.row = row;
            output.push(frame);
        }
    }
    Ok(output)
}

pub(super) fn frames(
    capture: &Capture,
    index: &Index,
    window: Range<i128>,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<Frame>, String> {
    valid_index(capture, index)?;
    if limit > MAX_DETAIL_ROWS || window.start > window.end {
        return Err("Invalid CAN frame page request".into());
    }
    if limit == 0 || window.is_empty() {
        return Ok(Vec::new());
    }
    let mut output = Vec::with_capacity(limit.min(256));
    let mut scanned = 0u64;
    let mut reader = None;
    for (point, checkpoint) in index.checkpoints.iter().enumerate() {
        if checkpoint.bounds[1] < window.start || checkpoint.bounds[0] >= window.end {
            reader = None;
            continue;
        }
        let scanner = match &mut reader {
            Some(scanner) => scanner,
            slot => slot.insert(Scanner::new(capture, Some(checkpoint.location), u64::MAX)?),
        };
        let end = index
            .checkpoints
            .get(point + 1)
            .map_or(index.summary.frames, |next| next.row);
        for row in checkpoint.row..end {
            let (_, mut frame) = scanner
                .next(true, cancel)?
                .ok_or("CAN frame index is incomplete")?;
            if window.contains(&frame.timestamp_ns) {
                frame.row = row;
                output.push(frame);
                if output.len() == limit {
                    return Ok(output);
                }
            }
            scanned += 1;
            if scanned >= MAX_RANGE_SCAN_ROWS {
                return Err(
                    "CAN time window is too widely interleaved; use the frame table".into(),
                );
            }
        }
    }
    Ok(output)
}

fn valid_index(capture: &Capture, index: &Index) -> Result<(), String> {
    if index.identity != capture.identity || index.summary.format != capture.format() {
        return Err("CAN frame index belongs to a different recording".into());
    }
    capture.validate_identity()
}

pub(super) fn filter(
    capture: &Capture,
    index: &Index,
    filter: &Filter,
    cancel: &AtomicBool,
) -> Result<Filtered, String> {
    valid_index(capture, index)?;
    let mut reader = Scanner::new(capture, None, u64::MAX)?;
    let count = index.summary.frames;
    let mut frames = Vec::new();
    let mut scanned = 0;
    while scanned < count.min(MAX_FILTER_SCAN_ROWS) && frames.len() < MAX_FILTER_HITS {
        let (_, mut frame) = reader
            .next(false, cancel)?
            .ok_or("CAN frame index is incomplete")?;
        frame.row = scanned;
        scanned += 1;
        if filter
            .channel
            .as_ref()
            .is_some_and(|channel| *channel != frame.channel)
            || filter.id.is_some_and(|id| Some(id) != frame.id)
            || (filter.errors_only && frame.kind != FrameKind::Error)
        {
            continue;
        }
        frames.push(frame);
    }
    capture.validate_identity()?;
    Ok(Filtered {
        frames,
        scanned,
        complete: scanned == count,
    })
}

#[derive(Default)]
struct Aggregate {
    frames: u64,
    errors: u64,
    channels: BTreeMap<Channel, Count>,
    identifiers: BTreeMap<(Channel, Identifier), u64>,
    activity: Activity,
    omitted_channel_frames: u64,
    omitted_identifier_frames: u64,
}

impl Aggregate {
    fn push(&mut self, frame: &Frame) {
        let count = Count::frame(frame, self.frames);
        let channel = frame.channel.clone();
        self.frames += 1;
        self.errors += count.errors;
        if let Some(stored) = self.channels.get_mut(&channel) {
            stored.merge(count);
        } else if self.channels.len() < MAX_CHANNELS {
            self.channels.insert(channel.clone(), count);
        } else {
            self.omitted_channel_frames += 1;
        }
        if let Some(id) = frame.id {
            let pair = (channel, id);
            if let Some(stored) = self.identifiers.get_mut(&pair) {
                *stored += 1;
            } else if self.identifiers.len() < MAX_IDS {
                self.identifiers.insert(pair, 1);
            } else {
                self.omitted_identifier_frames += 1;
            }
        }
        self.activity.push(frame.timestamp_ns, count);
    }

    fn finish(
        self,
        format: Format,
        covered_bytes: u64,
        file_bytes: u64,
        complete: bool,
    ) -> Summary {
        let channels = self
            .channels
            .into_iter()
            .map(|(channel, count)| ChannelSummary {
                channel,
                frames: count.frames,
                error_frames: count.errors,
                payload_bytes: count.bytes,
            })
            .collect();
        let mut identifiers = self
            .identifiers
            .into_iter()
            .map(|((channel, id), frames)| IdentifierSummary {
                channel,
                id,
                frames,
            })
            .collect::<Vec<_>>();
        identifiers.sort_by_key(|identifier| Reverse(identifier.frames));
        Summary {
            format,
            frames: self.frames,
            errors: self.errors,
            time_bounds_ns: self.activity.bounds,
            channels,
            identifiers,
            activity: self.activity.bins(),
            sample: Vec::new(),
            omitted_channel_frames: self.omitted_channel_frames,
            omitted_identifier_frames: self.omitted_identifier_frames,
            covered_bytes,
            file_bytes,
            complete,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Count {
    frames: u64,
    errors: u64,
    bytes: u64,
    first: u64,
    last: u64,
}

impl Count {
    fn frame(frame: &Frame, row: u64) -> Self {
        Self {
            frames: 1,
            errors: u64::from(frame.kind == FrameKind::Error),
            bytes: u64::from(frame.data_len),
            first: row,
            last: row,
        }
    }

    fn merge(&mut self, other: Self) {
        self.first = if self.frames == 0 {
            other.first
        } else {
            self.first.min(other.first)
        };
        self.frames += other.frames;
        self.errors += other.errors;
        self.bytes += other.bytes;
        self.last = self.last.max(other.last);
    }
}

#[derive(Default)]
struct Activity {
    bounds: Option<[i128; 2]>,
    shift: u32,
    bins: BTreeMap<i128, Count>,
}

impl Activity {
    fn push(&mut self, time: i128, count: Count) {
        let [first, last] = self.bounds.unwrap_or([time; 2]);
        let first = first.min(time);
        let last = last.max(time);
        self.bounds = Some([first, last]);
        let old_shift = self.shift;
        while (last >> self.shift) - (first >> self.shift) >= MAX_BINS {
            self.shift += 1;
        }
        if self.shift != old_shift {
            let difference = self.shift - old_shift;
            for (bin, values) in std::mem::take(&mut self.bins) {
                self.bins
                    .entry(bin >> difference)
                    .or_default()
                    .merge(values);
            }
        }
        self.bins
            .entry(time >> self.shift)
            .or_default()
            .merge(count);
    }

    fn bins(self) -> Vec<ActivityBin> {
        self.bins
            .into_iter()
            .map(|(bin, count)| ActivityBin {
                time_ns: (bin << self.shift)..((bin + 1) << self.shift),
                first_row: count.first,
                last_row: count.last,
                frames: count.frames,
                error_frames: count.errors,
                payload_bytes: count.bytes,
            })
            .collect()
    }
}
