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

fn text_content(text: &str, width: f64, height: f64) -> TextContent {
    TextContent::plain(
        text,
        Number::from_f64(width).unwrap(),
        Number::from_f64(height).unwrap(),
    )
}

#[test]
fn text_replacement_preserves_fonts_bindings_resources_and_opaque_neighbours() {
    let original = fixture();
    let edited = apply(
        &original,
        Change::Text {
            id: "capture-label".into(),
            content: text_content("New label", 100.0, 30.0),
        },
    );
    let label = item(&edited, "capture-label");
    assert_eq!(label["text"], json!("New label"));
    assert_eq!(label["originalText"], json!("New label"));
    for key in [
        "containerId",
        "fontFamily",
        "fontSize",
        "lineHeight",
        "textAlign",
        "verticalAlign",
    ] {
        assert_eq!(label[key], item(&original, "capture-label")[key]);
    }
    assert_eq!(item(&edited, "capture"), item(&original, "capture"));
    assert_eq!(edited["files"], original["files"]);
    assert_eq!(edited["future"], original["future"]);
    assert_eq!(
        edited["elements"].as_array().unwrap().last(),
        Some(&json!(17))
    );
    let scene = Scene::parse(&original.to_string()).unwrap();
    assert!(
        scene
            .apply(Change::Text {
                id: "capture-label".into(),
                content: text_content("Too wide", 500.0, 30.0)
            })
            .is_err()
    );
    assert!(
        scene
            .apply(Change::Text {
                id: "capture-label".into(),
                content: text_content("Capture", 500.0, 30.0)
            })
            .unwrap()
            .is_none()
    );
    assert_eq!(scene.source(), original.to_string());
}

#[test]
fn text_creation_keeps_existing_records_and_maintains_reciprocal_new_label_identity() {
    let source = r#"{"type":"excalidraw","version":2,"elements":[{"id":"box","type":"rectangle","x":0,"y":0,"width":200,"height":100},{"future":true}],"files":{"opaque":{"future":17}}}"#;
    let scene = Scene::parse(source).unwrap();
    let change = Change::CreateText(TextCreation {
        id: "label".into(),
        position: [50.0, 35.0],
        content: text_content("Label", 100.0, 30.0),
        font: TextStyle::default(),
        style: Style::default(),
        container: Some("box".into()),
    });
    let value: Value = serde_json::from_str(&scene.apply(change).unwrap().unwrap()).unwrap();
    assert_eq!(item(&value, "label")["containerId"], json!("box"));
    assert_eq!(
        item(&value, "box")["boundElements"],
        json!([{"id":"label","type":"text"}])
    );
    assert_eq!(value["elements"][1], json!({"future":true}));
    assert_eq!(
        value["files"],
        serde_json::from_str::<Value>(source).unwrap()["files"]
    );
    assert!(
        Scene::parse(&value.to_string())
            .unwrap()
            .text_info("box")
            .unwrap()
            .is_some()
    );
    let created = apply(
        &value,
        Change::CreateText(TextCreation {
            id: "free-text".into(),
            position: [-50.0, 120.0],
            content: text_content("Unbound", 100.0, 30.0),
            font: TextStyle::default(),
            style: Style::default(),
            container: None,
        }),
    );
    assert!(item(&created, "free-text")["containerId"].is_null());
}

#[test]
fn rectangular_resize_keeps_label_anchor_and_attached_arrow_gap_and_other_endpoint() {
    let original = fixture();
    let scene = Scene::parse(&original.to_string()).unwrap();
    let bounds = Bounds {
        x: 0.0,
        y: 40.0,
        width: 220.0,
        height: 130.0,
        angle: 0.0,
    };
    let plan = scene.resize("capture", bounds).unwrap();
    assert_eq!(plan.movement.paths["connection"].origin, [222.0, 105.0]);
    let resized = apply(
        &original,
        Change::Resize {
            id: "capture".into(),
            bounds,
        },
    );
    assert_eq!(item(&resized, "capture")["width"], json!(220.0));
    assert_eq!(
        item(&resized, "capture-label"),
        item(&original, "capture-label")
    );
    let arrow = item(&resized, "connection");
    assert_eq!(arrow["x"], json!(222.0));
    assert_eq!(arrow["y"], json!(105.0));
    assert_eq!(arrow["points"][1], json!([76.0, -10.0]));
    for key in ["startBinding", "endBinding"] {
        assert_eq!(arrow[key], item(&original, "connection")[key]);
    }
    assert_eq!(item(&resized, "explore"), item(&original, "explore"));
    assert_eq!(resized["future"], original["future"]);
}

#[test]
fn image_crop_flip_assets_and_freehand_pressures_survive_resize() {
    let mut original = fixture();
    let image = original["elements"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|value| value["id"] == "embedded-image")
        .unwrap();
    image["crop"] =
        json!({"x":0.1,"y":0.2,"width":0.7,"height":0.6,"naturalWidth":200,"naturalHeight":100});
    image["scale"] = json!([-1, 1]);
    let resized = apply(
        &original,
        Change::Resize {
            id: "embedded-image".into(),
            bounds: Bounds {
                x: 300.0,
                y: 180.0,
                width: 256.0,
                height: 192.0,
                angle: 0.0,
            },
        },
    );
    for key in ["crop", "scale", "fileId", "status"] {
        assert_eq!(
            item(&resized, "embedded-image")[key],
            item(&original, "embedded-image")[key]
        );
    }
    assert_eq!(resized["files"], original["files"]);
    let resized = apply(
        &original,
        Change::Resize {
            id: "stroke".into(),
            bounds: Bounds {
                x: 0.0,
                y: 205.0,
                width: 300.0,
                height: 90.0,
                angle: 0.0,
            },
        },
    );
    for (new, old) in item(&resized, "stroke")["points"]
        .as_array()
        .unwrap()
        .iter()
        .zip(item(&original, "stroke")["points"].as_array().unwrap())
    {
        assert_eq!(new[0].as_f64(), Some(old[0].as_f64().unwrap() * 2.0));
        assert_eq!(new[1].as_f64(), Some(old[1].as_f64().unwrap() * 2.0));
    }
    assert_eq!(
        item(&resized, "stroke")["pressures"],
        item(&original, "stroke")["pressures"]
    );
}

#[test]
fn proportional_text_resize_updates_font_once_and_refuses_unsupported_operations_atomically() {
    let original = fixture();
    let resized = apply(
        &original,
        Change::Resize {
            id: "caption".into(),
            bounds: Bounds {
                x: 0.0,
                y: 285.0,
                width: 740.0,
                height: 60.0,
                angle: 0.0,
            },
        },
    );
    assert_eq!(item(&resized, "caption")["fontSize"], json!(48.0));
    assert_eq!(
        item(&resized, "caption")["text"],
        item(&original, "caption")["text"]
    );
    assert_eq!(item(&resized, "caption")["version"], json!(2));
    let source = original.to_string();
    let scene = Scene::parse(&source).unwrap();
    for (id, bounds) in [
        (
            "caption",
            Bounds {
                x: 0.0,
                y: 285.0,
                width: 740.0,
                height: 30.0,
                angle: 0.0,
            },
        ),
        (
            "connection",
            Bounds {
                x: 182.0,
                y: 95.0,
                width: 150.0,
                height: 20.0,
                angle: 0.0,
            },
        ),
        (
            "explore",
            Bounds {
                x: 300.0,
                y: 40.0,
                width: 200.0,
                height: 120.0,
                angle: 0.0,
            },
        ),
        (
            "capture",
            Bounds {
                x: 0.0,
                y: 40.0,
                width: 10.0,
                height: 10.0,
                angle: 0.0,
            },
        ),
    ] {
        assert!(
            scene
                .apply(Change::Resize {
                    id: id.into(),
                    bounds
                })
                .is_err(),
            "{id}"
        );
    }
    assert_eq!(scene.source(), source);
}
