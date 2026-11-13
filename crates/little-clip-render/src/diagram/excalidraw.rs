//! Native drawing layout, verified handwriting faces and bounded raster assets.
use super::{DrawingLayout, ImagePolicy, Rendered};
use excalidraw_render::{
    OutputFormat, RenderRequest, Renderer, ResourceLimits, SceneDocument, Sizing,
};

/// Measures native unwrapped drawing text on the caller's bounded edit worker.
pub fn measure_text(
    text: &str,
    font_family: i32,
    font_size: f64,
    line_height: f64,
) -> Result<[f64; 2], String> {
    excalidraw_render::measure_text(text, font_family, font_size, line_height)
}

pub(super) fn render(source: &str) -> Result<Rendered, String> {
    let scene = SceneDocument::parse(source.trim_start_matches('\u{feff}').as_bytes())
        .map_err(|error| error.to_string())?;
    let limits = ResourceLimits::builder()
        .max_scene_bytes(super::MAX_DRAWING_SOURCE as u64)
        .max_elements(2048)
        .max_asset_entries(64)
        .max_asset_bytes(2 * 1024 * 1024)
        .max_total_asset_bytes(4 * 1024 * 1024)
        .max_decoded_pixels_per_asset(2048 * 2048)
        .max_total_decoded_pixels(8 * 1024 * 1024)
        .max_geometry_segments_per_element(16_384)
        .max_total_geometry_segments(131_072)
        .max_text_scalars_per_element(4096)
        .max_total_text_scalars(32_768)
        .max_svg_dimension(16_384.0)
        .max_diagnostics(128)
        .max_working_bytes(128 * 1024 * 1024)
        .max_output_bytes(super::MAX_SVG as u64)
        .build()
        .map_err(|error| error.to_string())?;
    let request = RenderRequest::builder(OutputFormat::Svg)
        .sizing(Sizing::Scale(1.0))
        .resource_limits(limits)
        .build()
        .map_err(|error| error.to_string())?;
    let output = Renderer::new()
        .render(&scene, &request)
        .map_err(|error| error.to_string())?;
    let viewport = output.viewport();
    let (artifact, report) = output.into_parts();
    let raw = String::from_utf8(artifact.into_bytes()).map_err(|error| error.to_string())?;
    let metadata = attribution(&raw)?;
    let (mut svg, mut warnings) = super::compose_svg(
        [raw],
        super::fonts::metric_fonts(),
        "Helvetica",
        ImagePolicy::EmbeddedRaster,
        super::ViewportPolicy::Preserve,
    )?;
    let layout = DrawingLayout {
        origin: viewport.origin,
        size: viewport.size,
    };
    if let Some(metadata) = metadata {
        // Keep the native asset notices as inert text after outlining removes
        // font programs and the original SVG's metadata carrier.
        let position = svg.find('>').ok_or("Drawing SVG has no root")? + 1;
        svg.insert_str(
            position,
            &format!("<metadata id=\"drawing-attribution\">{metadata}</metadata>"),
        );
    }
    warnings.extend(report.diagnostics().iter().map(ToString::to_string));
    let mut rendered = super::rasterize_svg(svg, true, ImagePolicy::EmbeddedRaster)?;
    rendered.drawing = Some(layout);
    rendered.warnings.extend(warnings);
    rendered.warnings = super::bounded_warnings(rendered.warnings);
    rendered.validate(little_clip_core::diagram::Format::Excalidraw)?;
    Ok(rendered)
}
fn attribution(svg: &str) -> Result<Option<String>, String> {
    let document = super::svg_document(svg)?;
    Ok(document
        .root_element()
        .children()
        .find(|node| node.has_tag_name("metadata"))
        .map(|node| quick_xml::escape::escape(&svg[node.range()]).into_owned()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_text_measurement_uses_shaping_advances_and_canonical_line_metrics() {
        for family in [1, 2, 3, 5, 6, 7, 8, 9] {
            let one = measure_text("AV", family, 20.0, 1.25).unwrap();
            let two = measure_text("AV", family, 40.0, 1.25).unwrap();
            assert!((two[0] - 2.0 * one[0]).abs() < 1e-6);
            assert_eq!(one[1], 25.0);
            let multiline = measure_text("AV\r\nAV", family, 20.0, 1.25).unwrap();
            assert_eq!(multiline, [one[0], 50.0]);
        }
        assert!(measure_text("Text", 99, 20.0, 1.25).is_err());
        assert!(measure_text(&"x".repeat(4097), 5, 20.0, 1.25).is_err());
        assert!(measure_text("Text", 5, f64::INFINITY, 1.25).is_err());
        let unicode = measure_text("Capture 漢字 😀", 5, 24.0, 1.25).unwrap();
        assert!(unicode[0] > 100.0);
    }

    #[test]
    fn measured_native_text_transactions_render_with_unchanged_assets_and_source_ids() {
        use little_clip_core::diagram::drawing::{
            Change, Number, Scene, Style, TextContent, TextCreation, TextStyle,
        };
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let model = Scene::parse(source).unwrap();
        let font = TextStyle::default();
        let size =
            measure_text("New native text", font.family, font.size, font.line_height).unwrap();
        let edited = model
            .apply(Change::CreateText(TextCreation {
                id: "native-text".into(),
                position: [20.0, 350.0],
                content: TextContent::plain(
                    "New native text",
                    Number::from_f64(size[0]).unwrap(),
                    Number::from_f64(size[1]).unwrap(),
                ),
                font,
                style: Style::default(),
                container: None,
            }))
            .unwrap()
            .unwrap();
        let rendered = render(&edited).unwrap();
        let tree = super::super::retained::prepare(&rendered.svg).unwrap().tree;
        let group = tree.node_by_id("graph-0-drawing-element-6e61746976652d74657874").unwrap();
        let usvg_vector::Node::Group(group) = group else { panic!("source identity must remain a group"); };
        assert!(group.transform().is_identity(), "resize affines rely on an identity source wrapper: {:?}", group.transform());
        assert!(
            tree.node_by_id("graph-0-drawing-element-6e61746976652d74657874")
                .unwrap()
                .abs_bounding_box()
                .width()
                > 100.0
        );
        assert!(rendered.drawing.unwrap().size[1] > 350.0);
        let old: serde_json::Value = serde_json::from_str(source).unwrap();
        let new: serde_json::Value = serde_json::from_str(&edited).unwrap();
        assert_eq!(old["files"], new["files"]);
    }

    #[test]
    fn source_groups_keep_unicode_identities_and_own_all_their_vector_paths() {
        let id = "Capture<&\" 🙂";
        let source = serde_json::json!({
            "type": "excalidraw", "version": 2,
            "elements": [{"id": id, "type": "rectangle", "updated": 1, "x": 0, "y": 0,
                "width": 140, "height": 80, "backgroundColor": "#b2f2bb", "fillStyle": "solid"}]
        });
        let rendered = render(&source.to_string()).unwrap();
        let suffix = format!(
            "drawing-element-{}",
            id.as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let document = roxmltree::Document::parse(&rendered.svg).unwrap();
        let groups: Vec<_> = document
            .descendants()
            .filter(|node| {
                node.has_tag_name("g")
                    && node.attribute("id").is_some_and(|id| id.ends_with(&suffix))
            })
            .collect();
        assert_eq!(groups.len(), 1);
        assert!(
            groups[0]
                .descendants()
                .any(|node| node.has_tag_name("path"))
        );
        assert_eq!(rendered.drawing.unwrap().origin, [-10.0, -10.0]);
        assert_eq!(rendered.drawing.unwrap().size, [160.0, 100.0]);
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    }

    #[test]
    fn native_scene_keeps_real_fonts_raster_pixels_and_static_attribution() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let rendered = render(source).unwrap();
        // The image is clipped to its authored box. Its invisible expanded
        // symbol must not enlarge the 480×275 scene plus ten-unit padding.
        assert_eq!(
            rendered.drawing,
            Some(DrawingLayout {
                origin: [-10.0, 30.0],
                size: [500.0, 295.0],
            })
        );
        assert!(!rendered.svg.contains("<text"));
        assert!(!rendered.svg.contains("<script"));
        assert!(rendered.svg.contains("drawing-attribution"));
        assert!(rendered.svg.contains("data:image/png;base64,"));
        let document = roxmltree::Document::parse(&rendered.svg).unwrap();
        let objects = document
            .descendants()
            .filter(|node| {
                node.has_tag_name("g")
                    && node
                        .attribute("id")
                        .is_some_and(|id| id.starts_with("graph-0-drawing-element-"))
            })
            .count();
        assert_eq!(
            objects, 8,
            "shapes, text, arrows, freehand and images retain their individual identities"
        );
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        let image = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
        assert!(
            image
                .pixels()
                .any(|pixel| pixel.0[2] > 120 && pixel.0[1] > 70 && pixel.0[0] < 90)
        );
        let restored: Rendered =
            serde_json::from_slice(&serde_json::to_vec(&rendered).unwrap()).unwrap();
        assert_eq!(restored.drawing, rendered.drawing);
    }
    #[test]
    fn drawing_responses_reject_missing_or_nonfinite_geometry_and_keep_bounded_source() {
        let source = r#"{"type":"excalidraw","version":2,"elements":[]}"#;
        let mut rendered = render(source).unwrap();
        let format = little_clip_core::diagram::Format::Excalidraw;
        assert!(
            rendered
                .validate(little_clip_core::diagram::Format::Pikchr)
                .is_err()
        );
        rendered.drawing.as_mut().unwrap().origin[0] = f64::NAN;
        assert!(rendered.validate(format).is_err());
        rendered.drawing = None;
        assert!(rendered.validate(format).is_err());
        let large = " ".repeat(super::super::MAX_SOURCE + 1);
        assert!(super::super::validate_source(format, &large).is_ok());
        assert!(
            super::super::validate_source(little_clip_core::diagram::Format::Pikchr, &large)
                .is_err()
        );
        assert!(
            super::super::validate_source(
                format,
                &" ".repeat(super::super::MAX_DRAWING_SOURCE + 1)
            )
            .is_err()
        );
    }

    #[test]
    fn artifact_world_mapping_is_stable_when_the_source_moves() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let first = render(source).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(source).unwrap();
        for element in value["elements"].as_array_mut().unwrap() {
            element["x"] = serde_json::json!(element["x"].as_f64().unwrap() + 1000.0);
            element["y"] = serde_json::json!(element["y"].as_f64().unwrap() - 500.0);
        }
        let moved = render(&value.to_string()).unwrap();
        let first = first.drawing.unwrap();
        let moved = moved.drawing.unwrap();
        assert!((moved.origin[0] - first.origin[0] - 1000.0).abs() < 0.01);
        assert!((moved.origin[1] - first.origin[1] + 500.0).abs() < 0.01);
        assert!((moved.size[0] - first.size[0]).abs() < 0.01);
        assert!((moved.size[1] - first.size[1]).abs() < 0.01);
    }
}
