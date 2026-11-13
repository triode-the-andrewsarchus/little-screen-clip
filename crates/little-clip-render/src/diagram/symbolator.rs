//! Real Symbolator HDL interface extraction and canonical symbol rendering.
mod sources;

const EXTRACTION_WARNING: &str = "Symbolator extracts declared interfaces; it does not compile or elaborate HDL, resolve external libraries, or validate implementation behavior";

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let (pages, mut warnings) = pages(source)?;
    let (svg, limitations) = super::compose_svg(
        pages,
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
        super::ViewportPolicy::FitContent,
    )?;
    warnings.extend(limitations);
    warnings.truncate(16);
    Ok((svg, warnings))
}

fn pages(source: &str) -> Result<(Vec<String>, Vec<String>), String> {
    if source.trim().is_empty() || source.len() > super::MAX_SOURCE {
        return Err("Symbolator source is empty or exceeds the preview budget".into());
    }
    let (output, mut warnings) = super::python::Program {
        name: "Symbolator",
        modules: sources::SOURCES,
        adapter: include_str!("symbolator/bootstrap.py"),
    }
    .call(serde_json::json!({"source": source}))?;
    let pages: Vec<String> = serde_json::from_str(&output)
        .map_err(|error| format!("Invalid Symbolator interface result: {error}"))?;
    if pages.is_empty() || pages.len() > 16 {
        return Err("Symbolator previews require between one and sixteen HDL interfaces".into());
    }
    warnings.push(EXTRACTION_WARNING.into());
    Ok((pages, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTITY: &str = include_str!("../../../../docs/examples/diagrams/entity.symbolator.vhdl");
    const MODULE: &str = include_str!("../../../../docs/examples/diagrams/module.symbolator.v");
    const COMPONENT: &str =
        include_str!("../../../../docs/examples/diagrams/component.symbolator.vhdl");

    #[test]
    fn canonical_parser_fixes_keep_first_ansi_port_named_entity_and_optional_component_is() {
        for (source, expected) in [
            (
                ENTITY,
                vec![
                    "sample_counter",
                    "WIDTH",
                    "clk",
                    "reset_n",
                    "enable",
                    "count",
                    "bus_io",
                ],
            ),
            (
                MODULE,
                vec![
                    "packet_buffer",
                    "WIDTH",
                    "clk",
                    "reset_n",
                    "enable",
                    "data_in",
                    "data_out",
                    "bus_io",
                ],
            ),
            (
                COMPONENT,
                vec!["mixer", "WIDTH", "clk", "left_in", "right_in", "result"],
            ),
        ] {
            let (pages, warnings) = pages(source).unwrap();
            assert_eq!(pages.len(), 1);
            let document = roxmltree::Document::parse(&pages[0]).unwrap();
            let labels: Vec<_> = document
                .descendants()
                .filter(|node| node.has_tag_name("text"))
                .filter_map(|node| node.text())
                .collect();
            for label in expected {
                assert!(labels.contains(&label), "missing {label}: {labels:?}");
            }
            assert!(pages[0].contains("id=\"clock\""));
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.contains("does not compile"))
            );
            let (svg, _) = self::svg(source).unwrap();
            assert!(svg.contains("<path"));
            assert!(!svg.contains("<text"));
            super::super::rasterize_svg(svg, true, super::super::ImagePolicy::Block).unwrap();
        }
    }

    #[test]
    fn multiple_modules_remain_distinct_and_clock_bubble_bus_markers_are_preserved() {
        let (pages, _) = pages(&format!(
            "{MODULE}\nmodule second(input wire a, output wire b); endmodule\n"
        ))
        .unwrap();
        assert_eq!(pages.len(), 2);
        for marker in ["clock", "bubble", "arrow_fwd", "arrow_back"] {
            assert!(pages[0].contains(&format!("id=\"{marker}\"")), "{marker}");
        }
        assert!(pages[0].contains("stroke-width=\"3\""));
        assert!(pages[1].contains("second"));
    }

    #[test]
    fn authored_section_markup_is_literal_text_without_svg_resources() {
        let source = ENTITY.replace(
            "data|Data",
            "data|<image href=\"file:///etc/shadow\"/> & <script>ignored</script>",
        );
        let (pages, _) = pages(&source).unwrap();
        let document = roxmltree::Document::parse(&pages[0]).unwrap();
        assert!(
            !document
                .descendants()
                .any(|node| { node.has_tag_name("image") || node.has_tag_name("script") })
        );
        let (svg, _) = self::svg(&source).unwrap();
        assert!(!svg.contains("<image"));
        assert!(!svg.contains("<script"));
    }

    #[test]
    fn incomplete_interfaces_and_external_preprocessing_fail_without_partial_output() {
        for (source, diagnostic) in [
            (
                "`include \"/etc/shadow\"\nmodule bad(input wire a); endmodule",
                "disabled",
            ),
            (
                "import \"DPI-C\" function int run();\nmodule bad(input wire a); endmodule",
                "disabled",
            ),
            ("module broken(input wire a; endmodule", "delimiter"),
            ("module broken(input wire a);", "Incomplete"),
            ("entity broken is port(a: in std_logic);", "Incomplete"),
            (
                "module valid(input wire a); endmodule\nmodule unfinished(input wire b);",
                "Incomplete",
            ),
            (
                "module valid(input wire a); endmodule\n/* unfinished",
                "Unterminated",
            ),
            ("not a hardware declaration", "interfaces"),
        ] {
            let error = svg(source).unwrap_err();
            assert!(error.contains(diagnostic), "{source}: {error}");
        }
        assert!(svg(" ").unwrap_err().contains("empty"));
        assert!(
            svg(&"x".repeat(super::super::MAX_SOURCE + 1))
                .unwrap_err()
                .contains("budget")
        );
    }
}
