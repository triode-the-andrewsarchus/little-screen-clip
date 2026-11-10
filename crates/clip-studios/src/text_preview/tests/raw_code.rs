use super::*;

#[test]
fn measured_row_offsets_find_wrapped_rows_and_invalidate_on_width_change() {
    let mut rows = CodeRowHeights::default();
    rows.reset(4, 200.0, 20.0, 1.0);
    rows.set_height(0, 80.0);
    rows.set_height(2, 40.0);
    assert_eq!(rows.offset(2), 100.0);
    assert_eq!(rows.total_height(), 160.0);
    assert_eq!(rows.row_at(79.0), 0);
    assert_eq!(rows.row_at(80.0), 1);
    assert_eq!(rows.row_at(139.0), 2);
    assert_eq!(rows.row_at(1000.0), 3);
    rows.reset(4, 200.0, 20.0, 1.0);
    assert_eq!(rows.total_height(), 160.0);
    rows.reset(4, 400.0, 20.0, 1.0);
    assert_eq!(rows.total_height(), 80.0);
    rows.reset(0, 400.0, 20.0, 1.0);
    assert_eq!(rows.row_at(0.0), 0);
    assert_eq!(rows.total_height(), 0.0);
}

#[test]
fn block_height_index_handles_boundaries_updates_and_partial_final_block() {
    let mut rows = CodeRowHeights::default();
    rows.reset(70, 200.0, 10.0, 1.0);
    assert_eq!(
        rows.sums.len(),
        4,
        "three blocks need three Fenwick entries plus zero"
    );
    assert_eq!(rows.total_height(), 700.0);
    assert_eq!(rows.offset(CODE_ROW_HEIGHT_BLOCK_SIZE), 320.0);
    assert_eq!(rows.row_at(319.0), 31);
    assert_eq!(
        rows.row_at(320.0),
        32,
        "exact block edge begins the next row"
    );

    rows.set_height(0, 20.0);
    rows.set_height(31, 15.0);
    rows.set_height(32, 7.0);
    rows.set_height(69, 30.0);
    assert_eq!(rows.offset(32), 335.0);
    assert_eq!(rows.offset(33), 342.0);
    assert_eq!(rows.row_at(334.0), 31);
    assert_eq!(rows.row_at(335.0), 32);
    assert_eq!(rows.row_at(342.0), 33);
    assert_eq!(rows.total_height(), 732.0);
    assert_eq!(rows.offset(usize::MAX), 732.0);
    assert_eq!(rows.row_at(732.0), 69);
    assert_eq!(rows.row_at(10_000.0), 69);

    rows.set_height(32, 6.0);
    rows.set_height(32, 7.0);
    assert_eq!(
        rows.offset(33),
        342.0,
        "repeated updates apply only the latest delta"
    );

    let mut empty = CodeRowHeights::default();
    empty.reset(0, 200.0, 10.0, 1.0);
    assert!(empty.heights.is_empty());
    assert_eq!(empty.sums, vec![0.0]);
    assert_eq!(empty.offset(0), 0.0);
    assert_eq!(empty.row_at(0.0), 0);
}

#[test]
fn block_height_index_resets_all_measurements_when_geometry_key_changes() {
    let mut rows = CodeRowHeights::default();
    rows.reset(70, 200.0, 10.0, 1.0);
    rows.set_height(34, 28.0);
    rows.next_measurement = 19;
    rows.reset(70, 400.0, 10.0, 1.0);
    assert_eq!(
        rows.total_height(),
        700.0,
        "width change restores estimates"
    );
    assert_eq!(rows.next_measurement, 0);

    rows.set_height(34, 28.0);
    rows.next_measurement = 19;
    rows.reset(70, 400.0, 12.0, 1.0);
    assert_eq!(
        rows.total_height(),
        840.0,
        "line-height change restores estimates"
    );
    assert_eq!(rows.next_measurement, 0);

    rows.set_height(34, 28.0);
    rows.next_measurement = 19;
    rows.reset(70, 400.0, 12.0, 1.5);
    assert_eq!(
        rows.total_height(),
        840.0,
        "scale change restores estimates"
    );
    assert_eq!(rows.next_measurement, 0);

    rows.set_height(34, 28.0);
    rows.next_measurement = 19;
    rows.reset(70, 400.0, 12.0, 1.5);
    assert_eq!(
        rows.total_height(),
        856.0,
        "an unchanged key preserves measurements"
    );
    assert_eq!(rows.next_measurement, 19);
}

#[test]
fn huge_single_line_uses_bounded_utf8_fragments_and_keeps_full_search_source() {
    let marker = "MATCH_AT_END";
    let content = format!("{}{}", "界ab".repeat(40_000), marker);
    let config = AppConfig::default();
    let mut state = TextPreviewState::from_text(content.clone(), None, &config);

    assert!(state.code_fragments.len() > 1);
    assert!(
        state
            .code_fragments
            .iter()
            .all(|fragment| fragment.end_byte - fragment.start_byte <= MAX_CODE_FRAGMENT_BYTES)
    );
    assert!(state.code_fragments.iter().all(|fragment| {
        content.is_char_boundary(fragment.start_byte) && content.is_char_boundary(fragment.end_byte)
    }));
    assert_eq!(
        state
            .code_fragments
            .iter()
            .map(|fragment| &content[fragment.start_byte..fragment.end_byte])
            .collect::<String>(),
        content
    );

    state.find_query = marker.to_owned();
    update_search_matches_and_wait(&mut state);
    assert_eq!(state.match_count(), 1);
    let matched = state.search_match(0).unwrap();
    assert_eq!(&state.content[matched.start_byte..matched.end_byte], marker);
    state.next_match();
    let row = state.scroll_to_line.unwrap() - 1;
    let fragment = state.code_fragments[row];
    assert!(fragment.start_byte <= matched.start_byte);
    assert!(matched.start_byte < fragment.end_byte);
}

#[test]
fn huge_line_rendering_only_shapes_bounded_fragments() {
    let ctx = egui::Context::default();
    let content = "long text ".repeat(10_000);
    let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(700.0, 240.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_raw_code_view(ui, &AppPalette::default());
            });
        },
    );
    let rendered_fragment_lengths: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().len()),
            _ => None,
        })
        .collect();
    assert!(!rendered_fragment_lengths.is_empty());
    assert!(
        rendered_fragment_lengths
            .iter()
            .all(|length| *length <= MAX_CODE_FRAGMENT_BYTES)
    );
}

#[test]
fn gutter_labels_wrapped_source_line_once_on_its_first_fragment() {
    let ctx = egui::Context::default();
    let long_line = "x".repeat(MAX_CODE_FRAGMENT_BYTES + 900);
    let content = format!("fn first() {{}}\n{long_line}\nfn last() {{}}\n");
    let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
    state.show_line_numbers = true;
    state.wrap_lines = false;
    let target_first_row = state.line_first_fragment[1];
    let target_fragment = state.code_fragments[target_first_row];
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(700.0, 280.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_raw_code_view(ui, &AppPalette::default());
            });
        },
    );
    let label_shapes: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "2" => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(
        label_shapes.len(),
        1,
        "logical line label should be painted once"
    );
    let first_fragment_text = &state.content[target_fragment.start_byte..target_fragment.end_byte];
    let first_fragment_shape = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == first_fragment_text => Some(text),
            _ => None,
        })
        .expect("first wrapped fragment should be visible");
    assert_eq!(label_shapes[0].pos.y, first_fragment_shape.pos.y);
}

#[test]
fn raw_selection_copies_exact_source_across_fragments_and_virtual_scroll() {
    let prefix = (0..180)
        .map(|row| format!("before-{row}\n"))
        .collect::<String>();
    let long_line = format!("{}界{}", "a".repeat(4090), "b".repeat(4200));
    let content = format!("{prefix}{long_line}\nafter\n");
    let mut state = TextPreviewState::from_text(content.clone(), None, &AppConfig::default());
    state.wrap_lines = true;
    let first_row = state.line_first_fragment[180];
    let first = state.code_fragments[first_row];
    let second = state.code_fragments[first_row + 1];
    let start = first.end_byte - 12;
    let end = second.start_byte + 16;
    assert!(content.is_char_boundary(start));
    let ctx = egui::Context::default();
    let mut time = 0.0;
    state.scroll_to_line = Some(first_row + 1);
    let render = |state: &mut TextPreviewState, time: &mut f64, events: Vec<egui::Event>| {
        *time += 0.05;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(5_000.0, 700.0),
                )),
                time: Some(*time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_raw_code_view(ui, &AppPalette::default());
                });
            },
        )
    };
    let mut frame = render(&mut state, &mut time, Vec::new());
    for _ in 0..4 {
        frame = render(&mut state, &mut time, Vec::new());
    }
    assert!(frame.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == &content[first.start_byte..first.end_byte] && text.galley.rows.len() > 1)
    }));
    let point_for = |frame: &egui::FullOutput, fragment: CodeFragment, byte: usize| {
        let fragment_text = &content[fragment.start_byte..fragment.end_byte];
        frame
            .shapes
            .iter()
            .find_map(|clipped| {
                let egui::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                if text.galley.text() != fragment_text {
                    return None;
                }
                let char_index = content[fragment.start_byte..byte].chars().count();
                let cursor = text
                    .galley
                    .pos_from_ccursor(egui::text::CCursor::new(char_index));
                assert_eq!(
                    text.galley
                        .cursor_from_pos(cursor.center().to_vec2())
                        .ccursor
                        .index,
                    char_index,
                    "galley cursor round-trip failed for source byte {byte}"
                );
                let point = text.pos + cursor.center().to_vec2();
                clipped.clip_rect.contains(point).then_some(point)
            })
            .expect("source cursor must be visible")
    };
    let start_pos = point_for(&frame, first, start);
    let end_pos = point_for(&frame, second, end);
    for (from, to, expected_anchor, expected_focus) in [
        (start_pos, end_pos, start, end),
        (end_pos, start_pos, end, start),
    ] {
        render(
            &mut state,
            &mut time,
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let dragged = render(&mut state, &mut time, vec![egui::Event::PointerMoved(to)]);
        let selection_color = hex_to_c32_alpha(&AppPalette::default().harmonies.base, 90);
        assert!(dragged.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == selection_color)
        }));
        render(
            &mut state,
            &mut time,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(
            state.code_selection,
            Some((expected_anchor, expected_focus))
        );
        assert_eq!(
            render(&mut state, &mut time, vec![egui::Event::Copy])
                .platform_output
                .copied_text,
            content[start..end]
        );
    }
    state.scroll_to_line = Some(1);
    let offscreen = render(&mut state, &mut time, Vec::new());
    assert!(!offscreen.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == &content[first.start_byte..first.end_byte])
    }));
    assert_eq!(state.code_selection, Some((end, start)));
    assert_eq!(
        render(&mut state, &mut time, vec![egui::Event::Copy])
            .platform_output
            .copied_text,
        content[start..end]
    );

    state.code_selection = None;
    assert_eq!(
        render(&mut state, &mut time, vec![egui::Event::Copy])
            .platform_output
            .copied_text,
        content
    );

    state.scroll_to_line = Some(first_row);
    let mut frame = render(&mut state, &mut time, Vec::new());
    // This row is near EOF. Let egui clamp the requested top row to
    // the final viewport before capturing positions for pointer input.
    for _ in 0..3 {
        frame = render(&mut state, &mut time, Vec::new());
    }
    let previous = state.code_fragments[first_row - 1];
    let line_start = previous.end_byte - 3;
    let line_end = first.start_byte + 9;
    let from = point_for(&frame, previous, line_start);
    let to = point_for(&frame, first, line_end);
    render(
        &mut state,
        &mut time,
        vec![
            egui::Event::PointerMoved(from),
            egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    render(&mut state, &mut time, vec![egui::Event::PointerMoved(to)]);
    render(
        &mut state,
        &mut time,
        vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(state.code_selection, Some((line_start, line_end)));
    assert!(content[line_start..line_end].contains('\n'));
    assert_eq!(
        render(&mut state, &mut time, vec![egui::Event::Copy])
            .platform_output
            .copied_text,
        content[line_start..line_end]
    );
}

#[test]
fn minimap_scroll_extent_stays_at_document_height_at_end_of_file() {
    for wrap in [false, true] {
        let ctx = egui::Context::default();
        let content = (0..100)
            .map(|line| format!("LINE_{line:03} {}\n", "word ".repeat(25)))
            .collect::<String>();
        let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
        state.wrap_lines = wrap;
        let render = |state: &mut TextPreviewState, events: Vec<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_raw_code_view(ui, &AppPalette::default())
                    });
                },
            )
        };
        for _ in 0..30 {
            let _ = render(&mut state, Vec::new());
        }
        let frame = render(&mut state, Vec::new());
        let map = frame
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if (rect.rect.width() - MINIMAP_WIDTH).abs() < 0.1 => {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("overflowing file should paint a minimap");
        let click = egui::pos2(map.center().x, map.bottom() - 4.0);
        for pressed in [true, false] {
            let _ = render(
                &mut state,
                vec![
                    egui::Event::PointerMoved(click),
                    egui::Event::PointerButton {
                        pos: click,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(
            state.scroll_to_offset.is_some(),
            "minimap click should request navigation"
        );
        for _ in 0..5 {
            let frame = render(&mut state, Vec::new());
            assert!(
                (state.minimap_content_height - state.code_rows.total_height()).abs() < 1.0,
                "scroll extent {} must match document height {} (wrap={wrap})",
                state.minimap_content_height,
                state.code_rows.total_height()
            );
            assert!(
                (state.code_scroll_offset
                    - (state.code_rows.total_height() - state.minimap_viewport_height).max(0.0))
                .abs()
                    < 1.0,
                "clicking at the bottom should reach the final viewport"
            );
            assert!(frame.shapes.iter().any(|shape| matches!(
                &shape.shape, egui::Shape::Text(text) if text.galley.job.text.contains("LINE_099")
            )), "the final source line should remain visible after a bottom minimap click");
            assert!(
                state.code_scroll_offset
                    <= (state.code_rows.total_height() - state.minimap_viewport_height).max(0.0)
                        + 1.0
            );
        }
    }
}

#[test]
fn minimap_wrapped_line_marker_and_click_use_virtual_row_geometry() {
    let target_line = 24;
    let mut first_fragment = Vec::new();
    let mut rows = CodeRowHeights::default();
    let heights: Vec<f32> = (0..80)
        .flat_map(|line| {
            first_fragment.push(if line == 0 {
                0
            } else {
                first_fragment[line - 1] + if line - 1 == target_line { 8 } else { 1 }
            });
            std::iter::repeat_n(20.0, if line == target_line { 8 } else { 1 })
        })
        .collect();
    rows.reset(heights.len(), 500.0, 20.0, 1.0);
    for (index, height) in heights.iter().enumerate() {
        rows.set_height(index, *height);
    }
    let lines: Vec<_> = (0..80)
        .map(|line| {
            let color = if line == target_line {
                syntect::highlighting::Color {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255,
                }
            } else {
                syntect::highlighting::Color {
                    r: 0,
                    g: 0,
                    b: 255,
                    a: 255,
                }
            };
            (
                line,
                MinimapLine {
                    segments: vec![MinimapSegment {
                        start_column: 0,
                        end_column: MINIMAP_COLUMNS,
                        color,
                    }],
                },
            )
        })
        .collect();
    let palette = AppPalette::default();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let mut clicked_fraction = None;
    let mut render = |events: Vec<egui::Event>| {
        time += 0.05;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 700.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    clicked_fraction = render_code_minimap(
                        ui,
                        &lines,
                        &first_fragment,
                        &rows,
                        640.0,
                        0.0,
                        0.2,
                        &palette,
                    );
                });
            },
        )
    };
    let frame = render(Vec::new());
    let marker_color = map_syntect_color_to_theme(lines[target_line].1.segments[0].color, &palette);
    let marker = frame
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill == marker_color => Some(rect.rect),
            _ => None,
        })
        .expect("wrapped source line marker should be painted");
    let map_rect = frame
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if rect.fill == hex_to_c32(&palette.colors.surface_panel)
                    && (rect.rect.width() - MINIMAP_WIDTH).abs() < 0.1 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("minimap panel should be painted");
    let content = map_rect.shrink2(Vec2::new(4.0, 3.0));
    let expected_top = content.top()
        + content.height() * rows.offset(first_fragment[target_line]) / rows.total_height();
    let expected_bottom = content.top()
        + content.height() * rows.offset(first_fragment[target_line + 1]) / rows.total_height();
    assert!(
        (marker.top() - expected_top).abs() < 1.0
            && (marker.bottom() - expected_bottom).abs() < 1.0
    );
    let click = marker.center();
    render(vec![
        egui::Event::PointerMoved(click),
        egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    render(vec![egui::Event::PointerButton {
        pos: click,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    let fraction = clicked_fraction.expect("marker click should yield a normalized scroll target");
    let marker_fraction = (click.y - content.top()) / content.height();
    assert!((fraction - marker_fraction).abs() < 0.01);
    let viewport_height = rows.total_height() * 0.2;
    let requested = (fraction * rows.total_height() - viewport_height * 0.5)
        .clamp(0.0, rows.total_height() - viewport_height);
    assert!(
        requested < rows.offset(first_fragment[target_line + 1])
            && requested + viewport_height > rows.offset(first_fragment[target_line]),
        "centered synthetic viewport should overlap the wrapped source-line span"
    );
}

#[test]
fn unwrapped_raw_line_scrolls_horizontally_without_eliding_its_end() {
    let ctx = egui::Context::default();
    let content = format!("LEFT_EDGE{}RIGHT_EDGE", "M".repeat(600));
    let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
    state.wrap_lines = false;
    let mut time = 0.0;

    let mut render = |events: Vec<egui::Event>| {
        time += 0.05;
        ctx.run(
            egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(540.0, 360.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_raw_code_view(ui, &AppPalette::default());
                });
            },
        )
    };

    let first = render(Vec::new());
    let text_shape = |output: &egui::FullOutput| {
        output.shapes.iter().find_map(|clipped| {
            let egui::Shape::Text(text) = &clipped.shape else {
                return None;
            };
            text.galley.text().starts_with("LEFT_EDGE").then_some((
                clipped.clip_rect,
                text.pos.x,
                text.galley.size().x,
                text.galley.elided,
            ))
        })
    };
    let (clip, first_x, first_width, first_elided) =
        text_shape(&first).expect("initial code row should render");
    assert!(first_width > clip.width() * 2.0);
    assert!(!first_elided);

    let scrolled = render(vec![
        egui::Event::PointerMoved(clip.center()),
        egui::Event::Scroll(egui::vec2(-8_000.0, 0.0)),
    ]);
    let mut settled = scrolled;
    for _ in 0..8 {
        settled = render(Vec::new());
    }
    let (scrolled_clip, scrolled_x, _, scrolled_elided) =
        text_shape(&settled).expect("code row should remain visible after horizontal scroll");
    assert!(scrolled_clip.width() <= clip.width());
    assert!(
        scrolled_x < first_x - 20.0,
        "horizontal wheel should move code left: {} -> {}",
        first_x,
        scrolled_x
    );
    assert!(!scrolled_elided);
    let scrolled_right = scrolled_x + first_width;
    assert!(
        scrolled_right <= scrolled_clip.right() + 1.0 && scrolled_right >= scrolled_clip.left(),
        "horizontal scrolling should reach the line end: right edge {scrolled_right}, viewport {:?}",
        scrolled_clip
    );
}

#[test]
fn wrapped_view_places_next_line_below_the_entire_galley() {
    for scale in [1.0, 1.5, 2.0] {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let mut state = TextPreviewState::from_text(
            format!("{}\nNEXT_ROW\n{}", "word ".repeat(30), "tail\n".repeat(60)),
            None,
            &AppConfig::default(),
        );
        let render_frame = |state: &mut TextPreviewState| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_raw_code_view(ui, &AppPalette::default())
                    });
                },
            )
        };
        let output = render_frame(&mut state);
        assert!(
            state.code_rows.heights[0] > state.code_rows.heights[1] * 2.0,
            "key {:?}, heights {:?}",
            state.code_rows.key,
            &state.code_rows.heights[..3]
        );
        let text_shapes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text)
                } else {
                    None
                }
            })
            .collect();
        let first = text_shapes
            .iter()
            .find(|shape| shape.galley.text().starts_with("word "))
            .unwrap();
        let next = text_shapes
            .iter()
            .find(|shape| shape.galley.text() == "NEXT_ROW")
            .unwrap();
        assert!(
            next.pos.y >= first.pos.y + first.galley.size().y,
            "wrapped rows overlap at scale {scale}"
        );
        state.scroll_to_line = Some(2);
        let _ = render_frame(&mut state);
        assert!(
            (state.code_scroll_offset - state.code_rows.offset(1)).abs() < 1.0,
            "search uses wrapped source offset"
        );
        state.wrap_lines = false;
        state.scroll_to_line = Some(1);
        let _ = render_frame(&mut state);
        assert!((state.code_rows.heights[0] - state.code_rows.heights[1]).abs() < 1.0);

        // Navigate past the background measurement cursor, then measure more
        // preceding wrapped lines. The same source row must stay at the top.
        state = TextPreviewState::from_text(
            format!("{}\n", "word ".repeat(30)).repeat(1024),
            None,
            &AppConfig::default(),
        );
        let _ = render_frame(&mut state);
        let _ = render_frame(&mut state);
        state.scroll_to_line = Some(801);
        let _ = render_frame(&mut state);
        for _ in 0..6 {
            let _ = render_frame(&mut state);
            assert!(
                (state.code_scroll_offset - state.code_rows.offset(800)).abs() < 1.0,
                "incremental measurements moved the source anchor at scale {scale}: actual {} expected {} row {}",
                state.code_scroll_offset,
                state.code_rows.offset(800),
                state.code_rows.row_at(state.code_scroll_offset)
            );
        }
    }
}

#[test]
fn test_gutter_text_alignment() {
    let ctx = egui::Context::default();
    let output = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                let gutter_resp = ui.label(
                    egui::RichText::new("1\n2\n3")
                        .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE)),
                );
                let mut text = "#include <stdio.h>\nint main() {}\n".to_string();
                let syntax = find_syntax("c");
                let palette = AppPalette::default();
                let mut layouter = |ui: &egui::Ui, text: &str, _wrap_width: f32| {
                    let layout_job = build_highlighted_layout_job(text, syntax, &palette, &[]);
                    ui.fonts(|f| f.layout_job(layout_job))
                };
                let edit = egui::TextEdit::multiline(&mut text)
                    .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                    .layouter(&mut layouter)
                    .frame(false)
                    .margin(egui::Margin::ZERO);
                let edit_resp = ui.add(edit);

                assert_eq!(gutter_resp.rect.min.y, edit_resp.rect.min.y);
            });
        });
    });

    // Verify that the gutter text shape and the editor text shape share the exact same top Y position and row heights
    let mut text_shapes = Vec::new();
    for s in &output.shapes {
        if let egui::epaint::Shape::Text(t) = &s.shape {
            text_shapes.push(t);
        }
    }
    assert_eq!(
        text_shapes.len(),
        2,
        "Expected gutter text and code text shapes"
    );
    let gutter_text = text_shapes[0];
    let code_text = text_shapes[1];

    assert_eq!(
        gutter_text.pos.y, code_text.pos.y,
        "Gutter and code must align on identical Y coordinate"
    );
    assert_eq!(
        gutter_text.galley.rows.len(),
        code_text.galley.rows.len(),
        "Same number of rows"
    );
    for (g_row, c_row) in gutter_text
        .galley
        .rows
        .iter()
        .zip(code_text.galley.rows.iter())
    {
        assert_eq!(g_row.rect.min.y, c_row.rect.min.y, "Row min Y must align");
        assert_eq!(
            g_row.rect.height(),
            c_row.rect.height(),
            "Row heights must be identical"
        );
    }
}
