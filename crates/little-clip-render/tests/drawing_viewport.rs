//! Verify the local renderer extension using its actual layout and SVG output.
use excalidraw_render::{OutputFormat, RenderRequest, Renderer, SceneDocument};

fn rectangle(x: f64, y: f64, angle: f64) -> SceneDocument {
    let source = serde_json::json!({
        "type":"excalidraw", "version":2, "elements":[{
            "id":"box", "type":"rectangle", "x":x, "y":y,
            "width":200, "height":100, "angle":angle,
            "strokeColor":"#111111", "backgroundColor":"transparent",
            "fillStyle":"solid", "strokeWidth":1, "strokeStyle":"solid",
            "roughness":0, "opacity":100, "seed":1,
        }], "appState":{"viewBackgroundColor":"#ffffff"}, "files":{},
    });
    SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap()
}

#[test]
fn translated_scenes_keep_logical_geometry_and_report_the_real_source_origin() {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .padding(10.0)
        .build()
        .unwrap();
    for angle in [0.0, 0.5] {
        let first = Renderer::new()
            .render(&rectangle(20.0, 30.0, angle), &request)
            .unwrap();
        let shifted = Renderer::new()
            .render(&rectangle(1020.0, -470.0, angle), &request)
            .unwrap();
        let a = first.viewport();
        let b = shifted.viewport();
        assert!((b.origin[0] - a.origin[0] - 1000.0).abs() < 1e-9);
        assert!((b.origin[1] - a.origin[1] + 500.0).abs() < 1e-9);
        for (a, b) in a.size.into_iter().zip(b.size) {
            assert!((a - b).abs() < 1e-9);
        }
        if angle == 0.0 {
            assert_eq!(a.origin, [10.0, 20.0]);
            assert_eq!(first.artifact().bytes(), shifted.artifact().bytes());
        }
    }
}

#[test]
fn empty_scene_viewport_keeps_padding_and_output_scale_separate() {
    let scene =
        SceneDocument::parse(br#"{"type":"excalidraw","version":2,"elements":[]}"#).unwrap();
    let request = RenderRequest::builder(OutputFormat::Svg)
        .padding(12.5)
        .sizing(excalidraw_render::Sizing::Scale(2.0))
        .build()
        .unwrap();
    let rendered = Renderer::new().render(&scene, &request).unwrap();
    assert_eq!(rendered.viewport().origin, [-12.5; 2]);
    assert_eq!(rendered.viewport().size, [25.0; 2]);
    let svg = std::str::from_utf8(rendered.artifact().bytes()).unwrap();
    assert!(svg.contains("viewBox=\"0 0 25 25\""));
    assert!(svg.contains("width=\"50\""));
}

#[test]
fn authored_scene_renders_shapes_handwriting_freehand_and_embedded_image() {
    let scene = SceneDocument::parse(include_bytes!(
        "../../../docs/examples/diagrams/drawing.excalidraw"
    ))
    .unwrap();
    let limits = excalidraw_render::ResourceLimits::builder()
        .max_scene_bytes(256 * 1024)
        .max_elements(128)
        .max_asset_entries(8)
        .max_output_bytes(2 * 1024 * 1024)
        .build()
        .unwrap();
    let request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(limits)
        .build()
        .unwrap();
    let output = Renderer::new().render(&scene, &request).unwrap();
    let svg = std::str::from_utf8(output.artifact().bytes()).unwrap();
    assert!(svg.contains("@font-face"));
    assert!(svg.contains("data:image/png;base64,"));
    assert!(svg.contains("Capture"));
    assert!(
        output
            .viewport()
            .size
            .into_iter()
            .all(|value| value > 0.0 && value.is_finite())
    );
    assert!(output.viewport().origin.into_iter().all(f64::is_finite));
    assert!(output.report().is_empty(), "{:?}", output.report());
}

#[test]
fn svg_outlines_keep_native_font_fallback_and_mixed_script_layout() {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/examples/diagrams/drawing.excalidraw"
    ))
    .unwrap();
    let mut element = value["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| element["type"] == "text")
        .unwrap()
        .clone();
    element["text"] = serde_json::json!("Café 中文 אבג");
    element["originalText"] = element["text"].clone();
    element["containerId"] = serde_json::Value::Null;
    value["elements"] = serde_json::json!([element]);
    let scene = SceneDocument::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    let request = RenderRequest::builder(OutputFormat::Svg).build().unwrap();
    let output = Renderer::new().render(&scene, &request).unwrap();
    let svg = std::str::from_utf8(output.artifact().bytes()).unwrap();
    assert!(
        !svg.contains("<text"),
        "consumer must not reshape native glyphs"
    );
    let doc = roxmltree::Document::parse(svg).unwrap();
    let glyphs = doc
        .descendants()
        .find(|node| {
            node.has_tag_name("path") && node.attribute("data-source-text") == Some("Café 中文 אבג")
        })
        .unwrap();
    let path = glyphs.attribute("d").unwrap();
    assert!(
        path.matches('Q').count() > 50,
        "real shaped contours, not an empty label"
    );
    assert!(path.matches('M').count() >= 8);
}
