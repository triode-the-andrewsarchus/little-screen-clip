//! Native mxGraph pages composed with the existing bounded SVG pipeline.
pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let report =
        document_svg_drawio::render(source.as_bytes(), document_svg_drawio::Limits::default())
            .map_err(|error| format!("draw.io: {error}"))?;
    let (svg, limitations) = super::compose_svg(
        report.pages,
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::EmbeddedRaster,
        super::ViewportPolicy::FitContent,
    )?;
    let mut warnings = report.warnings;
    warnings.extend(limitations);
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_pages_outline_labels_and_keep_authored_colors() {
        let source =
            include_str!("../../../../vendor/document-svg-drawio/tests/fixtures/basic.drawio");
        let (svg, warnings) = super::svg(source).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("<path"));
        assert!(svg.to_ascii_lowercase().contains("#dae8fc"));
        let rendered =
            super::super::render_inline(little_clip_core::diagram::Format::Drawio, source, None)
                .unwrap();
        assert!(!rendered.png.is_empty());
    }
}
