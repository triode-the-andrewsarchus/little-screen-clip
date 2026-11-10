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

#[test]
fn delayed_daemon_swap_retains_completion_without_speculative_state_or_duplicate_send() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let original = Arc::clone(&app.state);
    let ctx = egui::Context::default();
    let jobs = BackgroundJobs::new(1, 2);
    let (started, running) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    app.submit_daemon_mutation_with(
        IpcRequest::SwapSlots(SlotId::LeftHand, SlotId::RightHand),
        DaemonMutationKind::Swap,
        &jobs,
        move |request| {
            assert!(matches!(request, IpcRequest::SwapSlots(..)));
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(IpcResponse::Error {
                error: "swap rejected".into(),
            })
        },
    );
    running.recv_timeout(Duration::from_secs(5)).unwrap();
    app.submit_daemon_mutation_with(
        IpcRequest::SwapSlots(SlotId::LeftHand, SlotId::RightHand),
        DaemonMutationKind::Swap,
        &jobs,
        |_| panic!("must not submit a second mutation"),
    );
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        app.poll_daemon_mutation(ctx)
    });
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
    );
    assert!(Arc::ptr_eq(&original, &app.state));
    app.view = WorkbenchView::NodeMachine;
    release.send(()).unwrap();
    finish_daemon_mutation(&mut app, &ctx);
    assert_eq!(app.status_msg.as_deref(), Some("swap rejected"));
    assert!(Arc::ptr_eq(&original, &app.state));
}

#[test]
fn background_pipeline_completion_reaches_canvas_after_view_switch() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let ctx = egui::Context::default();
    let jobs = BackgroundJobs::new(1, 1);
    app.submit_daemon_mutation_with(
        IpcRequest::ExecuteGraphYaml {
            yaml: "fixture".into(),
        },
        DaemonMutationKind::Pipeline,
        &jobs,
        |_| {
            Ok(IpcResponse::PipelineExecuted {
                node_previews: Default::default(),
                output_title: Some("output".into()),
                duration_ms: 17,
            })
        },
    );
    app.view = WorkbenchView::Palette;
    finish_daemon_mutation(&mut app, &ctx);
    assert_eq!(app.canvas_state.last_run_duration_ms, Some(17));
}

fn finish_daemon_mutation(app: &mut WorkbenchApp, ctx: &egui::Context) -> usize {
    let mut closes = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while app.daemon_mutation.is_some() {
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.poll_daemon_mutation(ctx)
        });
        closes += viewport_close_command_count(&output);
        assert!(
            std::time::Instant::now() < deadline,
            "daemon completion was not published"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    closes
}

impl WorkbenchApp {
    fn execute_current_action_with(
        &mut self,
        ctx: &egui::Context,
        mut send: impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) {
        self.dispatch_current_action(|app, request| {
            app.apply_action_response(ctx, send(&request), true);
        });
    }
    pub(super) fn render_prompt_modal_with(
        &mut self,
        ctx: &egui::Context,
        mut send: impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) -> bool {
        self.render_prompt_modal_dispatch(ctx, |app, request| {
            app.apply_action_response(ctx, send(&request), true);
        })
    }
    fn execute_auto_action_change_with(
        &mut self,
        ctx: &egui::Context,
        previous_enabled: bool,
        enabled: bool,
        send: &mut impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) {
        let response = send(&IpcRequest::SetAutoAction {
            key: "trim_link_tracking".into(),
            enabled,
        });
        self.config.auto_actions.trim_link_tracking = previous_enabled;
        if self.apply_action_response(ctx, response, false) {
            self.config.auto_actions.trim_link_tracking = enabled;
        }
    }
}

#[test]
fn retained_actions_close_only_on_success_and_settings_publish_only_on_confirmation() {
    let ctx = egui::Context::default();
    let jobs = BackgroundJobs::new(1, 2);
    for success in [false, true] {
        let mut app = test_workbench_app(None, VecDeque::new());
        let (release, wait) = std::sync::mpsc::channel();
        app.submit_daemon_mutation_with(
            IpcRequest::ExecuteRecipe {
                recipe_name: "fixture".into(),
            },
            DaemonMutationKind::Action,
            &jobs,
            move |_| {
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                if success {
                    Ok(IpcResponse::Success {
                        message: "done".into(),
                    })
                } else {
                    Err("controlled failure".into())
                }
            },
        );
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.poll_daemon_mutation(ctx)
        });
        assert_eq!(viewport_close_command_count(&output), 0);
        release.send(()).unwrap();
        assert_eq!(finish_daemon_mutation(&mut app, &ctx), usize::from(success));
        assert_eq!(app.status_msg.is_some(), !success);

        app.config.auto_actions.trim_link_tracking = false;
        app.submit_daemon_mutation_with(
            IpcRequest::SetAutoAction {
                key: "trim_link_tracking".into(),
                enabled: true,
            },
            DaemonMutationKind::AutoAction { enabled: true },
            &jobs,
            move |_| {
                if success {
                    Ok(IpcResponse::Success {
                        message: "updated".into(),
                    })
                } else {
                    Err("controlled failure".into())
                }
            },
        );
        assert!(!app.config.auto_actions.trim_link_tracking);
        assert_eq!(finish_daemon_mutation(&mut app, &ctx), 0);
        assert_eq!(app.config.auto_actions.trim_link_tracking, success);
    }
}

#[test]
fn standalone_prompt_keeps_input_on_failure_and_closes_after_confirmed_retry() {
    let ctx = egui::Context::default();
    let jobs = BackgroundJobs::new(1, 2);
    let mut app = test_workbench_app(None, VecDeque::new());
    let mut prompt = PromptApp::new(
        "Recipe".into(),
        "Input".into(),
        "draft".into(),
        None,
        ui_appearance::palette(AppConfig::default().workbench.appearance),
    );
    for success in [false, true] {
        app.submit_daemon_mutation_with(
            IpcRequest::ExecuteLastAction {
                prompt_text: Some("draft".into()),
            },
            DaemonMutationKind::Action,
            &jobs,
            move |_| {
                if success {
                    Ok(IpcResponse::Success {
                        message: "done".into(),
                    })
                } else {
                    Ok(IpcResponse::Error {
                        error: "rejected".into(),
                    })
                }
            },
        );
        prompt.pending = app.daemon_mutation.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut closes = 0;
        while prompt.pending.is_some() {
            let frame = ctx.run(egui::RawInput::default(), |ctx| {
                prompt.poll_pending(ctx);
            });
            closes += viewport_close_command_count(&frame);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(closes, usize::from(success));
        assert_eq!(prompt.input_text, "draft");
        if !success {
            assert_eq!(prompt.error.as_deref(), Some("rejected"));
        }
    }
}
