use super::*;

#[derive(Clone, Copy)]
pub(super) struct Geometry {
    pub rows: usize,
    pub width: usize,
    pub protocol: &'static str,
}

impl Geometry {
    pub fn classic(kind: &str) -> Option<Self> {
        let rows = match kind.to_ascii_lowercase().as_str() {
            "mini" | "mifare mini" => 20,
            "1k" | "1 kb" => 64,
            "2k" | "2 kb" => 128,
            "4k" | "4 kb" => 256,
            _ => return None,
        };
        Self::classic_rows(rows)
    }

    fn classic_rows(rows: usize) -> Option<Self> {
        let protocol = match rows {
            20 => "MIFARE Mini",
            64 => "MIFARE Classic 1K",
            128 => "MIFARE Classic-compatible 2K",
            256 => "MIFARE Classic 4K",
            _ => return None,
        };
        Some(Self {
            rows,
            width: 16,
            protocol,
        })
    }

    pub fn row(self, address: u32, bytes: Vec<Option<u8>>) -> MemoryRow {
        let (sector, role, name) = if self.width == 16 {
            let (sector, trailer) = sector_at(address);
            let role = if address == 0 {
                RowRole::Manufacturer
            } else if trailer {
                RowRole::SectorTrailer
            } else {
                RowRole::Data
            };
            (Some(sector), role, format!("Block {address}"))
        } else {
            let role = if address < 3 {
                RowRole::Manufacturer
            } else {
                RowRole::Data
            };
            (None, role, format!("Page {address}"))
        };
        MemoryRow {
            address,
            name,
            sector,
            role,
            bytes,
        }
    }
}

fn sector_at(address: u32) -> (u16, bool) {
    if address < 128 {
        ((address / 4) as u16, address % 4 == 3)
    } else {
        ((32 + (address - 128) / 16) as u16, address % 16 == 15)
    }
}

pub(super) fn raw_geometry(bytes: &[u8]) -> Option<Geometry> {
    let (rows, tail) = bytes.as_chunks::<16>();
    if !tail.is_empty() {
        return None;
    }
    Geometry::classic_rows(rows.len())
}

pub(super) fn raw(bytes: &[u8]) -> Result<Memory, Error> {
    let geometry =
        raw_geometry(bytes).ok_or(Error::Invalid("expected a supported MIFARE dump length"))?;
    let rows = bytes
        .as_chunks::<16>()
        .0
        .iter()
        .enumerate()
        .map(|(index, bytes)| geometry.row(index as u32, bytes.iter().copied().map(Some).collect()))
        .collect();
    Ok(make_memory(Format::MifareDump, geometry, rows))
}

pub(super) fn probe_hex(bytes: &[u8]) -> bool {
    parse_hex_rows(bytes).is_ok()
}

pub(super) fn hex(bytes: &[u8]) -> Result<Memory, Error> {
    let ParsedHex { geometry, data } = parse_hex_rows(bytes)?;
    let rows = data
        .into_iter()
        .enumerate()
        .map(|(index, bytes)| geometry.row(index as u32, bytes))
        .collect();
    Ok(make_memory(Format::ProxmarkDump, geometry, rows))
}

struct ParsedHex {
    geometry: Geometry,
    data: Vec<Vec<Option<u8>>>,
}

fn parse_hex_rows(bytes: &[u8]) -> Result<ParsedHex, Error> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Invalid("expected UTF-8 hexadecimal rows"))?;
    let mut rows = Vec::new();
    let mut width = None;
    for line in text.trim_start_matches('\u{feff}').lines().map(str::trim) {
        if line.is_empty() || line.starts_with([';', '#']) {
            continue;
        }
        if line.len() > 64 || rows.len() >= 256 {
            return Err(Error::Limit("Proxmark memory geometry"));
        }
        let bytes = hex_bytes(line)?;
        let length = bytes.len();
        if !matches!(length, 4 | 16) || width.is_some_and(|width| width != length) {
            return Err(Error::Invalid("hex row geometry"));
        }
        width = Some(length);
        rows.push(bytes);
    }
    let geometry = match width {
        Some(16) => Geometry::classic_rows(rows.len()),
        Some(4) if matches!(rows.len(), 16 | 20 | 41 | 45 | 48 | 135 | 231) => Some(Geometry {
            rows: rows.len(),
            width: 4,
            protocol: "Type 2 tag memory",
        }),
        _ => None,
    }
    .ok_or(Error::Invalid("unrecognized tag memory geometry"))?;
    Ok(ParsedHex {
        geometry,
        data: rows,
    })
}

fn make_memory(format: Format, geometry: Geometry, rows: Vec<MemoryRow>) -> Memory {
    let identifier = if geometry.width == 16 {
        rows.first()
            .and_then(|row| four_byte_identifier(&row.bytes))
            .into_iter()
            .flatten()
            .map(Some)
            .collect()
    } else {
        Vec::new()
    };
    Memory {
        format,
        version: None,
        protocol: geometry.protocol.into(),
        identifier,
        rows,
        fields: vec![("Bytes per row".into(), geometry.width.to_string())],
        rom_check: None,
    }
}

fn four_byte_identifier(bytes: &[Option<u8>]) -> Option<[u8; 4]> {
    let [a, b, c, d, check, ..] = bytes else {
        return None;
    };
    let ([a, b, c, d], check) = (
        [
            a.as_ref().copied()?,
            b.as_ref().copied()?,
            c.as_ref().copied()?,
            d.as_ref().copied()?,
        ],
        (*check)?,
    );
    (a ^ b ^ c ^ d == check).then_some([a, b, c, d])
}
