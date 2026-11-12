//! Unit tests for normalization at its own seam: the diagnostics revision-1
//! normalization emits for a scene, in report order, and the normalized values
//! the reference renderer's restore path would produce, asserted through
//! `reported_diagnostics` and `normalize_scene` without a render.

use super::{
    codes_and_pointers, normalize_scene, pointers, reported_diagnostics, NormalizationOutcome,
    NormalizedElementKind, NormalizedRoundness,
};
use crate::outcome::{DiagnosticAttribution, DiagnosticOrder, PendingDiagnostic};
use crate::{
    CompatibilityDiagnostic, CompatibilityRevision, DiagnosticCode, DiagnosticEffect, OutputFormat,
    SceneDocument, SupplementalAssets,
};

/// A diagnostic `normalize_scene` emitted as its `ordinal`th, retained in
/// emission order without a source owner, so a test can read it as the report
/// would carry it.
fn emitted(diagnostic: PendingDiagnostic, ordinal: usize) -> CompatibilityDiagnostic {
    diagnostic.retained(DiagnosticOrder::retained(
        None,
        None,
        u64::try_from(ordinal).expect("emission ordinal fits"),
        DiagnosticAttribution::SceneGlobal,
    ))
}

/// The diagnostics a default SVG render of `source` reports from
/// normalization, in report order, with the normalized outcome.
fn reported(source: &[u8]) -> (Vec<CompatibilityDiagnostic>, NormalizationOutcome) {
    reported_for(source, OutputFormat::Svg, true)
}

/// [`reported`] for `format`, with the request's frame-name control set
/// to `frame_names`.
fn reported_for(
    source: &[u8],
    format: OutputFormat,
    frame_names: bool,
) -> (Vec<CompatibilityDiagnostic>, NormalizationOutcome) {
    let scene = SceneDocument::parse(source).expect("normalization vector is admitted");
    reported_diagnostics(&scene, format, frame_names, &SupplementalAssets::default())
}

#[test]
fn revision_1_identity_timestamp_and_seed_repairs_are_source_located() {
    let (diagnostics, normalized) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"type":"rectangle","width":180,"height":100,"seed":0}],"appState":{},"files":{}}"##,
    );
    assert_eq!(
        codes_and_pointers(&diagnostics),
        [
            (
                DiagnosticCode::NORMALIZATION_MISSING_ELEMENT_ID,
                "/elements/0/id"
            ),
            (
                DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
                "/elements/0/seed"
            ),
            (
                DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
                "/elements/0/updated"
            ),
        ]
    );
    assert_eq!(diagnostics[0].effects(), &[DiagnosticEffect::InputRepair]);
    assert_eq!(
        diagnostics[1].effects(),
        &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual]
    );
    assert_eq!(diagnostics[2].effects(), &[DiagnosticEffect::InputRepair]);
    // The missing identity repair exposes the normalized identity, and
    // every repair on the element is attributed to it.
    let generated_id = "generated-32182208bba6af9fdf6995f59ee6a4097851c4655cf10d9e0681746fc495b792";
    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic.element_id() == Some(generated_id)));
    for diagnostic in &diagnostics {
        let value = serde_json::to_value(diagnostic).expect("diagnostic serializes");
        assert_eq!(
            value["details"],
            serde_json::json!({
                "kind": "compatibility",
                "recovery": "repaired",
                "recipe_id": null,
            })
        );
    }
    // The repaired values are the ones an already-normalized scene would
    // carry: the generated identity and the pinned seed for it. A scene
    // that spells them out normalizes to the same element and repairs
    // nothing.
    assert_eq!(normalized.elements[0].source.id(), generated_id);
    assert_eq!(normalized.elements[0].seed, 642_336_616);
    let (diagnostics, already_normalized) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"generated-32182208bba6af9fdf6995f59ee6a4097851c4655cf10d9e0681746fc495b792","type":"rectangle","width":180,"height":100,"seed":642336616,"updated":0}],"appState":{},"files":{}}"##,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(already_normalized.elements[0].source.id(), generated_id);
    assert_eq!(
        already_normalized.elements[0].seed,
        normalized.elements[0].seed
    );
    assert_eq!(
        already_normalized.elements[0].kind,
        normalized.elements[0].kind
    );
}

#[test]
fn repair_diagnostics_carry_the_format_they_were_produced_for() {
    let source = br##"{"type":"excalidraw","version":2,"elements":[{"type":"rectangle","width":30,"height":20,"seed":0}],"appState":{},"files":{}}"##;
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let (diagnostics, _) = reported_for(source, format, true);
        assert_eq!(diagnostics.len(), 3, "{format:?}");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.format() == format),
            "{format:?}"
        );
    }
}

#[test]
fn revision_1_rekeys_each_later_duplicate_identity_in_source_order() {
    let (diagnostics, normalized) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"duplicate","type":"rectangle","x":0,"y":0,"width":80,"height":60,"seed":1,"updated":1},{"id":"duplicate","type":"rectangle","x":100,"y":0,"width":80,"height":60,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    );
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("the later duplicate produces exactly one repair diagnostic: {diagnostics:?}");
    };
    assert_eq!(
        diagnostic.code(),
        DiagnosticCode::NORMALIZATION_DUPLICATE_ELEMENT_ID
    );
    assert_eq!(diagnostic.json_pointer(), "/elements/1/id");
    assert_eq!(
        diagnostic.element_id(),
        Some("generated-3180035c77cb940ec4c79c4729441c64bad5072d494bd981721e7e1b9c85c3a3")
    );
    assert_eq!(diagnostic.effects(), &[DiagnosticEffect::InputRepair]);
    // Both elements survive: the first keeps its identity.
    assert_eq!(
        normalized
            .elements
            .iter()
            .map(|element| element.source.id())
            .collect::<Vec<_>>(),
        [
            "duplicate",
            "generated-3180035c77cb940ec4c79c4729441c64bad5072d494bd981721e7e1b9c85c3a3",
        ]
    );
}

#[test]
fn generated_identity_collision_is_rekeyed_without_displacing_the_first_owner() {
    // The first element already carries the identity the second would
    // generate, so the second is rekeyed once more and the first is left
    // alone: one diagnostic, on the element that had no identity.
    let (diagnostics, normalized) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"generated-d02dc903e43157ace063528d673d5657c6e6433aa467e643d1b7a2cb9296386a","type":"rectangle","x":0,"width":40,"height":40,"seed":1,"updated":1},{"type":"rectangle","x":100,"width":40,"height":40,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    );
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("the missing identity produces one repair diagnostic: {diagnostics:?}");
    };
    assert_eq!(
        diagnostic.code(),
        DiagnosticCode::NORMALIZATION_MISSING_ELEMENT_ID
    );
    assert_eq!(diagnostic.json_pointer(), "/elements/1/id");
    assert_eq!(
        diagnostic.element_id(),
        Some("generated-604f36b7127cb8ae0e242c4cf18cbb0f437f585790ef41c6f0edaa07a7676e63")
    );
    assert_eq!(
        normalized.elements[0].source.id(),
        "generated-d02dc903e43157ace063528d673d5657c6e6433aa467e643d1b7a2cb9296386a"
    );
}

#[test]
fn invalid_frame_rendering_controls_are_repaired_for_empty_scenes() {
    let (diagnostics, normalized) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[],"appState":{"frameRendering":{"enabled":false,"clip":"yes"}},"files":{}}"##,
    );
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("the invalid frame control produces one diagnostic: {diagnostics:?}");
    };
    assert_eq!(
        diagnostic.code(),
        DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE
    );
    assert_eq!(diagnostic.json_pointer(), "/appState/frameRendering/clip");
    assert_eq!(
        diagnostic.effects(),
        &[DiagnosticEffect::InputRepair, DiagnosticEffect::Visual]
    );
    assert!(!normalized.frame_rendering.enabled);
    assert!(
        normalized.frame_rendering.clip,
        "the invalid clip is defaulted"
    );
}

#[test]
fn assigned_element_repairs_are_reported_before_later_rendering_slices() {
    let (diagnostics, _) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","width":10,"height":10,"points":"invalid","seed":1,"updated":1},{"id":"image","type":"image","width":10,"height":10,"status":"future","seed":1,"updated":1}],"appState":{},"files":{}}"##,
    );
    assert_eq!(
        pointers(&diagnostics),
        [
            "/elements/0/points",
            "/elements/1/status",
            "/elements/1/fileId",
        ]
    );
}

#[test]
fn frame_name_diagnostics_follow_the_requested_frame_name_control() {
    // With frame names disabled by the request, a malformed `name` is
    // never read and so never diagnosed, whatever the scene's own
    // `frameRendering.name` says.
    let name_only = br##"{"type":"excalidraw","version":2,"elements":[{"id":"frame","type":"frame","width":10,"height":10,"name":3,"seed":1,"updated":1}],"appState":{},"files":{}}"##;
    let (diagnostics, _) = reported_for(name_only, OutputFormat::Svg, false);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    // Scene and request controls both gate participation: a request cannot
    // enable name validation when persisted name rendering is disabled.
    let scene_disabled = br##"{"type":"excalidraw","version":2,"elements":[{"id":"frame","type":"frame","width":10,"height":10,"name":3,"seed":1,"updated":1}],"appState":{"frameRendering":{"name":false}},"files":{}}"##;
    let (diagnostics, _) = reported_for(scene_disabled, OutputFormat::Svg, true);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn normalization_reports_follow_preserved_root_owner_order() {
    // `files` precedes `elements` in this document, so the file record's
    // repairs come first although the element references the file.
    let (diagnostics, _) = reported(
        br##"{"type":"excalidraw","version":2,"files":{"asset":{}},"elements":[{"id":"image","type":"image","width":10,"height":10,"fileId":"asset","status":"future","seed":1,"updated":1}],"appState":{}}"##,
    );
    assert_eq!(
        pointers(&diagnostics),
        [
            "/files/asset/id",
            "/files/asset/dataURL",
            "/files/asset/created",
            "/elements/0/status",
        ]
    );
}

#[test]
fn normalization_diagnostics_follow_total_source_and_field_order() {
    let (diagnostics, _) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"type":"rectangle","x":0,"width":40,"height":40,"seed":0},{"id":"same","type":"rectangle","x":50,"width":40,"height":40,"seed":0},{"id":"same","type":"rectangle","x":100,"width":40,"height":40,"seed":0}],"appState":{},"files":{}}"##,
    );
    assert_eq!(
        codes_and_pointers(&diagnostics),
        [
            (
                DiagnosticCode::NORMALIZATION_MISSING_ELEMENT_ID,
                "/elements/0/id",
            ),
            (
                DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
                "/elements/0/seed",
            ),
            (
                DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
                "/elements/0/updated",
            ),
            (
                DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
                "/elements/1/seed",
            ),
            (
                DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
                "/elements/1/updated",
            ),
            (
                DiagnosticCode::NORMALIZATION_DUPLICATE_ELEMENT_ID,
                "/elements/2/id",
            ),
            (
                DiagnosticCode::NORMALIZATION_ZERO_ROUGH_SEED,
                "/elements/2/seed",
            ),
            (
                DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
                "/elements/2/updated",
            ),
        ]
    );
}

#[test]
fn normalization_order_respects_source_member_position() {
    // `seed` is spelled before `id` in the second element, so its repair
    // is reported first although identity is repaired first.
    let (diagnostics, _) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"same","type":"rectangle","x":0,"width":40,"height":40,"seed":1,"updated":1},{"seed":0,"id":"same","type":"rectangle","x":50,"width":40,"height":40,"updated":1}],"appState":{},"files":{}}"##,
    );
    assert_eq!(
        pointers(&diagnostics),
        ["/elements/1/seed", "/elements/1/id"]
    );
}

#[test]
fn app_state_diagnostics_follow_source_member_order_across_surface_and_normalization() {
    // The unmatched members come from the scene-surface classification and
    // the frame-rendering control from compatibility normalization; spec
    // §11.3 orders them by the `appState` member they belong to regardless
    // of which layer reported them. Found by the scene fuzz.
    let (diagnostics, _) = reported(
        br##"{"type":"excalidraw","version":2,"elements":[],"appState":{"frameRendering":{"clip":"yes"},"alpha":1,"beta":2},"files":{}}"##,
    );
    assert_eq!(
        pointers(&diagnostics),
        [
            "/appState/frameRendering/clip",
            "/appState/alpha",
            "/appState/beta",
        ]
    );
}

#[test]
fn linear_points_move_the_first_point_to_the_local_origin() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","x":30,"y":40,"width":20,"height":20,"points":[[3,4],[5,6]],"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("linear normalization vector is admitted");

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let [line] = normalized.elements.as_slice() else {
        panic!("one linear element is normalized");
    };
    let NormalizedElementKind::Linear(line_kind) = &line.kind else {
        panic!("the line retains its normalized linear kind");
    };

    assert_eq!((line.x, line.y), (33.0, 44.0));
    assert_eq!((line.width, line.height), (2.0, 2.0));
    assert_eq!(line_kind.points, [[0.0, 0.0], [2.0, 2.0]]);
}

#[test]
fn linear_point_restore_precedes_common_negative_dimension_restore() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","x":30,"y":40,"width":-20,"height":-10,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("negative linear normalization vector is admitted");

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let line = &normalized.elements[0];
    let NormalizedElementKind::Linear(line_kind) = &line.kind else {
        panic!("the line retains normalized linear state");
    };

    assert_eq!((line.x, line.y), (30.0, 40.0));
    assert_eq!((line.width, line.height), (20.0, 10.0));
    assert_eq!(line_kind.points, [[0.0, 0.0], [-20.0, -10.0]]);
}

#[test]
fn short_linear_point_arrays_use_the_silent_pinned_migration() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","width":20,"height":10,"points":[[1,1]],"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("short linear-point vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(line) = &normalized.elements[0].kind else {
        panic!("the line retains normalized linear state");
    };

    assert_eq!(line.points, [[0.0, 0.0], [20.0, 10.0]]);
    assert!(diagnostics.is_empty());
}

#[test]
fn ordered_nonempty_legacy_indexes_remain_anchored() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"first","index":"foo","type":"rectangle","width":10,"height":10,"seed":1,"updated":1},{"id":"second","index":"goo","type":"rectangle","width":10,"height":10,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("legacy index vector is admitted");

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    assert_eq!(normalized.elements[0].index, "foo");
    assert_eq!(normalized.elements[1].index, "goo");
}

#[test]
fn elbow_bindings_and_arrow_defaults_match_the_pinned_restore_path() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"arrow","type":"arrow","x":0,"y":0,"width":100,"height":80,"points":[[0,0],[100,0],[100,80]],"elbowed":true,"startBinding":{"elementId":"target","gap":1,"fixedPoint":[0.5,0.49991]},"endBinding":{"elementId":"target","focus":0,"gap":1},"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("elbow normalization vector is admitted");

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(arrow) = &normalized.elements[0].kind else {
        panic!("the arrow retains its normalized linear kind");
    };

    assert_eq!(arrow.kind, super::NormalizedLinearKind::Arrow);
    assert_eq!(arrow.start_arrowhead, None);
    assert_eq!(arrow.end_arrowhead.as_deref(), Some("arrow"));
    assert_eq!(arrow.last_committed_point, None);
    assert_eq!(
        arrow.start_binding.as_ref().map(|binding| (
            binding.element_id.as_str(),
            binding.focus,
            binding.gap,
            binding.fixed_point,
        )),
        Some(("target", 0.0, 1.0, Some([0.5001, 0.5001])))
    );
    assert_eq!(arrow.end_binding, None);
}

#[test]
fn later_editor_bindings_keep_their_target_and_report_editor_state_as_info() {
    // Excalidraw after 0.18.1 persists `{elementId, mode, fixedPoint}`
    // without `focus` or `gap`. The pinned restore keeps such a binding; its
    // `mode` and non-elbow `fixedPoint` are editor state no export reads.
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"arrow","type":"arrow","width":10,"height":10,"points":[[0,0],[10,10]],"startBinding":{"mode":"orbit","elementId":"target","fixedPoint":[1,0.5001]},"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("later binding vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(arrow) = &normalized.elements[0].kind else {
        panic!("the arrow retains its normalized linear kind");
    };

    assert_eq!(
        arrow.start_binding.as_ref().map(|binding| (
            binding.element_id.as_str(),
            binding.focus,
            binding.gap,
            binding.fixed_point,
        )),
        Some(("target", 0.0, 0.0, None))
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| (
                diagnostic.json_pointer(),
                diagnostic.severity(),
                diagnostic.effects().to_vec(),
            ))
            .collect::<Vec<_>>(),
        [
            (
                "/elements/0/startBinding/mode",
                crate::DiagnosticSeverity::Info,
                vec![crate::DiagnosticEffect::InputRepair],
            ),
            (
                "/elements/0/startBinding/fixedPoint",
                crate::DiagnosticSeverity::Info,
                vec![crate::DiagnosticEffect::InputRepair],
            ),
        ]
    );
}

#[test]
fn image_frame_and_frame_rendering_state_is_normalized_without_mutating_source() {
    let source = br##"{"type":"excalidraw","version":2,"elements":[{"id":"image","type":"image","width":80,"height":60,"fileId":"asset","status":"saved","scale":[-1,1],"crop":{"x":1,"y":2,"width":30,"height":20,"naturalWidth":60,"naturalHeight":40},"seed":1,"updated":1},{"id":"frame","type":"frame","width":100,"height":100,"name":"Named","seed":1,"updated":1},{"id":"magic","type":"magicframe","width":100,"height":100,"seed":1,"updated":1}],"appState":{"frameRendering":{"enabled":false,"clip":true}},"files":{"asset":{"id":"asset","created":1}}}"##;
    let scene = SceneDocument::parse(source).expect("state normalization vector is admitted");
    let canonical_before = scene
        .canonicalized_original_bytes()
        .expect("the original scene serializes");

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    let NormalizedElementKind::Image(image) = &normalized.elements[0].kind else {
        panic!("the image retains normalized image state");
    };
    assert_eq!(image.file_id.as_deref(), Some("asset"));
    assert_eq!(image.status, super::NormalizedImageStatus::Saved);
    assert_eq!(image.scale, [-1, 1]);
    assert_eq!(
        image.crop,
        Some(super::NormalizedImageCrop {
            x: 1.0,
            y: 2.0,
            width: 30.0,
            height: 20.0,
            natural_width: 60.0,
            natural_height: 40.0,
        })
    );
    let NormalizedElementKind::Frame(frame) = &normalized.elements[1].kind else {
        panic!("the frame retains normalized frame state");
    };
    let NormalizedElementKind::MagicFrame(magic) = &normalized.elements[2].kind else {
        panic!("the magic frame retains normalized frame state");
    };
    assert_eq!(
        frame.name, None,
        "disabled name is retained only in the original scene"
    );
    assert_eq!(magic.name, None);
    assert_eq!(
        normalized.frame_rendering,
        super::NormalizedFrameRendering {
            enabled: false,
            clip: true,
            name: true,
            outline: true,
        }
    );
    assert_eq!(
        scene
            .canonicalized_original_bytes()
            .expect("the original scene still serializes"),
        canonical_before
    );
}

#[test]
fn freedraw_state_uses_closed_defaults_and_retains_valid_samples() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"free","type":"freedraw","width":10,"height":10,"points":[[0,0],[10,10]],"pressures":[0.25,0.75],"simulatePressure":false,"lastCommittedPoint":[10,10],"seed":1,"updated":1},{"id":"defaults","type":"freedraw","width":10,"height":10,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("freedraw normalization vector is admitted");

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Freedraw(explicit) = &normalized.elements[0].kind else {
        panic!("the explicit freedraw state is normalized");
    };
    let NormalizedElementKind::Freedraw(defaulted) = &normalized.elements[1].kind else {
        panic!("the defaulted freedraw state is normalized");
    };

    assert_eq!(explicit.points, [[0.0, 0.0], [10.0, 10.0]]);
    assert_eq!(explicit.pressures, [0.25, 0.75]);
    assert!(!explicit.simulate_pressure);
    assert_eq!(explicit.last_committed_point, None);
    assert!(defaulted.points.is_empty());
    assert!(defaulted.pressures.is_empty());
    assert!(defaulted.simulate_pressure);
}

#[test]
fn malformed_assigned_fields_emit_source_located_substitution_diagnostics() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","width":10,"height":10,"points":"invalid","seed":1,"updated":1},{"id":"image","type":"image","width":10,"height":10,"status":"loading","scale":[0,1],"crop":"invalid","seed":1,"updated":1},{"id":"frame","type":"frame","width":10,"height":10,"name":3,"seed":1,"updated":1}],"appState":{"frameRendering":{"clip":"yes"}},"files":{}}"##,
    )
    .expect("malformed normalization vector is admitted");
    let mut diagnostics = Vec::new();

    normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code(), diagnostic.json_pointer()))
            .collect::<Vec<_>>(),
        [
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/points",
            ),
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/1/status",
            ),
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/1/scale",
            ),
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/1/crop",
            ),
            (DiagnosticCode::IMAGE_MISSING_FILE, "/elements/1/fileId",),
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/2/name",
            ),
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/appState/frameRendering/clip",
            ),
        ]
    );
    assert!(diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() != DiagnosticCode::IMAGE_MISSING_FILE)
        .all(|diagnostic| {
            serde_json::to_value(diagnostic).expect("diagnostic serializes")["details"]["recovery"]
                == "substituted"
        }));
    assert_eq!(
        serde_json::to_value(
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.code() == DiagnosticCode::IMAGE_MISSING_FILE)
                .expect("missing image diagnostic is retained"),
        )
        .expect("diagnostic serializes")["details"]["recovery"],
        "omitted"
    );
}

#[test]
fn only_referenced_file_records_receive_deterministic_record_repairs() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"image","type":"image","width":10,"height":10,"fileId":"asset","seed":1,"updated":1}],"appState":{},"files":{"asset":{"mimeType":"image/png","dataURL":"data:image/png;base64,AA=="},"unused":{}}}"##,
    )
    .expect("file-record normalization vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    assert_eq!(normalized.files.len(), 1);
    assert_eq!(normalized.files[0].key, "asset");
    assert_eq!(normalized.files[0].id, "asset");
    assert_eq!(normalized.files[0].created, Some(0));
    assert_eq!(normalized.files[0].source_pointer, "/files/asset");
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code(), diagnostic.json_pointer()))
            .collect::<Vec<_>>(),
        [
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/files/asset/id",
            ),
            (
                DiagnosticCode::NORMALIZATION_MISSING_TIMESTAMP,
                "/files/asset/created",
            ),
        ]
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic.element_id().is_none()));
}

#[test]
fn invalid_file_created_values_remain_ignored() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"image","type":"image","width":10,"height":10,"fileId":"asset","seed":1,"updated":1}],"appState":{},"files":{"asset":{"id":"asset","created":"invalid","dataURL":"data:image/png;base64,AA=="}}}"##,
    )
    .expect("invalid file timestamp vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    assert_eq!(normalized.files[0].created, None);
    assert!(diagnostics.is_empty());
}

#[test]
fn malformed_elbow_binding_members_use_their_narrowest_recovery() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"arrow","type":"arrow","width":10,"height":10,"points":[[0,0],[10,10]],"elbowed":true,"startBinding":{"elementId":"target","gap":1,"fixedPoint":"invalid"},"endBinding":{"elementId":"target","gap":"wide","fixedPoint":[0,0]},"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("binding recovery vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(arrow) = &normalized.elements[0].kind else {
        panic!("the arrow retains normalized linear state");
    };

    assert_eq!(
        arrow
            .start_binding
            .as_ref()
            .and_then(|binding| binding.fixed_point),
        Some([0.0, 0.0])
    );
    assert_eq!(arrow.end_binding, None);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        [
            "/elements/0/startBinding/fixedPoint",
            "/elements/0/endBinding/gap",
        ]
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| {
                serde_json::to_value(diagnostic).expect("diagnostic serializes")["details"]
                    ["recovery"]
                    .as_str()
                    .expect("recovery is a string")
                    .to_owned()
            })
            .collect::<Vec<_>>(),
        ["substituted", "omitted"]
    );
}

#[test]
fn a_missing_nested_binding_member_keeps_its_parent_members_source_order() {
    // Spec §11.3 orders diagnostics by the owning source member before the
    // nested position. A missing `elementId` under `startBinding` therefore
    // precedes an invalid `elementId` under the later `endBinding`, even
    // though the missing member has no nested position of its own. Found
    // by the scene fuzz, whose saturated nested ordinal used to spill out
    // of the member lane and sort the diagnostic after the whole element.
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"arrow","type":"arrow","width":10,"height":10,"points":[[0,0],[10,10]],"startBinding":{"focus":0},"endBinding":{"elementId":""},"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("binding order vector is admitted");
    let mut diagnostics = Vec::new();

    normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        [
            "/elements/0/startBinding/elementId",
            "/elements/0/endBinding/elementId",
        ]
    );
}

#[test]
fn malformed_nested_linear_point_marks_only_that_geometry_unavailable() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","width":10,"height":10,"points":[[0,0],[10,"invalid"]],"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("nested-point recovery vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(line) = &normalized.elements[0].kind else {
        panic!("the line retains normalized linear state");
    };

    assert!(!line.geometry_available);
    let [diagnostic] = diagnostics.as_slice() else {
        panic!("one malformed point coordinate is diagnosed");
    };
    assert_eq!(diagnostic.json_pointer(), "/elements/0/points/1/1");
    assert_eq!(
        serde_json::to_value(diagnostic).expect("diagnostic serializes")["details"],
        serde_json::json!({
            "kind": "compatibility",
            "recovery": "placeholder",
            "recipe_id": "renderer.known-element-unavailable.v1",
        })
    );
}

#[test]
fn invalid_crop_members_disable_crop_and_report_each_member() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"image","type":"image","width":10,"height":10,"fileId":"asset","crop":{"x":-1},"seed":1,"updated":1}],"appState":{},"files":{"asset":{"id":"asset","created":1,"dataURL":"data:image/png;base64,AA=="}}}"##,
    )
    .expect("crop recovery vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Image(image) = &normalized.elements[0].kind else {
        panic!("the image retains normalized image state");
    };

    assert_eq!(image.crop, None);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        [
            "/elements/0/crop/x",
            "/elements/0/crop/y",
            "/elements/0/crop/width",
            "/elements/0/crop/height",
            "/elements/0/crop/naturalWidth",
            "/elements/0/crop/naturalHeight",
        ]
    );
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.effects() == [DiagnosticEffect::Visual]
            && serde_json::to_value(diagnostic).expect("diagnostic serializes")["details"]
                ["recovery"]
                == "omitted"
    }));
}

#[test]
fn invalid_arrow_defaults_are_substituted_and_diagnosed() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"arrow","type":"arrow","width":10,"height":10,"points":[[0,0],[10,10]],"elbowed":"yes","startArrowhead":"future","endArrowhead":3,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("arrow default vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(arrow) = &normalized.elements[0].kind else {
        panic!("the arrow retains normalized linear state");
    };

    assert!(!arrow.elbowed);
    assert_eq!(arrow.start_arrowhead, None);
    assert_eq!(arrow.end_arrowhead.as_deref(), Some("arrow"));
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        [
            "/elements/0/elbowed",
            "/elements/0/startArrowhead",
            "/elements/0/endArrowhead",
        ]
    );
}

#[test]
fn invalid_line_arrowhead_metadata_is_substituted_and_diagnosed() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"line","type":"line","width":10,"height":10,"points":[[0,0],[10,10]],"startArrowhead":"future","endArrowhead":3,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("line arrowhead vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Linear(line) = &normalized.elements[0].kind else {
        panic!("the line retains normalized linear state");
    };

    assert_eq!(line.start_arrowhead, None);
    assert_eq!(line.end_arrowhead, None);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        ["/elements/0/startArrowhead", "/elements/0/endArrowhead",]
    );
}

#[test]
fn invalid_freedraw_state_uses_the_declared_local_recoveries() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"free","type":"freedraw","width":10,"height":10,"points":"invalid","pressures":"invalid","simulatePressure":"no","seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("freedraw recovery vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Freedraw(freedraw) = &normalized.elements[0].kind else {
        panic!("the freedraw element retains normalized state");
    };

    assert!(!freedraw.geometry_available);
    assert!(freedraw.points.is_empty());
    assert!(freedraw.pressures.is_empty());
    assert!(freedraw.simulate_pressure);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        [
            "/elements/0/points",
            "/elements/0/pressures",
            "/elements/0/simulatePressure",
        ]
    );
}

#[test]
fn malformed_freedraw_points_do_not_invalidate_valid_pressures() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"free","type":"freedraw","width":10,"height":10,"points":[[0,0],[10,"invalid"]],"pressures":[0,1],"simulatePressure":false,"seed":1,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("freedraw sample recovery vector is admitted");
    let mut diagnostics = Vec::new();

    let normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push(emitted(diagnostic, diagnostics.len()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    let NormalizedElementKind::Freedraw(freedraw) = &normalized.elements[0].kind else {
        panic!("the freedraw element retains normalized state");
    };

    assert!(!freedraw.geometry_available);
    assert_eq!(freedraw.pressures, [0.0, 1.0]);
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.json_pointer())
            .collect::<Vec<_>>(),
        ["/elements/0/points/1/1"]
    );
}

#[test]
fn normalization_resource_events_have_stable_record_names_and_owners() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"image","type":"image","width":10,"height":10,"fileId":"asset","seed":1,"updated":1}],"appState":{},"files":{"asset":{}}}"##,
    )
    .expect("resource-event vector is admitted");
    let mut events = Vec::new();
    let mut diagnostic_owners = Vec::new();

    normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            match event {
                super::NormalizationEvent::CommitRecord {
                    event,
                    element_id,
                    local_owner,
                    json_pointer,
                    work_units,
                    ..
                } => events.push((
                    event,
                    element_id.map(str::to_owned),
                    local_owner.map(str::to_owned),
                    json_pointer.to_owned(),
                    work_units,
                )),
                super::NormalizationEvent::Diagnostic {
                    diagnostic,
                    local_owner,
                    ..
                } => diagnostic_owners
                    .push((diagnostic.into_location().1, local_owner.map(str::to_owned))),
                super::NormalizationEvent::PreflightRecord { .. } => {}
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");

    assert_eq!(
        events,
        [
            (
                "normalized_image",
                Some("image".to_owned()),
                Some("image".to_owned()),
                "/elements/0".to_owned(),
                19 * 8,
            ),
            (
                "normalized_frame_rendering",
                None,
                None,
                "/appState/frameRendering".to_owned(),
                5 * 8,
            ),
            (
                "normalized_file_record",
                None,
                Some("asset".to_owned()),
                "/files/asset".to_owned(),
                2 * 8,
            ),
        ]
    );
    assert_eq!(
        diagnostic_owners,
        [
            ("/files/asset/id".to_owned(), Some("asset".to_owned()),),
            ("/files/asset/dataURL".to_owned(), Some("asset".to_owned()),),
            ("/files/asset/created".to_owned(), Some("asset".to_owned()),),
        ]
    );
}

/// Normalizes a one-element scene whose element is `element_json` and
/// returns that element with the element-scoped diagnostics normalization
/// emitted for it.
fn normalized_single_element(
    element_json: &str,
) -> (super::NormalizedElement, Vec<(DiagnosticCode, String)>) {
    let scene = format!(
        r##"{{"type":"excalidraw","version":2,"elements":[{element_json}],"appState":{{}},"files":{{}}}}"##
    );
    let scene = SceneDocument::parse(scene.as_bytes()).expect("single-element vector is admitted");
    let mut diagnostics = Vec::new();
    let mut normalized = normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |event| {
            if let super::NormalizationEvent::Diagnostic { diagnostic, .. } = event {
                diagnostics.push((diagnostic.code(), diagnostic.json_pointer().to_owned()));
            }
            Ok::<_, ()>(())
        },
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice");
    assert_eq!(normalized.elements.len(), 1, "one element is normalized");
    (normalized.elements.remove(0), diagnostics)
}

/// Normalizes one text element and returns its line height with the
/// element-scoped diagnostics normalization emitted for it.
fn normalized_text_line_height(text_fields: &str) -> (f64, Vec<(DiagnosticCode, String)>) {
    let (element, diagnostics) = normalized_single_element(&format!(
        r##"{{"id":"text","type":"text","seed":1,"updated":1,{text_fields}}}"##
    ));
    let NormalizedElementKind::Text(text) = &element.kind else {
        panic!("the element retains its normalized text kind");
    };
    (text.line_height, diagnostics)
}

#[test]
fn an_explicit_positive_line_height_is_kept_verbatim() {
    let (line_height, diagnostics) = normalized_text_line_height(
        r##""text":"one","fontSize":20,"fontFamily":2,"width":40,"height":90,"lineHeight":1.4"##,
    );
    assert_eq!(line_height, 1.4);
    assert!(diagnostics.is_empty());
}

#[test]
fn a_missing_line_height_detects_from_the_height_line_count_and_font_size() {
    // Excalidraw's restore path: height / splitIntoLines(text).length / fontSize.
    let (line_height, diagnostics) = normalized_text_line_height(
        r##""text":"one","fontSize":20,"fontFamily":2,"width":40,"height":30"##,
    );
    assert_eq!(line_height, 1.5);
    assert!(diagnostics.is_empty());

    let (line_height, _) = normalized_text_line_height(
        r##""text":"one\ntwo\nthree","fontSize":10,"fontFamily":3,"width":40,"height":36"##,
    );
    assert_eq!(line_height, 1.2);
}

#[test]
fn a_missing_line_height_detects_with_the_normalized_font_size() {
    // The reference divides by the raw fontSize and yields NaN for a missing
    // or invalid one; revision 1 detects with the defaulted 20 instead (#108).
    let (line_height, diagnostics) =
        normalized_text_line_height(r##""text":"one","fontFamily":5,"width":40,"height":30"##);
    assert_eq!(line_height, 1.5);
    assert!(diagnostics.is_empty());

    let (line_height, diagnostics) = normalized_text_line_height(
        r##""text":"one","fontSize":-8,"fontFamily":5,"width":40,"height":30"##,
    );
    assert_eq!(line_height, 1.5);
    assert_eq!(
        diagnostics,
        [(
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
            "/elements/0/fontSize".to_owned(),
        )]
    );
}

#[test]
fn a_missing_line_height_counts_lines_after_eol_normalization() {
    // splitIntoLines normalizes \r\n and \r to \n before splitting; tab
    // expansion does not add lines. Four lines here, so 120 / 4 / 20.
    let (line_height, _) = normalized_text_line_height(
        r##""text":"a\r\nb\rc\n\td","fontSize":20,"fontFamily":5,"width":40,"height":120"##,
    );
    assert_eq!(line_height, 1.5);

    // A trailing line break still starts an (empty) final line.
    let (line_height, _) = normalized_text_line_height(
        r##""text":"a\n","fontSize":20,"fontFamily":5,"width":40,"height":60"##,
    );
    assert_eq!(line_height, 1.5);
}

#[test]
fn a_missing_line_height_without_a_height_takes_the_family_metadata_line_height() {
    // FONT_METADATA[family].metrics.lineHeight, keyed on the source family:
    // Helvetica keeps 1.15 although Liberation Sans shapes it.
    for (family, expected) in [
        (1, 1.25),
        (2, 1.15),
        (3, 1.2),
        (5, 1.25),
        (6, 1.35),
        (7, 1.15),
        (8, 1.25),
        (9, 1.15),
    ] {
        let (line_height, diagnostics) = normalized_text_line_height(&format!(
            r##""text":"one","fontSize":20,"fontFamily":{family},"width":40"##
        ));
        assert_eq!(line_height, expected, "family {family}");
        assert!(diagnostics.is_empty(), "family {family}");
    }

    // An unrecognized family normalizes to Excalifont first, which is
    // also the reference's fallback for an unregistered family.
    let (line_height, diagnostics) =
        normalized_text_line_height(r##""text":"one","fontSize":20,"fontFamily":4,"width":40"##);
    assert_eq!(line_height, 1.25);
    assert_eq!(
        diagnostics,
        [(
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
            "/elements/0/fontFamily".to_owned(),
        )]
    );
}

#[test]
fn a_zero_height_takes_the_family_metadata_line_height_like_the_reference_truthiness() {
    let (line_height, diagnostics) = normalized_text_line_height(
        r##""text":"one","fontSize":20,"fontFamily":6,"width":40,"height":0"##,
    );
    assert_eq!(line_height, 1.35);
    assert!(diagnostics.is_empty());

    // Negative heights are normalized to their magnitude before detection.
    let (line_height, _) = normalized_text_line_height(
        r##""text":"one","fontSize":20,"fontFamily":6,"width":40,"height":-30"##,
    );
    assert_eq!(line_height, 1.5);
}

#[test]
fn non_positive_line_heights_derive_and_keep_the_invalid_diagnostic() {
    // The reference's `||` treats 0 as missing and keeps a negative;
    // revision 1 derives both, and both keep the existing unsupported
    // diagnostic.
    for source in ["0", "-1.2"] {
        let (line_height, diagnostics) = normalized_text_line_height(&format!(
            r##""text":"one\ntwo","fontSize":20,"fontFamily":2,"width":40,"height":60,"lineHeight":{source}"##
        ));
        assert_eq!(line_height, 1.5, "lineHeight {source}");
        assert_eq!(
            diagnostics,
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/lineHeight".to_owned(),
            )],
            "lineHeight {source}"
        );

        let (line_height, _) = normalized_text_line_height(&format!(
            r##""text":"one","fontSize":20,"fontFamily":2,"width":40,"lineHeight":{source}"##
        ));
        assert_eq!(line_height, 1.15, "lineHeight {source} without height");
    }
}

/// Normalizes one text element and returns its font size and family with
/// the element-scoped diagnostics normalization emitted for it.
fn normalized_text_font(text_fields: &str) -> ((f64, i32), Vec<(DiagnosticCode, String)>) {
    let (element, diagnostics) = normalized_single_element(&format!(
        r##"{{"id":"text","type":"text","seed":1,"updated":1,"text":"one","width":40,"height":35,{text_fields}}}"##
    ));
    let NormalizedElementKind::Text(text) = &element.kind else {
        panic!("the element retains its normalized text kind");
    };
    ((text.font_size, text.font_family), diagnostics)
}

#[test]
fn a_legacy_font_string_migrates_to_font_size_and_font_family() {
    // restoreElement, text case: `font.split(" ")` gives the pixel token
    // and the family name; parseFloat strips the unit and FONT_FAMILY maps
    // the name to its id. The migration is silent.
    let (font, diagnostics) = normalized_text_font(r##""font":"28px Virgil""##);
    assert_eq!(font, (28.0, 1));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let (font, diagnostics) = normalized_text_font(r##""font":"14.5px Cascadia""##);
    assert_eq!(font, (14.5, 3));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn a_legacy_font_family_name_maps_like_font_family_and_unknown_names_fall_back() {
    // getFontFamilyByName: `Object.keys(FONT_FAMILY).includes(name)` is an
    // exact, case-sensitive match; anything else is DEFAULT_FONT_FAMILY
    // (Excalifont, 5). The lookup takes the second space-separated token
    // only, so a multi-word name such as "Lilita One" never matches, and
    // a missing token (`undefined`) falls back the same way, silently.
    for (font, expected_family) in [
        ("20px Virgil", 1),
        ("20px Helvetica", 2),
        ("20px Cascadia", 3),
        ("20px Excalifont", 5),
        ("20px Nunito", 6),
        ("20px Lilita One", 5),
        ("20px Comic Shanns", 5),
        ("20px Liberation Sans", 5),
        ("20px virgil", 5),
        ("20px Arial", 5),
        ("20px", 5),
        ("20px  Virgil", 5),
    ] {
        let (font_values, diagnostics) = normalized_text_font(&format!(r##""font":"{font}""##));
        assert_eq!(font_values, (20.0, expected_family), "{font:?}");
        assert!(diagnostics.is_empty(), "{font:?}: {diagnostics:?}");
    }
}

#[test]
fn a_legacy_font_size_reads_like_parse_float() {
    // parseFloat takes the longest decimal-literal prefix of the first
    // token: the unit is ignored whatever it is, a fraction may lack its
    // integer part, and an exponent is honored only when digits follow.
    for (font, expected_size) in [
        ("20 Virgil", 20.0),
        ("20em Virgil", 20.0),
        ("+20px Virgil", 20.0),
        (".5px Virgil", 0.5),
        ("20.px Virgil", 20.0),
        ("1e2px Virgil", 100.0),
        ("1.5E+1px Virgil", 15.0),
        ("2e Virgil", 2.0),
        ("2e+px Virgil", 2.0),
    ] {
        let (font_values, diagnostics) = normalized_text_font(&format!(r##""font":"{font}""##));
        assert_eq!(font_values, (expected_size, 1), "{font:?}");
        assert!(diagnostics.is_empty(), "{font:?}: {diagnostics:?}");
    }
}

#[test]
fn a_legacy_font_string_wins_over_the_modern_fields() {
    // `if ("font" in element)` overwrites both fontSize and fontFamily
    // whenever the key is present, and the key survives a re-save, so a
    // legacy scene edited in a modern editor still renders at the `font`
    // values. A modern field outside its domain keeps its own diagnostic.
    let (font, diagnostics) =
        normalized_text_font(r##""font":"28px Virgil","fontSize":40,"fontFamily":2"##);
    assert_eq!(font, (28.0, 1));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let (font, diagnostics) =
        normalized_text_font(r##""font":"28px Virgil","fontSize":"big","fontFamily":4"##);
    assert_eq!(font, (28.0, 1));
    assert_eq!(
        diagnostics,
        [
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/fontSize".to_owned(),
            ),
            (
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/fontFamily".to_owned(),
            ),
        ]
    );
}

#[test]
fn a_legacy_font_string_leaves_line_height_detection_to_the_source_fields() {
    // The restore computes lineHeight before the migration lands:
    // `detectLineHeight` divides by the element's own fontSize and
    // `getLineHeight` reads its own fontFamily, so the modern fields, not
    // the parsed ones, decide the restored line height (#108).
    let (line_height, diagnostics) = normalized_text_line_height(
        r##""text":"one","font":"20px Virgil","fontSize":40,"width":40,"height":50"##,
    );
    assert_eq!(line_height, 1.25);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    // Without a modern fontSize the reference detects NaN; revision 1
    // detects with the migrated size, as it does with the default (#108).
    let (line_height, diagnostics) = normalized_text_line_height(
        r##""text":"one","font":"36px Virgil","width":40,"height":45"##,
    );
    assert_eq!(line_height, 1.25);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    // Without a height the family metadata line height is the source
    // family's: Cascadia's 1.2 here although Helvetica shapes the text,
    // and Excalifont's 1.25 for the unregistered `undefined` when the
    // legacy string is the only family the element names.
    let (line_height, _) = normalized_text_line_height(
        r##""text":"one","font":"20px Helvetica","fontFamily":3,"width":40"##,
    );
    assert_eq!(line_height, 1.2);
    let (line_height, _) =
        normalized_text_line_height(r##""text":"one","font":"20px Helvetica","width":40"##);
    assert_eq!(line_height, 1.25);
}

#[test]
fn a_malformed_legacy_font_takes_the_independent_field_recovery_and_is_diagnosed() {
    // A string whose size token parseFloat cannot read (NaN), or reads as
    // a size nothing can render, is the reference's broken output; the
    // manifest's `/font` recovery substitutes the independent fontSize
    // and fontFamily dispositions and reports `unsupported-property-value`
    // at `/font`. Present modern fields then apply as if `font` were absent.
    for font in [
        r##""""##,
        r##""px Virgil""##,
        r##""bold 20px Virgil""##,
        r##"" 20px Virgil""##,
        r##""0px Virgil""##,
        r##""-20px Virgil""##,
        r##""0x10px Virgil""##,
        r##""Infinity Virgil""##,
    ] {
        let (font_values, diagnostics) = normalized_text_font(&format!(r##""font":{font}"##));
        assert_eq!(font_values, (20.0, 5), "{font}");
        assert_eq!(
            diagnostics,
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/font".to_owned(),
            )],
            "{font}"
        );

        let (font_values, diagnostics) =
            normalized_text_font(&format!(r##""font":{font},"fontSize":32,"fontFamily":3"##));
        assert_eq!(font_values, (32.0, 3), "{font} with modern fields");
        assert_eq!(
            diagnostics,
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/font".to_owned(),
            )],
            "{font} with modern fields"
        );
    }

    // A non-string is outside the manifest's `string` domain and takes the
    // same recovery; the reference would throw on `.split`.
    for font in ["null", "20", r##"{"size":20}"##] {
        let (font_values, diagnostics) = normalized_text_font(&format!(r##""font":{font}"##));
        assert_eq!(font_values, (20.0, 5), "{font}");
        assert_eq!(
            diagnostics,
            [(
                DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
                "/elements/0/font".to_owned(),
            )],
            "{font}"
        );
    }
}

/// Normalizes one element of `kind` and returns its roundness with the
/// element-scoped diagnostics normalization emitted for it. An image has
/// no payload here, so its unrelated `image.missing-file` is dropped.
fn normalized_roundness_of(
    kind: &str,
    element_fields: &str,
) -> (Option<NormalizedRoundness>, Vec<(DiagnosticCode, String)>) {
    let (element, mut diagnostics) = normalized_single_element(&format!(
        r##"{{"id":"element","type":"{kind}","width":120,"height":80,"seed":1,"updated":1,{element_fields}}}"##
    ));
    if kind == "image" {
        diagnostics.retain(|(code, _)| *code != DiagnosticCode::IMAGE_MISSING_FILE);
    }
    (element.roundness, diagnostics)
}

#[test]
fn legacy_round_migrates_for_every_kind_whose_roundness_the_manifest_reads() {
    // restoreElementWithProperties: no `roundness` and
    // `strokeSharpness === "round"` gives the isUsingAdaptiveRadius kinds
    // ROUNDNESS.LEGACY (1) and every other kind PROPORTIONAL_RADIUS (2).
    // getCornerRadius treats both as the proportional corner radius, so
    // the migration is kind-independent once it reaches the rendering view.
    for kind in [
        "rectangle",
        "image",
        "embeddable",
        "iframe",
        "diamond",
        "line",
        "arrow",
    ] {
        let (roundness, diagnostics) =
            normalized_roundness_of(kind, r##""strokeSharpness":"round""##);
        assert_eq!(roundness, Some(NormalizedRoundness::Proportional), "{kind}");
        assert!(diagnostics.is_empty(), "{kind}: {diagnostics:?}");
    }

    // Kinds whose `/roundness` the revision-1 manifest ignores keep
    // ignoring the migrated record as well.
    for kind in ["ellipse", "text", "freedraw", "frame", "magicframe"] {
        let (roundness, diagnostics) =
            normalized_roundness_of(kind, r##""strokeSharpness":"round""##);
        assert_eq!(roundness, None, "{kind}");
        assert!(diagnostics.is_empty(), "{kind}: {diagnostics:?}");
    }
}

#[test]
fn legacy_sharp_and_unrecognized_stroke_sharpness_round_nothing() {
    // The restore compares `strokeSharpness === "round"` strictly; every
    // other string leaves `roundness: null`. The manifest admits any
    // string at `/strokeSharpness`, so neither value is diagnosed here.
    for value in [r##""sharp""##, r##""ROUND""##] {
        let (roundness, diagnostics) =
            normalized_roundness_of("rectangle", &format!(r##""strokeSharpness":{value}"##));
        assert_eq!(roundness, None, "{value}");
        assert!(diagnostics.is_empty(), "{value}: {diagnostics:?}");
    }

    // A non-string value takes the manifest's `unsupported-property-value`
    // substitution at `/strokeSharpness` and likewise rounds nothing.
    let (roundness, diagnostics) = normalized_roundness_of("rectangle", r##""strokeSharpness":7"##);
    assert_eq!(roundness, None);
    assert_eq!(
        diagnostics,
        [(
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
            "/elements/0/strokeSharpness".to_owned(),
        )]
    );
}

#[test]
fn an_explicit_roundness_record_wins_over_legacy_stroke_sharpness() {
    // `element.roundness ? element.roundness : ...` — any record object,
    // even one that resolves to no rounding, takes precedence; only a
    // missing or `null` roundness consults the legacy value.
    for (fields, expected) in [
        (
            r##""roundness":{"type":3,"value":8},"strokeSharpness":"round""##,
            Some(NormalizedRoundness::Adaptive { radius: 8.0 }),
        ),
        (
            r##""roundness":{"type":2},"strokeSharpness":"sharp""##,
            Some(NormalizedRoundness::Proportional),
        ),
        (r##""roundness":{},"strokeSharpness":"round""##, None),
        (
            r##""roundness":null,"strokeSharpness":"round""##,
            Some(NormalizedRoundness::Proportional),
        ),
        (r##""roundness":null,"strokeSharpness":"sharp""##, None),
    ] {
        let (roundness, diagnostics) = normalized_roundness_of("rectangle", fields);
        assert_eq!(roundness, expected, "{fields}");
        assert!(diagnostics.is_empty(), "{fields}: {diagnostics:?}");
    }
}

#[test]
fn an_invalid_roundness_value_is_diagnosed_and_never_consults_stroke_sharpness() {
    // The reference keeps a truthy malformed `roundness` verbatim, which
    // then rounds nothing; revision 1 substitutes the same sharp result
    // for every malformed value and reports it at `/roundness`.
    let (roundness, diagnostics) =
        normalized_roundness_of("rectangle", r##""roundness":5,"strokeSharpness":"round""##);
    assert_eq!(roundness, None);
    assert_eq!(
        diagnostics,
        [(
            DiagnosticCode::COMPATIBILITY_UNSUPPORTED_PROPERTY_VALUE,
            "/elements/0/roundness".to_owned(),
        )]
    );
}
