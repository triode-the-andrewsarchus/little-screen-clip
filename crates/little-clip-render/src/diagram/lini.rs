//! Compile the original Lini language without filesystem asset resolution.
pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let options = lini::Options {
        static_mode: true,
        no_fs: true,
        ..Default::default()
    };
    let (svg, diagnostics) = lini::compile_str_checked(source, &options).map_err(|error| {
        error
            .display_with_source(source, "diagram.lini")
            .to_string()
    })?;
    if let Some(error) = diagnostics
        .iter()
        .find(|item| item.level == lini::Level::Error)
    {
        return Err(error
            .display_with_source(source, "diagram.lini")
            .to_string());
    }
    // Preserve warnings rather than silently accepting fallback routing or a
    // misspelled name that the language auto-created as a new box.
    let warnings = diagnostics
        .iter()
        .map(|item| item.display_with_source(source, "diagram.lini").to_string())
        .collect();
    Ok((svg, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_lini_keeps_outlined_labels_and_reported_source_warnings() {
        let (svg, warnings) = svg("|box#cat| \"cat\"\ncta -> bird\n").unwrap();
        assert!(svg.contains("<path"));
        assert!(
            !svg.contains("<text"),
            "static labels must not need browser fonts"
        );
        assert!(
            warnings
                .iter()
                .any(|item| item.contains("did you mean 'cat'"))
        );
        assert!(
            warnings
                .iter()
                .any(|item| item.contains("diagram.lini:2:1"))
        );
    }

    #[test]
    fn local_asset_paths_are_refused_before_any_filesystem_lookup() {
        for path in [
            "/etc/shadow",
            "../private.svg",
            "/proc/self/environ",
            "missing.png",
        ] {
            let source = format!("|image| {{ src: \"{path}\"; width: 40; height: 40 }}");
            let error = svg(&source).unwrap_err();
            assert!(error.contains("local files are off"), "{error}");
            assert!(!error.contains("Permission denied"));
            assert!(!error.contains("No such file"));
        }
    }

    #[test]
    fn excessive_warnings_are_summarized_without_losing_the_diagram() {
        let mut source = String::from("{\n");
        for index in 0..40 {
            source.push_str(&format!(".unused{index} {{ fill: red; }}\n"));
        }
        source.push_str("}\n|box| \"Source\"\n");
        let rendered =
            super::super::render_inline(little_clip_core::diagram::Format::Lini, &source).unwrap();
        let warnings = rendered.warnings;
        assert!(rendered.svg.contains("<svg"));
        assert_eq!(warnings.len(), 16);
        assert!(warnings.last().unwrap().contains("25 additional"));
        assert!(
            warnings
                .iter()
                .all(|warning| warning.chars().count() <= 1024)
        );
    }
}
