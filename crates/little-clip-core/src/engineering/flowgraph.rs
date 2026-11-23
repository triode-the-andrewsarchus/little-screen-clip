//! Static inspection of saved GNU Radio Companion signal flowgraphs.

mod layout;
mod rates;
#[cfg(test)]
mod tests;
mod xml;
mod yaml;

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    os::unix::fs::FileExt,
    sync::atomic::{AtomicBool, Ordering},
};

pub use layout::{BLOCK_SIZE, Layout, Position};
pub use rates::{BlockRates, NominalRate, RateMeaning, RateOrigin, RateSetting, Rates, StreamRate};

/// Prefix size for identifying a saved flowgraph.
pub const HEADER_BYTES: usize = 8192;
const PREVIEW_BYTES: usize = 256 * 1024;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_BLOCKS: usize = 8192;
const MAX_LINKS: usize = 32768;
const MAX_SETTINGS: usize = 256;
const MAX_VALUE_BYTES: usize = 16384;

/// GNU Radio Companion's modern and legacy file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Xml,
}

impl Format {
    /// Format label for inspection views.
    pub fn label(self) -> &'static str {
        match self {
            Self::Yaml => "GNU Radio Companion YAML",
            Self::Xml => "GNU Radio Companion XML",
        }
    }
}

/// Detects a companion flowgraph in a bounded file prefix.
pub fn identify(prefix: &[u8]) -> Option<Format> {
    if crate::xml_content::root(prefix).is_some_and(|root| root.name().as_ref() == b"flow_graph") {
        return Some(Format::Xml);
    }
    yaml::recognize(prefix).then_some(Format::Yaml)
}

/// A stored block or graph setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setting {
    pub name: String,
    pub value: String,
    /// The source value exceeded the display budget.
    pub shortened: bool,
}

impl Setting {
    fn new(name: &str, value: &str) -> Self {
        let length = value.floor_char_boundary(value.len().min(MAX_VALUE_BYTES));
        Self {
            name: name.to_owned(),
            value: value[..length].to_owned(),
            shortened: length != value.len(),
        }
    }
}

/// Recorded enable or bypass state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Enabled,
    Disabled,
    Bypassed,
    Unknown,
}

impl State {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            None | Some("enabled" | "true" | "True" | "1") => Self::Enabled,
            Some("disabled" | "false" | "False" | "0") => Self::Disabled,
            Some("bypassed" | "2") => Self::Bypassed,
            _ => Self::Unknown,
        }
    }

    /// Human-readable state.
    pub fn label(self) -> &'static str {
        match self {
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
            Self::Bypassed => "Bypassed",
            Self::Unknown => "Unknown state",
        }
    }
}

/// A block instance with source-authored settings and optional canvas geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub name: String,
    pub kind: String,
    pub settings: Vec<Setting>,
    pub state: State,
    pub position: Option<Position>,
    pub rotation: Option<i16>,
}

impl Block {
    /// Finds an authored parameter by its source key.
    pub fn setting(&self, name: &str) -> Option<&str> {
        setting(&self.settings, name)
    }
}

/// One endpoint of a resolved wire. The port key is kept as authored.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub block: usize,
    pub port: String,
}

/// A directed connection between two saved block instances.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub source: Endpoint,
    pub sink: Endpoint,
    pub settings: Vec<Setting>,
}

/// Direction of a connected-block inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upstream,
    Downstream,
}

/// One step of a breadth-first trace from the selected block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub block: usize,
    pub connection: usize,
    pub hops: usize,
}

/// Parsed flowgraph, with exact connections among the published blocks.
#[derive(Clone, Debug)]
pub struct Graph {
    pub format: Format,
    pub title: String,
    pub version: Option<String>,
    pub file_format: Option<String>,
    pub options: Vec<Setting>,
    pub blocks: Vec<Block>,
    pub connections: Vec<Connection>,
    /// Number of malformed or ambiguous block entries omitted from inspection.
    pub skipped_blocks: usize,
    /// Invalid or unresolved connection records, including prefix-only gaps.
    pub skipped_connections: usize,
    adjacency: Vec<Adjacent>,
    ambiguous_names: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
struct Adjacent {
    upstream: Vec<usize>,
    downstream: Vec<usize>,
}

impl Graph {
    /// Returns a saved sample-rate expression when named in the graph.
    pub fn sample_rate(&self) -> Option<&str> {
        self.blocks.iter().find_map(|block| {
            matches!(block.name.as_str(), "samp_rate" | "sample_rate")
                .then(|| block.setting("value"))
                .flatten()
        })
    }

    /// Returns source ordinals of directed connections attached to a block.
    pub fn adjacent(&self, block: usize, direction: Direction) -> &[usize] {
        let Some(edges) = self.adjacency.get(block) else {
            return &[];
        };
        match direction {
            Direction::Upstream => &edges.upstream,
            Direction::Downstream => &edges.downstream,
        }
    }

    /// Traverses connected blocks in hop order. Each block is visited once.
    pub fn trace(&self, start: usize, direction: Direction, limit: usize) -> Vec<Step> {
        if start >= self.blocks.len() || limit == 0 {
            return Vec::new();
        }
        let mut visited = vec![false; self.blocks.len()];
        let mut queue = VecDeque::from([(start, 0)]);
        let mut trace = Vec::new();
        visited[start] = true;
        while let Some((block, hops)) = queue.pop_front() {
            for &connection in self.adjacent(block, direction) {
                let link = &self.connections[connection];
                let next = match direction {
                    Direction::Upstream => link.source.block,
                    Direction::Downstream => link.sink.block,
                };
                if std::mem::replace(&mut visited[next], true) {
                    continue;
                }
                trace.push(Step {
                    block: next,
                    connection,
                    hops: hops + 1,
                });
                if trace.len() == limit {
                    return trace;
                }
                queue.push_back((next, hops + 1));
            }
        }
        trace
    }

    /// Places missing blocks in connection order while preserving saved positions.
    pub fn layout(&self) -> Layout {
        layout::make(self)
    }
}

/// Bounded data for the little screen and file strip.
#[derive(Clone, Debug)]
pub struct Preview {
    pub graph: Graph,
    pub layout: Layout,
    pub complete: bool,
}

/// Parses the saved graph from a guarded regular file.
///
/// # Errors
///
/// Fails on unknown content, invalid syntax, or a file beyond the parser budget.
pub fn read(file: File, cancel: &AtomicBool) -> Result<Graph, String> {
    let (bytes, complete) = source(file, MAX_SOURCE_BYTES, cancel)?;
    if !complete {
        return Err("GNU Radio flowgraph exceeds the file size limit".into());
    }
    let graph = parse(&bytes, true, cancel)?;
    check_cancel(cancel)?;
    Ok(graph)
}

/// Parses a bounded prefix for a compact topology and metadata preview.
///
/// # Errors
///
/// Fails when a bounded prefix cannot identify or parse a supported graph.
pub fn preview(file: File, cancel: &AtomicBool) -> Result<Preview, String> {
    let (bytes, complete) = source(file, PREVIEW_BYTES, cancel)?;
    let graph = parse(&bytes, complete, cancel)?;
    let layout = graph.layout();
    check_cancel(cancel)?;
    Ok(Preview {
        graph,
        layout,
        complete,
    })
}

fn source(file: File, limit: usize, cancel: &AtomicBool) -> Result<(Vec<u8>, bool), String> {
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("GNU Radio flowgraphs must be regular files".into());
    }
    let take = metadata.len().min(limit as u64) as usize;
    let mut buffer = vec![0; take];
    const CHUNK: usize = 64 * 1024;
    for (index, block) in buffer.chunks_mut(CHUNK).enumerate() {
        check_cancel(cancel)?;
        file.read_exact_at(block, (index * CHUNK) as u64)
            .map_err(|error| error.to_string())?;
    }
    Ok((buffer, metadata.len() <= limit as u64))
}

fn parse(bytes: &[u8], complete: bool, cancel: &AtomicBool) -> Result<Graph, String> {
    let signature = &bytes[..bytes.len().min(HEADER_BYTES)];
    let format = identify(signature).ok_or("Unrecognized GNU Radio flowgraph")?;
    let raw = match format {
        Format::Yaml => yaml::parse(bytes, complete, cancel)?,
        Format::Xml => xml::parse(bytes, complete, cancel)?,
    };
    Ok(raw.finish(format))
}

#[derive(Default)]
struct RawGraph {
    options: Vec<Setting>,
    blocks: Vec<Block>,
    wires: Vec<RawLink>,
    version: Option<String>,
    file_format: Option<String>,
    skipped_blocks: usize,
    skipped_connections: usize,
}

struct RawLink {
    source: String,
    source_port: String,
    sink: String,
    sink_port: String,
    settings: Vec<Setting>,
}

impl RawGraph {
    fn finish(self, format: Format) -> Graph {
        let mut by_name = BTreeMap::new();
        let mut blocks = Vec::new();
        let mut skipped_blocks = self.skipped_blocks;
        for block in self.blocks {
            let name = block.name.clone();
            match by_name.entry(name) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Some(blocks.len()));
                    blocks.push(block);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    // All wires for a name with competing definitions are ambiguous.
                    entry.insert(None);
                    skipped_blocks += 1;
                }
            }
        }
        let mut connections = Vec::new();
        let mut skipped_connections = self.skipped_connections;
        let mut adjacency = vec![Adjacent::default(); blocks.len()];
        for wire in self.wires {
            let source = by_name.get(&wire.source).copied().flatten();
            let sink = by_name.get(&wire.sink).copied().flatten();
            let (Some(source), Some(sink)) = (source, sink) else {
                skipped_connections += 1;
                continue;
            };
            adjacency[source].downstream.push(connections.len());
            adjacency[sink].upstream.push(connections.len());
            connections.push(Connection {
                source: Endpoint {
                    block: source,
                    port: wire.source_port,
                },
                sink: Endpoint {
                    block: sink,
                    port: wire.sink_port,
                },
                settings: wire.settings,
            });
        }
        let title = ["title", "id"]
            .into_iter()
            .find_map(|name| setting(&self.options, name).filter(|value| !value.trim().is_empty()))
            .unwrap_or("GNU Radio flowgraph")
            .to_owned();
        let ambiguous_names = by_name
            .into_iter()
            .filter_map(|(name, index)| index.is_none().then_some(name))
            .collect();
        Graph {
            format,
            title,
            version: self.version,
            file_format: self.file_format,
            options: self.options,
            blocks,
            connections,
            skipped_blocks,
            skipped_connections,
            adjacency,
            ambiguous_names,
        }
    }
}

fn setting<'a>(settings: &'a [Setting], name: &str) -> Option<&'a str> {
    settings
        .iter()
        .find(|setting| setting.name == name)
        .map(|setting| setting.value.as_str())
}

fn valid_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 192
        && text
            .chars()
            .all(|value| !value.is_whitespace() && !value.is_control())
}

fn valid_port(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 192
        && !text.chars().any(char::is_control)
        && !text
            .strip_prefix('-')
            .is_some_and(|n| n.bytes().all(|byte| byte.is_ascii_digit()))
}

impl RawLink {
    fn new(fields: [&str; 4], settings: Vec<Setting>) -> Option<Self> {
        let [source, source_port, sink, sink_port] = fields;
        (valid_id(source) && valid_id(sink) && valid_port(source_port) && valid_port(sink_port))
            .then(|| Self {
                source: source.into(),
                source_port: source_port.into(),
                sink: sink.into(),
                sink_port: sink_port.into(),
                settings,
            })
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("GNU Radio flowgraph read canceled".into());
    }
    Ok(())
}
