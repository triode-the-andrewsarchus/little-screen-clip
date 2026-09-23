//! Screen-relative sizing for the fixed-size workbench popup.

use std::time::Duration;

use eframe::egui::{Vec2, ViewportBuilder};
use little_clip_core::process::CommandRequest;
use serde::Deserialize;

const SCREEN_FRACTION: f32 = 0.92;
const FALLBACK_SIZE: Vec2 = Vec2::new(960.0, 540.0);

#[derive(Deserialize)]
struct Output {
    logical: Option<LogicalOutput>,
}

#[derive(Deserialize)]
struct LogicalOutput {
    width: u32,
    height: u32,
}

fn size_from_output(json: &[u8]) -> Result<Vec2, String> {
    let output: Option<Output> = serde_json::from_slice(json).map_err(|err| err.to_string())?;
    let logical = output
        .and_then(|output| output.logical)
        .filter(|logical| logical.width > 0 && logical.height > 0)
        .ok_or_else(|| "no active logical output".to_owned())?;
    // Niri already accounts for scale and rotation in these dimensions.
    Ok(Vec2::new(
        (logical.width as f32 * SCREEN_FRACTION).floor().max(1.0),
        (logical.height as f32 * SCREEN_FRACTION).floor().max(1.0),
    ))
}

fn discover_size() -> Result<Vec2, String> {
    if std::env::var_os("NIRI_SOCKET").is_none() {
        return Err("Niri IPC is unavailable".to_owned());
    }
    // Reuse the bounded process boundary; no daemon or visible probe window is needed.
    let output = CommandRequest::new("niri")
        .args(["msg", "--json", "focused-output"])
        .timeout(Duration::from_millis(250))
        .max_output_bytes(64 * 1024)
        .run()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err("Niri output query failed".to_owned());
    }
    size_from_output(&output.stdout)
}

fn fixed_viewport(viewport: ViewportBuilder, size: Vec2) -> ViewportBuilder {
    viewport
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size)
        .with_resizable(false)
        .with_maximized(false)
        .with_maximize_button(false)
}

pub(crate) fn window_builder(viewport: ViewportBuilder) -> ViewportBuilder {
    let size = discover_size().unwrap_or_else(|err| {
        tracing::warn!(%err, "Using fallback workbench popup size");
        FALLBACK_SIZE
    });
    tracing::info!(
        width = size.x,
        height = size.y,
        "Opening fixed workbench popup"
    );
    // This hook runs after eframe's initial size selection, before native creation.
    fixed_viewport(viewport, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_logical_dimensions_without_applying_output_scale_twice() {
        let size = size_from_output(
            br#"{"modes":[{"width":3840,"height":2160}],"logical":{"width":2560,"height":1440,"scale":1.5}}"#,
        )
        .unwrap();
        assert_eq!(size, Vec2::new(2355.0, 1324.0));
    }

    #[test]
    fn retains_output_proportions_and_fits_small_screens() {
        for (width, height) in [(1280, 720), (1920, 1200), (3440, 1440), (640, 480)] {
            let json = format!(r#"{{"logical":{{"width":{width},"height":{height}}}}}"#);
            let size = size_from_output(json.as_bytes()).unwrap();
            assert!(size.x < width as f32 && size.y < height as f32);
            assert!((size.x / size.y - width as f32 / height as f32).abs() < 0.01);
        }
    }

    #[test]
    fn rejects_missing_disabled_and_invalid_outputs() {
        for json in [
            "null",
            r#"{"logical":null}"#,
            r#"{"logical":{"width":0,"height":1080}}"#,
            r#"{"logical":{"width":1920,"height":0}}"#,
            "invalid JSON",
        ] {
            assert!(size_from_output(json.as_bytes()).is_err());
        }
    }

    #[test]
    fn fixed_constraints_override_previous_geometry_before_opening() {
        let size = Vec2::new(1177.0, 662.0);
        let viewport = fixed_viewport(
            ViewportBuilder::default()
                .with_inner_size([3000.0, 2000.0])
                .with_resizable(true)
                .with_maximized(true),
            size,
        );
        assert_eq!(viewport.inner_size, Some(size));
        assert_eq!(viewport.min_inner_size, Some(size));
        assert_eq!(viewport.max_inner_size, Some(size));
        assert_eq!(viewport.resizable, Some(false));
        assert_eq!(viewport.maximized, Some(false));
        assert_eq!(viewport.maximize_button, Some(false));
    }
}
