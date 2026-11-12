// SPDX-License-Identifier: MPL-2.0
//! Pinned master d2wasm.Render target boundary, independent of source compilation.
use crate::public_compile_request;
use base64::{Engine, engine::general_purpose::STANDARD};
use d2_rs_render::{RenderOptions, target::TargetDiagram};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub(super) fn render(diagram_json: &str, options_json: &str) -> Result<String, String> {
    render_checked(diagram_json, options_json).map_err(|error| error.message)
}

#[derive(serde::Serialize)]
struct RenderError {
    message: String,
    code: u16,
}
impl RenderError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 400,
        }
    }
}
impl From<String> for RenderError {
    fn from(message: String) -> Self {
        Self { message, code: 500 }
    }
}

/// Same data/error envelope as wrapWASMCall(Render), including its typed codes.
pub(super) fn response(diagram_json: &str, options_json: &str) -> String {
    match render_checked(diagram_json, options_json) {
        Ok(svg) if svg.is_empty() => serde_json::json!({"data": null}),
        Ok(svg) => serde_json::json!({"data": STANDARD.encode(svg)}),
        Err(error) => serde_json::json!({"error": error}),
    }
    .to_string()
}

fn render_checked(diagram_json: &str, options_json: &str) -> Result<String, RenderError> {
    let diagram = TargetDiagram::decode_json(diagram_json.as_bytes())
        .map_err(|_| RenderError::bad_request("invalid JSON input"))?
        .ok_or_else(|| RenderError::bad_request("missing 'diagram' field in input JSON"))?;
    let options =
        public_compile_request::render_options(options_json).map_err(RenderError::bad_request)?;
    // functions.go:363: validate the original tree before GetBoard and before
    // single-board rendering removes descendants. Invalid unselected boards
    // are still errors at this public boundary.
    diagram
        .validate_render_target()
        .map_err(|error| RenderError::bad_request(format!("invalid diagram: {error}")))?;
    let target = options.get("target").and_then(Value::as_str).unwrap_or("");
    let (selected, descendants) = if target == "*" {
        ("", true)
    } else if let Some(base) = target.strip_suffix(".*") {
        (base, true)
    } else {
        (target, false)
    };
    let path = if selected.is_empty() {
        Vec::new()
    } else {
        d2_rs_language::parse_key(selected.as_bytes())
            .ok()
            .and_then(|key| key.string_ida())
            .ok_or_else(|| {
                RenderError::bad_request(format!("target '{selected}' not recognized"))
            })?
    };
    let interval = options
        .get("animateInterval")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if descendants && interval <= 0 {
        return Err(format!("target '{target}' only supported for animated SVGs").into());
    }
    let board = diagram.get_board(&path).ok_or_else(|| {
        RenderError::bad_request(format!("render target '{}' not found", path.join(".")))
    })?;
    let isometric = options.get("isometric") == Some(&Value::Bool(true));
    if isometric {
        // The public ruler/font state is checked before format admission.
        public_compile_request::initialize_render_ruler()?;
        // functions.go:452–463: format errors precede renderer mode checks.
        if options.get("ascii") == Some(&Value::Bool(true)) {
            return Err(RenderError::bad_request(
                "isometric rendering requires SVG output; ASCII cannot preserve its geometry",
            ));
        }
        if interval > 0 {
            return Err(RenderError::bad_request(
                "isometric rendering does not yet support multi-board SVG animation",
            ));
        }
        if options.get("forceAppendix") == Some(&Value::Bool(true)) {
            return Err(RenderError::bad_request(
                "isometric rendering does not yet support an SVG appendix",
            ));
        }
    }
    if options.get("ascii") == Some(&Value::Bool(true)) {
        if descendants {
            return Err(RenderError::bad_request(
                "ASCII rendering does not support multi-board animation",
            ));
        }
        return d2_rs_export::ascii_target::render(
            &board.without_children(),
            options
                .get("asciiMode")
                .and_then(Value::as_str)
                .unwrap_or("extended"),
            options.get("scale").and_then(Value::as_f64),
        )
        .map_err(|error| format!("ASCII render failed: {error}").into());
    }
    let board = if descendants {
        board.clone()
    } else {
        board.without_children()
    };
    // Render computes the selected target's recursive hash before rendering
    // any frames, including when a folder-only tree has no painted boards.
    if interval > 0 {
        board
            .hash_input()
            .map_err(|error| format!("cannot process animate interval: {error}"))?;
    }
    if !isometric {
        public_compile_request::initialize_render_ruler()?;
    }
    if interval > 0 && options.get("forceAppendix") == Some(&Value::Bool(true)) {
        return Err(RenderError::bad_request(
            "forceAppendix is not supported for animated SVGs",
        ));
    }
    let sketch = options.get("sketch") == Some(&Value::Bool(true));
    let no_xml = options.get("noXMLTag") == Some(&Value::Bool(true));
    let dark_overrides_supplied = options
        .get("darkThemeOverrides")
        .is_some_and(|v| !v.is_null());
    let pad = options.get("pad").and_then(Value::as_i64).unwrap_or(100);
    let options = native_options(&options);
    if isometric {
        let overrides = |values: &BTreeMap<String, String>| {
            serde_json::from_value(Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect(),
            ))
            .map_err(|error| error.to_string())
        };
        let rendered = (|| {
            let document = d2_rs_render::SvgRenderOptions {
                pad: Some(pad),
                sketch: Some(sketch),
                isometric: Some(true),
                theme_id: options.theme_id,
                dark_theme_id: options.dark_theme_id,
                theme_overrides: if options.theme_overrides.is_empty() {
                    None
                } else {
                    Some(overrides(&options.theme_overrides)?)
                },
                dark_theme_overrides: if dark_overrides_supplied {
                    Some(overrides(&options.dark_theme_overrides)?)
                } else {
                    None
                },
                scale: options.scale,
                center: Some(options.center),
                salt: options.salt.clone(),
                no_xml_tag: Some(no_xml),
                ..Default::default()
            };
            let bytes =
                d2_rs_isometric::svg::render_isometric(None, Some(&board), &document, None)?;
            String::from_utf8(bytes).map_err(|error| error.to_string())
        })();
        return rendered.map_err(|e: String| format!("render failed: render failed: {e}").into());
    }
    let bytes = board
        .render_wasm_svg(
            &options,
            sketch,
            no_xml,
            interval.max(0) as u64,
            descendants,
        )
        .map_err(RenderError::from)?;
    String::from_utf8(bytes).map_err(|error| error.to_string().into())
}
fn native_options(options: &Map<String, Value>) -> RenderOptions {
    let colors = |name: &str| -> BTreeMap<String, String> {
        options
            .get(name)
            .and_then(Value::as_object)
            .map(|colors| {
                colors
                    .iter()
                    .filter_map(|(key, value)| {
                        value
                            .as_str()
                            .map(|value| (key.to_ascii_uppercase(), value.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    RenderOptions {
        pad: options.get("pad").and_then(Value::as_i64).unwrap_or(100) as f64,
        scale: options.get("scale").and_then(Value::as_f64),
        center: options.get("center") == Some(&Value::Bool(true)),
        theme_id: options.get("themeID").and_then(Value::as_i64),
        dark_theme_id: options.get("darkThemeID").and_then(Value::as_i64),
        theme_overrides: colors("themeOverrides"),
        dark_theme_overrides: colors("darkThemeOverrides"),
        salt: options
            .get("salt")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        force_appendix: options.get("forceAppendix") == Some(&Value::Bool(true)),
        ..Default::default()
    }
}
#[cfg(all(test, feature = "oracle-fixtures"))]
mod tests {
    use super::*;

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn original_master_isometric_compile_and_render_calls() {
        let cases: Vec<Value> = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/isometric-master.json"
        ))
        .unwrap();
        assert_eq!(cases.len(), 12);
        for case in cases {
            let request: Value = serde_json::from_str(case["request"].as_str().unwrap()).unwrap();
            let input = &request["input"];
            let got: Value = if request["operation"] == "compile" {
                let prepared: Value = serde_json::from_str(
                    &public_compile_request::prepare(&input.to_string()).unwrap(),
                )
                .unwrap();
                let source = prepared["fs"][prepared["inputPath"].as_str().unwrap()]
                    .as_str()
                    .unwrap();
                let compiled: Value = serde_json::from_str(
                    &crate::compile_d2_target_json_with_policy(
                        source,
                        &prepared["sourceOptions"].to_string(),
                        true,
                    )
                    .unwrap(),
                )
                .unwrap();
                serde_json::json!({"data":{"diagram":compiled["diagram"], "renderOptions":compiled["source_render_options"]}})
            } else {
                serde_json::from_str(&response(
                    &input["diagram"].to_string(),
                    &input["options"].to_string(),
                ))
                .unwrap()
            };
            assert_eq!(got, case["got"], "{}", case["name"]);
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn original_master_render_admission_responses() {
        let cases: Vec<Value> = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/render-admission-master.json"
        ))
        .unwrap();
        assert_eq!(cases.len(), 5);
        for case in cases {
            let request: Value = serde_json::from_str(case["request"].as_str().unwrap()).unwrap();
            let got: Value = serde_json::from_str(&response(
                &request["diagram"].to_string(),
                &request["options"].to_string(),
            ))
            .unwrap();
            assert_eq!(got, case["got"], "{}", case["name"]);
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn legacy_source_animation_keeps_the_original_wrap_xml_declaration() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/target-sdk-source-v090.json"
        ))
        .unwrap();
        let row = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "options-edge-clone-7")
            .unwrap();
        assert_eq!(row["options"]["noXMLTag"], true);
        assert_eq!(row["xml"], true);
        let svg = crate::render_d2_source_impl(
            "a -> b: message",
            r#"{"engine":"dagre","animate_interval":500,"no_xml_tag":true,"scale":0.5,"pad":20}"#,
        )
        .unwrap();
        assert!(svg.starts_with("<?xml"));
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn supplied_svg_and_animated_targets_match_original_go_contracts() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/target-sdk-source-v090.json"
        ))
        .unwrap();
        let rows = fixture["cases"].as_array().unwrap();
        assert_eq!(rows.len(), 117);
        for row in rows {
            let actual = render(&row["diagram"].to_string(), &row["options"].to_string());
            if let Some(error) = row["error"].as_str() {
                assert_eq!(actual.unwrap_err(), error, "{}", row["name"]);
                continue;
            }
            let svg = actual.unwrap();
            if let Some(raw) = row["raw_output"].as_str() {
                assert_eq!(svg, raw, "{}", row["name"]);
                continue;
            }
            assert_eq!(
                svg.starts_with("<?xml"),
                row["xml"].as_bool().unwrap(),
                "{}",
                row["name"]
            );
            let document = roxmltree::Document::parse(&svg).unwrap();
            let root = document.root_element();
            if row["root_omits_fill"] == true {
                let background = document
                    .descendants()
                    .find(|n| n.has_tag_name("rect"))
                    .unwrap();
                assert!(
                    background.attribute("fill").is_none(),
                    "{}: empty target root inherited a theme fill",
                    row["name"]
                );
            }
            for (node, field) in [
                (Some(root), "attributes"),
                (
                    root.children().find(|n| n.has_tag_name("svg")),
                    "inner_attributes",
                ),
            ] {
                if row[field].is_null() {
                    assert!(node.is_none());
                    continue;
                }
                let node = node.unwrap();
                for name in ["width", "height", "viewBox", "preserveAspectRatio"] {
                    assert_eq!(
                        node.attribute(name),
                        row[field][name].as_str(),
                        "{} {field} {name}",
                        row["name"]
                    );
                }
            }
            let frames = row["frames"].as_array().unwrap();
            assert_eq!(
                svg.matches("animation: d2Transition-").count(),
                frames.len(),
                "{}",
                row["name"]
            );
            for frame in frames {
                assert!(
                    svg.contains(&format!(
                        "animation: d2Transition-{}-{} {}ms infinite",
                        frame["hash"].as_str().unwrap(),
                        frame["index"],
                        frame["total_ms"]
                    )),
                    "{}",
                    row["name"]
                );
            }
            if let Some(hash) = row["hash"].as_str() {
                assert!(svg.contains(hash), "{}", row["name"]);
            }
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn raw_ascii_target_matches_go_without_ruler_or_source_compilation() {
        let master: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/render-regressions-master.json"
        ))
        .unwrap();
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/raw-target-render-v090.json"
        ))
        .unwrap();
        let cases = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["options"]["ascii"] == Value::Bool(true))
            .collect::<Vec<_>>();
        assert_eq!(cases.len(), 8);
        for row in cases {
            let expected = &master["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["kind"] == "ascii" && case["name"] == row["name"])
                .unwrap()["got"];
            let actual: Value = serde_json::from_str(&response(
                &row["diagram"].to_string(),
                &row["options"].to_string(),
            ))
            .unwrap();
            assert_eq!(actual, *expected, "{}", row["name"]);
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_exported_legends_render_without_compiling_source() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::load_str(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-render"),
            "tests/fixtures/target-legend-source-v090.json",
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 33);
        for row in cases {
            let svg = render(&row["diagram"].to_string(), &row["options"].to_string()).unwrap();
            assert!(
                svg.contains(row["main_hash"].as_str().unwrap()),
                "{}",
                row["name"]
            );
            assert!(
                svg.contains(&format!("viewBox=\"{}\"", row["viewbox"].as_str().unwrap())),
                "{}",
                row["name"]
            );
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn raw_target_render_does_not_require_source_or_compilation() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::load_str(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-render"),
            "tests/fixtures/public-target-mono-fonts-v090.json",
        ))
        .unwrap();
        for row in fixture["cases"].as_array().unwrap() {
            let diagram = serde_json::to_string(&row["diagram"]).unwrap();
            let options = serde_json::json!({"pad":20,"sketch":row["sketch"],"noXMLTag":true});
            let svg = render(&diagram, &options.to_string()).unwrap();
            assert!(
                svg.contains(row["expected_hash"].as_str().unwrap()),
                "{}",
                row["name"]
            );
            assert!(
                svg.contains(&format!(
                    "viewBox=\"{}\"",
                    row["expected_view_box"].as_str().unwrap()
                )),
                "{}",
                row["name"]
            );
        }
    }
    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn target_selection_and_render_only_option_boundary() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::load_str(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-render"),
            "tests/fixtures/public-target-v090.json",
        ))
        .unwrap();
        let row = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "boards-clone")
            .unwrap();
        let diagram = serde_json::to_string(&row["diagram"]).unwrap();
        assert!(
            render(&diagram, r#"{"target":"layers.alt"}"#)
                .unwrap()
                .starts_with("<?xml")
        );
        assert_eq!(
            render(&diagram, r#"{"target":"missing"}"#).unwrap_err(),
            "render target 'missing' not found"
        );
        assert_eq!(
            render(&diagram, r#"{"target":"*"}"#).unwrap_err(),
            "target '*' only supported for animated SVGs"
        );
        assert_eq!(
            render("null", "{}").unwrap_err(),
            "missing 'diagram' field in input JSON"
        );
        assert_eq!(
            render("{}", r#"{"pad":1.5}"#).unwrap_err(),
            "invalid JSON input"
        );
        assert!(
            render("{}", r#"{"layout":[],"fontRegular":42,"noXMLTag":true}"#)
                .unwrap()
                .starts_with("<svg")
        );
    }
}
