//! BPMN semantic models and authored DI, without scripts or resource resolution.
use bpmnkit_bpmn::{Attrs, BpmnDefinitions};
use bpmnkit_render::{ColorMode, FontOptions, RenderOptions, RenderWarning};
use std::collections::{BTreeSet, HashSet};

const MODEL: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";
const BPMNDI: &str = "http://www.omg.org/spec/BPMN/20100524/DI";
const DC: &str = "http://www.omg.org/spec/DD/20100524/DC";
const DI: &str = "http://www.omg.org/spec/DD/20100524/DI";

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let document = preflight(source)?;
    let mut definitions = parse_model(source, &document)?;
    normalize_colors(&document, &mut definitions);
    let mut warnings = Vec::new();
    if definitions
        .diagrams
        .iter()
        .all(|diagram| diagram.plane.shapes.is_empty())
    {
        definitions = bpmnkit_layout::apply_auto_layout_owned(
            definitions,
            bpmnkit_layout::LayoutEngine::Semantic,
            None,
        );
        warnings.push("No authored layout; the preview uses automatic layout".into());
        if definitions.collaborations.len() > 1 {
            warnings.push("Automatic layout uses the first collaboration; other collaborations may be incomplete".into());
        }
    }
    let diagrams = std::mem::take(&mut definitions.diagrams);
    if diagrams.is_empty() || diagrams.len() > 16 {
        return Err("BPMN previews require between one and sixteen diagram planes".into());
    }
    let options = RenderOptions::default()
        .background(false)
        .colors(ColorMode::Di)
        .font(FontOptions {
            family: "Noto Sans CJK JP".into(),
            ..Default::default()
        });
    let mut rendered_ids = HashSet::new();
    let mut missing_ids = BTreeSet::new();
    let mut svgs = Vec::new();
    for diagram in diagrams {
        // DI IDs are optional. Isolate the chosen plane rather than inventing IDs
        // or selecting another plane when two optional IDs are both empty.
        definitions.diagrams = vec![diagram];
        let rendered = bpmnkit_render::bpmn_to_svg(&definitions, &options)
            .map_err(|error| error.to_string())?;
        rendered_ids.extend(rendered.elements.into_iter().map(|element| element.id));
        for warning in rendered.warnings {
            if let RenderWarning::ElementWithoutDi { id } = warning {
                missing_ids.insert(id);
            }
        }
        svgs.push(rendered.markup);
    }
    missing_ids.retain(|id| !rendered_ids.contains(id));
    if !missing_ids.is_empty() {
        warnings.push(format!(
            "{} BPMN elements have no displayed geometry: {}",
            missing_ids.len(),
            missing_ids
                .iter()
                .take(12)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if rendered_ids.is_empty() {
        return Err("BPMN contains no drawable elements".into());
    }
    let (svg, limitations) = super::compose_svg(svgs, super::default_fonts(), "Noto Sans CJK JP")?;
    warnings.extend(limitations);
    Ok((svg, warnings))
}

// Adapt the parser's root-attribute requirement, separately from rendering and
// geometry. This model is transient; editor saves always retain the source text.
fn parse_model(
    source: &str,
    document: &roxmltree::Document<'_>,
) -> Result<BpmnDefinitions, String> {
    // BPMN makes the root ID optional, while the parser requires the attribute.
    // Supply an empty ID only to its private input; never serialize this copy or
    // change offsets/source used by the editor. No drawable object uses this ID.
    let parse_source = if document.root_element().attribute("id").is_none() {
        let start = document.root_element().range().start + 1;
        let end = start
            + source[start..]
                .find([' ', '\n', '\r', '\t', '/', '>'])
                .ok_or("Invalid BPMN root tag")?;
        std::borrow::Cow::Owned(format!("{} id=\"\"{}", &source[..end], &source[end..]))
    } else {
        std::borrow::Cow::Borrowed(source)
    };
    bpmnkit_bpmn::parse_bpmn(&parse_source).map_err(|error| error.to_string())
}

fn preflight(source: &str) -> Result<roxmltree::Document<'_>, String> {
    let document = roxmltree::Document::parse_with_options(
        source,
        roxmltree::ParsingOptions {
            nodes_limit: 16_384,
            ..Default::default()
        },
    )
    .map_err(|error| format!("Invalid BPMN XML: {error}"))?;
    if !document.root_element().has_tag_name((MODEL, "definitions")) {
        return Err("BPMN requires a definitions root in the BPMN 2.0 model namespace".into());
    }
    let mut ids = HashSet::new();
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        if node.ancestors().take(66).count() > 64 {
            return Err("BPMN XML exceeds the nesting budget".into());
        }
        // Vendor extension trees are inert data, not BPMN merely because a
        // local name happens to match. The library skips this container.
        if node
            .ancestors()
            .skip(1)
            .any(|parent| parent.has_tag_name((MODEL, "extensionElements")))
        {
            continue;
        }
        let tag = node.tag_name();
        let required = match tag.name() {
            "BPMNDiagram" | "BPMNPlane" | "BPMNShape" | "BPMNEdge" | "BPMNLabel"
            | "BPMNLabelStyle" => Some(BPMNDI),
            "Bounds" | "Font" => Some(DC),
            "waypoint" => Some(DI),
            name if bpmnkit_spec::elements::bpmn_element(name).is_some() => Some(MODEL),
            _ => None,
        };
        if required.is_some_and(|namespace| tag.namespace() != Some(namespace)) {
            return Err(format!(
                "Incorrect XML namespace for BPMN element {}",
                tag.name()
            ));
        }
        if let Some(id) = node.attribute("id")
            && (id.is_empty() || !ids.insert(id))
        {
            return Err(format!("Empty or duplicate BPMN ID: {id}"));
        }
        if matches!(tag.namespace(), Some(DC | DI)) {
            validate_geometry(node)?;
        }
    }
    Ok(document)
}

// Keep geometry validation independent of semantic parsing: the native model
// otherwise defaults missing coordinates and accepts NaN as an ordinary float.
fn validate_geometry(node: roxmltree::Node<'_, '_>) -> Result<(), String> {
    let required: &[&str] = match node.tag_name().name() {
        "Bounds" => &["x", "y", "width", "height"],
        "waypoint" => &["x", "y"],
        _ => &[],
    };
    if required.iter().any(|name| node.attribute(*name).is_none()) {
        return Err("BPMN geometry is missing a required coordinate or dimension".into());
    }
    for attribute in node.attributes().filter(|attribute| {
        attribute.namespace().is_none()
            && matches!(attribute.name(), "x" | "y" | "width" | "height" | "size")
    }) {
        let value = attribute
            .value()
            .parse::<f64>()
            .map_err(|_| "Invalid BPMN geometry")?;
        let dimension = matches!(attribute.name(), "width" | "height" | "size");
        if !value.is_finite() || value.abs() > 16_384.0 || (dimension && value < 0.0) {
            return Err("BPMN geometry exceeds the finite viewport budget".into());
        }
    }
    Ok(())
}

fn normalize_colors(document: &roxmltree::Document<'_>, definitions: &mut BpmnDefinitions) {
    let nodes: std::collections::HashMap<_, _> = document
        .descendants()
        .filter(|node| {
            node.has_tag_name((BPMNDI, "BPMNShape")) || node.has_tag_name((BPMNDI, "BPMNEdge"))
        })
        .filter_map(|node| Some((node.attribute("id")?, node)))
        .collect();
    for diagram in &mut definitions.diagrams {
        let attributes = diagram
            .plane
            .shapes
            .iter_mut()
            .map(|shape| (&shape.id, &mut shape.unknown_attributes))
            .chain(
                diagram
                    .plane
                    .edges
                    .iter_mut()
                    .map(|edge| (&edge.id, &mut edge.unknown_attributes)),
            );
        for (id, attrs) in attributes {
            if let Some(node) = nodes.get(id.as_str()) {
                copy_colors(*node, attrs);
            }
        }
    }
}

fn copy_colors(node: roxmltree::Node<'_, '_>, attrs: &mut Attrs) {
    // Remove literal prefix lookalikes before adding namespace-verified colors.
    for key in [
        "bioc:fill",
        "bioc:stroke",
        "color:background-color",
        "color:border-color",
    ] {
        attrs.shift_remove(key);
    }
    // The upstream renderer reads conventional prefixes literally. Populate its
    // private render model from resolved namespaces; the user's XML is untouched.
    for (namespace, fill, stroke) in [
        (
            "https://www.omg.org/spec/BPMN/non-normative/color/1.0",
            "background-color",
            "border-color",
        ),
        ("http://bpmn.io/schema/bpmn/biocolor/1.0", "fill", "stroke"),
    ] {
        for (name, key) in [(fill, "bioc:fill"), (stroke, "bioc:stroke")] {
            if let Some(value) = node.attribute((namespace, name)) {
                attrs.insert(key.into(), value.into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const AUTHORED: &str = include_str!("../../../../docs/examples/diagrams/order.bpmn");
    const MINIMAL: &str = r#"<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" id="definitions" targetNamespace="urn:test"><process id="p"><startEvent id="s"/><task id="t" name="Review"/><sequenceFlow id="f" sourceRef="s" targetRef="t"/></process></definitions>"#;

    #[test]
    fn authored_coordinates_colors_and_namespace_aliases_survive_rendering() {
        let (svg, warnings) = svg(AUTHORED).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("#cceeff"));
        assert!(svg.contains("#fff0b3"));
        let rendered =
            super::super::render_inline(little_clip_core::diagram::Format::Bpmn, AUTHORED, None)
                .unwrap();
        assert!(rendered.width > rendered.height * 2);
        assert!(
            rendered.height > 240,
            "External two-line labels must fit the viewport"
        );
        assert!(rendered.png.len() > 1000);
        let parsed = bpmnkit_bpmn::parse_bpmn(AUTHORED).unwrap();
        assert_eq!(parsed.diagrams[0].plane.shapes[1].bounds.x, 130.0);
    }

    #[test]
    fn semantic_models_without_geometry_are_laid_out_with_diagnostics() {
        let without_id = MINIMAL.replace(" id=\"definitions\"", "");
        let (image, warnings) = svg(&without_id).unwrap();
        assert!(image.contains("<path"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("automatic layout"))
        );
        let multiple = MINIMAL.replace(
            "</definitions>",
            "<process id=\"other\"><task id=\"second\"/></process></definitions>",
        );
        let (_, warnings) = svg(&multiple).unwrap();
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("no displayed geometry")),
            "{warnings:?}"
        );
    }

    #[test]
    fn multiple_optional_id_planes_are_all_rendered_without_marker_collisions() {
        let start = AUTHORED.find(" <layout:BPMNDiagram").unwrap();
        let end = AUTHORED.find("</definitions>").unwrap();
        let plane = AUTHORED[start..end]
            .replace(" id=\"diagram\"", "")
            .replace(" id=\"plane\"", "");
        let second = plane
            .replace("_shape\"", "_shape2\"")
            .replace("_edge\"", "_edge2\"");
        let source = format!("{}{plane}{second}</definitions>", &AUTHORED[..start]);
        let (svg, warnings) = svg(&source).unwrap();
        assert!(svg.contains("graph-0-"));
        assert!(svg.contains("graph-1-"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn malformed_geometry_namespace_confusion_entities_and_duplicate_ids_fail() {
        for source in [
            AUTHORED.replace("x=\"130\"", "x=\"NaN\""),
            AUTHORED.replace("width=\"120\"", "width=\"-5\""),
            AUTHORED.replace("20100524/MODEL", "foreign"),
            AUTHORED.replace("id=\"review\"", "id=\"received\""),
            AUTHORED.replace("20100524/DC", "foreign"),
            AUTHORED.replace("height=\"80\"", ""),
            MINIMAL.replace("<task", "<task xmlns=\"urn:foreign\""),
            format!("<!DOCTYPE definitions [<!ENTITY e 'payload'>]>{MINIMAL}"),
        ] {
            assert!(svg(&source).is_err(), "Accepted malformed BPMN: {source}");
        }
    }

    #[test]
    fn collaboration_pools_lanes_and_message_flows_are_drawn() {
        let source = MINIMAL.replace("<startEvent", "<laneSet id=\"lanes\"><lane id=\"lane\" name=\"Team\"><flowNodeRef>s</flowNodeRef><flowNodeRef>t</flowNodeRef></lane></laneSet><startEvent")
            .replace("</definitions>", "<collaboration id=\"collab\"><participant id=\"team\" name=\"Warehouse\" processRef=\"p\"/><participant id=\"customer\" name=\"Customer\"/><messageFlow id=\"message\" sourceRef=\"customer\" targetRef=\"t\"/></collaboration></definitions>");
        let (image, warnings) = svg(&source).unwrap();
        assert!(image.contains("<path"));
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("no displayed geometry")),
            "{warnings:?}"
        );
        let definitions = bpmnkit_layout::apply_auto_layout_owned(
            bpmnkit_bpmn::parse_bpmn(&source).unwrap(),
            bpmnkit_layout::LayoutEngine::Semantic,
            None,
        );
        let plane = &definitions.diagrams[0].plane;
        for id in ["team", "customer", "lane"] {
            assert!(
                plane.shapes.iter().any(|shape| shape.bpmn_element == id),
                "{id}"
            );
        }
        assert!(
            plane
                .edges
                .iter()
                .any(|edge| edge.bpmn_element == "message")
        );
    }

    #[test]
    fn foreign_color_prefix_lookalikes_do_not_override_the_theme() {
        let source = AUTHORED
            .replace("xmlns:paint=", "xmlns:bioc=\"urn:foreign\" xmlns:paint=")
            .replace("paint:fill=\"#cceeff\"", "bioc:fill=\"#aabbcc\"");
        let (image, _) = svg(&source).unwrap();
        assert!(!image.contains("#aabbcc"));
    }

    #[test]
    fn vendor_extensions_remain_inert_and_missing_geometry_is_reported() {
        let source = AUTHORED.replace("<userTask id=\"review\" name=\"Review order\"/>",
            "<userTask id=\"review\" name=\"Review order\"><extensionElements><task xmlns=\"urn:vendor\" path=\"/etc/passwd\"/></extensionElements></userTask><task id=\"missing\"/>");
        let (_, warnings) = svg(&source).unwrap();
        assert!(
            warnings.iter().any(|warning| warning.contains("missing")),
            "{warnings:?}"
        );
    }
}
