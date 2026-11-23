//! Icom ID-31 and ID-51-family memory channels.
//!
//! Field positions are from the published CHIRP ID-31 and ID-51 drivers, and
//! were checked against D-STAR UK ICF files and CHIRP's Plus2 memory fixture.
//! https://github.com/kk7ds/chirp/tree/97e61a6/chirp/drivers

use super::{Channel, Record, name};
use crate::engineering::radio::frequency_label;

const CHANNELS: usize = 500;
const RECORD_BYTES: usize = 49;
const BANKS: usize = 26;
const BANK_NAME_BYTES: usize = 16;
const ID51_VARIANT: usize = 0x1AF40;

struct Layout {
    code: &'static str,
    label: &'static str,
    bytes: usize,
    used: usize,
    skip: usize,
    program_skip: usize,
    banks: usize,
    bank_names: usize,
    bank_mask: u8,
    variant: Variant,
}

#[derive(Clone, Copy)]
enum Variant {
    Id31,
    Id51,
    Id51Plus,
}

const ID31: Layout = Layout {
    code: "33220001",
    label: "ID-31",
    bytes: 0x15500,
    used: 0x69C0,
    skip: 0x6A06,
    program_skip: 0x6A4B,
    banks: 0x6AC0,
    bank_names: 0x6F50,
    bank_mask: 0xFF,
    variant: Variant::Id31,
};

const ID51: Layout = Layout {
    code: "33900001",
    label: "ID-51",
    bytes: 0x1FB40,
    used: 0x6A40,
    skip: 0x6A86,
    program_skip: 0x6ACB,
    banks: 0x6B40,
    bank_names: 0x6FD0,
    bank_mask: 0xFF,
    variant: Variant::Id51,
};

const ID51_PLUS: Layout = Layout {
    code: "33900002",
    label: "ID-51 Plus",
    bank_mask: 0x1F,
    variant: Variant::Id51Plus,
    ..ID51
};

const ID51_PLUS2: Layout = Layout {
    code: "33900003",
    label: "ID-51 Plus2",
    ..ID51_PLUS
};

const TUNING_STEPS_HZ: [u64; 14] = [
    5_000, 6_250, 0, 0, 10_000, 12_500, 15_000, 20_000, 25_000, 30_000, 50_000, 100_000, 125_000,
    200_000,
];
const CTCSS_TENTH_HZ: [u16; 50] = [
    670, 693, 719, 744, 770, 797, 825, 854, 885, 915, 948, 974, 1000, 1035, 1072, 1109, 1148, 1188,
    1230, 1273, 1318, 1365, 1413, 1462, 1514, 1567, 1598, 1622, 1655, 1679, 1713, 1738, 1773, 1799,
    1835, 1862, 1899, 1928, 1966, 1995, 2035, 2065, 2107, 2181, 2257, 2291, 2336, 2418, 2503, 2541,
];
const DTCS_CODES: [u16; 104] = [
    23, 25, 26, 31, 32, 36, 43, 47, 51, 53, 54, 65, 71, 72, 73, 74, 114, 115, 116, 122, 125, 131,
    132, 134, 143, 145, 152, 155, 156, 162, 165, 172, 174, 205, 212, 223, 225, 226, 243, 244, 245,
    246, 251, 252, 255, 261, 263, 265, 266, 271, 274, 306, 311, 315, 325, 331, 332, 343, 346, 351,
    356, 364, 365, 371, 411, 412, 413, 423, 431, 432, 445, 446, 452, 454, 455, 462, 464, 465, 466,
    503, 506, 516, 523, 526, 532, 546, 565, 606, 612, 624, 627, 631, 632, 654, 662, 664, 703, 712,
    723, 731, 732, 734, 743, 754,
];

pub(super) fn decode(model: &str, records: &[Record]) -> Option<(&'static str, Vec<Channel>)> {
    let layout = [ID31, ID51, ID51_PLUS, ID51_PLUS2]
        .into_iter()
        .find(|layout| layout.code.eq_ignore_ascii_case(model))?;
    let image = assemble(records, layout.bytes)?;
    if !layout.variant.matches(&image) {
        return None;
    }
    let channels = (0..CHANNELS)
        .filter(|&channel| !flag(&image, layout.used, channel))
        .map(|channel| read_channel(&layout, &image, channel))
        .collect::<Option<_>>()?;
    Some((layout.label, channels))
}

fn assemble(records: &[Record], length: usize) -> Option<Vec<u8>> {
    let mut image = vec![0; length];
    let mut seen = vec![false; length];
    let mut covered = 0;
    for record in records {
        let start = usize::try_from(record.address).ok()?;
        let end = start.checked_add(record.bytes.len())?;
        let region = seen.get_mut(start..end)?;
        if region.iter().any(|known| *known) {
            return None;
        }
        region.fill(true);
        image[start..end].copy_from_slice(&record.bytes);
        covered += record.bytes.len();
    }
    (covered == length).then_some(image)
}

impl Variant {
    fn matches(self, image: &[u8]) -> bool {
        match self {
            Self::Id31 => true,
            Self::Id51 => image[ID51_VARIANT..ID51_VARIANT + 32]
                .iter()
                .all(|byte| *byte == 0xFF),
            Self::Id51Plus => &image[ID51_VARIANT + 16..ID51_VARIANT + 32] == b"IcomCloneFormat3",
        }
    }

    fn mode(self, code: u16) -> Option<&'static str> {
        match code {
            0 => Some("FM"),
            1 => Some("NFM"),
            3 if !matches!(self, Self::Id31) => Some("AM"),
            5 => Some("DV"),
            _ => None,
        }
    }
}

fn flag(image: &[u8], offset: usize, channel: usize) -> bool {
    image[offset + channel / 8] & (1 << (channel % 8)) != 0
}

fn read_channel(layout: &Layout, image: &[u8], channel: usize) -> Option<Channel> {
    let start = channel * RECORD_BYTES;
    let record = &image[start..start + RECORD_BYTES];
    let frequency = u32::from_be_bytes([0, record[0], record[1], record[2]]);
    let receive_hz = u64::from(frequency & 0x0003_FFFF) * multiplier(frequency, 0x0020_0000);
    if receive_hz == 0 {
        return None;
    }
    let offset_hz =
        u64::from(u16::from_be_bytes([record[3], record[4]])) * multiplier(frequency, 0x0004_0000);
    let tones = u16::from_be_bytes([record[5], record[6]]);
    let mode = layout.variant.mode(tones & 7)?;
    let duplex = match (record[10] >> 2) & 3 {
        0 => "",
        1 => "-",
        2 => "+",
        _ => "unknown",
    };
    let scan = if flag(image, layout.program_skip, channel) {
        "Program skip"
    } else if flag(image, layout.skip, channel) {
        "Skipped"
    } else {
        "Included"
    };
    let mut fields = vec![("Scan".into(), scan.into())];
    read_bank(layout, image, channel, &mut fields);
    if let Some(&step) = TUNING_STEPS_HZ
        .get(usize::from(record[8] >> 4))
        .filter(|&&step| step != 0)
    {
        fields.push(("Tuning step".into(), frequency_label(step)));
    }
    read_tone_settings(record, tones, &mut fields);
    if mode == "DV" {
        read_digital_settings(record, &mut fields);
    }
    Some(Channel {
        number: channel as u32,
        name: name(&record[11..27]),
        receive_hz,
        duplex: duplex.into(),
        offset_hz: matches!(duplex, "+" | "-").then_some(offset_hz),
        mode: mode.into(),
        fields,
    })
}

fn multiplier(frequency: u32, mask: u32) -> u64 {
    if frequency & mask == 0 { 5_000 } else { 6_250 }
}

fn read_bank(layout: &Layout, image: &[u8], channel: usize, fields: &mut Vec<(String, String)>) {
    let bank_entry = layout.banks + channel * 2;
    let bank = usize::from(image[bank_entry] & layout.bank_mask);
    if bank >= BANKS {
        return;
    }
    let label_index = layout.bank_names + BANK_NAME_BYTES * bank;
    let label = name(&image[label_index..label_index + BANK_NAME_BYTES]);
    let group = char::from(b'A' + bank as u8);
    let description = if label.is_empty() {
        group.to_string()
    } else {
        format!("{group} · {label}")
    };
    fields.push(("Bank".into(), description));
    if image[bank_entry + 1] != 0xFF {
        fields.push(("Bank position".into(), image[bank_entry + 1].to_string()));
    }
}

fn read_tone_settings(record: &[u8], tones: u16, fields: &mut Vec<(String, String)>) {
    let configuration = usize::from(record[10] >> 4);
    let Some(tone_mode) = [
        "Carrier", "Tone", "TSQL", "TSQL", "DTCS", "DTCS", "TSQL-R", "DTCS-R",
    ]
    .get(configuration) else {
        return;
    };
    fields.push(("Tone mode".into(), (*tone_mode).into()));
    match configuration {
        0 => {}
        1 => push_tone(fields, "Transmit tone", usize::from(tones >> 10)),
        2 | 3 | 6 => {
            push_tone(fields, "Transmit tone", usize::from(tones >> 10));
            push_tone(fields, "Receive tone", usize::from((tones >> 4) & 0x3F));
        }
        _ => {
            if let Some(code) = DTCS_CODES.get(usize::from(record[7])) {
                fields.push(("DTCS code".into(), format!("{code:03}")));
            }
            let polarity = ["NN", "NR", "RN", "RR"][usize::from(record[10] & 3)];
            fields.push(("DTCS polarity".into(), polarity.into()));
        }
    }
}

fn push_tone(fields: &mut Vec<(String, String)>, label: &str, slot: usize) {
    if let Some(tenth) = CTCSS_TENTH_HZ.get(slot) {
        fields.push((label.into(), format!("{}.{:1} Hz", tenth / 10, tenth % 10)));
    }
}

fn read_digital_settings(record: &[u8], fields: &mut Vec<(String, String)>) {
    let squelch = ["Off", "DSQL", "CSQL"].get(usize::from((record[8] >> 2) & 3));
    if let Some(squelch) = squelch {
        fields.push(("Digital squelch".into(), (*squelch).into()));
    }
    fields.push(("Digital code".into(), record[27].to_string()));
    for (index, label) in ["Destination", "Repeater 1", "Repeater 2"]
        .into_iter()
        .enumerate()
    {
        let start = 28 + index * 7;
        let call = call_sign(&record[start..start + 7]);
        if !call.is_empty() {
            fields.push((label.into(), call));
        }
    }
}

fn call_sign(bytes: &[u8]) -> String {
    let packed = bytes
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let decoded: [u8; 8] = std::array::from_fn(|i| {
        let ascii = ((packed >> ((7 - i) * 7)) & 0x7F) as u8;
        if ascii.is_ascii_graphic() {
            ascii
        } else {
            b' '
        }
    });
    name(&decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_independent_frequency_and_offset_multipliers() {
        let mut image = vec![0xFF; ID31.bytes];
        let record = &mut image[..RECORD_BYTES];
        record.fill(0);
        let receive = 443_312_500u32;
        let encoded = ((receive / 6_250) | 0x0020_0000).to_be_bytes();
        record[..3].copy_from_slice(&encoded[1..]);
        record[3..5].copy_from_slice(&((600_000u32 / 5_000) as u16).to_be_bytes());
        record[8] = 0x10;
        record[10] = 0x14;
        record[11..15].copy_from_slice(b"Test");
        image[ID31.used] = 0b1111_1110;
        image[ID31.skip] = 0;
        image[ID31.program_skip] = 0;
        let (label, channels) = decode(
            ID31.code,
            &[Record {
                address: 0,
                bytes: image,
            }],
        )
        .unwrap();
        assert_eq!(label, "ID-31");
        assert_eq!(channels.len(), 1);
        let channel = &channels[0];
        assert_eq!((channel.number, channel.name.as_str()), (0, "Test"));
        assert_eq!(channel.receive_hz, u64::from(receive));
        assert_eq!(channel.offset_hz, Some(600_000));
        assert_eq!(channel.duplex, "-");
        assert_eq!(channel.mode, "FM");
        assert!(
            channel
                .fields
                .contains(&("Transmit tone".into(), "67.0 Hz".into()))
        );
    }

    #[test]
    fn rejects_ambiguous_models_missing_memory_and_overlaps() {
        let wrong = Record {
            address: 0,
            bytes: vec![0xFF; ID51_PLUS.bytes],
        };
        assert!(decode(ID51_PLUS.code, &[wrong]).is_none());
        let mut bytes = vec![0xFF; ID51_PLUS.bytes];
        bytes[ID51_VARIANT + 16..ID51_VARIANT + 32].copy_from_slice(b"IcomCloneFormat3");
        assert!(
            decode(
                ID51.code,
                &[Record {
                    address: 0,
                    bytes: bytes.clone()
                }]
            )
            .is_none()
        );
        let records = [
            Record { address: 0, bytes },
            Record {
                address: 1,
                bytes: vec![0],
            },
        ];
        assert!(decode(ID51_PLUS.code, &records).is_none());
        let short = Record {
            address: 0,
            bytes: vec![0xFF; ID31.bytes - 1],
        };
        assert!(decode(ID31.code, &[short]).is_none());
    }

    #[test]
    fn reads_seven_bit_dstar_call_characters() {
        let value = b"CQCQCQ  "
            .iter()
            .fold(0u64, |value, byte| (value << 7) | u64::from(*byte));
        assert_eq!(call_sign(&value.to_be_bytes()[1..]), "CQCQCQ");
    }
}
