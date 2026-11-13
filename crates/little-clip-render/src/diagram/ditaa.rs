//! Canonical Ditaa in a separate, bounded local Java companion.
pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let svg = super::java::svg(
        "Ditaa",
        source,
        env!("LITTLE_CLIP_DITAA_HELPER_NAME"),
        env!("LITTLE_CLIP_DITAA_HELPER_PATH"),
    )?;
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

    #[test]
    fn canonical_tags_colors_shared_edges_and_wrappers_render() {
        for source in [
            include_str!("../../../../docs/examples/diagrams/pipeline.ditaa"),
            include_str!("../../../../docs/examples/diagrams/shapes.ditaa"),
            "@startditaa --no-shadows, scale=0.7\n+------+\n|cRED A|\n+------+\n@endditaa",
        ] {
            let (svg, warnings) = svg(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            assert!(svg.contains("<path"));
            assert!(!svg.contains("<text"));
            assert!(!svg.contains("<script"));
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }

    #[test]
    fn labels_cannot_escape_svg_text() {
        let (svg, _) = svg(
            "+------------------------+\n| ]]> <script> & 'quotes' |\n+------------------------+",
        )
        .unwrap();
        assert!(!svg.contains("<script"));
        assert!(!svg.contains("CDATA"));
        assert!(svg.contains("<path"));
    }

    #[test]
    fn rejected_inputs_keep_actionable_diagnostics() {
        for (source, expected) in [
            ("".to_owned(), "empty"),
            ("x".repeat(257), "columns"),
            ("x\n".repeat(129), "rows"),
            ("x".repeat(256 * 1024 + 1), "KiB"),
            ("@startditaa\n+---+".to_owned(), "@endditaa"),
            ("@startditaa --html\nx\n@endditaa".to_owned(), "option"),
            ("x\u{0}".to_owned(), "control"),
        ] {
            let error = svg(&source).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
    }
}
