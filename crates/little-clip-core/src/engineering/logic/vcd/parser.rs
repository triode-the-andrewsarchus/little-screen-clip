//! VCD value changes read from a bounded seek position.

use super::{
    super::{Level, archive::Positioned},
    header::{Header, Signal, number},
};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    ops::Range,
    sync::Arc,
};

const MAX_TOKEN: usize = 16_384;

pub(super) struct Parser {
    tokens: BufReader<Positioned>,
    pub(super) offset: u64,
    token: Vec<u8>,
    identifier: Vec<u8>,
}

pub(super) enum Event {
    Time(u64),
    Change(Signal, [Level; 64]),
    Reset,
    Ignore,
}

impl Parser {
    pub(super) fn open(file: Arc<File>, extent: Range<u64>) -> Self {
        let offset = extent.start;
        let tokens = BufReader::with_capacity(64 * 1024, Positioned::new(file, extent));
        Self {
            tokens,
            offset,
            token: Vec::with_capacity(64),
            identifier: Vec::with_capacity(16),
        }
    }

    pub(super) fn next(&mut self, header: &Header) -> Result<Option<Event>, String> {
        if !read_token(&mut self.tokens, &mut self.offset, &mut self.token)? {
            return Ok(None);
        }
        let event = match self.token[0] {
            b'#' => Event::Time(tick(&self.token[1..])?),
            b'0' | b'1' | b'x' | b'X' | b'z' | b'Z' => {
                change(header, &self.token[1..], &self.token[..1])?
            }
            b'b' | b'B' => {
                self.read_identifier()?;
                change(header, &self.identifier, &self.token[1..])?
            }
            b'r' | b'R' | b's' | b'S' => {
                self.read_identifier()?;
                Event::Ignore
            }
            b'$' => self.command()?,
            _ => return Err("Unsupported VCD value change syntax.".into()),
        };
        Ok(Some(event))
    }

    fn read_identifier(&mut self) -> Result<(), String> {
        if !read_token(&mut self.tokens, &mut self.offset, &mut self.identifier)? {
            return Err("VCD vector or real value lacks an identifier.".into());
        }
        Ok(())
    }

    fn command(&mut self) -> Result<Event, String> {
        match self.token.as_slice() {
            b"$dumpvars" | b"$dumpall" | b"$dumpon" | b"$end" => Ok(Event::Ignore),
            b"$dumpoff" => Ok(Event::Reset),
            b"$comment" => loop {
                if !read_token(&mut self.tokens, &mut self.offset, &mut self.token)? {
                    return Err("Truncated VCD comment.".into());
                }
                if self.token == b"$end" {
                    return Ok(Event::Ignore);
                }
            },
            _ => Err("Unknown VCD value change command.".into()),
        }
    }
}

fn read_token(
    input: &mut impl BufRead,
    offset: &mut u64,
    token: &mut Vec<u8>,
) -> Result<bool, String> {
    token.clear();
    loop {
        let bytes = input.fill_buf().map_err(io_error)?;
        if bytes.is_empty() {
            return Ok(!token.is_empty());
        }
        let start = if token.is_empty() {
            bytes
                .iter()
                .position(|byte| !byte.is_ascii_whitespace())
                .unwrap_or(bytes.len())
        } else {
            0
        };
        let tail = &bytes[start..];
        let len = tail
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(tail.len());
        if token.len() + len > MAX_TOKEN {
            return Err("VCD token exceeds the supported width.".into());
        }
        token.extend_from_slice(&tail[..len]);
        let end = start + len;
        let finished = end < bytes.len();
        input.consume(end);
        *offset += end as u64;
        if finished {
            return Ok(true);
        }
    }
}

fn change(header: &Header, identifier: &[u8], text: &[u8]) -> Result<Event, String> {
    let signal = header
        .symbols
        .get(identifier)
        .ok_or("VCD change names an undeclared signal.")?;
    if signal.width == 0 {
        return Ok(Event::Ignore);
    }
    if text.is_empty() || text.len() > signal.width {
        return Err("VCD value does not fit its declared signal width.".into());
    }
    let pad = match text[0] {
        b'x' | b'X' => Level::Unknown,
        b'z' | b'Z' => Level::HighImpedance,
        _ => Level::Low,
    };
    let mut bits = [pad; 64];
    for (value, byte) in bits[signal.width - text.len()..].iter_mut().zip(text) {
        *value = match byte {
            b'0' => Level::Low,
            b'1' => Level::High,
            b'x' | b'X' => Level::Unknown,
            b'z' | b'Z' => Level::HighImpedance,
            _ => return Err("Invalid VCD logic level.".into()),
        };
    }
    Ok(Event::Change(*signal, bits))
}

pub(super) fn tick(bytes: &[u8]) -> Result<u64, String> {
    number(bytes)
        .filter(|tick| *tick < 1 << 53)
        .ok_or("Invalid VCD timestamp.".into())
}

fn io_error(error: std::io::Error) -> String {
    format!("Reading VCD values. {error}")
}
