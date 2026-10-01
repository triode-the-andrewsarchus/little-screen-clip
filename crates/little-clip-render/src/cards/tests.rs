
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
