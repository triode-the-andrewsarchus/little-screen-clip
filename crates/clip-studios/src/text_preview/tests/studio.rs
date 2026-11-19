use super::*;

#[test]
fn find_shortcut_yields_to_focused_text_control() {
    let ctx = egui::Context::default();
    let mut text = String::new();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let response = ui.add(egui::TextEdit::singleline(&mut text));
            response.request_focus();
        });
    });
    assert!(ctx.wants_keyboard_input());

    let mut state =
        TextPreviewState::from_text("editable content".to_owned(), None, &AppConfig::default());
    let mut close = false;
    let modifiers = egui::Modifiers::COMMAND;
    let _ = ctx.run(
        egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::F,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        },
        |ctx| state.handle_shortcuts(ctx, &mut close),
    );
    assert!(!state.show_find);
    assert!(!close);
}

#[test]
fn find_shortcut_focuses_search_and_escape_dismisses_only_find() {
    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    let mut state =
        TextPreviewState::from_text("searchable content".to_owned(), None, &AppConfig::default());
    let mut close = false;
    let modifiers = egui::Modifiers::COMMAND;
    let _ = ctx.run(
        egui::RawInput {
            modifiers,
            events: vec![egui::Event::Key {
                key: egui::Key::F,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        },
        |ctx| {
            state.handle_shortcuts(ctx, &mut close);
            if state.show_find {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_search_toolbar(ui, &palette);
                });
            }
        },
    );
    assert!(state.show_find);
    assert!(ctx.wants_keyboard_input());

    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| state.handle_shortcuts(ctx, &mut close),
    );
    assert!(!state.show_find);
    assert!(!close);
}

#[test]
fn syntax_and_markdown_custom_labels_meet_contrast_in_both_appearances() {
    let syntax_colors = [
        [255, 0, 0],
        [255, 128, 0],
        [255, 255, 0],
        [0, 255, 0],
        [0, 255, 255],
        [0, 0, 255],
        [255, 0, 255],
        [80, 80, 80],
        [220, 220, 220],
    ];
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = AppPalette::for_appearance(appearance);
        let recessed = hex_to_c32(&palette.colors.surface_recessed);
        for [r, g, b] in syntax_colors {
            let foreground = map_syntect_color_to_theme(
                syntect::highlighting::Color { r, g, b, a: 255 },
                &palette,
            );
            assert!(
                contrast(recessed, foreground) >= 4.5,
                "{appearance:?} syntax hue [{r}, {g}, {b}] lacks contrast"
            );
        }

        let panel = hex_to_c32(&palette.colors.surface_panel);
        for preferred in [
            hex_to_c32(&palette.harmonies.triadic_2),
            hex_to_c32(&palette.harmonies.base),
        ] {
            let foreground =
                crate::ui_ext::contrast_text_c32_on(panel, panel, Some(preferred), &palette);
            assert!(
                contrast(panel, foreground) >= 4.5,
                "{appearance:?} Markdown code/link label lacks contrast"
            );
        }

        for preferred in [
            hex_to_c32(&palette.diff_del),
            hex_to_c32(&palette.harmonies.complementary),
        ] {
            let foreground =
                crate::ui_ext::contrast_text_c32_on(recessed, recessed, Some(preferred), &palette);
            assert!(
                contrast(recessed, foreground) >= 4.5,
                "{appearance:?} text-search status label lacks contrast"
            );
        }
    }
}

#[test]
fn text_search_toolbar_wraps_status_and_controls_at_compact_widths() {
    for width in [390.0, 760.0, 1280.0] {
        for (query, regex_error, matches, expected_status) in [
            (
                "(".to_string(),
                Some("unclosed group".to_string()),
                Vec::new(),
                "Regex error:",
            ),
            ("missing".to_string(), None, Vec::new(), "No matches"),
            (
                "Alpha".to_string(),
                None,
                vec![SearchMatch {
                    start_byte: 0,
                    end_byte: 5,
                    line: 0,
                }],
                "1/1 matches",
            ),
        ] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette = AppPalette::default();
            let mut state =
                TextPreviewState::from_text("Alpha beta".to_owned(), None, &AppConfig::default());
            state.find_query = query;
            state.regex_error = regex_error;
            if !matches.is_empty() {
                let mut spool = SearchMatchSpool::new().unwrap();
                for matched in matches {
                    spool
                        .append(matched.start_byte as u64, matched.end_byte as u64)
                        .unwrap();
                }
                state.match_count = spool.len();
                state.matches = Some(spool);
            }
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 300.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_search_toolbar(ui, &palette);
                    });
                },
            );

            for required in [expected_status, "Prev", "Next", "Aa", ".*"] {
                let shape = output.shapes.iter().find_map(|clipped| {
                    let egui::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    text.galley
                        .job
                        .text
                        .contains(required)
                        .then_some((clipped.clip_rect, text))
                });
                let Some((clip_rect, text)) = shape else {
                    panic!("{required:?} missing at width {width}");
                };
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                assert!(!text.galley.elided, "{required:?} elided at width {width}");
                assert!(
                    bounds.left() >= clip_rect.left() - 0.5
                        && bounds.right() <= clip_rect.right() + 0.5,
                    "{required:?} exceeds {width}-point search toolbar: {bounds:?} clipped to {clip_rect:?}"
                );
            }
        }
    }
}

#[test]
fn text_viewer_chrome_stays_inside_compact_and_wide_hosts() {
    for width in [760.0, 1280.0, 1760.0] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let mut state = TextPreviewState::from_text(
            "fn alpha() {}\nfn bravo() {}\nfn charlie() {}".to_owned(),
            Some("Example.rs".to_owned()),
            &AppConfig::default(),
        );
        state.language = "rs";
        state.show_line_numbers = true;
        state.wrap_lines = true;
        let mut close = false;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 720.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );

        for required in [
            "Copy All",
            "Find (Ctrl+F)",
            "Wrap: ON",
            "Lines: ON",
            "3 lines",
            "Ctrl+F: Find",
        ] {
            let shape = output.shapes.iter().find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley
                    .job
                    .text
                    .contains(required)
                    .then_some((clipped.clip_rect, text))
            });
            let Some((clip_rect, text)) = shape else {
                panic!("text viewer chrome omitted {required:?} at {width} points");
            };
            let bounds = text.galley.rect.translate(text.pos.to_vec2());
            assert!(!text.galley.elided, "{required:?} elided at {width} points");
            assert!(
                bounds.left() >= clip_rect.left() - 0.5
                    && bounds.right() <= clip_rect.right() + 0.5,
                "{required:?} exceeds text viewer at {width} points: {bounds:?} clipped to {clip_rect:?}"
            );
        }
    }
}

#[test]
fn rendered_copy_all_button_copies_full_text_preview_source() {
    let content = format!(
        "First line\n日本語 and emoji 🧪\n{}",
        "full source row\n".repeat(64)
    );
    let mut state = TextPreviewState::from_text(
        content.clone(),
        Some("Copy fixture.txt".to_owned()),
        &AppConfig::default(),
    );
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut close = false;
    let mut time = 0.0;
    let mut render = |state: &mut TextPreviewState, time: &mut f64, events: Vec<egui::Event>| {
        *time += 0.1;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                time: Some(*time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        )
    };

    let frame = render(&mut state, &mut time, Vec::new());
    let copy_position = frame
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().contains("Copy All") => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("rendered Copy All button label");
    render(
        &mut state,
        &mut time,
        vec![egui::Event::PointerMoved(copy_position)],
    );
    render(
        &mut state,
        &mut time,
        vec![
            egui::Event::PointerMoved(copy_position),
            egui::Event::PointerButton {
                pos: copy_position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let copied = render(
        &mut state,
        &mut time,
        vec![
            egui::Event::PointerMoved(copy_position),
            egui::Event::PointerButton {
                pos: copy_position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );

    assert_eq!(copied.platform_output.copied_text, content);
    assert!(!close);
}

#[test]
fn markdown_body_list_and_quote_wrap_without_clipping_at_compact_and_wide_widths() {
    let markers = ["BODY_MARKER", "LIST_MARKER", "QUOTE_MARKER"];
    let content = format!(
        "BODY_MARKER {}\n\n- LIST_MARKER {}\n\n> QUOTE_MARKER {}",
        "paragraph text that should wrap naturally without being elided or clipped ".repeat(5),
        "list text that should wrap naturally without being elided or clipped ".repeat(5),
        "quoted text that should wrap naturally without being elided or clipped ".repeat(5),
    );

    for (label, width) in [("compact", 390.0), ("wide", 1400.0)] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let mut state = TextPreviewState::from_text(content.clone(), None, &AppConfig::default());
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 1800.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_markdown_view(ui, &palette);
                });
            },
        );

        for marker in markers {
            let matching: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text().contains(marker) => {
                        Some((clipped.clip_rect, text))
                    }
                    _ => None,
                })
                .collect();
            assert!(
                !matching.is_empty(),
                "{label} Markdown render omitted {marker}"
            );

            let mut rendered_rows = 0;
            for (clip_rect, text) in matching {
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                assert!(
                    bounds.left() >= clip_rect.left() - 0.5
                        && bounds.right() <= clip_rect.right() + 0.5,
                    "{label} {marker} exceeds horizontal clip bounds: {bounds:?} vs {clip_rect:?}"
                );
                assert!(
                    bounds.top() >= clip_rect.top() - 0.5
                        && bounds.bottom() <= clip_rect.bottom() + 0.5,
                    "{label} {marker} exceeds vertical clip bounds: {bounds:?} vs {clip_rect:?}"
                );
                assert!(!text.galley.elided, "{label} {marker} was elided");
                assert!(
                    text.galley.job.sections.iter().all(|section| {
                        (section.format.font_id.size - BODY_FONT_SIZE).abs() < 0.1
                    }),
                    "{label} {marker} did not use the shared body font size"
                );
                rendered_rows += text.galley.rows.len();
            }
            if label == "compact" {
                assert!(
                    rendered_rows > 1,
                    "compact {marker} did not wrap: {rendered_rows} rendered rows"
                );
            }
        }
    }
}

#[test]
fn test_markdown_preview_mode() {
    let config = AppConfig::default();
    let content = "# Title\n\n- item 1\n- item 2\n\n```rust\nfn test() {}\n```".to_string();
    let state = TextPreviewState::from_text(content, None, &config);

    assert!(state.is_markdown);
    assert_eq!(state.view_mode, TextViewMode::RenderedMarkdown);
}

#[test]
fn test_c_code_not_markdown() {
    let config = AppConfig::default();
    let content = "#include <stdio.h>\n#include <math.h>\n\nint main(void) {\n    printf(\"pi\");\n    return 0;\n}".to_string();
    let state = TextPreviewState::from_text(content, None, &config);

    assert_eq!(state.language, "c");
    assert!(!state.is_markdown);
    assert_eq!(state.view_mode, TextViewMode::Raw);
    assert!(state.show_line_numbers);
}

#[test]
fn test_python_code_not_markdown() {
    let config = AppConfig::default();
    let content =
        "# A python script\nimport math\n\ndef calculate(n):\n    return math.pi * n".to_string();
    let state = TextPreviewState::from_text(content, None, &config);

    assert_eq!(state.language, "py");
    assert!(!state.is_markdown);
    assert_eq!(state.view_mode, TextViewMode::Raw);
    assert!(state.show_line_numbers);
}

#[test]
fn markdown_reading_column_reflows_inline_spans_lists_and_tables_cleanly() {
    let content = format!(
        concat!(
            "# Heading Title\n\n",
            "WIDE_PARAGRAPH soft wrapped line one\n",
            "continues on line two with **bold phrase**, *italic text*, `inline_code()`, ",
            "and a [helpful link](https://example.com) followed by trailing words {}\n\n",
            "1. FIRST_NUMBERED {}\n\n",
            "| Column A | Column B | Amount |\n",
            "| :--- | :---: | ---: |\n",
            "| TABLE_CELL_A | middle value | 42 |\n",
            "| second row | wrapping description text {} | 128 |\n",
        ),
        "more prose ".repeat(14),
        "list continuation words ".repeat(10),
        "cell details ".repeat(8),
    );

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut state = TextPreviewState::from_data(
        TextPreviewData::from_source(Arc::new(content), Some("md")),
        Some("Guide.md".to_owned()),
        &AppConfig::default(),
    );
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1800.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_markdown_view(ui, &palette);
            });
        },
    );

    let para_shape = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().contains("WIDE_PARAGRAPH") => Some(text),
            _ => None,
        })
        .expect("reflowed paragraph should render");
    let para_bounds = para_shape.galley.rect.translate(para_shape.pos.to_vec2());
    assert!(
        para_bounds.width() <= 761.0,
        "wide paragraph should constrain to reading column, got width {}",
        para_bounds.width()
    );
    assert!(
        para_bounds.left() > 200.0,
        "reading column should be centered on wide viewports, left={}",
        para_bounds.left()
    );
    assert!(
        para_shape.galley.text().contains("continues on line two"),
        "soft-wrapped lines must reflow into a single paragraph galley"
    );
    assert!(
        para_shape.galley.rows.len() > 1,
        "wide paragraph should wrap cleanly across multiple lines"
    );

    assert!(
        !output.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Text(text) if text.galley.text().contains("1..")
        )),
        "numbered list markers must not duplicate periods"
    );
    assert!(
        output.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Text(text) if text.galley.text() == "1."
        )),
        "numbered list should render single-dot 1. marker"
    );

    let table_cell = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().contains("TABLE_CELL_A") => {
                Some((clipped.clip_rect, text))
            }
            _ => None,
        })
        .expect("GFM table cell should render");
    let cell_bounds = table_cell
        .1
        .galley
        .rect
        .translate(table_cell.1.pos.to_vec2());
    assert!(cell_bounds.right() <= table_cell.0.right() + 0.5);
    assert!(!table_cell.1.galley.elided);
    assert!(
        !output.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Text(text) if text.galley.text().contains("| :--- |")
        )),
        "GFM table delimiter row must not render as raw pipe text"
    );
}

#[test]
fn markdown_wide_table_uses_horizontal_scroll_and_hides_raw_wrap_toggle() {
    let content = concat!(
        "# Matrix\n\n",
        "- [x] TASK_DONE_ITEM\n",
        "  - NESTED_BULLET\n\n",
        "> [!NOTE]\n",
        "> QUOTE_L1\n",
        "> QUOTE_L2\n\n",
        "| Col1 | Col2 | Col3 | Col4 | Col5 | Col6 |\n",
        "| --- | --- | --- | --- | --- | --- |\n",
        "| WIDE_COL_1 | alpha | beta | gamma | delta | WIDE_COL_6 |\n",
    );

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut state = TextPreviewState::from_data(
        TextPreviewData::from_source(Arc::new(content.to_owned()), Some("md")),
        Some("Matrix.md".to_owned()),
        &AppConfig::default(),
    );
    let mut close = false;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(390.0, 900.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );

    assert!(
        !output.shapes.iter().any(|clipped| matches!(
            &clipped.shape,
            egui::Shape::Text(text) if text.galley.text().contains("Wrap: ON")
        )),
        "RenderedMarkdown header should hide Raw-only Wrap toggle"
    );
    assert!(output.shapes.iter().any(|clipped| matches!(
        &clipped.shape,
        egui::Shape::Text(text) if text.galley.text().contains("TASK_DONE_ITEM")
    )));
    assert!(output.shapes.iter().any(|clipped| matches!(
        &clipped.shape,
        egui::Shape::Text(text) if text.galley.text().contains("QUOTE_L1 QUOTE_L2")
    )));
    let col1 = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().contains("WIDE_COL_1") => Some(text),
            _ => None,
        })
        .expect("first column of wide table should render");
    assert!(
        col1.galley.rect.width() >= 40.0,
        "wide table column must remain readable instead of collapsing: {:?}",
        col1.galley.rect
    );
}

#[test]
fn markdown_headings_preserve_clear_size_and_format_hierarchy() {
    let content = concat!(
        "# H1 Title\n\n",
        "## H2 Section\n\n",
        "### H3 Subsection\n\n",
        "#### H4 Topic\n\n",
        "##### H5 Detail\n\n",
        "###### H6 Note\n\n",
        "BODY_PROSE normal text.\n",
    );

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut state = TextPreviewState::from_data(
        TextPreviewData::from_source(Arc::new(content.to_owned()), Some("md")),
        Some("Headings.md".to_owned()),
        &AppConfig::default(),
    );
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 1400.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_markdown_view(ui, &palette);
            });
        },
    );

    let find_text = |needle: &str| {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().contains(needle) => Some(text),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing text shape for {needle}"))
    };

    let size_of = |needle: &str| find_text(needle).galley.job.sections[0].format.font_id.size;
    let sizes = [
        size_of("H1 Title"),
        size_of("H2 Section"),
        size_of("H3 Subsection"),
        size_of("H4 Topic"),
        size_of("H5 Detail"),
        size_of("H6 Note"),
    ];
    for pair in sizes.windows(2) {
        assert!(
            pair[0] > pair[1],
            "heading scale must decrease strictly: {sizes:?}"
        );
    }
    assert!(
        sizes[0] >= 32.0,
        "H1 must have display scale, got {}",
        sizes[0]
    );
    assert!(
        sizes[4] > BODY_FONT_SIZE,
        "H1..H5 must remain visibly larger than body prose ({BODY_FONT_SIZE}), got {:?}",
        sizes[4]
    );
    assert!(
        find_text("H6 Note").galley.job.sections[0].format.italics,
        "H6 should apply italic distinction when near body size"
    );

    let h1 = find_text("H1 Title");
    let h1_bottom = h1.pos.y + h1.galley.rect.height();
    let has_h1_rule = output.shapes.iter().any(|clipped| match &clipped.shape {
        egui::Shape::LineSegment { points, stroke } => {
            stroke.width >= 1.8 && points[0].y >= h1_bottom && (points[0].y - h1_bottom) < 24.0
        }
        _ => false,
    });
    assert!(
        has_h1_rule,
        "H1 should render a distinct bottom underline rule"
    );
}

#[test]
fn markdown_renders_inline_html_css_details_and_hugo_static_images() {
    let dir = tempfile::tempdir().expect("create temp hugo site");
    let static_img_dir = dir.path().join("static").join("nonfree");
    let doc_dir = dir.path().join("content").join("Engineering");
    std::fs::create_dir_all(&static_img_dir).expect("create static dir");
    std::fs::create_dir_all(&doc_dir).expect("create content dir");

    let img_path = static_img_dir.join("hero.png");
    let img = image::RgbaImage::from_pixel(80, 40, image::Rgba([48, 128, 224, 255]));
    img.save(&img_path).expect("save hugo static PNG");

    let md_path = doc_dir.join("guide.md");
    let markdown = "\
<!-- hidden draft note -->
<div hidden>secret div</div>
<script>window.nope = 1;</script>

<div style='color: red'>

* Crimson bullet

</div>

<details><summary>Hardware notes</summary>

Inside collapsible body

</details>

<table><thead><tr><th>Layer</th><th>Status</th></tr></thead><tbody><tr><td>HTML Table</td><td>Working</td></tr></tbody></table>

<img src=\"/nonfree/hero.png\" alt=\"Hero diagram\" style=\"float:right; width:50%; border-radius:12px;\">

Text flowing beside floated image.
";
    std::fs::write(&md_path, markdown).expect("write markdown fixture");

    let mut state =
        TextPreviewState::from_file(&md_path, &AppConfig::default()).expect("load markdown file");
    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());

    let run_frame = |state: &mut TextPreviewState, events: Vec<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_markdown_view(ui, &palette);
                });
            },
        )
    };

    let has_painted_image = |output: &egui::FullOutput| {
        output.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => rect.fill_texture_id != egui::TextureId::default(),
            egui::Shape::Mesh(mesh) => mesh.texture_id != egui::TextureId::default(),
            _ => false,
        })
    };

    let mut output = run_frame(&mut state, Vec::new());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !has_painted_image(&output) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(15));
        output = run_frame(&mut state, Vec::new());
    }
    assert!(
        has_painted_image(&output),
        "expected /nonfree/hero.png to resolve from Hugo static/ and paint a GPU texture"
    );

    let all_text: String = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!all_text.contains("hidden draft note"));
    assert!(!all_text.contains("secret div"));
    assert!(!all_text.contains("window.nope"));
    assert!(all_text.contains("Crimson bullet"));
    assert!(all_text.contains("HTML Table"));
    assert!(all_text.contains("Working"));
    assert!(all_text.contains("Hardware notes"));
    assert!(
        !all_text.contains("Inside collapsible body"),
        "collapsed <details> must hide its body initially"
    );

    let crimson_shape = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().contains("Crimson bullet") => Some(text),
            _ => None,
        })
        .expect("Crimson bullet text shape");
    let crimson_color = crimson_shape.galley.job.sections[0].format.color;
    assert!(
        crimson_color.r() > crimson_color.b(),
        "expected inherited reddish color from <div style='color: red'>, got {crimson_color:?}"
    );

    let summary_center = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text().contains("Hardware notes") => {
                Some(text.pos + text.galley.rect.center().to_vec2())
            }
            _ => None,
        })
        .expect("Hardware notes summary shape");

    let click_events = vec![
        egui::Event::PointerMoved(summary_center),
        egui::Event::PointerButton {
            pos: summary_center,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos: summary_center,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let _ = run_frame(&mut state, click_events);
    let expanded_output = run_frame(&mut state, Vec::new());
    let expanded_text: String = expanded_output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        expanded_text.contains("Inside collapsible body"),
        "clicking <summary> should expand <details> body; saw: {expanded_text:?}"
    );
}

#[test]
fn markdown_renders_hugo_shortcodes_code_mermaid_math_details_and_hl() {
    let markdown = "\
Use {{< hl rust >}}let val = 42;{{< /hl >}} directly.

Mix {{< hl python >}}x = 1{{< /hl >}} with inline {{< katex >}}\\frac{1}{2}{{< /katex >}}.

{{< latex display >}}

E = mc^2

{{< /latex >}}

{{< mermaid >}}
graph TD
    A --> B
{{< /mermaid >}}

{{< details \"Expand snippet\" >}}

{{< code lang=\"python\" lines=\"20\" compact=\"true\" >}}
def hi():
    return 1
{{< /code >}}

{{< /details >}}
";

    let mut state = TextPreviewState::from_text(
        markdown.to_owned(),
        Some("md".to_owned()),
        &AppConfig::default(),
    );
    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());

    let run_frame = |state: &mut TextPreviewState, events: Vec<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 960.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_markdown_view(ui, &palette);
                });
            },
        )
    };

    let output = run_frame(&mut state, Vec::new());
    let collect_text = |out: &egui::FullOutput| {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let text = collect_text(&output);
    assert!(
        text.contains("let val = 42;"),
        "missing inline hl text: {text:?}"
    );
    assert!(
        text.contains("Diagram source"),
        "mermaid shortcode should render diagram block with Diagram source toggle: {text:?}"
    );
    assert!(text.contains("Expand snippet"));
    assert!(
        !text.contains("def hi():"),
        "collapsed {{< details >}} must hide nested {{< code >}} initially"
    );

    // Verify inline {{< hl rust >}} emits distinct syntax-colored monospace sections.
    let hl_shape = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text().contains("let val = 42;") => {
                Some(shape)
            }
            _ => None,
        })
        .expect("inline hl text shape");
    let mono_colors: std::collections::BTreeSet<[u8; 4]> = hl_shape
        .galley
        .job
        .sections
        .iter()
        .filter(|sec| sec.format.font_id.family == egui::FontFamily::Monospace)
        .map(|sec| sec.format.color.to_array())
        .collect();
    assert!(
        mono_colors.len() >= 2,
        "expected distinct syntax highlighting colors for {{< hl rust >}}, got {mono_colors:?}"
    );

    // Click {{< details >}} summary to expand and reveal nested {{< code >}}.
    let summary_center = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text().contains("Expand snippet") => {
                Some(shape.pos + shape.galley.rect.center().to_vec2())
            }
            _ => None,
        })
        .expect("Expand snippet shape");
    let _ = run_frame(
        &mut state,
        vec![
            egui::Event::PointerMoved(summary_center),
            egui::Event::PointerButton {
                pos: summary_center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: summary_center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let expanded = run_frame(&mut state, Vec::new());
    let expanded_text = collect_text(&expanded);
    assert!(
        expanded_text.contains("def hi():") && expanded_text.contains("PYTHON"),
        "expanding {{< details >}} should reveal nested {{< code lang=\"python\" >}}: {expanded_text:?}"
    );
}

#[test]
fn markdown_renders_opguides_shortcodes_columns_tabs_quotes_hints_and_spoilers() {
    let markdown = "\
Check {{< spoiler >}}secret_word{{< /spoiler >}} and {{< smalltext >}}tiny_aside{{< /smalltext >}}.

{{< attribution >}}Credit to [Artist](https://example.com){{< /attribution >}}

{{< hint info >}}

Helpful info note.

{{< /hint >}}

{{< quote \"Speaker\" >}}

Block quote content.

{{< /quote >}}

{{< bigquote \"Grand Source\" >}}

Display quote statement.

{{< /bigquote >}}

{{< tabs \"OS\" >}}

{{< tab \"Linux\" >}}
Linux pane body
{{< /tab >}}

{{< tab \"Windows\" >}}
Windows pane body
{{< /tab >}}

{{< /tabs >}}

{{< columns3 >}}

Left column item

<--->

Middle column item

<--->

Right column item

{{< /columns3 >}}

{{< columns2 >}}

First two-col

<--->

Second two-col

{{< /columns2 >}}
";

    let mut state = TextPreviewState::from_text(
        markdown.to_owned(),
        Some("md".to_owned()),
        &AppConfig::default(),
    );
    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());

    let run_at = |state: &mut TextPreviewState, width: f32, events: Vec<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 1600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_markdown_view(ui, &palette);
                });
            },
        )
    };

    let output = run_at(&mut state, 1024.0, Vec::new());

    fn find_text<'a>(
        out: &'a egui::FullOutput,
        needle: &str,
    ) -> Option<&'a egui::epaint::TextShape> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text().contains(needle) => Some(shape),
            _ => None,
        })
    }

    // 1. Spoiler is masked initially (foreground == background), and clicking reveals it.
    let spoiler_line = find_text(&output, "secret_word").expect("spoiler paragraph shape");
    let secret_sec = spoiler_line
        .galley
        .job
        .sections
        .iter()
        .find(|sec| spoiler_line.galley.text()[sec.byte_range.clone()].contains("secret_word"))
        .expect("secret_word section");
    assert_eq!(
        secret_sec.format.color, secret_sec.format.background,
        "unrevealed spoiler must mask foreground onto redaction fill"
    );

    // 2. Smalltext is smaller than BODY_FONT_SIZE.
    let small_sec = spoiler_line
        .galley
        .job
        .sections
        .iter()
        .find(|sec| spoiler_line.galley.text()[sec.byte_range.clone()].contains("tiny_aside"))
        .expect("tiny_aside section");
    assert!(
        small_sec.format.font_id.size < BODY_FONT_SIZE,
        "smalltext should render smaller than body size, got {}",
        small_sec.format.font_id.size
    );

    // 3. Spoiler click reveals foreground text cleanly.
    let secret_glyph_pos = {
        let char_idx = spoiler_line.galley.text().find("secret_word").unwrap();
        let mut ccursor = egui::text::CCursor::new(char_idx + 3);
        ccursor.prefer_next_row = true;
        let cursor = spoiler_line.galley.from_ccursor(ccursor);
        spoiler_line.pos
            + spoiler_line
                .galley
                .pos_from_cursor(&cursor)
                .center()
                .to_vec2()
            + egui::vec2(2.0, 0.0)
    };
    let _ = run_at(
        &mut state,
        1024.0,
        vec![
            egui::Event::PointerMoved(secret_glyph_pos),
            egui::Event::PointerButton {
                pos: secret_glyph_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: secret_glyph_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let after_spoiler_click = run_at(&mut state, 1024.0, Vec::new());
    let revealed_line = find_text(&after_spoiler_click, "secret_word").unwrap();
    let revealed_sec = revealed_line
        .galley
        .job
        .sections
        .iter()
        .find(|sec| revealed_line.galley.text()[sec.byte_range.clone()].contains("secret_word"))
        .unwrap();
    assert_ne!(
        revealed_sec.format.color, revealed_sec.format.background,
        "clicked spoiler must reveal distinct readable foreground"
    );

    // 4. Hint, Quote, BigQuote, and Attribution render with expected typography.
    assert!(find_text(&output, "Note").is_some());
    assert!(find_text(&output, "Helpful info note.").is_some());
    assert!(find_text(&output, "Block quote content.").is_some());
    assert!(find_text(&output, "Speaker").is_some());
    let big_shape = find_text(&output, "Display quote statement.").expect("bigquote shape");
    assert!(
        big_shape.galley.job.sections[0].format.font_id.size >= 19.0
            && big_shape.galley.job.sections[0].format.italics,
        "bigquote should render larger italic text"
    );
    assert!(find_text(&output, "Artist").is_some());

    // 5. Tabs switch interactively from Linux to Windows.
    assert!(find_text(&output, "Linux pane body").is_some());
    assert!(find_text(&output, "Windows pane body").is_none());
    let win_tab = find_text(&output, "Windows").expect("Windows tab button shape");
    let win_pos = win_tab.pos + win_tab.galley.rect.center().to_vec2();
    let _ = run_at(
        &mut state,
        1024.0,
        vec![
            egui::Event::PointerMoved(win_pos),
            egui::Event::PointerButton {
                pos: win_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: win_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let after_tab_click = run_at(&mut state, 1024.0, Vec::new());
    assert!(
        find_text(&after_tab_click, "Windows pane body").is_some()
            && find_text(&after_tab_click, "Linux pane body").is_none(),
        "clicking Windows tab should switch visible tab body"
    );

    // 6. Columns3 renders side-by-side at wide width and stacks vertically at narrow width.
    let c1 = find_text(&output, "Left column item").expect("col1");
    let c2 = find_text(&output, "Middle column item").expect("col2");
    let c3 = find_text(&output, "Right column item").expect("col3");
    assert!(
        c1.pos.x < c2.pos.x && c2.pos.x < c3.pos.x && (c1.pos.y - c3.pos.y).abs() < 4.0,
        "columns3 must lay out side-by-side at 1024px: c1={:?}, c2={:?}, c3={:?}",
        c1.pos,
        c2.pos,
        c3.pos
    );

    let narrow_output = run_at(&mut state, 380.0, Vec::new());
    let nc1 = find_text(&narrow_output, "Left column item").expect("narrow col1");
    let nc2 = find_text(&narrow_output, "Middle column item").expect("narrow col2");
    assert!(
        nc2.pos.y > nc1.pos.y,
        "columns must wrap vertically at narrow width (380px): nc1={:?}, nc2={:?}",
        nc1.pos,
        nc2.pos
    );
}

#[test]
fn internal_hugo_links_paint_dashed_underlines_and_request_navigation_on_click() {
    let dir = tempfile::tempdir().unwrap();
    let content = dir.path().join("content");
    let source = content.join("webdev.md");
    let eng_page = content.join("Engineering/Engineering.md");
    let interview = content.join("Interviews/soatok.md");
    let toki = content.join("Other/Conlangs/tokipona.md");
    for p in [&source, &eng_page, &interview, &toki] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "# Target\n").unwrap();
    }

    let md = "\
Visit [Engineering Guide](/engineering/engineering) or [External Docs](https://example.com) or [Broken](/missing/page).

See [Soatok]({{< relref \"Interviews/soatok\" >}}) and {{< button relref=\"Other/conlangs/tokipona\" >}}toki pona button{{< /button >}} with math $x$.
";
    std::fs::write(&source, md).unwrap();

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut state = TextPreviewState::from_file(&source, &AppConfig::default()).unwrap();

    let run_frame = |state: &mut TextPreviewState, events: Vec<egui::Event>| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1024.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_markdown_view(ui, &palette);
                });
            },
        )
    };

    let output = run_frame(&mut state, Vec::new());

    let intro_shape = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text().contains("Engineering Guide") => {
                Some(shape)
            }
            _ => None,
        })
        .expect("intro paragraph shape");

    let eng_sec = intro_shape
        .galley
        .job
        .sections
        .iter()
        .find(|sec| intro_shape.galley.text()[sec.byte_range.clone()].contains("Engineering Guide"))
        .expect("internal link section");
    assert_eq!(
        eng_sec.format.underline,
        egui::Stroke::NONE,
        "internal Hugo link should replace solid underline with custom dashed underline"
    );

    let ext_sec = intro_shape
        .galley
        .job
        .sections
        .iter()
        .find(|sec| intro_shape.galley.text()[sec.byte_range.clone()].contains("External Docs"))
        .expect("external link section");
    assert_ne!(
        ext_sec.format.underline,
        egui::Stroke::NONE,
        "external link must retain normal solid underline"
    );

    let count_dashes_under = |text_shape: &egui::epaint::TextShape, needle: &str| {
        let byte_start = text_shape.galley.text().find(needle).unwrap();
        let start_char = text_shape.galley.text()[..byte_start].chars().count();
        let end_char = start_char + needle.chars().count();
        let r0 = text_shape.galley.pos_from_cursor(
            &text_shape
                .galley
                .from_ccursor(egui::text::CCursor::new(start_char)),
        );
        let r1 = text_shape.galley.pos_from_cursor(
            &text_shape
                .galley
                .from_ccursor(egui::text::CCursor::new(end_char)),
        );
        let min_x = text_shape.pos.x + r0.left() - 2.0;
        let max_x = text_shape.pos.x + r1.right() + 2.0;
        let min_y = text_shape.pos.y + r0.top();
        let max_y = text_shape.pos.y + r0.bottom() + 4.0;
        output
            .shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { points, .. } => {
                    (points[0].y - points[1].y).abs() < 0.5
                        && points[0].y >= min_y
                        && points[0].y <= max_y
                        && points[0].x >= min_x
                        && points[1].x <= max_x
                }
                _ => false,
            })
            .count()
    };

    assert!(
        count_dashes_under(intro_shape, "Engineering Guide") >= 2,
        "internal Hugo link should draw multiple dashed underline segments"
    );
    assert_eq!(
        count_dashes_under(intro_shape, "External Docs"),
        0,
        "external link should not draw dashed segments"
    );

    // Clicking the resolved internal link sets pending_navigation cleanly.
    let eng_click_pos = {
        let byte_start = intro_shape.galley.text().find("Engineering Guide").unwrap();
        let char_idx = intro_shape.galley.text()[..byte_start].chars().count() + 2;
        let cursor = intro_shape
            .galley
            .from_ccursor(egui::text::CCursor::new(char_idx));
        intro_shape.pos
            + intro_shape
                .galley
                .pos_from_cursor(&cursor)
                .center()
                .to_vec2()
    };
    let _ = run_frame(
        &mut state,
        vec![
            egui::Event::PointerMoved(eng_click_pos),
            egui::Event::PointerButton {
                pos: eng_click_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: eng_click_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(state.take_pending_navigation(), Some(eng_page));

    // Math paragraph: verify relref and button links render with dashed segments and navigate on click.
    let toki_word = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(shape) if shape.galley.text().contains("toki") => Some(shape),
            _ => None,
        })
        .expect("toki button atom shape");
    assert_eq!(
        toki_word.galley.job.sections[0].format.underline,
        egui::Stroke::NONE
    );
    let toki_click = toki_word.pos + toki_word.galley.rect.center().to_vec2();
    let _ = run_frame(
        &mut state,
        vec![
            egui::Event::PointerMoved(toki_click),
            egui::Event::PointerButton {
                pos: toki_click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: toki_click,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(state.take_pending_navigation(), Some(toki));

    // Clicking an external link does not set pending_navigation.
    let ext_click_pos = {
        let byte_start = intro_shape.galley.text().find("External Docs").unwrap();
        let char_idx = intro_shape.galley.text()[..byte_start].chars().count() + 2;
        let cursor = intro_shape
            .galley
            .from_ccursor(egui::text::CCursor::new(char_idx));
        intro_shape.pos
            + intro_shape
                .galley
                .pos_from_cursor(&cursor)
                .center()
                .to_vec2()
    };
    let _ = run_frame(
        &mut state,
        vec![
            egui::Event::PointerMoved(ext_click_pos),
            egui::Event::PointerButton {
                pos: ext_click_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: ext_click_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    assert_eq!(state.take_pending_navigation(), None);
}
