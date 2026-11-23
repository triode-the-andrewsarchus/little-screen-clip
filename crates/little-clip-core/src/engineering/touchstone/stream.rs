use super::{Complex64, Encoding, Error, MAX_LINE_BYTES, Metadata, Point, header::Header};
use std::{
    io::{BufRead, ErrorKind, Seek, SeekFrom},
    path::Path,
};

const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

pub(super) struct Lines<R> {
    input: R,
    offset: u64,
    buffer: Vec<u8>,
}

pub(super) struct Line<'a> {
    pub offset: u64,
    pub text: &'a str,
}

impl<R: BufRead + Seek> Lines<R> {
    fn new(input: R) -> Self {
        Self {
            input,
            offset: 0,
            buffer: Vec::new(),
        }
    }

    pub fn position(&self) -> u64 {
        self.offset
    }

    pub fn seek(&mut self, offset: u64) -> Result<(), Error> {
        self.input.seek(SeekFrom::Start(offset))?;
        self.offset = offset;
        Ok(())
    }

    pub fn content(&mut self, maximum_offset: u64) -> Result<Option<Line<'_>>, Error> {
        let offset = loop {
            let start = self.offset;
            let limit = maximum_offset
                .saturating_sub(start)
                .min(MAX_LINE_BYTES as u64);
            self.buffer.clear();
            self.read_source_line(limit as usize)?;
            let bytes = self.buffer.len();
            if bytes == 0 {
                return Ok(None);
            }
            self.offset += bytes as u64;
            if !content_text(&self.buffer)?.is_empty() {
                break start;
            }
        };
        Ok(Some(Line {
            offset,
            text: content_text(&self.buffer)?,
        }))
    }

    fn read_source_line(&mut self, limit: usize) -> Result<(), Error> {
        loop {
            let available = match self.input.fill_buf() {
                Ok([]) => return Ok(()),
                Ok(bytes) => bytes,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            let remaining = limit - self.buffer.len();
            let scan = &available[..available.len().min(remaining + 1)];
            let end = scan.iter().position(|&byte| is_line_break(byte));
            let count = end.map_or(scan.len(), |position| position + 1);
            if count > remaining {
                return Err(Error::Limit("header, line or record is too long"));
            }
            self.buffer.extend_from_slice(&scan[..count]);
            self.input.consume(count);
            if end.is_some() {
                return Ok(());
            }
        }
    }
}

pub(super) fn is_line_break(byte: u8) -> bool {
    matches!(byte, b'\r' | b'\n')
}

fn content_text(line: &[u8]) -> Result<&str, Error> {
    let end = line.iter().position(|&byte| byte == b'!');
    let line = std::str::from_utf8(&line[..end.unwrap_or(line.len())])
        .map_err(|_| Error::Invalid("expected ASCII network data"))?;
    Ok(line.trim().trim_start_matches('\u{feff}'))
}

/// Incremental reader over an already opened source.
///
/// Reading the header does not load the sweep. The caller owns guarded
/// file opening, revision checks, background admission and cancellation.
pub struct Reader<R> {
    lines: Lines<R>,
    header: Header,
    slot_order: Vec<(usize, usize)>,
    scratch: Vec<Complex64>,
    next_index: u64,
    previous_hz: Option<f64>,
    done: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Record {
    pub offset: u64,
    pub index: u64,
    pub frequency_hz: f64,
}

impl<R: BufRead + Seek> Reader<R> {
    /// Opens the source at its header and identifies its matrix and encoding.
    ///
    /// # Errors
    ///
    /// Returns an error if the format is unsupported or if the bounded header
    /// cannot be read and validated.
    pub fn open(input: R, path: &Path) -> Result<Self, Error> {
        let mut lines = Lines::new(input);
        lines.seek(0)?;
        let header = Header::read(&mut lines, path)?;
        let ports = header.metadata.ports;
        Ok(Self {
            slot_order: header.matrix_slots(),
            scratch: vec![Complex64::default(); ports * ports],
            header,
            lines,
            next_index: 0,
            previous_hz: None,
            done: false,
        })
    }

    /// Returns the validated header without scanning the sweep.
    pub fn metadata(&self) -> &Metadata {
        &self.header.metadata
    }

    /// Reads the next frequency record. An end marker or noise section ends the sweep.
    ///
    /// # Errors
    ///
    /// Returns an error for an incomplete, unordered, or malformed record.
    pub fn next_point(&mut self) -> Result<Option<Point>, Error> {
        Ok(self.read_record()?.map(|record| self.point(record)))
    }

    pub(super) fn sample_value(&self, position: usize) -> Complex64 {
        self.scratch[position]
    }

    pub(super) fn point(&self, record: Record) -> Point {
        Point {
            index: record.index,
            frequency_hz: record.frequency_hz,
            ports: self.header.metadata.ports,
            values: self.scratch.clone().into_boxed_slice(),
        }
    }

    pub(super) fn read_record(&mut self) -> Result<Option<Record>, Error> {
        if self.done {
            return Ok(None);
        }
        let limit = self.lines.position().saturating_add(MAX_RECORD_BYTES);
        let Some(line) = self.lines.content(limit)? else {
            return self.finish_at_eof();
        };
        if self.header.metadata.declared_points == Some(self.next_index) {
            self.done = true;
            return validate_tail(line.text, self.header.metadata.declared_noise_points);
        }
        let mut tokens = line.text.split_ascii_whitespace();
        let frequency_hz =
            number(tokens.next().ok_or(Error::Truncated)?)? * self.header.frequency_multiplier;
        if is_legacy_noise(&self.header, self.previous_hz, frequency_hz, tokens.clone()) {
            self.done = true;
            return Ok(None);
        }
        if !frequency_hz.is_finite() || frequency_hz < 0.0 {
            return Err(Error::Invalid("frequency is out of range"));
        }
        if self.previous_hz.is_some_and(|prev| frequency_hz < prev) {
            return Err(Error::Invalid("network frequencies must be ascending"));
        }
        let offset = line.offset;
        let mut decoder = Pairs {
            encoding: self.header.encoding,
            slots: &self.slot_order,
            values: &mut self.scratch,
            pair: 0,
            first: None,
        };
        decoder.consume(tokens)?;
        while decoder.pair < decoder.slots.len() {
            let next = self.lines.content(limit)?.ok_or(Error::Truncated)?;
            decoder.consume(next.text.split_ascii_whitespace())?;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.previous_hz = Some(frequency_hz);
        Ok(Some(Record {
            offset,
            index,
            frequency_hz,
        }))
    }

    fn finish_at_eof(&mut self) -> Result<Option<Record>, Error> {
        self.done = true;
        if self.header.metadata.version.legacy() {
            Ok(None)
        } else {
            Err(Error::Truncated)
        }
    }

    pub(super) fn position_at(&mut self, offset: u64, point: u64) -> Result<(), Error> {
        self.lines.seek(offset)?;
        self.done = false;
        self.next_index = point;
        self.previous_hz = None;
        Ok(())
    }

    pub(super) fn restart(&mut self) -> Result<(), Error> {
        self.position_at(self.header.network_start, 0)
    }
}

fn validate_tail(text: &str, declared_noise: Option<u64>) -> Result<Option<Record>, Error> {
    if text.eq_ignore_ascii_case("[End]") && declared_noise.is_none()
        || text.eq_ignore_ascii_case("[Noise Data]") && declared_noise.is_some()
    {
        Ok(None)
    } else {
        Err(Error::Invalid(
            "network point count disagrees with header or end marker is missing",
        ))
    }
}

fn is_legacy_noise(
    header: &Header,
    previous: Option<f64>,
    frequency: f64,
    tokens: std::str::SplitAsciiWhitespace<'_>,
) -> bool {
    header.metadata.version.legacy()
        && header.metadata.ports == 2
        && previous.is_some_and(|prev| frequency <= prev)
        && tokens.count() == 4
}

struct Pairs<'a> {
    encoding: Encoding,
    slots: &'a [(usize, usize)],
    values: &'a mut [Complex64],
    pair: usize,
    first: Option<f64>,
}

impl Pairs<'_> {
    fn consume<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) -> Result<(), Error> {
        for token in tokens {
            let Some(&(at, mirror)) = self.slots.get(self.pair) else {
                return Err(Error::Invalid(
                    "too many parameters in one frequency record",
                ));
            };
            if let Some(first) = self.first.take() {
                let value = self.encoding.pair(first, number(token)?)?;
                self.values[at] = value;
                self.values[mirror] = value;
                self.pair += 1;
            } else {
                self.first = Some(self.first_value(token)?);
            }
        }
        Ok(())
    }

    fn first_value(&self, token: &str) -> Result<f64, Error> {
        if self.encoding == Encoding::DecibelAngle && token.eq_ignore_ascii_case("-inf") {
            // Some instrument exporters use minus infinity for zero linear magnitude.
            Ok(f64::NEG_INFINITY)
        } else {
            number(token)
        }
    }
}

pub(super) fn number(token: &str) -> Result<f64, Error> {
    token
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or(Error::Invalid("expected a finite numeric value"))
}
