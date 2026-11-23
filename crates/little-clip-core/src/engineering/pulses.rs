//! Offline inspection of Flipper infrared banks and Sub-GHz records.

use crate::file_source::{FileSource, ReadCheck};
use std::{
    io::Read,
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub const HEADER_BYTES: usize = 512;
const MAX_PREVIEW_BYTES: usize = 48 * 1024;
const MAX_VIEW_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 4096;
const MAX_PULSES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Infrared,
    RadioRaw,
    RadioDecoded,
}

impl Format {
    pub fn title(self) -> &'static str {
        match self {
            Self::Infrared => "Infrared signals",
            Self::RadioRaw => "Sub-GHz timing",
            Self::RadioDecoded => "Sub-GHz record",
        }
    }
}

pub fn identify(prefix: &[u8]) -> Option<Format> {
    let mut lines = prefix.split(|&byte| byte == b'\n').filter_map(|line| {
        let line = std::str::from_utf8(line)
            .ok()?
            .trim_start_matches('\u{feff}')
            .trim();
        (!line.is_empty() && !line.starts_with('#')).then_some(line)
    });
    let format = match lines.next()? {
        "Filetype: IR signals file" | "Filetype: IR library file" => Format::Infrared,
        "Filetype: Flipper SubGhz RAW File" => Format::RadioRaw,
        "Filetype: Flipper SubGhz Key File" => Format::RadioDecoded,
        _ => return None,
    };
    (lines.next()? == "Version: 1").then_some(format)
}

#[derive(Clone, Copy, Debug)]
pub struct Pulse {
    pub start_us: u64,
    pub duration_us: u32,
    pub high: bool,
}

impl Pulse {
    pub fn end_us(self) -> u64 {
        self.start_us + u64::from(self.duration_us)
    }
}

#[derive(Default)]
pub struct Signal {
    pub name: String,
    pub model: Option<String>,
    pub protocol: Option<String>,
    pub carrier_hz: Option<u64>,
    pub duty_cycle: Option<f64>,
    pub fields: Vec<(String, String)>,
    pub pulses: Vec<Pulse>,
}

impl Signal {
    pub fn label(&self) -> std::borrow::Cow<'_, str> {
        match &self.model {
            Some(model) => format!("{model} · {}", self.name).into(),
            None => self.name.as_str().into(),
        }
    }

    pub fn duration_us(&self) -> u64 {
        self.pulses.last().map_or(0, |pulse| pulse.end_us())
    }

    /// Return all pulses intersecting a given half-open time span.
    pub fn intersecting(&self, span: Range<u64>) -> &[Pulse] {
        let first = self
            .pulses
            .partition_point(|pulse| pulse.end_us() <= span.start);
        let end = self
            .pulses
            .partition_point(|pulse| pulse.start_us < span.end);
        &self.pulses[first.min(end)..end]
    }

    fn has_data(&self) -> bool {
        !self.name.is_empty() || !self.pulses.is_empty() || self.protocol.is_some()
    }
}

pub struct Capture {
    pub format: Format,
    pub signals: Vec<Signal>,
    pub complete: bool,
}

impl Capture {
    pub fn preview(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        load(path, MAX_PREVIEW_BYTES, cancel).map(|(_, capture)| capture)
    }

    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<(FileSource, Self), String> {
        load(path, MAX_VIEW_BYTES, cancel)
    }
}

fn load(path: &Path, bound: usize, cancel: &AtomicBool) -> Result<(FileSource, Capture), String> {
    let (file, source) = ReadCheck::default().open(path, None, cancel)?;
    let mut prefix = Vec::new();
    let mut reader = (&file).take(bound as u64 + 1);
    let mut buffer = [0; 8192];
    loop {
        check_cancel(cancel)?;
        let size = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if size == 0 {
            break;
        }
        prefix.extend_from_slice(&buffer[..size]);
    }
    let prefix_complete = prefix.len() <= bound;
    if !prefix_complete {
        prefix.truncate(bound);
        let end = prefix
            .iter()
            .rposition(|&byte| byte == b'\n')
            .ok_or("Pulse file starts with an oversized line")?;
        prefix.truncate(end + 1);
    }
    let capture = parse(&prefix, prefix_complete, cancel)?;
    source.check_file(&file)?;
    Ok((source, capture))
}

fn parse(bytes: &[u8], mut complete: bool, cancel: &AtomicBool) -> Result<Capture, String> {
    let format = identify(&bytes[..bytes.len().min(HEADER_BYTES)])
        .ok_or("This file does not start with a supported Flipper signal header")?;
    let text = std::str::from_utf8(bytes).map_err(|_| "Signal capture must be UTF-8 text")?;
    let mut signals = Vec::new();
    let mut signal = Signal::default();
    let mut budget = MAX_PULSES;
    let mut next_model = None;
    for line in text.lines() {
        check_cancel(cancel)?;
        let line = line.trim_start_matches('\u{feff}').trim();
        if let Some(model) = line.strip_prefix("# Model:") {
            next_model = Some(model.trim().chars().take(256).collect());
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err("A signal field has no name separator".into());
        };
        if key.len() > 96 {
            return Err("Pulse field name exceeds its limit".into());
        }
        let value = value.trim();
        if key == "name" && signal.has_data() {
            signals.push(std::mem::take(&mut signal));
            if signals.len() >= MAX_RECORDS {
                complete = false;
                break;
            }
        }
        if key == "name" {
            signal.model = next_model.take();
        }
        let fully_read = parse_field(&mut signal, key, value, format, &mut budget)?;
        if !fully_read {
            complete = false;
            break;
        }
    }
    if signal.has_data() && signals.len() < MAX_RECORDS {
        if signal.name.is_empty() {
            signal.name = signal
                .protocol
                .clone()
                .unwrap_or_else(|| format.title().to_owned());
        }
        signals.push(signal);
    }
    if signals.is_empty() {
        return Err("The signal file contains no records".into());
    }
    Ok(Capture {
        format,
        signals,
        complete,
    })
}

fn parse_field(
    signal: &mut Signal,
    key: &str,
    value: &str,
    format: Format,
    budget: &mut usize,
) -> Result<bool, String> {
    match key {
        "Filetype" | "Version" => {}
        "name" => signal.name = value.chars().take(256).collect(),
        "protocol" | "Protocol" => signal.protocol = Some(value.chars().take(96).collect()),
        "frequency" | "Frequency" => {
            signal.carrier_hz = Some(value.parse().map_err(|_| "Invalid carrier frequency")?);
        }
        "duty_cycle" => {
            let ratio: f64 = value.parse().map_err(|_| "Invalid duty cycle")?;
            if !(0.0..=1.0).contains(&ratio) {
                return Err("Duty cycle is outside zero to one".into());
            }
            signal.duty_cycle = Some(ratio);
        }
        "data" if format == Format::Infrared => return add_pulses(signal, value, false, budget),
        "RAW_Data" if format == Format::RadioRaw => return add_pulses(signal, value, true, budget),
        _ if signal.fields.len() < 128 => {
            signal
                .fields
                .push((key.to_owned(), value.chars().take(512).collect()));
        }
        _ => {}
    }
    Ok(true)
}

fn add_pulses(
    signal: &mut Signal,
    value: &str,
    signed: bool,
    budget: &mut usize,
) -> Result<bool, String> {
    let mut start = signal.duration_us();
    for field in value.split_whitespace() {
        if *budget == 0 {
            return Ok(false);
        }
        let input: i32 = field.parse().map_err(|_| "Invalid pulse duration")?;
        if input == 0 || (!signed && input < 0) {
            return Err("Pulse durations must describe nonempty intervals".into());
        }
        let high = if signed {
            input > 0
        } else {
            signal.pulses.len().is_multiple_of(2)
        };
        let duration_us = input.unsigned_abs();
        signal.pulses.push(Pulse {
            start_us: start,
            duration_us,
            high,
        });
        start += u64::from(duration_us);
        *budget -= 1;
    }
    Ok(true)
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Pulse capture read cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_multiple_ir_signals_and_signed_radio_timings() {
        let cancel = AtomicBool::new(false);
        let ir = parse(b"Filetype: IR signals file\nVersion: 1\nname: A\ntype: parsed\nprotocol: NEC\naddress: 00 00 00 00\n#\nname: B\ntype: raw\nfrequency: 38000\ndata: 20 30 40\n", true, &cancel).unwrap();
        assert_eq!(ir.signals.len(), 2);
        assert_eq!(ir.signals[0].protocol.as_deref(), Some("NEC"));
        assert_eq!(ir.signals[1].duration_us(), 90);
        let radio = parse(b"Filetype: Flipper SubGhz RAW File\nVersion: 1\nFrequency: 433920000\nProtocol: RAW\nRAW_Data: 40 -50\nRAW_Data: 10 -20\n", true, &cancel).unwrap();
        let signal = &radio.signals[0];
        assert_eq!(signal.duration_us(), 120);
        assert_eq!(signal.intersecting(90..100).len(), 1);
        assert!(signal.intersecting(90..100)[0].high);
        assert!(identify(b"{1}{6}Ordinary MicroDVD subtitles").is_none());
    }
}
