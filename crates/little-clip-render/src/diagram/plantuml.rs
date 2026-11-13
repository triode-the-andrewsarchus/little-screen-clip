//! Native PlantUML, with bundled-only preprocessing and shared SVG composition.
use std::rc::Rc;

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    if source.trim().is_empty() {
        return Err("PlantUML source is empty".into());
    }
    let mut blocks = plantuml_core::preproc::split_complete_blocks(source, &[])?;
    if blocks.is_empty() {
        // Markdown integrations commonly omit UML delimiters. Wrap only the
        // preview copy; an explicit unfinished @start block remains an error.
        blocks = plantuml_core::preproc::split_complete_blocks(
            &format!("@startuml\n{source}\n@enduml"),
            &[],
        )?;
    }
    if blocks.is_empty() || blocks.len() > 16 {
        return Err(
            "PlantUML previews require one to sixteen complete @start/@end diagram blocks".into(),
        );
    }
    // Share the same Graphviz instance and its resource policy as direct DOT.
    graphviz_pure_core::usershape::set_server_name(Some("native diagram preview"));
    let mut diagrams = Vec::new();
    let mut warnings = Vec::new();
    for block in blocks {
        let source = block
            .iter()
            .map(|line| line.string())
            .collect::<Vec<_>>()
            .join("\n");
        let report = plantuml_core::render_report(
            &source,
            Rc::new(plantuml_core::tim::resolver::BundledResolver),
            false,
        )
        .map_err(|error| format!("PlantUML: {error}"))?;
        if report.svg.len() > super::MAX_SVG {
            return Err("PlantUML generated SVG exceeds the preview budget".into());
        }
        diagrams.push(report.svg);
        warnings.extend(report.warnings);
    }
    let (svg, limitations) = super::compose_svg(
        diagrams,
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::EmbeddedRaster,
        super::ViewportPolicy::FitContent,
    )?;
    warnings.extend(limitations);
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_native_uml_c4_and_extended_families() {
        for source in [
            "@startuml\nAlice -> Bob: Hello\nBob --> Alice: Ack\n@enduml",
            "Alice -> Bob: Bare UML fence",
            "@startuml\nclass Person {\n +name: String\n}\nPerson --> Address\n@enduml",
            "@startuml\n[*] --> Ready\nReady --> Active : start\nActive --> [*]\n@enduml",
            "@startuml\nstart\n:Read source;\nif (valid?) then (yes)\n:Render;\nelse (no)\n:Report error;\nendif\nstop\n@enduml",
            "@startmindmap\n* Files\n** Diagrams\n** Photos\n@endmindmap",
            "@startjson\n{\"message\":\"Hello\",\"count\":3}\n@endjson",
            "@startsalt\n{\n[OK] | [Cancel]\n}\n@endsalt",
            "@startuml\n!include <C4/C4_Container>\nPerson(user, \"Reader\")\nSystem(app, \"File manager\")\nRel(user, app, \"Browses\")\n@enduml",
        ] {
            let (svg, warnings) = svg(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            assert!(svg.contains("<path"));
            // usvg can retain a whitespace-only text span (C4 uses NBSP for
            // padding). All actual label glyphs must have become paths.
            let document = roxmltree::Document::parse(&svg).unwrap();
            assert!(
                document
                    .descendants()
                    .filter(|n| n.has_tag_name("text"))
                    .all(|n| {
                        n.descendants()
                            .filter(|child| child.is_text())
                            .all(|child| child.text().unwrap_or_default().trim().is_empty())
                    }),
                "{source}"
            );
            assert!(!svg.contains("<script"));
            assert!(warnings.is_empty(), "{source}: {warnings:?}");
        }
    }

    #[test]
    fn reports_external_include_and_parser_failures_instead_of_using_host_resources() {
        for source in [
            "@startuml\n!include /etc/passwd\nAlice -> Bob\n@enduml",
            "@startuml\n!includeurl https://example.com/diagram.puml\nAlice -> Bob\n@enduml",
            "@startuml\n!include <Missing/Resource>\nAlice -> Bob\n@enduml",
            "@startuml\nclass {\n@enduml",
            "@startuml\nAlice -> Bob",
            "@startuml\nA -> B\n@enduml\n@startuml\nC -> D",
        ] {
            assert!(svg(source).is_err(), "{source}");
        }
    }

    #[test]
    fn c4_bundled_sprite_survives_the_final_png_rasterization() {
        let source = "@startuml\n!include <C4/C4_Container>\nPerson(user, \"Reader\")\nSystem(app, \"File manager\")\nRel(user, app, \"Browses\")\n@enduml";
        let rendered =
            super::super::render_inline(little_clip_core::diagram::Format::Plantuml, source, None)
                .unwrap();
        assert!(rendered.svg.contains("<image"));
        assert!(rendered.warnings.is_empty());
        let blocked = super::super::rasterize_svg(
            rendered.svg.clone(),
            true,
            super::super::ImagePolicy::Block,
        )
        .unwrap();
        assert_ne!(
            rendered.png, blocked.png,
            "the PNG must retain the bundled person sprite"
        );
    }

    #[test]
    fn preserves_preprocessor_and_renderer_warnings_in_the_report() {
        let (_, warnings) =
            svg("@startuml\nA -> B\n/ note over B: old-style note\n@enduml").unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("deprecated")),
            "{warnings:?}"
        );
    }

    #[test]
    fn keeps_all_complete_diagrams_and_bounds_the_plane_count() {
        let source = "@startuml\nAlice -> Bob: first\n@enduml\n@startuml\nclass Second\n@enduml";
        let (svg, _) = svg(source).unwrap();
        assert!(svg.contains("graph-0"));
        assert!(svg.contains("graph-1"));
        assert!(
            super::svg(&"@startuml\nA -> B\n@enduml\n".repeat(17))
                .unwrap_err()
                .contains("sixteen")
        );
    }
}
