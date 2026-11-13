use super::resolve::SemanticWorkingRecord;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn equal_source_owners_have_a_total_semantic_release_order() {
    let make = || {
        [
            ("/elements/0/frameId", "relationship_edge", 64),
            ("/elements/0", "resolved_text_run", 320),
            (
                "/elements/0",
                "resolved_text_layout_and_glyph_geometry",
                1024,
            ),
            ("/elements/0/link", "relationship_edge", 64),
            ("/elements/0", "resolved_text_run", 640),
        ]
        .into_iter()
        .map(|(pointer, event, bytes)| SemanticWorkingRecord {
            source_order: 1,
            event,
            bytes,
            json_pointer: pointer.into(),
            element_id: Some("owner".into()),
            local_owner: Some("owner".into()),
        })
        .collect::<Vec<_>>()
    };
    let mut forward = make();
    let mut reverse = make();
    reverse.reverse();
    forward.sort_unstable_by(SemanticWorkingRecord::release_order);
    reverse.sort_unstable_by(SemanticWorkingRecord::release_order);
    for (a, b) in forward.iter().zip(&reverse) {
        assert_eq!(a.release_order(b), std::cmp::Ordering::Equal);
    }
    for pair in forward.windows(2) {
        assert_eq!(pair[0].release_order(&pair[1]), std::cmp::Ordering::Less);
    }
    assert_eq!(forward[0].event, "resolved_text_layout_and_glyph_geometry");
    assert_eq!(forward[1].bytes, 320);
    assert_eq!(forward[2].bytes, 640);
    assert_eq!(forward[3].json_pointer, "/elements/0/frameId");
    assert_eq!(forward[4].json_pointer, "/elements/0/link");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn release_order_distinguishes_source_and_optional_owners() {
    let make = |source_order, element_id, local_owner| SemanticWorkingRecord {
        source_order,
        event: "relationship_edge",
        json_pointer: "/elements/0".into(),
        bytes: 64,
        element_id,
        local_owner,
    };
    let mut records = [
        make(2, None, None),
        make(1, Some("b".into()), None),
        make(1, Some("a".into()), Some("z".into())),
        make(1, Some("a".into()), None),
        make(1, None, None),
        make(1, None, None),
    ];
    records.sort_unstable_by(SemanticWorkingRecord::release_order);
    let observed: Vec<_> = records
        .iter()
        .map(|r| {
            (
                r.source_order,
                r.element_id.as_deref(),
                r.local_owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            (1, None, None),
            (1, None, None),
            (1, Some("a"), None),
            (1, Some("a"), Some("z")),
            (1, Some("b"), None),
            (2, None, None)
        ]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn bound_text_release_events_follow_pointer_then_event_order() {
    let scene = crate::SceneDocument::parse(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/conformance/cases/focused/bound-text-containers/scene.excalidraw"
    )))
    .unwrap();
    let request = crate::RenderRequest::builder(crate::OutputFormat::Png)
        .build()
        .unwrap();
    let (result, trace) =
        super::render_with_trace(crate::catalog::REVISION_1.profile(), &scene, &request);
    result.unwrap();
    let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
    let events: Vec<_> = trace["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["phase"] == "format_planning"
                && e["decision"] == "released"
                && e["element_id"] == "rectangle-right-label"
        })
        .map(|e| {
            (
                e["json_pointer"].as_str().unwrap(),
                e["event"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(
        events.len() >= 3,
        "the regression must observe multiple release records"
    );
    assert!(
        events.windows(2).all(|p| p[0] <= p[1]),
        "release order: {events:?}"
    );
}
