//! Canonical, data-only UMLet rendering in the separate local companion.
pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let svg = super::java::svg(
        "UMLet",
        source,
        env!("LITTLE_CLIP_UMLET_HELPER_NAME"),
        env!("LITTLE_CLIP_UMLET_HELPER_PATH"),
    )?;
    super::compose_svg(
        [svg],
        super::fonts::metric_fonts(),
        "Arimo",
        super::ImagePolicy::Block,
        super::ViewportPolicy::Preserve,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_positioned_elements_and_relations_render() {
        let (svg, warnings) = svg(include_str!(
            "../../../../docs/examples/diagrams/umlet/class-relations.uxf"
        ))
        .unwrap();
        assert!(svg.contains("<path"));
        assert!(!svg.contains("<text"));
        assert!(!svg.contains("<script"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn executable_custom_elements_and_external_xml_fail() {
        for source in [
            "<!DOCTYPE diagram [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><diagram>&x;</diagram>",
            "<diagram><element><id>UMLClass</id><custom_code>System.exit(0);</custom_code></element></diagram>",
            "<diagram><element><id>UMLClass</id><coordinates><x>0</x><y>0</y><w>100</w><h>100</h></coordinates><panel_attributes>_umletcode=\njava.lang.System.exit(0);</panel_attributes></element></diagram>",
            "<diagram><element><id>CustomElement</id></element></diagram>",
        ] {
            assert!(svg(source).is_err(), "{source}");
        }
    }

    #[test]
    fn malformed_xml_and_geometry_report_errors() {
        for source in [
            "",
            "<diagram>",
            "<svg/>",
            "<diagram><element><id>DoesNotExist</id></element></diagram>",
            "<diagram><element><id>UMLClass</id><coordinates><x>0</x><y>0</y><w>999999</w><h>100</h></coordinates></element></diagram>",
        ] {
            assert!(svg(source).is_err(), "{source}");
        }
    }
}
