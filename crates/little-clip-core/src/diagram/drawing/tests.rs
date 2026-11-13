use super::*;

fn fixture() -> Value {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../../../docs/examples/diagrams/drawing.excalidraw"
    ))
    .unwrap();
    value["elements"][3]["containerId"] = json!("capture");
    value["future"] =
        serde_json::from_str("{\"exact\":123456789012345678901234567890.125,\"null\":null}")
            .unwrap();
    value["elements"].as_array_mut().unwrap().push(json!(17));
    value
}
fn item<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == id)
        .unwrap()
}
fn apply(value: &Value, change: Change) -> Value {
    let scene = Scene::parse(&value.to_string()).unwrap();
    serde_json::from_str(&scene.apply(change).unwrap().unwrap()).unwrap()
}

#[test]
fn moving_a_box_keeps_its_label_and_arrow_attached_without_moving_the_other_box() {
    let original = fixture();
    let moved = apply(
        &original,
        Change::Move {
            ids: vec!["capture".into()],
            by: [20.0, 0.0],
        },
    );
    assert_eq!(item(&moved, "capture")["x"].as_f64(), Some(20.0));
    assert_eq!(item(&moved, "capture-label")["x"].as_f64(), Some(55.0));
    assert_eq!(item(&moved, "explore"), item(&original, "explore"));
    assert_eq!(item(&moved, "connection")["x"].as_f64(), Some(202.0));
    assert_eq!(
        item(&moved, "connection")["points"][1][0].as_f64(),
        Some(96.0)
    );
    assert_eq!(
        item(&moved, "connection")["endBinding"],
        item(&original, "connection")["endBinding"]
    );
    assert_eq!(moved["files"], original["files"]);
    assert_eq!(moved["future"], original["future"]);
    assert_eq!(
        moved["elements"].as_array().unwrap().last(),
        Some(&json!(17))
    );
    for id in ["capture", "capture-label", "connection"] {
        assert_eq!(item(&moved, id)["version"], json!(2));
    }
}

#[test]
fn grouped_boxes_translate_their_path_rigidly_and_property_edits_keep_mixed_fills() {
    let mut original = fixture();
    for index in [0, 1] {
        original["elements"][index]["groupIds"] = json!(["group"]);
    }
    let moved = apply(
        &original,
        Change::Move {
            ids: vec!["capture".into()],
            by: [-30.0, 15.0],
        },
    );
    assert_eq!(item(&moved, "explore")["x"].as_f64(), Some(270.0));
    assert_eq!(item(&moved, "connection")["x"].as_f64(), Some(152.0));
    assert_eq!(
        item(&moved, "connection")["points"],
        item(&original, "connection")["points"]
    );
    let painted = apply(
        &original,
        Change::Paint {
            ids: vec!["capture".into()],
            paint: Paint::Width(4.0),
        },
    );
    for id in ["capture", "explore"] {
        assert_eq!(
            item(&painted, id)["backgroundColor"],
            item(&original, id)["backgroundColor"]
        );
        assert_eq!(item(&painted, id)["strokeWidth"].as_f64(), Some(4.0));
    }
}

#[test]
fn arrow_labels_follow_the_changed_midpoint_once() {
    let mut original = fixture();
    let mut label = original["elements"][5].clone();
    label["id"] = json!("arrow-label");
    label["containerId"] = json!("connection");
    label["x"] = json!(200);
    original["elements"].as_array_mut().unwrap().push(label);
    let moved = apply(
        &original,
        Change::Move {
            ids: vec!["capture".into()],
            by: [20.0, 0.0],
        },
    );
    assert_eq!(item(&moved, "arrow-label")["x"].as_f64(), Some(210.0));
    assert_eq!(item(&moved, "arrow-label")["version"], json!(2));
}

#[test]
fn deleting_a_box_retains_tombstones_and_resources_and_detaches_surviving_arrows() {
    let original = fixture();
    let deleted = apply(
        &original,
        Change::Delete {
            ids: vec!["capture".into()],
        },
    );
    assert_eq!(
        deleted["elements"].as_array().unwrap().len(),
        original["elements"].as_array().unwrap().len()
    );
    for id in ["capture", "capture-label"] {
        assert_eq!(item(&deleted, id)["isDeleted"], json!(true));
    }
    assert_eq!(item(&deleted, "connection")["startBinding"], Value::Null);
    assert_eq!(
        item(&deleted, "connection")["endBinding"],
        item(&original, "connection")["endBinding"]
    );
    assert_eq!(deleted["files"], original["files"]);
    assert_eq!(deleted["future"], original["future"]);
}

#[test]
fn no_op_gestures_keep_original_bytes_and_failed_compositions_never_publish_partial_changes() {
    let original = fixture();
    let source = format!("\u{feff}{}\n", original);
    let scene = Scene::parse(&source).unwrap();
    assert!(
        scene
            .apply(Change::Move {
                ids: vec!["capture".into()],
                by: [0.0; 2]
            })
            .unwrap()
            .is_none()
    );
    assert!(
        scene
            .apply(Change::Paint {
                ids: vec!["capture".into()],
                paint: Paint::Width(item(&original, "capture")["strokeWidth"].as_f64().unwrap())
            })
            .unwrap()
            .is_none()
    );
    assert!(
        scene
            .apply(Change::Move {
                ids: vec!["connection".into()],
                by: [10.0, 0.0]
            })
            .unwrap()
            .is_none()
    );
    assert_eq!(scene.source(), source);
    let mut locked = original.clone();
    locked["elements"][2]["locked"] = json!(true);
    let scene = Scene::parse(&locked.to_string()).unwrap();
    assert!(
        scene
            .apply(Change::Move {
                ids: vec!["capture".into()],
                by: [20.0, 0.0]
            })
            .is_err()
    );
    assert_eq!(scene.document.as_object(), locked.as_object().unwrap());
}

#[test]
fn native_creation_and_path_validation_preserve_existing_scene_data() {
    let original = fixture();
    let scene = Scene::parse(&original.to_string()).unwrap();
    let change = Change::Create {
        id: "new-line".into(),
        kind: Kind::Arrow,
        bounds: Bounds {
            x: 10.0,
            y: 20.0,
            width: 20.0,
            height: 30.0,
            angle: 0.0,
        },
        points: vec![[0.0, 0.0], [-20.0, 30.0]],
        style: Style::default(),
    };
    let added: Value =
        serde_json::from_str(&scene.apply(change.clone()).unwrap().unwrap()).unwrap();
    assert_eq!(item(&added, "new-line")["width"].as_f64(), Some(20.0));
    assert_eq!(item(&added, "new-line")["endArrowhead"], json!("arrow"));
    assert_eq!(added["future"], original["future"]);
    assert_eq!(added["files"], original["files"]);
    assert!(
        Scene::parse(&added.to_string())
            .unwrap()
            .apply(change)
            .is_err()
    );
    let invalid = Change::Create {
        id: "bad".into(),
        kind: Kind::Freedraw,
        bounds: Bounds {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            angle: 0.0,
        },
        points: vec![[1.0, 0.0], [2.0, 1.0]],
        style: Style::default(),
    };
    assert!(scene.apply(invalid).is_err());
}

#[test]
fn source_budgets_duplicate_ids_and_nonfinite_transforms_are_rejected() {
    assert!(Scene::parse(&" ".repeat(super::super::MAX_DRAWING_SOURCE_BYTES + 1)).is_err());
    let mut value = fixture();
    value["elements"][1]["id"] = json!("capture");
    assert!(Scene::parse(&value.to_string()).is_err());
    let scene = Scene::parse(&fixture().to_string()).unwrap();
    assert!(
        scene
            .apply(Change::Move {
                ids: vec!["capture".into()],
                by: [f64::NAN, 0.0]
            })
            .is_err()
    );
}

#[test]
fn rotated_geometry_round_trips_and_quarter_turn_has_the_expected_corners() {
    let mut bounds = Bounds {
        x: 20.0,
        y: 30.0,
        width: 100.0,
        height: 60.0,
        angle: std::f64::consts::FRAC_PI_2,
    };
    let corner = bounds.to_world([0.0, 0.0]);
    assert!((corner[0] - 100.0).abs() < 1e-8 && (corner[1] - 10.0).abs() < 1e-8);
    for angle in [-1.0, 0.0, 0.7, 3.1] {
        bounds.angle = angle;
        let local = [17.0, 42.0];
        let restored = bounds.to_local(bounds.to_world(local));
        assert!((restored[0] - local[0]).abs() < 1e-8);
        assert!((restored[1] - local[1]).abs() < 1e-8);
    }
}

#[test]
fn live_movement_matches_committed_bound_endpoints_and_labels() {
    let original = fixture();
    let scene = Scene::parse(&original.to_string()).unwrap();
    for ids in [
        vec!["capture".into()],
        vec!["capture".into(), "explore".into()],
    ] {
        let by = [-75.0, 43.0];
        let preview = scene.movement(&ids, by).unwrap();
        let committed = apply(&original, Change::Move { ids, by });
        for element in &scene.items {
            let value = item(&committed, &element.id);
            if let Some(delta) = preview.offsets.get(&element.id) {
                assert_eq!(value["x"].as_f64(), Some(element.bounds.x + delta[0]));
                assert_eq!(value["y"].as_f64(), Some(element.bounds.y + delta[1]));
            }
            if let Some(path) = preview.paths.get(&element.id) {
                assert_eq!(value["x"].as_f64(), Some(path.origin[0]));
                assert_eq!(value["y"].as_f64(), Some(path.origin[1]));
                assert_eq!(value["points"], json!(path.points));
            }
        }
        assert_eq!(scene.source(), original.to_string());
    }
}
