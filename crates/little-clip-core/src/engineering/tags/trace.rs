use super::{
    Error,
    source::{Checkpoints, cancelled},
};
use std::{
    io::{BufRead, Cursor, ErrorKind, Read, Seek, SeekFrom},
    ops::Range,
    sync::atomic::AtomicBool,
};

const HEADER: usize = 8;
const PREVIEW_ROWS: usize = 128;
const MAX_WINDOW: u64 = 1024;
const MAX_PAYLOAD_WINDOW: usize = 2 * 1024 * 1024;
const MAX_BINS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Reader,
    Tag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    pub row: u64,
    pub timestamp_ticks: u32,
    pub duration_ticks: u16,
    pub direction: Direction,
    pub data_len: u16,
}

impl FrameInfo {
    pub fn parity_len(self) -> usize {
        usize::from(self.data_len).div_ceil(8)
    }

    fn body_len(self) -> usize {
        usize::from(self.data_len) + self.parity_len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub info: FrameInfo,
    pub data: Vec<u8>,
    pub parity: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TracePreview {
    pub frames: Vec<FrameInfo>,
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub struct TraceBin {
    pub first: FrameInfo,
    pub last: FrameInfo,
    pub readers: u64,
    pub tags: u64,
}

impl TraceBin {
    fn new(frame: FrameInfo) -> Self {
        Self {
            first: frame,
            last: frame,
            readers: u64::from(frame.direction == Direction::Reader),
            tags: u64::from(frame.direction == Direction::Tag),
        }
    }

    fn merge(&mut self, next: &Self) {
        self.last = next.last;
        self.readers += next.readers;
        self.tags += next.tags;
    }
}

#[derive(Clone, Debug)]
pub struct TraceIndex {
    offsets: Checkpoints,
    rows: u64,
    bytes: u64,
    pub reader_frames: u64,
    pub tag_frames: u64,
    /// Consecutive source-order groups. Time values remain in the source clock's ticks.
    pub bins: Vec<TraceBin>,
    /// Number of frames represented by each complete overview bin.
    pub bin_frames: u64,
}

impl TraceIndex {
    pub fn rows(&self) -> u64 {
        self.rows
    }
}

pub(super) fn recognizes(prefix: &[u8], whole: bool) -> bool {
    // This headerless format needs substantial binary/record evidence before
    // taking an extension also used by text logs and other tools.
    if prefix.len() < 20
        || prefix
            .iter()
            .all(|byte| byte.is_ascii_graphic() || byte.is_ascii_whitespace())
    {
        return false;
    }
    let mut cursor = 0;
    let mut previous = None;
    let mut count = 0;
    while cursor < prefix.len() {
        let Some(header) = prefix
            .get(cursor..)
            .and_then(|rest| rest.first_chunk::<HEADER>())
        else {
            break;
        };
        let Ok(frame) = from_header(*header, count) else {
            return false;
        };
        if previous.is_some_and(|previous| frame.timestamp_ticks.wrapping_sub(previous) > (1 << 31))
        {
            return false;
        }
        let end = cursor + HEADER + frame.body_len();
        if end > prefix.len() {
            break;
        }
        previous = Some(frame.timestamp_ticks);
        cursor = end;
        count += 1;
    }
    count >= 2 && (!whole || cursor == prefix.len())
}

pub(super) fn preview(
    input: &mut (impl BufRead + Seek),
    cancel: &AtomicBool,
) -> Result<TracePreview, Error> {
    let bytes = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(0))?;
    let mut probe = vec![0; bytes.min(super::MAX_PROBE as u64) as usize];
    input.read_exact(&mut probe).map_err(read_error)?;
    if !recognizes(&probe, bytes == probe.len() as u64) {
        return Err(Error::Invalid("unrecognized Proxmark trace structure"));
    }
    let mut preview = Cursor::new(&probe);
    let mut position = 0;
    let mut frames = Vec::new();
    while frames.len() < PREVIEW_ROWS && position + HEADER as u64 <= probe.len() as u64 {
        cancelled(cancel)?;
        let (frame, end) = next_header(&mut preview, position, bytes, frames.len() as u64)?;
        if end > probe.len() as u64 {
            break;
        }
        preview.set_position(end);
        frames.push(frame);
        position = end;
    }
    Ok(TracePreview {
        frames,
        complete: position == bytes,
    })
}

pub fn build_trace_index(
    mut input: impl BufRead + Seek,
    cancel: &AtomicBool,
) -> Result<TraceIndex, Error> {
    let bytes = input.seek(SeekFrom::End(0))?;
    input.seek(SeekFrom::Start(0))?;
    let mut index = TraceIndex {
        offsets: Checkpoints::new(128),
        rows: 0,
        bytes,
        reader_frames: 0,
        tag_frames: 0,
        bins: Vec::new(),
        bin_frames: 1,
    };
    let mut position = 0;
    while position < bytes {
        cancelled(cancel)?;
        let (frame, end) = next_header(&mut input, position, bytes, index.rows)?;
        index.offsets.record(index.rows, position);
        index.push(frame);
        skip(&mut input, frame.body_len(), end)?;
        position = end;
    }
    if index.rows < 2 {
        return Err(Error::Invalid("trace contains too few recognizable frames"));
    }
    Ok(index)
}

impl TraceIndex {
    fn push(&mut self, frame: FrameInfo) {
        let next = TraceBin::new(frame);
        self.reader_frames += next.readers;
        self.tag_frames += next.tags;
        if self.rows.is_multiple_of(self.bin_frames) {
            if self.bins.len() == MAX_BINS {
                self.bin_frames *= 2;
                for slot in 0..MAX_BINS / 2 {
                    let mut merged = self.bins[slot * 2].clone();
                    merged.merge(&self.bins[slot * 2 + 1]);
                    self.bins[slot] = merged;
                }
                self.bins.truncate(MAX_BINS / 2);
            }
            self.bins.push(next);
        } else if let Some(last) = self.bins.last_mut() {
            last.merge(&next);
        }
        self.rows += 1;
    }
}

/// Reads exact frames with captured parity bytes from a revision-matched index.
pub fn read_frames(
    input: impl BufRead + Seek,
    index: &TraceIndex,
    rows: Range<u64>,
    cancel: &AtomicBool,
) -> Result<Vec<Frame>, Error> {
    let mut retained = 0;
    read_window(input, index, rows, cancel, |input, info, _end| {
        retained += info.body_len();
        if retained > MAX_PAYLOAD_WINDOW {
            return Err(Error::Limit("requested frame bytes"));
        }
        let mut data = vec![0; usize::from(info.data_len)];
        let mut parity = vec![0; info.parity_len()];
        input.read_exact(&mut data).map_err(read_error)?;
        input.read_exact(&mut parity).map_err(read_error)?;
        Ok(Frame { info, data, parity })
    })
}

pub fn read_frame_info(
    input: impl BufRead + Seek,
    index: &TraceIndex,
    rows: Range<u64>,
    cancel: &AtomicBool,
) -> Result<Vec<FrameInfo>, Error> {
    read_window(input, index, rows, cancel, |input, info, end| {
        skip(input, info.body_len(), end)?;
        Ok(info)
    })
}

fn read_window<R: BufRead + Seek, T>(
    mut input: R,
    index: &TraceIndex,
    rows: Range<u64>,
    cancel: &AtomicBool,
    mut collect: impl FnMut(&mut R, FrameInfo, u64) -> Result<T, Error>,
) -> Result<Vec<T>, Error> {
    let end = rows.end.min(index.rows);
    let count = end.saturating_sub(rows.start);
    if count > MAX_WINDOW {
        return Err(Error::Limit("requested frame rows"));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let [first, mut position] = index.offsets.before(rows.start)?;
    input.seek(SeekFrom::Start(position))?;
    let mut output = Vec::with_capacity(count as usize);
    for row in first..end {
        cancelled(cancel)?;
        let (info, next) = next_header(&mut input, position, index.bytes, row)?;
        if row >= rows.start {
            output.push(collect(&mut input, info, next)?);
        } else {
            skip(&mut input, info.body_len(), next)?;
        }
        position = next;
    }
    Ok(output)
}

fn next_header(
    input: &mut impl Read,
    position: u64,
    bytes: u64,
    row: u64,
) -> Result<(FrameInfo, u64), Error> {
    let mut header = [0; HEADER];
    input.read_exact(&mut header).map_err(read_error)?;
    let info = from_header(header, row)?;
    let next = position
        .checked_add((HEADER + info.body_len()) as u64)
        .ok_or(Error::Truncated)?;
    if next > bytes {
        return Err(Error::Truncated);
    }
    Ok((info, next))
}

fn from_header([a, b, c, d, e, f, g, h]: [u8; HEADER], row: u64) -> Result<FrameInfo, Error> {
    let length = u16::from_le_bytes([g, h]);
    let data_len = length & 0x7fff;
    if data_len == 0 {
        return Err(Error::Invalid("zero-length trace frame"));
    }
    Ok(FrameInfo {
        row,
        timestamp_ticks: u32::from_le_bytes([a, b, c, d]),
        duration_ticks: u16::from_le_bytes([e, f]),
        direction: if length & 0x8000 == 0 {
            Direction::Reader
        } else {
            Direction::Tag
        },
        data_len,
    })
}

fn skip(input: &mut (impl BufRead + Seek), amount: usize, end: u64) -> Result<(), Error> {
    if input.fill_buf()?.len() >= amount {
        input.consume(amount);
    } else {
        input.seek(SeekFrom::Start(end))?;
    }
    Ok(())
}

fn read_error(error: std::io::Error) -> Error {
    if error.kind() == ErrorKind::UnexpectedEof {
        Error::Truncated
    } else {
        Error::Io(error)
    }
}
