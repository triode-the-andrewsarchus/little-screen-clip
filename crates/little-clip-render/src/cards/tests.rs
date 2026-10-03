use super::*;
use little_clip_core::config::AppConfig;

#[test]
fn code_file_extension_takes_precedence_over_spreadsheet_heuristics() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    // Semicolons in consecutive imports satisfy the table shape heuristic.
    let source = "use crate::one;\nuse crate::two;\nuse crate::three;";
    assert!(spreadsheet::detect_csv_text(source).is_some());
    let mut code = Pixmap::new(500, 400).unwrap();
    let mut table = code.clone();
    renderer.draw_text_preview(
        &mut code,
        source,
        Some("rs"),
        0.0,
        0.0,
        500.0,
        400.0,
        true,
        Color::WHITE,
    );
    renderer.draw_text_preview(
        &mut table,
        source,
        None,
        0.0,
        0.0,
        500.0,
        400.0,
        true,
        Color::WHITE,
    );
    assert_ne!(
        code.data(),
        table.data(),
        "Rust source must use the code preview"
    );
}

#[test]
fn text_preview_rasters_have_readable_ink_on_light_and_dark_cards() {
    use little_clip_core::{config::WorkbenchAppearance, palette::contrast_ratio};

    for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
        let mut config = AppConfig::default();
        config.workbench.appearance = appearance;
        let renderer = ScreenRenderer::new(config).unwrap();
        for surface in [
            &renderer.palette.colors.surface_raised,
            &renderer.palette.colors.surface_panel,
            &renderer.palette.colors.surface_recessed,
        ] {
            let (r, g, b, _) = parse_hex_color(surface);
            for (source, extension) in [
                ("Ordinary clipboard text", Some("txt")),
                ("let value = 42;", Some("rs")),
            ] {
                let mut raster = Pixmap::new(400, 200).unwrap();
                raster.fill(Color::from_rgba8(r, g, b, 255));
                renderer.draw_text_preview(
                    &mut raster,
                    source,
                    extension,
                    0.0,
                    0.0,
                    400.0,
                    200.0,
                    true,
                    Color::WHITE,
                );
                // Anti-aliased edges blend into the surface; solid glyph interiors
                // must still provide enough readable ink in the rendered preview.
                let ink_pixels = raster
                    .data()
                    .chunks_exact(4)
                    .filter(|pixel| {
                        contrast_ratio((r, g, b), (pixel[0], pixel[1], pixel[2])) >= 4.5
                    })
                    .count();
                assert!(
                    ink_pixels > 30,
                    "{appearance:?} {source:?} on {surface} is faint"
                );
            }
        }
    }
}
