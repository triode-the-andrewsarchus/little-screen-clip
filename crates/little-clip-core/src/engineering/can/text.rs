mod asc;
mod candump;
mod trc;

use std::{
    io::{BufRead, BufReader, Read},
    sync::atomic::AtomicBool,
};

use super::{Capture, Format, Frame, Location, Positioned, check_cancel};

const MAX_LINE_BYTES: usize = 32768;

#[derive(Debug)]
pub(super) struct Configuration(Encoding);

#[derive(Debug)]
enum Encoding {
    Asc(asc::Asc),
    Trc(trc::Trc),
    Candump,
}

impl Configuration {
    pub(super) fn recognize(prefix: &[u8]) -> Option<Self> {
        // Comments can contain localized legacy text. Frame fields are ASCII.
        if prefix.contains(&0) {
            return None;
        }
        let text = String::from_utf8_lossy(prefix);
        let text = text.trim_start_matches('\u{feff}');
        if let Some(asc) = asc::Asc::recognize(text) {
            return Some(Self(Encoding::Asc(asc)));
        }
        if let Some(trc) = trc::Trc::recognize(text) {
            return Some(Self(Encoding::Trc(trc)));
        }
        let valid_rows = text
            .lines()
            .take(8)
            .filter(|line| candump::parse(line, false).is_some())
            .count();
        (valid_rows >= 2 || (valid_rows == 1 && text.lines().count() == 1))
            .then_some(Self(Encoding::Candump))
    }

    pub(super) fn format(&self) -> Format {
        match &self.0 {
            Encoding::Asc(_) => Format::VectorAsc,
            Encoding::Trc(_) => Format::PcanTrc,
            Encoding::Candump => Format::Candump,
        }
    }
}

pub(super) struct Scanner<'a> {
    configuration: &'a Configuration,
    stream: BufReader<Positioned<'a>>,
    file_end: u64,
    offset: u64,
    clock: i128,
    buffer: Vec<u8>,
}

impl<'a> Scanner<'a> {
    pub(super) fn new(
        capture: &'a Capture,
        configuration: &'a Configuration,
        resume: Option<Location>,
        byte_limit: u64,
    ) -> Result<Self, String> {
        let (offset, clock) = match resume {
            None => (0, 0),
            Some(Location::Text { offset, clock }) => (offset, clock),
            _ => return Err("Invalid CAN text capture checkpoint".into()),
        };
        let end = capture.byte_len.min(offset.saturating_add(byte_limit));
        Ok(Self {
            configuration,
            stream: BufReader::with_capacity(
                64 * 1024,
                Positioned {
                    file: &capture.file,
                    offset,
                    end,
                },
            ),
            file_end: capture.byte_len,
            offset,
            clock,
            buffer: Vec::new(),
        })
    }

    pub(super) fn next(
        &mut self,
        bytes: bool,
        cancel: &AtomicBool,
    ) -> Result<Option<(Location, Frame)>, String> {
        loop {
            check_cancel(cancel)?;
            self.buffer.clear();
            let location = Location::Text {
                offset: self.offset,
                clock: self.clock,
            };
            let read = self
                .stream
                .by_ref()
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut self.buffer)
                .map_err(|error| format!("Read CAN trace line: {error}"))?;
            if read == 0 {
                return Ok(None);
            }
            if read > MAX_LINE_BYTES {
                return Err("CAN trace line exceeds the line size limit".into());
            }
            self.offset += read as u64;
            if !self.buffer.ends_with(b"\n")
                && self.offset == self.stream.get_ref().end
                && self.offset != self.file_end
            {
                return Ok(None);
            }
            let text = String::from_utf8_lossy(&self.buffer);
            let line = text.trim().trim_start_matches('\u{feff}');
            let frame = match &self.configuration.0 {
                Encoding::Asc(configuration) => configuration.parse(line, &mut self.clock, bytes),
                Encoding::Trc(configuration) => configuration.parse(line, bytes),
                Encoding::Candump => candump::parse(line, bytes),
            };
            if let Some(frame) = frame {
                return Ok(Some((location, frame)));
            }
        }
    }

    pub(super) fn covered_bytes(&self) -> u64 {
        self.offset
    }
}

fn decimal_ns(text: &str, fractional_digits: u32) -> Option<i128> {
    let negative = text.starts_with('-');
    let text = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (whole, fraction) = text.split_once(['.', ',']).unwrap_or((text, ""));
    if whole.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|digit| digit.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.parse::<u64>().ok()?;
    let sub = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(fractional_digits as usize)
        .fold(0i128, |sum, digit| sum * 10 + i128::from(digit - b'0'));
    let value = i128::from(whole) * 10i128.pow(fractional_digits) + sub;
    Some(if negative { -value } else { value })
}

fn radix_value(text: &str, base: u32) -> Option<u32> {
    let text = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    u32::from_str_radix(text, base).ok()
}

fn data_bytes<'a>(
    fields: impl Iterator<Item = &'a str>,
    base: u32,
    length: u16,
    retain: bool,
) -> Option<Vec<u8>> {
    let mut fields = fields;
    let mut data = if retain {
        Vec::with_capacity(usize::from(length))
    } else {
        Vec::new()
    };
    for _ in 0..length {
        let byte = u8::try_from(radix_value(fields.next()?, base)?).ok()?;
        if retain {
            data.push(byte);
        }
    }
    Some(data)
}
