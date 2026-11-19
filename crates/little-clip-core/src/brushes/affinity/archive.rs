use flate2::Crc;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_ENTRIES: usize = 8192;
const MAX_NAME_BYTES: usize = 128;
const INDEX_HEADER_BYTES: usize = 59;
const RECORD_BYTES: usize = 42;

pub(super) struct Archive<'a> {
    entries: BTreeMap<&'a [u8], Entry<'a>>,
    decoder: zstd::bulk::Decompressor<'static>,
}

struct Entry<'a> {
    data: &'a [u8],
    size: usize,
    codec: u8,
    checksum: u32,
    packed_checksum: u32,
}

impl<'a> Archive<'a> {
    pub(super) fn read(bytes: &'a [u8], cancelled: &AtomicBool) -> Result<Self, &'static str> {
        let (payload, mut table, count) = read_index(bytes)?;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Cancelled");
            }
            let (name, entry) =
                read_entry(&mut table, payload).ok_or("Invalid Affinity brush index entry")?;
            if entries.insert(name, entry).is_some() {
                return Err("Duplicate Affinity brush index entry");
            }
        }
        let mut decoder = zstd::bulk::Decompressor::new()
            .map_err(|_| "Cannot initialize Affinity brush decompression")?;
        decoder
            .window_log_max(22)
            .map_err(|_| "Cannot limit Affinity brush decompression")?;
        Ok(Self { entries, decoder })
    }

    pub(super) fn unpack(
        &mut self,
        name: &[u8],
        max_size: usize,
        output: &mut Vec<u8>,
    ) -> Result<(), &'static str> {
        let entry = self
            .entries
            .get(name)
            .ok_or("Missing Affinity brush resource")?;
        if entry.size > max_size || entry.data.len() > max_size.saturating_add(64 * 1024) {
            return Err("Affinity brush resource exceeds its size limit");
        }
        if crc(entry.data) != entry.packed_checksum {
            return Err("Invalid Affinity brush resource checksum");
        }
        output.clear();
        match entry.codec {
            0 => output.extend_from_slice(entry.data),
            2 | 0x42 => {
                output.reserve(entry.size);
                self.decoder
                    .decompress_to_buffer(entry.data, output)
                    .map_err(|_| "Invalid Affinity brush compressed data")?;
            }
            _ => return Err("Unsupported Affinity brush compression"),
        }
        if output.len() != entry.size {
            return Err("Incorrect Affinity brush resource size");
        }
        if entry.codec == 0x42 {
            let mut last = 0_u8;
            for byte in output.iter_mut() {
                last = last.wrapping_add(*byte);
                *byte = last;
            }
        }
        if crc(output) != entry.checksum {
            return Err("Invalid Affinity brush decoded checksum");
        }
        Ok(())
    }
}

fn read_index(bytes: &[u8]) -> Result<(&[u8], &[u8], usize), &'static str> {
    if le_u16(bytes, 4) != Some(11) || bytes.get(12..16) != Some(b"#Inf") {
        return Err("Unsupported Affinity brush container");
    }
    let index = le_offset(bytes, 16).ok_or("Truncated Affinity brush header")?;
    let (payload, fat) = bytes
        .split_at_checked(index)
        .ok_or("Missing Affinity brush index")?;
    if !fat.starts_with(b"#FT4") || le_offset(fat, 4) != Some(0) {
        return Err("Unsupported Affinity brush index");
    }
    let count = le_u32(fat, 44).ok_or("Truncated Affinity brush index")? as usize;
    if !(1..=MAX_ENTRIES).contains(&count) {
        return Err("Affinity brush index exceeds its entry limit");
    }
    let table_len = le_u32(fat, 52).ok_or("Truncated Affinity brush index")? as usize;
    let table = fat
        .get(INDEX_HEADER_BYTES..)
        .and_then(|data| data.get(..table_len))
        .ok_or("Truncated Affinity brush index")?;
    Ok((payload, table, count))
}

fn read_entry<'a>(table: &mut &'a [u8], payload: &'a [u8]) -> Option<(&'a [u8], Entry<'a>)> {
    let (record, rest) = table.split_at_checked(RECORD_BYTES)?;
    // Named entries in the current save point.
    if record[4] != 0 {
        return None;
    }
    let offset = le_offset(record, 5)?;
    let size = le_offset(record, 13)?;
    let packed_size = le_offset(record, 21)?;
    let block = payload.get(offset..)?.strip_prefix(b"#Fil")?;
    let data = block.get(..packed_size)?;
    let name_len = usize::from(le_u16(rest, 0)?);
    if !(1..=MAX_NAME_BYTES).contains(&name_len) {
        return None;
    }
    let (name, remaining) = rest[2..].split_at_checked(name_len)?;
    *table = remaining;
    Some((
        name,
        Entry {
            data,
            size,
            codec: record[33],
            checksum: le_u32(record, 29)?,
            packed_checksum: le_u32(record, 38)?,
        },
    ))
}

pub(super) fn le_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(offset..)?.first_chunk()?))
}

fn le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*bytes.get(offset..)?.first_chunk()?))
}

fn le_offset(bytes: &[u8], offset: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(*bytes.get(offset..)?.first_chunk()?)).ok()
}

pub(super) fn crc(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum()
}
