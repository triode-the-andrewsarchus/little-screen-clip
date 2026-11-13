//! Native DOT layout, with file loading disabled before any layout begins.
pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let source = source.trim_start_matches('\u{feff}');
    // This library switch rejects image/shapefile/HTML IMG paths before lookup.
    // Set it directly in the disposable worker, without mutating environment
    // variables or trusting a diagram's own imagepath. It survives run resets.
    graphviz_pure_core::usershape::set_server_name(Some("native diagram preview"));
    let mut warnings = preflight(source)?;
    let result = graphviz_pure::render_dot_all(
        source,
        "dot",
        "svg_inline",
        &graphviz_pure::GlobalAttrs::default(),
    )
    .map_err(|error| error.to_string())?;
    let (diagrams, diagnostics) = result.into_parts();
    if let Some(error) = diagnostics.iter().find(|item| {
        matches!(
            item.level(),
            graphviz_pure::DiagnosticLevel::Error | graphviz_pure::DiagnosticLevel::CountedAsError
        )
    }) {
        return Err(error.message().trim().to_owned());
    }
    if diagrams.is_empty() || diagrams.len() > 16 {
        return Err("DOT previews require between one and sixteen graphs".into());
    }
    warnings.extend(
        diagnostics
            .into_iter()
            .map(|item| {
                if item.message().contains("file loading is disabled because") {
                    "Linked image and shape files are disabled in diagram previews".to_owned()
                } else {
                    item.message().trim().to_owned()
                }
            })
            .collect::<Vec<_>>(),
    );
    let diagrams = diagrams.into_iter().map(|svg| {
        // The inline writer expects an SVG host and omits its namespaces.
        svg.replacen("<svg ", "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" ", 1)
    });
    let (svg, limitations) = super::compose_svg(
        diagrams,
        super::fonts::metric_fonts(),
        "Tinos",
        super::ImagePolicy::Block,
        super::ViewportPolicy::FitContent,
    )?;
    warnings.extend(limitations);
    Ok((svg, warnings))
}

fn preflight(source: &str) -> Result<Vec<String>, String> {
    // Use the same native parser and character codec as the facade. Inspect
    // count/resources before paying for layout, then drop this temporary model.
    // The facade's second parse supplies its complete normal diagnostics.
    let (graphs, _) = graphviz_pure_core::diag::capture(|| {
        graphviz_pure_parser::parse_dot_all(
            &graphviz_pure_core::charset::from_text(source),
            &graphviz_pure::GlobalAttrs::default(),
        )
    });
    let graphs = graphs
        .map_err(|error| graphviz_pure_core::charset::to_text(&error.to_string()).into_owned())?;
    if graphs.is_empty() || graphs.len() > 16 {
        return Err("DOT previews require between one and sixteen graphs".into());
    }
    Ok(
        if graphs.iter().any(|graph| {
            graph
                .graph_attrs
                .get("stylesheet")
                .is_some_and(|value| !value.is_empty())
        }) {
            vec!["External diagram stylesheets are not loaded".into()]
        } else {
            Vec::new()
        },
    )
}

/// Escapes authored text for generated DOT HTML labels; identities stay ordinal.
pub(super) fn label(text: &str) -> String {
    quick_xml::escape::escape(text)
        .replace('\r', "")
        .replace('\n', "<BR ALIGN=\"LEFT\"/>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_graphviz_renders_all_layout_engines_with_outlined_labels() {
        for engine in [
            "dot",
            "neato",
            "fdp",
            "sfdp",
            "circo",
            "twopi",
            "osage",
            "patchwork",
        ] {
            let source = format!(
                "digraph {{ layout={engine}; a [label=\"Source\"]; b [label=\"Preview\"]; a -> b; b -> c }}"
            );
            let (svg, _) = svg(&source).unwrap_or_else(|error| panic!("{engine}: {error}"));
            assert!(svg.contains("<path"));
            assert!(!svg.contains("<text"));
            super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
        }
        assert!(svg("\u{feff}digraph { a -> b }").is_ok());
    }

    #[test]
    fn multiple_graphs_keep_all_diagrams_and_isolate_gradient_references() {
        let source = "digraph A { node [style=filled fillcolor=\"red:yellow\"]; a -> b }\ndigraph B { node [style=filled fillcolor=\"blue:green\"]; c -> d }";
        let (svg, _) = svg(source).unwrap();
        let document = roxmltree::Document::parse(&svg).unwrap();
        assert_eq!(
            document
                .root_element()
                .children()
                .filter(|node| node.has_tag_name("g"))
                .count(),
            2
        );
        let ids: Vec<_> = document
            .descendants()
            .filter_map(|node| node.attribute("id"))
            .collect();
        let distinct: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids.len(), distinct.len());
        assert!(ids.iter().any(|id| id.starts_with("graph-0-")));
        assert!(ids.iter().any(|id| id.starts_with("graph-1-")));
        super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
        assert!(
            self::svg("digraph { a -> b }\ndigraph { c -> }").is_err(),
            "errors after the first graph must not disappear"
        );
    }

    #[test]
    fn file_assets_are_denied_and_reported_without_opening_their_paths() {
        let directory = tempfile::tempdir().unwrap();
        let png_path = directory.path().join("image.png");
        let eps_path = directory.path().join("shape.eps");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255]))
            .save(&png_path)
            .unwrap();
        std::fs::write(
            &eps_path,
            "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 40 20\n",
        )
        .unwrap();
        let png = serde_json::to_string(&png_path.to_string_lossy()).unwrap();
        let eps = serde_json::to_string(&eps_path.to_string_lossy()).unwrap();
        for source in [
            format!("digraph {{ a [image={png}] }}"),
            format!("digraph {{ a [shape=epsf shapefile={eps}] }}"),
            format!("digraph {{ a [label=<<TABLE><TR><TD><IMG SRC={png}/></TD></TR></TABLE>>] }}"),
        ] {
            match svg(&source) {
                Ok((_, warnings)) => assert!(
                    warnings
                        .iter()
                        .any(|warning| warning.contains("disabled") || warning.contains("unsafe")),
                    "{warnings:?}"
                ),
                Err(error) => assert!(
                    error.contains("image") || error.contains("label"),
                    "{error}"
                ),
            }
        }
        let (_, warnings) = svg("digraph { stylesheet=\"file:///etc/shadow\"; a -> b }").unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("stylesheets are not loaded"))
        );
        assert!(
            svg(&"digraph {}\n".repeat(17))
                .unwrap_err()
                .contains("sixteen")
        );
    }
}
