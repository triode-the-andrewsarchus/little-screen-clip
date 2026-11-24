use super::{Format, Memory};

/// One decoded record and the first captured Type 2 page contributing to it.
#[derive(Debug, PartialEq, Eq)]
pub struct Record {
    pub page: u32,
    pub title: String,
    pub value: String,
}

#[derive(Default)]
pub struct Records {
    pub entries: Vec<Record>,
    pub partial: bool,
}

/// Read NDEF messages from the capability-container data area of a saved Type 2 tag.
pub fn type2_records(memory: &Memory) -> Option<Records> {
    if memory.format != Format::FlipperNfc {
        return None;
    }
    let cc = memory
        .rows
        .iter()
        .find(|row| row.address == 3 && row.name == "Page 3")?;
    let &[Some(0xe1), Some(version), Some(capacity), Some(_)] = cc.bytes.as_slice() else {
        return None;
    };
    if version >> 4 != 1 || capacity == 0 {
        return None;
    }
    let mut data = vec![None; usize::from(capacity) * 8];
    for row in &memory.rows {
        if let Some(page) = row.address.checked_sub(4)
            && let Some(slot) = data.chunks_mut(4).nth(page as usize)
            && row.bytes.len() == slot.len()
        {
            slot.copy_from_slice(&row.bytes);
        }
    }
    Some(read_tlvs(&data))
}

fn read_tlvs(data: &[Option<u8>]) -> Records {
    let mut records = Records::default();
    let mut offset = 0;
    while offset < data.len() {
        match tlv(&mut offset, data) {
            Some(0x00) => continue,
            Some(0xfe) => return records,
            Some(kind) if read_tlv(kind, &mut offset, data, &mut records).is_some() => {}
            _ => {
                records.partial = true;
                return records;
            }
        }
    }
    records
}

fn tlv(offset: &mut usize, data: &[Option<u8>]) -> Option<u8> {
    let value = (*data.get(*offset)?)?;
    *offset += 1;
    Some(value)
}

fn read_tlv(
    kind: u8,
    offset: &mut usize,
    data: &[Option<u8>],
    records: &mut Records,
) -> Option<()> {
    let len = match tlv(offset, data)? {
        0xff => usize::from(u16::from_be_bytes([tlv(offset, data)?, tlv(offset, data)?])),
        value => usize::from(value),
    };
    let start = *offset;
    let bytes = data.get(start..start + len)?;
    *offset += len;
    if kind == 0x03 && len != 0 {
        let message: Vec<_> = bytes.iter().copied().collect::<Option<_>>()?;
        records.entries.extend(parse(&message, start, 0)?);
    }
    Some(())
}

struct Raw<'a> {
    first: bool,
    last: bool,
    continues: bool,
    tnf: u8,
    kind: &'a [u8],
    payload: &'a [u8],
}

fn take<'a>(remaining: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
    let (head, tail) = remaining.split_at_checked(count)?;
    *remaining = tail;
    Some(head)
}

fn raw<'a>(remaining: &mut &'a [u8]) -> Option<Raw<'a>> {
    let flags = take(remaining, 1)?[0];
    let type_len = usize::from(take(remaining, 1)?[0]);
    let payload_len = if flags & 0x10 != 0 {
        usize::from(take(remaining, 1)?[0])
    } else {
        u32::from_be_bytes(take(remaining, 4)?.try_into().ok()?) as usize
    };
    let id_len = if flags & 0x08 != 0 {
        usize::from(take(remaining, 1)?[0])
    } else {
        0
    };
    let kind = take(remaining, type_len)?;
    take(remaining, id_len)?;
    Some(Raw {
        first: flags & 0x80 != 0,
        last: flags & 0x40 != 0,
        continues: flags & 0x20 != 0,
        tnf: flags & 7,
        kind,
        payload: take(remaining, payload_len)?,
    })
}

fn parse(bytes: &[u8], start: usize, depth: usize) -> Option<Vec<Record>> {
    let mut remaining = bytes;
    let mut records = Vec::new();
    for index in 0..128 {
        let offset = start + bytes.len() - remaining.len();
        let record = raw(&mut remaining)?;
        if record.first != (index == 0) || record.tnf >= 6 {
            return None;
        }
        let (payload, last) = complete_payload(&record, &mut remaining)?;
        let (title, value) = describe(record.tnf, record.kind, &payload, depth);
        records.push(Record {
            page: (4 + offset / 4) as u32,
            title,
            value,
        });
        if last {
            return remaining.is_empty().then_some(records);
        }
    }
    None
}

fn complete_payload<'a>(
    record: &Raw<'a>,
    remaining: &mut &'a [u8],
) -> Option<(std::borrow::Cow<'a, [u8]>, bool)> {
    if !record.continues {
        return Some((record.payload.into(), record.last));
    }
    if record.last {
        return None;
    }
    let mut payload = record.payload.to_vec();
    loop {
        let chunk = raw(remaining)?;
        if chunk.tnf != 6
            || !chunk.kind.is_empty()
            || chunk.first
            || (chunk.last && chunk.continues)
        {
            return None;
        }
        payload.extend_from_slice(chunk.payload);
        if !chunk.continues {
            return Some((payload.into(), chunk.last));
        }
    }
}

fn describe(tnf: u8, kind: &[u8], payload: &[u8], depth: usize) -> (String, String) {
    let text = || String::from_utf8_lossy(payload).into_owned();
    let type_name = String::from_utf8_lossy(kind);
    match (tnf, kind) {
        (1, b"U") => uri(payload)
            .map(|value| (uri_title(&value).into(), value))
            .unwrap_or_else(|| ("URI".into(), "Invalid URI record".into())),
        (1, b"T") => (
            "Text".into(),
            text_record(payload).unwrap_or_else(|| "Invalid text record".into()),
        ),
        (1, b"Sp") if depth < 3 => {
            let lines = parse(payload, 0, depth + 1).map(|records| {
                records
                    .into_iter()
                    .map(|record| record.value)
                    .collect::<Vec<_>>()
                    .join("\n")
            });
            (
                "Smart poster".into(),
                lines.unwrap_or_else(|| "Invalid nested message".into()),
            )
        }
        (2, _)
            if matches!(
                type_name.to_ascii_lowercase().as_str(),
                "text/vcard" | "text/x-vcard"
            ) =>
        {
            ("Contact · vCard".into(), text())
        }
        (2, _) if type_name.starts_with("text/") || type_name == "application/json" => {
            (type_name.into_owned(), text())
        }
        (2, b"application/vnd.wfa.wsc") => ("Wi-Fi configuration".into(), wifi_summary(payload)),
        (2, b"application/vnd.bluetooth.ep.oob" | b"application/vnd.bluetooth.le.oob") => (
            "Bluetooth pairing".into(),
            bluetooth_summary(payload, kind == b"application/vnd.bluetooth.le.oob")
                .unwrap_or_else(|| format!("Invalid Bluetooth record · {} bytes", payload.len())),
        ),
        (3, _) => (uri_title(&type_name).into(), type_name.into_owned()),
        (4, b"android.com:pkg") => ("Android application".into(), text()),
        (_, _) => (
            if type_name.is_empty() {
                "NDEF record".into()
            } else {
                type_name.into_owned()
            },
            format!("{} payload bytes", payload.len()),
        ),
    }
}

fn text_record(payload: &[u8]) -> Option<String> {
    let (&flags, rest) = payload.split_first()?;
    let (language, text) = rest.split_at_checked(usize::from(flags & 0x3f))?;
    let language = std::str::from_utf8(language).ok()?;
    let value = if flags & 0x80 == 0 {
        std::str::from_utf8(text).ok()?.to_owned()
    } else {
        let (text, little) = if let Some(text) = text.strip_prefix(&[0xff, 0xfe]) {
            (text, true)
        } else {
            (text.strip_prefix(&[0xfe, 0xff]).unwrap_or(text), false)
        };
        let words = text.as_chunks::<2>();
        if !words.1.is_empty() {
            return None;
        }
        let words = words
            .0
            .iter()
            .map(|&word| {
                if little {
                    u16::from_le_bytes(word)
                } else {
                    u16::from_be_bytes(word)
                }
            })
            .collect::<Vec<_>>();
        String::from_utf16(&words).ok()?
    };
    Some(if language.is_empty() {
        value
    } else {
        format!("{language} · {value}")
    })
}

fn uri(payload: &[u8]) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "",
        "http://www.",
        "https://www.",
        "http://",
        "https://",
        "tel:",
        "mailto:",
        "ftp://anonymous:anonymous@",
        "ftp://ftp.",
        "ftps://",
        "sftp://",
        "smb://",
        "nfs://",
        "ftp://",
        "dav://",
        "news:",
        "telnet://",
        "imap:",
        "rtsp://",
        "urn:",
        "pop:",
        "sip:",
        "sips:",
        "tftp:",
        "btspp://",
        "btl2cap://",
        "btgoep://",
        "tcpobex://",
        "irdaobex://",
        "file://",
        "urn:epc:id:",
        "urn:epc:tag:",
        "urn:epc:pat:",
        "urn:epc:raw:",
        "urn:epc:",
        "urn:nfc:",
    ];
    let (&prefix, suffix) = payload.split_first()?;
    Some(format!(
        "{}{}",
        PREFIXES.get(usize::from(prefix))?,
        std::str::from_utf8(suffix).ok()?
    ))
}

fn uri_title(value: &str) -> &'static str {
    match value
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .as_deref()
    {
        Some("tel") => "Phone number",
        Some("sms" | "smsto") => "Text message",
        Some("mailto") => "Email",
        Some("geo") => "Location",
        Some("http" | "https") => "Link",
        _ => "URI",
    }
}

fn wifi_summary(payload: &[u8]) -> String {
    fn ssid(mut bytes: &[u8], nested: bool) -> Option<String> {
        while !bytes.is_empty() {
            let tag = u16::from_be_bytes(take(&mut bytes, 2)?.try_into().ok()?);
            let count = u16::from_be_bytes(take(&mut bytes, 2)?.try_into().ok()?);
            let value = take(&mut bytes, usize::from(count))?;
            match tag {
                0x1045 => return Some(String::from_utf8_lossy(value).into_owned()),
                0x100e if !nested => {
                    if let Some(value) = ssid(value, true) {
                        return Some(value);
                    }
                }
                _ => {}
            }
        }
        None
    }
    ssid(payload, false).map_or_else(
        || format!("{} bytes", payload.len()),
        |name| format!("SSID · {name}"),
    )
}

fn bluetooth_summary(mut payload: &[u8], low_energy: bool) -> Option<String> {
    let mut address = None;
    let mut address_kind = "";
    if !low_energy {
        let length = payload.len();
        let encoded = u16::from_le_bytes(take(&mut payload, 2)?.try_into().ok()?);
        if usize::from(encoded) != length {
            return None;
        }
        address = Some(take(&mut payload, 6)?);
    }
    let mut name = None;
    let mut short_name = None;
    while let Some((&len, bytes)) = payload.split_first() {
        if len == 0 {
            break;
        }
        payload = bytes;
        let (&kind, value) = take(&mut payload, usize::from(len))?.split_first()?;
        match (kind, value) {
            (0x09, _) => name = std::str::from_utf8(value).ok(),
            (0x08, _) => short_name = std::str::from_utf8(value).ok(),
            (0x1b, [device @ .., 0 | 1]) if low_energy && device.len() == 6 => {
                address = Some(device);
                address_kind = if value[6] == 0 {
                    " · public"
                } else {
                    " · random"
                };
            }
            _ => {}
        }
    }
    let address = address?
        .iter()
        .rev()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    let transport = if low_energy {
        "Bluetooth LE"
    } else {
        "Bluetooth Classic"
    };
    let device = name
        .or(short_name)
        .filter(|name| !name.is_empty())
        .unwrap_or(transport);
    Some(format!("{device} · {address}{address_kind}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_record_reads_identity_and_bounded_advertising_data() {
        let classic = b"\x0e\0\x06\x05\x04\x03\x02\x01\x05\x09Test";
        assert_eq!(
            bluetooth_summary(classic, false).as_deref(),
            Some("Test · 01:02:03:04:05:06")
        );
        assert_eq!(
            bluetooth_summary(b"\x08\x1b\x06\x05\x04\x03\x02\x01\x01", true).as_deref(),
            Some("Bluetooth LE · 01:02:03:04:05:06 · random")
        );
        assert_eq!(
            bluetooth_summary(&classic[..classic.len() - 1], false),
            None
        );
    }

    #[test]
    fn uri_and_contact_records_preserve_source_pages() {
        let card = b"BEGIN:VCARD\r\nFN:Test Person\r\nEND:VCARD";
        let mut message = b"\x91\x01\x0cU\x04example.org".to_vec();
        message.extend([0x52, 10, card.len() as u8]);
        message.extend(b"text/vcard");
        message.extend(card);
        let mut bytes = vec![Some(3), Some(message.len() as u8)];
        bytes.extend(message.into_iter().map(Some));
        bytes.push(Some(0xfe));
        let records = read_tlvs(&bytes);
        assert!(!records.partial);
        assert_eq!(
            records.entries,
            [
                Record {
                    page: 4,
                    title: "Link".into(),
                    value: "https://example.org".into()
                },
                Record {
                    page: 8,
                    title: "Contact · vCard".into(),
                    value: String::from_utf8_lossy(card).into_owned()
                },
            ]
        );
        bytes[10] = None;
        let partial = read_tlvs(&bytes);
        assert!(partial.partial && partial.entries.is_empty());
    }
}
