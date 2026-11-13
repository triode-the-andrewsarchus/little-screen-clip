//! Unmodified upstream UML syntax/layout in the shared data-only interpreter.
const BRIDGE: &str = r#"
globalThis.__renderNomnoml = (() => {
    const parse = nomnoml.parse;
    const render = nomnoml.renderSvg;
    return source => {
        const parsed = parse(source);
        if (parsed.directives.some(d => d.key.toLowerCase() === 'import'))
            throw Error('Imports are not loaded in diagram previews; inline the imported source');
        let nodes = 0, edges = 0, lines = 0;
        const pending = [[parsed.root, 0]];
        while (pending.length) {
            const [part, depth] = pending.pop();
            nodes += part.nodes.length;
            edges += part.assocs.length;
            lines += part.lines.length;
            if (depth > 32 || nodes > 512 || edges > 2048 || lines > 8192)
                throw Error('Diagram exceeds the node, edge, text or nesting budget');
            for (const node of part.nodes)
                for (const nested of node.parts) pending.push([nested, depth + 1]);
        }
        if (!nodes && !lines) throw Error('Source does not contain a Nomnoml diagram');
        for (const key of ['fontSize', 'zoom', 'leading', 'lineWidth', 'padding', 'spacing', 'gutter', 'edgeMargin', 'gravity', 'arrowSize', 'bendSize']) {
            const value = parsed.config[key];
            if (!Number.isFinite(value) || Math.abs(value) > 16384)
                throw Error('Invalid or excessive layout value: ' + key);
        }
        if (parsed.config.fontSize <= 0 || parsed.config.zoom <= 0 || parsed.config.leading <= 0)
            throw Error('Font size, zoom and line spacing must be positive');
        return render(source);
    };
})();
globalThis.nomnoml = undefined;
"#;

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let source = source.trim_start_matches('\u{feff}');
    let svg = super::script::Program {
        name: "Nomnoml",
        stack_bytes: 512 * 1024,
        scripts: &[
            include_str!("../../../../vendor/graphre/dist/graphre.js"),
            include_str!("../../../../vendor/nomnoml/dist/nomnoml.js"),
        ],
        bridge: BRIDGE,
        entry: "__renderNomnoml",
    }
    .call(source)?;
    let (svg, mut warnings) = super::compose_svg(
        [svg],
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
        super::ViewportPolicy::FitContent,
    )?;
    // Canonical server layout uses its own approximate character-width table;
    // outlining uses the shared bundled fonts. Do not silently present custom
    // font or mixed-script spacing as exact browser-canvas font measurement.
    if !source.is_ascii()
        || source
            .lines()
            .any(|line| line.trim_start().starts_with("#font:"))
    {
        warnings.push("Nomnoml uses estimated text widths; custom fonts and mixed-script labels may need extra padding. Preview fonts come from the bundled font set.".into());
    }
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::diagram::Format;

    #[test]
    fn renders_classifiers_compartments_nested_graphs_and_custom_styles_as_native_artwork() {
        for source in [
            "[Source|+read();+save()] -> [Preview|pixels]",
            "#direction: right\n[<actor>User] -> [<database>Store]",
            "#.service: fill=#00ff00 visual=roundrect\n[<service>Worker|[Input] -> [Output]]",
            "[<table>Values|a|1||b|2]",
        ] {
            let rendered = super::super::render_inline(Format::Nomnoml, source, None).unwrap();
            assert!(rendered.png.len() > 500);
            assert!(rendered.svg.contains("<path"));
            if source.contains("#.service") {
                assert!(rendered.svg.contains("#00ff00"));
            }
            assert!(!rendered.svg.contains("<text"));
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        }
    }

    #[test]
    fn reports_parse_import_and_resource_failures_instead_of_partial_or_external_results() {
        for (source, expected) in [
            ("[Unclosed", "line"),
            ("#import: /etc/passwd\n[A] -> [B]", "Imports"),
            ("#zoom: Infinity\n[A]", "layout"),
            ("#zoom: -1\n[A]", "positive"),
            ("// empty", "does not contain"),
        ] {
            assert!(svg(source).unwrap_err().contains(expected), "{source}");
        }
        let source = (0..513).map(|i| format!("[Node{i}]\n")).collect::<String>();
        assert!(svg(&source).unwrap_err().contains("budget"));
    }

    #[test]
    fn labels_are_data_figures_are_isolated_and_mixed_script_spacing_is_disclosed() {
        let source = "[<note>throw Error('executed') & \\<script\\>] -> [Result]";
        let first = svg(source).unwrap();
        assert!(!first.0.contains("<script"));
        svg("#fill: #00ff00\n[Other] -> [Figure]").unwrap();
        assert_eq!(first, svg(source).unwrap());
        let (svg, warnings) = svg("[日本語|مرحبا|🙂] -> [Preview]").unwrap();
        assert!(svg.contains("<path"));
        assert!(warnings.iter().any(|w| w.contains("estimated text widths")));
    }
}
