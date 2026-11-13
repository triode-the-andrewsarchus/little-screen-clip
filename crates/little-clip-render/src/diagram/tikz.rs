//! Native TikZ/PGF/pgfplots, with resource loading denied in the engine itself.
pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let output = tikra::convert(source.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("TikZ/PGF: {error}"))?;
    if output
        .warnings
        .iter()
        .any(|warning| warning.contains(tikra::RESOURCE_ACCESS_DENIED))
    {
        return Err(format!("TikZ/PGF: {}", tikra::RESOURCE_ACCESS_DENIED));
    }
    let (svg, limitations) = super::compose_svg(
        [output.svg],
        super::fonts::metric_fonts(),
        "Tinos",
        super::ImagePolicy::Block,
        super::ViewportPolicy::Preserve,
    )?;
    let mut warnings = output.warnings;
    warnings.extend(limitations);
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tikz_keeps_paths_macros_scopes_nodes_and_outlined_math() {
        let (svg, warnings) = svg(include_str!(
            "../../../../docs/examples/diagrams/native-tikz.tikz"
        ))
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(svg.contains("#295f98"));
        assert!(!svg.contains("<text"));
        assert!(svg.matches("<path").count() > 10);
        super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
    }

    #[test]
    fn native_pgf_system_layer_and_pgfplots_share_the_svg_pipeline() {
        for source in [
            include_str!("../../../../docs/examples/diagrams/native-pgf.pgf"),
            include_str!("../../../../docs/examples/diagrams/native-pgfplots.pgfplots"),
        ] {
            let (svg, warnings) = svg(source).unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert!(svg.matches("<path").count() > 10);
            super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
        }
    }

    #[test]
    fn resource_requests_fail_before_filesystem_resolution() {
        for source in [
            r"\input{Cargo.toml}\begin{tikzpicture}\draw (0,0)--(1,1);\end{tikzpicture}",
            r"\begin{tikzpicture}\includegraphics{Cargo.toml}\end{tikzpicture}",
            r"\begin{tikzpicture}\includegraphics{unresolved-without-extension}\end{tikzpicture}",
            r"\begin{tikzpicture}\node{\includegraphics{Cargo.toml}};\end{tikzpicture}",
            r"\def\externaldata{Cargo.toml}\begin{tikzpicture}\begin{axis}\addplot table {\externaldata};\end{axis}\end{tikzpicture}",
            r"\begin{tikzpicture}\begin{axis}\addplot table {Cargo.toml};\end{axis}\end{tikzpicture}",
            r"\begin{tikzpicture}\begin{axis}\addplot graphics[xmin=0,xmax=1,ymin=0,ymax=1] {Cargo.toml};\end{axis}\end{tikzpicture}",
            r"\begin{tikzpicture}\immediate\write18{touch forbidden}\draw (0,0)--(1,1);\end{tikzpicture}",
            r"\openout1=forbidden\begin{tikzpicture}\draw (0,0)--(1,1);\end{tikzpicture}",
        ] {
            let error = svg(source).unwrap_err();
            assert!(
                error.contains(tikra::RESOURCE_ACCESS_DENIED),
                "{error}: {source}"
            );
        }
        let denied =
            tikra::engine::pgfplots::table::read_bytes("Cargo.toml", &[], 1024).unwrap_err();
        assert_eq!(denied, tikra::RESOURCE_ACCESS_DENIED);
        assert!(tikra::engine::pgfplots::table::find_file("Cargo.toml", &[]).is_err());
    }

    #[test]
    fn unsupported_commands_keep_diagnostics_and_unusable_source_is_an_error() {
        let (_, warnings) = svg(
            r"\begin{tikzpicture}\draw (0,0)--(1,1);\unsupportedfeature{data};\end{tikzpicture}",
        )
        .unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("unsupportedfeature"))
        );
        let (_, warnings) =
            svg(r"\unknownpreamble\begin{tikzpicture}\draw (0,0)--(1,1);\end{tikzpicture}")
                .unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("unknownpreamble"))
        );
        assert!(svg("ordinary text, not a picture").is_err());
    }

    #[test]
    fn macro_expansion_is_bounded_and_definitions_do_not_leak_between_figures() {
        let error =
            svg(r"\def\again{\again}\begin{tikzpicture}\again\end{tikzpicture}").unwrap_err();
        assert!(error.contains("macro expansion"), "{error}");
        let (_, warnings) = svg(
            r"\def\custompath{\draw (0,0)--(1,1);}\begin{tikzpicture}\custompath\end{tikzpicture}",
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let (_, warnings) =
            svg(r"\begin{tikzpicture}\draw (0,0)--(1,0);\custompath;\end{tikzpicture}").unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("custompath"))
        );
    }
}
