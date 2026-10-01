
use super::*;

#[test]
fn pipeline_run_saves_drafts_for_unselected_script_nodes() {
    let mut canvas = NodeCanvasState::new();
    let mut node = instantiate_template(
        little_clip_ops::pipeline::catalog::find_template("script_python").unwrap(),
        [0.0, 0.0],
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run.py");
    std::fs::write(&path, "print('before')").unwrap();
    node.props
        .insert("script_file".to_owned(), path.display().to_string());
    let mut editor = crate::script_editor::ScriptEditor::load(&node);
    editor.text = "print('after')".to_owned();
    canvas.script_editors.insert(node.id, editor);
    canvas.graph.nodes = vec![node];
    canvas.selected_node = None;
    assert!(canvas.run_pipeline_action().is_some());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "print('after')");
    std::fs::write(&path, "external").unwrap();
    canvas.script_editors.values_mut().next().unwrap().text = "draft".to_owned();
    assert!(canvas.run_pipeline_action().is_none());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external");
    assert!(canvas.status_message.as_ref().unwrap().1);
}

#[test]
fn run_pipeline_emits_graph_for_single_daemon_execution() {
    let mut canvas = NodeCanvasState::new();
    let source = instantiate_template(
        little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap(),
        [0.0, 0.0],
    );
    let mut source = source;
    source.props.insert("text".to_owned(), "once".to_owned());
    let sink = instantiate_template(
        little_clip_ops::pipeline::catalog::find_template("sink_lh").unwrap(),
        [300.0, 0.0],
    );
    canvas.graph.nodes = vec![source.clone(), sink.clone()];
    canvas.graph.connections = vec![Connection {
        from_node: source.id,
        from_pin: "Text".to_owned(),
        to_node: sink.id,
        to_pin: "In".to_owned(),
    }];

    let action = canvas.run_pipeline_action();
    let Some(CanvasAction::RunPipeline { yaml }) = action else {
        panic!("run action must send the graph to its single execution owner");
    };
    let sent_graph: NodeGraph = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(sent_graph.nodes.len(), 2);
    assert!(
        canvas.node_previews.is_empty(),
        "UI must not execute locally"
    );
    assert!(canvas.last_run_duration_ms.is_none());
    assert!(matches!(
        canvas.status_message.as_ref(),
        Some((message, false)) if message == "Running pipeline…"
    ));

    // Exercise the daemon's production graph executor and slot-result behavior in-process.
    // This graph contains only a constant text node and the Left Hand sink, so no external
    // command, daemon, clipboard, or display refresh is involved.
    let mut slots = little_clip_core::SlotManager::new();
    slots.push_left(little_clip_core::ClipItem::from_text("previous item"));
    let execution = little_clip_ops::PipelineExecutor::new(&sent_graph, &slots)
        .execute()
        .expect("constant-text Left Hand graph should execute");
    let output = execution
        .left_hand_item
        .expect("SetLeftHand should declare a Left Hand output");
    assert_eq!(output.title, "once");
    assert_eq!(output.to_clipboard_text().as_deref(), Some("once"));
    slots.apply_op_result(output);
    assert_eq!(
        slots.left_hand().and_then(|item| item.to_clipboard_text()),
        Some("once".to_owned())
    );
    assert_eq!(
        slots.undo_slot().and_then(|item| item.to_clipboard_text()),
        Some("previous item".to_owned())
    );
}

#[test]
fn rendered_save_as_dialog_writes_reloadable_recipe_in_private_directory() {
    let source = instantiate_template(
        little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap(),
        [0.0, 0.0],
    );
    let sink = instantiate_template(
        little_clip_ops::pipeline::catalog::find_template("sink_lh").unwrap(),
        [300.0, 0.0],
    );
    let graph = NodeGraph {
        nodes: vec![source.clone(), sink.clone()],
        connections: vec![Connection {
            from_node: source.id,
            from_pin: "Text".to_owned(),
            to_node: sink.id,
            to_pin: "In".to_owned(),
        }],
        pan: [0.0, 0.0],
        zoom: 1.0,
    };
    let mut canvas = NodeCanvasState {
        graph: graph.clone(),
        script_editors: HashMap::new(),
        selected_node: None,
        dragging_from: None,
        node_previews: HashMap::new(),
        catalog_open: false,
        catalog_search: String::new(),
        catalog_category: "All".to_owned(),
        spawn_node_pos: None,
        save_dialog_open: true,
        save_name: "Private Save As Regression".to_owned(),
        save_is_save_as: true,
        current_recipe_file: None,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
        status_message: None,
        available_recipes: Vec::new(),
        selected_recipe_name: "Original Recipe".to_owned(),
        last_run_duration_ms: None,
    };
    let recipes_dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette = AppPalette::default();
    let mut time = 0.0;
    let mut render = |events: Vec<egui::Event>| {
        time += 0.1;
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 620.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    canvas.render_save_window_in_dir(ui, &palette, recipes_dir.path());
                });
            },
        )
    };

    let _warmup = render(Vec::new());
    let initial = render(Vec::new());
    let save_position = initial
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) if text.galley.text() == "Save" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("rendered Save action in recipe dialog");
    render(vec![egui::Event::PointerMoved(save_position)]);
    render(vec![
        egui::Event::PointerMoved(save_position),
        egui::Event::PointerButton {
            pos: save_position,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    render(vec![
        egui::Event::PointerMoved(save_position),
        egui::Event::PointerButton {
            pos: save_position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    drop(render);

    let saved_path = canvas
        .current_recipe_file
        .as_ref()
        .expect("Save should store current recipe path");
    assert_eq!(saved_path.parent(), Some(recipes_dir.path()));
    assert_eq!(canvas.selected_recipe_name, "Private Save As Regression");
    assert!(!canvas.save_dialog_open);
    assert!(
        canvas
            .available_recipes
            .iter()
            .any(|recipe| { recipe.name == canvas.selected_recipe_name && recipe.graph == graph })
    );
    let serialized = std::fs::read_to_string(saved_path).unwrap();
    let saved: Recipe = serde_yaml::from_str(&serialized).unwrap();
    assert_eq!(saved.name, "Private Save As Regression");
    assert_eq!(saved.graph, graph);
}

#[test]
fn daemon_pipeline_response_restores_previews_and_output_status() {
    let mut canvas = NodeCanvasState::new();
    let node_id = canvas.graph.nodes[0].id;
    canvas.apply_pipeline_response(little_clip_core::ipc::IpcResponse::PipelineExecuted {
        node_previews: [(node_id, "preview".to_owned())].into_iter().collect(),
        output_title: Some("once".to_owned()),
        duration_ms: 17,
    });

    assert_eq!(
        canvas.node_previews.get(&node_id).map(String::as_str),
        Some("preview")
    );
    assert_eq!(canvas.last_run_duration_ms, Some(17));
    assert!(matches!(
        canvas.status_message.as_ref(),
        Some((message, false)) if message == "Executed in 17ms. Output: once"
    ));
}

#[test]
fn node_machine_history_and_delete_shortcuts_yield_to_focused_text_input() {
    fn test_canvas() -> NodeCanvasState {
        let mut canvas = NodeCanvasState::new();
        let node = instantiate_template(
            little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap(),
            [0.0, 0.0],
        );
        let mut previous = node.clone();
        previous.title = "Previous title".to_owned();
        let mut next = node.clone();
        next.title = "Next title".to_owned();
        canvas.graph.nodes = vec![node.clone()];
        canvas.selected_node = Some(node.id);
        let mut previous_graph = canvas.graph.clone();
        previous_graph.nodes[0] = previous;
        let mut next_graph = canvas.graph.clone();
        next_graph.nodes[0] = next;
        canvas.undo_stack.push(previous_graph);
        canvas.redo_stack.push(next_graph);
        canvas
    }

    fn input(events: Vec<egui::Event>) -> egui::RawInput {
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
            events,
            modifiers,
            ..Default::default()
        }
    }

    fn key(key: egui::Key, command: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                command,
                ctrl: command,
                ..Default::default()
            },
        }
    }

    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let cases = [
        (egui::Key::Z, true, "Previous title"),
        (egui::Key::Y, true, "Next title"),
        (egui::Key::Delete, false, "Deleted node"),
    ];

    for (shortcut, command, unfocused_title) in cases {
        let ctx = egui::Context::default();
        let mut focused_canvas = test_canvas();
        focused_canvas.catalog_open = true;
        let _ = ctx.run(input(Vec::new()), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                focused_canvas.ui(ui, &palette);
            });
        });
        let node_id = focused_canvas.selected_node.unwrap();
        let _ = ctx.run(input(vec![key(shortcut, command)]), |ctx| {
            assert!(
                ctx.wants_keyboard_input(),
                "catalog search owns keyboard focus"
            );
            egui::CentralPanel::default().show(ctx, |ui| {
                focused_canvas.ui(ui, &palette);
            });
        });
        assert!(
            focused_canvas
                .graph
                .nodes
                .iter()
                .any(|node| node.id == node_id),
            "focused text input must prevent {shortcut:?} from changing the canvas"
        );
        assert_eq!(
            focused_canvas.graph.nodes[0].title,
            test_canvas().graph.nodes[0].title,
            "focused text input must prevent {shortcut:?} from changing history state"
        );

        let ctx = egui::Context::default();
        let mut unfocused_canvas = test_canvas();
        let _ = ctx.run(input(Vec::new()), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                unfocused_canvas.ui(ui, &palette);
            });
        });
        let _ = ctx.run(input(vec![key(shortcut, command)]), |ctx| {
            assert!(!ctx.wants_keyboard_input());
            egui::CentralPanel::default().show(ctx, |ui| {
                unfocused_canvas.ui(ui, &palette);
            });
        });
        if shortcut == egui::Key::Delete {
            assert!(unfocused_canvas.graph.nodes.is_empty());
        } else {
            assert_eq!(unfocused_canvas.graph.nodes[0].title, unfocused_title);
        }
    }
}

#[test]
fn node_machine_toolbar_wraps_controls_and_status_at_compact_widths() {
    for width in [760.0, 1280.0, 1760.0] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut canvas = NodeCanvasState::new();
        canvas.status_message = Some(("Pipeline complete".to_string(), false));
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 120.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let host = ui.available_rect_before_wrap();
                    canvas.render_toolbar(ui, &mut action, &palette);
                    assert!(ui.min_rect().left() >= host.left() - 0.5);
                });
            },
        );

        let required_labels = [
            "Add Node",
            "New Recipe",
            "Save",
            "Save As...",
            "Undo",
            "Redo",
            "Clear",
            "RUN PIPELINE",
            "Pipeline complete",
        ];
        let mut label_bounds = HashMap::new();
        for required in required_labels {
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
                panic!("Node Machine toolbar omitted {required:?} at {width} points");
            };
            let bounds = text.galley.rect.translate(text.pos.to_vec2());
            assert!(!text.galley.elided, "{required:?} elided at {width} points");
            assert!(
                bounds.left() >= clip_rect.left() - 0.5
                    && bounds.right() <= clip_rect.right() + 0.5,
                "{required:?} exceeds {width}-point toolbar: {bounds:?} clipped to {clip_rect:?}"
            );
            assert!(
                bounds.top() >= clip_rect.top() - 0.5
                    && bounds.bottom() <= clip_rect.bottom() + 0.5,
                "{required:?} is vertically clipped at {width} points: {bounds:?} clipped to {clip_rect:?}"
            );
            label_bounds.insert(required, bounds);
        }
        let status = label_bounds["Pipeline complete"];
        for action in [
            "Add Node",
            "New Recipe",
            "Save",
            "Save As...",
            "Undo",
            "Redo",
            "Clear",
            "RUN PIPELINE",
        ] {
            let action_bounds = label_bounds[action];
            assert!(
                status.top() >= action_bounds.bottom() - 0.5
                    || status.bottom() <= action_bounds.top() + 0.5,
                "status overlaps {action} at {width} points: {status:?} vs {action_bounds:?}"
            );
        }
    }
}

#[test]
fn compact_canvas_keeps_complete_python_script_controls_inside_its_viewport() {
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut canvas = NodeCanvasState::new();
    let mut script = canvas.graph.nodes[0].clone();
    script.kind = NodeKind::PythonScript;
    script.title = "Python Transform".to_owned();
    script.pos = [0.0, 0.0];
    script.props.clear();
    script
        .props
        .insert("script_file".to_owned(), "transform.py".to_owned());
    let script_id = script.id;
    canvas.graph.nodes = vec![script];
    canvas.graph.connections.clear();
    canvas.graph.pan = [4.0, 4.0];
    canvas.graph.zoom = 1.24;
    canvas.selected_node = Some(script_id);
    canvas.status_message = Some(("Saved recipe 'Pad and Caption'".to_owned(), false));
    let mut canvas_bounds = None;
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1177.0, 662.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // Exercise a constrained internal canvas without resizing the host window.
                let allocated = ui.allocate_ui(Vec2::new(ui.available_width(), 270.0), |ui| {
                    canvas.ui(ui, &palette);
                });
                canvas_bounds = Some(allocated.response.rect);
            });
        },
    );
    let footer_top = canvas_bounds.expect("compact canvas allocation").bottom();

    let required = ["Python Transform", "transform.py", "Edit Script"];
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| {
            let egui::epaint::Shape::Text(text) = &clipped.shape else {
                return None;
            };
            Some((clipped.clip_rect, text))
        })
        .collect();
    for label in required {
        let (clip, text) = texts
            .iter()
            .find(|(_, text)| text.galley.job.text.contains(label))
            .unwrap_or_else(|| panic!("missing script property {label:?}"));
        let bounds = text.galley.rect.translate(text.pos.to_vec2());
        assert!(!text.galley.elided, "{label:?} was elided");
        assert!(
            bounds.left() >= clip.left() - 0.5
                && bounds.right() <= clip.right() + 0.5
                && bounds.top() >= clip.top() - 0.5
                && bounds.bottom() <= clip.bottom() + 0.5,
            "{label:?} is clipped in compact canvas: {bounds:?} within {clip:?}"
        );
        assert!(
            bounds.bottom() <= footer_top + 0.5,
            "{label:?} overlaps the fixed footer beginning at {footer_top}: {bounds:?}"
        );
    }
}

#[test]
fn node_canvas_custom_labels_meet_contrast_for_every_category() {
    let categories = [
        NodeKind::LeftHandSource,
        NodeKind::TextTrim,
        NodeKind::ImagePadFrame,
        NodeKind::AudioNormalize,
        NodeKind::FileSha256,
        NodeKind::PythonScript,
        NodeKind::SetLeftHand,
    ];
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let palette = crate::ui_appearance::palette(appearance);
        for kind in &categories {
            let background = get_node_category_color(kind, &palette);
            for preferred in [
                hex_to_c32(&palette.colors.text_primary),
                hex_to_c32(&palette.diff_del),
            ] {
                let foreground = crate::ui_ext::contrast_text_c32_on(
                    background,
                    background,
                    Some(preferred),
                    &palette,
                );
                assert!(
                    little_clip_core::palette::contrast_ratio(
                        (background.r(), background.g(), background.b()),
                        (foreground.r(), foreground.g(), foreground.b()),
                    ) >= 4.5,
                    "{appearance:?} node category {kind:?} label lacks contrast"
                );
            }
        }

        let recessed = hex_to_c32(&palette.colors.surface_recessed);
        let preview_text = crate::ui_ext::contrast_text_c32_on(
            recessed,
            recessed,
            Some(hex_to_c32(&palette.diff_add)),
            &palette,
        );
        assert!(
            little_clip_core::palette::contrast_ratio(
                (recessed.r(), recessed.g(), recessed.b()),
                (preview_text.r(), preview_text.g(), preview_text.b()),
            ) >= 4.5,
            "{appearance:?} node preview label lacks contrast"
        );
    }
}

#[test]
fn test_canvas_undo_redo() {
    let mut state = NodeCanvasState::new();
    let initial_nodes_len = state.graph.nodes.len();
    let initial_conns_len = state.graph.connections.len();

    // 1. Mutate: add a node
    state.snapshot_undo();
    let t_const = little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap();
    let new_node = instantiate_template(t_const, [100.0, 100.0]);
    let new_node_id = new_node.id;
    state.graph.nodes.push(new_node);
    assert_eq!(state.graph.nodes.len(), initial_nodes_len + 1);

    // 2. Undo node addition
    state.undo();
    assert_eq!(state.graph.nodes.len(), initial_nodes_len);
    assert!(!state.graph.nodes.iter().any(|n| n.id == new_node_id));

    // 3. Redo node addition
    state.redo();
    assert_eq!(state.graph.nodes.len(), initial_nodes_len + 1);
    assert!(state.graph.nodes.iter().any(|n| n.id == new_node_id));

    // 4. Mutate: delete connection
    if !state.graph.connections.is_empty() {
        state.snapshot_undo();
        state.graph.connections.clear();
        assert_eq!(state.graph.connections.len(), 0);

        // Undo connection deletion
        state.undo();
        assert_eq!(state.graph.connections.len(), initial_conns_len);

        // Redo connection deletion
        state.redo();
        assert_eq!(state.graph.connections.len(), 0);
    }
}

#[test]
fn test_eval_cubic_bezier() {
    let p0 = Pos2::new(10.0, 20.0);
    let p1 = Pos2::new(40.0, 20.0);
    let p2 = Pos2::new(60.0, 80.0);
    let p3 = Pos2::new(90.0, 80.0);

    // At t = 0.0, exact start point
    let start = eval_cubic_bezier(p0, p1, p2, p3, 0.0);
    assert!((start.x - p0.x).abs() < 1e-4);
    assert!((start.y - p0.y).abs() < 1e-4);

    // At t = 1.0, exact end point
    let end = eval_cubic_bezier(p0, p1, p2, p3, 1.0);
    assert!((end.x - p3.x).abs() < 1e-4);
    assert!((end.y - p3.y).abs() < 1e-4);

    // At t = 0.5, intermediate point between p0 and p3
    let mid = eval_cubic_bezier(p0, p1, p2, p3, 0.5);
    assert!(mid.x > p0.x && mid.x < p3.x);
    assert!(mid.y > p0.y && mid.y < p3.y);
}

#[test]
fn test_magnetic_snap_distance_threshold() {
    let snap_threshold = (32.0 * 1.0_f32).clamp(22.0, 48.0);
    assert_eq!(snap_threshold, 32.0);

    let pin_pos = Pos2::new(200.0, 150.0);
    let close_mouse = Pos2::new(215.0, 160.0);
    let far_mouse = Pos2::new(250.0, 200.0);

    assert!(close_mouse.distance(pin_pos) < snap_threshold);
    assert!(far_mouse.distance(pin_pos) >= snap_threshold);
}
