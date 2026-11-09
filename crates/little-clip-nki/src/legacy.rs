// SPDX-License-Identifier: LGPL-3.0-only
// Read subset adapted from ConvertWithMoss Kontakt1Format, Kontakt2Header and
// AbstractNKIMetadataFileHandler, Jürgen Moßgraber, copyright 2019–2026.
use crate::{Result, reader::Reader};
use serde_json::{Value, json};
use std::io::Read;

pub(crate) fn read(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut r = Reader::new(bytes);
    let magic = u32::from_be_bytes(r.take(4)?.try_into().unwrap());
    let first = matches!(magic, 0x5ee56eb3 | 0xb36ee55e);
    if !first && !matches!(magic, 0x1290a87f | 0x7fa89012) {
        return Err("Unsupported or protected Kontakt container".into());
    }
    r.big = matches!(magic, 0xb36ee55e | 0x7fa89012);
    let compressed = r.u32()? as usize;
    if first || compressed == 36 {
        return xml(inflate(bytes.get(36..).ok_or("Truncated NKI header")?)?);
    }
    older_payload(r, compressed)
}
fn older_payload(mut r: Reader<'_>, compressed: usize) -> Result<Vec<Value>> {
    let version = r.u16()?;
    r.take(4)?;
    r.u16()?;
    r.take(4 + 4 + 4 + 4 + 6 + 4)?;
    let monolith = r.u32()? != 0;
    r.take(4 + 4 + 104 + 4)?;
    r.take(if version == 0x110 { 16 } else { 4 })?;
    r.u32()?;
    if monolith {
        return Err("Legacy NKI monolith sample tables are not supported".into());
    }
    if version == 0x110 {
        binary_payload(&mut r, compressed)
    } else {
        xml(inflate(r.data)?)
    }
}
fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(crate::PAYLOAD as u64 + 1)
        .read_to_end(&mut result)
        .map_err(|e| e.to_string())?;
    if result.len() > crate::PAYLOAD {
        return Err("NKI XML exceeds 16 MiB".into());
    }
    Ok(result)
}
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn xml(bytes: Vec<u8>) -> Result<Vec<Value>> {
    let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 100_000,
        },
    )
    .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    let mut count = 0;
    for program in doc
        .descendants()
        .filter(|n| matches!(n.tag_name().name(), "NiSS_Program" | "K2_Program"))
    {
        if result.len() >= 256 {
            return Err("Too many NKI instruments".into());
        }
        let mut regions = Vec::new();
        for zone in program
            .descendants()
            .filter(|n| matches!(n.tag_name().name(), "NiSS_Zone" | "K2_Zone"))
        {
            count += 1;
            if count > 8192 {
                return Err("Too many NKI zones".into());
            }
            regions.push(xml_region(zone)?);
        }
        result.push(json!({"name":program.attribute("name").unwrap_or(""),"regions":regions}));
    }
    if result.is_empty() {
        return Err("No readable NKI program".into());
    }
    Ok(result)
}
fn values<'a>(zone: roxmltree::Node<'a, 'a>, tag: &str) -> Vec<(&'a str, &'a str)> {
    zone.children()
        .find(|n| n.has_tag_name(tag))
        .into_iter()
        .flat_map(|n| n.children())
        .filter(|n| n.has_tag_name("V"))
        .filter_map(|n| Some((n.attribute("name")?, n.attribute("value")?)))
        .collect()
}
fn xml_region(zone: roxmltree::Node<'_, '_>) -> Result<Value> {
    let params = values(zone, "Parameters");
    let sample = values(zone, "Sample");
    let get = |key: &str, fallback: u64| -> Result<u64> {
        params
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.parse::<u64>().map_err(|e| e.to_string()))
            .unwrap_or(Ok(fallback))
    };
    let path = ["file_ex2", "file_ex", "file"]
        .iter()
        .find_map(|key| sample.iter().find(|(k, _)| k == key).map(|(_, v)| *v))
        .unwrap_or("");
    let path = decode_path(path)?;
    let start = get("sampleStart", 0)?;
    let end = get("sampleEnd", 0)?;
    if params.len() > 128 || path.len() > 4096 {
        return Err("NKI zone metadata exceeds limits".into());
    }
    Ok(
        json!({"name":path,"sample":path,"key":[get("lowKey",0)?,get("highKey",127)?],
        "velocity":[get("lowVelocity",0)?,get("highVelocity",127)?],"root":get("rootKey",60)?,
        "start":start,"end":if end>start {Some(end)} else {None},"loops":[],"properties":params}),
    )
}
fn decode_path(text: &str) -> Result<String> {
    if !text.starts_with('@') {
        return Ok(text.replace('\\', "/"));
    }
    let mut r = Reader::new(&text.as_bytes()[1..]);
    let mut path = String::new();
    let mut library = false;
    while !r.data.is_empty() {
        let kind = r.u8()?;
        match kind {
            b'v' | b'd' | b'm' => {
                let size = std::str::from_utf8(r.take(3)?)
                    .map_err(|e| e.to_string())?
                    .parse::<usize>()
                    .map_err(|e| e.to_string())?;
                let part = std::str::from_utf8(r.take(size)?).map_err(|e| e.to_string())?;
                if kind == b'm' {
                    library = true;
                    path.clear();
                } else {
                    if kind == b'v' && !part.contains(':') {
                        path.push('/');
                    }
                    path.push_str(part);
                    path.push('/');
                }
            }
            b'b' => path.push_str("../"),
            b'f' | b'F' => {
                r.take(if kind == b'f' { 6 } else { 11 })?;
                path.push_str(std::str::from_utf8(r.data).map_err(|e| e.to_string())?);
                let path = decode_utf16_escapes(&path)?;
                return Ok(if library {
                    format!("[Library] {path}")
                } else {
                    path
                });
            }
            _ => return Err("Unsupported encoded NKI sample path".into()),
        }
    }
    Err("NKI path has no filename".into())
}

// Kontakt's extended paths use percent-prefixed UTF-16 units rather than URL
// byte escapes. Decode the assembled path once, preserving literal '%' names.
fn decode_utf16_escapes(mut text: &str) -> Result<String> {
    let mut units = Vec::new();
    while !text.is_empty() {
        let escaped = text
            .strip_prefix('%')
            .and_then(|s| s.get(..4))
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|hex| u16::from_str_radix(hex, 16).ok());
        if let Some(unit) = escaped {
            units.push(unit);
            text = &text[5..];
        } else {
            let value = text.chars().next().unwrap();
            let mut buffer = [0; 2];
            units.extend_from_slice(value.encode_utf16(&mut buffer));
            text = &text[value.len_utf8()..];
        }
    }
    String::from_utf16(&units).map_err(|e| format!("Invalid encoded NKI path: {e}"))
}

fn binary_payload(r: &mut Reader<'_>, compressed: usize) -> Result<Vec<Value>> {
    let checksum = r.u32()?;
    let expected = r.u32()? as usize;
    r.take(32)?;
    let data = r.take(compressed)?;
    if crc32(data) != checksum {
        return Err("NKI payload checksum mismatch".into());
    }
    crate::binary::read(&crate::reader::fastlz(data, expected)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extended_sample_paths_decode_spaces_unicode_and_library_references() {
        assert_eq!(
            decode_path("@d007Samplesf000000K%00e9y%0020C4.wav").unwrap(),
            "Samples/Kéy C4.wav"
        );
        assert_eq!(
            decode_path("@f000000Tone%0020%D83D%DE00.wav").unwrap(),
            "Tone \u{1f600}.wav"
        );
        assert_eq!(
            decode_path("@m003Libf000000Tone.wav").unwrap(),
            "[Library] Tone.wav"
        );
        assert_eq!(decode_path("plain%20name.wav").unwrap(), "plain%20name.wav");
        assert!(decode_path("@f000000Invalid%D800.wav").is_err());
    }
}
