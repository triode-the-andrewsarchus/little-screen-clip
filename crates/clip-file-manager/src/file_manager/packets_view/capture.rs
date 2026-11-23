use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::engineering::packets::{
    self, Format, Index, Packet, PacketRow, Protocol, Reader, Summary,
};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    net::IpAddr,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const SCAN_LIMIT: u64 = 4_000_000;
const MATCH_LIMIT: usize = 64_000;
const SCAN_PAGE: u64 = 512;

pub(super) struct Capture {
    path: PathBuf,
    revision: FileSource,
    pub check: ReadCheck,
}

impl Capture {
    pub fn open(path: &Path, cancel: &AtomicBool) -> Result<(Self, Summary), String> {
        let check = ReadCheck::default();
        let (input, revision) = check.open(path, None, cancel)?;
        let summary = packets::summary(BufReader::new(&input), cancel).map_err(message)?;
        let capture = Self {
            path: path.to_owned(),
            revision,
            check,
        };
        capture.validate(&input, cancel)?;
        Ok((capture, summary))
    }

    fn validate(&self, file: &File, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Packet capture read cancelled".into());
        }
        self.revision.check_file(file)?;
        if FileSource::capture(&self.path)? != self.revision {
            return Err("Packet capture changed; reopen it before reading".into());
        }
        self.check.validate(cancel)
    }

    fn read<T>(
        &self,
        cancel: &AtomicBool,
        work: impl FnOnce(&mut Reader<BufReader<&File>>) -> Result<T, packets::Error>,
    ) -> Result<T, String> {
        let (input, _) = self.check.open(&self.path, Some(&self.revision), cancel)?;
        let mut reader = Reader::open(BufReader::new(&input)).map_err(message)?;
        let result = work(&mut reader).map_err(message)?;
        self.validate(&input, cancel)?;
        Ok(result)
    }

    pub fn index(&self, cancel: &AtomicBool) -> Result<Index, String> {
        self.read(cancel, |reader| reader.build_index(cancel))
    }

    pub fn rows(
        &self,
        index: &Index,
        rows: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<PacketRow>, String> {
        self.read(cancel, |reader| reader.read_window(index, rows, cancel))
    }

    pub fn packet(
        &self,
        index: Option<&Index>,
        row: u64,
        cancel: &AtomicBool,
    ) -> Result<Packet, String> {
        self.read(cancel, |reader| {
            if let Some(index) = index {
                return reader.read_packet(index, row, cancel);
            }
            for point in 0..=row.min(255) {
                let packet = reader
                    .next_packet(cancel)?
                    .ok_or(packets::Error::Truncated)?;
                if point == row {
                    return Ok(packet);
                }
            }
            Err(packets::Error::Limit(
                "packet lies beyond the initial preview",
            ))
        })
    }

    pub fn copy(
        &self,
        index: &Index,
        rows: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let mut count = 0;
        super::super::preview_copy::copy_file(
            &self.path,
            "packets",
            index.format().kind.extension(),
            |file| {
                let mut file = BufWriter::new(file);
                count = self.read(cancel, |reader| {
                    reader.export(index, rows, &mut file, cancel)
                })?;
                file.flush().map_err(message)
            },
        )?;
        Ok(format!("Copied {count} packets"))
    }

    pub fn filter(
        &self,
        index: &Index,
        filter: &Filter,
        cancel: &AtomicBool,
    ) -> Result<Filtered, String> {
        self.read(cancel, |reader| {
            let end = index.points().min(SCAN_LIMIT);
            let mut result = Filtered {
                rows: Vec::new(),
                scanned: 0,
            };
            while result.scanned < end {
                let next = (result.scanned + SCAN_PAGE).min(end);
                let page = reader.read_window(index, result.scanned..next, cancel)?;
                for row in page {
                    result.scanned = row.header.row + 1;
                    if filter.matches(&row) {
                        result.rows.push(row);
                        if result.rows.len() == MATCH_LIMIT {
                            return Ok(result);
                        }
                    }
                }
            }
            Ok(result)
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Filter {
    pub protocol: Option<Protocol>,
    endpoint: EndpointFilter,
}

#[derive(Clone, PartialEq, Eq)]
enum EndpointFilter {
    Any,
    Address(IpAddr),
    Port(u16),
    Fragment(String),
}

impl Filter {
    pub fn new(protocol: Option<Protocol>, endpoint: &str) -> Self {
        let endpoint = endpoint.trim();
        let endpoint = if endpoint.is_empty() {
            EndpointFilter::Any
        } else if let Ok(address) = endpoint.parse() {
            EndpointFilter::Address(address)
        } else if let Ok(port) = endpoint.parse() {
            EndpointFilter::Port(port)
        } else {
            EndpointFilter::Fragment(endpoint.to_ascii_lowercase())
        };
        Self { protocol, endpoint }
    }

    pub fn active(&self) -> bool {
        self.protocol.is_some() || self.endpoint != EndpointFilter::Any
    }

    fn matches(&self, row: &PacketRow) -> bool {
        use packets::Endpoint;
        let data = row.protocol;
        if self
            .protocol
            .is_some_and(|protocol| protocol != data.protocol)
        {
            return false;
        }
        let endpoints = [data.source, data.destination];
        match &self.endpoint {
            EndpointFilter::Any => true,
            EndpointFilter::Port(port) => {
                [data.source_port, data.destination_port].contains(&Some(*port))
            }
            EndpointFilter::Address(address) => match address {
                IpAddr::V4(address) => endpoints.contains(&Some(Endpoint::V4(*address))),
                IpAddr::V6(address) => endpoints.contains(&Some(Endpoint::V6(*address))),
            },
            EndpointFilter::Fragment(fragment) => endpoints
                .into_iter()
                .flatten()
                .any(|endpoint| endpoint.to_string().contains(fragment)),
        }
    }
}

pub(super) struct Filtered {
    pub rows: Vec<PacketRow>,
    pub scanned: u64,
}

pub(super) fn name(format: Format) -> &'static str {
    match format.kind {
        packets::Kind::Pcap => "PCAP",
        packets::Kind::PcapNg => "PCAPNG",
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
