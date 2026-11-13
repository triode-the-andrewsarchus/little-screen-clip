use super::*;

const SOURCE: &str = include_str!("../../../../docs/examples/diagrams/harness.wireviz");

#[test]
fn selected_pin_traces_real_cable_and_single_ended_shield_connections() {
    let mut editor = WireEditor {
        source: Some(Arc::from(SOURCE)),
        model: Some(Arc::new(Model::parse(SOURCE).unwrap())),
        ..Default::default()
    };
    editor.select(Some(Endpoint::Pin {
        connector: 0,
        pin: 0,
    }));
    assert_eq!(editor.label, "RETURN");
    assert_eq!(editor.connected.len(), 4);
    for endpoint in [
        Endpoint::Pin {
            connector: 1,
            pin: 2,
        },
        Endpoint::Cable {
            cable: 0,
            conductor: Conductor::Wire(0),
        },
        Endpoint::Cable {
            cable: 0,
            conductor: Conductor::Shield,
        },
    ] {
        assert!(editor.connected.contains(&endpoint));
    }
    assert!(!editor.connected.contains(&Endpoint::Pin {
        connector: 0,
        pin: 1
    }));
    editor.select(Some(Endpoint::Cable {
        cable: 0,
        conductor: Conductor::Wire(2),
    }));
    assert_eq!(editor.color, "GNYE");
    assert_eq!(editor.connected.len(), 3);
}

#[test]
fn whole_connector_mating_is_not_invented_as_pin_connectivity() {
    let source =
        "connectors:\n  A: {pincount: 1}\n  B: {pincount: 1}\nconnections: [[A, '==>', B]]\n";
    let mut editor = WireEditor {
        model: Some(Arc::new(Model::parse(source).unwrap())),
        ..Default::default()
    };
    editor.select(Some(Endpoint::Pin {
        connector: 0,
        pin: 0,
    }));
    assert_eq!(editor.connected.len(), 1);
}

#[test]
fn changing_source_retires_accepted_edits_before_their_result_can_publish() {
    let edit = Edit::PinLabel {
        connector: 0,
        pin: 0,
        label: "GROUND".into(),
    };
    let (mut editor, release) = delayed_edit(SOURCE, edit);
    assert!(editor.busy());
    release.send(()).unwrap();
    let replacement = format!("{SOURCE}\n# A newer manual edit\n");
    assert!(matches!(
        editor.poll(&egui::Context::default(), &replacement),
        Some(GraphicalUpdate::Replaced)
    ));
    assert!(!editor.busy());
    assert!(editor.failed());
    assert!(editor.model.is_none() && editor.pending.is_none());
}

#[test]
fn duplicate_connection_finishes_without_a_host_undo_update() {
    let source: Arc<str> = Arc::from(SOURCE);
    let model = Arc::new(Model::parse(&source).unwrap());
    let link = model
        .editable
        .harness()
        .links
        .iter()
        .find(|link| link.kind == LinkKind::Cable)
        .unwrap();
    let [from, to] = link.endpoints;
    let update = edit_source(model, source.clone(), Edit::Connect { from, to }).unwrap();
    assert!(!update.publish);
    assert_eq!(update.source, source);
}

#[test]
fn disconnect_preserves_the_selected_terminal_identity_and_rebuilds_its_trace() {
    let source: Arc<str> = Arc::from(SOURCE);
    let model = Arc::new(Model::parse(&source).unwrap());
    let index = model
        .editable
        .harness()
        .links
        .iter()
        .position(|link| link.kind == LinkKind::Cable)
        .unwrap();
    let selected = model.editable.harness().links[index].endpoints[0];
    let update = edit_source(model, source, Edit::Disconnect { link: index }).unwrap();
    assert!(update.publish);
    let mut editor = WireEditor {
        source: Some(update.source),
        model: Some(update.model),
        ..Default::default()
    };
    editor.select(Some(selected));
    assert_eq!(editor.selected, Some(selected));
    assert!(editor.connected.contains(&selected));
}

#[test]
fn source_change_and_suspension_cancel_terminal_drags() {
    let source: Arc<str> = Arc::from(SOURCE);
    let model = Arc::new(Model::parse(&source).unwrap());
    let from = Endpoint::Pin {
        connector: 0,
        pin: 0,
    };
    let mut editor = WireEditor {
        source: Some(source.clone()),
        model: Some(model.clone()),
        drag: Some(TerminalDrag {
            from,
            source: source.clone(),
            model: model.clone(),
        }),
        ..Default::default()
    };
    editor.poll(&egui::Context::default(), &format!("{SOURCE}\n# changed"));
    assert!(editor.drag.is_none());
    editor.drag = Some(TerminalDrag {
        from,
        source,
        model,
    });
    editor.suspend();
    assert!(editor.drag.is_none());
}

fn gesture_frame(
    ctx: &egui::Context,
    editor: &mut WireEditor,
    events: Vec<egui::Event>,
    hover: Option<Endpoint>,
) {
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(200.0, 150.0), Sense::click_and_drag());
                editor.terminal_input(ui, &response, hover);
            });
        },
    );
}
fn pointer(pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: egui::pos2(50.0, 50.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}
fn unwired_editor() -> WireEditor {
    let source: Arc<str> = Arc::from(
        "connectors: {J: {pincount: 1}}\ncables: {W: {wirecount: 1}}\nconnections: [[J], [W]]\n",
    );
    WireEditor {
        source: Some(source.clone()),
        model: Some(Arc::new(Model::parse(&source).unwrap())),
        ..Default::default()
    }
}

#[test]
fn terminal_drag_submits_one_typed_connection_only_on_valid_release() {
    let ctx = egui::Context::default();
    let mut editor = unwired_editor();
    let from = Endpoint::Pin {
        connector: 0,
        pin: 0,
    };
    let to = Endpoint::Cable {
        cable: 0,
        conductor: Conductor::Wire(0),
    };
    gesture_frame(&ctx, &mut editor, vec![pointer(true)], Some(from));
    assert!(editor.drag.is_some());
    assert!(!editor.busy());
    gesture_frame(&ctx, &mut editor, vec![pointer(false)], Some(to));
    assert!(editor.drag.is_none());
    assert!(
        matches!(editor.request.as_ref().map(|request| &request.edit), Some(Edit::Connect { from: a, to: b }) if *a == from && *b == to)
    );
}

#[test]
fn releasing_outside_or_over_a_pin_cancels_without_a_transaction() {
    for target in [
        None,
        Some(Endpoint::Pin {
            connector: 0,
            pin: 0,
        }),
    ] {
        let ctx = egui::Context::default();
        let mut editor = unwired_editor();
        gesture_frame(
            &ctx,
            &mut editor,
            vec![pointer(true)],
            Some(Endpoint::Pin {
                connector: 0,
                pin: 0,
            }),
        );
        gesture_frame(&ctx, &mut editor, vec![pointer(false)], target);
        assert!(editor.drag.is_none());
        assert!(!editor.busy());
    }
}

#[test]
fn changing_model_snapshot_cancels_an_in_progress_drag() {
    let ctx = egui::Context::default();
    let mut editor = unwired_editor();
    gesture_frame(
        &ctx,
        &mut editor,
        vec![pointer(true)],
        Some(Endpoint::Pin {
            connector: 0,
            pin: 0,
        }),
    );
    editor.model = Some(Arc::new(
        Model::parse(editor.source.as_deref().unwrap()).unwrap(),
    ));
    gesture_frame(
        &ctx,
        &mut editor,
        vec![pointer(false)],
        Some(Endpoint::Cable {
            cable: 0,
            conductor: Conductor::Wire(0),
        }),
    );
    assert!(editor.drag.is_none());
    assert!(!editor.busy());
}
