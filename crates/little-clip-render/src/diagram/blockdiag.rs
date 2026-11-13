//! Canonical BlockDiag-family grammars/layouts in an isolated local Python child.

mod sources;

const BOOTSTRAP: &str = include_str!("blockdiag/bootstrap.py");
const FLAVORS: [&str; 6] = [
    "blockdiag",
    "seqdiag",
    "actdiag",
    "nwdiag",
    "packetdiag",
    "rackdiag",
];

pub(super) fn svg(flavor: &str, source: &str) -> Result<(String, Vec<String>), String> {
    if !FLAVORS.contains(&flavor) {
        return Err("Unknown BlockDiag family language".into());
    }
    if source.len() > super::MAX_SOURCE || source.trim().is_empty() {
        return Err("BlockDiag source is empty or exceeds the preview budget".into());
    }
    let (svg, mut warnings) = super::python::Program {
        name: "BlockDiag",
        modules: sources::SOURCES,
        adapter: BOOTSTRAP,
    }
    .call(serde_json::json!({"flavor": flavor, "source": source}))?;
    let (svg, limitations) = super::compose_svg(
        [svg],
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
        super::ViewportPolicy::FitContent,
    )?;
    warnings.extend(limitations);
    warnings.truncate(16);
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLES: [(&str, &str); 6] = [
        (
            "blockdiag",
            include_str!("../../../../docs/examples/diagrams/pipeline.blockdiag"),
        ),
        (
            "seqdiag",
            include_str!("../../../../docs/examples/diagrams/request.seqdiag"),
        ),
        (
            "actdiag",
            include_str!("../../../../docs/examples/diagrams/review.actdiag"),
        ),
        (
            "nwdiag",
            include_str!("../../../../docs/examples/diagrams/service.nwdiag"),
        ),
        (
            "packetdiag",
            include_str!("../../../../docs/examples/diagrams/header.packetdiag"),
        ),
        (
            "rackdiag",
            include_str!("../../../../docs/examples/diagrams/server.rackdiag"),
        ),
    ];

    #[test]
    fn canonical_languages_render_their_own_layout_with_outlined_labels() {
        for (flavor, source) in EXAMPLES {
            let (svg, warnings) =
                self::svg(flavor, source).unwrap_or_else(|error| panic!("{flavor}: {error}"));
            assert!(svg.contains("<path"), "{flavor}");
            assert!(!svg.contains("<text"), "{flavor}");
            assert!(!svg.contains("<image"), "{flavor}");
            assert!(warnings.is_empty(), "{flavor}: {warnings:?}");
            super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
        }
    }

    #[test]
    fn every_language_rejects_external_resources_before_model_building() {
        for flavor in FLAVORS {
            for attribute in [
                "default_icon = \"/etc/shadow\";",
                "background = \"https://example.invalid/pixel.png\";",
                "fontpath = \"/etc/shadow\";",
                "plugins = \"host_plugin\";",
                "plugin attributes [name = x];",
            ] {
                let source = format!("{flavor} {{ {attribute} }}");
                let error = svg(flavor, &source).unwrap_err();
                assert!(error.contains("disabled"), "{source}: {error}");
            }
        }
        for flavor in ["blockdiag", "seqdiag", "actdiag", "nwdiag"] {
            let source = format!("{flavor} {{ class external [icon = \"/etc/shadow\"]; }}");
            assert!(svg(flavor, &source).unwrap_err().contains("disabled"));
        }
    }

    #[test]
    fn syntax_and_source_budget_failures_do_not_return_partial_diagrams() {
        for flavor in FLAVORS {
            let error = svg(flavor, &format!("{flavor} {{ broken [label = }}")).unwrap_err();
            assert!(error.contains("BlockDiag"), "{flavor}: {error}");
        }
        assert!(svg("blockdiag", " ").unwrap_err().contains("empty"));
        assert!(
            svg("blockdiag", &"x".repeat(super::super::MAX_SOURCE + 1))
                .unwrap_err()
                .contains("budget")
        );
        assert!(svg("python", "print(1)").unwrap_err().contains("Unknown"));
    }
}
