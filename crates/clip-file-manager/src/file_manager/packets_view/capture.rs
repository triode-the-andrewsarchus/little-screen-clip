use super::display_filter::{self, DisplayFilter};
use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::engineering::packets::{
    self, Conversation, ConversationKey, Conversations, Format, Index, Packet, PacketRow, Protocol,
    Reader, Summary, TcpPoint, TcpTracker,
};
use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    net::IpAddr,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const SCAN_LIMIT: u64 = 4_000_000;
const MATCH_LIMIT: usize = 64_000;
const SCAN_PAGE: u64 = 512;
const CONVERSATION_SCAN_LIMIT: u64 = 1_000_000;
const CONVERSATION_LIMIT: usize = 32_768;
const TCP_FLOW_LIMIT: usize = 8_192;

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
        let rows = filter.rows(index.points());
        let display = self.prepare_display(&filter.display, rows.end, cancel)?;
        self.read(cancel, |reader| {
            let mut result = Filtered {
                rows: Vec::new(),
                scanned: 0,
                scope_size: filter.scope_size(index.points()),
            };
            result.scanned = visit_rows(reader, index, rows, cancel, |row| {
                if filter.matches(&row) && display.matches(&row) {
                    result.rows.push(row);
                }
                result.rows.len() < MATCH_LIMIT
            })?;
            Ok(result)
        })
    }

    pub fn conversations(
        &self,
        index: &Index,
        scope: &ConversationScope,
        cancel: &AtomicBool,
    ) -> Result<ConversationSummary, String> {
        self.read(cancel, |reader| {
            let mut groups = Conversations::new(CONVERSATION_LIMIT);
            let scanned = visit_rows(reader, index, scope.rows.clone(), cancel, |row| {
                if scope.contains(&row) {
                    groups.observe(&row);
                }
                true
            })?;
            Ok(ConversationSummary {
                scope: scope.clone(),
                scanned,
                omitted: groups.omitted_packets,
                unclassified: groups.unclassified_packets,
                groups: groups.ranked(),
            })
        })
    }

    pub fn tcp_flow(
        &self,
        index: &Index,
        flow: &TcpFlowQuery,
        cancel: &AtomicBool,
    ) -> Result<TcpFlow, String> {
        self.read(cancel, |reader| {
            let mut tracker = TcpTracker::new(flow.key);
            let mut points = Vec::new();
            let scope = &flow.scope;
            let scanned = visit_rows(reader, index, scope.rows.clone(), cancel, |row| {
                if scope.contains(&row)
                    && let Some(point) = tracker.observe(&row)
                {
                    points.push(point);
                }
                points.len() < TCP_FLOW_LIMIT
            })?;
            Ok(TcpFlow { points, scanned })
        })
    }

    fn prepare_display(
        &self,
        input: &str,
        limit: u64,
        cancel: &AtomicBool,
    ) -> Result<DisplayMatches, String> {
        if input.is_empty() || limit == 0 {
            return Ok(DisplayMatches::All);
        }
        if let Some(filter) = DisplayFilter::compile(input) {
            return Ok(DisplayMatches::Native(filter));
        }
        if limit > SCAN_LIMIT {
            return Err(format!(
                "This expression needs TShark, which searches the first {SCAN_LIMIT} packets"
            ));
        }
        self.tshark_frames(input, limit, cancel)
            .map(DisplayMatches::Frames)
    }

    fn tshark_frames(
        &self,
        input: &str,
        limit: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u32>, String> {
        let tshark = little_clip_core::command_available("tshark").ok_or(
            "This expression requires TShark. The built-in filter supports protocol names, IP addresses, TCP/UDP ports, TCP flags, frame lengths, and boolean operators."
        )?;
        let (file, _) = self.check.open(&self.path, Some(&self.revision), cancel)?;
        let source = file.try_clone().map_err(message)?;
        let output = little_clip_core::CommandRequest::new(tshark)
            .args(["-n", "-r"])
            .arg_owned_content_file(source)
            .map_err(message)?
            .arg("-c")
            .arg(limit.to_string())
            .arg("-Y")
            .arg(input)
            .args(["-T", "fields", "-e", "frame.number"])
            .timeout(Duration::from_secs(80))
            .max_output_bytes(32 * 1024 * 1024)
            .die_with_parent()
            .run_cancellable(cancel)
            .map_err(message)?;
        self.validate(&file, cancel)?;
        if !output.status.success() {
            let detail = output.stderr_lossy();
            let detail = detail.trim();
            return Err(format!(
                "Display filter: {}",
                &detail[..detail.floor_char_boundary(detail.len().min(512))]
            ));
        }
        if output.stdout_truncated {
            return Err("Display filter exceeded the result limit".into());
        }
        frame_numbers(&output.stdout, limit)
    }
}

fn visit_rows(
    reader: &mut Reader<BufReader<&File>>,
    index: &Index,
    mut rows: Range<u64>,
    cancel: &AtomicBool,
    mut visit: impl FnMut(PacketRow) -> bool,
) -> Result<u64, packets::Error> {
    let first = rows.start;
    while rows.start < rows.end {
        let end = (rows.start + SCAN_PAGE).min(rows.end);
        for row in reader.read_window(index, rows.start..end, cancel)? {
            let scanned = row.header.row - first + 1;
            if !visit(row) {
                return Ok(scanned);
            }
        }
        rows.start = end;
    }
    Ok(rows.end - first)
}

fn frame_numbers(output: &[u8], limit: u64) -> Result<Vec<u32>, String> {
    let text = std::str::from_utf8(output).map_err(message)?;
    let mut rows = Vec::new();
    for number in text.lines() {
        let number: u32 = number.parse().map_err(message)?;
        if number == 0
            || u64::from(number) > limit
            || rows.last().is_some_and(|&last| last >= number)
        {
            return Err("TShark returned inconsistent frame numbers".into());
        }
        rows.push(number);
    }
    Ok(rows)
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Filter {
    pub protocol: Option<Protocol>,
    endpoint: EndpointFilter,
    display: String,
    conversation: Option<(ConversationKey, ConversationScope)>,
    time_restriction: Option<TimeRestriction>,
}

#[derive(Clone, Default, PartialEq, Eq)]
enum EndpointFilter {
    #[default]
    Any,
    Address(IpAddr),
    Port(u16),
    Fragment(String),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TimeRestriction {
    pub bounds: [i128; 2],
    pub rows: Range<u64>,
}

impl EndpointFilter {
    fn parse(text: &str) -> Self {
        let text = text.trim();
        if text.is_empty() {
            Self::Any
        } else if let Ok(address) = text.parse() {
            Self::Address(address)
        } else if let Ok(port) = text.parse() {
            Self::Port(port)
        } else {
            Self::Fragment(text.to_ascii_lowercase())
        }
    }

    fn description(&self) -> Option<String> {
        match self {
            Self::Any => None,
            Self::Address(address) => Some(address.to_string()),
            Self::Port(port) => Some(format!("Port {port}")),
            Self::Fragment(text) => Some(text.clone()),
        }
    }
}

impl Filter {
    pub fn with_text(mut self, endpoint: &str, display: &str) -> Self {
        self.endpoint = EndpointFilter::parse(endpoint);
        self.display = display.trim().to_owned();
        self
    }

    pub fn text_matches(&self, endpoint: &str, display: &str) -> bool {
        self.display == display.trim() && self.endpoint == EndpointFilter::parse(endpoint)
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn endpoint_label(&self) -> Option<String> {
        self.endpoint.description()
    }

    pub fn with_conversation(
        mut self,
        conversation: Option<(ConversationKey, ConversationScope)>,
    ) -> Self {
        self.conversation = conversation;
        self
    }

    pub fn conversation(&self) -> Option<ConversationKey> {
        self.conversation.as_ref().map(|(key, _)| *key)
    }

    pub fn with_time(mut self, restriction: Option<TimeRestriction>) -> Self {
        self.time_restriction = restriction;
        self
    }

    pub fn time(&self) -> Option<[i128; 2]> {
        self.time_restriction.as_ref().map(|time| time.bounds)
    }

    pub fn active(&self) -> bool {
        self.protocol.is_some()
            || self.endpoint != EndpointFilter::Any
            || !self.display.is_empty()
            || self.conversation.is_some()
            || self.time_restriction.is_some()
    }

    fn source_span(&self, total: u64) -> Range<u64> {
        let mut range = 0..total;
        if let Some((_, scope)) = &self.conversation {
            range = intersect_rows(
                range,
                scope.rows.start..scope.rows.start.saturating_add(scope.total),
            );
        }
        if let Some(time) = &self.time_restriction {
            range = intersect_rows(range, time.rows.clone());
        }
        range
    }

    fn rows(&self, total: u64) -> Range<u64> {
        let range = self.source_span(total);
        let limit = if self.conversation.is_some() {
            CONVERSATION_SCAN_LIMIT
        } else {
            SCAN_LIMIT
        };
        range.start..range.end.min(range.start.saturating_add(limit))
    }

    fn scope_size(&self, total: u64) -> u64 {
        let range = self.source_span(total);
        range.end - range.start
    }

    fn matches(&self, row: &PacketRow) -> bool {
        if let Some((key, scope)) = &self.conversation
            && (!scope.contains(row)
                || !ConversationKey::identify(row.protocol).is_some_and(|(found, _)| found == *key))
        {
            return false;
        }
        if let Some(time) = &self.time_restriction
            && !has_time(row, time.bounds)
        {
            return false;
        }
        use packets::Endpoint;
        let data = row.protocol;
        if self
            .protocol
            .is_some_and(|protocol| !display_filter::has_protocol(row, protocol))
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

fn intersect_rows(left: Range<u64>, right: Range<u64>) -> Range<u64> {
    let start = left.start.max(right.start);
    start..left.end.min(right.end).max(start)
}

fn has_time(row: &PacketRow, [low, high]: [i128; 2]) -> bool {
    row.header
        .time
        .is_some_and(|time| (low..=high).contains(&time.unix_nanoseconds()))
}

enum DisplayMatches {
    All,
    Native(DisplayFilter),
    Frames(Vec<u32>),
}

impl DisplayMatches {
    fn matches(&self, row: &PacketRow) -> bool {
        match self {
            Self::All => true,
            Self::Native(filter) => filter.matches(row),
            Self::Frames(frames) => frames.binary_search(&((row.header.row + 1) as u32)).is_ok(),
        }
    }
}

pub(super) struct Filtered {
    pub rows: Vec<PacketRow>,
    pub scanned: u64,
    pub scope_size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConversationScope {
    pub rows: Range<u64>,
    pub total: u64,
    pub time: Option<[i128; 2]>,
}

impl ConversationScope {
    pub fn new(rows: Range<u64>, time: Option<[i128; 2]>) -> Self {
        Self {
            total: rows.end - rows.start,
            rows: rows.start..(rows.start.saturating_add(CONVERSATION_SCAN_LIMIT)).min(rows.end),
            time,
        }
    }

    pub fn around(total: u64, row: u64) -> Self {
        let start = row / CONVERSATION_SCAN_LIMIT * CONVERSATION_SCAN_LIMIT;
        Self::new(start..total, None)
    }

    fn contains(&self, row: &PacketRow) -> bool {
        self.time.is_none_or(|bounds| has_time(row, bounds))
    }
}

pub(super) struct ConversationSummary {
    pub scope: ConversationScope,
    pub groups: Vec<Conversation>,
    pub scanned: u64,
    pub omitted: u64,
    pub unclassified: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TcpFlowQuery {
    pub key: ConversationKey,
    pub scope: ConversationScope,
}

pub(super) struct TcpFlow {
    pub points: Vec<TcpPoint>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_restrictions_preserve_the_predicate_and_inspect_only_their_source_range() {
        let query = Filter::default().with_text("443", "tcp");
        let narrowed = query.clone().with_time(Some(TimeRestriction {
            bounds: [20, 40],
            rows: 100..200,
        }));
        assert_eq!(narrowed.rows(250), 100..200);
        assert_eq!(narrowed.scope_size(250), 100);
        assert_eq!(narrowed.with_time(None).display(), "tcp");
        assert!(query.text_matches("443", "tcp"));

        let mut row = PacketRow {
            header: packets::PacketHeader {
                row: 170,
                interface: packets::InterfaceId {
                    section: 0,
                    ordinal: 0,
                },
                link_type: 1,
                time: None,
                captured_len: 60,
                original_len: 60,
            },
            protocol: packets::ProtocolFields {
                destination_port: Some(443),
                ..Default::default()
            },
        };
        let filter = query.with_time(Some(TimeRestriction {
            bounds: [20, 40],
            rows: 100..200,
        }));
        assert!(!filter.matches(&row));
        row.header.time = Some(packets::Timestamp {
            ticks: 30,
            resolution: packets::Resolution::Decimal(9),
            offset_seconds: 0,
        });
        assert!(filter.matches(&row));
        row.header.time.as_mut().unwrap().ticks = 41;
        assert!(!filter.matches(&row));
    }

    #[test]
    fn narrowing_a_bounded_scan_keeps_the_correct_coverage_denominator() {
        let filter = Filter::default().with_time(Some(TimeRestriction {
            bounds: [0, 100],
            rows: 1_000_000..9_000_000,
        }));
        assert_eq!(filter.rows(10_000_000), 1_000_000..1_000_000 + SCAN_LIMIT);
        assert_eq!(filter.scope_size(10_000_000), 8_000_000);
    }
}
