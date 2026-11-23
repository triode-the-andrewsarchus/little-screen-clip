use super::{Reader, u16_at, u32_at, u64_at};

const CHUNK_HEADER_BYTES: u64 = 32;
const TABLE_HEADER_BYTES: u64 = 16;
const CRC_BYTES: u64 = 4;
const ALIGNMENT: u64 = 8;

pub(super) fn check_header(reader: &Reader) -> Result<(), String> {
    let header = reader.array::<32>(0)?;
    if header[27] != 1 || checksum(&header[..28]) != u32_at(&header, 28) {
        return Err("Unsupported or damaged Joulescope v2 file header".into());
    }
    let length = u64_at(&header, 16);
    if length != 0 && length != reader.len {
        return Err("Joulescope file does not match its recorded length".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Chunk {
    offset: u64,
    pub(super) tag: u8,
    pub(super) meta: u16,
    pub(super) next: u64,
    payload_bytes: u64,
    pub(super) end: u64,
}

impl Chunk {
    pub(super) fn read(reader: &Reader, offset: u64) -> Result<Self, String> {
        if offset < 32 || !offset.is_multiple_of(ALIGNMENT) {
            return Err("Invalid Joulescope chunk offset".into());
        }
        let bytes = reader.array::<32>(offset)?;
        if checksum(&bytes[..28]) != u32_at(&bytes, 28) {
            return Err(format!(
                "Invalid Joulescope chunk header checksum at {offset}"
            ));
        }
        let payload_bytes = u64::from(u32_at(&bytes, 20));
        let padded = if payload_bytes == 0 {
            0
        } else {
            (payload_bytes + CRC_BYTES).next_multiple_of(ALIGNMENT)
        };
        let end = offset
            .checked_add(CHUNK_HEADER_BYTES + padded)
            .filter(|end| *end <= reader.len)
            .ok_or("Joulescope chunk extends beyond the recording")?;
        Ok(Self {
            offset,
            tag: bytes[16],
            meta: u16_at(&bytes, 18),
            next: u64_at(&bytes, 0),
            payload_bytes,
            end,
        })
    }

    pub(super) fn forward(self) -> Result<u64, String> {
        if self.next != 0 && self.next < self.end {
            return Err("Invalid Joulescope chunk chain".into());
        }
        Ok(self.next)
    }

    pub(super) fn payload(self, reader: &Reader, limit: usize) -> Result<Vec<u8>, String> {
        if self.payload_bytes > limit as u64 {
            return Err("Joulescope metadata exceeds its read limit".into());
        }
        let bytes = reader.bytes(
            self.offset + CHUNK_HEADER_BYTES,
            self.payload_bytes as usize,
        )?;
        if !bytes.is_empty()
            && checksum(&bytes) != u32::from_le_bytes(reader.array(self.end - CRC_BYTES)?)
        {
            return Err(format!(
                "Invalid Joulescope payload checksum at {}",
                self.offset
            ));
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Table {
    pub(super) chunk: Chunk,
    pub(super) sample: i64,
    pub(super) count: u64,
    pub(super) bits: u16,
    pub(super) data_offset: u64,
    pub(super) data_bytes: u64,
}

impl Table {
    pub(super) fn read(reader: &Reader, offset: u64, tag: u8, meta: u16) -> Result<Self, String> {
        let chunk = Chunk::read(reader, offset)?;
        if chunk.tag != tag || chunk.meta != meta || chunk.payload_bytes < TABLE_HEADER_BYTES {
            return Err(format!("Unexpected Joulescope table at {offset}"));
        }
        let bytes = reader.array::<16>(offset + CHUNK_HEADER_BYTES)?;
        Ok(Self {
            chunk,
            sample: i64::from_le_bytes(bytes[..8].try_into().unwrap()),
            count: u64::from(u32_at(&bytes, 8)),
            bits: u16_at(&bytes, 12),
            data_offset: offset + CHUNK_HEADER_BYTES + TABLE_HEADER_BYTES,
            data_bytes: chunk.payload_bytes - TABLE_HEADER_BYTES,
        })
    }

    pub(super) fn require_bytes(&self, required: u64) -> Result<(), String> {
        if required > self.data_bytes {
            return Err("Truncated Joulescope table".into());
        }
        Ok(())
    }

    pub(super) fn offset_at(&self, reader: &Reader, index: u64) -> Result<u64, String> {
        if index >= self.count || self.bits != 64 {
            return Err("Invalid Joulescope index entry".into());
        }
        reader
            .array(self.data_offset + index * 8)
            .map(u64::from_le_bytes)
    }
}

const fn crc_table(polynomial: u32) -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < table.len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { polynomial } else { 0 };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

pub(super) fn checksum(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc_table(0x82f6_3b78);
    !bytes.iter().fold(u32::MAX, |crc, &byte| {
        TABLE[((crc as u8) ^ byte) as usize] ^ (crc >> 8)
    })
}
