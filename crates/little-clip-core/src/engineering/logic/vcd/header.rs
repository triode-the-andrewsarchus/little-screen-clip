//! Bounded VCD declarations and four-state bit lanes.

use super::super::MAX_LANES;
use std::{collections::HashMap, ops::Range};

pub(super) const MAX_HEADER: u64 = 1024 * 1024;
const MAX_VECTOR: usize = 64;
const MAX_SCOPE: usize = 64;

pub(super) struct Header {
    pub(super) interval: f64,
    pub(super) data_start: u64,
    pub(super) names: Vec<String>,
    pub(super) symbols: HashMap<Vec<u8>, Signal>,
    pub(super) values: usize,
    pub(super) omitted: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Signal {
    pub(super) start: usize,
    pub(super) width: usize,
}

impl Header {
    pub(super) fn read(bytes: &[u8]) -> Result<(Self, Vec<usize>), String> {
        let mut header = Self {
            interval: 0.0,
            data_start: 0,
            names: Vec::new(),
            symbols: HashMap::new(),
            values: 0,
            omitted: 0,
        };
        let mut scopes = Vec::new();
        let mut bits = Vec::new();
        let mut tokens = Tokens { bytes, position: 0 };
        loop {
            let keyword = tokens.next().ok_or("Missing VCD end of declarations.")?;
            let words = tokens.section()?;
            match keyword {
                b"$timescale" => header.interval = timescale(&words)?,
                b"$scope" if words.len() == 2 && scopes.len() < MAX_SCOPE => {
                    scopes.push(words[1]);
                }
                b"$upscope" if !scopes.is_empty() => {
                    scopes.pop();
                }
                b"$var" => header.declare(&words, &scopes, &mut bits)?,
                b"$enddefinitions" if words.is_empty() => {
                    header.data_start = tokens.position as u64;
                    break;
                }
                b"$date" | b"$version" | b"$comment" => {}
                _ => return Err("Invalid or unsupported VCD declaration.".into()),
            }
        }
        if header.interval <= 0.0 || header.names.is_empty() || !scopes.is_empty() {
            return Err(
                "VCD needs an explicit timescale, closed scopes, and digital signals.".into(),
            );
        }
        Ok((header, bits))
    }

    fn declare(
        &mut self,
        words: &[&[u8]],
        scopes: &[&[u8]],
        bits: &mut Vec<usize>,
    ) -> Result<(), String> {
        let [kind, width, id, label, suffix @ ..] = words else {
            return Err("Truncated VCD signal declaration.".into());
        };
        let width = number(width)
            .and_then(|width| usize::try_from(width).ok())
            .filter(|width| *width > 0)
            .ok_or("Invalid VCD signal width.")?;
        if id.len() > 64 {
            return Err("VCD signal identifier is too long.".into());
        }
        let skipped =
            !digital_kind(kind) || width > MAX_VECTOR || self.names.len() + width > MAX_LANES;
        if skipped {
            self.omitted += 1;
            self.symbols
                .entry(id.to_vec())
                .or_insert(Signal { start: 0, width: 0 });
            return Ok(());
        }
        let signal = self.symbol(id, width)?;
        let name = qualified_name(scopes, label);
        let indices = if width == 1 {
            None
        } else {
            Some(bit_indices(suffix, width)?)
        };
        for bit in 0..width {
            let name = match &indices {
                None => name.clone(),
                Some(indices) => format!("{name}[{}]", indices[bit]),
            };
            self.names.push(name);
            bits.push(signal.start + bit);
        }
        Ok(())
    }

    fn symbol(&mut self, id: &[u8], width: usize) -> Result<Signal, String> {
        if let Some(previous) = self.symbols.get(id).filter(|signal| signal.width > 0) {
            if previous.width != width {
                return Err("A VCD alias has a different width from its signal.".into());
            }
            return Ok(*previous);
        }
        let signal = Signal {
            start: self.values,
            width,
        };
        self.values += width;
        self.symbols.insert(id.to_vec(), signal);
        Ok(signal)
    }
}

fn digital_kind(kind: &[u8]) -> bool {
    matches!(
        kind,
        b"wire"
            | b"reg"
            | b"bit"
            | b"logic"
            | b"integer"
            | b"time"
            | b"parameter"
            | b"event"
            | b"tri"
            | b"tri0"
            | b"tri1"
            | b"triand"
            | b"trior"
            | b"trireg"
            | b"wand"
            | b"wor"
            | b"supply0"
            | b"supply1"
    )
}

fn bit_indices(suffix: &[&[u8]], width: usize) -> Result<Vec<i32>, String> {
    let Some([range]) = (!suffix.is_empty()).then_some(suffix) else {
        return Ok((0..width as i32).rev().collect());
    };
    let range = std::str::from_utf8(range).map_err(|_| "Invalid VCD bus range.")?;
    let (left, right) = range
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.split_once(':'))
        .ok_or("Invalid VCD bus range.")?;
    let ends = left
        .parse::<i32>()
        .ok()
        .zip(right.parse::<i32>().ok())
        .filter(|(left, right)| left.abs_diff(*right) as usize + 1 == width)
        .ok_or("VCD bus width disagrees with its range.")?;
    let step = if ends.0 > ends.1 { -1 } else { 1 };
    Ok((0..width as i32)
        .map(|index| ends.0 + index * step)
        .collect())
}

fn qualified_name(scopes: &[&[u8]], label: &[u8]) -> String {
    scopes
        .iter()
        .copied()
        .chain(std::iter::once(label))
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>()
        .join(".")
        .chars()
        .take(200)
        .collect()
}

fn timescale(words: &[&[u8]]) -> Result<f64, String> {
    let (amount, unit) = match words {
        [single] => {
            let split = single
                .iter()
                .position(|&ch| ch.is_ascii_alphabetic())
                .unwrap_or(single.len());
            single.split_at(split)
        }
        [amount, unit] => (*amount, *unit),
        _ => return Err("Invalid VCD timescale declaration.".into()),
    };
    let amount = number(amount)
        .filter(|n| matches!(*n, 1 | 10 | 100))
        .ok_or("Unsupported VCD timescale precision.")?;
    let seconds = match unit {
        b"s" => 1.0,
        b"ms" => 1e-3,
        b"us" => 1e-6,
        b"ns" => 1e-9,
        b"ps" => 1e-12,
        b"fs" => 1e-15,
        _ => return Err("Unsupported VCD timescale unit.".into()),
    };
    Ok(amount as f64 * seconds)
}

pub(super) fn number(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

struct Tokens<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Tokens<'a> {
    fn next(&mut self) -> Option<&'a [u8]> {
        let input = &self.bytes[self.position..];
        let skip = input.iter().position(|&ch| !ch.is_ascii_whitespace())?;
        self.position += skip;
        let input = &self.bytes[self.position..];
        let size = input
            .iter()
            .position(u8::is_ascii_whitespace)
            .unwrap_or(input.len());
        let range: Range<usize> = self.position..self.position + size;
        self.position = range.end;
        Some(&self.bytes[range])
    }

    fn section(&mut self) -> Result<Vec<&'a [u8]>, String> {
        let mut words = Vec::new();
        loop {
            let word = self.next().ok_or("Truncated VCD declaration.")?;
            if word == b"$end" {
                return Ok(words);
            }
            words.push(word);
        }
    }
}
