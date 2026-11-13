//! Faithful ASCII/UTF-8 rendering in the bundled, data-only upstream helper.
use little_clip_core::CommandRequest;
use std::{path::PathBuf, time::Duration};

fn helper() -> PathBuf {
    let name = env!("LITTLE_CLIP_GOAT_HELPER_NAME");
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(env!("LITTLE_CLIP_GOAT_HELPER_PATH")))
}

pub(super) fn svg(source: &str) -> Result<(String, Vec<String>), String> {
    let source = source.trim_start_matches('\u{feff}');
    let helper = helper();
    if !helper.is_file() {
        return Err(format!(
            "Bundled GoAT helper is missing; distribute {} alongside the application",
            env!("LITTLE_CLIP_GOAT_HELPER_NAME")
        ));
    }
    // GoAT has separate frontends. Select box mode from actual box-drawing
    // characters; ordinary Unicode labels do not select that grammar.
    let box_mode = source
        .chars()
        .any(|c| ('\u{2500}'..='\u{257f}').contains(&c));
    let request = CommandRequest::new(helper)
        .stdin_bytes(source.as_bytes().to_vec())
        .resource_limit(rustix::process::Resource::As, super::MEMORY_LIMIT)
        .resource_limit(rustix::process::Resource::Cpu, 4)
        .timeout(Duration::from_secs(5))
        .max_output_bytes(super::MAX_SVG)
        .own_process_group()
        .die_with_parent();
    let request = if box_mode {
        request.arg("--utf8")
    } else {
        request
    };
    let output = request
        .run()
        .map_err(|error| format!("GoAT helper: {error}"))?;
    if !output.status.success() || output.stdout_truncated {
        return Err(format!(
            "GoAT: {}",
            output.stderr_lossy().chars().take(2048).collect::<String>()
        ));
    }
    let svg = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
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
    fn ascii_and_unicode_modes_render_real_geometry_and_outlined_labels() {
        for source in [
            "    .------------.\no-->|Hello, world +--*\n    '------------'\n",
            "    ╭────────────┐\n○──►│Hello, world├─●\n    └────────────╯\n",
        ] {
            let (svg, warnings) = svg(source).unwrap();
            assert!(svg.contains("<path"));
            assert!(!svg.contains("<text"));
            assert!(!svg.contains("<script"));
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }
    #[test]
    fn input_errors_keep_actionable_diagnostics_and_no_partial_artifact() {
        for (source, text) in [
            ("", "empty"),
            ("a\tb", "tab"),
            (&"x".repeat(257), "columns"),
            (&"x\n".repeat(129), "rows"),
        ] {
            assert!(svg(source).unwrap_err().to_lowercase().contains(text));
        }
    }
}
