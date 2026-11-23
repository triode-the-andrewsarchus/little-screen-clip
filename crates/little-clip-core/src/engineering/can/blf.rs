//! Vector BLF containers and CAN object layouts.
//!
//! The layout was cross-checked against the Apache-2.0 autors-blf decoder
//! and MIT/Apache-2.0 ablf crate. The scanner keeps physical container
//! checkpoints so frame lookups need neither a full replay nor a temp file.

use std::{collections::VecDeque, io::Read, os::unix::fs::FileExt, sync::atomic::AtomicBool};

use flate2::read::ZlibDecoder;

use super::{
    Capture, Channel, Direction, Frame, FrameKind, Identifier, Location, Positioned, check_cancel,
};

const BASE: usize = 16;
const CONTAINER: u32 = 10;
const MAX_CONTAINER_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct Configuration {
    start: u64,
}

pub(super) fn header_size(prefix: &[u8]) -> Option<u64> {
    if prefix.len() < 8 || &prefix[..4] != b"LOGG" {
        return None;
    }
    let length = u32_at(prefix, 4);
    (72..=4096).contains(&length).then_some(u64::from(length))
}

impl Configuration {
    pub(super) fn open(prefix: &[u8], file_len: u64) -> Result<Self, String> {
        let start = header_size(prefix)
            .filter(|start| *start <= file_len)
            .ok_or("Invalid Vector BLF file header")?;
        Ok(Self { start })
    }
}

#[derive(Clone, Copy)]
struct Object {
    size: usize,
    header: usize,
    version: u16,
    kind: u32,
}

impl Object {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if &bytes[..4] != b"LOBJ" {
            return Err("Missing Vector BLF object signature".into());
        }
        let size = u32_at(bytes, 8) as usize;
        let header = usize::from(u16_at(bytes, 4));
        if size < BASE || !(BASE..=size).contains(&header) {
            return Err("Invalid Vector BLF object length".into());
        }
        Ok(Self {
            size,
            header,
            version: u16_at(bytes, 6),
            kind: u32_at(bytes, 12),
        })
    }

    fn is_can(self) -> bool {
        matches!(self.kind, 1 | 2 | 73 | 86 | 100 | 101)
    }
}

struct Segment {
    start: usize,
    end: usize,
    decoded_base: usize,
    container: u64,
}

pub(super) struct Scanner<'a> {
    capture: &'a Capture,
    outer_offset: u64,
    outer_end: u64,
    inner: Vec<u8>,
    segments: VecDeque<Segment>,
    pos: usize,
}

impl<'a> Scanner<'a> {
    pub(super) fn new(
        capture: &'a Capture,
        configuration: &Configuration,
        resume: Option<Location>,
        byte_limit: u64,
    ) -> Result<Self, String> {
        let (start, pos) = match resume {
            None => (configuration.start, 0),
            Some(Location::Blf { container, decoded }) => (container, decoded),
            _ => return Err("Invalid Vector BLF checkpoint".into()),
        };
        Ok(Self {
            capture,
            outer_offset: start,
            outer_end: capture.byte_len.min(start.saturating_add(byte_limit)),
            inner: Vec::new(),
            segments: VecDeque::new(),
            pos,
        })
    }

    pub(super) fn next(
        &mut self,
        bytes: bool,
        cancel: &AtomicBool,
    ) -> Result<Option<(Location, Frame)>, String> {
        // A checkpoint can begin partway into its first compressed container.
        if self.inner.is_empty() && self.pos != 0 {
            self.load(cancel)?;
            if self.pos >= self.inner.len() {
                return Err("Vector BLF checkpoint exceeds the container".into());
            }
        }
        loop {
            check_cancel(cancel)?;
            if !self.ensure(BASE, cancel)? {
                return Ok(None);
            }
            while self.inner[self.pos] == 0 {
                // Byte padding separates some object types inside containers.
                self.pos += 1;
                if !self.ensure(BASE, cancel)? {
                    return Ok(None);
                }
            }
            let object = Object::parse(&self.inner[self.pos..self.pos + BASE])?;
            if object.size > MAX_OBJECT_BYTES {
                return Err("Vector BLF object exceeds the read limit".into());
            }
            let location = self
                .location()
                .ok_or("Missing Vector BLF object location")?;
            if !self.ensure(object.size, cancel)? {
                return Ok(None);
            }
            let start = self.pos;
            self.pos += object.size;
            if object.is_can()
                && let Some(frame) = parse_frame(object, &self.inner[start..self.pos], bytes)?
            {
                return Ok(Some((location, frame)));
            }
        }
    }

    pub(super) fn covered_bytes(&self) -> u64 {
        self.outer_offset
    }

    fn location(&self) -> Option<Location> {
        let segment = self
            .segments
            .iter()
            .find(|segment| (segment.start..segment.end).contains(&self.pos))?;
        Some(Location::Blf {
            container: segment.container,
            decoded: segment.decoded_base + self.pos - segment.start,
        })
    }

    fn ensure(&mut self, count: usize, cancel: &AtomicBool) -> Result<bool, String> {
        while self.inner.len().saturating_sub(self.pos) < count {
            self.compact();
            if !self.load(cancel)? {
                if self.outer_end == self.capture.byte_len
                    && self.inner.iter().any(|byte| *byte != 0)
                {
                    return Err("Truncated Vector BLF object payload".into());
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn compact(&mut self) {
        let consumed = self.pos;
        if consumed == 0 {
            return;
        }
        self.inner.copy_within(consumed.., 0);
        self.inner.truncate(self.inner.len() - consumed);
        self.pos = 0;
        while self
            .segments
            .front()
            .is_some_and(|segment| segment.end <= consumed)
        {
            self.segments.pop_front();
        }
        for segment in &mut self.segments {
            segment.decoded_base += consumed.saturating_sub(segment.start);
            segment.start = segment.start.saturating_sub(consumed);
            segment.end -= consumed;
        }
    }

    fn load(&mut self, cancel: &AtomicBool) -> Result<bool, String> {
        while self.outer_offset < self.outer_end {
            check_cancel(cancel)?;
            let container = self.outer_offset;
            if (self.outer_end - container) < BASE as u64 {
                return Ok(false);
            }
            let header = self.array::<BASE>(container)?;
            let object = Object::parse(&header)?;
            let end = container + object.size as u64;
            if end > self.capture.byte_len {
                return Err("Vector BLF container is truncated".into());
            }
            if end > self.outer_end {
                self.outer_offset = self.outer_end;
                return Ok(false);
            }
            self.outer_offset = end;
            if object.kind == CONTAINER {
                self.outer_offset = (end + (object.size % 4) as u64).min(self.capture.byte_len);
                if let Some(extra) = self.decompress(container, object, cancel)? {
                    self.append(container, &extra);
                    return Ok(true);
                }
            } else if object.is_can() {
                if object.size > MAX_OBJECT_BYTES {
                    return Err("Vector BLF object exceeds the read limit".into());
                }
                let mut object_bytes = vec![0; object.size];
                self.read_at(container, &mut object_bytes)?;
                self.append(container, &object_bytes);
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn append(&mut self, container: u64, bytes: &[u8]) {
        let start = self.inner.len();
        self.inner.extend_from_slice(bytes);
        self.segments.push_back(Segment {
            container,
            start,
            end: self.inner.len(),
            decoded_base: 0,
        });
    }

    fn decompress(
        &self,
        offset: u64,
        object: Object,
        cancel: &AtomicBool,
    ) -> Result<Option<Vec<u8>>, String> {
        let start = object.header + BASE;
        if start > object.size {
            return Err("Incomplete Vector BLF log container header".into());
        }
        let header = self.array::<BASE>(offset + object.header as u64)?;
        let method = u16_at(&header, 0);
        let uncompressed = u32_at(&header, 8) as usize;
        let encoded = object.size - start;
        if uncompressed > MAX_CONTAINER_BYTES || encoded > MAX_CONTAINER_BYTES {
            return Err("Vector BLF container exceeds the decompression limit".into());
        }
        if uncompressed == 0 {
            return Ok(None);
        }
        let start = offset + start as u64;
        let mut out = vec![0; uncompressed];
        match method {
            0 if encoded == uncompressed => self.read_at(start, &mut out)?,
            2 => {
                let source = Positioned {
                    file: &self.capture.file,
                    offset: start,
                    end: start + encoded as u64,
                };
                let mut decoder = ZlibDecoder::new(source);
                for block in out.chunks_mut(65536) {
                    check_cancel(cancel)?;
                    decoder
                        .read_exact(block)
                        .map_err(|error| format!("Decode Vector BLF container: {error}"))?;
                }
                if decoder.read(&mut [0]).map_err(|error| error.to_string())? != 0 {
                    return Err("Vector BLF decompressed length differs from its header".into());
                }
            }
            _ => return Err("Unsupported Vector BLF container compression or length".into()),
        }
        Ok(Some(out))
    }

    fn array<const N: usize>(&self, offset: u64) -> Result<[u8; N], String> {
        let mut bytes = [0; N];
        self.read_at(offset, &mut bytes)?;
        Ok(bytes)
    }

    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.capture
            .file
            .read_exact_at(bytes, offset)
            .map_err(|error| format!("Read Vector BLF file at {offset}: {error}"))
    }
}

fn parse_frame(object: Object, bytes: &[u8], retain: bool) -> Result<Option<Frame>, String> {
    let minimum_header = match object.version {
        1 => 32,
        2 => 40,
        _ => return Ok(None),
    };
    if object.header < minimum_header {
        return Err("Truncated Vector BLF timestamp header".into());
    }
    let flags = u32_at(bytes, 16);
    let raw = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let time = i128::from(raw) * if flags & 1 != 0 { 10_000 } else { 1 };
    let data = &bytes[object.header..];
    let minimum_data = match object.kind {
        1 | 86 => 8,
        2 => 2,
        73 => 24,
        100 => 20,
        101 => 40,
        _ => return Ok(None),
    };
    if data.len() < minimum_data {
        return Err("Truncated CAN frame in Vector BLF file".into());
    }
    let frame = match object.kind {
        1 | 86 => classic(time, data, retain),
        2 => Frame::new(time, Channel::Number(u16_at(data, 0)), FrameKind::Error),
        73 => extended_error(time, data, retain),
        100 => fd(time, data, retain),
        101 => fd64(time, data, object.header, retain),
        _ => unreachable!(),
    };
    Ok(Some(frame))
}

fn classic(time: i128, data: &[u8], retain: bool) -> Frame {
    let flags = data[2];
    let kind = if flags & 0x80 != 0 {
        FrameKind::Remote
    } else {
        FrameKind::Classic
    };
    let mut frame = Frame::new(time, Channel::Number(u16_at(data, 0)), kind);
    frame.id = identifier(u32_at(data, 4));
    frame.direction = direction(flags & 1 != 0);
    frame.dlc = Some(u16::from(data[3]));
    if kind != FrameKind::Remote {
        set_data(&mut frame, &data[8..], u16::from(data[3].min(8)), retain);
    }
    frame
}

fn extended_error(time: i128, data: &[u8], retain: bool) -> Frame {
    let mut frame = Frame::new(time, Channel::Number(u16_at(data, 0)), FrameKind::Error);
    frame.id = identifier(u32_at(data, 16));
    frame.dlc = Some(u16::from(data[10]));
    let size = u16::from(data[10].min(8));
    set_data(&mut frame, &data[24..], size, retain);
    frame
}

fn fd(time: i128, data: &[u8], retain: bool) -> Frame {
    let flags = data[13];
    let kind = if data[2] & 0x80 != 0 {
        FrameKind::Remote
    } else if flags & 1 != 0 {
        FrameKind::Flexible {
            bitrate_switch: flags & 2 != 0,
            error_state: flags & 4 != 0,
        }
    } else {
        FrameKind::Classic
    };
    let mut frame = Frame::new(time, Channel::Number(u16_at(data, 0)), kind);
    frame.id = identifier(u32_at(data, 4));
    frame.direction = direction(data[2] & 1 != 0);
    frame.dlc = Some(u16::from(data[3]));
    if kind != FrameKind::Remote {
        set_data(&mut frame, &data[20..], u16::from(data[14].min(64)), retain);
    }
    frame
}

fn fd64(time: i128, data: &[u8], header: usize, retain: bool) -> Frame {
    let flags = u32_at(data, 12);
    let kind = if flags & 0x0010 != 0 {
        FrameKind::Remote
    } else if flags & 0x1000 != 0 {
        FrameKind::Flexible {
            bitrate_switch: flags & 0x2000 != 0,
            error_state: flags & 0x4000 != 0,
        }
    } else {
        FrameKind::Classic
    };
    let mut frame = Frame::new(time, Channel::Number(u16::from(data[0])), kind);
    frame.id = identifier(u32_at(data, 4));
    frame.direction = direction(data[34] != 0);
    frame.dlc = Some(u16::from(data[1]));
    if kind != FrameKind::Remote {
        let end = if data[35] == 0 {
            data.len()
        } else {
            usize::from(data[35])
                .saturating_sub(header)
                .clamp(40, data.len())
        };
        set_data(
            &mut frame,
            &data[40..end],
            u16::from(data[2].min(64)),
            retain,
        );
    }
    frame
}

fn set_data(frame: &mut Frame, data: &[u8], length: u16, retain: bool) {
    frame.data_len = length;
    if retain {
        frame
            .data
            .extend_from_slice(&data[..usize::from(length).min(data.len())]);
    }
}

fn identifier(raw: u32) -> Option<Identifier> {
    Identifier::new(raw & 0x1fff_ffff, raw & 0x8000_0000 != 0)
}

fn direction(tx: bool) -> Direction {
    if tx { Direction::Tx } else { Direction::Rx }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
