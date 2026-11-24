use super::*;
use little_clip_core::config::AppConfig;

#[test]
fn spreadsheet_card_paints_one_surface_across_a_merge() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    let mut sheet = spreadsheet::SpreadsheetSheet::new(
        "Forecast".into(),
        vec![
            vec!["Quarterly forecast".into(), "".into(), "".into()],
            vec!["".into(); 3],
        ],
    );
    let mut unmerged = Pixmap::new(360, 300).unwrap();
    renderer.draw_spreadsheet_card(&mut unmerged, "Test", &sheet, 0.0, 0.0, 360.0, 300.0, true);
    sheet.merges.push(spreadsheet::MergedCell {
        rows: 0..2,
        columns: 0..3,
    });
    let mut merged = Pixmap::new(360, 300).unwrap();
    renderer.draw_spreadsheet_card(&mut merged, "Test", &sheet, 0.0, 0.0, 360.0, 300.0, true);

    // Sample away from the title glyphs and across the internal row boundary.
    assert_ne!(unmerged.pixel(300, 180), unmerged.pixel(300, 210));
    assert_eq!(merged.pixel(300, 180), merged.pixel(300, 210));
}

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
        false,
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
        false,
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
                    false,
                );
                // Anti-aliased edges blend into the surface; solid glyph interiors
                // must still provide enough readable ink in the rendered preview.
                let ink_pixels = raster
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
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
            false,
        );

        // Collect distinct rendered non-background colors in the pixmap.
        // A syntax-highlighted snippet must produce multiple distinct foreground hues.
        let mut distinct_colors = std::collections::HashSet::new();
        for pixel in pixmap.data().as_chunks::<4>().0 {
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
fn structured_and_code_previews_reuse_only_the_current_file_revision() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let midi: &[u8] =
        b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x0c\0\x90\x3c\x5a\x60\x80\x3c\0\0\xff\x2f\0";
    let fixtures = [
        ("card.rs", b"fn first() {}" as &[u8]),
        ("card.csv", b"note,velocity\nC4,90\n"),
        ("card.mid", midi),
    ];
    for (name, contents) in fixtures {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        let mut item = ClipItem::from_text(path.to_str().unwrap());
        let first = renderer.file_preview(&item).expect(name);
        item.payload = ClipPayload::FileBatch(vec![path.clone()]);
        let cached = renderer.clone().file_preview(&item).expect(name);
        assert!(std::sync::Arc::ptr_eq(&first, &cached));

        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, contents).unwrap();
        std::fs::rename(replacement, path).unwrap();
        let updated = renderer.file_preview(&item).expect(name);
        assert!(!std::sync::Arc::ptr_eq(&first, &updated));
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

    let page = renderer.file_preview(&item).expect("prepared page");
    let reused = renderer.clone().file_preview(&item).expect("cached page");
    assert!(std::sync::Arc::ptr_eq(&page, &reused));
    std::fs::remove_file(rtf_path).unwrap();
    assert!(renderer.file_preview(&item).is_none());
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
        &mut None,
        false,
        Some(&page),
    );

    let non_black_count = pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| (p[0], p[1], p[2]) != (0, 0, 0))
        .count();
    assert!(
        non_black_count > 500,
        "Expected thumbnail preview to be rendered on card"
    );
}

#[test]
fn prepared_audio_raster_uses_summary_readiness_without_following_its_source_path() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    let render = |waveform, path| {
        let mut item = ClipItem::from_text("Prepared audio");
        item.payload = ClipPayload::AudioRef {
            path,
            duration_ms: 1000,
            sample_rate: 48000,
            channels: 1,
            format: "WAV".into(),
            waveform,
        };
        let mut raster = Pixmap::new(600, 400).unwrap();
        renderer.draw_item_content(
            &mut raster,
            &item,
            0.0,
            0.0,
            600.0,
            400.0,
            true,
            Color::WHITE,
            &mut None,
            false,
            None,
        );
        raster
    };
    let waveform = little_clip_core::AudioWaveform {
        dadaspec: None,
        channels: 1,
        sample_rate: 48000,
        duration_ms: 1000,
        ch1_peaks: vec![0.2, 0.8, 0.3, 0.9],
        ch2_peaks: None,
        spectral_colors: Some(vec![[20, 150, 240]; 4]),
        is_snippet: false,
        snippet_start_ms: 0,
        snippet_duration_ms: 0,
    };
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.wav");
    std::fs::write(
        &source,
        "Not audio: prepared content must already be sufficient",
    )
    .unwrap();
    let pending = render(None, source.clone());
    let mut unavailable = waveform.clone();
    unavailable.ch1_peaks.clear();
    let unavailable = render(Some(unavailable), source.clone());
    let ready = render(Some(waveform.clone()), source);
    assert_ne!(pending.data(), unavailable.data());
    assert_ne!(unavailable.data(), ready.data());
    assert_eq!(
        ready.data(),
        render(Some(waveform), "/dev/zero".into()).data()
    );
}

#[test]
fn prepared_text_preserves_exact_raster_and_keeps_structured_cards_as_fallbacks() {
    use little_clip_core::{SlotId, SlotManager};
    let renderer = ScreenRenderer::fallback(AppConfig::default());
    let mut state = SlotManager::default();
    let mut code = ClipItem::from_text("fn main() {\n    let café = 42;\n}");
    code.title = "example.rs".into();
    state.push_left(code);
    let baseline = renderer.render_pixmap(&state).unwrap();
    let (reference, text) = renderer.render_pixmap_with_previews(&state).unwrap();
    assert_eq!(reference.data(), baseline.data());
    let (_, prepared) = text
        .iter()
        .find(|(slot, _)| *slot == SlotId::LeftHand)
        .unwrap();
    let first_line: String = prepared.lines[0]
        .iter()
        .map(|span| span.text.as_str())
        .collect();
    assert_eq!(first_line, "fn main() {");
    assert!(
        prepared
            .lines
            .iter()
            .flatten()
            .all(|span| span.text.chars().count() <= 512)
    );
    // A CSV preview is a table, not native unformatted source text.
    state.push_left(ClipItem::from_text("name,value\nalpha,42\nbeta,84"));
    let (_, text) = renderer.render_pixmap_with_previews(&state).unwrap();
    assert!(!text.iter().any(|(slot, _)| *slot == SlotId::LeftHand));
    state.push_left(ClipItem::from_text("x".repeat(10_000)));
    let (_, text) = renderer.render_pixmap_with_previews(&state).unwrap();
    let (_, prepared) = text
        .iter()
        .find(|(slot, _)| *slot == SlotId::LeftHand)
        .unwrap();
    assert_eq!(prepared.lines[0][0].text.chars().count(), 512);

    let source = "let café = 42;\r\n".repeat(10_000);
    let mut code = ClipItem::from_text(source);
    code.title = "many-lines.rs".into();
    state.push_left(code);
    let (_, text) = renderer.render_pixmap_with_previews(&state).unwrap();
    let (_, prepared) = text
        .iter()
        .find(|(slot, _)| *slot == SlotId::LeftHand)
        .unwrap();
    assert!(prepared.more_lines > 0);
    assert_eq!(prepared.lines.len() + prepared.more_lines, 10_000);
}

#[test]
fn prepared_file_text_is_retained_after_the_original_file_disappears() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("observed.rs");
    std::fs::write(&path, "fn captured_source() {}\n").unwrap();
    let mut item = ClipItem::from_text("placeholder");
    item.payload = ClipPayload::Text(path.to_string_lossy().into_owned());
    item.title = "observed.rs".into();
    let mut state = SlotManager::default();
    state.push_left(item);
    let renderer = ScreenRenderer::fallback(AppConfig::default());
    let (_, text) = renderer.render_pixmap_with_previews(&state).unwrap();
    std::fs::remove_file(path).unwrap();
    let prepared: String = text
        .iter()
        .flat_map(|(_, text)| text.lines.iter().flatten())
        .map(|span| span.text.as_str())
        .collect();
    assert!(prepared.contains("captured_source"));
}

#[test]
fn failed_clipboard_diagrams_remain_readable_highlighted_text() {
    let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
    // The test executable has no worker dispatch, exercising decoder/process
    // failure as well as incomplete candidates without relying on parser text.
    for source in [
        "digraph {",
        "flowchart LR\nA[",
        "direction: right\na -> {",
        "[A] -> [B]\n[Unclosed",
        "Table broken {",
        "box \"unterminated",
    ] {
        let mut pixmap = Pixmap::new(500, 400).unwrap();
        let preview = renderer
            .draw_text_preview(
                &mut pixmap,
                source,
                None,
                0.0,
                0.0,
                500.0,
                400.0,
                true,
                Color::WHITE,
                true,
            )
            .expect("failed inference must fall back to text");
        let lines: Vec<String> = preview
            .lines
            .iter()
            .map(|line| line.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert_eq!(lines.join("\n"), source, "{source}");
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0));
    }
    for (language, expected) in [
        ("mermaid", "Mermaid"),
        ("d2", "D2"),
        ("graphviz", "Graphviz"),
        ("bpmn", "BPMN"),
        ("wireviz", "WireViz"),
        ("nomnoml", "Nomnoml"),
        ("dbml", "DBML"),
        ("goat", "GoAT"),
        ("bytefield", "Bytefield"),
        ("erd", "Erd"),
        ("plantuml", "PlantUML"),
    ] {
        assert_eq!(find_syntax(language).name, expected);
    }
}

#[test]
fn dadaspec_display_choice_renders_prepared_stereo_without_source_io() {
    let left: Vec<_> = (0..32000)
        .map(|i| (std::f32::consts::TAU * 80.0 * i as f32 / 32000.0).sin() * 0.5)
        .collect();
    let right: Vec<_> = (0..32000)
        .map(|i| (std::f32::consts::TAU * 5000.0 * i as f32 / 32000.0).sin() * 0.5)
        .collect();
    let image = little_clip_core::audio::dadaspec::render_planar(
        &left,
        Some(&right),
        32000,
        300,
        60,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let waveform = little_clip_core::AudioWaveform {
        dadaspec: Some(image),
        channels: 2,
        sample_rate: 32000,
        duration_ms: 1000,
        ch1_peaks: vec![0.5; 200],
        ch2_peaks: Some(vec![0.5; 200]),
        spectral_colors: None,
        is_snippet: false,
        snippet_start_ms: 0,
        snippet_duration_ms: 1000,
    };
    let draw = |enabled, appearance| {
        let mut config = AppConfig::default();
        config.audio.dadaspec = enabled;
        config.workbench.appearance = appearance;
        let renderer = ScreenRenderer::fallback(config);
        let mut raster = Pixmap::new(600, 400).unwrap();
        raster.fill(renderer.color_from_hex(&renderer.palette.colors.surface_panel));
        renderer.draw_audio_waveform(
            &mut raster,
            1000,
            32000,
            2,
            "WAV",
            Some(&waveform),
            0.0,
            0.0,
            600.0,
            400.0,
            true,
        );
        raster
    };
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let spectrogram = draw(true, appearance);
        assert_ne!(spectrogram.data(), draw(false, appearance).data());
        let red = spectrogram
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 50 && f32::from(p[0]) > f32::from(p[2]) * 1.6)
            .count();
        let blue = spectrogram
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[2] > 50 && f32::from(p[2]) > f32::from(p[0]) * 1.6)
            .count();
        assert!(
            red > 500 && blue > 500,
            "stereo image must retain bass and high-frequency halves"
        );
        if let Some(path) = std::env::var_os("LITTLE_CLIP_DADASPEC_SCREENSHOT") {
            spectrogram
                .save_png(std::path::Path::new(&path).with_extension(format!("{appearance:?}.png")))
                .unwrap();
        }
    }
}

#[test]
fn scope_file_preview_renders_calibrated_analog_channels() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scope.wfm");
    let mut capture = vec![0; 280];
    capture[..8].copy_from_slice(&[1, 0xff, 0xff, 0xff, 0xa6, 0xa5, 0x38, 0]);
    capture[8..15].copy_from_slice(b"DS1054Z");
    capture[48] = 1;
    capture[84] = 0xd8;
    capture[88] = 1;
    capture[92..96].copy_from_slice(&280_u32.to_le_bytes());
    capture[116..120].copy_from_slice(&2048_u32.to_le_bytes());
    capture[120..124].copy_from_slice(&0.25_f32.to_le_bytes());
    capture[124] = 1;
    capture[132..136].copy_from_slice(&1.0_f32.to_le_bytes());
    capture[141] = 2;
    capture[260..264].copy_from_slice(&280_u32.to_le_bytes());
    capture.extend((0..2048).map(|i| 55 + (i % 140) as u8));
    std::fs::write(&path, capture).unwrap();

    let mut item = ClipItem::from_text("placeholder");
    item.payload = ClipPayload::Text(path.to_string_lossy().into_owned());
    item.title = "scope.wfm".into();
    let renderer = ScreenRenderer::fallback(AppConfig::default());
    let preview = renderer.file_preview(&item).expect("prepared scope card");
    assert!(matches!(
        preview.as_ref(),
        crate::file_preview::Preview::Scope(scope)
            if scope.model == "DS1054Z" && scope.channels[0].unit == "V"
    ));
    let mut slots = SlotManager::default();
    slots.push_left(item);
    let (pixmap, _) = renderer.render_pixmap_with_previews(&slots).unwrap();
    assert!(pixmap.pixels().iter().any(|pixel| {
        pixel.alpha() == 255 && pixel.red() == 102 && pixel.green() == 207 && pixel.blue() == 252
    }));
}

#[test]
fn brush_file_preview_prepares_and_renders_cohesive_card() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("charcoal_soft.vbr");
    std::fs::write(
        &path,
        "GIMP-VBR\n1.0\nCharcoal Soft\n0.25\n28.0\n0.65\n1.4\n15.0\n",
    )
    .unwrap();
    let mut item = ClipItem::from_text("placeholder");
    item.payload = ClipPayload::Text(path.to_string_lossy().into_owned());
    item.title = "charcoal_soft.vbr".into();
    let renderer = ScreenRenderer::fallback(AppConfig::default());
    let preview = renderer.file_preview(&item).expect("prepared brush card");
    assert!(matches!(
        preview.as_ref(),
        crate::file_preview::Preview::Brush(library) if library.brushes[0].name == "Charcoal Soft"
    ));
    let mut state = SlotManager::default();
    state.push_left(item);
    let (pixmap, _) = renderer.render_pixmap_with_previews(&state).unwrap();
    assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0));
}

#[test]
fn embroidery_cards_render_embedded_thread_colors_and_follow_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Wildflower.PES");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../little-clip-core/tests/fixtures/embroidery/Wildflower.pes");
    std::fs::copy(source, &path).unwrap();

    let renderer = ScreenRenderer::fallback(AppConfig::default());
    let item = ClipItem::from_single_file(&path);
    let mut state = SlotManager::default();
    state.push_left(item.clone());
    let preview = renderer.file_preview(&item).expect("prepared embroidery");
    assert!(matches!(
        preview.as_ref(),
        Preview::Page(page) if page.width == 0 && page.height == 0
    ));
    assert!(std::sync::Arc::ptr_eq(
        &renderer.file_preview(&item).unwrap(),
        &preview
    ));

    let image = renderer.render_pixmap(&state).unwrap();
    for rgb in [[34, 118, 90], [174, 64, 94], [227, 166, 50]] {
        assert!(
            image.data().chunks_exact(4).any(|pixel| pixel[..3] == rgb),
            "missing embroidery thread color {rgb:?} on the display card"
        );
    }

    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, b"no stitches").unwrap();
    std::fs::rename(replacement, path).unwrap();
    assert!(renderer.file_preview(&item).is_none());
}

#[test]
fn sigmf_card_updates_when_its_dataset_changes() {
    use crate::file_preview::Preview;

    let folder = tempfile::tempdir().unwrap();
    let metadata = folder.path().join("signal.sigmf-meta");
    let data = folder.path().join("signal.sigmf-data");
    std::fs::write(
        &metadata,
        r#"{"global":{"core:version":"1.2.0","core:datatype":"ci8","core:sample_rate":1000},"captures":[],"annotations":[]}"#,
    ).unwrap();
    std::fs::write(&data, [0; 16]).unwrap();
    let mut item = ClipItem::from_text("placeholder");
    item.payload = ClipPayload::Text(metadata.to_string_lossy().into_owned());
    let renderer = ScreenRenderer::fallback(AppConfig::default());
    let before = renderer.file_preview(&item).unwrap();
    assert!(matches!(&*before, Preview::Iq(iq) if iq.power.samples.end == 8));
    std::fs::write(&data, [0; 32]).unwrap();
    let after = renderer.file_preview(&item).unwrap();
    assert!(matches!(&*after, Preview::Iq(iq) if iq.power.samples.end == 16));
}

#[test]
#[ignore = "requires the sourced and generated Engineering fixtures"]
fn engineering_fixture_cards() {
    use crate::file_preview::Preview;
    use std::{path::PathBuf, time::Instant};

    let directory = PathBuf::from(
        std::env::var_os("LITTLE_CLIP_ENGINEERING_FIXTURES")
            .expect("Engineering fixture directory"),
    );
    let cases = [
        ("Oscilloscope captures/DS1054Z-C.wfm", "waveform"),
        ("Oscilloscope captures/DS1054Z-A.csv", "waveform"),
        (
            "Oscilloscope captures/Siglent SDS1000X-E V1 sample.bin",
            "waveform",
        ),
        ("Oscilloscope captures/LeCroy WavePro 254HD.trc", "waveform"),
        (
            "Oscilloscope captures/LeCroy WaveRunner 20 trigger sequence.trc",
            "waveform",
        ),
        ("Oscilloscope captures/Keysight DSO-X 1102G.bin", "waveform"),
        ("Power recordings/Auto_Sample.jls", "waveform"),
        ("Power recordings/Standby and load pulses.jls", "waveform"),
        ("Logic captures/Sigrok Amulet LCD boot UART.sr", "logic"),
        ("Logic captures/DSView protocol demo.dsl", "logic"),
        ("Logic captures/Saleae long UART capture.sal", "logic"),
        ("Logic captures/ChronoVu SPI flash.vcd", "logic"),
        (
            "Logic captures/Synthetic Saleae narrow pulse and gap.bin",
            "logic",
        ),
        (
            "Infrared and Sub-GHz captures/Flipper universal projector.ir",
            "pulses",
        ),
        (
            "Infrared and Sub-GHz captures/Generated pulse timing.sub",
            "pulses",
        ),
        ("RF measurements/LiteVNA 2.4 GHz antenna.s1p", "touchstone"),
        ("RF measurements/Generated three-port.ts", "touchstone"),
        ("Packet captures/Generated four UDP bursts.cap", "packets"),
        (
            "Packet captures/Generated multi-interface capture.pcapng.lz4",
            "packets",
        ),
        ("CAN logs/Vector trace window export.trace", "can"),
        ("CAN logs/PEAK PCAN frames.trc", "can"),
        ("NFC and RFID data/NFC/Flipper NTAG215 sample.nfc", "tags"),
        ("NFC and RFID data/NFC/Generated Classic memory.eml", "tags"),
        (
            "NFC and RFID data/NFC/Proxmark Ultralight lab capture.trace",
            "tags",
        ),
        (
            "NFC and RFID data/RFID/Proxmark Q5 lab waveform.pm3",
            "tags",
        ),
        ("1-Wire data/Generated DS1992 memory.ibtn", "tags"),
        ("Radio programming/IC-R8600 memories.icf", "radio"),
        (
            "Radio programming/Charlottesville CHIRP channels.chirp",
            "radio",
        ),
        (
            "Signal flowgraphs/Noise power (GNU Radio 3.7 XML).grc",
            "flowgraph",
        ),
        (
            "Signal flowgraphs/Noise power (GNU Radio YAML).grc",
            "flowgraph",
        ),
        ("SDR recordings/Synthetic retuned sweep.sigmf-meta", "iq"),
        (
            "SDR recordings/RTL-SDR Burnhard thermometer.sigmf-data",
            "iq",
        ),
    ];
    let renderer = ScreenRenderer::fallback(AppConfig::default());
    for (relative, expected) in cases {
        let path = directory.join(relative);
        let mut item = ClipItem::from_text("placeholder");
        item.payload = ClipPayload::Text(path.to_string_lossy().into_owned());
        item.title = path.file_name().unwrap().to_string_lossy().into_owned();
        let start = Instant::now();
        let preview = renderer
            .file_preview(&item)
            .unwrap_or_else(|| panic!("No engineering preview for {relative}"));
        let elapsed = start.elapsed();
        let kind = match &*preview {
            Preview::Scope(_) => "waveform",
            Preview::Logic(_) => "logic",
            Preview::Pulses(_) => "pulses",
            Preview::Touchstone(_) => "touchstone",
            Preview::Packets(_) => "packets",
            Preview::Can(_) => "can",
            Preview::Tags(_) => "tags",
            Preview::Radio(_) => "radio",
            Preview::Flowgraph(_) => "flowgraph",
            Preview::Iq(_) => "iq",
            _ => "other",
        };
        assert_eq!(kind, expected, "{relative}");
        let mut state = SlotManager::default();
        state.push_left(item);
        let start = Instant::now();
        let (pixmap, _) = renderer.render_pixmap_with_previews(&state).unwrap();
        assert!(pixmap.pixels().iter().any(|pixel| pixel.alpha() > 0));
        println!(
            "{relative}: prepare {elapsed:?}, render {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn eda_cards_render_stage1_pcb_files_and_preserve_gimp_brushes() {
    let dir = tempfile::tempdir().unwrap();
    let gerber_path = dir.path().join("top-F_Cu.gbr");
    std::fs::write(
        &gerber_path,
        "%TF.FileFunction,Copper,L1,Top*%\n%MOMM*%\n%FSLAX46Y46*%\n%ADD10C,0.500000*%\nD10*\nX0Y0D02*\nX10000000Y10000000D01*\nM02*\n",
    )
    .unwrap();
    let drl_path = dir.path().join("holes.drl");
    std::fs::write(
        &drl_path,
        "M48\nMETRIC,TZ\nT1C1.000\n%\nT1\nX10.0Y10.0\nM30\n",
    )
    .unwrap();
    let pos_path = dir.path().join("board.pos");
    std::fs::write(
        &pos_path,
        "Ref,Val,Package,PosX,PosY,Rot,Side\nU1,MCU,QFN-32,12.5,8.0,90.0,top\n",
    )
    .unwrap();
    let plt_path = dir.path().join("outline.plt");
    std::fs::write(&plt_path, "IN;SP1;PU0,0;PD1000,0,1000,1000,0,1000,0,0;").unwrap();

    let mut gimp = Vec::new();
    gimp.extend_from_slice(&34_u32.to_be_bytes());
    gimp.extend_from_slice(&2_u32.to_be_bytes());
    gimp.extend_from_slice(&4_u32.to_be_bytes());
    gimp.extend_from_slice(&4_u32.to_be_bytes());
    gimp.extend_from_slice(&1_u32.to_be_bytes());
    gimp.extend_from_slice(b"GIMP");
    gimp.extend_from_slice(&20_u32.to_be_bytes());
    gimp.extend_from_slice(b"Tip\0\0\0");
    gimp.extend_from_slice(&[240_u8; 16]);
    let brush_path = dir.path().join("stamp.gbr");
    std::fs::write(&brush_path, gimp).unwrap();

    let renderer = ScreenRenderer::fallback(AppConfig::default());
    for eda_path in [&gerber_path, &drl_path, &pos_path, &plt_path] {
        let item = ClipItem::from_single_file(eda_path);
        let preview = renderer
            .file_preview(&item)
            .unwrap_or_else(|| panic!("missing preview for {}", eda_path.display()));
        assert!(matches!(
            preview.as_ref(),
            Preview::Page(page) if page.width == 0 && page.height == 0
        ));
        let mut state = SlotManager::default();
        state.push_left(item);
        let image = renderer.render_pixmap(&state).unwrap();
        assert!(image.pixels().iter().any(|p| p.alpha() > 0));
    }

    let brush_item = ClipItem::from_single_file(&brush_path);
    let brush_preview = renderer.file_preview(&brush_item).expect("gimp brush card");
    assert!(matches!(brush_preview.as_ref(), Preview::Brush(_)));
}

#[test]
fn eda_cards_render_stage2_kicad_files() {
    let dir = tempfile::tempdir().unwrap();
    let pcb = dir.path().join("board.kicad_pcb");
    std::fs::write(
        &pcb,
        "(kicad_pcb (version 20240108) (layers (0 \"F.Cu\" signal) (44 \"Edge.Cuts\" user)) (gr_rect (start 0 0) (end 20 15) (layer \"Edge.Cuts\")) (segment (start 2 2) (end 18 12) (width 0.4) (layer \"F.Cu\")))",
    )
    .unwrap();
    let sch = dir.path().join("sheet.kicad_sch");
    std::fs::write(
        &sch,
        "(kicad_sch (version 20231120) (paper \"A4\") (wire (pts (xy 10 10) (xy 50 10))) (label \"VIN\" (at 20 10 0)))",
    )
    .unwrap();
    let pro = dir.path().join("proj.kicad_pro");
    std::fs::write(
        &pro,
        r#"{"meta":{"filename":"proj.kicad_pro","version":1},"net_settings":{"classes":[{"name":"Default","track_width":0.25}]}}"#,
    )
    .unwrap();
    let fp = dir.path().join("part.kicad_mod");
    std::fs::write(
        &fp,
        "(footprint \"Test:R\" (layer \"F.Cu\") (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\")))",
    )
    .unwrap();
    let sym = dir.path().join("lib.kicad_sym");
    std::fs::write(
        &sym,
        "(kicad_symbol_lib (version 20231120) (symbol \"R\" (symbol \"R_0_1\" (rectangle (start -1 -2) (end 1 2)))))",
    )
    .unwrap();
    let gbrjob = dir.path().join("fab.gbrjob");
    std::fs::write(
        &gbrjob,
        r#"{"GeneralSpecs":{"Size":{"X":50,"Y":30},"LayerNumber":2},"MaterialStackup":[{"Type":"Copper","Name":"F.Cu","Thickness":0.035},{"Type":"Dielectric","Name":"Core","Thickness":1.5}]}"#,
    )
    .unwrap();

    let renderer = ScreenRenderer::fallback(AppConfig::default());
    for path in [&pcb, &sch, &pro, &fp, &sym, &gbrjob] {
        let item = ClipItem::from_single_file(path);
        let preview = renderer
            .file_preview(&item)
            .unwrap_or_else(|| panic!("missing card preview for {}", path.display()));
        assert!(matches!(
            preview.as_ref(),
            Preview::Page(page) if page.width == 0 && page.height == 0
        ));
        let mut state = SlotManager::default();
        state.push_left(item);
        let image = renderer.render_pixmap(&state).unwrap();
        assert!(image.pixels().iter().any(|p| p.alpha() > 0));
    }
}
