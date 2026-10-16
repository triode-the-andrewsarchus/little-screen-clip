//! Bounded report-length summary; never reads actual HID reports.
use std::{collections::BTreeMap, io};
#[derive(Clone, Copy, Default)]
struct Globals {
    size: u32,
    count: u32,
    id: u8,
}
struct Item {
    kind: u8,
    tag: u8,
    value: u32,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn item(bytes: &[u8], cursor: &mut usize) -> io::Result<Item> {
    let prefix = bytes[*cursor];
    *cursor += 1;
    if prefix == 0xfe {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "HID long items are not summarized",
        ));
    }
    let size = [0, 1, 2, 4][(prefix & 3) as usize];
    let end = cursor
        .checked_add(size)
        .ok_or_else(|| invalid("HID item offset overflow"))?;
    let data = bytes
        .get(*cursor..end)
        .ok_or_else(|| invalid("Truncated HID item"))?;
    *cursor = end;
    let value = data.iter().enumerate().fold(0u32, |value, (index, byte)| {
        value | u32::from(*byte) << (index * 8)
    });
    Ok(Item {
        kind: (prefix >> 2) & 3,
        tag: prefix >> 4,
        value,
    })
}
fn global(item: &Item, globals: &mut Globals, stack: &mut Vec<Globals>) -> io::Result<()> {
    match item.tag {
        0..=6 => {}
        7 => globals.size = item.value,
        8 => {
            globals.id = u8::try_from(item.value)
                .ok()
                .filter(|id| *id != 0)
                .ok_or_else(|| invalid("Invalid HID report ID"))?
        }
        9 => globals.count = item.value,
        10 if stack.len() < 8 => stack.push(*globals),
        10 => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HID global stack limit exceeded",
            ));
        }
        11 => {
            *globals = stack
                .pop()
                .ok_or_else(|| invalid("HID global stack underflow"))?
        }
        _ => return Err(invalid("Unknown HID global item")),
    }
    Ok(())
}
fn main(item: &Item, globals: Globals, reports: &mut BTreeMap<(u8, u8), u32>) -> io::Result<()> {
    let kind = match item.tag {
        8 => 0,
        9 => 1,
        11 => 2,
        10 | 12 => return Ok(()),
        _ => return Err(invalid("Unknown HID main item")),
    };
    let bits = globals
        .size
        .checked_mul(globals.count)
        .ok_or_else(|| invalid("HID report size overflow"))?;
    let total = reports
        .get(&(globals.id, kind))
        .copied()
        .unwrap_or(0)
        .checked_add(bits)
        .filter(|bits| *bits <= 1024 * 1024)
        .ok_or_else(|| invalid("HID report length exceeds summary limit"))?;
    if reports.len() >= 64 && !reports.contains_key(&(globals.id, kind)) {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "HID report inventory capped at 64",
        ));
    }
    reports.insert((globals.id, kind), total);
    Ok(())
}
pub(super) fn summarize(bytes: &[u8]) -> io::Result<String> {
    if bytes.is_empty() || bytes.len() > 16 * 1024 {
        return Err(invalid("Invalid HID descriptor size"));
    }
    let mut cursor = 0;
    let mut globals = Globals::default();
    let mut stack = Vec::new();
    let mut reports = BTreeMap::new();
    let mut items = 0;
    while cursor < bytes.len() {
        items += 1;
        if items > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HID item inventory capped at 4096",
            ));
        }
        let item = item(bytes, &mut cursor)?;
        match item.kind {
            0 => main(&item, globals, &mut reports)?,
            1 => global(&item, &mut globals, &mut stack)?,
            2 => {}
            _ => return Err(invalid("Reserved HID item type")),
        }
    }
    if !stack.is_empty() {
        return Err(invalid("Unbalanced HID global stack"));
    }
    if reports.is_empty() {
        return Ok("No input/output/feature report items declared".into());
    }
    Ok(reports
        .iter()
        .map(|(&(id, kind), &bits)| {
            format!(
                "{}{}: {bits} bits ({} bytes{})",
                ["Input", "Output", "Feature"][kind as usize],
                if id == 0 {
                    String::new()
                } else {
                    format!(" ID {id}")
                },
                bits.div_ceil(8),
                if id == 0 { "" } else { " + ID byte" },
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_include_padding_and_restore_global_report_state() {
        let descriptor = [
            0x75, 8, 0x95, 3, 0x81, 2, 0x75, 1, 0x95, 5, 0x81, 1, 0xa4, 0x85, 2, 0x75, 8, 0x95, 4,
            0xb1, 2, 0xb4, 0x91, 2,
        ];
        let summary = summarize(&descriptor).unwrap();
        assert!(summary.contains("Input: 29 bits (4 bytes)"));
        assert!(summary.contains("Output: 5 bits (1 bytes)"));
        assert!(summary.contains("Feature ID 2: 32 bits (4 bytes + ID byte)"));
    }
    #[test]
    fn malformed_and_unbounded_descriptors_do_not_create_report_claims() {
        for descriptor in [
            vec![],
            vec![0x77, 1],
            vec![0xb4],
            vec![0x85, 0],
            vec![0x85, 0xff, 0xff],
            vec![0x77, 0xff, 0xff, 0xff, 0xff, 0x95, 2, 0x81, 2],
        ] {
            assert!(summarize(&descriptor).is_err());
        }
        assert_eq!(
            summarize(&[0xfe, 0, 0]).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(summarize(&vec![0xa4; 9]).is_err());
        assert!(summarize(&vec![0x05; 16 * 1024 + 1]).is_err());
    }
}
