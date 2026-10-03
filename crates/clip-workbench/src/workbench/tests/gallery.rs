use super::*;
#[test]
fn contextual_palette_preview_actions_use_neutral_secondary_surface() {
    for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
        let palette = ui_appearance::palette(appearance);
        let expected_fill = hex_to_c32(&palette.colors.surface_panel);
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(contextual_palette_action_button(
                    Some(Icon::Pdf),
                    "Edit PDF",
                    &palette,
                ));
            });
        });

        let mut saw_fill = false;
        let mut saw_label = false;
        let mut pending: Vec<_> = output.shapes.iter().map(|shape| &shape.shape).collect();
        let mut surface_colors = Vec::new();
        let mut label_colors = Vec::new();
        while let Some(shape) = pending.pop() {
            match shape {
                egui::Shape::Vec(shapes) => pending.extend(shapes),
                egui::Shape::Mesh(mesh)
                    if mesh
                        .vertices
                        .iter()
                        .any(|vertex| vertex.color == expected_fill) =>
                {
                    saw_fill = true;
                    surface_colors.extend(mesh.vertices.iter().map(|vertex| vertex.color));
                }
                egui::Shape::Text(text) if text.galley.text() == "Edit PDF" => {
                    saw_label = true;
                    for section in &text.galley.job.sections {
                        label_colors.push(section.format.color);
                    }
                }
                _ => {}
            }
        }
        for surface in surface_colors {
            assert_eq!(
                (surface.r(), surface.r()),
                (surface.g(), surface.b()),
                "contextual action should remain neutral"
            );
            for foreground in &label_colors {
                assert!(
                    little_clip_core::palette::contrast_ratio(
                        (surface.r(), surface.g(), surface.b()),
                        (foreground.r(), foreground.g(), foreground.b()),
                    ) >= 4.5,
                    "{appearance:?} contextual action label lacks gradient contrast"
                );
            }
        }
        assert!(saw_fill, "{appearance:?} action should use panel fill");
        assert!(saw_label, "{appearance:?} action label should render");
    }
}

#[test]
fn gallery_requests_map_to_each_shared_source_full_viewer() {
    let config = AppConfig::default();
    for kind in [
        component_gallery::GalleryPreviewKind::Image,
        component_gallery::GalleryPreviewKind::Text,
        component_gallery::GalleryPreviewKind::Spreadsheet,
        component_gallery::GalleryPreviewKind::Midi,
        component_gallery::GalleryPreviewKind::Model3d,
        component_gallery::GalleryPreviewKind::Audio,
        component_gallery::GalleryPreviewKind::Pdf,
    ] {
        let mut gallery = component_gallery::ComponentGalleryState::default();
        gallery.request_open(kind);
        let request = gallery
            .take_pending_open()
            .expect("gallery action should produce one pending source request");
        let viewer = gallery_request_to_viewer_session(request, &config);
        match (kind, viewer) {
            (component_gallery::GalleryPreviewKind::Image, ViewerSession::Image(state)) => {
                assert_eq!(state.title, "Gallery image");
                assert_eq!(state.original_rgba.width(), 24);
                assert!(state.palette_ticket.is_none());
            }
            (component_gallery::GalleryPreviewKind::Text, ViewerSession::Text(state)) => {
                assert_eq!(state.title, "Gallery text");
                assert!(state.content.contains("gallery-line-alpha"));
            }
            (
                component_gallery::GalleryPreviewKind::Spreadsheet,
                ViewerSession::Spreadsheet(state),
            ) => {
                assert_eq!(state.title, "Gallery workbook");
                assert_eq!(
                    state.current_sheet().unwrap().rows[0][0],
                    "gallery-cell-alpha"
                );
            }
            (component_gallery::GalleryPreviewKind::Midi, ViewerSession::Midi(state)) => {
                assert_eq!(state.title, "Gallery MIDI");
                assert_eq!(state.data.total_notes, 4);
                assert!(!state.is_synthesizing);
            }
            (component_gallery::GalleryPreviewKind::Model3d, ViewerSession::Model3d(state)) => {
                assert_eq!(state.status, model3d_view::ModelLoadingStatus::Ready);
                assert_eq!(state.mesh.as_ref().unwrap().name, "gallery-model.obj");
                assert!(state.rx.is_none());
            }
            (component_gallery::GalleryPreviewKind::Audio, ViewerSession::Audio(state)) => {
                assert_eq!(state.title, "Gallery audio");
                assert!(!state.is_loading_analysis);
                assert!(state.analysis_rx.is_none());
                assert_eq!(state.completed_source_analysis().unwrap().duration_ms, 1000);
            }
            (component_gallery::GalleryPreviewKind::Pdf, ViewerSession::Pdf(state)) => {
                assert_eq!(state.title, "gallery.pdf");
                assert_eq!(state.page_count, 1);
                assert!(state.pages[0].raster.is_some());
                assert!(state.path.is_file());
            }
            _ => panic!("gallery request mapped to the wrong full viewer variant"),
        }
        assert!(gallery.take_pending_open().is_none());
    }
}

#[test]
fn visible_gallery_actions_open_full_studios() {
    for (label, kind) in [
        (
            "Open Image Studio",
            component_gallery::GalleryPreviewKind::Image,
        ),
        (
            "Open Text Studio",
            component_gallery::GalleryPreviewKind::Text,
        ),
        (
            "Open Spreadsheet Studio",
            component_gallery::GalleryPreviewKind::Spreadsheet,
        ),
        (
            "Open MIDI Studio",
            component_gallery::GalleryPreviewKind::Midi,
        ),
        (
            "Open Model3D Studio",
            component_gallery::GalleryPreviewKind::Model3d,
        ),
        (
            "Open Audio Studio",
            component_gallery::GalleryPreviewKind::Audio,
        ),
        (
            "Open PDF Editor",
            component_gallery::GalleryPreviewKind::Pdf,
        ),
    ] {
        let viewer =
            ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
        let mut app = test_workbench_app(Some(viewer), VecDeque::new());
        let ctx = egui::Context::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(2240.0, 1280.0),
            )),
            events,
            ..Default::default()
        };
        let mut first_frame = ctx.run(input(Vec::new()), |ctx| {
            assert!(app.render_modal_viewers(ctx));
        });
        for _ in 0..12 {
            let action_is_visible = first_frame.shapes.iter().any(|shape| {
                matches!(
                    &shape.shape,
                    egui::Shape::Text(text) if text.galley.job.text == label
                )
            });
            if action_is_visible {
                break;
            }
            first_frame = ctx.run(
                input(vec![
                    egui::Event::PointerMoved(egui::pos2(640.0, 540.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -500.0),
                        modifiers: egui::Modifiers::NONE,
                    },
                ]),
                |ctx| assert!(app.render_modal_viewers(ctx)),
            );
        }
        let action_position = first_frame
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{label} should be visible in the gallery"));
        let pointer_event = |pressed| egui::Event::PointerButton {
            pos: action_position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(action_position),
                pointer_event(true),
            ]),
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(action_position),
                pointer_event(false),
            ]),
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );
        let opened_expected_studio = match (kind, app.viewer.as_ref()) {
            (component_gallery::GalleryPreviewKind::Image, Some(ViewerSession::Image(_)))
            | (component_gallery::GalleryPreviewKind::Text, Some(ViewerSession::Text(_)))
            | (
                component_gallery::GalleryPreviewKind::Spreadsheet,
                Some(ViewerSession::Spreadsheet(_)),
            )
            | (component_gallery::GalleryPreviewKind::Midi, Some(ViewerSession::Midi(_)))
            | (component_gallery::GalleryPreviewKind::Model3d, Some(ViewerSession::Model3d(_))) => {
                true
            }
            (component_gallery::GalleryPreviewKind::Audio, Some(ViewerSession::Audio(state))) => {
                !state.is_loading_analysis
                    && state.analysis_rx.is_none()
                    && state
                        .completed_source_analysis()
                        .is_some_and(|source| source.duration_ms == 1000)
            }
            (component_gallery::GalleryPreviewKind::Pdf, Some(ViewerSession::Pdf(state))) => {
                state.page_count == 1 && state.path.is_file() && state.pages[0].raster.is_some()
            }
            _ => false,
        };
        assert!(opened_expected_studio, "{label} opened the wrong studio");
    }
}

#[test]
fn unit_preview_copy_button_copies_exact_primary_conversion() {
    assert_eq!(
        click_text_smart_copy_action("12 in", "12 in ➔ 30.5 cm (0.30 m)"),
        "30.5 cm"
    );
}

#[test]
fn network_preview_copy_button_copies_exact_mac_summary() {
    assert_eq!(
        click_text_smart_copy_action(
            "02:00:00:00:00:01",
            "02:00:00:00:00:01 (Randomized / Private)",
        ),
        "MAC: 02:00:00:00:00:01\nVendor: Randomized / Private\nType: Locally Administered (Private/Randomized MAC)"
    );
}

#[test]
fn timestamp_preview_copy_button_copies_exact_iso_8601_value() {
    let timestamp = detect_timestamp("1700000000").expect("epoch timestamp");
    let button_label = format!("{} ({})", timestamp.human_readable, timestamp.relative);
    assert_eq!(
        click_text_smart_copy_action("1700000000", &button_label),
        "2023-11-14T22:13:20+00:00"
    );
}

#[test]
fn inspector_markdown_classification_tracks_clipboard_item_identity() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let slot = SlotId::LeftHand;
    let markdown = ClipItem::from_text("# A heading\n\n- One item");
    let markdown_id = markdown.id;
    *Arc::make_mut(&mut app.state)
        .get_mut(slot)
        .expect("left hand slot") = Some(markdown);

    assert_eq!(app.check_inspector_text_preview(), Some((slot, None, true)));
    assert_eq!(
        app.inspector_text_preview_cache
            .as_ref()
            .and_then(|cache| cache.selected_item_ids.first().copied().flatten()),
        Some(markdown_id)
    );

    let plain_text = ClipItem::from_text("ordinary clipboard text");
    let plain_text_id = plain_text.id;
    *Arc::make_mut(&mut app.state)
        .get_mut(slot)
        .expect("left hand slot") = Some(plain_text);

    assert_eq!(
        app.check_inspector_text_preview(),
        Some((slot, None, false))
    );
    assert_eq!(
        app.inspector_text_preview_cache
            .as_ref()
            .and_then(|cache| cache.selected_item_ids.first().copied().flatten()),
        Some(plain_text_id)
    );
}

#[test]
fn inspector_caches_clipboard_spreadsheet_detection_until_item_changes() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let slot = SlotId::LeftHand;
    *Arc::make_mut(&mut app.state).get_mut(slot).unwrap() =
        Some(ClipItem::from_text("name,count\nalpha,1"));

    assert_eq!(
        app.check_inspector_text_preview(),
        Some((slot, None, false))
    );
    assert_eq!(
        app.inspector_text_preview_cache
            .as_ref()
            .and_then(|cache| cache.spreadsheet_slot),
        Some(slot)
    );
    assert_eq!(
        app.check_inspector_text_preview(),
        Some((slot, None, false))
    );

    *Arc::make_mut(&mut app.state).get_mut(slot).unwrap() = Some(ClipItem::from_text("plain text"));
    assert_eq!(
        app.check_inspector_text_preview(),
        Some((slot, None, false))
    );
    assert_eq!(
        app.inspector_text_preview_cache
            .as_ref()
            .and_then(|cache| cache.spreadsheet_slot),
        None
    );
}

#[test]
fn oversized_markdown_uses_bounded_inspector_hint_and_full_worker_classification() {
    let content = format!("# Heading\n\n{}", "- a list item\n".repeat(20_000));
    let expected = content.clone();
    let mut item = ClipItem::from_text("placeholder");
    item.payload = ClipPayload::Text(content);
    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .unwrap() = Some(item.clone());

    let (_, path, quick_hint) = app.check_inspector_text_preview().unwrap();
    assert!(path.is_none());
    assert!(
        !quick_hint,
        "large Text payload should not be scanned inline"
    );

    app.open_slot_preview_modal(&item);
    wait_for_text_load(&mut app);
    let Some(ViewerSession::Text(state)) = &app.viewer else {
        panic!("missing text viewer")
    };
    assert_eq!(
        state.view_mode,
        text_preview::TextViewMode::RenderedMarkdown
    );
    assert_eq!(state.content.as_str(), expected);
}

#[test]
fn closing_startup_viewers_advances_fifo_then_returns_to_selected_workbench_view() {
    let gallery =
        || ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
    let mut app = test_workbench_app(
        Some(gallery()),
        VecDeque::from([ViewerSession::Spreadsheet(SpreadsheetViewerState::new(
            SpreadsheetData {
                title: "Queued spreadsheet".to_owned(),
                sheets: Vec::new(),
            },
            None,
        ))]),
    );
    app.view = WorkbenchView::NodeMachine;
    let ctx = egui::Context::default();
    let input = |events| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 700.0),
        )),
        events,
        ..Default::default()
    };
    let escape = || egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };

    let _ = ctx.run(input(vec![escape()]), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
    assert!(app.startup_viewers.is_empty());

    let _ = ctx.run(input(Vec::new()), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));

    let _ = ctx.run(input(vec![escape()]), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(app.viewer.is_none());
    assert_eq!(app.view, WorkbenchView::NodeMachine);
}

#[test]
fn midi_escape_after_region_clear_returns_to_source_view_and_preserves_slots() {
    let data = MidiData {
        title: "Synthetic MIDI".to_owned(),
        track_count: 1,
        track_names: vec!["Track".to_owned()],
        total_notes: 0,
        duration_sec: 4.0,
        bpm: 120.0,
        min_key: 60,
        max_key: 60,
        notes: Vec::new(),
    };
    let mut midi = MidiViewerState::from_parsed_file(PathBuf::from("synthetic.mid"), data);
    midi.selected_region = Some((0.5, 2.5));
    midi.play_stop_time = Some(2.5);
    let mut app = test_workbench_app(Some(ViewerSession::Midi(midi)), VecDeque::new());
    app.view = WorkbenchView::NodeMachine;
    app.selected_slots = vec![SlotId::RightHand, SlotId::LeftPrior1];
    let original_slots = app.selected_slots.clone();

    let ctx = egui::Context::default();
    let input = |events| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 700.0),
        )),
        events,
        ..Default::default()
    };
    let escape = || egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };

    let _ = ctx.run(input(vec![escape()]), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    let Some(ViewerSession::Midi(state)) = &app.viewer else {
        panic!("first Escape should keep MIDI viewer open");
    };
    assert!(state.selected_region.is_none());
    assert!(state.play_stop_time.is_none());
    assert_eq!(app.view, WorkbenchView::NodeMachine);
    assert_eq!(app.selected_slots, original_slots);

    let _ = ctx.run(input(vec![escape()]), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(
        app.viewer.is_none(),
        "second Escape should close MIDI viewer"
    );
    assert_eq!(app.view, WorkbenchView::NodeMachine);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn startup_pdf_queue_precedes_background_midi_entry() {
    let queued_pdf = ViewerSession::Pdf(PdfEditorState::new(PathBuf::from("queued.pdf")));
    let mut app = test_workbench_app(None, VecDeque::from([queued_pdf]));
    app.startup_midi = Some(PathBuf::from("dense.mid"));

    app.advance_startup();
    assert!(matches!(app.viewer, Some(ViewerSession::Pdf(_))));
    assert!(app.startup_midi.is_some());
    assert!(app.midi_load_job.is_none());

    app.viewer = None;
    app.advance_startup();
    assert!(app.viewer.is_none());
    assert!(app.startup_midi.is_none());
    assert!(app.midi_load_job.is_some());
}

#[test]
fn closing_dirty_pdf_requests_workbench_mirror_redraw() {
    let mut pdf = PdfEditorState::new(PathBuf::from("edited.pdf"));
    pdf.dirty = true;
    let mut app = test_workbench_app(Some(ViewerSession::Pdf(pdf)), VecDeque::new());
    let ctx = egui::Context::default();
    let escape = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![escape],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(app.viewer.is_none());
    assert!(
        app.dirty_mirror,
        "closing an edited PDF must invalidate the mirror"
    );
    wait_for_mirror(&mut app);
    assert!(
        !app.dirty_mirror,
        "the next mirror render should consume invalidation"
    );
    assert!(app.mirror_texture.is_some());
}
