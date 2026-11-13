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
