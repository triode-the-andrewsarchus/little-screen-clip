//! Canonical Vega and Vega-Lite grammars in the shared bounded interpreter.
use serde::Deserialize;

#[derive(Deserialize)]
struct Chart {
    svg: String,
    warnings: Vec<String>,
}

pub(super) fn svg(source: &str, lite: bool) -> Result<(String, Vec<String>), String> {
    let name = if lite { "Vega-Lite" } else { "Vega" };
    let spec: serde_json::Value = serde_json::from_str(source.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("{name} JSON: {error}"))?;
    if !spec.is_object() {
        return Err(format!("{name} requires a JSON specification object"));
    }
    if let Some(schema) = spec.get("$schema").and_then(serde_json::Value::as_str) {
        let expected = if lite { "/vega-lite/" } else { "/vega/" };
        if !schema.contains(expected) {
            return Err(format!("The declared schema does not match {name}"));
        }
    }
    if !lite
        && [
            "mark", "encoding", "layer", "facet", "repeat", "hconcat", "vconcat", "concat",
        ]
        .iter()
        .any(|key| spec.get(key).is_some())
    {
        return Err("This specification uses Vega-Lite fields; choose the Vega-Lite format".into());
    }
    let input = serde_json::json!({"lite": lite, "spec": spec}).to_string();
    let result = super::script::Program {
        name,
        stack_bytes: 2 * 1024 * 1024,
        scripts: &[
            include_str!("vega/prelude.js"),
            include_str!("../../../../vendor/vega-js/build/vega.min.js"),
            include_str!("../../../../vendor/vega-lite-js/build/vega-lite.min.js"),
        ],
        bridge: include_str!("vega/bridge.js"),
        entry: "__renderVega",
    }
    .call(&input)?;
    let chart: Chart = serde_json::from_str(&result)
        .map_err(|error| format!("{name} generated an invalid chart response: {error}"))?;
    let (svg, limitations) = super::compose_svg(
        [chart.svg],
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
        super::ViewportPolicy::Preserve,
    )?;
    let mut warnings = chart.warnings;
    warnings.extend(limitations);
    warnings.push(
        "Static chart preview: pointer events, tooltips and input bindings are inactive".into(),
    );
    warnings.push(
        "Chart layout uses Vega's headless text-width estimates; labels use bundled fonts".into(),
    );
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_vega_renders_data_transforms_scales_axes_and_marks() {
        let source = include_str!("../../../../docs/examples/diagrams/native-vega.vega.json");
        let (svg, warnings) = svg(source, false).unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("Static chart"))
        );
        assert!(svg.contains("#295f98"));
        assert!(svg.contains("<path"));
        assert!(!svg.contains("<text"));
        super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
    }

    #[test]
    fn canonical_lite_compiles_layers_aggregation_and_facets() {
        for source in [
            include_str!("../../../../docs/examples/diagrams/native-vega-lite.vl.json"),
            r#"{"data":{"values":[{"g":"a","x":1},{"g":"a","x":3},{"g":"b","x":2}]},"facet":{"field":"g","type":"nominal"},"spec":{"mark":"bar","encoding":{"y":{"aggregate":"mean","field":"x","type":"quantitative"}}}}"#,
            r#"{"data":{"values":[{"x":1,"y":2},{"x":3,"y":4}]},"layer":[{"mark":"line"},{"mark":"point"}],"encoding":{"x":{"field":"x","type":"quantitative"},"y":{"field":"y","type":"quantitative"}}}"#,
        ] {
            let (svg, _) = svg(source, true).unwrap();
            assert!(svg.contains("<path"));
            super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
        }
    }

    #[test]
    fn clone_prelude_preserves_compiler_value_types_and_shared_references() {
        let output = super::super::script::Program {
            name: "Vega clone",
            stack_bytes: 512 * 1024,
            scripts: &[include_str!("vega/prelude.js")],
            bridge: r#"globalThis.checkClone = () => {
                const buffer = new ArrayBuffer(4);
                const source = {date: new Date(1234), missing: undefined, number: NaN,
                    map: new Map([['x', new Set([2])]]), bytes: new Uint8Array(buffer),
                    other: new Uint16Array(buffer), holes: new Array(3)};
                source.self = source;
                const result = structuredClone(source);
                if (result === source || result.self !== result || result.date.getTime() !== 1234 ||
                    !Object.hasOwn(result, 'missing') || !Number.isNaN(result.number) ||
                    !result.map.get('x').has(2) || result.bytes.buffer !== result.other.buffer ||
                    result.bytes.buffer === buffer || result.holes.length !== 3 || 0 in result.holes)
                    throw Error('clone lost compiler value semantics');
                return 'ok';
            };"#,
            entry: "checkClone",
        }.call("").unwrap();
        assert_eq!(output, "ok");
    }

    #[test]
    fn linked_resources_are_errors_instead_of_plausible_empty_charts() {
        for uri in [
            "file:///etc/passwd",
            "https://example.invalid/data.json",
            "data:application/json,[1,2]",
        ] {
            let source = serde_json::json!({
                "data": {"url": uri}, "mark": "point",
                "encoding": {"x": {"field": "x", "type": "quantitative"}}
            })
            .to_string();
            assert!(svg(&source, true).unwrap_err().contains("inline data"));
            let source = serde_json::json!({"marks": [{"type":"image", "encode":{"enter":{"url":{"value":uri}}}}]}).to_string();
            assert!(svg(&source, false).unwrap_err().contains("inline data"));
        }
    }

    #[test]
    fn schemas_invalid_expressions_and_runtime_failures_are_reported() {
        for (source, lite) in [
            ("[]", true),
            ("{", false),
            (r#"{"mark":"bar","encoding":{}}"#, false),
            (
                r#"{"$schema":"https://vega.github.io/schema/vega-lite/v6.json"}"#,
                false,
            ),
            (r#"{"mark":"definitely-not-a-mark"}"#, true),
            (
                r#"{"signals":[{"name":"x","update":"require('fs')"}]}"#,
                false,
            ),
            (
                r#"{"signals":[{"name":"x","update":"datum.missing.field"}]}"#,
                false,
            ),
        ] {
            assert!(svg(source, lite).is_err(), "{source}");
        }
    }
}
