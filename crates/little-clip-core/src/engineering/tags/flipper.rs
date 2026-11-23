use super::{memory::Geometry, source::cancelled, *};
use std::collections::BTreeMap;

const MAX_ROWS: usize = 4096;
const MAX_ROW_BYTES: usize = 64;
const MAX_FIELDS: usize = 128;

pub(super) fn identify(bytes: &[u8]) -> Result<(Format, u32), Error> {
    let text = String::from_utf8_lossy(bytes);
    let mut fields = fields(&text);
    let (key, value) = fields.next().ok_or(Error::Truncated)?;
    if key != "Filetype" {
        return Err(Error::Invalid("missing Flipper Filetype header"));
    }
    let format = match value {
        "Flipper NFC device" => Format::FlipperNfc,
        "Flipper RFID key" => Format::FlipperRfid,
        "Flipper iButton key" => Format::FlipperIButton,
        _ => return Err(Error::Unsupported("Flipper document header")),
    };
    let (key, value) = fields.next().ok_or(Error::Truncated)?;
    if key != "Version" {
        return Err(Error::Invalid("missing Flipper Version header"));
    }
    let version = decimal(value)?;
    let supported = match format {
        Format::FlipperNfc => version <= 4,
        Format::FlipperRfid => version == 1,
        Format::FlipperIButton => version <= 2,
        _ => false,
    };
    if version == 0 || !supported {
        return Err(Error::Unsupported("Flipper document version"));
    }
    Ok((format, version))
}

fn fields(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(':'))
        .map(|(field, value)| (field.trim(), value.trim()))
}

pub(super) fn parse(bytes: &[u8], format: Format, cancel: &AtomicBool) -> Result<Memory, Error> {
    let (identified, version) = identify(bytes)?;
    if identified != format {
        return Err(Error::Invalid(
            "Flipper header does not match the file extension",
        ));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| Error::Invalid("Flipper document is not UTF-8"))?;
    let fields = Fields::new(text);
    match format {
        Format::FlipperNfc => fields.nfc(version, cancel),
        Format::FlipperRfid => fields.lf_rfid(version),
        Format::FlipperIButton => fields.ibutton(version),
        _ => Err(Error::Unsupported("Flipper document header")),
    }
}

struct Fields<'a> {
    entries: Vec<(&'a str, &'a str)>,
    memory: Memory,
}

impl<'a> Fields<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            entries: fields(text).skip(2).collect(),
            memory: Memory {
                format: Format::FlipperNfc,
                version: None,
                protocol: String::new(),
                identifier: Vec::new(),
                rows: Vec::new(),
                fields: Vec::new(),
                rom_check: None,
            },
        }
    }

    fn get(&self, name: &str) -> Option<&'a str> {
        self.entries
            .iter()
            .find_map(|(field, value)| (*field == name).then_some(*value))
    }

    fn required(&self, name: &'static str) -> Result<&'a str, Error> {
        self.get(name)
            .filter(|value| !value.is_empty())
            .ok_or(Error::Invalid(name))
    }

    fn describe(&mut self, format: Format, version: u32, protocol: &str) {
        self.memory.format = format;
        self.memory.version = Some(version);
        self.memory.protocol = protocol.chars().take(128).collect();
    }

    fn meta(&mut self, omit: &[&str]) {
        self.memory.fields = self
            .entries
            .iter()
            .filter(|(name, value)| {
                !omit.contains(name)
                    && indexed_row(name).is_none()
                    && name.len() <= 96
                    && value.len() <= 256
            })
            .take(MAX_FIELDS)
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
    }

    fn nfc(mut self, version: u32, cancel: &AtomicBool) -> Result<Memory, Error> {
        let protocol = self.required("Device type")?;
        self.describe(Format::FlipperNfc, version, protocol);
        self.memory.identifier = hex_bytes(self.required("UID")?)?;
        if self.memory.identifier.is_empty() || self.memory.identifier.len() > MAX_ROW_BYTES {
            return Err(Error::Invalid("unsupported UID length"));
        }
        let classic = self.get("Mifare Classic type").and_then(Geometry::classic);
        let layout = self.nfc_layout(classic)?;
        let rows = self.nfc_rows(classic, layout, cancel)?;
        self.memory.rows = rows;
        self.meta(&[
            "Device type",
            "UID",
            "Data Content",
            "Security Status",
            "Password Read",
            "Password Write",
        ]);
        Ok(self.memory)
    }

    fn nfc_layout(&self, classic: Option<Geometry>) -> Result<Option<(usize, usize)>, Error> {
        if let Some(classic) = classic {
            return Ok(Some((classic.rows, classic.width)));
        }
        if let Some(pages) = self.get("Pages total") {
            return Ok(Some((checked_count(pages)?, 4)));
        }
        if let (Some(count), Some(width)) = (self.get("Block Count"), self.get("Block Size")) {
            let width = u8::from_str_radix(width, 16)
                .ok()
                .map(usize::from)
                .filter(|width| (1..=MAX_ROW_BYTES).contains(width))
                .ok_or(Error::Invalid("NFC block size"))?;
            return Ok(Some((checked_count(count)?, width)));
        }
        Ok(None)
    }

    fn nfc_rows(
        &self,
        classic: Option<Geometry>,
        layout: Option<(usize, usize)>,
        cancel: &AtomicBool,
    ) -> Result<Vec<MemoryRow>, Error> {
        let mut rows = BTreeMap::new();
        for &(name, value) in &self.entries {
            cancelled(cancel)?;
            let Some((address, page)) = indexed_row(name) else {
                continue;
            };
            let value = field_data(value);
            let bytes = hex_bytes(value)?;
            check_geometry(address, &bytes, layout)?;
            let row = if let Some(classic) = classic {
                classic.row(address, bytes)
            } else {
                named_row(address, name, bytes, page && address < 3)
            };
            if rows.insert(address, row).is_some() {
                return Err(Error::Invalid("repeated NFC memory row address"));
            }
        }
        if let Some((count, width)) = layout {
            add_unrecorded(&mut rows, count, width, classic);
        }
        if let Some(payload) = self.get("Data Content") {
            let width = layout.map_or(16, |(_, width)| width);
            let payload = hex_bytes(payload)?;
            if let Some((count, _)) = layout
                && payload.len() != count * width
            {
                return Err(Error::Invalid(
                    "NFC data content length disagrees with block geometry",
                ));
            }
            for row in region("Block", payload, width)? {
                rows.insert(row.address, row);
            }
        }
        Ok(rows.into_values().collect())
    }

    fn lf_rfid(mut self, version: u32) -> Result<Memory, Error> {
        self.describe(Format::FlipperRfid, version, self.required("Key type")?);
        let bytes = hex_bytes(self.required("Data")?)?;
        check_geometry(0, &bytes, None)?;
        self.memory.identifier = bytes.clone();
        self.memory.rows = vec![named_row(0, "Identifier", bytes, false)];
        self.meta(&["Key type", "Data"]);
        Ok(self.memory)
    }

    fn ibutton(mut self, version: u32) -> Result<Memory, Error> {
        let key = if version == 1 { "Key type" } else { "Protocol" };
        let protocol = self.required(key)?;
        self.describe(Format::FlipperIButton, version, protocol);
        let rom = self.get("Rom Data");
        let is_rom = rom.is_some() || protocol.eq_ignore_ascii_case("Dallas");
        let data = rom
            .or_else(|| self.get("Data"))
            .ok_or(Error::Invalid("missing iButton data"))?;
        let data = hex_bytes(data)?;
        check_geometry(0, &data, None)?;
        self.memory.identifier = data.clone();
        let mut row = named_row(0, if is_rom { "ROM" } else { "Identifier" }, data, false);
        if is_rom {
            if row.bytes.len() != 8 {
                return Err(Error::Invalid("Dallas ROM must have eight bytes"));
            }
            self.memory.rom_check = check_rom(&row.bytes);
            row.role = RowRole::Rom;
        }
        self.memory.rows.push(row);
        self.ibutton_regions()?;
        self.meta(&[key, "Rom Data", "Data", "Sram Data", "Eeprom Data"]);
        Ok(self.memory)
    }

    fn ibutton_regions(&mut self) -> Result<(), Error> {
        for (field, title) in [("Sram Data", "SRAM"), ("Eeprom Data", "EEPROM")] {
            if let Some(value) = self.get(field) {
                let rows = region(title, hex_bytes(value)?, 32)?;
                self.memory.rows.extend(rows);
            }
        }
        Ok(())
    }
}

fn indexed_row(name: &str) -> Option<(u32, bool)> {
    let (kind, address) = name.split_once(' ')?;
    let page = match kind {
        "Page" => true,
        "Block" => false,
        _ => return None,
    };
    Some((address.parse().ok()?, page))
}

fn field_data(value: &str) -> &str {
    if let Some((_, data)) = value.split_once("Data:") {
        data.split('|').next().unwrap_or(data).trim()
    } else {
        value
    }
}

fn add_unrecorded(
    rows: &mut BTreeMap<u32, MemoryRow>,
    count: usize,
    width: usize,
    classic: Option<Geometry>,
) {
    for address in 0..count as u32 {
        rows.entry(address).or_insert_with(|| {
            let bytes = vec![None; width];
            if let Some(classic) = classic {
                classic.row(address, bytes)
            } else {
                named_row(address, &format!("Row {address}"), bytes, false)
            }
        });
    }
}

fn named_row(address: u32, name: &str, bytes: Vec<Option<u8>>, manufacturer: bool) -> MemoryRow {
    MemoryRow {
        address,
        name: name.into(),
        sector: None,
        role: if manufacturer {
            RowRole::Manufacturer
        } else {
            RowRole::Data
        },
        bytes,
    }
}

fn region(name: &str, bytes: Vec<Option<u8>>, width: usize) -> Result<Vec<MemoryRow>, Error> {
    if bytes.len().div_ceil(width) > MAX_ROWS {
        return Err(Error::Limit("memory rows"));
    }
    Ok(bytes
        .chunks(width)
        .enumerate()
        .map(|(index, bytes)| {
            let name = format!("{name} {index}");
            named_row(index as u32, &name, bytes.to_vec(), false)
        })
        .collect())
}

fn decimal(value: &str) -> Result<u32, Error> {
    value
        .parse()
        .map_err(|_| Error::Invalid("expected a decimal integer"))
}

fn checked_count(value: &str) -> Result<usize, Error> {
    let count = decimal(value)? as usize;
    if count > MAX_ROWS || count == 0 {
        Err(Error::Limit("memory rows"))
    } else {
        Ok(count)
    }
}

fn check_geometry(
    address: u32,
    bytes: &[Option<u8>],
    layout: Option<(usize, usize)>,
) -> Result<(), Error> {
    if address as usize >= MAX_ROWS || bytes.len() > MAX_ROW_BYTES || bytes.is_empty() {
        return Err(Error::Limit("memory row"));
    }
    if layout.is_some_and(|(count, width)| address as usize >= count || bytes.len() != width) {
        return Err(Error::Invalid(
            "memory row does not match declared geometry",
        ));
    }
    Ok(())
}

fn check_rom(bytes: &[Option<u8>]) -> Option<RomCheck> {
    let (last, body) = bytes.split_last()?;
    let mut crc = 0_u8;
    for &byte in body {
        crc ^= byte?;
        for _ in 0..8 {
            let bottom = crc & 1;
            crc >>= 1;
            if bottom == 1 {
                crc ^= 0x8c;
            }
        }
    }
    Some(RomCheck {
        stored: (*last)?,
        calculated: crc,
    })
}
