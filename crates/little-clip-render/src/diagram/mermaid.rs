//! Mermaid’s native parser/layout path with an explicit non-browser SVG contract.
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
use std::time::Duration;

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    // The SVG stage permits font references: our following raster stage resolves
    // them against owned bundled fonts. RequirePortable would reject that input
    // before font materialization (svg_fonts_not_self_contained).
    let request = SvgRequest {
        environment: merman::render::SvgEnvironment::deterministic()
            .with_resource_policy(merman::svg::RenderResourcePolicy::constrained()),
        pipeline: Some(merman::svg::SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    // Mermaid uses local "today" for Gantt markers; a fixed test epoch would
    // silently display the wrong date. Keep only the layout/random seed fixed.
    let runtime = merman::model::runtime::RuntimePolicy::deterministic()
        .try_with_system_clock()
        .and_then(|policy| policy.try_with_system_time_zone())
        .map_err(|error| error.to_string())?;
    let output = Renderer::new()
        .with_runtime_policy(runtime)
        .render(RenderRequest::svg(
            source,
            OperationControl::new().with_deadline(Duration::from_secs(5)),
            request,
        ))
        .map_err(|error| error.to_string())?;
    let RenderOutput::Svg(Some(output)) = output else {
        return Err("Source does not contain a Mermaid diagram".into());
    };
    let annotated = selection_groups(output.svg())?;
    super::compose_svg(
        [annotated],
        super::default_fonts(),
        "Noto Sans CJK JP",
        super::ImagePolicy::Block,
        super::ViewportPolicy::Preserve,
    )
}

/// Wrap actual renderer semantics without replacing generated DOM/resource IDs.
fn selection_groups(svg: &str) -> Result<String, String> {
    use quick_xml::{
        Reader, Writer,
        events::{BytesEnd, BytesStart, Event},
    };
    use std::collections::HashMap;
    let mut reader = Reader::from_str(svg);
    let mut writer = Writer::new(Vec::with_capacity(svg.len()));
    let mut stack = Vec::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    loop {
        let event = reader.read_event().map_err(|error| error.to_string())?;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let selection = selection_base(element, &reader)?;
                if let Some(base) = &selection {
                    let count = counts.entry(base.clone()).or_default();
                    let id = format!("{base}-part-{count}");
                    *count += 1;
                    let mut group = BytesStart::new("g");
                    group.push_attribute(("id", id.as_str()));
                    writer
                        .write_event(Event::Start(group))
                        .map_err(|error| error.to_string())?;
                }
                writer
                    .write_event(event.clone())
                    .map_err(|error| error.to_string())?;
                if matches!(event, Event::Start(_)) {
                    stack.push(selection.is_some());
                } else if selection.is_some() {
                    writer
                        .write_event(Event::End(BytesEnd::new("g")))
                        .map_err(|error| error.to_string())?;
                }
            }
            Event::End(_) => {
                writer
                    .write_event(event.clone())
                    .map_err(|error| error.to_string())?;
                if stack.pop().unwrap_or(false) {
                    writer
                        .write_event(Event::End(BytesEnd::new("g")))
                        .map_err(|error| error.to_string())?;
                }
            }
            Event::Eof => break,
            _ => writer
                .write_event(event)
                .map_err(|error| error.to_string())?,
        }
    }
    String::from_utf8(writer.into_inner()).map_err(|error| error.to_string())
}

fn selection_base(
    element: &quick_xml::events::BytesStart<'_>,
    reader: &quick_xml::Reader<&[u8]>,
) -> Result<Option<String>, String> {
    let mut kind = None;
    let mut identity = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        let value = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| error.to_string())?;
        match attribute.key.as_ref() {
            b"data-et" => kind = Some(value.into_owned()),
            b"data-id" => identity = Some(value.into_owned()),
            _ => (),
        }
    }
    Ok(kind
        .as_deref()
        .zip(identity.as_deref())
        .filter(|(kind, _)| {
            matches!(
                (*kind, element.name().as_ref()),
                ("node", b"g") | ("edge", b"path") | ("edge-label", b"g")
            )
        })
        .map(|(kind, id)| little_clip_core::diagram::mermaid::selection_id(kind, id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_mermaid_families_produce_native_outlined_svg() {
        for source in [
            "flowchart LR\nA[Source] --> B[Preview]",
            "sequenceDiagram\nAlice->>Bob: Hello\nBob-->>Alice: Ready",
            "classDiagram\nVehicle <|-- Bicycle\nVehicle : +start()",
            "erDiagram\nUSER ||--o{ ORDER : places",
        ] {
            let (output, _) = svg(source).unwrap();
            assert!(output.contains("<svg"));
            assert!(output.contains("<path"));
            assert!(!output.contains("<text"));
            assert!(!output.contains("<foreignObject"));
            super::super::retained::prepare(&output).unwrap();
        }
    }

    #[test]
    fn flowchart_semantic_groups_survive_outlining_without_guessing_dom_ids() {
        let (output, _) = svg("flowchart LR\nA[Source] -->|ready| B[Preview]").unwrap();
        for (kind, identity) in [("node", "A"), ("node", "B")] {
            let id = little_clip_core::diagram::mermaid::selection_id(kind, identity);
            assert!(output.contains(&format!("graph-0-{id}-part-0")));
        }
        let document = roxmltree::Document::parse(&output).unwrap();
        let ids: Vec<_> = document
            .descendants()
            .filter_map(|node| node.attribute("id"))
            .collect();
        let distinct: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids.len(), distinct.len());
        super::super::rasterize_svg(output, true, super::super::ImagePolicy::Block).unwrap();
    }

    #[test]
    fn selection_wrappers_preserve_original_resource_ids_and_segment_identity() {
        let input = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><defs><marker id="arrow"/></defs><g id="original" data-et="node" data-id="a&amp;b"><rect width="10" height="10"/></g><path data-et="edge" data-id="e" marker-end="url(#arrow)" d="M0 0L10 10"/><path data-et="edge" data-id="e" d="M10 10L20 10"/></svg>"##;
        let output = selection_groups(input).unwrap();
        assert!(output.contains("id=\"original\""));
        assert!(output.contains("url(#arrow)"));
        assert!(output.contains("clip-mermaid-node-612662-part-0"));
        assert!(output.contains("clip-mermaid-edge-65-part-0"));
        assert!(output.contains("clip-mermaid-edge-65-part-1"));
        roxmltree::Document::parse(&output).unwrap();
    }
}
