// SPDX-License-Identifier: LGPL-3.0-only
use crate::Result;
pub(crate) struct Reader<'a> {
    pub data: &'a [u8],
    pub big: bool,
}
impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, big: false }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let value = self.data.get(..n).ok_or("Truncated NKI structure")?;
        self.data = &self.data[n..];
        Ok(value)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?.try_into().unwrap();
        Ok(if self.big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?.try_into().unwrap();
        Ok(if self.big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn block(&mut self) -> Result<&'a [u8]> {
        let n = u64::from_le_bytes(self.take(8)?.try_into().unwrap());
        let n = usize::try_from(n).map_err(|_| "NKI block size overflow")?;
        self.take(n.checked_sub(8).ok_or("Invalid NKI block size")?)
    }
    pub fn sized(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn text(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        if n > 4096 {
            return Err("NKI string exceeds 4,096 UTF-16 units".into());
        }
        let raw = self.take(n * 2)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        String::from_utf16(&units)
            .map(|s| s.trim_end_matches('\0').to_owned())
            .map_err(|e| e.to_string())
    }
}

/// Checked FastLZ read subset adapted from ConvertWithMoss's FastLZ reader.
pub(crate) fn fastlz(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    if expected > crate::PAYLOAD {
        return Err("NKI expansion exceeds 16 MiB".into());
    }
    let mut reader = Reader::new(data);
    let first = reader.u8()?;
    let level = (first >> 5) + 1;
    if level > 2 {
        return Err("Unsupported FastLZ level".into());
    }
    let mut control = first & 31;
    let mut out = Vec::with_capacity(expected);
    loop {
        if control < 32 {
            let n = control as usize + 1;
            if out.len() + n > expected {
                return Err("FastLZ literal exceeds output".into());
            }
            out.extend_from_slice(reader.take(n)?);
        } else {
            let len = match_length(&mut reader, control, level)?;
            let distance = match_distance(&mut reader, control, level)?;
            if distance > out.len() || out.len() + len + 3 > expected {
                return Err("Invalid FastLZ match".into());
            }
            for _ in 0..len + 3 {
                out.push(out[out.len() - distance]);
            }
        }
        if reader.data.is_empty() {
            break;
        }
        control = reader.u8()?;
    }
    if out.len() != expected {
        return Err("NKI expansion length mismatch".into());
    }
    Ok(out)
}

fn match_length(reader: &mut Reader<'_>, control: u8, level: u8) -> Result<usize> {
    let mut len = (control >> 5) as usize - 1;
    if len == 6 {
        loop {
            let code = reader.u8()? as usize;
            len += code;
            if level == 1 || code != 255 {
                break;
            }
        }
    }
    Ok(len)
}
fn match_distance(reader: &mut Reader<'_>, control: u8, level: u8) -> Result<usize> {
    let high = ((control & 31) as usize) << 8;
    let low = reader.u8()? as usize;
    if level == 2 && low == 255 && high == 31 << 8 {
        let far = (reader.u8()? as usize) << 8 | reader.u8()? as usize;
        Ok(far + 8192)
    } else {
        Ok(high + low + 1)
    }
}
