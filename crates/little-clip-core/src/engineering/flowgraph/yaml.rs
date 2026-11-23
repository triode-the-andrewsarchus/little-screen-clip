use std::{borrow::Cow, sync::atomic::AtomicBool};

use oneyaml::{Compat, Document, Limits, MappingRef, NodeKind, NodeRef, SequenceRef};

use super::{
    Block, MAX_BLOCKS, MAX_LINKS, MAX_SETTINGS, MAX_SOURCE_BYTES, RawGraph, RawLink, Setting,
    State, check_cancel, layout, valid_id,
};

pub(super) fn recognize(prefix: &[u8]) -> bool {
    let Some(text) = std::str::from_utf8(prefix)
        .or_else(|error| std::str::from_utf8(&prefix[..error.valid_up_to()]))
        .ok()
    else {
        return false;
    };
    if text.contains('\0') {
        return false;
    }
    let mut sections = 0u8;
    let mut parameters = false;
    for line in text.trim_start_matches('\u{feff}').lines() {
        if line.starts_with("options:") {
            sections |= 1;
        } else if line.starts_with("blocks:") {
            sections |= 2;
        } else if line.starts_with("connections:") {
            sections |= 4;
        } else if line.trim_start().starts_with("parameters:") {
            parameters = true;
        }
    }
    sections & 3 == 3 && (parameters || sections & 4 != 0)
}

pub(super) fn parse(bytes: &[u8], complete: bool, cancel: &AtomicBool) -> Result<RawGraph, String> {
    let limits = Limits::untrusted()
        .max_depth(32)
        .max_typed_depth(32)
        .max_document_size(MAX_SOURCE_BYTES)
        .max_scalar_len(1024 * 1024)
        .max_documents(1)
        .alias_budget(0)
        .alias_byte_budget(0)
        .max_diagnostics(16);
    let settings = Compat::yaml_1_1().limits(limits);
    let document = if complete {
        Document::from_slice_with(bytes, &settings)
            .map_err(|error| format!("Invalid GNU Radio YAML: {error}"))?
    } else {
        Document::from_slice_recovering(bytes, &settings).into_value()
    };
    let root = document
        .root()
        .and_then(mapping)
        .ok_or("GNU Radio YAML root must be a mapping")?;
    let mut graph = RawGraph::default();
    let options = field(root, "options")
        .and_then(mapping)
        .ok_or("Missing GNU Radio flowgraph options")?;
    graph.options = settings_map(field(options, "parameters"));
    if let Some(metadata) = field(root, "metadata").and_then(mapping) {
        graph.version = field(metadata, "grc_version")
            .and_then(scalar)
            .map(Cow::into_owned);
        graph.file_format = field(metadata, "file_format")
            .and_then(scalar)
            .map(Cow::into_owned);
    }
    parse_blocks(field(root, "blocks"), complete, &mut graph, cancel)?;
    parse_connections(field(root, "connections"), complete, &mut graph, cancel)?;
    Ok(graph)
}

fn parse_blocks(
    nodes: Option<NodeRef<'_>>,
    complete: bool,
    graph: &mut RawGraph,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let Some(nodes) = nodes.and_then(sequence) else {
        if complete {
            return Err("GNU Radio flowgraph blocks must be a list".into());
        }
        return Ok(());
    };
    if nodes.len() > MAX_BLOCKS {
        return Err("GNU Radio flowgraph exceeds the block limit".into());
    }
    for node in nodes.iter() {
        check_cancel(cancel)?;
        if let Some(block) = block(node) {
            graph.blocks.push(block);
        } else {
            graph.skipped_blocks += 1;
        }
    }
    Ok(())
}

fn parse_connections(
    nodes: Option<NodeRef<'_>>,
    complete: bool,
    graph: &mut RawGraph,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let Some(nodes) = nodes.and_then(sequence) else {
        if complete {
            return Err("GNU Radio flowgraph connections must be a list".into());
        }
        return Ok(());
    };
    if nodes.len() > MAX_LINKS {
        return Err("GNU Radio flowgraph exceeds the connection limit".into());
    }
    for node in nodes.iter() {
        check_cancel(cancel)?;
        if let Some(link) = link(node) {
            graph.wires.push(link);
        } else {
            graph.skipped_connections += 1;
        }
    }
    Ok(())
}

fn block(node: NodeRef<'_>) -> Option<Block> {
    let fields = mapping(node)?;
    let name = scalar(field(fields, "name")?)?;
    let kind = scalar(field(fields, "id")?)?;
    if !valid_id(&name) || !valid_id(&kind) {
        return None;
    }
    let states = field(fields, "states").and_then(mapping);
    let state = states
        .and_then(|states| field(states, "state"))
        .and_then(scalar);
    let position = states
        .and_then(|states| field(states, "coordinate"))
        .and_then(sequence)
        .and_then(|pair| {
            if pair.len() != 2 {
                return None;
            }
            let coordinate = |index| scalar(pair.get(index)?)?.parse::<f32>().ok();
            layout::valid([coordinate(0)?, coordinate(1)?])
        });
    let rotation = states
        .and_then(|states| field(states, "rotation"))
        .and_then(scalar)
        .and_then(|value| value.parse::<i16>().ok());
    Some(Block {
        name: name.into_owned(),
        kind: kind.into_owned(),
        settings: settings_map(field(fields, "parameters")),
        state: State::parse(state.as_deref()),
        position,
        rotation,
    })
}

fn link(node: NodeRef<'_>) -> Option<RawLink> {
    if let Some(list) = sequence(node) {
        if list.len() != 4 {
            return None;
        }
        let [source, output, sink, input] = [
            scalar(list.get(0)?)?,
            scalar(list.get(1)?)?,
            scalar(list.get(2)?)?,
            scalar(list.get(3)?)?,
        ];
        return RawLink::new([&source, &output, &sink, &input], Vec::new());
    }
    let map = mapping(node)?;
    let get = |name| field(map, name).and_then(scalar);
    let source = get("src_blk_id")?;
    let output = get("src_port_id")?;
    let sink = get("snk_blk_id")?;
    let input = get("snk_port_id")?;
    RawLink::new(
        [&source, &output, &sink, &input],
        settings_map(field(map, "params")),
    )
}

fn field<'a>(map: MappingRef<'a>, name: &str) -> Option<NodeRef<'a>> {
    map.iter()
        .find_map(|(key, value)| (scalar(key).as_deref() == Some(name)).then_some(value))
}

fn scalar(node: NodeRef<'_>) -> Option<Cow<'_, str>> {
    (node.kind() == NodeKind::Scalar)
        .then(|| node.as_scalar().map(|value| value.value()))
        .flatten()
}

fn mapping(node: NodeRef<'_>) -> Option<MappingRef<'_>> {
    (node.kind() == NodeKind::Mapping)
        .then(|| node.as_mapping())
        .flatten()
}

fn sequence(node: NodeRef<'_>) -> Option<SequenceRef<'_>> {
    (node.kind() == NodeKind::Sequence)
        .then(|| node.as_sequence())
        .flatten()
}

fn settings_map(node: Option<NodeRef<'_>>) -> Vec<Setting> {
    let Some(mapping) = node.and_then(mapping) else {
        return Vec::new();
    };
    mapping
        .iter()
        .take(MAX_SETTINGS)
        .filter_map(|(key, value)| {
            let name = scalar(key)?;
            let text = scalar(value).unwrap_or_else(|| value.text());
            valid_id(&name).then(|| Setting::new(&name, &text))
        })
        .collect()
}
