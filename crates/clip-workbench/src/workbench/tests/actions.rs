use super::*;

#[test]
fn idle_pdf_escape_returns_to_source_route_and_preserves_slots() {
    let temp_dir = tempfile::tempdir().expect("private PDF fixture directory");
    let path = temp_dir.path().join("missing-idle.pdf");
    assert!(!path.exists());
    let pdf = PdfEditorState::new(path.clone());
    let mut app = test_workbench_app(Some(ViewerSession::Pdf(pdf)), VecDeque::new());
    app.view = WorkbenchView::Palette;
    app.selected_slots = vec![SlotId::RightHand, SlotId::LeftPrior1];
    let original_slots = app.selected_slots.clone();
    let Some(ViewerSession::Pdf(state)) = &app.viewer else {
        panic!("PDF viewer should be active before Escape");
    };
    assert_eq!(state.path, path, "active PDF path belongs to this session");

    let ctx = egui::Context::default();
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(
        app.viewer.is_none(),
        "Escape should close only the PDF viewer"
    );
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.selected_slots, original_slots);
    let close_commands = output
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
    assert_eq!(close_commands, 0, "viewer Escape must keep popup open");
}

#[test]
fn palette_text_diff_opens_and_escape_returns_to_palette() {
    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .expect("left slot") = Some(ClipItem::from_text("left text\n"));
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::RightHand)
        .expect("right slot") = Some(ClipItem::from_text("right text\n"));
    app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
    let original_slots = app.selected_slots.clone();
    app.search_query = "diff".to_owned();
    let diff_action = ready_palette_actions(&mut app)
        .iter()
        .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "diff"))
        .expect("built-in text Diff action");
    app.selected_action_index = diff_action;

    app.execute_current_action(&egui::Context::default());

    assert!(matches!(app.viewer, Some(ViewerSession::Diff(_))));
    assert_eq!(app.view, WorkbenchView::Palette);

    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(app.viewer.is_none());
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn palette_image_diff_opens_and_escape_returns_to_palette() {
    let png_item = |color: [u8; 4], title: &str| {
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode tiny PNG fixture");
        ClipItem {
            id: uuid::Uuid::new_v4(),
            created_at: 0,
            payload: ClipPayload::Image {
                bytes: bytes.into_inner(),
                path: None,
                width: 2,
                height: 2,
                mime: "image/png".to_owned(),
                thumbnail: None,
            },
            title: title.to_owned(),
            subtitle: "PNG".to_owned(),
            details: vec!["PNG".to_owned()],
            handler_tag: "IMAGE".to_owned(),
        }
    };

    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .expect("left slot") = Some(png_item([255, 0, 0, 255], "left.png"));
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::RightHand)
        .expect("right slot") = Some(png_item([0, 0, 255, 255], "right.png"));
    app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
    let original_slots = app.selected_slots.clone();
    app.search_query = "diff".to_owned();
    let diff_action = ready_palette_actions(&mut app)
        .iter()
        .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "diff"))
        .expect("built-in image Diff action");
    app.selected_action_index = diff_action;

    app.execute_current_action(&egui::Context::default());

    assert!(matches!(
        app.viewer,
        Some(ViewerSession::Diff(ref state)) if state.is_image()
    ));
    assert_eq!(app.view, WorkbenchView::Palette);

    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(app.viewer.is_none());
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn palette_binary_diff_opens_and_escape_returns_to_palette() {
    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .expect("left slot") = Some(ClipItem::from_text("left binary fixture\n"));
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::RightHand)
        .expect("right slot") = Some(ClipItem::from_text("right binary fixture\n"));
    app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
    let original_slots = app.selected_slots.clone();
    app.search_query = "binary-diff".to_owned();
    let binary_diff_action = ready_palette_actions(&mut app)
        .iter()
        .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "binary-diff"))
        .expect("built-in Binary Diff action");
    app.selected_action_index = binary_diff_action;

    app.execute_current_action(&egui::Context::default());

    assert!(matches!(app.viewer, Some(ViewerSession::BinaryDiff(_))));
    assert_eq!(app.view, WorkbenchView::Palette);

    let ctx = egui::Context::default();
    let search_id = egui::Id::new("binary_diff_search_field");
    if let Some(ViewerSession::BinaryDiff(state)) = &mut app.viewer {
        state.search_query = "fixture".to_owned();
        state.update_search();
    }
    ctx.memory_mut(|memory| memory.request_focus(search_id));
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(search_id));
    let Some(ViewerSession::BinaryDiff(state)) = app.viewer.as_ref() else {
        panic!("Binary Diff should remain open while search is focused");
    };
    assert_eq!(state.search_query, "fixture");

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    let Some(ViewerSession::BinaryDiff(state)) = app.viewer.as_ref() else {
        panic!("first Escape should leave Binary Diff open");
    };
    assert!(state.search_query.is_empty());
    assert_eq!(app.view, WorkbenchView::Palette);

    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![key_press_event(egui::Key::Escape)],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(app.viewer.is_none());
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(app.selected_slots, original_slots);
}

#[test]
fn quick_palette_operation_uses_injected_request_sender() {
    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .expect("left slot") = Some(ClipItem::from_text(" input text "));
    app.selected_slots = vec![SlotId::LeftHand];
    app.search_query = "uppercase".to_owned();
    app.selected_action_index = ready_palette_actions(&mut app)
        .iter()
        .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "uppercase"))
        .expect("uppercase operation is available for text");

    let sent = std::cell::RefCell::new(None);
    app.execute_current_action_with(&egui::Context::default(), |request| {
        *sent.borrow_mut() = Some(request.clone());
        Err("controlled transport failure".to_owned())
    });

    assert!(matches!(
        sent.into_inner(),
        Some(IpcRequest::ExecuteOp {
            op_name,
            selected_slots: Some(slots),
            ..
        }) if op_name == "uppercase" && slots == vec![SlotId::LeftHand]
    ));
    assert_eq!(
        app.status_msg.as_deref(),
        Some("Error: controlled transport failure")
    );
}

#[test]
fn quick_palette_operation_success_closes_popup_once() {
    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .expect("left slot") = Some(ClipItem::from_text("fixture text"));
    app.selected_slots = vec![SlotId::LeftHand];
    app.search_query = "uppercase".to_owned();
    app.selected_action_index = ready_palette_actions(&mut app)
        .iter()
        .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "uppercase"))
        .expect("uppercase operation is available for text");

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        app.execute_current_action_with(ctx, |_| {
            Ok(IpcResponse::OpResult {
                item: ClipItem::from_text("FIXTURE TEXT"),
            })
        });
    });

    assert_eq!(viewport_close_command_count(&output), 1);
    assert_eq!(app.view, WorkbenchView::Palette);
}

#[test]
fn quick_palette_operation_error_keeps_popup_open_with_feedback() {
    let mut app = test_workbench_app(None, VecDeque::new());
    *Arc::make_mut(&mut app.state)
        .get_mut(SlotId::LeftHand)
        .expect("left slot") = Some(ClipItem::from_text("fixture text"));
    app.selected_slots = vec![SlotId::LeftHand];
    app.search_query = "uppercase".to_owned();
    app.selected_action_index = ready_palette_actions(&mut app)
        .iter()
        .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "uppercase"))
        .expect("uppercase operation is available for text");

    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        app.execute_current_action_with(ctx, |_| {
            Ok(IpcResponse::Error {
                error: "fixture operation failure".to_owned(),
            })
        });
    });

    assert_eq!(viewport_close_command_count(&output), 0);
    assert_eq!(app.view, WorkbenchView::Palette);
    assert_eq!(
        app.status_msg.as_deref(),
        Some("Error: fixture operation failure")
    );
}

#[test]
fn quick_palette_recipe_success_closes_popup_once() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let recipe_name = "No Prompt Success Fixture";
    app.recipes.push(Recipe {
        name: recipe_name.to_owned(),
        description: String::new(),
        input_type: little_clip_ops::pipeline::types::RecipeInputType::None,
        tags: Vec::new(),
        graph: little_clip_ops::pipeline::types::NodeGraph::default(),
    });
    app.search_query = recipe_name.to_owned();
    app.selected_action_index = ready_palette_actions(&mut app)
        .iter()
        .position(
            |action| matches!(action, PaletteAction::Recipe(recipe) if recipe.name == recipe_name),
        )
        .expect("recipe is available");

    let sent = std::cell::RefCell::new(None);
    let ctx = egui::Context::default();
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        app.execute_current_action_with(ctx, |request| {
            *sent.borrow_mut() = Some(request.clone());
            Ok(IpcResponse::Success {
                message: "fixture completed".to_owned(),
            })
        });
    });

    assert!(matches!(
        sent.into_inner(),
        Some(IpcRequest::ExecuteRecipe { recipe_name: sent_name }) if sent_name == recipe_name
    ));
    assert_eq!(viewport_close_command_count(&output), 1);
    assert_eq!(app.view, WorkbenchView::Palette);
}

#[test]
fn auto_action_success_sends_once_and_keeps_enabled_config() {
    let mut app = test_workbench_app(None, VecDeque::new());
    app.config.auto_actions.trim_link_tracking = false;
    let ctx = egui::Context::default();
    let mut checkbox_rect = egui::Rect::NOTHING;
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                checkbox_rect = render_auto_actions(ui, &mut app.config, &app.ui_palette)
                    .1
                    .checkbox;
            });
        },
    );
    assert!(checkbox_rect.is_positive());
    let click_pos = checkbox_rect.center();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            events: vec![egui::Event::PointerButton {
                pos: click_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_auto_actions(ui, &mut app.config, &app.ui_palette);
            });
        },
    );
    let mut changed = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(screen_rect),
            events: vec![egui::Event::PointerButton {
                pos: click_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                changed = render_auto_actions(ui, &mut app.config, &app.ui_palette).0;
            });
        },
    );
    assert_eq!(
        changed,
        Some(true),
        "rendered checkbox click should enable it"
    );
    assert!(app.config.auto_actions.trim_link_tracking);
    let sent = std::cell::RefCell::new(Vec::new());

    app.execute_auto_action_change_with(&ctx, false, true, &mut |request| {
        sent.borrow_mut().push(request.clone());
        Ok(IpcResponse::Success {
            message: "updated".to_owned(),
        })
    });

    assert!(matches!(
        sent.borrow().as_slice(),
        [IpcRequest::SetAutoAction { key, enabled: true }]
            if key == "trim_link_tracking"
    ));
    assert!(app.config.auto_actions.trim_link_tracking);
    assert!(app.status_msg.is_none());
}

#[test]
fn auto_action_failure_rolls_back_optimistic_toggle_with_feedback() {
    let mut app = test_workbench_app(None, VecDeque::new());
    app.config.auto_actions.trim_link_tracking = true;
    let sent = std::cell::RefCell::new(Vec::new());

    app.execute_auto_action_change_with(&egui::Context::default(), false, true, &mut |request| {
        sent.borrow_mut().push(request.clone());
        Err("controlled daemon failure".to_owned())
    });

    assert!(matches!(
        sent.borrow().as_slice(),
        [IpcRequest::SetAutoAction { key, enabled: true }]
            if key == "trim_link_tracking"
    ));
    assert!(!app.config.auto_actions.trim_link_tracking);
    assert_eq!(
        app.status_msg.as_deref(),
        Some("Error: controlled daemon failure")
    );
}
