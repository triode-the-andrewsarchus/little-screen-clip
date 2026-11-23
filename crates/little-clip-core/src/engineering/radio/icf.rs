mod handheld;

use super::{Channel, Format, Plan, check_cancel};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

const R8600: &str = "38180001";
const R8600_SLOTS: usize = 2_000;
const R8600_RECORD: usize = 47;
const R8600_GROUPS: usize = 0x1B8C0;
const R8600_LINKS: usize = 0x1C02C;
const R8600_IN_USE: usize = 0x1CFCC;
const R8600_FLAGS: usize = 0x1D4E5;
const R8600_BYTES: usize = R8600_FLAGS + R8600_SLOTS;

pub struct Record {
    pub address: u32,
    pub bytes: Vec<u8>,
}

// Addressed records are byte pairs. The payload count separates the old
// two-byte and extended four-byte address forms without guessing from model.
fn parse_record(line: &str) -> Option<Record> {
    if !line.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    [8, 4].into_iter().find_map(|width| {
        let address = u32::from_str_radix(line.get(..width)?, 16).ok()?;
        let count = u8::from_str_radix(line.get(width..width + 2)?, 16).ok()?;
        let data = line.as_bytes().get(width + 2..)?;
        if count == 0 || data.len() != usize::from(count) * 2 {
            return None;
        }
        let bytes = data
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect::<Option<Vec<_>>>()?;
        address.checked_add(u32::from(count))?;
        Some(Record { address, bytes })
    })
}

pub fn recognizes(prefix: &[u8]) -> bool {
    let text = String::from_utf8_lossy(prefix);
    let mut lines = text
        .lines()
        .map(|line| line.trim_start_matches('\u{feff}').trim())
        .filter(|line| !line.is_empty());
    lines
        .next()
        .is_some_and(|line| line.len() == 8 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && lines
            .filter(|line| !line.starts_with('#'))
            .next()
            .is_some_and(|line| parse_record(line).is_some())
}

pub(super) fn read(bytes: &[u8], cancel: &AtomicBool) -> Result<Plan, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text
        .lines()
        .map(|line| line.trim_start_matches('\u{feff}').trim())
        .filter(|line| !line.is_empty());
    let model = lines
        .next()
        .filter(|line| line.len() == 8 && line.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Invalid Icom clone model code")?;
    let mut fields = vec![("Model code".to_owned(), model.to_ascii_uppercase())];
    let mut records = BTreeMap::new();
    for line in lines {
        check_cancel(cancel)?;
        if let Some(comment) = line.strip_prefix('#') {
            if let Some((key, value)) = comment.split_once('=')
                && fields.len() < 256
            {
                fields.push((key.to_owned(), value.to_owned()));
            }
        } else {
            let record = parse_record(line).ok_or("Malformed Icom addressed memory record")?;
            if records.insert(record.address, record).is_some() {
                return Err("Repeated address in Icom memory image".into());
            }
        }
    }
    let records: Vec<_> = records.into_values().collect();
    if records.is_empty() {
        return Err("No addressed data in the Icom image".into());
    }
    let map_one = fields
        .iter()
        .any(|(field, value)| field.eq_ignore_ascii_case("MapRev") && value.trim() == "1");
    let (model_name, channels) = if model == R8600 {
        (
            "IC-R8600".to_owned(),
            r8600_channels(&records).unwrap_or_default(),
        )
    } else if map_one && let Some((model, channels)) = handheld::decode(model, &records) {
        (model.to_owned(), channels)
    } else {
        (model.to_owned(), Vec::new())
    };
    Ok(Plan {
        format: Format::Icom,
        model: Some(model_name),
        fields,
        channels,
        records,
    })
}

fn r8600_channels(records: &[Record]) -> Option<Vec<Channel>> {
    let mut image = vec![0xff; R8600_BYTES];
    let mut known = vec![false; R8600_BYTES];
    for record in records {
        let start = (record.address as usize).min(R8600_BYTES);
        let end = (start + record.bytes.len()).min(R8600_BYTES);
        image[start..end].copy_from_slice(&record.bytes[..end - start]);
        known[start..end].fill(true);
    }
    if !known.iter().all(|byte| *byte) {
        return None;
    }
    let used =
        (0..R8600_SLOTS).filter(|&slot| image[R8600_IN_USE + slot / 8] & (1 << (slot % 8)) == 0);
    let mut seen = vec![false; R8600_SLOTS];
    let mut channels = Vec::new();
    for group in 0..100 {
        let offset = R8600_GROUPS + group * 19;
        let group_entry = &image[offset..offset + 19];
        if group_entry[0] == 0xff {
            continue;
        }
        let group_name = name(&group_entry[3..]);
        let mut slot = usize::from(u16::from_le_bytes([group_entry[1], group_entry[2]]));
        while slot < R8600_SLOTS && !seen[slot] {
            seen[slot] = true;
            let slice = &image[slot * R8600_RECORD..][..R8600_RECORD];
            let skip = image[R8600_FLAGS + slot];
            if let Some(channel) = r8600_channel(slice, group, &group_name, skip) {
                channels.push(channel);
            }
            let next = R8600_LINKS + slot * 2;
            slot = usize::from(u16::from_le_bytes([image[next], image[next + 1]]));
        }
    }
    let numbers: Vec<_> = used.collect();
    if channels.len() != numbers.len() {
        return None;
    }
    for (channel, number) in channels.iter_mut().zip(numbers) {
        channel.number = number as u32;
    }
    Some(channels)
}

fn name(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .rposition(|byte| !matches!(byte, 0 | b' ' | 0xff))
        .map_or(0, |i| i + 1);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn r8600_channel(bytes: &[u8], group: usize, group_name: &str, skip: u8) -> Option<Channel> {
    // IC-R8600 CS-R8600 layout. Field offsets were checked against the
    // published OH1KK sample and model reference in the fixture provenance.
    let rx = u32::from_le_bytes(bytes[..4].try_into().ok()?);
    if rx == 0 || rx == u32::MAX {
        return None;
    }
    let mode_code = usize::from(bytes[4] >> 3);
    let mode = [
        "LSB", "USB", "CW", "CW-R", "FSK", "FSK-R", "AM", "S-AM(D)", "S-AM(U)", "S-AM(L)", "FM",
        "WFM", "D-STAR", "P25", "dPMR", "NXDN-VN", "NXDN-N", "DCR",
    ]
    .get(mode_code)
    .map_or_else(|| format!("Mode {mode_code}"), |mode| (*mode).to_owned());
    let flags = u16::from_le_bytes([bytes[5], bytes[6]]);
    let fields = vec![
        ("Group".into(), format!("{} {group_name}", group + 1)),
        (
            "Receive offset".into(),
            format!("{} Hz", u32::from_le_bytes(bytes[10..14].try_into().ok()?)),
        ),
        ("Filter".into(), ((bytes[4] & 7) + 1).to_string()),
        ("Antenna".into(), (((flags >> 13) & 3) + 1).to_string()),
        (
            "Scan".into(),
            if skip & 6 == 0 { "Included" } else { "Skipped" }.into(),
        ),
    ];
    Some(Channel {
        number: 0,
        name: name(&bytes[15..31]),
        receive_hz: u64::from(rx),
        duplex: "off".into(), // IC-R8600 is a receiver.
        offset_hz: None,
        mode,
        fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_both_clone_address_widths() {
        assert_eq!(parse_record("001002AABB").unwrap().address, 16);
        let record = parse_record("0001002003AABBCC").unwrap();
        assert_eq!(record.address, 0x10020);
        assert_eq!(record.bytes, [0xaa, 0xbb, 0xcc]);
        assert!(parse_record("0001002004AA").is_none());
    }
}
