//! Saleae's native digital channel headers and independently encoded RLE blocks.
//!
//! The original channel block and state lookup layout follows the vendor discussion
//! and is verified against saved Logic 2 captures. Later layouts are documented by
//! sigrok2sal and checked against native member version 2 captures.
//! https://discuss.saleae.com/t/logic-2-capture-format-sal/1858
//! https://github.com/nemanjan00/sigrok2sal/blob/master/SAL_FORMAT.md

use super::{Level, archive::Member};
use std::{ops::Range, sync::atomic::AtomicBool};

const MAX_CHUNKS: u64 = 65_536;
const MAX_CHUNK_BYTES: u64 = 4 << 20;

#[derive(Clone, Copy, Debug)]
pub(super) struct Header {
    version: u32,
    pub(super) rate: f64,
    pub(super) count: u64,
    pub(super) first: u64,
    pub(super) start: Option<u64>,
    pub(super) end: Option<u64>,
}

impl Header {
    pub(super) fn read(member: &Member, cancel: &AtomicBool) -> Result<Self, String> {
        let mut bytes = [0; 67];
        let prefix = &mut bytes[..member.size.min(67) as usize];
        let mut remaining = 67;
        if !member.read_exact_at(prefix, 0, &mut remaining, cancel)? || prefix.len() < 51 {
            return Err("Saleae native channel header is truncated.".into());
        }
        if !prefix.starts_with(b"<SALEAE>") || le_u32(prefix, 12) != 100 || prefix[16] != 1 {
            return Err("Not a supported Saleae native digital channel.".into());
        }
        let version = le_u32(prefix, 8);
        let rate = le_f64(prefix, 17);
        if !matches!(version, 1..=3) || !rate.is_finite() || rate <= 0.0 {
            return Err("Saleae native channel version or rate is unsupported.".into());
        }
        let mut offset = 41;
        let start = optional_index(prefix, &mut offset)?;
        let end = optional_index(prefix, &mut offset)?;
        let count = prefix
            .get(offset..offset + 8)
            .map(|bytes| le_u64(bytes, 0))
            .ok_or("Saleae native channel header lacks a chunk count.")?;
        if !(1..=MAX_CHUNKS).contains(&count)
            || start.zip(end).is_some_and(|(start, end)| start >= end)
        {
            return Err("Saleae native channel has inconsistent bounds.".into());
        }
        Ok(Self {
            version,
            rate,
            count,
            first: (offset + 8) as u64,
            start,
            end,
        })
    }

    pub(super) fn read_chunk(
        self,
        member: &Member,
        offset: u64,
        budget: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<Option<Chunk>, String> {
        if self.version == 1 {
            read_legacy_chunk(member, self.rate, offset, budget, cancel)
        } else {
            read_current_chunk(member, offset, budget, cancel)
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Chunk {
    pub(super) ticks: Range<u64>,
    payload: Range<u64>,
    pub(super) next: u64,
    first: Level,
}

impl Chunk {
    pub(super) fn read(
        &self,
        member: &Member,
        budget: &mut u64,
        cancel: &AtomicBool,
    ) -> Result<Option<Runs>, String> {
        let mut bytes = vec![0; (self.payload.end - self.payload.start) as usize];
        if !member.read_exact_at(&mut bytes, self.payload.start, budget, cancel)? {
            return Ok(None);
        }
        Ok(Some(Runs {
            bytes,
            cursor: 0,
            sample: self.ticks.start,
            sample_end: self.ticks.end,
            next: self.first,
        }))
    }
}

pub(super) struct Runs {
    bytes: Vec<u8>,
    cursor: usize,
    sample: u64,
    sample_end: u64,
    next: Level,
}

impl Runs {
    pub(super) fn next(&mut self) -> Result<Option<(Range<u64>, Level)>, String> {
        if self.cursor == self.bytes.len() {
            if self.sample == self.sample_end {
                return Ok(None);
            }
            return Err("Saleae RLE runs do not fill the declared interval.".into());
        }
        let length = read_run(&self.bytes, &mut self.cursor)?;
        let end = self
            .sample
            .checked_add(length)
            .filter(|end| *end <= self.sample_end)
            .ok_or("Saleae RLE exceeds its chunk's sample count.")?;
        let value = self.next;
        self.next = match self.next {
            Level::Low => Level::High,
            _ => Level::Low,
        };
        let interval = self.sample..end;
        self.sample = end;
        Ok(Some((interval, value)))
    }
}

fn read_run(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let first = *bytes.get(*cursor).ok_or("Missing Saleae run length.")?;
    *cursor += 1;
    if first < 0x40 {
        return Ok(u64::from(first) + 1);
    }
    if first >= 0x80 {
        return Err("Unsupported Saleae digital run encoding.".into());
    }
    let mut run = u64::from(first & 0x3f);
    for _ in 0..9 {
        let byte = *bytes.get(*cursor).ok_or("Truncated Saleae run length.")?;
        *cursor += 1;
        run = run
            .checked_mul(128)
            .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
            .ok_or("Saleae run length overflows.")?;
        if byte < 0x80 {
            return run
                .checked_add(1)
                .ok_or_else(|| "Saleae run length overflows.".into());
        }
    }
    Err("Overlong Saleae run length.".into())
}

fn read_current_chunk(
    member: &Member,
    offset: u64,
    budget: &mut u64,
    cancel: &AtomicBool,
) -> Result<Option<Chunk>, String> {
    let mut header = [0; 26];
    if !member.read_exact_at(&mut header, offset, budget, cancel)? {
        return Ok(None);
    }
    let ticks = valid_ticks(&header)?;
    let flags = le_u64(&header, 16);
    let first = level(flags & 0xffff)?;
    if header[24..26] != [0, 0] {
        return Err("Unsupported Saleae digital block flags.".into());
    }
    let payload = payload(member, offset + 26, flags >> 16)?;
    let next = payload.end;
    Ok(Some(Chunk {
        ticks,
        payload,
        next,
        first,
    }))
}

fn read_legacy_chunk(
    member: &Member,
    rate: f64,
    offset: u64,
    budget: &mut u64,
    cancel: &AtomicBool,
) -> Result<Option<Chunk>, String> {
    let mut header = [0; 48];
    if !member.read_exact_at(&mut header, offset, budget, cancel)? {
        return Ok(None);
    }
    let ticks = valid_ticks(&header)?;
    if ticks.end - ticks.start != le_u64(&header, 16) || le_u64(&header, 24) as f64 != rate {
        return Err("Saleae legacy digital block has inconsistent timing.".into());
    }
    let payload = payload(member, offset + 48, le_u64(&header, 40))?;
    let mut first_entry = [0; 28];
    if !member.read_exact_at(&mut first_entry, payload.end, budget, cancel)? {
        return Ok(None);
    }
    let entries = le_u64(&first_entry, 0);
    let next = entries
        .checked_mul(20)
        .and_then(|size| size.checked_add(payload.end + 8))
        .filter(|next| entries > 0 && *next <= member.size)
        .ok_or("Saleae legacy digital lookup table is truncated.")?;
    if first_entry[8..24] != [0; 16] {
        return Err("Saleae legacy digital block lacks its initial state.".into());
    }
    let first = level(u64::from(le_u32(&first_entry, 24)))?;
    Ok(Some(Chunk {
        ticks,
        payload,
        next,
        first,
    }))
}

fn payload(member: &Member, start: u64, length: u64) -> Result<Range<u64>, String> {
    let end = start
        .checked_add(length)
        .filter(|end| (1..=MAX_CHUNK_BYTES).contains(&length) && *end <= member.size)
        .ok_or("Saleae digital block is too large or truncated.")?;
    Ok(start..end)
}

fn valid_ticks(header: &[u8]) -> Result<Range<u64>, String> {
    let start = le_u64(header, 0);
    let end = le_u64(header, 8);
    if start >= end || end > 1 << 53 {
        return Err("Saleae digital block has invalid sample bounds.".into());
    }
    Ok(start..end)
}

fn level(value: u64) -> Result<Level, String> {
    match value {
        0 => Ok(Level::Low),
        1 => Ok(Level::High),
        _ => Err("Saleae digital block has an invalid initial state.".into()),
    }
}

fn optional_index(bytes: &[u8], offset: &mut usize) -> Result<Option<u64>, String> {
    let present = bytes
        .get(*offset)
        .copied()
        .ok_or("Truncated Saleae channel bounds.")?;
    *offset += 1;
    match present {
        0 => Ok(None),
        1 => {
            let index = bytes
                .get(*offset..*offset + 8)
                .map(|bytes| le_u64(bytes, 0))
                .filter(|index| *index <= 1 << 53)
                .ok_or("Invalid Saleae channel bounds.")?;
            *offset += 8;
            Ok(Some(index))
        }
        _ => Err("Unsupported Saleae channel bounds marker.".into()),
    }
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
