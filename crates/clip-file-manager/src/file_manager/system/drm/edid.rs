//! Checked EDID base identity and detailed timings; these are advertised data.
use std::io;
pub(in crate::file_manager::system) struct Monitor {
    pub manufacturer: String,
    pub product: u16,
    pub serial: u32,
    pub name: Option<String>,
    pub text_serial: Option<String>,
    pub year: u16,
    pub size: Option<(u8, u8)>,
    pub timings: Vec<String>,
    pub extensions: u8,
}
pub(super) fn parse(bytes: &[u8]) -> io::Result<Monitor> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed EDID header, length or checksum",
        )
    };
    if bytes.len() < 128
        || bytes[..8] != [0, 255, 255, 255, 255, 255, 255, 0]
        || bytes.len() % 128 != 0
    {
        return Err(invalid());
    }
    let blocks = 1 + bytes[126] as usize;
    if bytes.len() != blocks * 128
        || bytes
            .chunks_exact(128)
            .any(|block| block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0)
    {
        return Err(invalid());
    }
    if bytes[18] != 1 || bytes[19] > 4 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unsupported EDID version",
        ));
    }
    let manufacturer = u16::from_be_bytes([bytes[8], bytes[9]]);
    let letters = [
        (manufacturer >> 10) & 31,
        (manufacturer >> 5) & 31,
        manufacturer & 31,
    ];
    if letters.iter().any(|letter| !(1..=26).contains(letter)) {
        return Err(invalid());
    }
    let mut monitor = Monitor {
        manufacturer: letters
            .iter()
            .map(|letter| (b'A' + *letter as u8 - 1) as char)
            .collect(),
        product: u16::from_le_bytes([bytes[10], bytes[11]]),
        serial: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        name: None,
        text_serial: None,
        year: 1990 + bytes[17] as u16,
        size: None,
        timings: vec![],
        extensions: bytes[126],
    };
    if bytes[21] != 0 && bytes[22] != 0 {
        monitor.size = Some((bytes[21], bytes[22]));
    }
    for descriptor in bytes[54..126].chunks_exact(18) {
        if descriptor[0] == 0 && descriptor[1] == 0 {
            if descriptor[2] != 0 || descriptor[4] != 0 {
                continue;
            }
            let value = descriptor[5..]
                .iter()
                .take_while(|byte| **byte != 10)
                .filter(|byte| byte.is_ascii_graphic() || **byte == b' ')
                .map(|byte| *byte as char)
                .collect::<String>()
                .trim()
                .to_string();
            if !value.is_empty() {
                match descriptor[3] {
                    0xfc => monitor.name = Some(value),
                    0xff => monitor.text_serial = Some(value),
                    _ => {}
                }
            }
        } else if let Some(timing) = timing(descriptor) {
            monitor.timings.push(timing);
        }
    }
    Ok(monitor)
}
fn timing(data: &[u8]) -> Option<String> {
    let clock = u16::from_le_bytes([data[0], data[1]]) as u32 * 10000;
    let width = data[2] as u32 + ((data[4] as u32 & 0xf0) << 4);
    let height = data[5] as u32 + ((data[7] as u32 & 0xf0) << 4);
    let horizontal = width + data[3] as u32 + ((data[4] as u32 & 15) << 8);
    let vertical = height + data[6] as u32 + ((data[7] as u32 & 15) << 8);
    if width == 0 || height == 0 || horizontal == 0 || vertical == 0 {
        return None;
    }
    if data[17] & 128 != 0 {
        return Some(format!("{width}×{height} interlaced raw DTD"));
    }
    Some(format!(
        "{width}×{height} {:.2} Hz",
        clock as f64 / (horizontal as f64 * vertical as f64)
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut data = vec![0; 128];
        data[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        let vendor = (4u16 << 10) | (5 << 5) | 12;
        data[8..10].copy_from_slice(&vendor.to_be_bytes());
        data[18] = 1;
        data[19] = 4;
        data[17] = 30;
        data[21] = 60;
        data[22] = 34;
        data[54..59].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
        data[59..72].copy_from_slice(b"Test Monitor\n");
        data[127] = 0u8.wrapping_sub(
            data[..127]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
        data
    }
    #[test]
    fn parses_identity_without_claiming_active_mode() {
        let data = fixture();
        let monitor = parse(&data).unwrap();
        assert_eq!(monitor.manufacturer, "DEL");
        assert_eq!(monitor.name.as_deref(), Some("Test Monitor"));
        assert_eq!(monitor.year, 2020);
        assert_eq!(monitor.size, Some((60, 34)));
        assert!(monitor.timings.is_empty());
    }
    #[test]
    fn detailed_timing_is_advertised_and_interlace_stays_explicit() {
        // 148.5 MHz, 1920+280 horizontal and 1080+45 vertical pixels.
        let mut dtd = [0u8; 18];
        dtd[..2].copy_from_slice(&14850u16.to_le_bytes());
        dtd[2..8].copy_from_slice(&[128, 24, 0x71, 56, 45, 0x40]);
        assert_eq!(timing(&dtd).as_deref(), Some("1920×1080 60.00 Hz"));
        dtd[17] = 128;
        assert_eq!(
            timing(&dtd).as_deref(),
            Some("1920×1080 interlaced raw DTD")
        );
        assert!(timing(&[0; 18]).is_none());
    }
    #[test]
    fn every_extension_checksum_is_validated() {
        let mut data = fixture();
        data[126] = 1;
        data[127] = data[127].wrapping_sub(1);
        data.extend([0u8; 128]);
        assert_eq!(parse(&data).unwrap().extensions, 1);
        data[140] = 1;
        assert!(parse(&data).is_err());
    }
    #[test]
    fn rejects_bad_checksums_and_missing_extension_blocks() {
        let mut data = fixture();
        data[20] ^= 1;
        assert!(parse(&data).is_err());
        let mut data = fixture();
        data[126] = 1;
        data[127] = data[127].wrapping_sub(1);
        assert!(parse(&data).is_err());
        assert!(parse(&[0; 64]).is_err());
    }
}
