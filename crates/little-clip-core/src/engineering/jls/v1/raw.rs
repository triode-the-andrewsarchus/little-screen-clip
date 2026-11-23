use flate2::{Crc, read::ZlibDecoder};
use serde_json::Value;
use std::io::Read;

use super::super::{Reader, V1_MAGIC, u16_at, u32_at, u64_at};

const MAX_PAYLOAD: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct Chunk {
    pub(super) tag: [u8; 3],
    flags: u8,
    pub(super) payload: u64,
    pub(super) len: u64,
    pub(super) end: u64,
}

impl Chunk {
    pub(super) fn read(reader: &Reader, offset: u64) -> Result<Self, String> {
        let header = reader.array::<8>(offset)?;
        let len = u64::from(u32_at(&header, 4));
        let end = offset
            .checked_add(8 + (len + 4).next_multiple_of(8))
            .filter(|end| *end <= reader.len)
            .ok_or("Legacy Joulescope chunk extends beyond the recording")?;
        Ok(Self {
            tag: header[..3].try_into().unwrap(),
            flags: header[3],
            payload: offset + 8,
            len,
            end,
        })
    }

    pub(super) fn is_plain(self) -> bool {
        self.flags & 0xc0 == 0
    }

    pub(super) fn collection(self, reader: &Reader) -> Result<Self, String> {
        if self.tag != *b"CLS" || self.len < 12 || !self.is_plain() {
            return Err("Invalid legacy Joulescope collection".into());
        }
        let offset = u64::from_le_bytes(reader.array(self.payload)?);
        if offset < self.end || !offset.is_multiple_of(8) {
            return Err("Legacy Joulescope collection has no valid end offset".into());
        }
        let end = Self::read(reader, offset)?;
        if end.tag != *b"CLE" {
            return Err("Legacy Joulescope collection end is missing".into());
        }
        Ok(end)
    }

    pub(super) fn payload(self, reader: &Reader) -> Result<Vec<u8>, String> {
        if self.len > MAX_PAYLOAD as u64 || self.flags & 0x40 != 0 {
            return Err("Unsupported legacy Joulescope metadata payload".into());
        }
        let bytes = reader.bytes(self.payload - 8, (self.end - self.payload + 8) as usize)?;
        let payload = slice_payload(&bytes, self.len as usize)?;
        if self.is_plain() {
            Ok(payload.to_vec())
        } else {
            let mut result = Vec::new();
            ZlibDecoder::new(payload)
                .take(MAX_PAYLOAD as u64 + 1)
                .read_to_end(&mut result)
                .map_err(|error| format!("Decode legacy Joulescope metadata: {error}"))?;
            if result.len() > MAX_PAYLOAD {
                return Err("Legacy Joulescope metadata exceeds its limit".into());
            }
            Ok(result)
        }
    }
}

pub(super) fn check_header(bytes: &[u8; 32], len: u64) -> Result<(), String> {
    let actual_len = u64_at(bytes, 16);
    if bytes[27] != 1 || checksum(&bytes[..28]) != u32_at(bytes, 28) {
        return Err("Unsupported or damaged Joulescope v1 header".into());
    }
    if actual_len != len {
        return Err("Legacy Joulescope recording was not closed with a valid footer".into());
    }
    Ok(())
}

fn slice_payload(bytes: &[u8], payload_len: usize) -> Result<&[u8], String> {
    let crc_offset = bytes.len() - 4;
    if checksum(&bytes[..crc_offset]) != u32_at(bytes, crc_offset) {
        return Err("Invalid legacy Joulescope metadata checksum".into());
    }
    Ok(&bytes[8..8 + payload_len])
}

fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum()
}

#[derive(Debug)]
pub(super) struct Calibration {
    pub(super) model: String,
    i_offset: [f32; 7],
    i_gain: [f32; 7],
    v_offset: [f32; 2],
    v_gain: [f32; 2],
}

impl Calibration {
    pub(super) fn parse(payload: &[u8]) -> Result<Option<Self>, String> {
        if payload.get(..12) != Some(b"calibration\0") {
            return Ok(None);
        }
        let subfile = payload
            .get(128..)
            .ok_or("Truncated Joulescope calibration")?;
        let header = subfile
            .first_chunk::<32>()
            .ok_or("Truncated calibration header")?;
        if !header.starts_with(V1_MAGIC) {
            return Err("Invalid Joulescope calibration header".into());
        }
        check_header(header, subfile.len() as u64)?;
        let mut cursor = 32;
        while let Some(header) = subfile.get(cursor..cursor + 8) {
            let payload_len = u32_at(header, 4) as usize;
            let end = cursor + 8 + (payload_len + 4).next_multiple_of(8);
            let raw = subfile
                .get(cursor..end)
                .ok_or("Truncated calibration chunk")?;
            if (header.starts_with(b"AJS") || header.starts_with(b"CJS")) && header[3] == 0 {
                let json = slice_payload(raw, payload_len)?;
                return Self::coefficients(json).map(Some);
            }
            cursor = end;
        }
        Err("Legacy Joulescope recording has incomplete calibration".into())
    }

    fn coefficients(json: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(json).map_err(|error| error.to_string())?;
        // Older Python writers emit an unused NaN coefficient after the valid
        // current ranges. JSON has no non-finite number literal.
        let value: Value = serde_json::from_str(&text.replace("NaN", "null"))
            .map_err(|error| format!("Decode Joulescope calibration: {error}"))?;
        let model = value["product_name"].as_str().unwrap_or("Joulescope JS110");
        Ok(Self {
            model: model.to_owned(),
            i_offset: coefficients(&value["current"]["offset"])?,
            i_gain: coefficients(&value["current"]["gain"])?,
            v_offset: coefficients(&value["voltage"]["offset"])?,
            v_gain: coefficients(&value["voltage"]["gain"])?,
        })
    }

    fn current(&self, adc: u16, range: usize) -> f32 {
        if range == 7 {
            return 0.0;
        }
        (f32::from(adc) + self.i_offset[range]) * self.i_gain[range]
    }

    fn voltage(&self, adc: u16, range: usize) -> f32 {
        (f32::from(adc) + self.v_offset[range]) * self.v_gain[range]
    }
}

fn coefficients<const N: usize>(value: &Value) -> Result<[f32; N], String> {
    let values = value
        .as_array()
        .ok_or("Missing Joulescope calibration coefficients")?;
    let mut output = [0.0; N];
    if values.len() < N {
        return Err("Incomplete Joulescope calibration coefficients".into());
    }
    for (slot, value) in output.iter_mut().zip(values) {
        *slot = value
            .as_f64()
            .ok_or("Invalid Joulescope calibration coefficient")? as f32;
        if !slot.is_finite() {
            return Err("Invalid Joulescope calibration coefficient".into());
        }
    }
    Ok(output)
}

pub(super) fn raw_sample(
    pair: &[u8],
    calibration: Option<&Calibration>,
    channel: usize,
    voltage_range: usize,
) -> f32 {
    let current = u16_at(pair, 0);
    let voltage = u16_at(pair, 2);
    if current == u16::MAX && voltage == u16::MAX {
        return f32::NAN;
    }
    let range = usize::from((current & 3) | (voltage & 1) << 2);
    match channel {
        0 => calibration.map_or(f32::from(current >> 2), |cal| {
            cal.current(current >> 2, range)
        }),
        1 => calibration.map_or(f32::from(voltage >> 2), |cal| {
            cal.voltage(voltage >> 2, voltage_range)
        }),
        2 => calibration.map_or(f32::NAN, |cal| {
            cal.current(current >> 2, range) * cal.voltage(voltage >> 2, voltage_range)
        }),
        3 => range as f32,
        4 => f32::from((current >> 2) & 1),
        _ => f32::from((voltage >> 2) & 1),
    }
}
