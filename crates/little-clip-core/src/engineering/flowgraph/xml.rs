use std::{borrow::Cow, sync::atomic::AtomicBool};

use quick_xml::{Reader, events::Event};
use roxmltree::{Document, Node, ParsingOptions};

use super::{
    Block, MAX_BLOCKS, MAX_LINKS, MAX_SETTINGS, RawGraph, RawLink, Setting, State, check_cancel,
    layout, setting, valid_id,
};

pub(super) fn parse(bytes: &[u8], complete: bool, cancel: &AtomicBool) -> Result<RawGraph, String> {
    let bytes = if complete {
        Cow::Borrowed(bytes)
    } else {
        prefix(bytes, cancel)?
    };
    crate::xml_content::validate(&bytes, 650_000, 32)
        .map_err(|error| format!("Invalid GNU Radio XML: {error}"))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| format!("Invalid GNU Radio XML text: {error}"))?;
    let document = Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: true,
            entity_resolver: None,
            nodes_limit: 250_000,
        },
    )
    .map_err(|error| format!("Invalid GNU Radio XML: {error}"))?;
    let root = document.root_element();
    let metadata = document
        .root()
        .children()
        .find_map(|node| node.pi().filter(|pi| pi.target == "grc"))
        .and_then(|pi| pi.value)
        .unwrap_or_default();
    let mut graph = RawGraph {
        version: attribute(metadata, "created").map(str::to_owned),
        file_format: attribute(metadata, "format").map(str::to_owned),
        ..Default::default()
    };
    let mut block_count = 0;
    let mut link_count = 0;
    for node in root.children().filter(Node::is_element) {
        check_cancel(cancel)?;
        match node.tag_name().name() {
            "block" => {
                block_count += 1;
                if block_count > MAX_BLOCKS + 1 {
                    return Err("GNU Radio flowgraph exceeds the block limit".into());
                }
                read_block(node, &mut graph);
            }
            "connection" => {
                link_count += 1;
                if link_count > MAX_LINKS {
                    return Err("GNU Radio flowgraph exceeds the connection limit".into());
                }
                if let Some(link) = read_link(node) {
                    graph.wires.push(link);
                } else {
                    graph.skipped_connections += 1;
                }
            }
            _ => {}
        }
    }
    Ok(graph)
}

fn prefix<'a>(bytes: &'a [u8], cancel: &AtomicBool) -> Result<Cow<'a, [u8]>, String> {
    let mut reader = Reader::from_reader(bytes);
    let mut depth = 0usize;
    let mut end = None;
    loop {
        check_cancel(cancel)?;
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                depth += 1;
                if depth == 1 {
                    end = Some(reader.buffer_position());
                }
            }
            Ok(Event::End(_)) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(Cow::Borrowed(&bytes[..reader.buffer_position() as usize]));
                }
                if depth == 1 {
                    end = Some(reader.buffer_position());
                }
            }
            Ok(Event::Empty(_)) if depth == 1 => end = Some(reader.buffer_position()),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let end = end.ok_or("No complete GNU Radio XML opening tag in the preview")? as usize;
    let mut prefix = bytes[..end].to_vec();
    prefix.extend_from_slice(b"</flow_graph>");
    Ok(Cow::Owned(prefix))
}

fn read_block(node: Node<'_, '_>, graph: &mut RawGraph) {
    let kind = child(node, "key").unwrap_or_default().trim();
    let settings = node
        .children()
        .filter(|child| child.has_tag_name("param"))
        .take(MAX_SETTINGS)
        .filter_map(|parameter| {
            let key = child(parameter, "key")?.trim();
            let value = child(parameter, "value").unwrap_or_default();
            valid_id(key).then(|| Setting::new(key, value))
        })
        .collect::<Vec<_>>();
    if kind == "options" {
        graph.options = settings;
        return;
    }
    let name = setting(&settings, "id").unwrap_or_default().trim();
    if !valid_id(name) || !valid_id(kind) {
        graph.skipped_blocks += 1;
        return;
    }
    let state = State::parse(setting(&settings, "_enabled"));
    let position = setting(&settings, "_coordinate").and_then(parse_coordinate);
    let rotation = setting(&settings, "_rotation").and_then(|value| value.trim().parse().ok());
    graph.blocks.push(Block {
        name: name.into(),
        kind: kind.into(),
        state,
        position,
        rotation,
        settings: settings
            .into_iter()
            .filter(|option| {
                !matches!(
                    option.name.as_str(),
                    "id" | "_enabled" | "_coordinate" | "_rotation"
                )
            })
            .collect(),
    });
}

fn parse_coordinate(value: &str) -> Option<layout::Position> {
    let value = value
        .trim()
        .trim_start_matches(['(', '['])
        .trim_end_matches([')', ']']);
    let (x, y) = value.split_once(',')?;
    layout::valid([x.trim().parse().ok()?, y.trim().parse().ok()?])
}

fn read_link(node: Node<'_, '_>) -> Option<RawLink> {
    let source = child(node, "source_block_id")?.trim();
    let output = child(node, "source_key")?.trim();
    let sink = child(node, "sink_block_id")?.trim();
    let input = child(node, "sink_key")?.trim();
    RawLink::new([source, output, sink, input], Vec::new())
}

fn child<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.has_tag_name(name))?
        .text()
}

fn attribute<'a>(metadata: &'a str, name: &str) -> Option<&'a str> {
    metadata.split_ascii_whitespace().find_map(|field| {
        let (key, value) = field.split_once('=')?;
        (key == name).then(|| value.trim_matches(['"', '\'']))
    })
}
