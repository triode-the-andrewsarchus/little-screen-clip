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
