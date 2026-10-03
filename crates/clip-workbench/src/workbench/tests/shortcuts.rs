use super::*;

#[test]
fn quick_palette_space_routes_to_both_hands_only_without_audio_selection() {
    let mut app = test_workbench_app(None, VecDeque::new());
    app.selected_slots = vec![SlotId::LeftPrior1];
    let ctx = egui::Context::default();

    let _ = ctx.run(
        egui::RawInput {
            events: vec![space_key_event()],
            ..Default::default()
        },
        |ctx| app.handle_empty_search_shortcuts(ctx, false),
    );
    assert_eq!(
        app.selected_slots,
        vec![SlotId::LeftHand, SlotId::RightHand]
    );
    assert_eq!(app.selected_action_index, 0);

    app.selected_slots = vec![SlotId::LeftPrior1];
    let _ = ctx.run(
        egui::RawInput {
            events: vec![space_key_event()],
            ..Default::default()
        },
        |ctx| app.handle_empty_search_shortcuts(ctx, true),
    );
    assert_eq!(app.selected_slots, vec![SlotId::LeftPrior1]);
}

#[test]
fn palette_digit_shortcuts_select_exact_slot_sets() {
    let cases = [
        (egui::Key::Num1, vec![SlotId::LeftHand, SlotId::LeftPrior1]),
        (egui::Key::Num2, vec![SlotId::LeftHand, SlotId::LeftPrior2]),
        (
            egui::Key::Num3,
            vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2],
        ),
        (egui::Key::Num0, vec![SlotId::LeftHand]),
    ];

    for (key, expected_slots) in cases {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.selected_slots = vec![SlotId::RightPrior2];
        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(key)],
                ..Default::default()
            },
            |ctx| app.handle_empty_search_shortcuts(ctx, false),
        );

        assert_eq!(app.selected_slots, expected_slots, "key {key:?}");
        assert!(app.dirty_mirror, "key {key:?} should refresh the mirror");
        assert_eq!(app.selected_action_index, 0, "key {key:?}");
    }
}

#[test]
fn palette_search_prefixes_select_slots_and_keep_remaining_query() {
    let cases = [
        (
            "1",
            vec![SlotId::LeftHand, SlotId::LeftPrior1],
            String::new(),
        ),
        (
            "2",
            vec![SlotId::LeftHand, SlotId::LeftPrior2],
            String::new(),
        ),
        (
            "3",
            vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2],
            String::new(),
        ),
        ("0", vec![SlotId::LeftHand], String::new()),
        ("`", vec![SlotId::LeftHand], String::new()),
        (
            "  uppercase",
            vec![SlotId::LeftHand, SlotId::RightHand],
            "uppercase".to_owned(),
        ),
    ];

    for (query, expected_slots, expected_query) in cases {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.search_query = query.to_owned();
        app.handle_search_prefix_shortcuts(false);

        assert_eq!(app.selected_slots, expected_slots, "query {query:?}");
        assert_eq!(app.search_query, expected_query, "query {query:?}");
        assert!(
            app.dirty_mirror,
            "query {query:?} should refresh the mirror"
        );
    }
}

#[test]
fn palette_swap_prefix_stays_available_without_exactly_two_selected_slots() {
    for selected_slots in [
        vec![SlotId::LeftHand],
        vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::RightHand],
    ] {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.selected_slots = selected_slots.clone();
        Arc::make_mut(&mut app.state).left_hand =
            Some(ClipItem::from_text("left fixture".to_owned()));
        Arc::make_mut(&mut app.state).right_hand =
            Some(ClipItem::from_text("right fixture".to_owned()));
        let state_before = app.state.clone();
        app.search_query = "swap".to_owned();
        app.dirty_mirror = false;
        app.selected_action_index = 2;

        // With no daemon running, this production parser path must keep the
        // command as a query and avoid entering the IPC-backed swap path.
        app.handle_search_prefix_shortcuts(false);

        assert_eq!(app.search_query, "swap");
        assert_eq!(app.selected_slots, selected_slots);
        assert_eq!(app.state, state_before);
        assert!(!app.dirty_mirror);
        assert_eq!(app.selected_action_index, 2);
        assert!(app.status_msg.is_none());
    }
}

#[test]
fn modified_palette_swap_keys_do_not_swap_slots() {
    let modified_keys = [
        (egui::Key::S, egui::Modifiers::CTRL),
        (egui::Key::S, egui::Modifiers::COMMAND),
        (egui::Key::S, egui::Modifiers::MAC_CMD),
        (egui::Key::S, egui::Modifiers::SHIFT),
        (egui::Key::S, egui::Modifiers::ALT),
        (egui::Key::X, egui::Modifiers::CTRL),
        (egui::Key::X, egui::Modifiers::COMMAND),
        (egui::Key::X, egui::Modifiers::MAC_CMD),
        (egui::Key::X, egui::Modifiers::SHIFT),
        (egui::Key::X, egui::Modifiers::ALT),
    ];

    for (key, modifiers) in modified_keys {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
        Arc::make_mut(&mut app.state).left_hand =
            Some(ClipItem::from_text("left fixture".to_owned()));
        Arc::make_mut(&mut app.state).right_hand =
            Some(ClipItem::from_text("right fixture".to_owned()));
        let state_before = app.state.clone();
        app.dirty_mirror = false;
        app.search_query.clear();
        let mut swap_calls = 0;

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                modifiers,
                ..Default::default()
            },
            |ctx| app.handle_empty_search_shortcuts_with_swap(ctx, false, |_| swap_calls += 1),
        );

        assert_eq!(swap_calls, 0, "key {key:?}, modifiers {modifiers:?}");
        assert_eq!(
            app.selected_slots,
            vec![SlotId::LeftHand, SlotId::RightHand]
        );
        assert_eq!(
            app.state, state_before,
            "key {key:?}, modifiers {modifiers:?}"
        );
        assert!(!app.dirty_mirror, "key {key:?}, modifiers {modifiers:?}");
        assert!(
            app.status_msg.is_none(),
            "key {key:?}, modifiers {modifiers:?}"
        );
    }
}

#[test]
fn unmodified_palette_swap_shortcuts_request_one_slot_swap() {
    for key in [egui::Key::S, egui::Key::X] {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
        let mut swap_calls = 0;

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(key)],
                ..Default::default()
            },
            |ctx| app.handle_empty_search_shortcuts_with_swap(ctx, false, |_| swap_calls += 1),
        );

        assert_eq!(swap_calls, 1, "key {key:?}");
    }
}

#[test]
fn palette_action_navigation_wraps_in_both_directions() {
    let mut app = test_workbench_app(None, VecDeque::new());
    for name in ["Navigation Fixture A", "Navigation Fixture B"] {
        app.recipes.push(Recipe {
            name: name.to_owned(),
            description: String::new(),
            input_type: little_clip_ops::pipeline::types::RecipeInputType::None,
            tags: Vec::new(),
            graph: little_clip_ops::pipeline::types::NodeGraph::default(),
        });
    }
    app.search_query = "Navigation Fixture".to_owned();
    assert_eq!(ready_palette_actions(&mut app).len(), 2);

    let _ = egui::Context::default().run(
        egui::RawInput {
            events: vec![key_press_event(egui::Key::ArrowDown)],
            ..Default::default()
        },
        |ctx| app.handle_palette_nav_and_execution(ctx),
    );
    assert_eq!(app.selected_action_index, 1, "Down selects the next action");

    let _ = egui::Context::default().run(
        egui::RawInput {
            events: vec![key_press_event(egui::Key::ArrowDown)],
            ..Default::default()
        },
        |ctx| app.handle_palette_nav_and_execution(ctx),
    );
    assert_eq!(
        app.selected_action_index, 0,
        "Down wraps to the first action"
    );

    let _ = egui::Context::default().run(
        egui::RawInput {
            events: vec![key_press_event(egui::Key::ArrowUp)],
            ..Default::default()
        },
        |ctx| app.handle_palette_nav_and_execution(ctx),
    );
    assert_eq!(app.selected_action_index, 1, "Up wraps to the last action");
}

#[test]
fn rolling_audio_space_requires_selection_and_yields_to_focused_text() {
    let mut query = String::new();
    let mut eligible = false;
    let focused_ctx = egui::Context::default();
    let _ = focused_ctx.run(
        egui::RawInput {
            events: vec![space_key_event()],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut query))
                    .request_focus();
                assert!(ctx.wants_keyboard_input());
                assert!(!audio_strip_space_pressed(ctx, true));
                assert!(!audio_strip_space_pressed(ctx, false));
            });
        },
    );

    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            events: vec![space_key_event()],
            ..Default::default()
        },
        |ctx| eligible = audio_strip_space_pressed(ctx, true),
    );
    assert!(
        eligible,
        "an unfocused Space with a selected audio region routes to playback"
    );
}

#[test]
fn rolling_audio_f5_dispatches_one_snapshot_refresh_request() {
    let mut app = test_workbench_app(None, VecDeque::new());
    app.rolling_audio_strip = Some(RollingAudioStripState::empty_test_strip());
    let ctx = egui::Context::default();
    let mut refresh_requests = 0;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![key_press_event(egui::Key::F5)],
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |_ui| {
                app.handle_audio_strip_refresh_with_load(ctx, |strip| {
                    refresh_requests += 1;
                    assert!(!strip.is_loaded);
                });
            });
        },
    );

    assert_eq!(refresh_requests, 1);
    assert!(
        app.rolling_audio_strip
            .as_ref()
            .is_some_and(|strip| !strip.is_loaded)
    );
}

#[test]
fn auto_actions_chrome_stays_inside_compact_and_wide_hosts() {
    for size in [egui::vec2(1280.0, 720.0), egui::vec2(3440.0, 1440.0)] {
        let ctx = egui::Context::default();
        ui_ext::configure_workbench_style(&ctx);
        let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
        let mut config = AppConfig::default();
        let mut host = egui::Rect::NOTHING;
        let mut rendered = None;

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                ui_appearance::apply_visuals(ctx, &palette);
                egui::CentralPanel::default().show(ctx, |ui| {
                    host = ui.available_rect_before_wrap();
                    rendered = Some(render_auto_actions(ui, &mut config, &palette));
                });
            },
        );

        let layout = rendered.expect("Auto Actions view should render").1;
        for (name, rect) in [
            ("header", layout.header),
            ("subtitle", layout.subtitle),
            ("card", layout.card),
            ("status", layout.status),
            ("explanation", layout.explanation),
            ("behavior", layout.behavior),
        ] {
            assert!(
                rect.is_positive(),
                "{name} was not laid out at {size:?}: {rect:?}"
            );
            assert!(
                host.contains_rect(rect),
                "{name} {rect:?} falls outside Auto Actions host {host:?} at {size:?}"
            );
        }
    }
}

#[test]
fn node_machine_substates_each_consume_escape_before_palette_return() {
    let mut view = WorkbenchView::NodeMachine;
    let mut canvas = NodeCanvasState::new();
    canvas.catalog_open = true;
    canvas.save_dialog_open = true;
    canvas.dragging_from = Some((uuid::Uuid::new_v4(), "Out".to_owned(), true));
    let graph_before = canvas.graph.clone();
    let mut search_needs_focus = false;

    assert_eq!(
        route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
        WorkbenchEscapeRoute::Handled
    );
    assert!(!canvas.catalog_open);
    assert!(canvas.save_dialog_open);
    assert!(canvas.dragging_from.is_some());
    assert_eq!(canvas.graph, graph_before);
    assert_eq!(view, WorkbenchView::NodeMachine);

    assert_eq!(
        route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
        WorkbenchEscapeRoute::Handled
    );
    assert!(!canvas.save_dialog_open);
    assert!(canvas.dragging_from.is_some());
    assert_eq!(canvas.graph, graph_before);
    assert_eq!(view, WorkbenchView::NodeMachine);

    assert_eq!(
        route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
        WorkbenchEscapeRoute::Handled
    );
    assert!(canvas.dragging_from.is_none());
    assert_eq!(canvas.graph, graph_before);
    assert_eq!(view, WorkbenchView::NodeMachine);

    assert_eq!(
        route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
        WorkbenchEscapeRoute::ReturnToPalette
    );
    assert_eq!(view, WorkbenchView::Palette);
    assert!(search_needs_focus);
}

#[test]
fn auto_actions_escape_returns_to_palette_and_requests_search_focus() {
    let mut view = WorkbenchView::AutoActions;
    let mut canvas = NodeCanvasState::new();
    let mut search_needs_focus = false;

    assert_eq!(
        route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
        WorkbenchEscapeRoute::ReturnToPalette
    );
    assert_eq!(view, WorkbenchView::Palette);
    assert!(search_needs_focus);
}

#[test]
fn node_machine_catalog_and_save_overlays_hold_f1_f3_view_switches() {
    fn press_shortcut(app: &mut WorkbenchApp, key: egui::Key) {
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.handle_global_shortcuts(ctx),
        );
    }

    let shortcuts = [egui::Key::F1, egui::Key::F2, egui::Key::F3];
    for catalog_open in [true, false] {
        for shortcut in shortcuts {
            let mut app = test_workbench_app(None, VecDeque::new());
            app.view = WorkbenchView::NodeMachine;
            app.canvas_state.catalog_open = catalog_open;
            app.canvas_state.save_dialog_open = !catalog_open;
            let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();

            press_shortcut(&mut app, shortcut);

            assert_eq!(app.view, WorkbenchView::NodeMachine);
            assert_eq!(app.canvas_state.catalog_open, catalog_open);
            assert_eq!(app.canvas_state.save_dialog_open, !catalog_open);
            assert_eq!(
                serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
                graph_before,
                "{shortcut:?} must not mutate the graph while an overlay is open"
            );

            app.canvas_state.catalog_open = false;
            app.canvas_state.save_dialog_open = false;
            for (resume_key, expected_view) in [
                (egui::Key::F2, WorkbenchView::Palette),
                (egui::Key::F1, WorkbenchView::NodeMachine),
                (egui::Key::F3, WorkbenchView::AutoActions),
            ] {
                press_shortcut(&mut app, resume_key);
                assert_eq!(app.view, expected_view);
                assert!(!app.canvas_state.catalog_open);
                assert!(!app.canvas_state.save_dialog_open);
                assert_eq!(
                    serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
                    graph_before,
                    "{resume_key:?} after dismissal must preserve graph state"
                );
            }
        }
    }
}

#[test]
fn palette_f1_opens_node_machine_through_global_shortcuts() {
    let mut app = test_workbench_app(None, VecDeque::new());
    assert_eq!(app.view, WorkbenchView::Palette);
    let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();

    let _ = egui::Context::default().run(
        egui::RawInput {
            events: vec![key_press_event(egui::Key::F1)],
            ..Default::default()
        },
        |ctx| app.handle_global_shortcuts(ctx),
    );

    assert_eq!(app.view, WorkbenchView::NodeMachine);
    assert_eq!(
        serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
        graph_before
    );
}

#[test]
fn palette_f4_and_f6_switch_workbench_and_display_palettes_together() {
    let mut app = test_workbench_app(None, VecDeque::new());
    let ctx = egui::Context::default();
    let initial = app.config.workbench.appearance;
    let mut saved = Vec::new();

    for (key, expected) in [(egui::Key::F4, initial.toggled()), (egui::Key::F6, initial)] {
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_press_event(key)],
                ..Default::default()
            },
            |ctx| {
                app.handle_appearance_shortcut_with(ctx, |appearance| {
                    saved.push(appearance);
                    Ok(())
                });
            },
        );

        let expected_palette = ui_appearance::palette(expected);
        assert_eq!(app.config.workbench.appearance, expected);
        assert_eq!(
            app.renderer.as_ref().unwrap().config.workbench.appearance,
            expected
        );
        assert_eq!(
            app.renderer.as_ref().unwrap().palette.colors,
            expected_palette.colors
        );
        assert_eq!(app.ui_palette.colors, expected_palette.colors);
        assert!(
            app.dirty_mirror,
            "display mirror must be repainted on {key:?}"
        );
        app.dirty_mirror = false;
    }

    assert_eq!(saved, vec![initial.toggled(), initial]);
}

#[test]
fn palette_f3_opens_auto_actions_through_global_shortcuts() {
    let mut app = test_workbench_app(None, VecDeque::new());
    assert_eq!(app.view, WorkbenchView::Palette);
    let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();

    let _ = egui::Context::default().run(
        egui::RawInput {
            events: vec![key_press_event(egui::Key::F3)],
            ..Default::default()
        },
        |ctx| app.handle_global_shortcuts(ctx),
    );

    assert_eq!(app.view, WorkbenchView::AutoActions);
    assert_eq!(
        serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
        graph_before
    );
}

#[test]
fn node_canvas_state_persists_across_palette_and_auto_actions_routes() {
    let mut app = test_workbench_app(None, VecDeque::new());
    app.view = WorkbenchView::NodeMachine;
    app.canvas_state.selected_recipe_name = "Persistence fixture".to_owned();
    app.canvas_state.graph.nodes[0].title = "Persistent graph node".to_owned();
    app.canvas_state.selected_node = Some(app.canvas_state.graph.nodes[0].id);
    app.canvas_state
        .undo_stack
        .push(app.canvas_state.graph.clone());
    let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();
    let undo_before = serde_yaml::to_string(&app.canvas_state.undo_stack).unwrap();
    let selected_node = app.canvas_state.selected_node;
    let recipe_name = app.canvas_state.selected_recipe_name.clone();

    for (key, expected_view) in [
        (egui::Key::F2, WorkbenchView::Palette),
        (egui::Key::F3, WorkbenchView::AutoActions),
        (egui::Key::F2, WorkbenchView::Palette),
        (egui::Key::F1, WorkbenchView::NodeMachine),
    ] {
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.handle_global_shortcuts(ctx),
        );

        assert_eq!(app.view, expected_view);
        assert_eq!(
            serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
            graph_before
        );
        assert_eq!(app.canvas_state.selected_node, selected_node);
        assert_eq!(app.canvas_state.selected_recipe_name, recipe_name);
        assert_eq!(
            serde_yaml::to_string(&app.canvas_state.undo_stack).unwrap(),
            undo_before
        );
    }
}

#[test]
fn node_machine_prompt_and_viewer_preempt_workbench_shortcuts_until_closed() {
    fn frame_with_key(app: &mut WorkbenchApp, key: egui::Key) -> bool {
        let ctx = egui::Context::default();
        let mut modal_active = false;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                modal_active = app.render_modal_viewers(ctx);
                // Match update(): the prompt/viewer renderer owns the frame and
                // returns before workbench shortcut dispatch or canvas rendering.
                if modal_active {
                    return;
                }
                app.handle_global_shortcuts(ctx);
            },
        );
        modal_active
    }

    let mut prompt_app = test_workbench_app(None, VecDeque::new());
    prompt_app.view = WorkbenchView::NodeMachine;
    prompt_app.canvas_state.selected_node = prompt_app
        .canvas_state
        .graph
        .nodes
        .first()
        .map(|node| node.id);
    prompt_app.in_app_prompt = Some(InAppPromptState {
        recipe_name: "Modal ownership fixture".to_owned(),
        message: "Enter text".to_owned(),
        input_text: "input".to_owned(),
        first_frame: true,
    });
    let prompt_graph = serde_yaml::to_string(&prompt_app.canvas_state.graph).unwrap();
    assert!(frame_with_key(&mut prompt_app, egui::Key::F2));
    assert!(frame_with_key(&mut prompt_app, egui::Key::Delete));
    assert_eq!(prompt_app.view, WorkbenchView::NodeMachine);
    assert!(prompt_app.in_app_prompt.is_some());
    assert_eq!(
        serde_yaml::to_string(&prompt_app.canvas_state.graph).unwrap(),
        prompt_graph
    );
    assert!(frame_with_key(&mut prompt_app, egui::Key::Escape));
    assert!(prompt_app.in_app_prompt.is_none());
    assert_eq!(prompt_app.view, WorkbenchView::NodeMachine);
    assert!(!frame_with_key(&mut prompt_app, egui::Key::F2));
    assert_eq!(prompt_app.view, WorkbenchView::Palette);

    let mut viewer_app = test_workbench_app(
        Some(ViewerSession::ComponentGallery(
            component_gallery::ComponentGalleryState::default(),
        )),
        VecDeque::new(),
    );
    viewer_app.view = WorkbenchView::NodeMachine;
    viewer_app.canvas_state.selected_node = viewer_app
        .canvas_state
        .graph
        .nodes
        .first()
        .map(|node| node.id);
    let viewer_graph = serde_yaml::to_string(&viewer_app.canvas_state.graph).unwrap();
    assert!(frame_with_key(&mut viewer_app, egui::Key::F2));
    assert!(frame_with_key(&mut viewer_app, egui::Key::Delete));
    assert_eq!(viewer_app.view, WorkbenchView::NodeMachine);
    assert!(viewer_app.viewer.is_some());
    assert_eq!(
        serde_yaml::to_string(&viewer_app.canvas_state.graph).unwrap(),
        viewer_graph
    );
    assert!(frame_with_key(&mut viewer_app, egui::Key::Escape));
    assert!(viewer_app.viewer.is_none());
    assert_eq!(viewer_app.view, WorkbenchView::NodeMachine);
    assert!(!frame_with_key(&mut viewer_app, egui::Key::F2));
    assert_eq!(viewer_app.view, WorkbenchView::Palette);
    assert_eq!(
        serde_yaml::to_string(&viewer_app.canvas_state.graph).unwrap(),
        viewer_graph
    );
}
