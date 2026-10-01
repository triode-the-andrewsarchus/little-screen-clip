use super::*;

#[test]
fn test_byte_subtle_color_endpoints_and_interpolation() {
    // 0x00: slightly blue
    let c_00 = byte_subtle_color(0x00);
    assert_eq!(c_00, Color32::from_rgb(140, 175, 230));

    // 0x7F: slightly green
    let c_7f = byte_subtle_color(0x7F);
    assert_eq!(c_7f, Color32::from_rgb(140, 220, 165));

    // 0xFF: slightly red
    let c_ff = byte_subtle_color(0xFF);
    assert_eq!(c_ff, Color32::from_rgb(230, 150, 150));
}

#[test]
fn binary_cell_text_meets_contrast_for_all_byte_values_and_modes() {
    fn contrast(a: Color32, b: Color32) -> f32 {
        little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
    }

    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = AppPalette::for_appearance(appearance);
        let surface = hex_to_c32(&palette.colors.canvas);
        for byte in 0..=u8::MAX {
            for kind in [
                DiffKind::Match,
                DiffKind::Mismatch,
                DiffKind::LeftOnly,
                DiffKind::RightOnly,
            ] {
                let cell = AlignedCell {
                    byte_val: Some(byte),
                    offset: Some(byte as usize),
                    kind,
                };
                for color_mode in [ByteColorMode::Digits, ByteColorMode::Background] {
                    for is_left in [true, false] {
                        let (hex, foreground, fill) =
                            format_cell_hex(&cell, is_left, color_mode, &palette);
                        let background = fill.unwrap_or(surface);
                        assert!(
                            contrast(background, foreground) >= 4.5,
                            "{appearance:?} byte {byte:02X} {kind:?} {color_mode:?} hex label lacks contrast"
                        );

                        let (character, ascii_foreground) =
                            format_cell_ascii(&cell, is_left, color_mode, &palette);
                        if character != '·' {
                            assert!(
                                contrast(surface, ascii_foreground) >= 4.5,
                                "{appearance:?} byte {byte:02X} {kind:?} {color_mode:?} ASCII label lacks contrast"
                            );
                        }
                        assert!(!hex.is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn binary_diff_status_accents_meet_contrast_on_canvas_and_inspector() {
    fn contrast(a: Color32, b: Color32) -> f32 {
        little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
    }

    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = AppPalette::for_appearance(appearance);
        let canvas = hex_to_c32(&palette.colors.canvas);
        let recessed = hex_to_c32(&palette.colors.surface_recessed);
        let status_accents = [
            palette.harmonies.base.as_str(),
            palette.diff_add.as_str(),
            palette.diff_del.as_str(),
            palette.diff_mod.as_str(),
        ];

        for accent in status_accents {
            let preferred = hex_to_c32(accent);
            let on_canvas =
                crate::ui_ext::contrast_text_c32_on(canvas, canvas, Some(preferred), &palette);
            let on_inspector =
                crate::ui_ext::contrast_text_c32_on(recessed, recessed, Some(preferred), &palette);
            assert!(
                contrast(canvas, on_canvas) >= 4.5,
                "{appearance:?} {accent} on canvas"
            );
            assert!(
                contrast(recessed, on_inspector) >= 4.5,
                "{appearance:?} {accent} on inspector"
            );
        }
    }
}

#[test]
fn binary_difference_panel_labels_meet_contrast_in_both_appearances() {
    fn contrast(a: Color32, b: Color32) -> f32 {
        little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
    }

    fn composite(fill: Color32, surface: Color32) -> Color32 {
        let fg = egui::Rgba::from(fill);
        let bg = egui::Rgba::from(surface);
        let remaining = 1.0 - fg.a();
        Color32::from(egui::Rgba::from_rgb(
            fg.r() + bg.r() * remaining,
            fg.g() + bg.g() * remaining,
            fg.b() + bg.b() * remaining,
        ))
    }

    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = AppPalette::for_appearance(appearance);
        let canvas = hex_to_c32(&palette.colors.canvas);
        let recessed = hex_to_c32(&palette.colors.surface_recessed);
        let header_color = crate::ui_ext::contrast_text_c32_on(
            recessed,
            recessed,
            Some(hex_to_c32(&palette.diff_mod)),
            &palette,
        );
        let identical_color = crate::ui_ext::contrast_text_c32_on(
            canvas,
            canvas,
            Some(hex_to_c32(&palette.diff_add)),
            &palette,
        );
        assert!(
            contrast(recessed, header_color) >= 4.5,
            "{appearance:?} panel heading"
        );
        assert!(
            contrast(canvas, identical_color) >= 4.5,
            "{appearance:?} identical status"
        );

        for kind in [
            DiffKind::Mismatch,
            DiffKind::LeftOnly,
            DiffKind::RightOnly,
            DiffKind::Match,
        ] {
            let preferred = match kind {
                DiffKind::Mismatch => hex_to_c32(&palette.diff_mod),
                DiffKind::LeftOnly => hex_to_c32(&palette.diff_del),
                DiffKind::RightOnly => hex_to_c32(&palette.diff_add),
                DiffKind::Match => hex_to_c32(&palette.colors.text_secondary),
            };
            for selected in [false, true] {
                let fill = if selected {
                    hex_to_c32_alpha(&palette.harmonies.base, 35)
                } else {
                    recessed
                };
                let label =
                    crate::ui_ext::contrast_text_c32_on(fill, canvas, Some(preferred), &palette);
                assert!(
                    contrast(composite(fill, canvas), label) >= 4.5,
                    "{appearance:?} {kind:?} selected={selected} badge"
                );
            }
        }
    }
}

#[test]
fn binary_diff_ordinary_chrome_renders_within_compact_and_wide_hosts() {
    let left = b"header shared-left tail".to_vec();
    let right = b"header altered-right tail".to_vec();
    let mut state = BinaryDiffViewState::new(
        "left-source.bin".into(),
        left.clone(),
        "right-source.bin".into(),
        right.clone(),
    );
    state.result = Some(compute_binary_diff(
        "left-source.bin",
        &left,
        "right-source.bin",
        &right,
        BinaryDiffMode::Aligned,
        8,
    ));
    state.is_computing = false;
    state.receiver = None;
    state.search_query = "header".into();
    state.update_search();
    state.hovered_cell = Some((0, 0));

    let palette = AppPalette::default();
    for width in [1200.0, 1760.0] {
        let ctx = egui::Context::default();
        let host = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 760.0));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(host),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );

        let visible_text: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    clipped.clip_rect,
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
        for label in [
            "Binary Diff",
            "left-source.bin",
            "right-source.bin",
            "Swap [S]",
            "Summary",
            "Diffs (Hide)",
            "Mode: Aligned [Tab]",
            "Width:",
            "8B",
            "16B",
            "32B",
            "Digits",
            "Bg",
            "Prev [K]",
            "Next [J]",
            "1/1",
            "header",
            "#1",
            "Replacement",
            "9 B",
            "0x000000",
            "shle ≠ lter",
            "INSPECT: ",
        ] {
            let (clip_rect, text_rect) = visible_text
                .iter()
                .find_map(|(text, clip_rect, text_rect)| {
                    (text == label
                        || (matches!(label, "left-source.bin" | "right-source.bin")
                            && text.contains(label)))
                    .then_some((*clip_rect, *text_rect))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "{label:?} missing at host width {width}; rendered: {:?}",
                        visible_text
                            .iter()
                            .map(|(text, _, _)| text)
                            .collect::<Vec<_>>()
                    )
                });
            assert!(
                host.contains_rect(text_rect),
                "{label:?} outside {width}-point host: {text_rect:?}"
            );
            assert!(
                clip_rect.contains_rect(text_rect),
                "{label:?} elided/clipped at {width} points: text={text_rect:?}, clip={clip_rect:?}"
            );
        }

        let footer_hint = visible_text
            .iter()
            .find(|(text, _, _)| text.contains("Navigation: [Esc] Close"))
            .unwrap_or_else(|| panic!("navigation footer missing at host width {width}"));
        assert!(
            host.contains_rect(footer_hint.2),
            "navigation footer outside {width}-point host: {:?}",
            footer_hint.2
        );
        assert!(
            footer_hint.1.contains_rect(footer_hint.2),
            "navigation footer clipped at {width} points: text={:?}, clip={:?}",
            footer_hint.2,
            footer_hint.1
        );
    }
}

#[test]
fn rendered_binary_diff_summary_button_copies_exact_summary() {
    let left = b"abc".to_vec();
    let right = b"axc!".to_vec();
    let mut state = BinaryDiffViewState::new(
        "left.bin".to_owned(),
        left.clone(),
        "right.bin".to_owned(),
        right.clone(),
    );
    if let Some(ticket) = state.compute_ticket.take() {
        ticket.cancel();
    }
    state.receiver = None;
    state.result = Some(compute_binary_diff(
        "left.bin",
        &left,
        "right.bin",
        &right,
        BinaryDiffMode::Aligned,
        8,
    ));
    state.is_computing = false;

    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut close = false;
    let mut time = 0.0;
    let mut render = |state: &mut BinaryDiffViewState, time: &mut f64, events: Vec<egui::Event>| {
        *time += 0.1;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1200.0, 800.0),
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
    let summary_position = frame
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Summary" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("rendered Summary button label");
    render(
        &mut state,
        &mut time,
        vec![egui::Event::PointerMoved(summary_position)],
    );
    render(
        &mut state,
        &mut time,
        vec![
            egui::Event::PointerMoved(summary_position),
            egui::Event::PointerButton {
                pos: summary_position,
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
            egui::Event::PointerMoved(summary_position),
            egui::Event::PointerButton {
                pos: summary_position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );

    assert_eq!(
        copied.platform_output.copied_text,
        "Binary Diff Summary:\nLeft: left.bin (3 bytes)\nRight: right.bin (4 bytes)\nSimilarity: 50.00%\nMatches: 2 B | Mismatches: 1 B | Insertions: 1 B | Deletions: 0 B\nMode: Aligned"
    );
    assert!(!close);
}
