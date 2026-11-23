//! File-backed classic PCAP and PCAPNG inspection for recorded network packets.

mod export;
mod index;
mod legacy;
mod ng;
mod protocol;
mod source;

pub use index::{ActivityBin, Index};
pub use protocol::{Endpoint, Protocol, ProtocolFields};

use source::{Source, cancelled};
use std::{
    ffi::OsStr,
    io::{self, BufRead, Cursor, Seek, Write},
    ops::Range,
    path::Path,
    sync::atomic::AtomicBool,
};
use thiserror::Error;

const PREFIX_BYTES: u64 = 256 * 1024;
const PREFIX_ROWS: usize = 256;
const DETAIL_ROWS: u64 = 4096;
const PACKET_BYTES: usize = 16 * 1024 * 1024;
const PROTOCOL_BYTES: usize = 512;
const NG_MAGIC: [u8; 4] = [0x0a, 0x0d, 0x0d, 0x0a];

#[derive(Debug, Error)]
pub enum Error {
    #[error("Cannot read packet capture: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid packet capture: {0}")]
    Invalid(&'static str),
    #[error("Unsupported packet capture: {0}")]
    Unsupported(&'static str),
    #[error("Packet capture ended inside a record")]
    Truncated,
    #[error("Packet capture limit exceeded: {0}")]
    Limit(&'static str),
    #[error("Packet capture prefix limit reached")]
    PrefixLimit,
    #[error("Packet capture read cancelled")]
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pcap,
    PcapNg,
}

impl Kind {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pcap => "pcap",
            Self::PcapNg => "pcapng",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Plain,
    Gzip,
    Zstd,
    Lz4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub kind: Kind,
    pub compression: Compression,
}

/// Interface numbers are local to a PCAPNG section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InterfaceId {
    pub section: u32,
    pub ordinal: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Decimal(u8),
    Binary(u8),
}

impl Resolution {
    fn units_per_second(self) -> Option<u128> {
        match self {
            Self::Decimal(exponent) => 10_u128.checked_pow(u32::from(exponent)),
            Self::Binary(exponent) => 1_u128.checked_shl(u32::from(exponent)),
        }
    }
}

/// Exact source timestamp, independently of the rounded nanosecond view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timestamp {
    pub ticks: u64,
    pub resolution: Resolution,
    pub offset_seconds: i64,
}

impl Timestamp {
    pub fn unix_nanoseconds(self) -> i128 {
        let ticks = u128::from(self.ticks) * 1_000_000_000;
        let within = self
            .resolution
            .units_per_second()
            .map_or(0, |units| ticks / units);
        i128::from(self.offset_seconds) * 1_000_000_000 + within as i128
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub id: InterfaceId,
    pub link_type: u16,
    pub snap_len: u32,
    pub resolution: Resolution,
    pub offset_seconds: i64,
    pub name: Option<String>,
    pub fcs_bits: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    pub row: u64,
    pub interface: InterfaceId,
    pub link_type: u16,
    pub time: Option<Timestamp>,
    pub captured_len: u32,
    pub original_len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PacketRow {
    pub header: PacketHeader,
    pub protocol: ProtocolFields,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub header: PacketHeader,
    pub bytes: Vec<u8>,
}

impl Packet {
    pub fn protocol(&self) -> ProtocolFields {
        protocol::inspect(self.header.link_type, &self.bytes)
    }
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub format: Format,
    pub interfaces: Vec<Interface>,
    pub packets: Vec<PacketRow>,
    pub complete: bool,
}

impl Summary {
    /// Returns timestamp activity for the packet rows present in this prefix.
    pub fn activity(&self) -> Vec<ActivityBin> {
        index::activity(self.packets.iter().map(|row| row.header))
    }
}

pub fn is_candidate_path(path: &Path) -> bool {
    let valid = |extension: &OsStr| {
        ["pcap", "pcapng", "cap", "ntar"]
            .into_iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
    };
    let Some(extension) = path.extension() else {
        return false;
    };
    valid(extension)
        || (["gz", "zst", "zstd", "lz4"]
            .into_iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            && path
                .file_stem()
                .and_then(|stem| Path::new(stem).extension())
                .is_some_and(valid))
}

pub fn recognizes(prefix: &[u8], path: &Path) -> bool {
    is_candidate_path(path)
        && Reader::open(Cursor::new(&prefix[..prefix.len().min(64 * 1024)])).is_ok()
}

pub fn summary(input: impl BufRead + Seek, cancel: &AtomicBool) -> Result<Summary, Error> {
    let mut reader = Reader::open(input)?;
    reader.source.limit = Some(PREFIX_BYTES);
    let mut packets = Vec::new();
    let mut scratch = Vec::new();
    let complete = loop {
        let take = if packets.len() == PREFIX_ROWS {
            0
        } else {
            PROTOCOL_BYTES
        };
        match reader.next(take, &mut scratch, cancel) {
            Ok(Some(record)) if take != 0 => packets.push(record.row(&scratch)),
            Ok(Some(_)) | Err(Error::PrefixLimit) => break false,
            Ok(None) => break true,
            Err(error) => return Err(error),
        }
    };
    Ok(Summary {
        format: reader.format(),
        interfaces: reader.interfaces().cloned().collect(),
        packets,
        complete,
    })
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Order {
    Little,
    Big,
}

impl Order {
    fn u16(self, bytes: [u8; 2]) -> u16 {
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    fn u32(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    fn u64(self, bytes: [u8; 8]) -> u64 {
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }
}

fn at<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], Error> {
    bytes
        .get(offset..)
        .and_then(|bytes| bytes.first_chunk::<N>())
        .copied()
        .ok_or(Error::Truncated)
}

#[derive(Clone, Copy, Debug)]
struct Record {
    header: PacketHeader,
    offset: u64,
}

impl Record {
    fn row(self, bytes: &[u8]) -> PacketRow {
        PacketRow {
            header: self.header,
            protocol: protocol::inspect(self.header.link_type, bytes),
        }
    }
}

enum Container {
    Legacy(legacy::Legacy),
    Ng(ng::Ng),
}

/// Streaming reader. The caller supplies a pinned ordinary file and checks its revision.
pub struct Reader<R: BufRead> {
    source: Source<R>,
    container: Container,
    row: u64,
}

impl<R: BufRead + Seek> Reader<R> {
    pub fn open(input: R) -> Result<Self, Error> {
        let mut source = Source::open(input)?;
        let signature = source.read::<4>()?;
        let container = if signature == NG_MAGIC {
            Container::Ng(ng::Ng::open(&mut source)?)
        } else {
            Container::Legacy(legacy::Legacy::open(signature, &mut source)?)
        };
        Ok(Self {
            source,
            container,
            row: 0,
        })
    }

    pub fn format(&self) -> Format {
        Format {
            kind: match self.container {
                Container::Legacy(_) => Kind::Pcap,
                Container::Ng(_) => Kind::PcapNg,
            },
            compression: self.source.compression,
        }
    }

    pub fn interfaces(&self) -> Box<dyn Iterator<Item = &Interface> + '_> {
        match &self.container {
            Container::Legacy(legacy) => Box::new(std::iter::once(&legacy.interface)),
            Container::Ng(ng) => Box::new(ng.interfaces()),
        }
    }

    pub fn next_packet(&mut self, cancel: &AtomicBool) -> Result<Option<Packet>, Error> {
        let mut bytes = Vec::new();
        Ok(self
            .next(PACKET_BYTES, &mut bytes, cancel)?
            .map(|record| Packet {
                header: record.header,
                bytes,
            }))
    }

    pub fn build_index(&mut self, cancel: &AtomicBool) -> Result<Index, Error> {
        index::build(self, cancel)
    }

    pub fn read_window(
        &mut self,
        index: &Index,
        rows: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<PacketRow>, Error> {
        index::window(self, index, rows, cancel)
    }

    pub fn read_packet(
        &mut self,
        index: &Index,
        row: u64,
        cancel: &AtomicBool,
    ) -> Result<Packet, Error> {
        self.position_at(index, row, cancel)?;
        self.next_packet(cancel)?.ok_or(Error::Truncated)
    }

    pub fn export(
        &mut self,
        index: &Index,
        rows: Range<u64>,
        output: impl Write,
        cancel: &AtomicBool,
    ) -> Result<u64, Error> {
        export::window(self, index, rows, output, cancel)
    }

    fn next(
        &mut self,
        take: usize,
        scratch: &mut Vec<u8>,
        cancel: &AtomicBool,
    ) -> Result<Option<Record>, Error> {
        cancelled(cancel)?;
        scratch.clear();
        let record = match &mut self.container {
            Container::Legacy(legacy) => {
                legacy.next(&mut self.source, self.row, take, scratch, cancel)
            }
            Container::Ng(ng) => ng.next(&mut self.source, self.row, take, scratch, cancel),
        }?;
        if record.is_some() {
            self.row += 1;
        }
        Ok(record)
    }

    fn rewind(&mut self, cancel: &AtomicBool) -> Result<(), Error> {
        self.source.seek(0, cancel)?;
        self.row = 0;
        let signature = self.source.read::<4>()?;
        self.container = if signature == NG_MAGIC {
            Container::Ng(ng::Ng::open(&mut self.source)?)
        } else {
            Container::Legacy(legacy::Legacy::open(signature, &mut self.source)?)
        };
        Ok(())
    }

    fn position_at(&mut self, index: &Index, row: u64, cancel: &AtomicBool) -> Result<(), Error> {
        index.position(self, row, cancel)
    }
}

fn read_payload(
    source: &mut Source<impl BufRead + Seek>,
    captured: u32,
    take: usize,
    scratch: &mut Vec<u8>,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let length = captured as usize;
    if take == PACKET_BYTES && length > PACKET_BYTES {
        return Err(Error::Limit("selected packet bytes"));
    }
    let count = length.min(take);
    scratch.resize(count, 0);
    source.fill(scratch)?;
    source.skip((length - count) as u64, cancel)
}

#[cfg(test)]
mod tests;
