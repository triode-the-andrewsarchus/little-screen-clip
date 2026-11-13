//! Canonical Bytefield-SVG DSL in the existing isolated, hostless interpreter.
const BRIDGE: &str = r#"
globalThis.__renderBytefield = (() => {
    const generate = returnExports;
    // SCI throws Clojure data objects rather than native JS Errors. Preserve
    // their useful line/column/message instead of an opaque QuickJS pointer.
    return source => {
        try { return generate(source); }
        catch (error) { throw new Error(String(error)); }
    };
})();
globalThis.returnExports = undefined;
"#;

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let svg = super::script::Program {
        name: "Bytefield",
        stack_bytes: 4 * 1024 * 1024,
        scripts: &[include_str!("../../../../vendor/bytefield-svg/lib.js")],
        bridge: BRIDGE,
        entry: "__renderBytefield",
    }
    .call(source.trim_start_matches('\u{feff}'))?;
    let document = super::svg_document(&svg).map_err(|error| format!(
        "Bytefield generated invalid SVG: {error}. Labels follow upstream XML conventions: escape & as &amp; and < as &lt;."
    ))?;
    if !document.descendants().any(|node| {
        matches!(
            node.tag_name().name(),
            "text" | "path" | "rect" | "line" | "circle" | "ellipse" | "polygon" | "polyline"
        )
    }) {
        return Err("Bytefield source did not draw any fields or labels".into());
    }
    super::compose_svg(
        [svg],
        super::fonts::metric_fonts(),
        "Cousine",
        super::ImagePolicy::Block,
        super::ViewportPolicy::FitContent,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::diagram::Format;

    #[test]
    fn renders_upstream_protocol_fixtures_with_real_fields_labels_and_gaps() {
        for source in [
            include_str!("../../../../vendor/bytefield-svg/doc/modules/ROOT/examples/ipv4.edn"),
            include_str!("../../../../vendor/bytefield-svg/doc/modules/ROOT/examples/ipv6.edn"),
            include_str!("../../../../vendor/bytefield-svg/doc/modules/ROOT/examples/tcp.edn"),
            include_str!("../../../../vendor/bytefield-svg/doc/modules/ROOT/examples/udp.edn"),
            include_str!("../../../../vendor/bytefield-svg/doc/modules/ROOT/examples/ethernet.edn"),
            include_str!("../../../../vendor/bytefield-svg/doc/modules/ROOT/examples/ccsds.edn"),
            include_str!("../../../../vendor/bytefield-svg/test.edn"),
        ] {
            let rendered = super::super::render_inline(Format::Bytefield, source, None)
                .unwrap_or_else(|error| panic!("{error}: {}", &source[..source.len().min(120)]));
            assert!(rendered.png.len() > 500);
            assert!(rendered.svg.contains("<path"));
            assert!(!rendered.svg.contains("<text"));
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        }
    }

    #[test]
    fn preserves_full_dsl_functions_styling_and_per_call_isolation() {
        let source = r##"
(def boxes-per-row 8)
(defattrs :header {:fill "#b8d8ec"})
(defn field [s n] (draw-box (str/upper-case s) [:header {:span n}]))
(draw-column-headers)
(field "Type" 2)
(field "Length" 2)
(draw-related-boxes (range 4))
(draw-gap "Payload")
(draw-bottom)
"##;
        let first = svg(source).unwrap();
        assert!(first.0.contains("#b8d8ec"));
        svg("(def boxes-per-row 4) (draw-box 1)").unwrap();
        assert_eq!(first, svg(source).unwrap());
    }

    #[test]
    fn reports_syntax_host_access_empty_and_nonterminating_source_errors() {
        for (source, expected) in [
            ("(draw-box", "EOF"),
            ("(draw-box 0 {:span 17})", "span"),
            ("(slurp \"/etc/passwd\")", "slurp"),
            ("(js/eval \"throw Error('executed')\")", "js/eval"),
            ("(loop [] (recur))", "not allowed"),
            (";; comments only", "did not draw"),
            ("(draw-box \"A & B\")", "escape &"),
        ] {
            let error = svg(source).unwrap_err();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    #[test]
    fn custom_svg_is_inert_and_cannot_load_images_or_execute_scripts() {
        let (svg, warnings) = svg(r#"
(draw-box "&lt;script&gt;alert(1)&lt;/script&gt; &amp; payload" {:span 16})
(append-svg [:script "throw Error('executed')"])
(append-svg [:image {:href "file:///etc/passwd" :width 40 :height 40}])
"#)
        .unwrap();
        assert!(!svg.contains("<script"));
        assert!(!svg.contains("<image"));
        assert!(!svg.contains("file:"));
        assert!(warnings.iter().any(|warning| warning.contains("images")));
    }
}
