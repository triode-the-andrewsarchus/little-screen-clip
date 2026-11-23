//! Channel lists and addressed clone records saved by CHIRP and Icom tools.

mod icf;

use crate::file_source::ReadCheck;
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_CHANNELS: usize = 32_768;

pub use icf::{Record, recognizes as is_icf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    ChirpXml,
    ChirpCsv,
    Icom,
}

impl Format {
    pub fn title(self) -> &'static str {
        match self {
            Self::ChirpXml => "CHIRP channel bank",
            Self::ChirpCsv => "CHIRP channel list",
            Self::Icom => "Icom clone image",
        }
    }
}

pub struct Plan {
    pub format: Format,
    pub model: Option<String>,
    pub fields: Vec<(String, String)>,
    pub channels: Vec<Channel>,
    pub records: Vec<Record>,
}

pub const BANDS: [&str; 6] = ["LF", "MF", "HF", "VHF", "UHF", "SHF+"];

pub fn band(frequency_hz: u64) -> usize {
    [300_000, 3_000_000, 30_000_000, 300_000_000, 3_000_000_000]
        .partition_point(|limit| *limit <= frequency_hz)
}

pub fn frequency_label(hz: u64) -> String {
    let (scale, precision, unit) = if hz >= 1_000_000 {
        (1_000_000, 6, "MHz")
    } else if hz >= 1_000 {
        (1_000, 3, "kHz")
    } else {
        return format!("{hz} Hz");
    };
    let fraction = format!("{:0precision$}", hz % scale);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{} {unit}", hz / scale)
    } else {
        format!("{}.{fraction} {unit}", hz / scale)
    }
}

impl Plan {
    pub fn band_counts(&self) -> [usize; BANDS.len()] {
        let mut counts = [0; BANDS.len()];
        for channel in &self.channels {
            counts[band(channel.receive_hz)] += 1;
        }
        counts
    }
}

pub struct Summary {
    pub format: Format,
    pub model: Option<String>,
    pub count: usize,
    pub band_counts: [usize; BANDS.len()],
    pub channels: Vec<CompactChannel>,
    pub records: usize,
    pub address_span: Option<[u32; 2]>,
}

pub struct CompactChannel {
    pub number: u32,
    pub name: String,
    pub receive_hz: u64,
    pub mode: String,
}

pub fn preview(path: &Path, cancel: &AtomicBool) -> Result<Summary, String> {
    let plan = read(path, cancel)?;
    let band_counts = plan.band_counts();
    let count = plan.channels.len();
    let address_span = plan
        .records
        .first()
        .zip(plan.records.last())
        .map(|(first, last)| {
            [
                first.address,
                last.address
                    .saturating_add(last.bytes.len() as u32)
                    .saturating_sub(1),
            ]
        });
    Ok(Summary {
        format: plan.format,
        model: plan.model,
        count,
        band_counts,
        channels: plan
            .channels
            .into_iter()
            .take(6)
            .map(|channel| CompactChannel {
                number: channel.number,
                name: channel.name,
                receive_hz: channel.receive_hz,
                mode: channel.mode,
            })
            .collect(),
        records: plan.records.len(),
        address_span,
    })
}

pub struct Channel {
    pub number: u32,
    pub name: String,
    pub receive_hz: u64,
    pub duplex: String,
    pub offset_hz: Option<u64>,
    pub mode: String,
    pub fields: Vec<(String, String)>,
}

impl Channel {
    pub fn transmit_hz(&self) -> Option<u64> {
        match self.duplex.as_str() {
            "" | "none" => Some(self.receive_hz),
            "-" | "negative" => self.receive_hz.checked_sub(self.offset_hz?),
            "+" | "positive" => self.receive_hz.checked_add(self.offset_hz?),
            "split" => self.offset_hz,
            _ => None,
        }
    }
}

pub fn is_chirp_csv(prefix: &[u8]) -> bool {
    let prefix = prefix.strip_prefix(b"\xef\xbb\xbf").unwrap_or(prefix);
    let Some(first) = prefix.split(|&byte| byte == b'\n').next() else {
        return false;
    };
    let mut csv = csv::ReaderBuilder::new()
        .has_headers(false)
        .trim(csv::Trim::All)
        .from_reader(first);
    csv.records().next().is_some_and(|row| {
        row.is_ok_and(|row| {
            ["Location", "Name", "Frequency", "Duplex", "Offset", "Mode"]
                .iter()
                .all(|name| row.iter().any(|field| field == *name))
                && row
                    .iter()
                    .any(|field| matches!(field, "rToneFreq" | "DtcsCode"))
        })
    })
}

pub fn read(path: &Path, cancel: &AtomicBool) -> Result<Plan, String> {
    let (file, source) = ReadCheck::default().open(path, None, cancel)?;
    let mut reader = (&file).take(MAX_BYTES as u64 + 1);
    let mut bytes = Vec::new();
    let mut buffer = [0; 16_384];
    loop {
        check_cancel(cancel)?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if bytes.len() > MAX_BYTES {
        return Err("Radio programming file exceeds the 8 MiB inspection limit".into());
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let plan = if extension.eq_ignore_ascii_case("icf") {
        icf::read(&bytes, cancel)
    } else if is_chirp_csv(&bytes) {
        chirp_csv(&bytes, cancel)
    } else {
        chirp_xml(&bytes, cancel)
    }?;
    source.check_file(&file)?;
    Ok(plan)
}

fn chirp_csv(bytes: &[u8], cancel: &AtomicBool) -> Result<Plan, String> {
    let mut input = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);
    let columns = input.headers().map_err(|e| e.to_string())?.clone();
    let mut channels = Vec::new();
    for record in input.records() {
        check_cancel(cancel)?;
        let record = record.map_err(|e| e.to_string())?;
        let at = |name: &str| {
            columns
                .iter()
                .position(|field| field == name)
                .and_then(|i| record.get(i))
                .unwrap_or_default()
        };
        let receive_hz = mhz(at("Frequency"));
        if let Some(receive_hz) = receive_hz.filter(|freq| *freq > 0) {
            let number = at("Location")
                .trim()
                .parse()
                .map_err(|_| "Invalid CHIRP channel location")?;
            let fields = columns
                .iter()
                .zip(record.iter())
                .filter(|(_, value)| !value.is_empty())
                .map(|(field, value)| (field.to_owned(), value.to_owned()))
                .collect();
            channels.push(Channel {
                number,
                name: at("Name").to_owned(),
                receive_hz,
                duplex: at("Duplex").trim().to_owned(),
                offset_hz: mhz(at("Offset")),
                mode: at("Mode").to_owned(),
                fields,
            });
        }
        check_channel_count(channels.len())?;
    }
    Ok(channel_plan(Format::ChirpCsv, channels, None))
}

fn chirp_xml(bytes: &[u8], cancel: &AtomicBool) -> Result<Plan, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "CHIRP XML must be UTF-8")?;
    let tree = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            nodes_limit: 700_000,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let radio = tree.root_element();
    if !radio.has_tag_name("radio") {
        return Err("Missing CHIRP radio root element".into());
    }
    let bank = radio
        .children()
        .find(|node| node.has_tag_name("memories"))
        .ok_or("Missing CHIRP memories element")?;
    let mut channels = Vec::new();
    for memory in bank.children().filter(|node| node.has_tag_name("memory")) {
        check_cancel(cancel)?;
        let value = |name: &str| {
            memory
                .children()
                .find(|node| node.has_tag_name(name))
                .and_then(|node| node.text())
                .unwrap_or_default()
        };
        let frequency = memory
            .children()
            .find(|node| node.has_tag_name("frequency"));
        let Some(receive_hz) = frequency.and_then(frequency_node).filter(|freq| *freq > 0) else {
            continue;
        };
        let number = memory
            .attribute("location")
            .and_then(|v| v.parse().ok())
            .ok_or("Invalid CHIRP XML channel location")?;
        let fields = memory
            .children()
            .filter(|node| node.is_element())
            .map(|node| {
                let name = node
                    .attribute("id")
                    .unwrap_or_else(|| node.tag_name().name());
                let value = node
                    .descendants()
                    .filter_map(|item| item.is_text().then(|| item.text()).flatten())
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                (name.to_owned(), value)
            })
            .collect();
        channels.push(Channel {
            number,
            name: value("shortName").to_owned(),
            receive_hz,
            duplex: value("duplex").trim().to_owned(),
            offset_hz: memory
                .children()
                .find(|node| node.has_tag_name("offset"))
                .and_then(frequency_node),
            mode: value("mode").to_owned(),
            fields,
        });
        check_channel_count(channels.len())?;
    }
    let version = radio.attribute("version").map(str::to_owned);
    Ok(channel_plan(Format::ChirpXml, channels, version))
}

fn frequency_node(node: roxmltree::Node<'_, '_>) -> Option<u64> {
    match node.attribute("units") {
        Some("Hz") => node.text()?.trim().parse().ok(),
        None | Some("MHz") => mhz(node.text()?),
        _ => None,
    }
}

fn channel_plan(format: Format, channels: Vec<Channel>, version: Option<String>) -> Plan {
    let fields = version
        .map(|version| vec![("Version".into(), version)])
        .unwrap_or_default();
    Plan {
        format,
        channels,
        model: None,
        fields,
        records: Vec::new(),
    }
}

fn mhz(value: &str) -> Option<u64> {
    let value = value.trim();
    let (whole, decimal) = value.split_once('.').unwrap_or((value, ""));
    let whole = if whole.is_empty() && !decimal.is_empty() {
        0
    } else {
        whole.parse::<u64>().ok()?
    };
    let decimal = decimal.trim_end_matches('0');
    let fraction = if decimal.is_empty() {
        0
    } else {
        decimal.parse::<u64>().ok()?
    };
    if decimal.len() > 6 || !decimal.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    whole
        .checked_mul(1_000_000)?
        .checked_add(fraction.checked_mul(10_u64.pow(6 - decimal.len() as u32))?)
}

fn check_channel_count(count: usize) -> Result<(), String> {
    if count > MAX_CHANNELS {
        Err("Too many radio memories to inspect".into())
    } else {
        Ok(())
    }
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Radio programming read cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_chirp_tables_from_generic_frequency_csv() {
        assert!(!is_chirp_csv(b"Name,Frequency,Offset\nA,145.2,0.6\n"));
        let csv = b"Location,Name,Frequency,Duplex,Offset,Mode,rToneFreq\n1,Repeater,145.125000,-,0.6,FM,88.5\n";
        assert!(is_chirp_csv(csv));
        let plan = chirp_csv(csv, &AtomicBool::new(false)).unwrap();
        assert_eq!(plan.channels[0].transmit_hz(), Some(144_525_000));
    }
}
