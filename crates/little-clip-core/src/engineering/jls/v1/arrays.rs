use std::ops::Range;

use super::{
    super::{Reader, u32_at},
    raw::Chunk,
};

#[derive(Debug, Clone, Copy)]
struct Field {
    offset: u64,
    count: u64,
    id: u8,
    kind: Kind,
}

pub(super) struct Arrays {
    pub(super) count: u64,
    fields: Vec<Field>,
}

impl Arrays {
    pub(super) fn open(reader: &Reader, chunk: Chunk) -> Result<Self, String> {
        if !chunk.is_plain() || chunk.len < 16 {
            return Err("Invalid legacy Joulescope sample arrays".into());
        }
        let header = reader.array::<16>(chunk.payload)?;
        if &header[..4] != b"ARRY" || u64::from(u32_at(&header, 4)) != chunk.len {
            return Err("Invalid legacy Joulescope array header".into());
        }
        let array_count = usize::from(header[15]);
        let mut offset = u64::from(u32_at(&header, 8));
        if offset < 16 + array_count as u64 * 8 || offset > chunk.len {
            return Err("Invalid legacy Joulescope array directory".into());
        }
        let tags = reader.bytes(chunk.payload + 16, array_count * 8)?;
        let mut fields = Vec::with_capacity(array_count);
        for tag in tags.as_chunks::<8>().0 {
            let len = u64::from(u32_at(tag, 4) & 0x00ff_ffff);
            let end = offset + len.next_multiple_of(8);
            if end > chunk.len {
                return Err("Joulescope array extends beyond its sample chunk".into());
            }
            if let Some(kind) = Kind::parse(tag[0]) {
                let width = kind.width() as u64;
                if tag[3] == 0 && len.is_multiple_of(width) {
                    fields.push(Field {
                        offset: chunk.payload + offset,
                        count: len / width,
                        id: tag[1],
                        kind,
                    });
                }
            }
            offset = end;
        }
        Ok(Self {
            count: u64::from(u32_at(&header, 12) & 0x00ff_ffff),
            fields,
        })
    }

    pub(super) fn has(&self, id: u8) -> bool {
        self.fields.iter().any(|field| field.id == id)
    }

    pub(super) fn read(
        &self,
        reader: &Reader,
        id: u8,
        range: Range<u64>,
    ) -> Result<Vec<f32>, String> {
        let field = self
            .fields
            .iter()
            .find(|field| field.id == id)
            .ok_or("Missing legacy Joulescope waveform field")?;
        if range.end > field.count {
            return Err("Legacy Joulescope sample window extends beyond its array".into());
        }
        let width = field.kind.width();
        let bytes = reader.bytes(
            field.offset + range.start * width as u64,
            (range.end - range.start) as usize * width,
        )?;
        Ok(bytes
            .chunks_exact(width)
            .map(|entry| field.kind.decode(entry))
            .collect())
    }
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Float32,
    Float64,
    Signed(u8),
    Unsigned(u8),
}

impl Kind {
    fn parse(value: u8) -> Option<Self> {
        let width = 1 << (value & 3);
        match value {
            2 => Some(Self::Float32),
            3 => Some(Self::Float64),
            8..=11 => Some(Self::Unsigned(width)),
            12..=15 => Some(Self::Signed(width)),
            _ => None,
        }
    }

    fn width(self) -> usize {
        match self {
            Self::Float32 => 4,
            Self::Float64 => 8,
            Self::Signed(width) | Self::Unsigned(width) => usize::from(width),
        }
    }

    fn decode(self, entry: &[u8]) -> f32 {
        match self {
            Self::Float32 => f32::from_le_bytes(entry.try_into().unwrap()),
            Self::Float64 => f64::from_le_bytes(entry.try_into().unwrap()) as f32,
            Self::Signed(_) | Self::Unsigned(_) => {
                let mut padded = [0; 8];
                padded[..entry.len()].copy_from_slice(entry);
                let value = u64::from_le_bytes(padded);
                if matches!(self, Self::Signed(_)) {
                    let shift = (8 - entry.len()) * 8;
                    (((value << shift) as i64) >> shift) as f32
                } else {
                    value as f32
                }
            }
        }
    }
}
