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

#[test]
fn text_preview_syntax_resolution_matches_workbench_supported_languages() {
    assert_eq!(find_syntax("ts").name, "TypeScript");
    assert_eq!(find_syntax("tsx").name, "TypeScript");
    assert_eq!(find_syntax("toml").name, "Configuration");
    assert_eq!(find_syntax("ini").name, "Configuration");
    assert_eq!(find_syntax("kdl").name, "Configuration");
    assert_eq!(find_syntax("objc").name, "Objective-C");
    assert_eq!(find_syntax("rs").name, "Rust");
    assert_eq!(find_syntax("py").name, "Python");
    assert_eq!(find_syntax("kt").name, "Java");
    assert_eq!(find_syntax("zig").name, "C");
    assert_eq!(find_syntax("swift").name, "Rust");
}

#[test]
fn text_preview_rasters_render_syntax_highlighted_code() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();

    for (code, ext) in [
        ("const greeting: string = \"hello\";", "ts"),
        (
            "[package]\nname = \"little-clip\"\nversion = \"0.1.0\"",
            "toml",
        ),
        ("[database]\nserver = 192.0.2.62\nport = 1433", "ini"),
        ("fn main() {\n    let answer = 42;\n}", "rs"),
        (
            "def greet(name: str) -> str:\n    return f\"Hello, {name}\"",
            "py",
        ),
        ("fun main() {\n    val greeting = \"hello\"\n}", "kt"),
    ] {
        let mut pixmap = Pixmap::new(400, 200).unwrap();
        pixmap.fill(Color::BLACK);
        renderer.draw_text_preview(
            &mut pixmap,
            code,
            Some(ext),
            0.0,
            0.0,
            400.0,
            200.0,
            true,
            Color::WHITE,
        );

        // Collect distinct rendered non-background colors in the pixmap.
        // A syntax-highlighted snippet must produce multiple distinct foreground hues.
        let mut distinct_colors = std::collections::HashSet::new();
        for pixel in pixmap.data().chunks_exact(4) {
            let color = (pixel[0], pixel[1], pixel[2]);
            if color != (0, 0, 0) {
                distinct_colors.insert(color);
            }
        }
        assert!(
            distinct_colors.len() >= 3,
            "Expected multiple syntax colors for {ext}, got {} colors",
            distinct_colors.len()
        );
    }
}

#[test]
fn office_document_file_batch_renders_page_thumbnail_preview() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let rtf_path = dir.path().join("preview_card.rtf");
    std::fs::write(
        &rtf_path,
        b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Courier;}}\\f0\\fs24 Office Document Card Preview\\par Page 1.}",
    )
    .unwrap();

    let mut item = ClipItem::from_text("preview_card.rtf");
    item.payload = ClipPayload::FileBatch(vec![rtf_path.clone()]);
    item.title = "preview_card.rtf".to_string();
    item.subtitle = "RTF (1 pg)".to_string();
    item.details = vec!["1 pages".to_string(), "RTF".to_string()];
    item.handler_tag = "RTF (DOCUMENT)".to_string();

    let mut pixmap = Pixmap::new(500, 400).unwrap();
    pixmap.fill(Color::BLACK);
    renderer.draw_item_content(
        &mut pixmap,
        &item,
        0.0,
        0.0,
        500.0,
        400.0,
        true,
        Color::WHITE,
    );

    let non_black_count = pixmap
        .data()
        .chunks_exact(4)
        .filter(|p| (p[0], p[1], p[2]) != (0, 0, 0))
        .count();
    assert!(
        non_black_count > 500,
        "Expected thumbnail preview to be rendered on card"
    );
}
