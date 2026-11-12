//! Native drawing layout, verified handwriting faces and bounded raster assets.
use super::{DrawingLayout, ImagePolicy, Rendered};
use excalidraw_render::{
    OutputFormat, RenderRequest, Renderer, ResourceLimits, SceneDocument, Sizing,
};

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
    )?;
    let layout = viewport_after_composition(&svg, viewport.origin)?;
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
fn viewport_after_composition(svg: &str, origin: [f64; 2]) -> Result<DrawingLayout, String> {
    let document = super::svg_document(svg)?;
    let inner = document
        .root_element()
        .descendants()
        .skip(1)
        .find(|node| node.has_tag_name("svg"))
        .ok_or("Composed drawing has no viewport")?;
    let bounds = super::svg_viewbox(inner)?.ok_or("Composed drawing has no viewBox")?;
    Ok(DrawingLayout {
        origin: [
            origin[0] + f64::from(bounds.x()),
            origin[1] + f64::from(bounds.y()),
        ],
        size: [f64::from(bounds.width()), f64::from(bounds.height())],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_scene_keeps_real_fonts_raster_pixels_and_static_attribution() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let rendered = render(source).unwrap();
        assert!(rendered.drawing.unwrap().valid());
        assert!(!rendered.svg.contains("<text"));
        assert!(!rendered.svg.contains("<script"));
        assert!(rendered.svg.contains("drawing-attribution"));
        assert!(rendered.svg.contains("data:image/png;base64,"));
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
