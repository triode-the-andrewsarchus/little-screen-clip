//! Passive inspection of saved NFC, RFID and 1-Wire data.

mod compare;
mod flipper;
mod memory;
mod signal;
mod source;
mod trace;

pub use compare::{Agreement, ComparedRow, MemoryComparison, ObservedByte};
pub use signal::{SampleBin, SignalIndex, SignalPreview, build_signal_index, read_samples};
pub use trace::{
    Direction, Frame, FrameInfo, TraceBin, TraceIndex, TracePreview, build_trace_index,
    read_frame_info, read_frames,
};

use std::{
    io::{self, BufRead, Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
    sync::atomic::AtomicBool,
};
use thiserror::Error;

const MAX_MEMORY_BYTES: u64 = 1024 * 1024;
const MAX_PROBE: usize = 32 * 1024;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Cannot read tag recording: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid tag recording: {0}")]
    Invalid(&'static str),
    #[error("Unsupported tag recording: {0}")]
    Unsupported(&'static str),
    #[error("Tag recording ended inside a record")]
    Truncated,
    #[error("Tag recording limit exceeded: {0}")]
    Limit(&'static str),
    #[error("Tag recording read cancelled")]
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    FlipperNfc,
    FlipperRfid,
    FlipperIButton,
    MifareDump,
    ProxmarkDump,
    ProxmarkTrace,
    ProxmarkSignal,
}

impl Format {
    pub fn title(self) -> &'static str {
        match self {
            Self::FlipperNfc => "Flipper NFC",
            Self::FlipperRfid => "Flipper LF RFID",
            Self::FlipperIButton => "Flipper iButton",
            Self::MifareDump => "MIFARE memory",
            Self::ProxmarkDump => "Proxmark memory",
            Self::ProxmarkTrace => "Proxmark trace",
            Self::ProxmarkSignal => "Proxmark samples",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRole {
    Data,
    Manufacturer,
    SectorTrailer,
    Rom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryRow {
    pub address: u32,
    pub name: String,
    pub sector: Option<u16>,
    pub role: RowRole,
    /// Unknown bytes are preserved separately from known zero bytes.
    pub bytes: Vec<Option<u8>>,
}

impl MemoryRow {
    pub fn known_bytes(&self) -> usize {
        self.bytes.iter().filter(|byte| byte.is_some()).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RomCheck {
    pub stored: u8,
    pub calculated: u8,
}

impl RomCheck {
    pub fn valid(self) -> bool {
        self.stored == self.calculated
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub format: Format,
    pub version: Option<u32>,
    pub protocol: String,
    pub identifier: Vec<Option<u8>>,
    pub rows: Vec<MemoryRow>,
    pub fields: Vec<(String, String)>,
    pub rom_check: Option<RomCheck>,
}

impl Memory {
    pub fn window(&self, rows: Range<usize>) -> &[MemoryRow] {
        let start = rows.start.min(self.rows.len());
        let end = rows.end.clamp(start, self.rows.len());
        &self.rows[start..end]
    }

    pub fn coverage(&self) -> [usize; 2] {
        self.rows.iter().fold([0, 0], |[known, total], row| {
            [known + row.known_bytes(), total + row.bytes.len()]
        })
    }
}

#[derive(Clone, Debug)]
pub enum Document {
    Memory(Memory),
    Trace(TracePreview),
    Signal(SignalPreview),
}

impl Document {
    pub fn format(&self) -> Format {
        match self {
            Self::Memory(memory) => memory.format,
            Self::Trace(_) => Format::ProxmarkTrace,
            Self::Signal(_) => Format::ProxmarkSignal,
        }
    }
}

fn candidate(path: &Path) -> Option<Format> {
    let ext = path.extension()?;
    [
        ("nfc", Format::FlipperNfc),
        ("rfid", Format::FlipperRfid),
        ("ibtn", Format::FlipperIButton),
        ("mfd", Format::MifareDump),
        ("eml", Format::ProxmarkDump),
        ("trace", Format::ProxmarkTrace),
        ("pm3", Format::ProxmarkSignal),
    ]
    .into_iter()
    .find_map(|(name, format)| ext.eq_ignore_ascii_case(name).then_some(format))
}

pub fn is_candidate_path(path: &Path) -> bool {
    candidate(path).is_some()
}

/// Use when the supplied bytes contain the whole file.
pub fn recognizes(bytes: &[u8], path: &Path) -> bool {
    recognizes_with_length(bytes, path, bytes.len() as u64)
}

/// Header/geometry detection using a bounded source prefix and its observed length.
pub fn recognizes_with_length(prefix: &[u8], path: &Path, file_bytes: u64) -> bool {
    let prefix = &prefix[..prefix.len().min(MAX_PROBE)];
    let whole = file_bytes == prefix.len() as u64;
    match candidate(path) {
        Some(format @ (Format::FlipperNfc | Format::FlipperRfid | Format::FlipperIButton)) => {
            flipper::identify(prefix).ok().map(|(format, _)| format) == Some(format)
        }
        Some(Format::MifareDump) => whole && memory::raw_geometry(prefix).is_some(),
        Some(Format::ProxmarkDump) => whole && memory::probe_hex(prefix),
        Some(Format::ProxmarkTrace) => trace::recognizes(prefix, whole),
        Some(Format::ProxmarkSignal) => signal::recognizes(prefix, whole),
        None => false,
    }
}

/// Reads a bounded initial document. The input should be a revision-checked, pinned file.
pub fn preview(
    path: &Path,
    mut input: impl BufRead + Seek,
    cancel: &AtomicBool,
) -> Result<Document, Error> {
    source::cancelled(cancel)?;
    input.seek(SeekFrom::Start(0))?;
    let format = candidate(path).ok_or(Error::Unsupported("filename extension"))?;
    match format {
        Format::ProxmarkTrace => trace::preview(&mut input, cancel).map(Document::Trace),
        Format::ProxmarkSignal => signal::preview(&mut input, cancel).map(Document::Signal),
        _ => {
            let bytes = read_memory(input, cancel)?;
            let memory = match format {
                Format::MifareDump => memory::raw(&bytes)?,
                Format::ProxmarkDump => memory::hex(&bytes)?,
                _ => flipper::parse(&bytes, format, cancel)?,
            };
            Ok(Document::Memory(memory))
        }
    }
}

fn read_memory(input: impl Read, cancel: &AtomicBool) -> Result<Vec<u8>, Error> {
    let mut input = input.take(MAX_MEMORY_BYTES + 1);
    let mut bytes = Vec::new();
    let mut block = [0; 8192];
    loop {
        source::cancelled(cancel)?;
        match input.read(&mut block) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&block[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    if bytes.len() as u64 > MAX_MEMORY_BYTES {
        return Err(Error::Limit("memory image bytes"));
    }
    Ok(bytes)
}

fn hex_bytes(text: &str) -> Result<Vec<Option<u8>>, Error> {
    let mut bytes = Vec::new();
    for word in text.split_ascii_whitespace() {
        let (pairs, tail) = word.as_bytes().as_chunks::<2>();
        if !tail.is_empty() {
            return Err(Error::Invalid("incomplete hexadecimal byte"));
        }
        for &pair in pairs {
            bytes.push(hex_byte(pair)?);
        }
    }
    Ok(bytes)
}

fn hex_byte([high, low]: [u8; 2]) -> Result<Option<u8>, Error> {
    if [high, low] == *b"??" || [high, low].eq_ignore_ascii_case(b"XX") {
        return Ok(None);
    }
    let high = char::from(high)
        .to_digit(16)
        .ok_or(Error::Invalid("invalid hexadecimal byte"))?;
    let low = char::from(low)
        .to_digit(16)
        .ok_or(Error::Invalid("invalid hexadecimal byte"))?;
    Ok(Some((high * 16 + low) as u8))
}

#[cfg(test)]
mod tests;
