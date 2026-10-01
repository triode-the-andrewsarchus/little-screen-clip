use super::*;
#[test]
fn inline_text_previews_share_source_with_independent_scroll_state() {
    let data = TextPreviewData::from_text(Arc::new(
        (0..400).map(|line| format!("row-{line:04}\n")).collect(),
    ));
    let mut first = InlineTextPreview::new(Arc::clone(&data), Vec2::new(180.0, 72.0));
    let mut second = InlineTextPreview::new(Arc::clone(&data), Vec2::new(180.0, 72.0));
    assert!(Arc::ptr_eq(&first.data, &second.data));
    assert_ne!(first.id, second.id);

    let ctx = egui::Context::default();
    let palette = AppPalette::default();
    let mut rects = [egui::Rect::NOTHING; 2];
    let render_frame = |ctx: &egui::Context,
                        first: &mut InlineTextPreview,
                        second: &mut InlineTextPreview,
                        events: Vec<egui::Event>,
                        time: f64,
                        rects: &mut [egui::Rect; 2]| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(420.0, 180.0),
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
        )
    };
    let initial = render_frame(&ctx, &mut first, &mut second, Vec::new(), 0.0, &mut rects);
    assert!(initial.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text == "row-0000")
    }));
    let initial_first_host_rows: Vec<_> = initial
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text)
                if rects[0].contains(text.pos) && text.galley.job.text.starts_with("row-") =>
            {
                text.galley.job.text[4..].parse::<usize>().ok()
            }
            _ => None,
        })
        .collect();
    let initial_first_host_max = *initial_first_host_rows.iter().max().unwrap();

    render_frame(
        &ctx,
        &mut first,
        &mut second,
        vec![egui::Event::PointerMoved(rects[0].center())],
        0.1,
        &mut rects,
    );
    render_frame(
        &ctx,
        &mut first,
        &mut second,
        vec![
            egui::Event::PointerMoved(rects[0].center()),
            egui::Event::Scroll(Vec2::new(0.0, -260.0)),
        ],
        0.2,
        &mut rects,
    );
    let scrolled = render_frame(&ctx, &mut first, &mut second, Vec::new(), 0.3, &mut rects);
    assert!(first.scroll_offset > 0.0, "hovered host should scroll");
    assert_eq!(second.scroll_offset, 0.0, "other host should stay at top");
    assert!(scrolled.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::epaint::Shape::Text(text) if rects[1].contains(text.pos) && text.galley.job.text == "row-0000")
    }), "unhovered host should still show its first row");

    let row_height = ctx.fonts(|fonts| fonts.row_height(&FontId::monospace(MONO_FONT_SIZE)));
    let mut first_host_rows: Vec<_> = scrolled
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text)
                if rects[0].contains(text.pos) && text.galley.job.text.starts_with("row-") =>
            {
                Some((text.pos.y, text.galley.job.text.clone()))
            }
            _ => None,
        })
        .collect();
    assert!(
        first_host_rows.iter().any(|(_, label)| {
            label
                .strip_prefix("row-")
                .and_then(|index| index.parse::<usize>().ok())
                .is_some_and(|index| index > initial_first_host_max)
        }),
        "scrolled first host should render a row beyond its initial visible range: {first_host_rows:?}"
    );
    first_host_rows.sort_by(|left, right| left.0.total_cmp(&right.0));
    assert!(
        first_host_rows.len() >= 2,
        "scrolled host should show multiple rows"
    );
    let row_pitch = row_height + ctx.style().spacing.item_spacing.y;
    assert!(
        ((first_host_rows[1].0 - first_host_rows[0].0) - row_pitch).abs() < 1.0,
        "virtual row y positions must match measured row height: {first_host_rows:?}"
    );
    assert!(first.scroll_offset > 0.0);
    assert_eq!(second.scroll_offset, 0.0);
}

#[test]
fn inline_text_preview_skips_layout_when_host_is_clipped() {
    let data = TextPreviewData::from_text(Arc::new("visible only in source".repeat(500)));
    let mut preview = InlineTextPreview::new(data, Vec2::new(180.0, 100.0));
    let ctx = egui::Context::default();
    let palette = AppPalette::default();
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
    assert!(preview.scroll_offset.is_finite());
    assert_eq!(preview.scroll_offset, 0.0);
    assert!(
        output
            .shapes
            .iter()
            .all(|shape| { !matches!(&shape.shape, egui::epaint::Shape::Text(_)) })
    );
}

#[test]
fn external_editor_handoff_keeps_text_source_search_and_scroll_state() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("source.txt");
    std::fs::write(&path, "first line\nmatch line\nlast line\n").unwrap();
    let mut state = TextPreviewState::from_file(&path, &AppConfig::default()).unwrap();
    state.editor_cmd = "test-editor".to_owned();
    state.show_find = true;
    state.find_query = "match".to_owned();
    state.scroll_to_line = Some(2);
    let content = Arc::clone(&state.content);
    let mut launched_path = None;

    state.trigger_escape_hatch_with(|app, command, target| {
        assert_eq!(app, "test-editor");
        assert_eq!(command, "test-editor");
        launched_path = Some(target.to_path_buf());
        Ok("Launched test editor".to_owned())
    });

    assert_eq!(launched_path.as_deref(), Some(path.as_path()));
    assert!(Arc::ptr_eq(&state.content, &content));
    assert_eq!(state.path.as_deref(), Some(path.as_path()));
    assert!(state.show_find);
    assert_eq!(state.find_query, "match");
    assert_eq!(state.scroll_to_line, Some(2));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert_eq!(
        state.status_toast.message(now),
        Some("Launched test editor")
    );
}

#[test]
fn independent_text_views_share_immutable_data_and_keep_view_state_separate() {
    let original_content = Arc::new("fn main() {\n    println!(\"shared\");\n}\n".to_owned());
    let source = TextPreviewData::from_text(Arc::clone(&original_content));
    assert!(Arc::ptr_eq(&source.content, &original_content));
    let mut first = TextPreviewState::from_data(
        Arc::clone(&source),
        Some("First view".to_owned()),
        &AppConfig::default(),
    );
    let second = TextPreviewState::from_data(
        Arc::clone(&source),
        Some("Second view".to_owned()),
        &AppConfig::default(),
    );

    assert!(Arc::ptr_eq(&first.content, &second.content));
    assert!(Arc::ptr_eq(&first.code_fragments, &second.code_fragments));
    assert!(Arc::ptr_eq(
        &first.line_first_fragment,
        &second.line_first_fragment
    ));
    assert_eq!(first.language, "rs");
    assert_eq!(second.language, "rs");
    assert_ne!(first.title, second.title);

    first.show_find = true;
    first.find_query = "shared".to_owned();
    first.scroll_to_line = Some(2);
    first.code_selection = Some((0, 2));

    assert!(!second.show_find);
    assert!(second.find_query.is_empty());
    assert_eq!(second.scroll_to_line, None);
    assert_eq!(second.code_selection, None);
}

#[test]
fn inline_text_source_seeds_independent_full_studios_without_loading_again() {
    let source = TextPreviewData::from_text(Arc::new(
        "fn gallery() {\n    println!(\"source\");\n}\n".to_owned(),
    ));
    let inline = InlineTextPreview::new(Arc::clone(&source), Vec2::new(180.0, 72.0));
    let mut first = TextPreviewState::from_data(
        Arc::clone(&source),
        Some("First editor".to_owned()),
        &AppConfig::default(),
    );
    let second = TextPreviewState::from_data(
        Arc::clone(&source),
        Some("Second editor".to_owned()),
        &AppConfig::default(),
    );

    assert!(Arc::ptr_eq(&inline.data, &source));
    assert!(Arc::ptr_eq(&first.content, &source.content));
    assert!(Arc::ptr_eq(&second.content, &source.content));
    assert!(Arc::ptr_eq(&first.code_fragments, &source.code_fragments));
    assert!(!first.search_pending && first.search_job.is_none());
    assert!(!second.search_pending && second.search_job.is_none());

    first.show_find = true;
    first.find_query = "source".to_owned();
    first.scroll_to_line = Some(2);
    first.code_selection = Some((0, 3));
    assert!(!second.show_find);
    assert!(second.find_query.is_empty());
    assert_eq!(second.scroll_to_line, None);
    assert_eq!(second.code_selection, None);
}
