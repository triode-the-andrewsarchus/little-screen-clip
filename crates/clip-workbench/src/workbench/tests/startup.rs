use super::*;

#[test]
fn image_load_queue_full_reports_status_and_accepts_later_completion() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let (_ticket_jobs, ticket) = isolated_job_ticket();
    app.image_load_job = Some(ImageLoadJob {
        key: None,
        ticket,
        receiver: synthetic_job_receiver(Err(JobError::QueueFull)),
    });

    app.poll_image_load(&egui::Context::default());

    assert!(app.image_load_job.is_none());
    assert!(
        app.status_msg
            .as_deref()
            .is_some_and(|message| message.contains("background job queue is full"))
    );

    app.start_image_load(ImageLoadSource::Bytes {
        bytes: synthetic_image_bytes(),
        mime: "image/png".to_owned(),
        path: None,
        title: "synthetic image".to_owned(),
    });
    wait_for_image_load(&mut app);

    let Some(ViewerSession::Image(state)) = &app.viewer else {
        panic!("image load should recover after queue saturation")
    };
    assert_eq!(state.current_rgba.dimensions(), (1, 1));
    assert!(app.status_msg.is_none());
}

#[test]
fn text_load_queue_full_reports_status_and_accepts_later_completion() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let (_ticket_jobs, ticket) = isolated_job_ticket();
    app.text_load_job = Some(TextPreviewLoadJob {
        ticket,
        receiver: synthetic_job_receiver(Err(JobError::QueueFull)),
    });

    app.poll_text_load(&egui::Context::default());

    assert!(app.text_load_job.is_none());
    assert!(
        app.status_msg
            .as_deref()
            .is_some_and(|message| message.contains("background job queue is full"))
    );

    app.start_text_preview_load("synthetic text".to_owned(), Some("queue retry".to_owned()));
    wait_for_text_load(&mut app);

    let Some(ViewerSession::Text(state)) = &app.viewer else {
        panic!("text load should recover after queue saturation")
    };
    assert!(state.content.contains("synthetic text"));
    assert!(app.status_msg.is_none());
}

#[test]
fn spreadsheet_load_queue_full_reports_status_and_accepts_later_completion() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let (_ticket_jobs, ticket) = isolated_job_ticket();
    app.spreadsheet_load_job = Some((
        ticket,
        synthetic_job_receiver(Err(JobError::QueueFull)),
        None,
    ));

    app.poll_spreadsheet_load(&egui::Context::default());

    assert!(app.spreadsheet_load_job.is_none());
    assert!(
        app.status_msg
            .as_deref()
            .is_some_and(|message| message.contains("background job queue is full"))
    );

    app.start_spreadsheet_load(SpreadsheetLoadSource::Text {
        content: Arc::new("name,value\nclip,1\nnext,2".to_owned()),
        title: "synthetic table".to_owned(),
    });
    wait_for_text_load(&mut app);

    assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
    assert!(app.status_msg.is_none());
}

#[test]
fn midi_load_queue_full_reports_status_and_accepts_later_completion() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let (_ticket_jobs, ticket) = isolated_job_ticket();
    app.midi_load_job = Some((ticket, synthetic_job_receiver(Err(JobError::QueueFull))));

    app.poll_midi_load(&egui::Context::default());

    assert!(app.midi_load_job.is_none());
    assert!(
        app.status_msg
            .as_deref()
            .is_some_and(|message| message.contains("background job queue is full"))
    );

    let temp_dir = tempfile::tempdir().unwrap();
    let midi_path = temp_dir.path().join("synthetic.mid");
    let mut midi_bytes = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x0c".to_vec();
    midi_bytes.extend_from_slice(&[
        0x00, 0x90, 0x3c, 0x40, // note on
        0x60, 0x80, 0x3c, 0x00, // note off
        0x00, 0xff, 0x2f, 0x00, // end track
    ]);
    std::fs::write(&midi_path, midi_bytes).unwrap();
    app.start_midi_load(midi_path);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.midi_load_job.is_some() {
        app.poll_midi_load(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "MIDI load did not recover after queue saturation"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    let Some(ViewerSession::Midi(state)) = &app.viewer else {
        panic!("MIDI load should recover after queue saturation")
    };
    assert_eq!(state.data.title, "synthetic");
    assert!(app.status_msg.is_none());
}

#[test]
fn replaced_image_request_and_replaced_viewer_discard_stale_completion() {
    let temp_dir = tempfile::tempdir().unwrap();
    let first = temp_dir.path().join("first.png");
    let second = temp_dir.path().join("second.png");
    write_test_image(&first, 3);
    write_test_image(&second, 5);
    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());
    app.open_slot_file_preview(&first, true);
    app.open_slot_file_preview(&second, true);
    wait_for_image_load(&mut app);
    let Some(ViewerSession::Image(state)) = &app.viewer else {
        panic!("missing image viewer")
    };
    assert_eq!(state.current_rgba.width(), 5);

    app.open_slot_file_preview(&first, true);
    app.replace_viewer(Some(gallery_viewer()));
    assert!(app.image_load_job.is_none());
    app.poll_image_load(&egui::Context::default());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
}

#[test]
fn replacing_startup_image_request_cancels_its_placeholder() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("image.png");
    write_test_image(&path, 3);
    let loading = ViewerSession::ImageLoading(ImageLoadJob::new(ImageLoadSource::File(path)));
    let mut app = test_workbench_app(Some(loading), VecDeque::new());
    app.start_image_load(ImageLoadSource::File(temp_dir.path().join("missing.png")));
    assert!(app.viewer.is_none());
    wait_for_image_load(&mut app);
    app.poll_image_load(&egui::Context::default());
    assert!(app.viewer.is_none());
    assert!(app.status_msg.as_deref().is_some_and(|message| {
        message.starts_with("Image preview failed: Failed to read file")
    }));
}

#[test]
fn closing_viewer_cancels_pending_image_replacement() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("image.png");
    write_test_image(&path, 3);
    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());
    app.open_slot_file_preview(&path, true);
    let ctx = egui::Context::default();
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
        |ctx| {
            app.render_modal_viewers(ctx);
        },
    );
    assert!(app.viewer.is_none());
    assert!(app.image_load_job.is_none());
    app.poll_image_load(&ctx);
    assert!(app.viewer.is_none());
}

#[test]
fn startup_image_loading_keeps_fifo_position_and_waits_for_foreground() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("image.png");
    write_test_image(&path, 4);
    let loading = ViewerSession::ImageLoading(ImageLoadJob::new(ImageLoadSource::File(path)));
    let mut app = test_workbench_app(
        Some(gallery_viewer()),
        VecDeque::from([loading, gallery_viewer()]),
    );
    app.poll_image_load(&egui::Context::default());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    assert_eq!(app.startup_viewers.len(), 2);
    app.viewer = app.startup_viewers.pop_front();
    wait_for_image_load(&mut app);
    assert!(matches!(app.viewer, Some(ViewerSession::Image(_))));
    assert_eq!(app.startup_viewers.len(), 1);
}

#[test]
fn palette_prompt_escape_cancels_only_the_overlay() {
    let viewer =
        ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
    let mut app = test_workbench_app(Some(viewer), VecDeque::new());
    app.search_query = "preserve this Palette query".to_owned();
    app.selected_action_index = 3;
    app.selected_slots = vec![SlotId::RightHand, SlotId::LeftPrior1];
    let original_slots = app.selected_slots.clone();
    app.in_app_prompt = Some(InAppPromptState {
        recipe_name: "Prompt Escape Fixture".to_owned(),
        message: "Enter test text".to_owned(),
        input_text: "unchanged input".to_owned(),
        first_frame: true,
    });
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

    let _ = ctx.run(input(Vec::new()), |ctx| {
        assert!(app.render_modal_viewers(ctx));
        assert!(
            ctx.wants_keyboard_input(),
            "prompt editor should own keyboard focus"
        );
    });
    assert!(app.in_app_prompt.is_some());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));

    let first_escape_output = ctx.run(input(vec![escape()]), |ctx| {
        let modal_active = app.render_modal_viewers(ctx);
        assert!(modal_active, "the prompt owns the Escape frame");
        if !modal_active {
            app.handle_global_shortcuts(ctx);
        }
    });
    let close_commands = first_escape_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|output| {
            output
                .commands
                .iter()
                .filter(|command| matches!(command, egui::ViewportCommand::Close))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(close_commands, 0, "Escape cancels only the prompt overlay");
    assert!(app.in_app_prompt.is_none());
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.search_query, "preserve this Palette query");
    assert_eq!(app.selected_action_index, 3);
    assert_eq!(app.selected_slots, original_slots);

    let second_escape_output = ctx.run(input(vec![escape()]), |ctx| {
        let modal_active = app.render_modal_viewers(ctx);
        assert!(
            modal_active,
            "the gallery viewer owns the second Escape frame"
        );
        if !modal_active {
            app.handle_global_shortcuts(ctx);
        }
    });
    let close_commands = second_escape_output
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|output| {
            output
                .commands
                .iter()
                .filter(|command| matches!(command, egui::ViewportCommand::Close))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(close_commands, 0, "Escape closes the viewer, not the popup");
    assert!(app.in_app_prompt.is_none());
    assert!(app.viewer.is_none());
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn quick_palette_enter_executes_prompt_recipe_while_search_is_focused() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let recipe_name = "Enter Prompt Fixture";
    let node = little_clip_ops::pipeline::types::NodeInstance {
        id: uuid::Uuid::new_v4(),
        kind: NodeKind::PromptOrHandText,
        title: "Prompt".to_owned(),
        pos: [0.0, 0.0],
        props: [
            ("hand".to_owned(), "right".to_owned()),
            ("prompt_message".to_owned(), "Enter test text".to_owned()),
        ]
        .into_iter()
        .collect(),
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    app.recipes.push(Recipe {
        name: recipe_name.to_owned(),
        description: String::new(),
        input_type: little_clip_ops::pipeline::types::RecipeInputType::None,
        tags: Vec::new(),
        graph: little_clip_ops::pipeline::types::NodeGraph {
            nodes: vec![node],
            ..Default::default()
        },
    });
    app.search_query = recipe_name.to_owned();

    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.add(egui::TextEdit::singleline(&mut app.search_query));
                response.request_focus();
                assert!(ctx.wants_keyboard_input());
                app.handle_palette_nav_and_execution(ctx);
            });
        },
    );

    let prompt = app
        .in_app_prompt
        .as_ref()
        .expect("Enter executes the selected recipe");
    assert_eq!(prompt.recipe_name, recipe_name);
    assert_eq!(prompt.message, "Enter test text");
}

#[test]
fn quick_palette_focused_search_receives_ordinary_text_without_changing_selection() {
    let mut app = test_workbench_app(None, VecDeque::new());
    app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
    let original_slots = app.selected_slots.clone();
    let ctx = egui::Context::default();

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.search_query))
                    .request_focus();
            });
        },
    );

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Text("q".to_owned())],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.search_query));
                assert!(ctx.wants_keyboard_input());
                app.handle_global_shortcuts(ctx);
            });
        },
    );

    assert_eq!(app.search_query, "q");
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn in_app_prompt_enter_submits_from_focused_editor_once() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let recipe_name = "Prompt Enter Fixture";
    app.in_app_prompt = Some(InAppPromptState {
        recipe_name: recipe_name.to_owned(),
        message: "Enter test text".to_owned(),
        input_text: "submitted value".to_owned(),
        first_frame: true,
    });
    let ctx = egui::Context::default();
    let sent = std::cell::RefCell::new(Vec::new());
    let frame = |app: &mut WorkbenchApp, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                assert!(app.render_prompt_modal_with(ctx, |request| {
                    sent.borrow_mut().push(request.clone());
                    Ok(IpcResponse::Success {
                        message: "fixture complete".to_owned(),
                    })
                }));
            },
        )
    };

    frame(&mut app, Vec::new());
    assert!(
        ctx.wants_keyboard_input(),
        "prompt editor should receive focus"
    );
    assert!(
        app.in_app_prompt.is_some(),
        "opening the prompt must not close it"
    );

    let submitted = frame(
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let close_commands = submitted
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|output| {
            output
                .commands
                .iter()
                .filter(|command| matches!(command, egui::ViewportCommand::Close))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(
        close_commands, 1,
        "Enter submits and requests one window close"
    );
    assert!(
        matches!(
            sent.borrow().as_slice(),
            [IpcRequest::ExecuteRecipeWithPrompt {
                recipe_name: sent_recipe,
                prompt_text: Some(text),
            }] if sent_recipe == recipe_name && text == "submitted value"
        ),
        "Enter must send the captured prompt exactly once"
    );
    assert!(
        app.in_app_prompt.is_some(),
        "submission should close the popup through its viewport command, not also dismiss prompt state"
    );
    assert_eq!(
        app.view,
        WorkbenchView::Palette,
        "successful prompt submission must retain its Palette route"
    );
}

#[test]
fn shared_workbench_status_is_visible_in_every_host_view() {
    let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
    for view in [
        WorkbenchView::Palette,
        WorkbenchView::NodeMachine,
        WorkbenchView::AutoActions,
    ] {
        let ctx = egui::Context::default();
        let mut status_height = 0.0;
        let mut hint_height = 0.0;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    status_height = show_workbench_footer(
                        ui,
                        view,
                        Some("Image preview failed: invalid PNG header"),
                        &palette,
                    )
                    .height();
                });
            },
        );
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                hint_height = show_workbench_footer(ui, view, None, &palette).height();
            });
        });
        assert!(
            status_height > hint_height,
            "status was not reserved in {view:?}"
        );
    }
}

#[test]
fn palette_search_focus_yields_to_audio_and_returns_on_activation() {
    fn render_field(
        ctx: &egui::Context,
        value: &mut String,
        needs_focus: &mut bool,
        audio_active: bool,
        activate_from_audio: bool,
    ) -> egui::Id {
        let mut field_id = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 180.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut field = egui::TextEdit::singleline(value);
                    if audio_active {
                        field = field.interactive(false);
                    }
                    let response = ui.add(field.desired_width(300.0));
                    field_id = Some(response.id);
                    update_palette_search_focus(
                        ctx,
                        &response,
                        needs_focus,
                        audio_active,
                        activate_from_audio,
                    );
                });
            },
        );
        field_id.unwrap()
    }

    let ctx = egui::Context::default();
    ui_ext::configure_workbench_style(&ctx);
    let mut value = String::new();
    let mut needs_focus = true;
    let id = render_field(&ctx, &mut value, &mut needs_focus, false, false);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));

    render_field(&ctx, &mut value, &mut needs_focus, true, false);
    assert_eq!(ctx.memory(|memory| memory.focused()), None);

    render_field(&ctx, &mut value, &mut needs_focus, true, true);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));

    render_field(&ctx, &mut value, &mut needs_focus, false, false);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
}

#[test]
fn compact_palette_footer_reserves_wrapped_status_above_scrollable_results() {
    let ctx = egui::Context::default();
    ui_ext::configure_workbench_style(&ctx);
    let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
    let status = "Action failed: a long diagnostic message that wraps across the compact footer while preserving the palette hint and the full error status.";
    let mut remaining_height = 0.0;
    let mut footer_rect = egui::Rect::NOTHING;
    let mut body_rect = egui::Rect::NOTHING;
    let mut results_content_height = 0.0;
    let mut results_viewport_height = 0.0;

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1177.0, 662.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.allocate_space(egui::vec2(ui.available_width(), 480.0));
                body_rect = ui.available_rect_before_wrap();
                footer_rect =
                    show_workbench_footer(ui, WorkbenchView::Palette, Some(status), &palette);
                remaining_height = ui.available_height();
                let results = egui::ScrollArea::vertical()
                    .max_height(ui.available_height().max(0.0))
                    .show(ui, |ui| {
                        for result in 0..40 {
                            ui.label(format!("Result {result}: representative operation"));
                        }
                    });
                results_content_height = results.content_size.y;
                results_viewport_height = results.inner_rect.height();
            });
        },
    );

    assert!(remaining_height > 0.0);
    assert!(footer_rect.min.y > 0.0);
    assert!(
        footer_rect.max.y <= body_rect.max.y,
        "footer {footer_rect:?} exceeds body {body_rect:?}"
    );
    assert!(results_viewport_height > 0.0);
    assert!(results_content_height > results_viewport_height);
}

#[test]
fn startup_initial_text_file_opens_text_viewer() {
    let temp_dir = tempfile::tempdir().unwrap();
    let text_path = temp_dir.path().join("sample.txt");
    std::fs::write(&text_path, "Hello from startup text").unwrap();

    let app = WorkbenchApp::new(
        egui::Context::default(),
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(text_path),
        false,
        false,
        false,
    );

    let Some(ViewerSession::Text(state)) = &app.viewer else {
        panic!("expected Text viewer session");
    };
    assert_eq!(state.content.as_str(), "Hello from startup text");
    assert!(app.status_msg.is_none());
}

#[test]
fn startup_initial_sheet_file_opens_spreadsheet_viewer() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sheet_path = temp_dir.path().join("sample.csv");
    std::fs::write(&sheet_path, "col1,col2\nval1,val2\n").unwrap();

    let app = WorkbenchApp::new(
        egui::Context::default(),
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        Some(sheet_path),
        None,
        false,
        false,
        false,
    );

    let Some(ViewerSession::Spreadsheet(state)) = &app.viewer else {
        panic!("expected Spreadsheet viewer session");
    };
    assert_eq!(state.data.sheets.len(), 1);
    assert_eq!(state.data.sheets[0].rows.len(), 2);
    assert!(app.status_msg.is_none());
}

#[test]
fn startup_view_options_select_node_machine_and_auto_actions() {
    let node_app = WorkbenchApp::new(
        egui::Context::default(),
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        true,
        false,
    );
    assert_eq!(node_app.view, WorkbenchView::NodeMachine);

    let actions_app = WorkbenchApp::new(
        egui::Context::default(),
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        false,
        true,
    );
    assert_eq!(actions_app.view, WorkbenchView::AutoActions);
}
