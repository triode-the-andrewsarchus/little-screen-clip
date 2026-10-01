use super::*;

fn inline_preview_data() -> Arc<SpreadsheetData> {
    let mut first_rows = vec![vec!["first-sheet".to_owned()]];
    let mut second_rows = vec![vec!["choose-b".to_owned()]];
    for row in 0..400 {
        first_rows.push(vec![format!("A-row-{row:04}")]);
        second_rows.push(vec![format!("B-row-{row:04}")]);
    }
    Arc::new(SpreadsheetData {
        title: "Shared preview data".to_owned(),
        sheets: vec![
            SpreadsheetSheet::new("Sheet A".to_owned(), first_rows),
            SpreadsheetSheet::new("Sheet B".to_owned(), second_rows),
        ],
    })
}

fn render_inline_pair(
    ctx: &egui::Context,
    first: &mut InlineSpreadsheetPreview,
    second: &mut InlineSpreadsheetPreview,
    events: Vec<egui::Event>,
    time: f64,
) -> (egui::FullOutput, [egui::Rect; 2]) {
    let palette = little_clip_core::palette::AppPalette::default();
    let mut rects = [egui::Rect::NOTHING; 2];
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(480.0, 210.0),
            )),
            events,
            time: Some(time),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    rects[0] = first.show(ui, &palette).rect;
                    rects[1] = second.show(ui, &palette).rect;
                });
            });
        },
    );
    (output, rects)
}

fn rendered_text_position(output: &egui::FullOutput, host: egui::Rect, text: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text_shape)
                if host.contains(text_shape.pos) && text_shape.galley.job.text == text =>
            {
                Some(text_shape.pos)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("could not find rendered {text:?} in {host:?}"))
}

fn pointer_click_events(position: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(position),
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn inline_spreadsheet_previews_share_data_with_independent_navigation() {
    let data = inline_preview_data();
    let mut first = InlineSpreadsheetPreview::new(Arc::clone(&data), Vec2::new(220.0, 124.0));
    let mut second = InlineSpreadsheetPreview::new(Arc::clone(&data), Vec2::new(220.0, 124.0));
    assert!(Arc::ptr_eq(&first.data, &second.data));
    assert_ne!(first.id, second.id);

    let ctx = egui::Context::default();
    let (initial, rects) = render_inline_pair(&ctx, &mut first, &mut second, Vec::new(), 0.0);
    let second_first_cell_clip = initial
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text)
                if rects[1].contains(text.pos) && text.galley.job.text == "first-sheet" =>
            {
                Some(shape.clip_rect)
            }
            _ => None,
        })
        .expect("second preview should render its first cell");
    assert!(second_first_cell_clip.intersects(rects[1]));
    assert!(second_first_cell_clip.min.x >= rects[1].min.x);
    assert!(second_first_cell_clip.max.x <= rects[1].max.x);
    assert!(second_first_cell_clip.width() <= INLINE_SPREADSHEET_COLUMN_WIDTH + 1.0);
    let next_position = rendered_text_position(&initial, rects[0], "›");
    render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        pointer_click_events(next_position, true),
        0.1,
    );
    let (after_tab_click, rects) = render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        pointer_click_events(next_position, false),
        0.2,
    );
    assert_eq!(first.active_sheet_idx, 1);
    assert_eq!(second.active_sheet_idx, 0);

    let cell_position = rendered_text_position(&after_tab_click, rects[0], "choose-b");
    render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        pointer_click_events(cell_position, true),
        0.3,
    );
    render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        pointer_click_events(cell_position, false),
        0.4,
    );
    assert_eq!(first.selected_cell, Some((0, 0)));
    assert_eq!(second.selected_cell, None);

    let wheel_position = egui::pos2(rects[0].center().x, rects[0].bottom() - 18.0);
    let (scrolled, rects) = render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        vec![
            egui::Event::PointerMoved(wheel_position),
            egui::Event::Scroll(Vec2::new(0.0, -500.0)),
        ],
        0.5,
    );
    assert!(first.scroll_offset.y > 0.0);
    assert_eq!(second.scroll_offset, Vec2::ZERO);
    assert!(
        scrolled.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text)
                if rects[0].contains(text.pos)
                    && text.galley.job.text.starts_with("B-row-00")
                    && shape.clip_rect.intersects(rects[0])
                    && shape.clip_rect.width() <= INLINE_SPREADSHEET_COLUMN_WIDTH + 1.0)
        }),
        "scrolling should reveal later rows whose text remains clipped to a cell inside the first host"
    );
    assert!(
        scrolled.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text)
                if rects[1].contains(text.pos)
                    && text.galley.job.text == "first-sheet"
                    && shape.clip_rect.intersects(rects[1]))
        }),
        "the second preview should remain on the first sheet"
    );
}

#[test]
fn clipped_inline_spreadsheet_preview_renders_no_cells() {
    let data = inline_preview_data();
    let mut preview = InlineSpreadsheetPreview::new(data, Vec2::new(220.0, 124.0));
    let ctx = egui::Context::default();
    let palette = little_clip_core::palette::AppPalette::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(300.0, 100.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(200.0);
                preview.show(ui, &palette);
            });
        },
    );
    assert_eq!(preview.scroll_offset, Vec2::ZERO);
    assert!(
        output
            .shapes
            .iter()
            .all(|shape| { !matches!(&shape.shape, egui::epaint::Shape::Text(_)) })
    );
}

#[test]
fn inline_cell_display_text_caps_unicode_scalars_without_changing_source() {
    let source = format!("{}終", "あ".repeat(INLINE_SPREADSHEET_MAX_CELL_CHARS));
    let display = inline_cell_display_text(&source);
    assert_eq!(
        display.chars().count(),
        INLINE_SPREADSHEET_MAX_CELL_CHARS + 1
    );
    assert!(display.ends_with('…'));
    assert_eq!(
        source.chars().count(),
        INLINE_SPREADSHEET_MAX_CELL_CHARS + 1
    );

    let short = "short text";
    assert!(matches!(inline_cell_display_text(short), Cow::Borrowed(value) if value == short));
}

#[test]
fn inline_spreadsheet_navigation_stays_one_row_for_many_sheets() {
    let sheets = (0..200)
        .map(|index| {
            SpreadsheetSheet::new(
                format!("{}-{index}", "非常に長いシート名".repeat(8)),
                vec![vec![format!("sheet-{index}")]],
            )
        })
        .collect();
    let data = Arc::new(SpreadsheetData {
        title: "Many sheets".to_owned(),
        sheets,
    });
    let mut first = InlineSpreadsheetPreview::new(Arc::clone(&data), Vec2::new(220.0, 124.0));
    let mut second = InlineSpreadsheetPreview::new(data, Vec2::new(220.0, 124.0));
    let ctx = egui::Context::default();
    let (output, rects) = render_inline_pair(&ctx, &mut first, &mut second, Vec::new(), 0.0);

    let current_sheet_labels = output
        .shapes
        .iter()
        .filter(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => {
                rects[0].contains(text.pos) && text.galley.job.text.starts_with("1/200 · ")
            }
            _ => false,
        })
        .count();
    assert_eq!(
        current_sheet_labels, 1,
        "only one current-sheet label is shown"
    );
    let first_cell = rendered_text_position(&output, rects[0], "sheet-0");
    assert!(
        first_cell.y < rects[0].top() + 100.0,
        "grid should remain visible below a single navigation row"
    );

    let next_position = rendered_text_position(&output, rects[0], "›");
    render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        pointer_click_events(next_position, true),
        0.1,
    );
    render_inline_pair(
        &ctx,
        &mut first,
        &mut second,
        pointer_click_events(next_position, false),
        0.2,
    );
    assert_eq!(
        first.active_sheet_idx, 1,
        "next keeps every sheet one click away"
    );
    assert_eq!(
        second.active_sheet_idx, 0,
        "navigation state remains instance-local"
    );
}

#[test]
fn independent_spreadsheet_views_share_data_and_keep_navigation_separate() {
    let source = Arc::new(SpreadsheetData {
        title: "Shared workbook".into(),
        sheets: vec![SpreadsheetSheet::new(
            "Data".into(),
            vec![vec!["Name".into()], vec!["Shared cell".into()]],
        )],
    });
    let mut first = SpreadsheetViewerState::from_shared(Arc::clone(&source), None);
    let second = SpreadsheetViewerState::from_shared(Arc::clone(&source), None);

    assert!(Arc::ptr_eq(&first.data, &second.data));
    assert!(Arc::ptr_eq(&first.data, &source));
    first.search_query = "shared".into();
    first.selected_cell = Some((1, 0));
    first.scroll_offset = Vec2::new(40.0, 80.0);

    assert!(second.search_query.is_empty());
    assert_eq!(second.selected_cell, None);
    assert_eq!(second.scroll_offset, Vec2::ZERO);
}

#[test]
fn inline_spreadsheet_source_seeds_independent_full_studios_without_loading_again() {
    let source = Arc::new(SpreadsheetData {
        title: "Gallery workbook".to_owned(),
        sheets: vec![
            SpreadsheetSheet::new("One".to_owned(), vec![vec!["one".to_owned()]]),
            SpreadsheetSheet::new("Two".to_owned(), vec![vec!["two".to_owned()]]),
        ],
    });
    let inline = InlineSpreadsheetPreview::new(Arc::clone(&source), Vec2::new(220.0, 120.0));
    let mut first = SpreadsheetViewerState::from_shared(Arc::clone(&source), None);
    let second = SpreadsheetViewerState::from_shared(Arc::clone(&source), None);

    assert!(Arc::ptr_eq(&inline.data, &source));
    assert!(Arc::ptr_eq(&first.data, &source));
    assert!(Arc::ptr_eq(&second.data, &source));
    first.active_sheet_idx = 1;
    first.search_query = "two".to_owned();
    first.selected_cell = Some((0, 0));
    first.scroll_offset = Vec2::new(10.0, 20.0);
    assert_eq!(second.active_sheet_idx, 0);
    assert!(second.search_query.is_empty());
    assert_eq!(second.selected_cell, None);
    assert_eq!(second.scroll_offset, Vec2::ZERO);
    // `from_shared` wraps already-parsed data directly and has no file/load job state.
}

#[test]
fn escape_clears_spreadsheet_search_before_closing_viewer() {
    let data = SpreadsheetData {
        title: "Escape filter fixture".into(),
        sheets: vec![SpreadsheetSheet::new(
            "Data".into(),
            vec![vec!["Name".into()], vec!["Needle".into()]],
        )],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    state.search_query = "needle".into();
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let escape = || egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut close = false;

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 620.0),
            )),
            events: vec![escape()],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );
    assert!(state.search_query.is_empty());
    assert!(
        !close,
        "the first Escape clears the filter and keeps the viewer open"
    );

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 620.0),
            )),
            events: vec![escape()],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut close);
            });
        },
    );
    assert!(close, "the next Escape closes with an empty filter");
}

#[test]
fn visible_range_tracks_scroll_with_bounded_overscan() {
    assert_eq!(visible_row_range(0.0, 270.0, 2000, 27.0), 0..11);
    assert_eq!(visible_row_range(27000.0, 270.0, 2000, 27.0), 999..1011);
    assert_eq!(visible_row_range(80000.0, 270.0, 2000, 27.0), 1998..2000);
    assert_eq!(visible_row_range(0.0, 270.0, 0, 27.0), 0..0);
}

#[test]
fn visible_columns_follow_measured_widths() {
    let offsets = column_offsets(&[100.0, 120.0, 80.0, 90.0], 1.0);
    assert_eq!(offsets, vec![0.0, 101.0, 222.0, 303.0, 393.0]);
    assert_eq!(visible_column_range(0.0, 100.0, &offsets), 0..2);
    assert_eq!(visible_column_range(230.0, 80.0, &offsets), 1..4);
    assert_eq!(visible_column_range(1000.0, 80.0, &offsets), 2..4);
    assert_eq!(visible_column_range(0.0, 80.0, &[0.0]), 0..0);
}

#[test]
fn wide_sheet_renders_only_visible_columns_with_aligned_header() {
    let first_row = (0..1000)
        .map(|column| {
            if column == 0 {
                "A (1)".to_string()
            } else {
                format!("Cell {column}")
            }
        })
        .collect();
    let data = SpreadsheetData {
        title: "Wide sheet".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), vec![first_row])],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 620.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );

    assert!(
        output.shapes.len() < 1000,
        "rendered {} shapes for a 1,000-column sheet",
        output.shapes.len()
    );
    let matching_x: Vec<f32> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "A (1)" => Some(text.pos.x),
            _ => None,
        })
        .collect();
    assert_eq!(matching_x.len(), 2);
    assert!((matching_x[0] - matching_x[1]).abs() <= 2.0);
}

#[test]
fn spreadsheet_grid_scrolls_horizontally_and_vertically() {
    let rows = (0..160)
        .map(|row| {
            (0..80)
                .map(|column| format!("r{row}c{column}"))
                .collect::<Vec<_>>()
        })
        .collect();
    let data = SpreadsheetData {
        title: "Two-axis scroll".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let mut render = |events: Vec<egui::Event>| {
        time += 0.05;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(860.0, 620.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        (output, state.scroll_offset)
    };

    let (first, first_offset) = render(Vec::new());
    let grid_clip = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "r0c0" => Some(clipped.clip_rect),
            _ => None,
        })
        .expect("initial grid cell should render");
    let initial_header = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "A (1)" => Some(text.pos),
            _ => None,
        })
        .expect("initial column header should render");
    let initial_gutter_clip_left = first
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "1" => Some(clipped.clip_rect.left()),
            _ => None,
        })
        .expect("initial row number should render");
    assert_eq!(first_offset, Vec2::ZERO);

    let _ = render(vec![
        egui::Event::PointerMoved(grid_clip.center()),
        // Spreadsheet code normalizes horizontal wheel direction before the
        // grid ScrollArea consumes it.
        egui::Event::Scroll(Vec2::new(800.0, -800.0)),
    ]);
    let mut settled = None;
    for _ in 0..8 {
        settled = Some(render(Vec::new()));
    }

    let (output, offset) = settled.unwrap();
    assert!(offset.x > 20.0, "x offset: {offset:?}");
    assert!(offset.y > 20.0, "y offset: {offset:?}");
    let (scrolled_row, scrolled_column, scrolled_cell) = output
        .shapes
        .iter()
        .find_map(|clipped| {
            let egui::Shape::Text(text) = &clipped.shape else {
                return None;
            };
            let (row, column) = text.galley.text().strip_prefix('r')?.split_once('c')?;
            let row = row.parse::<usize>().ok()?;
            let column = column.parse::<usize>().ok()?;
            (row > 0 && column > 0).then_some((row, column, text.pos))
        })
        .expect("scrolled visible cell should have nonzero row and column");
    let header_label = format!("{} ({})", col_label(scrolled_column), scrolled_column + 1);
    let scrolled_header = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == header_label => Some(text.pos),
            _ => None,
        })
        .expect("visible scrolled column header should stay rendered");
    assert!(
        (scrolled_header.y - initial_header.y).abs() <= 1.0,
        "column header moved vertically: {initial_header:?} -> {scrolled_header:?}"
    );
    let gutter_label = (scrolled_row + 1).to_string();
    let scrolled_gutter = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == gutter_label => {
                Some((text.pos, clipped.clip_rect))
            }
            _ => None,
        })
        .expect("row number for scrolled visible row should render");
    assert_eq!(
        scrolled_gutter.1.left(),
        initial_gutter_clip_left,
        "row-number gutter should remain pinned to the left"
    );
    assert!(
        (scrolled_gutter.0.y - scrolled_cell.y).abs() <= 12.0,
        "row number and visible cell are not aligned: {:?} vs {:?}",
        scrolled_gutter.0,
        scrolled_cell
    );
    assert!(
        output.shapes.iter().any(|clipped| {
            let egui::Shape::Text(text) = &clipped.shape else {
                return false;
            };
            let Some((row, column)) = text
                .galley
                .text()
                .strip_prefix('r')
                .and_then(|cell| cell.split_once('c'))
            else {
                return false;
            };
            row.parse::<usize>().is_ok_and(|row| row > 0)
                && column.parse::<usize>().is_ok_and(|column| column > 0)
                && text.pos.x < grid_clip.right()
                && text.pos.y < grid_clip.bottom()
        }),
        "scrolled grid should continue rendering visible cells"
    );
}

#[test]
fn spreadsheet_scroll_targets_stay_aligned_across_host_sizes_and_zoom() {
    let rows = (0..320)
        .map(|row| {
            (0..72)
                .map(|column| format!("r{row}c{column}"))
                .collect::<Vec<_>>()
        })
        .collect();
    let data = SpreadsheetData {
        title: "Zoomed scroll alignment".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
    };
    let palette = little_clip_core::palette::AppPalette::default();

    for (size, zoom) in [
        (Vec2::new(640.0, 520.0), 1.0),
        (Vec2::new(640.0, 520.0), 1.5),
        (Vec2::new(1180.0, 720.0), 1.0),
        (Vec2::new(1180.0, 720.0), 1.5),
    ] {
        let mut state = SpreadsheetViewerState::new(data.clone(), None);
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        ctx.set_zoom_factor(zoom);
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            (output, state.scroll_offset)
        };

        let (initial, initial_offset) = render(Vec::new());
        assert_eq!(initial_offset, Vec2::ZERO, "host={size:?}, zoom={zoom}");
        let cell_clip = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "r0c0" => {
                    Some((text.pos + text.galley.size() * 0.5, clipped.clip_rect))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("initial cell missing at host={size:?}, zoom={zoom}"));
        let initial_header_y = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "A (1)" => Some(text.pos.y),
                _ => None,
            })
            .expect("initial column header should render");
        let gutter_left = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "1" => {
                    Some(clipped.clip_rect.left())
                }
                _ => None,
            })
            .expect("initial row number should render");

        let (cell_center, grid_clip) = cell_clip;
        assert!(
            grid_clip.contains(cell_center),
            "initial cell center {cell_center:?} outside grid clip {grid_clip:?} at host={size:?}, zoom={zoom}"
        );
        let _ = render(vec![
            egui::Event::PointerMoved(cell_center),
            egui::Event::Scroll(Vec2::new(800.0, -800.0)),
        ]);
        let mut settled = None;
        for _ in 0..10 {
            settled = Some(render(Vec::new()));
        }
        let (output, offset) = settled.expect("scroll settling frames ran");
        assert!(
            offset.x > 10.0 && offset.y > 10.0,
            "two-axis scroll should move at host={size:?}, zoom={zoom}: {offset:?}"
        );

        let (row, column, cell_center) = output
            .shapes
            .iter()
            .find_map(|clipped| {
                let egui::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let (row, column) = text.galley.text().strip_prefix('r')?.split_once('c')?;
                let row = row.parse::<usize>().ok()?;
                let column = column.parse::<usize>().ok()?;
                (row > 0 && column > 0).then_some((
                    row,
                    column,
                    text.pos + text.galley.size() * 0.5,
                ))
            })
            .unwrap_or_else(|| {
                panic!("no later cell visible at host={size:?}, zoom={zoom}, offset={offset:?}")
            });
        let header_label = format!("{} ({})", col_label(column), column + 1);
        let header = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == header_label => {
                    Some((text.pos, text.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("scrolled header {header_label:?} missing at host={size:?}, zoom={zoom}")
            });
        let header_center = header.0 + header.1 * 0.5;
        assert!(
            (header_center.x - cell_center.x).abs() <= 4.0,
            "cell/header x mismatch at host={size:?}, zoom={zoom}: cell={cell_center:?}, header={header_center:?}"
        );
        assert!(
            (header.0.y - initial_header_y).abs() <= 1.0,
            "sticky column header moved vertically at host={size:?}, zoom={zoom}"
        );

        let row_label = (row + 1).to_string();
        let gutter = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == row_label
                        && clipped.clip_rect.left() == gutter_left =>
                {
                    Some((text.pos, text.galley.size(), clipped.clip_rect))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("scrolled row label {row_label:?} missing at host={size:?}, zoom={zoom}")
            });
        let gutter_center = gutter.0 + gutter.1 * 0.5;
        assert_eq!(
            gutter.2.left(),
            gutter_left,
            "row-number gutter shifted horizontally at host={size:?}, zoom={zoom}"
        );
        assert!(
            (gutter_center.y - cell_center.y).abs() <= 14.0,
            "row gutter/cell y mismatch at host={size:?}, zoom={zoom}: gutter={gutter_center:?}, cell={cell_center:?}"
        );
    }
}

#[test]
fn spreadsheet_scrolled_cell_selection_copies_exact_value_across_host_sizes_and_zoom() {
    let rows = (0..320)
        .map(|row| {
            (0..72)
                .map(|column| format!("r{row}c{column}"))
                .collect::<Vec<_>>()
        })
        .collect();
    let data = SpreadsheetData {
        title: "Zoomed cell selection".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
    };
    let palette = little_clip_core::palette::AppPalette::default();

    for (size, zoom) in [
        (Vec2::new(640.0, 520.0), 1.0),
        (Vec2::new(640.0, 520.0), 1.5),
        (Vec2::new(1180.0, 720.0), 1.0),
        (Vec2::new(1180.0, 720.0), 1.5),
    ] {
        let mut state = SpreadsheetViewerState::new(data.clone(), None);
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        ctx.set_zoom_factor(zoom);
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            (output, state.scroll_offset, state.selected_cell)
        };

        let (initial, _, _) = render(Vec::new());
        let first_cell_center = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "r0c0" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("initial cell missing at host={size:?}, zoom={zoom}"));
        let _ = render(vec![
            egui::Event::PointerMoved(first_cell_center),
            egui::Event::Scroll(Vec2::new(800.0, -800.0)),
        ]);
        let mut settled = None;
        for _ in 0..10 {
            settled = Some(render(Vec::new()));
        }
        let (scrolled, offset, _) = settled.expect("scroll settling frames ran");
        assert!(
            offset.x > 10.0 && offset.y > 10.0,
            "two-axis scroll should move at host={size:?}, zoom={zoom}: {offset:?}"
        );

        let (row, column, target_center, target_clip) = scrolled
            .shapes
            .iter()
            .find_map(|clipped| {
                let egui::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let (row, column) = text.galley.text().strip_prefix('r')?.split_once('c')?;
                let row = row.parse::<usize>().ok()?;
                let column = column.parse::<usize>().ok()?;
                let center = text.pos + text.galley.size() * 0.5;
                (row > 0 && column > 0 && clipped.clip_rect.contains(center)).then_some((
                    row,
                    column,
                    center,
                    clipped.clip_rect,
                ))
            })
            .unwrap_or_else(|| {
                panic!("no later target cell at host={size:?}, zoom={zoom}, offset={offset:?}")
            });
        assert!(
            target_clip.contains(target_center),
            "target center {target_center:?} outside clip {target_clip:?} at host={size:?}, zoom={zoom}"
        );
        let expected_value = format!("r{row}c{column}");
        let (pressed, _, _) = render(vec![
            egui::Event::PointerMoved(target_center),
            egui::Event::PointerButton {
                pos: target_center,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let (released, _, selected_cell) = render(vec![
            egui::Event::PointerMoved(target_center),
            egui::Event::PointerButton {
                pos: target_center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(
            selected_cell,
            Some((row, column)),
            "selection mismatch for {expected_value} at host={size:?}, zoom={zoom}"
        );
        assert_eq!(
            released.platform_output.copied_text, expected_value,
            "copied value mismatch at host={size:?}, zoom={zoom}"
        );
        assert!(pressed.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == expected_value)
            }));
    }
}

#[test]
fn spreadsheet_search_highlights_visible_result_across_host_sizes_and_zoom() {
    let match_row = 137;
    let match_column = 2;
    let rows = (0..320)
        .map(|row| {
            (0..72)
                .map(|column| {
                    if row == match_row && column == match_column {
                        "needle".to_owned()
                    } else {
                        format!("r{row}c{column}")
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let data = SpreadsheetData {
        title: "Zoomed search result".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
    };
    let palette = little_clip_core::palette::AppPalette::default();
    let highlight = crate::hex_to_c32(&palette.harmonies.warm_attention);

    for (size, zoom) in [
        (Vec2::new(640.0, 520.0), 1.0),
        (Vec2::new(640.0, 520.0), 1.5),
        (Vec2::new(1180.0, 720.0), 1.0),
        (Vec2::new(1180.0, 720.0), 1.5),
    ] {
        let mut state = SpreadsheetViewerState::new(data.clone(), None);
        // Use the same state path the search field updates; rendering applies
        // row filtering and the per-cell match background.
        state.search_query = "needle".into();
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        ctx.set_zoom_factor(zoom);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );

        assert_eq!(state.data.sheets[0].rows.len(), 320);
        assert_eq!(state.filtered_rows, vec![0, match_row]);
        let target_center = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "needle" => {
                        let center = text.pos + text.galley.size() * 0.5;
                        clipped.clip_rect.contains(center).then_some(center)
                    }
                    _ => None,
                })
                .find(|center| {
                    output.shapes.iter().any(|clipped| {
                        matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == highlight && rect.rect.contains(*center))
                    })
                })
                .unwrap_or_else(|| {
                    panic!("matching cell should be visible and highlighted at host={size:?}, zoom={zoom}")
                });

        let highlight_rect = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == highlight && rect.rect.contains(target_center) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("matching cell should have the search highlight background");
        let row_label = (match_row + 1).to_string();
        let gutter = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == row_label => {
                        let center = text.pos + text.galley.size() * 0.5;
                        clipped.clip_rect.contains(center).then_some(center)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("matching row number {row_label} should be visible at host={size:?}, zoom={zoom}")
                });
        assert!(
            (gutter.y - target_center.y).abs() <= 14.0,
            "search result and row label lost alignment at host={size:?}, zoom={zoom}: cell={target_center:?}, gutter={gutter:?}"
        );
        assert!(
            highlight_rect.contains(target_center),
            "highlight should cover the visible result at host={size:?}, zoom={zoom}"
        );
        assert!(state.scroll_offset.y == 0.0);
    }
}

#[test]
fn selecting_long_cell_shows_and_copies_full_value() {
    let long_value = format!(
        "Long clipboard cell value: {}",
        "complete-value ".repeat(40)
    );
    let data = SpreadsheetData {
        title: "Long cell".into(),
        sheets: vec![SpreadsheetSheet::new(
            "Data".into(),
            vec![vec!["Header".into()], vec![long_value.clone()]],
        )],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(760.0, 620.0));
    let mut time = 0.0;
    let mut render = |events: Vec<egui::Event>| {
        time += 0.05;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        let column_width = state.column_width_cache[0]
            .as_ref()
            .map(|widths| widths[0].clamp(90.0, 420.0));
        (output, column_width, state.selected_cell)
    };

    let (initial, measured_cell_width, _) = render(Vec::new());
    let (cell_text, text_width) = initial
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == long_value => {
                Some((text.pos + Vec2::splat(1.0), text.galley.size().x))
            }
            _ => None,
        })
        .expect("long cell value should be rendered");
    let measured_cell_width = measured_cell_width.unwrap();
    assert!(
        measured_cell_width < text_width,
        "test value must exceed the grid column width ({measured_cell_width} < {text_width})"
    );

    let (pressed, _, _) = render(vec![
        egui::Event::PointerMoved(cell_text),
        egui::Event::PointerButton {
            pos: cell_text,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    let (copied, _, selected_cell) = render(vec![
        egui::Event::PointerMoved(cell_text),
        egui::Event::PointerButton {
            pos: cell_text,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    assert_eq!(selected_cell, Some((1, 0)));
    assert_eq!(copied.platform_output.copied_text, long_value);
    assert!(pressed.shapes.iter().any(|clipped| {
        matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == long_value)
    }));

    let (selected, _, _) = render(Vec::new());
    let readout = format!("Selected R2 C1 [A]: {long_value}");
    assert!(
        selected.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == readout)
        }),
        "selected-cell footer should expose the complete value"
    );
}

#[test]
fn rendered_copy_table_button_copies_active_sheet_as_exact_tsv() {
    let data = SpreadsheetData {
        title: "TSV copy fixture".into(),
        sheets: vec![
            SpreadsheetSheet::new("Other".into(), vec![vec!["not copied".into()]]),
            SpreadsheetSheet::new(
                "Active".into(),
                vec![
                    vec!["Name".into(), "Count".into(), "Note".into()],
                    vec!["alpha".into(), "1".into(), "日本語".into()],
                    vec!["beta".into(), "2".into(), "✓".into()],
                ],
            ),
        ],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    state.active_sheet_idx = 1;
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(900.0, 620.0));
    let mut time = 0.0;
    let mut render = |events: Vec<egui::Event>| {
        time += 0.1;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        )
    };

    let initial = render(Vec::new());
    let copy_position = initial
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Copy Table" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("rendered Copy Table toolbar label");
    render(vec![egui::Event::PointerMoved(copy_position)]);
    render(vec![
        egui::Event::PointerMoved(copy_position),
        egui::Event::PointerButton {
            pos: copy_position,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    let copied = render(vec![
        egui::Event::PointerMoved(copy_position),
        egui::Event::PointerButton {
            pos: copy_position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);

    assert_eq!(
        copied.platform_output.copied_text,
        "Name\tCount\tNote\nalpha\t1\t日本語\nbeta\t2\t✓"
    );
}

#[test]
fn spreadsheet_header_controls_fit_compact_and_wide_hosts() {
    let data = SpreadsheetData {
        title: "Sheet overview".into(),
        sheets: vec![
            SpreadsheetSheet::new("Overview".into(), vec![vec!["Name".into()]]),
            SpreadsheetSheet::new("Archive".into(), vec![vec!["Old name".into()]]),
        ],
    };
    let palette = little_clip_core::palette::AppPalette::default();

    for width in [900.0, 1500.0] {
        let mut state = SpreadsheetViewerState::new(data.clone(), None);
        let ctx = egui::Context::default();
        let host = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 620.0));
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

        let required_labels = [
            "Sheet overview",
            "Copy Table",
            "Filter cells...",
            "Calc",
            "Close (Esc)",
            "Sheets:",
            "Overview",
            "Archive",
        ];
        let rendered = |label: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some((
                        clipped.clip_rect,
                        egui::Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    _ => None,
                })
        };
        let mut header_text_rects = Vec::new();
        for label in required_labels {
            let (clip_rect, text_rect) = rendered(label)
                .unwrap_or_else(|| panic!("{label:?} was not rendered at host width {width}"));
            assert!(
                host.contains_rect(text_rect),
                "{label:?} extends outside the {width}-point host: {text_rect:?}"
            );
            assert!(
                clip_rect.contains_rect(text_rect),
                "{label:?} is clipped at host width {width}: text={text_rect:?}, clip={clip_rect:?}"
            );
            if matches!(
                label,
                "Sheet overview" | "Copy Table" | "Filter cells..." | "Calc" | "Close (Esc)"
            ) {
                header_text_rects.push((label, text_rect));
            }
        }

        for (idx, (left_label, left_rect)) in header_text_rects.iter().enumerate() {
            for (right_label, right_rect) in header_text_rects.iter().skip(idx + 1) {
                assert!(
                    !left_rect.intersects(*right_rect),
                    "header labels {left_label:?} and {right_label:?} overlap at {width} points"
                );
            }
        }
    }
}

#[test]
fn large_sheet_renders_only_visible_cells() {
    let rows = std::iter::once(vec!["Name".into(), "Value".into()])
        .chain((0..5000).map(|row| vec![format!("Row {row}"), row.to_string()]))
        .collect();
    let data = SpreadsheetData {
        title: "Large sheet".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            Vec2::new(900.0, 620.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut close = false;
            state.render_ui(ui, ctx, &palette, &mut close);
            assert!(!close);
        });
    });

    assert_eq!(state.filtered_rows.len(), 5001);
    assert!(
        output.shapes.len() < 1000,
        "rendered {} shapes for a 5,001-row sheet",
        output.shapes.len()
    );
    let text_y = |label: &str| {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos.y),
            _ => None,
        })
    };
    let first_gutter = text_y("1").expect("first row number is visible");
    let first_cell = text_y("Name").expect("first row is visible");
    assert!(
        (first_gutter - first_cell).abs() <= 2.0,
        "gutter y={first_gutter}, cell y={first_cell}"
    );

    state.search_query = "Row 4975".into();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.render_ui(ui, ctx, &palette, &mut false);
        });
    });
    assert_eq!(state.filtered_rows, vec![0, 4976]);
}

#[test]
fn large_virtual_sheet_bounds_rendered_cells_and_reaches_far_corner() {
    let column_count = 512;
    let rows = std::iter::once(
        (0..column_count)
            .map(|column| format!("Column {column}"))
            .collect(),
    )
    .chain((0..3000).map(|row| {
        (0..column_count)
            .map(|column| format!("r{row}c{column}"))
            .collect::<Vec<_>>()
    }))
    .collect();
    let data = SpreadsheetData {
        title: "Large virtual sheet".into(),
        sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    let mut render = |events: Vec<egui::Event>| {
        time += 0.05;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 620.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        (output, state.scroll_offset)
    };
    let is_grid_cell = |text: &str| {
        text.strip_prefix('r')
            .and_then(|cell| cell.split_once('c'))
            .and_then(|(row, column)| {
                Some((row.parse::<usize>().ok()?, column.parse::<usize>().ok()?))
            })
            .is_some()
    };
    let grid_cells = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => is_grid_cell(text.galley.text()),
                _ => false,
            })
            .count()
    };

    let (initial, initial_offset) = render(Vec::new());
    assert_eq!(initial_offset, Vec2::ZERO);
    let initial_count = grid_cells(&initial);
    assert!(initial_count > 0);
    assert!(
        initial_count <= 400,
        "rendered {initial_count} grid cells initially for a 3,001 × {column_count} sheet"
    );
    let first_cell_clip = initial
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "r0c0" => Some(clipped.clip_rect),
            _ => None,
        })
        .expect("top-left grid cell should be visible initially");

    let _ = render(vec![
        egui::Event::PointerMoved(first_cell_clip.center()),
        // Spreadsheet code normalizes horizontal wheel direction before the
        // grid ScrollArea consumes it.
        egui::Event::Scroll(Vec2::new(100_000.0, -100_000.0)),
    ]);
    let mut settled = None;
    for _ in 0..16 {
        settled = Some(render(Vec::new()));
    }
    let (scrolled, offset) = settled.expect("scroll settling frames ran");
    let scrolled_count = grid_cells(&scrolled);
    assert!(scrolled_count > 0);
    assert!(
        scrolled_count <= 400,
        "rendered {scrolled_count} grid cells after scrolling for a 3,001 × {column_count} sheet"
    );
    assert!(
        offset.x > 10_000.0,
        "horizontal offset did not reach later columns: {offset:?}"
    );
    assert!(
        offset.y > 50_000.0,
        "vertical offset did not reach later rows: {offset:?}"
    );
    assert!(
        scrolled.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => text.galley.text() == "r2999c511",
            _ => false,
        }),
        "far-corner cell should render after scrolling; offset={offset:?}"
    );
    println!(
        "large sheet rendered {initial_count} initial and {scrolled_count} scrolled cells; offset={offset:?}"
    );
}

#[test]
fn column_widths_follow_rendered_text_width() {
    let data = SpreadsheetData {
        title: "Widths".into(),
        sheets: vec![SpreadsheetSheet::new(
            "Data".into(),
            vec![vec!["W".repeat(20), "i".repeat(20)]],
        )],
    };
    let mut state = SpreadsheetViewerState::new(data, None);
    let palette = little_clip_core::palette::AppPalette::default();
    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 620.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        },
    );
    let measured = state.column_width_cache[0].as_ref().unwrap();
    assert!(measured[0] > measured[1] * 2.0);
}

#[test]
fn test_col_label() {
    assert_eq!(col_label(0), "A");
    assert_eq!(col_label(1), "B");
    assert_eq!(col_label(25), "Z");
    assert_eq!(col_label(26), "AA");
    assert_eq!(col_label(27), "AB");
    assert_eq!(col_label(51), "AZ");
    assert_eq!(col_label(52), "BA");
    assert_eq!(col_label(701), "ZZ");
    assert_eq!(col_label(702), "AAA");
}

#[test]
fn test_spreadsheet_state_creation() {
    let sheet = SpreadsheetSheet::new(
        "Sheet1".to_string(),
        vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ],
    );
    let data = SpreadsheetData {
        title: "Test".to_string(),
        sheets: vec![sheet],
    };
    let state = SpreadsheetViewerState::new(data, None);
    assert_eq!(state.scroll_offset, Vec2::ZERO);
    assert_eq!(state.active_sheet_idx, 0);
    assert_eq!(state.current_sheet().unwrap().total_rows, 2);
}
