//! Offline CAN bus captures and sparse frame indexes.

mod blf;
pub mod dbc;
mod index;
#[cfg(test)]
mod tests;
mod text;

use std::{
    fmt,
    fs::{File, Metadata},
    io::{self, Read},
    ops::Range,
    os::unix::fs::{FileExt, MetadataExt},
    sync::atomic::{AtomicBool, Ordering},
};

pub use index::{ActivityBin, ChannelSummary, Filter, Filtered, IdentifierSummary, Index, Summary};

/// Maximum file prefix needed to identify and initialize a CAN capture.
pub const HEADER_BYTES: usize = 8192;
const MAX_DETAIL_ROWS: usize = 4096;
const MAX_FRAME_BYTES: usize = 2048;

/// Supported CAN log container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    VectorAsc,
    VectorBlf,
    PcanTrc,
    Candump,
}

impl Format {
    /// Display name of the file format.
    pub fn label(self) -> &'static str {
        match self {
            Self::VectorAsc => "Vector ASC",
            Self::VectorBlf => "Vector BLF",
            Self::PcanTrc => "PEAK PCAN TRC",
            Self::Candump => "SocketCAN log",
        }
    }
}

/// Identifies a known CAN format from content, including renamed `.trace` files.
pub fn identify(prefix: &[u8]) -> Option<Format> {
    if blf::header_size(prefix).is_some() {
        return Some(Format::VectorBlf);
    }
    text::Configuration::recognize(prefix).map(|configuration| configuration.format())
}

/// A numbered logging channel or a named SocketCAN interface.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    Number(u16),
    Interface(String),
    Unknown,
}

impl fmt::Display for Channel {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(output, "CAN {number}"),
            Self::Interface(name) => output.write_str(name),
            Self::Unknown => output.write_str("Unknown channel"),
        }
    }
}

/// An 11-bit standard or 29-bit extended arbitration identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Identifier {
    pub value: u32,
    pub extended: bool,
}

impl Identifier {
    fn new(value: u32, extended: bool) -> Option<Self> {
        let max = if extended { 0x1fff_ffff } else { 0x7ff };
        (value <= max).then_some(Self { value, extended })
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.extended {
            write!(output, "{:08X}x", self.value)
        } else {
            write!(output, "{:03X}", self.value)
        }
    }
}

/// Direction from the logging interface's perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Rx,
    Tx,
    Unknown,
}

impl Direction {
    fn parse(text: &str) -> Self {
        if text.eq_ignore_ascii_case("rx") {
            Self::Rx
        } else if text.eq_ignore_ascii_case("tx") {
            Self::Tx
        } else {
            Self::Unknown
        }
    }
}

/// Traffic and status event types retained from the capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Classic,
    Flexible {
        bitrate_switch: bool,
        error_state: bool,
    },
    CanXl,
    J1939,
    Remote,
    Error,
    Status,
}

/// One captured frame or bus event in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub row: u64,
    /// Nanoseconds on the recording's timebase. Can contain pre-trigger times.
    pub timestamp_ns: i128,
    pub channel: Channel,
    pub kind: FrameKind,
    pub id: Option<Identifier>,
    pub direction: Direction,
    /// Data length code when the source records it.
    pub dlc: Option<u16>,
    /// Number of bytes represented by the frame.
    pub data_len: u16,
    /// Only the bytes actually present in the file.
    pub data: Vec<u8>,
}

impl Frame {
    fn new(timestamp_ns: i128, channel: Channel, kind: FrameKind) -> Self {
        Self {
            row: 0,
            timestamp_ns,
            channel,
            kind,
            id: None,
            direction: Direction::Unknown,
            dlc: None,
            data_len: 0,
            data: Vec::new(),
        }
    }
}

/// A supported capture using an already guarded, ordinary file handle.
#[derive(Debug)]
pub struct Capture {
    file: File,
    byte_len: u64,
    configuration: Configuration,
    identity: FileIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
    len: u64,
    modified: [i64; 2],
    changed: [i64; 2],
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            modified: [metadata.mtime(), metadata.mtime_nsec()],
            changed: [metadata.ctime(), metadata.ctime_nsec()],
        }
    }
}

#[derive(Debug)]
enum Configuration {
    Blf(blf::Configuration),
    Text(text::Configuration),
}

impl Capture {
    /// Opens a capture using only its bounded prefix.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported, invalid, or unreadable input.
    pub fn open(file: File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("CAN capture must be a regular file".into());
        }
        let byte_len = metadata.len();
        let mut prefix = [0; HEADER_BYTES];
        let prefix = &mut prefix[..byte_len.min(HEADER_BYTES as u64) as usize];
        file.read_exact_at(prefix, 0)
            .map_err(|error| error.to_string())?;
        let configuration = if prefix.starts_with(b"LOGG") {
            Configuration::Blf(blf::Configuration::open(prefix, byte_len)?)
        } else {
            Configuration::Text(
                text::Configuration::recognize(prefix).ok_or("Unrecognized CAN recording")?,
            )
        };
        Ok(Self {
            file,
            byte_len,
            configuration,
            identity: FileIdentity::from_metadata(&metadata),
        })
    }

    /// Returns the detected container type.
    pub fn format(&self) -> Format {
        match &self.configuration {
            Configuration::Blf(_) => Format::VectorBlf,
            Configuration::Text(configuration) => configuration.format(),
        }
    }

    /// Returns the size of the retained capture file.
    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }

    /// Summarizes a bounded beginning of the file for a quick preview.
    ///
    /// # Errors
    ///
    /// Returns an error for damaged data, I/O failure, or cancellation.
    pub fn preview(&self, cancel: &AtomicBool) -> Result<Summary, String> {
        index::preview(self, cancel)
    }

    /// Streams the capture to build a bounded index on a background worker.
    ///
    /// # Errors
    ///
    /// Returns an error for damaged data, I/O failure, or cancellation.
    pub fn index(&self, cancel: &AtomicBool) -> Result<Index, String> {
        index::build(self, cancel)
    }

    /// Reads at most `limit` frames inside a half-open timestamp interval.
    ///
    /// Frames retain file order even when transmit and receive timestamps cross.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid index, excessive page request, or a read failure.
    pub fn frames(
        &self,
        index: &Index,
        window: Range<i128>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<Frame>, String> {
        let frames = index::frames(self, index, window, limit, cancel)?;
        self.validate_identity()?;
        Ok(frames)
    }

    /// Reads exact frame ordinals for a paginated table or a selected region.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid index, excessive page request, or a read failure.
    pub fn rows(
        &self,
        index: &Index,
        rows: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<Frame>, String> {
        let frames = index::rows(self, index, rows, cancel)?;
        self.validate_identity()?;
        Ok(frames)
    }

    /// Finds matching frame headers for a bounded filter result.
    ///
    /// The returned rows retain their original ordinals. Read actual payload
    /// bytes on demand with [Self::rows].
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid index, cancellation, or a read failure.
    pub fn filter(
        &self,
        index: &Index,
        filter: &Filter,
        cancel: &AtomicBool,
    ) -> Result<Filtered, String> {
        index::filter(self, index, filter, cancel)
    }

    fn validate_identity(&self) -> Result<(), String> {
        let metadata = self.file.metadata().map_err(|error| error.to_string())?;
        if FileIdentity::from_metadata(&metadata) != self.identity {
            return Err("CAN recording changed while it was open".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum Location {
    Text { offset: u64, clock: i128 },
    Blf { container: u64, decoded: usize },
}

enum Scanner<'a> {
    Text(text::Scanner<'a>),
    Blf(blf::Scanner<'a>),
}

impl<'a> Scanner<'a> {
    fn new(
        capture: &'a Capture,
        resume: Option<Location>,
        byte_limit: u64,
    ) -> Result<Self, String> {
        match &capture.configuration {
            Configuration::Text(configuration) => {
                text::Scanner::new(capture, configuration, resume, byte_limit).map(Self::Text)
            }
            Configuration::Blf(configuration) => {
                blf::Scanner::new(capture, configuration, resume, byte_limit).map(Self::Blf)
            }
        }
    }

    fn next(
        &mut self,
        bytes: bool,
        cancel: &AtomicBool,
    ) -> Result<Option<(Location, Frame)>, String> {
        match self {
            Self::Text(reader) => reader.next(bytes, cancel),
            Self::Blf(reader) => reader.next(bytes, cancel),
        }
    }

    fn covered_bytes(&self) -> u64 {
        match self {
            Self::Text(reader) => reader.covered_bytes(),
            Self::Blf(reader) => reader.covered_bytes(),
        }
    }
}

struct Positioned<'a> {
    file: &'a File,
    offset: u64,
    end: u64,
}

impl Read for Positioned<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = (buffer.len() as u64).min(self.end.saturating_sub(self.offset));
        let count = self
            .file
            .read_at(&mut buffer[..length as usize], self.offset)?;
        self.offset += count as u64;
        Ok(count)
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("CAN capture read canceled".into());
    }
    Ok(())
}

fn dlc_bytes(dlc: u16) -> Option<u16> {
    Some(match dlc {
        0..=8 => dlc,
        9 => 12,
        10 => 16,
        11 => 20,
        12 => 24,
        13 => 32,
        14 => 48,
        15 => 64,
        _ => return None,
    })
}
