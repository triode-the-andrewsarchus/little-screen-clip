use super::geometry::{adjusted_roughness, rough_drawing};
use super::resolve::{
    ResolvedImageCornerClip, ResolvedImageCropPlacement, ResolvedImageProjection,
};
use super::*;
use crate::diagnostic_registry::{DiagnosticKind, MetadataFailure};
use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::resource_accounting::lower_hex;
use crate::{
    font_metadata::FamilyMetrics, CompatibilityRevision, DiagnosticCode, DiagnosticEffect,
    Renderer, ResourceLimits,
};
use base64::Engine as _;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::fs;
use std::path::Path;

const EMPTY_SCENE: &[u8] =
    br#"{"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}"#;

mod optional_font_tests {
    use super::*;

    fn scene(text: &str, original_text: &str) -> SceneDocument {
        let source = json!({
            "type": "excalidraw", "version": 2,
            "elements": [{
                "id": "text", "type": "text", "width": 60, "height": 30,
                "fontSize": 20, "fontFamily": 5, "text": text,
                "originalText": original_text, "updated": 1
            }],
            "appState": {}, "files": {}
        });
        SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap()
    }

    fn assert_unavailable(error: RenderError, format: OutputFormat) {
        assert_eq!(error.code(), ErrorCode::RENDER_FONT_UNAVAILABLE);
        assert_eq!(error.category(), RenderErrorCategory::Capability);
        assert_eq!(error.category().as_str(), "capability");
        assert_eq!(error.format(), format);
        let Some(DiagnosticDetails::FontUnavailable {
            resolved_family,
            asset_identity,
        }) = error.details()
        else {
            panic!("expected unavailable font details: {error:?}");
        };
        assert_eq!(resolved_family, "Xiaolai");
        let record = crate::fonts::revision_1()
            .assets
            .iter()
            .find(|record| record.identity == asset_identity)
            .expect("the error identifies an official chunk");
        assert!(!record.is_available());
    }

    #[test]
    fn glyph_demand_propagates_capability_failure_in_every_format() {
        let scene = scene("中", "中");
        for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
            let request = RenderRequest::builder(format).build().unwrap();
            let result = Renderer::new().render(&scene, &request);
            if cfg!(feature = "cjk") {
                result.expect("default CJK payload renders the demanded glyph");
            } else {
                assert_unavailable(result.expect_err("glyph demand needs Xiaolai"), format);
            }
        }
    }

    #[test]
    fn cjk_trigger_without_declared_chunk_demand_keeps_missing_glyph_recovery() {
        // This Extension-B ideograph fires the frozen trigger but no Xiaolai
        // face declares it. The trigger alone must not cause a capability failure.
        let text = "\u{20000}";
        assert!(text::CjkSubsetTrigger::evaluate([text]).selects_xiaolai());
        assert!(crate::fonts::revision_1()
            .assets
            .iter()
            .filter(|record| record.family == "Xiaolai")
            .all(|record| {
                crate::fonts::inspection::font_face_unicode_ranges("Xiaolai", record.upstream_path)
                    .unwrap()
                    .iter()
                    .all(|(start, end)| !(*start..=*end).contains(&0x20000))
            }));
        let scene = scene(text, text);
        for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
            let request = RenderRequest::builder(format).build().unwrap();
            let outcome = Renderer::new()
                .render(&scene, &request)
                .expect("an uncovered scalar keeps the ordinary replacement recovery");
            assert!(outcome
                .report()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::FONT_MISSING_GLYPH));
        }
    }

    #[test]
    fn svg_semantic_demand_propagates_capability_failure_without_glyph_demand() {
        // Layout only sees the uncovered trigger scalar. SVG alone selects a
        // Xiaolai chunk from originalText, independently of shaped glyphs.
        let scene = scene("\u{20000}", "中");
        for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
            let request = RenderRequest::builder(format).build().unwrap();
            let result = Renderer::new().render(&scene, &request);
            if format == OutputFormat::Svg && !cfg!(feature = "cjk") {
                assert_unavailable(result.expect_err("semantic demand needs Xiaolai"), format);
            } else {
                result.expect("available semantic sources or glyph-only output renders");
            }
        }
    }
}

#[test]
fn focused_latin_svg_font_subset_beats_same_revision_complete_chunk_control() {
    let scene = SceneDocument::parse(
        br##"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","x":20,"y":30,"width":90,"height":72,"angle":0.125,"strokeColor":"#1e66f5","opacity":70,"fontSize":20,"fontFamily":5,"text":"One two three\nFour","textAlign":"center","verticalAlign":"middle","lineHeight":1.2,"frameId":null,"updated":1}],"appState":{},"files":{}}"##,
    )
    .expect("focused Latin scene is admitted");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");

    let candidate = Renderer::new()
        .render(&scene, &request)
        .expect("focused Latin candidate renders");
    let control = render_with_svg_font_payload(
        &crate::catalog::REVISION_1,
        &scene,
        &request,
        svg_fonts::Payload::CompleteChunks,
    )
    .expect("focused Latin complete-chunk control renders");

    assert_eq!(candidate.report(), control.report());
    assert_focused_svg_font_savings(&candidate, &control);
}

#[test]
fn multi_chunk_svg_font_subsets_beat_same_revision_complete_chunk_control() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","width":160,"height":40,"fontSize":20,"fontFamily":5,"text":"A","originalText":"A\u0416","updated":1}],"appState":{},"files":{}}"#,
    )
    .expect("multi-chunk scene is admitted");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");

    let candidate = Renderer::new()
        .render(&scene, &request)
        .expect("multi-chunk candidate renders");
    let control = render_with_svg_font_payload(
        &crate::catalog::REVISION_1,
        &scene,
        &request,
        svg_fonts::Payload::CompleteChunks,
    )
    .expect("multi-chunk complete-chunk control renders");

    assert_eq!(candidate.report(), control.report());
    let candidate_svg = std::str::from_utf8(candidate.artifact().bytes()).expect("SVG is UTF-8");
    let control_svg = std::str::from_utf8(control.artifact().bytes()).expect("SVG is UTF-8");
    assert_eq!(candidate_svg.matches("font-family:'Excalifont'").count(), 2);
    assert_eq!(control_svg.matches("font-family:'Excalifont'").count(), 2);
    assert_focused_svg_font_savings(&candidate, &control);
}

#[test]
fn complete_cjk_svg_font_subsets_beat_same_revision_complete_chunk_control() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","width":60,"height":30,"fontSize":20,"fontFamily":5,"text":"\u6f22\u6f22A","updated":1}],"appState":{},"files":{}}"#,
    )
    .expect("complete-CJK scene is admitted");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");

    let candidate = Renderer::new()
        .render(&scene, &request)
        .expect("complete-CJK candidate renders");
    let control = render_with_svg_font_payload(
        &crate::catalog::REVISION_1,
        &scene,
        &request,
        svg_fonts::Payload::CompleteChunks,
    )
    .expect("complete-CJK complete-chunk control renders");

    assert_eq!(candidate.report(), control.report());
    for artifact in [&candidate, &control] {
        let svg = std::str::from_utf8(artifact.artifact().bytes()).expect("SVG is UTF-8");
        let xiaolai = svg
            .find("font-family:'Xiaolai'")
            .expect("Xiaolai chunk is selected");
        let excalifont = svg
            .find("font-family:'Excalifont'")
            .expect("Excalifont Latin chunk is selected");
        assert!(xiaolai < excalifont);
    }
    assert_focused_svg_font_savings(&candidate, &control);
}

#[test]
fn full_intersection_svg_font_subset_does_not_grow_against_complete_chunk_control() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","width":160,"height":40,"fontSize":20,"fontFamily":5,"text":"A","originalText":"A\u0100\u0416\u03b1\u02c7\u0462\u0300","updated":1}],"appState":{},"files":{}}"#,
    )
    .expect("full-intersection scene is admitted");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");

    let candidate = Renderer::new()
        .render(&scene, &request)
        .expect("full-intersection candidate renders");
    let repeated = Renderer::new()
        .render(&scene, &request)
        .expect("full-intersection candidate renders repeatedly");
    let control = render_with_svg_font_payload(
        &crate::catalog::REVISION_1,
        &scene,
        &request,
        svg_fonts::Payload::CompleteChunks,
    )
    .expect("full-intersection complete-chunk control renders");

    assert_eq!(candidate.artifact().bytes(), repeated.artifact().bytes());
    assert_eq!(candidate.report(), control.report());
    assert_eq!(
        candidate.profile_identity().compatibility_revision(),
        CompatibilityRevision::REVISION_1
    );
    assert_eq!(
        control.profile_identity().compatibility_revision(),
        CompatibilityRevision::REVISION_1
    );
    let candidate_svg = std::str::from_utf8(candidate.artifact().bytes()).expect("SVG is UTF-8");
    let control_svg = std::str::from_utf8(control.artifact().bytes()).expect("SVG is UTF-8");
    assert_eq!(candidate_svg.matches("font-family:'Excalifont'").count(), 7);
    assert_eq!(control_svg.matches("font-family:'Excalifont'").count(), 7);
    assert!(candidate_svg.contains("svg-font-subset@sha256:"));
    assert!(!control_svg.contains("svg-font-subset@sha256:"));
    assert!(candidate.artifact().bytes().len() <= control.artifact().bytes().len());

    let mut expected_records = crate::fonts::revision_1()
        .assets
        .iter()
        .filter(|record| record.family == "Excalifont")
        .collect::<Vec<_>>();
    expected_records.sort_unstable_by_key(|record| {
        crate::fonts::inspection::font_face_order("Excalifont", record.upstream_path)
            .expect("Excalifont chunk has a pinned order")
    });
    let expected = expected_records
        .into_iter()
        .map(|record| {
            crate::fonts::inspection::load_asset(record.upstream_path)
                .expect("official Excalifont chunk is readable")
                .expect("official Excalifont chunk is present")
                .bytes()
                .to_vec()
        })
        .collect::<Vec<_>>();
    let actual = embedded_test_woff2(control_svg);
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "complete-chunk control payload {index} differs from its official source"
        );
    }
}

#[test]
fn font_subset_representative_direct_svg_corpus_does_not_grow_against_complete_chunk_controls() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/qualification/svg-font-subset-size-corpus.json"
    )))
    .expect("representative SVG font subset size corpus is JSON");
    assert_eq!(
        corpus["schema"],
        "excalidraw-render/svg-font-subset-size-corpus/v1"
    );
    let cases = corpus["cases"]
        .as_array()
        .expect("representative size corpus cases are an array");
    assert!(!cases.is_empty());
    assert!(cases.iter().any(|case| case["layer"] == "representative"));
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");
    for case in cases {
        let id = case["id"].as_str().expect("size case has an ID");
        let layer = case["layer"]
            .as_str()
            .expect("size case has a conformance layer");
        assert!(matches!(layer, "focused" | "representative"));
        let path = case["scene"].as_str().expect("size case has a scene path");
        let scene_bytes = fs::read(root.join(path))
            .unwrap_or_else(|error| panic!("failed to read size case {id}: {error}"));
        assert_eq!(
            lower_hex(&Sha256::digest(&scene_bytes)),
            case["scene_sha256"]
                .as_str()
                .expect("size case has a scene digest"),
            "size case {id} scene digest",
        );
        let scene = SceneDocument::parse(&scene_bytes)
            .unwrap_or_else(|error| panic!("failed to parse size case {id}: {error}"));
        let candidate = Renderer::new()
            .render(&scene, &request)
            .unwrap_or_else(|error| panic!("subset size case {id} failed: {error}"));
        let control = render_with_svg_font_payload(
            &crate::catalog::REVISION_1,
            &scene,
            &request,
            svg_fonts::Payload::CompleteChunks,
        )
        .unwrap_or_else(|error| panic!("complete-chunk size case {id} failed: {error}"));
        assert_eq!(candidate.report(), control.report(), "size case {id}");
        assert!(
            candidate.artifact().bytes().len() <= control.artifact().bytes().len(),
            "representative size case {id} grew: candidate={}, control={}",
            candidate.artifact().bytes().len(),
            control.artifact().bytes().len(),
        );
    }
}

fn assert_focused_svg_font_savings(candidate: &RenderOutcome, control: &RenderOutcome) {
    assert_eq!(
        candidate.profile_identity().compatibility_revision(),
        CompatibilityRevision::REVISION_1
    );
    assert_eq!(
        control.profile_identity().compatibility_revision(),
        CompatibilityRevision::REVISION_1
    );
    let candidate_len = candidate.artifact().bytes().len();
    let control_len = control.artifact().bytes().len();
    let savings = control_len.checked_sub(candidate_len).unwrap_or_else(|| {
        panic!("subset artifact grew: candidate={candidate_len} bytes, control={control_len} bytes")
    });
    assert!(
        savings > 4 * 1_024,
        "subset saved only {savings} bytes: candidate={candidate_len}, control={control_len}"
    );
    assert!(
        candidate_len * 5 < control_len * 4,
        "subset was not more than 20% smaller: candidate={candidate_len}, control={control_len}"
    );
}

fn embedded_test_woff2(svg: &str) -> Vec<Vec<u8>> {
    const PREFIX: &str = "data:font/woff2;base64,";
    let mut fonts = Vec::new();
    let mut remaining = svg;
    while let Some((_, tail)) = remaining.split_once(PREFIX) {
        let (encoded, rest) = tail.split_once(')').expect("font data URL terminates");
        fonts.push(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("font data URL is valid base64"),
        );
        remaining = rest;
    }
    fonts
}

#[test]
fn diagnostic_transaction_uses_fatal_precedence_and_never_appends_a_rejection() {
    let cases = [
        (1, 640, 8, ErrorCode::RESOURCE_DIAGNOSTIC_COUNT_EXCEEDED),
        (2, 640, 8, ErrorCode::RESOURCE_WORKING_BYTES_EXCEEDED),
        (2, 1_280, 8, ErrorCode::RESOURCE_TOTAL_WORK_EXCEEDED),
    ];

    for (max_diagnostics, max_working_bytes, max_total_work_units, expected_code) in cases {
        let limits = ResourceLimits::builder()
            .max_diagnostics(max_diagnostics)
            .max_working_bytes(max_working_bytes)
            .max_element_work_units(1)
            .max_asset_work_units(1)
            .max_total_work_units(max_total_work_units)
            .build()
            .expect("diagnostic precedence limits are valid");
        let request = RenderRequest::builder(OutputFormat::Svg)
            .resource_limits(limits)
            .build()
            .expect("diagnostic request is valid");
        let profile = Renderer::new().profile_identity();
        let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
        let mut session = RenderSession::new(
            profile,
            crate::catalog::REVISION_1.scene_surface(),
            &scene,
            &request,
        );
        let diagnostic = test_diagnostic();

        session
            .retain_diagnostic(
                &request,
                ResourcePhase::JsonTraversalAndNormalization,
                "test_diagnostic",
                diagnostic.clone(),
            )
            .expect("the first diagnostic fits at equality");
        let error = session
            .retain_diagnostic(
                &request,
                ResourcePhase::JsonTraversalAndNormalization,
                "test_diagnostic",
                diagnostic.clone(),
            )
            .expect_err("the second diagnostic crosses an aggregate limit");

        assert_eq!(error.code(), expected_code);
        let retained = diagnostic.retained(DiagnosticOrder::retained(
            Some(scene.source_owner_order("elements", 0)),
            None,
            0,
            DiagnosticAttribution::SceneGlobal,
        ));
        assert_eq!(error.report().diagnostics(), &[retained]);
    }
}

#[test]
fn later_output_exhaustion_preserves_the_accumulated_report() {
    let limits = ResourceLimits::builder()
        .max_output_bytes(1)
        .build()
        .expect("output limit is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(limits)
        .build()
        .expect("request is valid");
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let profile = Renderer::new().profile_identity();
    let mut session = RenderSession::new(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );
    let diagnostic = test_diagnostic();
    session
        .retain_diagnostic(
            &request,
            ResourcePhase::ProjectionAndFallback,
            "test_diagnostic",
            diagnostic.clone(),
        )
        .expect("diagnostic is retained");

    let error = session
        .render(&scene, &request, None, svg_fonts::Payload::Subset)
        .expect_err("output exhaustion is fatal");

    assert_eq!(error.code(), ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED);
    let retained = diagnostic.retained(DiagnosticOrder::retained(
        Some(scene.source_owner_order("elements", 0)),
        None,
        0,
        DiagnosticAttribution::SceneGlobal,
    ));
    assert_eq!(error.report().diagnostics(), &[retained]);
}

#[test]
fn diagnostic_trace_uses_the_callers_canonical_phase() {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let profile = Renderer::new().profile_identity();
    let mut session = RenderSession::new_with_trace(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );

    session
        .retain_diagnostic(
            &request,
            ResourcePhase::JsonTraversalAndNormalization,
            "normalization_diagnostic",
            test_diagnostic(),
        )
        .expect("diagnostic is retained");

    let trace: serde_json::Value =
        serde_json::from_slice(&session.meter.trace_bytes()).expect("trace is valid JSON");
    assert_eq!(
        trace["entries"][0]["phase"],
        "json_traversal_and_normalization"
    );
    assert_eq!(trace["entries"][0]["event"], "normalization_diagnostic");
}

#[test]
fn local_recovery_trace_commits_only_the_aggregate_observation() {
    let source = br##"{
        "type":"excalidraw","version":2,
        "elements":[{"id":"text","type":"text","x":0,"y":0,"width":100,"height":30,"strokeColor":"#000000","opacity":100,"fontSize":20,"fontFamily":5,"text":"ab","textAlign":"left","verticalAlign":"top","lineHeight":1.2,"updated":1}],
        "appState":{},"files":{}
    }"##;
    let scene = SceneDocument::parse(source).expect("text vector is valid");
    let limits = ResourceLimits::builder()
        .max_text_scalars_per_element(1)
        .build()
        .expect("local text limit is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(limits)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("local text exhaustion is recoverable");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is valid JSON");
    let rejection = trace["entries"]
        .as_array()
        .expect("trace entries are an array")
        .iter()
        .find(|entry| entry["decision"] == "rejected_local")
        .expect("local rejection is represented in the trace");

    assert_eq!(rejection["event"], "text_scalar_and_run_preflight");
    assert_eq!(rejection["deltas"]["text_scalars"], 2);
    assert_eq!(rejection["deltas"]["working_bytes"], 0);
    assert_eq!(rejection["deltas"]["total_work_units"], 0);
    assert_eq!(rejection["after"]["text_scalars"], 2);
}

/// The trace is held stable as the equalities below (repeated traced renders
/// agree, and a traced render leaves the artifact and report untouched) and by
/// the cross-target aggregate, not as pinned digests (ADR-0002).
#[test]
fn canonical_resource_trace_is_uncharged_stable_and_semantic() {
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
        let request = RenderRequest::builder(format)
            .build()
            .expect("request is valid");
        let profile = Renderer::new().profile_identity();
        let ordinary = Renderer::new()
            .render(&scene, &request)
            .expect("ordinary render succeeds");

        let (first_result, first_trace) = render_with_trace(profile, &scene, &request);
        let (second_result, second_trace) = render_with_trace(profile, &scene, &request);
        let first = first_result.expect("traced render succeeds");
        let second = second_result.expect("repeated traced render succeeds");

        assert_eq!(first_trace, second_trace);
        assert_eq!(first.artifact().bytes(), ordinary.artifact().bytes());
        assert_eq!(second.artifact().bytes(), ordinary.artifact().bytes());
        assert_eq!(first.report(), ordinary.report());
        assert_eq!(second.report(), ordinary.report());

        let trace_text = std::str::from_utf8(&first_trace).expect("trace is UTF-8 JSON");
        for forbidden in ["allocator", "backend", "cache", "thread", "timing"] {
            assert!(
                !trace_text.contains(forbidden),
                "trace exposes forbidden implementation fact {forbidden}"
            );
        }
        let trace: serde_json::Value =
            serde_json::from_slice(&first_trace).expect("trace schema is valid JSON");
        assert_eq!(trace["schema_version"], 1);
        assert_eq!(trace["header"]["compatibility_revision"], 1);
        assert_eq!(trace["header"]["format"], format.as_str());
        assert_eq!(trace["header"]["limits"]["max_scene_bytes"], 33_554_432);
        assert_eq!(
            trace["header"]["input_digests"]["supplemental_assets"],
            json!([])
        );

        let entries = trace["entries"]
            .as_array()
            .expect("trace entries are an array");
        assert!(entries.iter().all(|entry| {
            matches!(
                entry["decision"].as_str(),
                Some(
                    "committed"
                        | "rejected_local"
                        | "rejected_fatal"
                        | "shared_intrinsic"
                        | "released"
                )
            )
        }));
        assert_eq!(entries[0]["event"], "request_session");
        assert_eq!(entries[1]["event"], "scene_source_octets");
        assert_eq!(entries[2]["event"], "json_node");
        assert_eq!(entries[2]["json_pointer"], "");
    }
}

#[test]
fn direct_svg_font_subset_trace_follows_semantic_selection_and_derivation_order() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","width":80,"height":30,"fontSize":20,"fontFamily":5,"text":"A","originalText":"A\u0416","updated":1}],"appState":{},"files":{}}"#,
    )
    .expect("font subset trace scene is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("font subset render succeeds");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is valid JSON");
    let entries = trace["entries"].as_array().expect("entries are an array");
    let events = entries
        .iter()
        .filter_map(|entry| entry["event"].as_str())
        .collect::<Vec<_>>();
    let family = events
        .iter()
        .position(|event| *event == "svg_font_family_scalar_set")
        .expect("family scalar set is traced");
    let first_considered = events
        .iter()
        .position(|event| *event == "svg_font_chunk_considered")
        .expect("candidate chunks are traced");
    let first_selected = events
        .iter()
        .position(|event| *event == "svg_font_chunk_selected")
        .expect("selected chunks are traced");
    let envelope = events
        .iter()
        .position(|event| *event == "svg_font_dependency_envelope")
        .expect("the dependency envelope is traced");
    let decode = events
        .iter()
        .position(|event| *event == "svg_font_woff2_decode")
        .expect("decode is traced");
    let css = events
        .iter()
        .rposition(|event| *event == "svg_font_css_data_url")
        .expect("CSS data URL is traced");
    assert!(family < first_considered);
    assert!(first_considered < first_selected);
    assert!(first_selected < envelope);
    assert!(envelope < decode);
    assert!(decode < css);
    assert!(!events.contains(&"svg_font_subset_workspace_preflight"));
    assert_eq!(
        events
            .iter()
            .zip(entries)
            .filter(|(event, entry)| {
                **event == "svg_font_chunk_considered" && entry["decision"] == "committed"
            })
            .count(),
        7
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| {
                entry["event"] == "svg_font_chunk_selected" && entry["decision"] == "committed"
            })
            .count(),
        2
    );

    let envelope_entry = &entries[envelope];
    assert!(envelope_entry["deltas"]["working_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 0));
    assert!(envelope_entry["deltas"]["total_work_units"]
        .as_u64()
        .is_some_and(|work| work > 0));
    let crossing = envelope_entry["attempted"]["working_bytes"]
        .as_u64()
        .expect("the attempted envelope has working bytes")
        - 1;
    let limited_request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(
            ResourceLimits::builder()
                .max_working_bytes(crossing)
                .build()
                .expect("the envelope crossing limit is valid"),
        )
        .build()
        .expect("the envelope crossing request is valid");
    let (limited_result, limited_trace) = render_with_trace(profile, &scene, &limited_request);
    assert_eq!(
        limited_result
            .expect_err("the envelope crosses before derivation")
            .code(),
        ErrorCode::RESOURCE_WORKING_BYTES_EXCEEDED
    );
    let limited_trace: serde_json::Value =
        serde_json::from_slice(&limited_trace).expect("the limited trace is valid JSON");
    let limited_entries = limited_trace["entries"]
        .as_array()
        .expect("limited entries are an array");
    let rejected_envelope = limited_entries
        .iter()
        .position(|entry| {
            entry["event"] == "svg_font_dependency_envelope"
                && entry["decision"] == "rejected_fatal"
        })
        .expect("the dependency envelope is rejected");
    assert!(!limited_entries
        .iter()
        .any(|entry| entry["event"] == "svg_font_woff2_decode"));
    assert!(limited_entries[rejected_envelope + 1..]
        .iter()
        .all(|entry| entry["decision"] == "released"));

    let output_request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(
            ResourceLimits::builder()
                .max_output_bytes(1)
                .build()
                .expect("the output crossing limit is valid"),
        )
        .build()
        .expect("the output crossing request is valid");
    let (output_result, output_trace) = render_with_trace(profile, &scene, &output_request);
    assert_eq!(
        output_result
            .expect_err("output rejection occurs after derivation")
            .code(),
        ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED
    );
    let output_trace: serde_json::Value =
        serde_json::from_slice(&output_trace).expect("the output trace is valid JSON");
    let output_entries = output_trace["entries"]
        .as_array()
        .expect("output entries are an array");
    let workspace_release = output_entries
        .iter()
        .position(|entry| {
            entry["event"] == "svg_serializer_workspace" && entry["decision"] == "released"
        })
        .expect("serializer workspace is released after failure");
    let envelope_release = output_entries
        .iter()
        .position(|entry| {
            entry["event"] == "svg_font_dependency_envelope" && entry["decision"] == "released"
        })
        .expect("dependency envelope is released after failure");
    assert!(workspace_release < envelope_release);
}

#[test]
fn cjk_subset_trigger_matches_script_and_explicit_symbol_classes() {
    let selects = |text: &str| text::CjkSubsetTrigger::evaluate([text]).selects_xiaolai();
    assert!(selects("\u{4e2d}"));
    assert!(selects("\u{d7b0}"));
    assert!(selects("￥"));
    assert!(!selects("\u{2ff0}"));
    assert!(!selects("A"));
    assert!(!selects(""));
    // The trigger is a scalar-set intersection over every occurrence, not a
    // per-occurrence property.
    assert!(text::CjkSubsetTrigger::evaluate(["A", "\u{4e2d}"]).selects_xiaolai());
}

#[test]
fn cascadia_calt_ligatures_lay_out_through_their_empty_spacer_glyphs() {
    // Cascadia Code spells `->`, `??`, `www`, and the rest of its ligature set
    // through `calt`: every character but the last becomes the empty `LIG`
    // spacer glyph, and the last carries the joined outline. An empty outline
    // in a reviewed chunk is font content, not corruption, so the text lays
    // out and renders. Found by the scene fuzz as `fontFamily: 3` with two
    // CJK scalars, whose `??` replacement glyphs form the same ligature.
    for text_value in ["->", "??", "www", "a != b", "\u{4e2d}\u{6587}"] {
        let font = text::OfficialFont::load(3, text_value)
            .ok()
            .unwrap_or_else(|| panic!("Cascadia Code loads for {text_value:?}"));
        let laid_out = font
            .layout(
                text_value,
                text::TextLayout {
                    font_family: 3,
                    outline_limit: u64::MAX,
                    ..excalifont_layout()
                },
            )
            .ok()
            .unwrap_or_else(|| panic!("{text_value:?} lays out in Cascadia Code"));
        let glyphs = laid_out
            .lines
            .iter()
            .map(|line| line.glyphs.len())
            .sum::<usize>();
        assert_eq!(
            glyphs,
            text_value.chars().count(),
            "{text_value:?} keeps one glyph slot per scalar"
        );

        let source = format!(
            r#"{{"type":"excalidraw","version":2,"elements":[{{"id":"code","type":"text","width":120,"height":25,"fontSize":20,"fontFamily":3,"text":{},"updated":1}}],"appState":{{}},"files":{{}}}}"#,
            serde_json::to_string(text_value).expect("text serializes")
        );
        let scene = SceneDocument::parse(source.as_bytes()).expect("ligature scene is admitted");
        let expects_replacement = !text_value.is_ascii();
        for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
            let request = RenderRequest::builder(format)
                .build()
                .expect("default request is valid");
            let outcome = Renderer::new()
                .render(&scene, &request)
                .unwrap_or_else(|error| panic!("{text_value:?} renders to {format:?}: {error}"));
            let replaced = outcome
                .report()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == DiagnosticCode::FONT_MISSING_GLYPH);
            assert_eq!(
                replaced,
                expects_replacement,
                "{text_value:?} in {format:?} replacement glyphs: {:?}",
                outcome.report()
            );
        }
    }
}

/// Lays `text` out with fallbacks against a fresh font list whose only face
/// is the whole-text face `OfficialFont::load` selects for it, and returns the
/// font fallback probes it charged.
fn fallback_probes_for(font_family: i32, text: &str) -> u64 {
    let mut fonts = vec![text::OfficialFont::load(font_family, text)
        .ok()
        .unwrap_or_else(|| panic!("family {font_family} loads for {text:?}"))];
    fallback_probes_with(&mut fonts, font_family, text)
}

/// Lays `text` out with fallbacks against `fonts`, whose first face is the
/// whole-text face of `font_family`, and returns the font fallback probes it
/// charged.
fn fallback_probes_with(fonts: &mut Vec<text::OfficialFont>, font_family: i32, text: &str) -> u64 {
    let trigger = text::CjkSubsetTrigger::evaluate([text]);
    let layout = text::TextLayout {
        font_family,
        outline_limit: u64::MAX,
        ..excalifont_layout()
    };
    let (laid_out, _) = text::layout_with_fallbacks(0, fonts, text, layout, trigger)
        .ok()
        .unwrap_or_else(|| panic!("{text:?} lays out with fallbacks"));
    laid_out.fallback_probes
}

#[test]
fn a_font_fallback_probe_is_one_consulted_face_per_shaped_scalar_occurrence() {
    // Spec 8.3.1: a shaped scalar the whole-text face maps pays no probe.
    // Otherwise it pays one probe for each official font chunk of its family
    // whose declared Unicode range covers it, in pinned face order; then, for
    // Excalifont when the CJK subset trigger selected Xiaolai, one for each
    // Xiaolai chunk likewise; then one for the Noto Emoji consultation; then
    // one for the Liberation Sans replacement glyph. Every occurrence pays its
    // own probes. Expected counts follow that rule, not the code.
    assert_eq!(
        fallback_probes_for(5, "A"),
        0,
        "the whole-text face maps it"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{20a9}"),
        2,
        "no Excalifont chunk declares the won sign: Noto Emoji, then the replacement"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{10ffff}"),
        2,
        "nothing declares it either: Noto Emoji, then the replacement"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{4e2d}"),
        1,
        "triggered: the one Xiaolai chunk declaring the ideograph maps it"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{1f600}"),
        1,
        "Noto Emoji maps the emoji on its one consultation"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{4e2d}\u{20a9}"),
        2,
        "triggered: the won sign is served by the one Xiaolai chunk declaring it"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{20a9}\u{20a9}"),
        4,
        "a repeated scalar pays its probes on every occurrence"
    );
    assert_eq!(
        fallback_probes_for(9, "\u{5d0}"),
        3,
        "Liberation declares every scalar but maps no Hebrew: its chunk, Noto Emoji, the replacement"
    );
}

#[test]
fn a_selected_noto_emoji_face_is_consulted_once_per_scalar_like_an_unselected_one() {
    // Spec 8.3.1: whether a consulted face already serves the target changes
    // neither the probe count nor the order. The Noto Emoji consultation is
    // one probe whether the face is being selected now, was selected by an
    // earlier scalar of the same occurrence, or by an earlier occurrence.
    assert_eq!(
        fallback_probes_for(5, "\u{1f600}\u{20a9}"),
        1 + 2,
        "the emoji selects Noto Emoji; the won sign then pays Noto Emoji and the replacement"
    );
    assert_eq!(
        fallback_probes_for(5, "\u{1f600}\u{1f601}"),
        1 + 1,
        "the second emoji pays the one consultation of the selected face"
    );
    let mut fonts = vec![text::OfficialFont::load(5, "\u{1f600}").expect("Excalifont loads")];
    assert_eq!(fallback_probes_with(&mut fonts, 5, "\u{1f600}"), 1);
    assert_eq!(
        fallback_probes_with(&mut fonts, 5, "\u{20a9}"),
        2,
        "a later occurrence pays the won sign what a first occurrence would"
    );
}

#[test]
fn png_frame_clip_masks_are_charged_once_per_frame_and_owned_by_no_child() {
    // Two frames with two children each, plus one unframed rectangle. With
    // names and outlines off the canvas is 250x80; the frame clips cover
    // 100x60 and 80x40 device pixels.
    let scene = SceneDocument::parse(br##"{
        "type":"excalidraw","version":2,
        "elements":[
            {"id":"left","type":"frame","x":0,"y":0,"width":100,"height":60,"updated":1},
            {"id":"right","type":"frame","x":150,"y":0,"width":80,"height":40,"updated":1},
            {"id":"l1","type":"rectangle","x":10,"y":10,"width":20,"height":20,"frameId":"left","roughness":0,"updated":1},
            {"id":"r1","type":"rectangle","x":160,"y":10,"width":20,"height":20,"frameId":"right","roughness":0,"updated":1},
            {"id":"l2","type":"rectangle","x":40,"y":10,"width":20,"height":20,"frameId":"left","roughness":0,"updated":1},
            {"id":"r2","type":"rectangle","x":190,"y":10,"width":20,"height":20,"frameId":"right","roughness":0,"updated":1},
            {"id":"free","type":"rectangle","x":110,"y":45,"width":20,"height":10,"roughness":0,"updated":1}
        ],
        "appState":{},"files":{}
    }"##)
    .expect("two-frame scene is admitted");
    let request = RenderRequest::builder(OutputFormat::Png)
        .frame_options(
            crate::FrameOptions::default()
                .with_show_name(false)
                .with_outline(false),
        )
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("two-frame PNG renders");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is valid JSON");
    let entries = trace["entries"].as_array().expect("entries are an array");

    let mask_samples: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|entry| entry["event"] == "frame_clip_mask_samples")
        .collect();
    let expected = [
        ("left", "/elements/0", 100 * 60),
        ("right", "/elements/1", 80 * 40),
    ];
    assert_eq!(mask_samples.len(), expected.len());
    for (entry, (frame_id, pointer, pixels)) in mask_samples.iter().zip(expected) {
        assert_eq!(entry["element_id"], frame_id);
        assert_eq!(entry["json_pointer"], pointer);
        assert_eq!(entry["local_owner"], serde_json::Value::Null);
        assert_eq!(entry["deltas"]["rasterized_pixels"], pixels);
        assert_eq!(entry["deltas"]["total_work_units"], 8 * pixels);
        assert_eq!(entry["decision"], "committed");
    }

    let mask_surfaces: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|entry| {
            entry["event"] == "frame_clip_mask_surface" && entry["record_action"] == "acquire"
        })
        .collect();
    assert_eq!(mask_surfaces.len(), expected.len());
    for (entry, (_, _, pixels)) in mask_surfaces.iter().zip(expected) {
        assert_eq!(entry["record_kind"], "raster_mask_surface");
        assert_eq!(entry["deltas"]["working_bytes"], 256 + pixels);
    }

    let last_mask = entries
        .iter()
        .rposition(|entry| entry["event"] == "frame_clip_mask_samples")
        .expect("frame masks are traced");
    let first_projection = entries
        .iter()
        .position(|entry| entry["event"] == "vector_coverage_samples")
        .expect("element projection is traced");
    assert!(last_mask < first_projection);
    assert!(
        !entries
            .iter()
            .any(|entry| entry["event"] == "paint_mask_samples"),
        "plain framed children need no mask of their own"
    );
}

#[test]
fn frame_name_is_laid_out_as_the_reference_text_element() {
    let font = text::FallbackFont::load().expect("the bundled Liberation Sans loads");
    let baseline = FamilyMetrics::for_source_family(2)
        .expect("Helvetica has metadata metrics")
        .first_baseline(14.0, 17.5);
    assert!((baseline - 12.530_273_437_5).abs() < 1e-12);

    // A name narrower than its frame keeps its measured width and starts at
    // the frame's x.
    let name = font
        .resolve_frame_name("Clip", 580.0)
        .expect("the name shapes");
    assert_eq!(name.label.text, "Clip");
    assert_eq!(name.label.display_text, "Clip");
    assert!(name.width > 20.0 && name.width < 30.0, "{}", name.width);
    assert_eq!(name.label.glyphs.len(), 4);
    assert!(name.label.glyphs[0].x.abs() < 1e-12, "left aligned");
    for glyph in &name.label.glyphs {
        assert!((glyph.y - baseline).abs() < 1e-12, "baseline {}", glyph.y);
    }
    assert!((name.label.baseline - baseline).abs() < 1e-12);

    // Wider than the frame: the longest prefix that fits with `...` appended,
    // the text element as wide as the frame.
    let full = font
        .resolve_frame_name("A rather long frame name", 1_000.0)
        .expect("the full name shapes");
    let narrow = font
        .resolve_frame_name("A rather long frame name", full.width / 2.0)
        .expect("the truncated name shapes");
    assert_eq!(narrow.width, full.width / 2.0);
    let kept = narrow
        .label
        .text
        .strip_suffix("...")
        .expect("ASCII dots, as truncateText appends");
    assert!("A rather long frame name".starts_with(kept) && !kept.is_empty());
    let fits = |text: &str| {
        font.resolve_frame_name(text, f64::INFINITY)
            .expect("candidate shapes")
            .width
    };
    assert!(fits(&narrow.label.text) <= narrow.width, "the cut fits");
    let one_more = format!(
        "{}...",
        "A rather long frame name"
            .chars()
            .take(kept.chars().count() + 1)
            .collect::<String>()
    );
    assert!(
        fits(&one_more) > narrow.width,
        "and is the longest that does"
    );

    // Too narrow for even one character with the dots: the whole name stays,
    // as truncateText leaves it, at the frame's width.
    let tiny = font
        .resolve_frame_name("Clip", 1.0)
        .expect("the tiny frame's name shapes");
    assert_eq!(tiny.label.text, "Clip");
    assert_eq!(tiny.width, 1.0);

    // A blank name measures as one space, as measureText does.
    let blank = font
        .resolve_frame_name("", 100.0)
        .expect("the blank name shapes");
    assert!(blank.label.glyphs.is_empty());
    assert!(blank.width > 0.0);
}

/// The Excalifont layout the line-ending tests share: 20 px at line height
/// 1.25, so lines sit 25 px apart.
fn excalifont_layout() -> text::TextLayout<'static> {
    text::TextLayout {
        width: 100.0,
        font_size: 20.0,
        line_height: 1.25,
        text_align: "left",
        font_family: 5,
        outline_limit: u64::MAX,
    }
}

/// `(source, display)` per laid-out line: what `data-source-text` and
/// `ActualText` carry beside what the glyphs show.
fn line_texts(laid_out: &text::ResolvedText) -> Vec<(&str, &str)> {
    laid_out
        .lines
        .iter()
        .map(|line| (line.text.as_str(), line.display_text.as_str()))
        .collect()
}

#[test]
fn a_carriage_return_ends_a_line_like_the_reference_and_stays_in_the_source_text() {
    // The reference renders `element.text.replace(/\r\n?/g, "\n").split("\n")`
    // and leaves `text` alone, so `\r\n` and `\r` each end a line while the
    // source bytes round-trip through the per-line source text (#110).
    let font = text::OfficialFont::load(5, "a\r\nb").expect("Excalifont loads");
    let laid_out = font
        .layout("a\r\nb", excalifont_layout())
        .expect("CRLF text lays out");
    assert_eq!(line_texts(&laid_out), [("a\r\n", "a"), ("b", "b")]);
    assert!((laid_out.lines[1].baseline - laid_out.lines[0].baseline - 25.0).abs() < 1e-9);
    assert!(laid_out.missing_codepoints.is_empty(), "\\r is not a glyph");
    assert_eq!(laid_out.line_break_steps, 2);
    assert_eq!(laid_out.prepaid_line_break_steps, 2);
    assert_eq!(laid_out.prepaid_glyphs, 2);
    assert_eq!(laid_out.glyph_count(), 2);
}

#[test]
fn every_line_ending_shape_splits_as_replace_then_split_does() {
    // Worked through `text.replace(/\r\n?/g, "\n").split("\n")`: `\r\n` is one
    // break, a bare `\r` is one, `\n\r` and `\r\r` are two each, a trailing
    // terminator leaves an empty last line, and empty text is one empty line.
    let font = text::OfficialFont::load(5, "ab").expect("Excalifont loads");
    let cases: [(&str, &[(&str, &str)]); 7] = [
        ("a\rb", &[("a\r", "a"), ("b", "b")]),
        ("a\n\rb", &[("a\n", "a"), ("\r", ""), ("b", "b")]),
        ("a\r\r\nb", &[("a\r", "a"), ("\r\n", ""), ("b", "b")]),
        ("a\r", &[("a\r", "a"), ("", "")]),
        ("a\r\n", &[("a\r\n", "a"), ("", "")]),
        ("\r", &[("\r", ""), ("", "")]),
        ("", &[("", "")]),
    ];
    for (source, expected) in cases {
        let laid_out = font
            .layout(source, excalifont_layout())
            .ok()
            .unwrap_or_else(|| panic!("{source:?} lays out"));
        assert_eq!(line_texts(&laid_out), expected, "{source:?}");
        assert!(laid_out.missing_codepoints.is_empty(), "{source:?}");
        let lines = u64::try_from(expected.len()).expect("small");
        assert_eq!(laid_out.line_break_steps, lines, "{source:?}");
        assert_eq!(laid_out.prepaid_line_break_steps, lines, "{source:?}");
        assert_eq!(
            laid_out.prepaid_glyphs,
            laid_out.glyph_count(),
            "{source:?} prepays exactly its shaped glyphs"
        );
        // The per-line source text reassembles the stored text byte for byte.
        let reassembled = laid_out
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<String>();
        assert_eq!(reassembled, source);
    }
}

#[test]
fn the_fallback_layout_splits_lines_by_the_same_rule() {
    // Missing glyphs and bidirectional text take `layout_with_fallbacks`; a
    // `\r` there must end the line too rather than reach the fallback probes.
    let mut fonts = vec![text::OfficialFont::load(5, "ab").expect("Excalifont loads")];
    for (source, expected) in [
        ("a\r\nb", vec![("a\r\n", "a"), ("b", "b")]),
        ("a\rb\r", vec![("a\r", "a"), ("b\r", "b"), ("", "")]),
        // CJK is not in Excalifont, so each line here is a Xiaolai fallback run.
        ("中\r\n文", vec![("中\r\n", "中"), ("文", "文")]),
    ] {
        let trigger = text::CjkSubsetTrigger::evaluate([source]);
        let (laid_out, _) =
            text::layout_with_fallbacks(0, &mut fonts, source, excalifont_layout(), trigger)
                .ok()
                .unwrap_or_else(|| panic!("{source:?} lays out with fallbacks"));
        assert_eq!(line_texts(&laid_out), expected, "{source:?}");
        assert!(laid_out.missing_codepoints.is_empty(), "{source:?}");
        assert!(!laid_out.noto_emoji_substituted, "{source:?}");
        let lines = u64::try_from(expected.len()).expect("small");
        assert_eq!(laid_out.line_break_steps, lines, "{source:?}");
        assert_eq!(laid_out.prepaid_line_break_steps, lines, "{source:?}");
        assert_eq!(
            laid_out.prepaid_glyphs,
            laid_out.glyph_count(),
            "{source:?}"
        );
        for (index, line) in laid_out.lines.iter().enumerate() {
            let expected_baseline = laid_out.lines[0].baseline + index as f64 * 25.0;
            assert!(
                (line.baseline - expected_baseline).abs() < 1e-9,
                "{source:?} line {index} baseline {}",
                line.baseline
            );
        }
    }
}

/// The vertical family metadata metrics Excalidraw's `FONT_METADATA` declares
/// and `getVerticalOffset` reads to place a first baseline: the independent
/// source the baseline assertions below are worked from.
struct Metrics {
    units_per_em: f64,
    ascender: f64,
    descender: f64,
}

impl Metrics {
    /// `font_size * (line_height / 2 + (ascender + descender) / units_per_em / 2)`.
    fn first_baseline(&self, font_size: f64, line_height: f64) -> f64 {
        font_size * (line_height / 2.0 + (self.ascender + self.descender) / self.units_per_em / 2.0)
    }
}

const HELVETICA: Metrics = Metrics {
    units_per_em: 2048.0,
    ascender: 1577.0,
    descender: -471.0,
};
const LIBERATION_SANS: Metrics = Metrics {
    units_per_em: 2048.0,
    ascender: 1854.0,
    descender: -434.0,
};
const CASCADIA: Metrics = Metrics {
    units_per_em: 2048.0,
    ascender: 1900.0,
    descender: -480.0,
};
const NUNITO: Metrics = Metrics {
    units_per_em: 1000.0,
    ascender: 1011.0,
    descender: -353.0,
};
const EXCALIFONT: Metrics = Metrics {
    units_per_em: 1000.0,
    ascender: 886.0,
    descender: -374.0,
};

/// Asserts the laid-out lines sit at `expected` baselines, in order.
fn assert_baselines(laid_out: &text::ResolvedText, expected: &[f64]) {
    let actual = laid_out
        .lines
        .iter()
        .map(|line| line.baseline)
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), expected.len(), "line count: {actual:?}");
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "baseline {actual} should be {expected}"
        );
    }
}

/// Lays `text` out in the font `OfficialFont::load` selects for `loaded_family`,
/// positioned by `layout_family`'s metadata metrics.
fn layout_in_family(
    loaded_family: i32,
    layout_family: i32,
    text: &str,
    font_size: f64,
    line_height: f64,
) -> text::ResolvedText {
    text::OfficialFont::load(loaded_family, text)
        .ok()
        .unwrap_or_else(|| panic!("family {loaded_family} loads"))
        .layout(
            text,
            text::TextLayout {
                width: 200.0,
                font_size,
                line_height,
                text_align: "center",
                font_family: layout_family,
                outline_limit: u64::MAX,
            },
        )
        .ok()
        .unwrap_or_else(|| panic!("family {layout_family} lays out"))
}

#[test]
fn helvetica_text_is_positioned_by_excalidraw_metadata_metrics_not_the_shaping_font() {
    // Helvetica (family 2) is shaped with the bundled Liberation Sans, but
    // `getVerticalOffset` reads Helvetica's own `FONT_METADATA` metrics, so
    // its baselines differ from Liberation Sans text (family 9) at the same
    // size although one font shapes both (#105). Whichever family loaded the
    // shared font, the layout's own family decides the baseline.
    // `1.15 / 2 + (1577 - 471) / 2048 / 2` and `1.15 / 2 + (1854 - 434) / 2048 / 2`:
    // the first-baseline factors at line height 1.15 for each family.
    let helvetica_first = HELVETICA.first_baseline(28.0, 1.15);
    assert_eq!(helvetica_first, 28.0 * 0.84501953125);
    let liberation_first = LIBERATION_SANS.first_baseline(28.0, 1.15);
    assert_eq!(liberation_first, 28.0 * 0.9216796875);
    let line_step = 28.0 * 1.15;
    for loaded_family in [2, 9] {
        let helvetica = layout_in_family(loaded_family, 2, "Two lines\nof Helvetica", 28.0, 1.15);
        assert_baselines(&helvetica, &[helvetica_first, helvetica_first + line_step]);
        assert_eq!(helvetica.font_family, "Liberation Sans");

        let liberation = layout_in_family(loaded_family, 9, "Two lines\nof Helvetica", 28.0, 1.15);
        assert_baselines(
            &liberation,
            &[liberation_first, liberation_first + line_step],
        );
        assert_eq!(liberation.font_family, "Liberation Sans");
    }
}

#[test]
fn the_first_baseline_follows_the_family_metadata_metrics_at_any_line_height() {
    // The layout positions the first baseline from the source family's
    // metadata metrics and the line height it is given, whether that is the
    // element's own or the one normalization restored for it (#108; the
    // restore itself is tested in `normalization::tests`, and that a restored
    // line height reaches the render in `legacy_font_strings_render_as_the_
    // official_font_size_and_family_migration_in_every_format`). The values
    // here are what the restore detects, `height / lines / fontSize`, for the
    // first three and the family metadata line height for the rest.
    for (family, text, font_size, line_height, metrics) in [
        (2, "First", 20.0, 1.5, &HELVETICA),
        (3, "Code", 20.0, 1.5, &CASCADIA),
        (2, "Two\nlines", 12.0, 2.0, &HELVETICA),
        (6, "Nunito", 20.0, 1.35, &NUNITO),
        (5, "Control", 20.0, 1.25, &EXCALIFONT),
        (2, "Helvetica", 20.0, 1.15, &HELVETICA),
        (9, "Liberation", 20.0, 1.15, &LIBERATION_SANS),
    ] {
        let laid_out = layout_in_family(family, family, text, font_size, line_height);
        let first = metrics.first_baseline(font_size, line_height);
        let expected = (0..laid_out.lines.len())
            .map(|index| first + index as f64 * font_size * line_height)
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), text.split('\n').count(), "{text:?}");
        assert_baselines(&laid_out, &expected);
    }
}

#[test]
fn crlf_and_cr_lines_lay_out_as_lf_lines_do() {
    // The reference renders `text.replace(/\r\n?/g, "\n").split("\n")` and
    // leaves `text` alone: the laid-out lines, their glyphs and their
    // baselines are the same for every line ending, and only the per-line
    // source text, which SVG carries in `data-source-text`, keeps the
    // element's own bytes (#110).
    let font = text::OfficialFont::load(5, "ab").expect("Excalifont loads");
    let lf = font
        .layout("a\nb", excalifont_layout())
        .expect("LF text lays out");
    let first = EXCALIFONT.first_baseline(20.0, 1.25);
    assert_baselines(&lf, &[first, first + 25.0]);
    let display_glyphs = |laid_out: &text::ResolvedText| {
        laid_out
            .lines
            .iter()
            .map(|line| {
                (
                    line.display_text.clone(),
                    line.glyphs
                        .iter()
                        .map(|glyph| (glyph.id, glyph.x))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    for (source, sources) in [("a\r\nb", ["a\r\n", "b"]), ("a\rb", ["a\r", "b"])] {
        let other = font
            .layout(source, excalifont_layout())
            .ok()
            .unwrap_or_else(|| panic!("{source:?} lays out"));
        assert_baselines(&other, &[first, first + 25.0]);
        assert_eq!(display_glyphs(&other), display_glyphs(&lf), "{source:?}");
        assert_eq!(line_texts(&other), [(sources[0], "a"), (sources[1], "b")]);
        assert!(other.missing_codepoints.is_empty(), "{source:?}");
    }
}

/// Lays `source` out with fallbacks for `font_family`, against a fresh font
/// list whose only face is the whole-text face `OfficialFont::load` selects.
fn fallback_layout(font_family: i32, source: &str) -> text::ResolvedText {
    let mut fonts = vec![text::OfficialFont::load(font_family, source)
        .ok()
        .unwrap_or_else(|| panic!("family {font_family} loads for {source:?}"))];
    let layout = text::TextLayout {
        font_family,
        outline_limit: u64::MAX,
        ..excalifont_layout()
    };
    let trigger = text::CjkSubsetTrigger::evaluate([source]);
    text::layout_with_fallbacks(0, &mut fonts, source, layout, trigger)
        .ok()
        .unwrap_or_else(|| panic!("{source:?} lays out with fallbacks"))
        .0
}

/// The one line of `source` laid out with fallbacks for `font_family`.
#[test]
fn glyphs_of_one_long_cluster_share_its_text() {
    // A base and its joiners shape as one cluster with a glyph per scalar.
    // Each glyph carries the whole cluster text for PDF `ToUnicode`; copying
    // it into every glyph took time and memory quadratic in the cluster.
    for family in [1, 5] {
        let source = format!("a{}", "\u{200d}".repeat(20_000));
        let line = fallback_line(family, &source);
        assert_eq!(line.glyphs.len(), 20_001);
        let first = &line.glyphs[0].unicode;
        assert_eq!(**first, utf16(&source));
        assert!(line
            .glyphs
            .iter()
            .all(|glyph| std::sync::Arc::ptr_eq(&glyph.unicode, first)));
    }
}

fn fallback_line(font_family: i32, source: &str) -> text::ResolvedLabel {
    let mut laid_out = fallback_layout(font_family, source);
    assert_eq!(laid_out.lines.len(), 1, "{source:?} is one line");
    laid_out.lines.remove(0)
}

/// `(display text, rtl)` per glyph run of the one line `laid_out` has.
fn single_line_runs(laid_out: &text::ResolvedText) -> Vec<(&str, bool)> {
    let [line] = &laid_out.lines[..] else {
        panic!("one source line, got {}", laid_out.lines.len());
    };
    runs_of(line)
}

/// `(display text, rtl)` per glyph run of `line`.
fn runs_of(line: &text::ResolvedLabel) -> Vec<(&str, bool)> {
    line.runs
        .iter()
        .map(|run| (run.display_text.as_str(), run.rtl))
        .collect()
}

/// `(display text, rtl, face family)` per glyph run of `line`.
fn faced_runs_of(line: &text::ResolvedLabel) -> Vec<(&str, bool, &str)> {
    line.runs
        .iter()
        .map(|run| (run.display_text.as_str(), run.rtl, run.font_family))
        .collect()
}

/// The glyphs of the run at `index`, in the order the line places them, as
/// `(logical cluster text, glyph id, x)`.
fn run_glyphs(line: &text::ResolvedLabel, index: usize) -> Vec<(String, u16, f64)> {
    let run = &line.runs[index];
    line.glyphs[run.glyph_start..run.glyph_end]
        .iter()
        .map(|glyph| {
            (
                String::from_utf16(&glyph.unicode).expect("cluster text is UTF-16"),
                glyph.id,
                glyph.x,
            )
        })
        .collect()
}

/// The glyph id each scalar of the run at `index` is positioned by in SVG, in
/// logical order.
fn logical_glyph_ids(line: &text::ResolvedLabel, index: usize) -> Vec<u16> {
    line.runs[index]
        .logical_glyphs
        .iter()
        .map(|glyph| line.glyphs[*glyph].id)
        .collect()
}

/// The glyph ids `face` maps `characters` to.
fn glyph_ids(font_family: i32, characters: &str) -> Vec<u16> {
    let (_, sfnt) = text::OfficialFont::load(font_family, characters)
        .ok()
        .unwrap_or_else(|| panic!("family {font_family} loads for {characters:?}"))
        .into_parts();
    let face = crate::shaping::Face::from_slice(&sfnt, 0).expect("face parses");
    characters
        .chars()
        .map(|character| {
            face.glyph_index(character)
                .unwrap_or_else(|| panic!("family {font_family} maps {character:?}"))
                .0
        })
        .collect()
}

#[test]
fn every_bidi_paragraph_of_a_source_line_is_laid_out() {
    // Spec 8.3 pins the reference's line rule (`\r\n`, `\r`, `\n` only), so a
    // UAX #9 paragraph separator stays inside one source line while
    // unicode-bidi splits the line into paragraphs at it (P1). Every paragraph
    // is content: each is laid out in order, and the separator itself, which
    // no bundled face maps, is the replacement glyph between them (#125).
    for separator in ['\u{2029}', '\u{85}', '\u{1c}', '\u{1d}', '\u{1e}'] {
        let source = format!("ab{separator}cd");
        let laid_out = fallback_layout(5, &source);
        assert_eq!(
            line_texts(&laid_out),
            [(source.as_str(), "ab?cd")],
            "{source:?} is one source line whose every paragraph is shown"
        );
        assert_eq!(
            single_line_runs(&laid_out),
            [("ab", false), ("?", false), ("cd", false)],
            "{source:?} lays its paragraphs out in order"
        );
        assert_eq!(
            laid_out.missing_codepoints,
            [u32::from(separator)],
            "{source:?} reports only the separator"
        );
        assert_eq!(laid_out.glyph_count(), 5, "{source:?} shapes every scalar");
        assert_eq!(
            laid_out.glyph_count(),
            laid_out.prepaid_glyphs,
            "{source:?}"
        );
    }

    // Every paragraph takes the element's base level (#166): Hebrew then a
    // separator is one right-to-left paragraph, and the Latin after it is
    // laid out as its own paragraph at the same right-to-left level, whose
    // one left-to-right run is unchanged by that level.
    let source = "\u{5d0}\u{2029}ab";
    let laid_out = fallback_layout(9, source);
    assert_eq!(line_texts(&laid_out), [(source, "??ab")]);
    assert_eq!(single_line_runs(&laid_out), [("??", true), ("ab", false)]);
    assert_eq!(laid_out.missing_codepoints, [0x5d0, 0x2029]);
}

#[test]
fn the_bidi_base_direction_is_the_elements_and_applies_to_every_line() {
    // The reference detects the direction once per text element with
    // `isRTL(element.text)` and sets the canvas `dir` and every line's SVG
    // `direction` from it, so a line is laid out at the element's base level
    // whatever its own first strong character is (#166). The issue's probe:
    // a Hebrew first line makes the element right-to-left, and its second
    // line, which starts with Latin, reads `world 123 <Arabic> Hello` in
    // visual order, as the official canvas draws it. Under a line's own
    // detection the second line would read `Hello 123 <Arabic> world`.
    let probe = "\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}\nHello \u{645}\u{631}\u{62d}\u{628}\u{627} 123 world";
    assert!(text::element_is_rtl(probe));
    assert!(text::needs_bidi_layout(probe));
    let laid_out = fallback_layout(5, probe);
    assert_eq!(laid_out.lines.len(), 2);
    let visual: String = laid_out.lines[1]
        .runs
        .iter()
        .map(|run| run.display_text.as_str())
        .collect();
    assert_eq!(
        visual, "world 123 ????? Hello",
        "the Latin-first second line takes the element's right-to-left level"
    );
    assert_eq!(laid_out.lines[1].display_text, visual);
    let leftmost = laid_out.lines[1].runs.first().expect("runs");
    assert!(
        laid_out.lines[1].glyphs[leftmost.glyph_start].x.abs() < 1e-9,
        "the visually first run starts at the left-aligned line origin"
    );

    // The inverse: a Latin first line makes the element left-to-right, and a
    // second line that starts with Hebrew is laid out at that level, so its
    // trailing Latin word stays at the right and the digits follow the Hebrew
    // in reading order: `123 <Hebrew> world`. Its own detection would give
    // `world 123 <Hebrew>`.
    let inverse = "Hello world\n\u{5e9}\u{5dc}\u{5d5}\u{5dd} 123 world";
    assert!(!text::element_is_rtl(inverse));
    assert!(text::needs_bidi_layout(inverse));
    let laid_out = fallback_layout(5, inverse);
    let visual: String = laid_out.lines[1]
        .runs
        .iter()
        .map(|run| run.display_text.as_str())
        .collect();
    assert_eq!(visual, "123 ???? world");

    // A left-to-right element with no right-to-left content needs no bidi
    // layout at all, while a right-to-left element does even for a line whose
    // scalars are all left-to-right or neutral: at the right-to-left level a
    // trailing neutral takes the paragraph direction and moves to the left.
    assert!(!text::needs_bidi_layout("Hello!\nworld"));
    let trailing = "\u{5d0}\nHello!";
    assert!(text::needs_bidi_layout(trailing));
    let laid_out = fallback_layout(5, trailing);
    assert_eq!(runs_of(&laid_out.lines[1]), [("!", true), ("Hello", false)]);
}

#[test]
fn the_elements_direction_is_the_references_regex_over_utf16_units() {
    // `isRTL` skips every unit in neither class and decides on the first unit
    // in either; it reads UTF-16 code units, so a supplementary scalar (a
    // surrogate pair inside `\u2C00-\uFB1C`) and a combining mark of
    // `\u0300-\u0590` are left-to-right, unlike UAX #9's treatment of them.
    assert!(text::element_is_rtl("\u{5d0}"));
    assert!(text::element_is_rtl("\u{627}\u{644}"));
    assert!(text::element_is_rtl(" 123 (\u{5d0}) abc"));
    assert!(text::element_is_rtl("\n\r\t\u{5d0}"));
    assert!(text::element_is_rtl("\u{fb1d}"));
    assert!(text::element_is_rtl("\u{fe70}"));
    assert!(
        text::element_is_rtl("\u{660}12 abc"),
        "Arabic digits are in the RTL class"
    );
    assert!(!text::element_is_rtl(""));
    assert!(!text::element_is_rtl("123 !?"));
    assert!(!text::element_is_rtl("abc \u{5d0}"));
    assert!(!text::element_is_rtl("\u{e9}\u{5d0}"));
    assert!(
        !text::element_is_rtl("\u{300}\u{5d0}"),
        "a combining mark is in the LTR class"
    );
    assert!(
        !text::element_is_rtl("\u{1f600}\u{5d0}"),
        "a surrogate pair is in the LTR class"
    );
    assert!(
        !text::element_is_rtl("\u{4e2d}\u{5d0}"),
        "CJK is in the LTR class"
    );
    assert!(
        !text::element_is_rtl("\u{800}\u{5d0}"),
        "U+0800-U+1FFF is in the LTR class"
    );
}

/// The default-ignorable code points #167 names, plus one from each further
/// range of the shaper's class.
const DEFAULT_IGNORABLES: &[char] = &[
    '\u{ad}',
    '\u{34f}',
    '\u{61c}',
    '\u{17b4}',
    '\u{180b}',
    '\u{200b}',
    '\u{200c}',
    '\u{200d}',
    '\u{200e}',
    '\u{200f}',
    '\u{202a}',
    '\u{202b}',
    '\u{202c}',
    '\u{202d}',
    '\u{202e}',
    '\u{2060}',
    '\u{2066}',
    '\u{2067}',
    '\u{2068}',
    '\u{2069}',
    '\u{fe0f}',
    '\u{feff}',
    '\u{fff0}',
    '\u{1d173}',
    '\u{e0001}',
    '\u{e0100}',
];

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Whether two label paths draw the same outlines: the same operations with
/// every coordinate within floating-point noise, since a scalar that splits a
/// line into runs positions the later glyphs through a shifted run origin
/// rather than one pen, which differs by an ulp or two.
fn same_outlines(left: &text::LabelPath, right: &text::LabelPath) -> bool {
    use text::LabelPathOperation as Op;
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    left.operations.len() == right.operations.len()
        && left
            .operations
            .iter()
            .zip(&right.operations)
            .all(|(left, right)| match (left, right) {
                (Op::Move { x: ax, y: ay }, Op::Move { x: bx, y: by })
                | (Op::Line { x: ax, y: ay }, Op::Line { x: bx, y: by }) => {
                    close(*ax, *bx) && close(*ay, *by)
                }
                (
                    Op::Quadratic {
                        control_x: acx,
                        control_y: acy,
                        x: ax,
                        y: ay,
                    },
                    Op::Quadratic {
                        control_x: bcx,
                        control_y: bcy,
                        x: bx,
                        y: by,
                    },
                ) => close(*acx, *bcx) && close(*acy, *bcy) && close(*ax, *bx) && close(*ay, *by),
                (Op::Close, Op::Close) => true,
                _ => false,
            })
}

#[test]
fn a_default_ignorable_code_point_is_hidden_and_never_a_missing_codepoint() {
    // Spec 8.3 (#167): a default-ignorable code point is not a missing
    // codepoint. No bundled face maps U+200F, but the browser draws nothing
    // for it and neither does the renderer: it is kept in its run, hidden by
    // the shaper with a zero advance, pays no font fallback probe, and is
    // reported by no diagnostic, so `rlm<RLM>mark` reads `rlmmark` with no gap.
    for family in [1, 2, 3, 5, 6, 7, 8, 9] {
        for character in DEFAULT_IGNORABLES {
            assert!(text::is_default_ignorable(*character));
            let source = format!("a{character}b");
            let laid_out = fallback_layout(family, &source);
            let scalar = u32::from(*character);
            assert!(
                laid_out.missing_codepoints.is_empty(),
                "family {family} reports U+{scalar:04X} as missing"
            );
            assert_eq!(
                laid_out.fallback_probes, 0,
                "family {family} probes a face for U+{scalar:04X}"
            );
            assert!(!laid_out.noto_emoji_substituted);
            let [line] = &laid_out.lines[..] else {
                panic!("one line");
            };
            assert_eq!(
                line.display_text.chars().collect::<Vec<_>>(),
                ['a', *character, 'b'],
                "family {family}: the scalar stays in the display text and is not replaced"
            );
            // One glyph per scalar; the hidden one is the face's space glyph
            // at zero advance, so `b` sits where it sits without the scalar.
            // A joiner or variation selector is a cluster continuation, so
            // its glyph shares `a`'s cluster text; a bidi mark is its own run.
            assert_eq!(
                line.glyphs.len(),
                3,
                "family {family} U+{scalar:04X}: one glyph per scalar"
            );
            let plain = fallback_line(family, "ab");
            let b_x = |line: &text::ResolvedLabel| {
                line.glyphs
                    .iter()
                    .find(|glyph| *glyph.unicode == utf16("b"))
                    .expect("b has a glyph")
                    .x
            };
            assert!(
                (b_x(line) - b_x(&plain)).abs() < 1e-9,
                "family {family} U+{scalar:04X}: the hidden glyph adds no advance ({} vs {})",
                b_x(line),
                b_x(&plain)
            );
            let hidden = &line.glyphs[1];
            assert!(
                (hidden.x - b_x(line)).abs() < 1e-9,
                "family {family} U+{scalar:04X}: the hidden glyph has zero advance"
            );
            assert!(
                same_outlines(&line.path, &plain.path),
                "family {family} U+{scalar:04X}: the hidden glyph draws nothing"
            );
        }
    }

    // The probe: with the mark and without, the visible glyphs sit at the
    // same positions and draw the same outlines. The mark is its own
    // right-to-left UAX #9 run of zero width between the two Latin runs.
    let plain = fallback_line(5, "rlmmark");
    let marked = fallback_line(5, "rlm\u{200f}mark");
    assert_eq!(marked.display_text, "rlm\u{200f}mark");
    assert_eq!(
        runs_of(&marked),
        [("rlm", false), ("\u{200f}", true), ("mark", false)]
    );
    let visible = |line: &text::ResolvedLabel| {
        line.glyphs
            .iter()
            .filter(|glyph| *glyph.unicode != utf16("\u{200f}"))
            .map(|glyph| (glyph.id, glyph.x, glyph.y))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        visible(&marked),
        visible(&plain),
        "the mark adds no advance"
    );
    assert!(
        same_outlines(&marked.path, &plain.path),
        "the mark draws nothing"
    );
    let mark = marked
        .glyphs
        .iter()
        .find(|glyph| *glyph.unicode == utf16("\u{200f}"))
        .expect("the mark has a glyph");
    let m_of_mark = marked.glyphs[marked.runs[2].glyph_start].x;
    assert!(
        (mark.x - m_of_mark).abs() < 1e-9,
        "the zero-advance mark sits where the next glyph starts: {} vs {m_of_mark}",
        mark.x
    );

    // A joiner between two emoji stays in their Noto Emoji run rather than
    // splitting it, and a mark at the very start of a run takes the run's
    // face from the scalar after it.
    let joined = fallback_line(5, "\u{1f600}\u{200d}\u{1f389}");
    assert_eq!(
        faced_runs_of(&joined),
        [("\u{1f600}\u{200d}\u{1f389}", false, "Noto Emoji")]
    );
    assert!(joined.missing_codepoints.is_empty());
    let leading = fallback_line(5, "\u{200d}\u{1f600}");
    assert_eq!(
        faced_runs_of(&leading),
        [("\u{200d}\u{1f600}", false, "Noto Emoji")]
    );

    // The whole-text face is chosen over the shaped scalars without the
    // default-ignorables: a Cyrillic text with a joiner still shapes whole in
    // the Cyrillic chunk and pays no probe.
    assert_eq!(fallback_probes_for(5, "\u{41f}\u{440}\u{438}\u{200d}"), 0);

    // The paragraph separator is not default-ignorable and stays the missing
    // codepoint #125 pinned.
    assert!(!text::is_default_ignorable('\u{2029}'));
    assert!(
        !text::is_default_ignorable('\u{115f}'),
        "HarfBuzz omits the Hangul fillers"
    );
    assert!(!text::is_default_ignorable(' '));
}

#[test]
fn a_default_ignorable_code_point_produces_no_diagnostic_in_any_format() {
    // End to end: a text element carrying U+200F renders with an empty
    // compatibility report in every format, where it used to report one
    // `font.missing-glyph` per format (#167).
    let scene = SceneDocument::parse(
        r##"{"type":"excalidraw","version":2,"elements":[{"id":"probe","type":"text","x":10,"y":10,"width":120,"height":25,"angle":0,"strokeColor":"#1e1e1e","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","roughness":1,"opacity":100,"groupIds":[],"frameId":null,"index":"a1","roundness":null,"seed":1,"version":1,"versionNonce":2,"isDeleted":false,"boundElements":null,"updated":1,"link":null,"locked":false,"text":"rlm\u200fmark","fontSize":20,"fontFamily":5,"textAlign":"left","verticalAlign":"top","containerId":null,"originalText":"rlm\u200fmark","autoResize":true,"lineHeight":1.25}],"appState":{"viewBackgroundColor":"#ffffff"},"files":{}}"##.as_bytes(),
    )
    .expect("probe scene is admitted");
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let request = RenderRequest::builder(format)
            .build()
            .expect("request is valid");
        let outcome = Renderer::new()
            .render(&scene, &request)
            .expect("probe renders");
        assert!(
            outcome.report().is_empty(),
            "{format:?} reports {:?}",
            outcome.report()
        );
    }
}

#[test]
fn a_right_to_left_run_is_shaped_right_to_left() {
    // Spec 8.3: within a run, glyph order and placement are the shaper's for
    // the run's display text in the run's direction. No bundled face maps
    // Hebrew, so an RTL run is replacement glyphs plus neutrals; guessing the
    // direction from that text would shape it left to right and leave an
    // asymmetric neutral at the run's logical end instead of its visual end
    // (#126). UAX #9 puts the space of "אבג " and the period of "אב." at the
    // left edge of their runs.
    let line = fallback_line(9, "abc \u{5d0}\u{5d1}\u{5d2} 123");
    assert_eq!(
        runs_of(&line),
        [("abc ", false), ("123", false), ("??? ", true)]
    );
    let hebrew = run_glyphs(&line, 2);
    assert_eq!(
        hebrew
            .iter()
            .map(|(text, ..)| text.as_str())
            .collect::<Vec<_>>(),
        [" ", "?", "?", "?"],
        "the space is the run's leftmost glyph"
    );
    assert!(
        hebrew.windows(2).all(|pair| pair[0].2 < pair[1].2),
        "glyphs are placed left to right in visual order: {hebrew:?}"
    );
    let digits = run_glyphs(&line, 1);
    let digits_right_edge = digits
        .last()
        .map(|(_, _, x)| *x)
        .expect("digits have glyphs");
    assert!(
        hebrew[0].2 > digits_right_edge,
        "the Hebrew run starts after the digits' last glyph origin"
    );

    let line = fallback_line(9, "\u{5d0}\u{5d1}.");
    assert_eq!(runs_of(&line), [("??.", true)]);
    assert_eq!(
        run_glyphs(&line, 0)
            .iter()
            .map(|(text, ..)| text.as_str())
            .collect::<Vec<_>>(),
        [".", "?", "?"],
        "the period is the run's leftmost glyph"
    );

    // UAX #9 L4: a mirrored character in a right-to-left run shows its mirror
    // glyph, so "(א)" still reads as an opening paren, the text, a closing
    // paren from left to right. The cluster text each glyph carries stays the
    // logical character.
    let [opening, closing] = glyph_ids(9, "()")[..] else {
        panic!("two glyphs");
    };
    let line = fallback_line(9, "(\u{5d0})");
    assert_eq!(runs_of(&line), [("(?)", true)]);
    let parens = run_glyphs(&line, 0);
    assert_eq!(
        parens
            .iter()
            .map(|(text, id, _)| (text.as_str(), *id))
            .collect::<Vec<_>>(),
        [(")", opening), ("?", parens[1].1), ("(", closing)],
        "the logical closing paren shows an opening glyph at the left, and vice versa"
    );
}

#[test]
fn face_sub_runs_of_a_right_to_left_run_are_placed_right_to_left() {
    // Spec 8.3: a visual run is split further by the face that shapes its
    // scalars, and every glyph run is laid out left to right in visual order.
    // Excalifont shapes the period of "אב." while Liberation Sans shapes the
    // replacement glyphs, so the one right-to-left visual run is two glyph
    // runs; the period is logically last, hence visually leftmost (#126).
    let line = fallback_line(5, "\u{5d0}\u{5d1}.");
    assert_eq!(
        faced_runs_of(&line),
        [(".", true, "Excalifont"), ("??", true, "Liberation Sans")],
        "the face runs of a right-to-left visual run follow in visual order"
    );
    let period = run_glyphs(&line, 0);
    let replacements = run_glyphs(&line, 1);
    assert!(
        period[0].2 < replacements[0].2,
        "the period sits left of the replacement glyphs: {period:?} vs {replacements:?}"
    );
    assert!(
        period[0].2.abs() < 1e-9,
        "left-aligned text starts at the line origin"
    );
    assert_eq!(
        line.display_text, ".??",
        "the line's display text follows its glyph runs"
    );
}

#[test]
fn every_scalar_of_a_run_is_positioned_by_its_own_glyph_in_either_direction() {
    // SVG assigns the nth x/y of a run's tspan to its nth logical character.
    // The shaper merges a combining mark into its base's cluster. A
    // right-to-left run comes back in visual order, but whether each
    // cluster's glyphs are reversed too depends on its script
    // (`shaping::reverses_cluster_glyphs`); each scalar is still positioned
    // by the glyph that shows it, so the mark keeps the offset the shaper
    // gave it over the base rather than the base's origin.
    // Nunito's Latin chunk maps the combining dot below and keeps it a glyph
    // of its own; Liberation's chunk maps no combining mark.
    let [e, question, dot_below] = glyph_ids(6, "e?\u{323}")[..] else {
        panic!("three glyphs");
    };

    // Left to right: "e" with a dot below, forced onto the fallback path by
    // Hebrew.
    let line = fallback_line(6, "e\u{323}\u{5d0}");
    assert_eq!(runs_of(&line), [("e\u{323}", false), ("?", true)]);
    assert_eq!(logical_glyph_ids(&line, 0), [e, dot_below]);
    let [base, mark] = line.runs[0].logical_glyphs[..] else {
        panic!("two scalars");
    };
    assert_ne!(base, mark, "each scalar is positioned by its own glyph");
    assert_eq!(
        line.glyphs[base].cluster, line.glyphs[mark].cluster,
        "the shaper merged the mark into its base's cluster"
    );
    assert_ne!(
        line.glyphs[base].x, line.glyphs[mark].x,
        "the mark keeps the offset the shaper gave it over the base"
    );

    // Right to left: a dot below a question mark inside a Hebrew paragraph.
    // The Hebrew is a Liberation Sans replacement glyph, the rest Nunito's,
    // so the visual run is two glyph runs placed right to left. Nunito's run
    // has no script, so the shaper reverses its graphemes, shapes them left
    // to right, and keeps the mark after its base: the mark sits where it
    // does left to right, not one advance before the base.
    let line = fallback_line(6, "\u{5d0}?\u{323}");
    assert_eq!(
        faced_runs_of(&line),
        [("?\u{323}", true, "Nunito"), ("?", true, "Liberation Sans")]
    );
    let run = &line.runs[0];
    assert_eq!(
        line.glyphs[run.glyph_start..run.glyph_end]
            .iter()
            .map(|glyph| glyph.id)
            .collect::<Vec<_>>(),
        [question, dot_below],
        "the cluster's glyphs stay in logical order"
    );
    assert_eq!(logical_glyph_ids(&line, 0), [question, dot_below]);
    let [base, mark] = run.logical_glyphs[..] else {
        panic!("two scalars");
    };
    assert!(
        line.glyphs[mark].x > line.glyphs[base].x,
        "the mark follows its base"
    );
    let mut positioned = line.runs[0].logical_glyphs.clone();
    positioned.sort_unstable();
    assert_eq!(
        positioned,
        (line.runs[0].glyph_start..line.runs[0].glyph_end).collect::<Vec<_>>(),
        "every glyph positions exactly one scalar"
    );

    // When the shaper composes base and mark into one glyph (Nunito's `ccmp`
    // for the acute), both scalars are positioned by that glyph.
    let [e_acute] = glyph_ids(6, "\u{e9}")[..] else {
        panic!("one glyph");
    };
    let line = fallback_line(6, "e\u{301}\u{5d0}");
    assert_eq!(runs_of(&line), [("e\u{301}", false), ("?", true)]);
    assert_eq!(logical_glyph_ids(&line, 0), [e_acute, e_acute]);
    assert_eq!(line.runs[0].glyph_end - line.runs[0].glyph_start, 1);
}

#[test]
fn the_mirrored_backward_scripts_are_those_the_shaper_reverses_clusters_in() {
    // `shaping::BACKWARD_SCRIPTS` mirrors HarfRust's private table. Shape one
    // base-and-mark cluster right to left in each of its scripts, in a sample
    // of natively left-to-right ones, and with no script: the cluster's
    // glyphs come back reversed exactly when the mirror says so. Glyphs the
    // shaper adds (a dotted circle) are left out of the comparison.
    use crate::shaping::{reverses_cluster_glyphs, shape_in_script, Direction, BACKWARD_SCRIPTS};
    use harfrust::Script;
    let [question, dot_below] = glyph_ids(6, "?\u{323}")[..] else {
        panic!("two glyphs");
    };
    let (_, sfnt) = text::OfficialFont::load(6, "?\u{323}")
        .ok()
        .unwrap_or_else(|| panic!("Nunito loads"))
        .into_parts();
    let face = crate::shaping::Face::from_slice(&sfnt, 0).expect("face parses");
    let forward = [
        None,
        Some(Script::LATIN),
        Some(Script::CYRILLIC),
        Some(Script::GREEK),
        Some(Script::ARMENIAN),
        Some(Script::GEORGIAN),
    ];
    let backward = BACKWARD_SCRIPTS.map(Some);
    for (script, reversed) in forward
        .iter()
        .map(|script| (*script, false))
        .chain(backward.iter().map(|script| (*script, true)))
    {
        let buffer = shape_in_script(&face, "?\u{323}", Direction::RightToLeft, script);
        let glyphs: Vec<u32> = buffer
            .glyph_infos()
            .iter()
            .map(|info| info.glyph_id)
            .filter(|glyph| [question, dot_below].map(u32::from).contains(glyph))
            .collect();
        let expected = if reversed {
            [dot_below, question]
        } else {
            [question, dot_below]
        };
        assert_eq!(glyphs, expected.map(u32::from), "{script:?}");
        assert_eq!(reverses_cluster_glyphs(&buffer), reversed, "{script:?}");
    }
}

#[test]
fn line_terminators_do_not_steer_face_selection() {
    // Face selection asks whether every shaped scalar has a glyph; a `\r` is
    // never shaped, so it picks the face `\n` text would.
    for family in [1, 2, 3, 5, 6, 7, 8, 9] {
        let identity = |text: &str| {
            let (identity, _) = text::OfficialFont::load(family, text)
                .ok()
                .unwrap_or_else(|| panic!("family {family} loads for {text:?}"))
                .into_parts();
            identity
        };
        assert_eq!(identity("a\rb"), identity("a\nb"), "family {family} \\r");
        assert_eq!(
            identity("a\r\nb"),
            identity("a\nb"),
            "family {family} \\r\\n"
        );
    }
}

#[test]
fn required_raster_trace_precedes_projection_work() {
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let request = RenderRequest::builder(OutputFormat::Png)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("empty PNG renders");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is valid JSON");
    let entries = trace["entries"].as_array().expect("entries are an array");
    let main = entries
        .iter()
        .position(|entry| entry["event"] == "main_raster_surface")
        .expect("required main raster is traced");
    let background = entries
        .iter()
        .position(|entry| entry["event"] == "solid_background_samples")
        .expect("background projection is traced");

    assert!(main < background);
    assert_eq!(entries[main]["deltas"]["rasterized_pixels"], 400);
    assert_eq!(entries[main]["decision"], "committed");
}

#[test]
fn optional_metadata_stage_rejection_keeps_only_committed_work() {
    let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{},"source":"x".repeat(100_000)});
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    let profile = crate::catalog::REVISION_1.profile();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let request = RenderRequest::builder(format)
            .metadata_options(crate::MetadataOptions::default().with_embedded_scene(true))
            .build()
            .unwrap();
        let (full, trace) = render_with_trace(profile, &scene, &request);
        full.unwrap();
        let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
        let entries = trace["entries"].as_array().unwrap();
        let generation = entries
            .iter()
            .find(|e| {
                e["event"] == "embedded_scene_metadata_omitted"
                    && e["deltas"]["total_work_units"].as_u64().unwrap_or(0) > 0
            })
            .unwrap();
        let g = generation["deltas"]["total_work_units"].as_u64().unwrap();
        let before = generation["before"]["total_work_units"].as_u64().unwrap();
        for (stage, event, committed) in [
            (0, "embedded_scene_metadata_omitted", 0),
            (1, "embedded_scene_metadata_measurement", g),
        ] {
            let ceiling = before + (stage + 1) * g - 1;
            let request = RenderRequest::builder(format)
                .metadata_options(crate::MetadataOptions::default().with_embedded_scene(true))
                .resource_limits(
                    crate::ResourceLimits::builder()
                        .max_element_work_units(1)
                        .max_asset_work_units(1)
                        .max_total_work_units(ceiling)
                        .build()
                        .unwrap(),
                )
                .build()
                .unwrap();
            let (outcome, bytes) = render_with_trace(profile, &scene, &request);
            outcome.unwrap();
            let limited: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let entries = limited["entries"].as_array().unwrap();
            let rejected = entries
                .iter()
                .find(|e| e["event"] == event && e["decision"] == "rejected_local")
                .unwrap();
            assert_eq!(rejected["before"]["total_work_units"], before + committed);
            assert_eq!(rejected["after"]["total_work_units"], before + committed);
            assert!(!entries
                .iter()
                .any(|e| e["event"] == "embedded_scene_metadata_retention"));
            let (_, again) = render_with_trace(profile, &scene, &request);
            assert_eq!(bytes, again);
        }
    }
}

#[test]
fn normalization_resource_events_repeat_with_generated_identity_and_source_order() {
    let source = br#"{"type":"excalidraw","version":2,"elements":[{"type":"rectangle","width":180,"height":100,"seed":0}],"appState":{},"files":{}}"#;
    let scene = SceneDocument::parse(source).expect("normalization vector is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (first_result, first_trace) = render_with_trace(profile, &scene, &request);
    let (second_result, second_trace) = render_with_trace(profile, &scene, &request);
    let first = first_result.expect("normalization vector renders");
    let second = second_result.expect("repeated normalization vector renders");

    assert_eq!(first.artifact().bytes(), second.artifact().bytes());
    assert_eq!(first.report(), second.report());
    assert_eq!(first_trace, second_trace);

    let trace: serde_json::Value =
        serde_json::from_slice(&first_trace).expect("trace is valid JSON");
    let normalization_entries = trace["entries"]
        .as_array()
        .expect("trace entries are an array")
        .iter()
        .filter(|entry| {
            matches!(
                entry["event"].as_str(),
                Some("normalization_repair" | "normalized_rectangle")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(normalization_entries.len(), 4);
    assert_eq!(
        normalization_entries
            .iter()
            .map(|entry| entry["json_pointer"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec![
            "/elements/0",
            "/elements/0/id",
            "/elements/0/seed",
            "/elements/0/updated",
        ]
    );
    assert!(normalization_entries.iter().all(|entry| {
        entry["element_id"]
            == "generated-32182208bba6af9fdf6995f59ee6a4097851c4655cf10d9e0681746fc495b792"
    }));
    assert!(normalization_entries[1..].iter().all(|entry| {
        entry["deltas"]["diagnostics"] == 1 && entry["deltas"]["total_work_units"] == 8
    }));
    assert_eq!(
        normalization_entries[0]["deltas"]["total_work_units"],
        crate::normalization::NORMALIZATION_WORK_UNITS + 8
    );
}

#[test]
fn format_plan_and_semantic_records_keep_each_rectangle_owner() {
    let source = br#"{"type":"excalidraw","version":2,"elements":[{"id":"first","type":"rectangle","x":0,"width":40,"height":40,"seed":1,"updated":1},{"id":"second","type":"rectangle","x":50,"width":40,"height":40,"seed":1,"updated":1}],"appState":{},"files":{}}"#;
    let scene = SceneDocument::parse(source).expect("multi-rectangle vector is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("multi-rectangle vector renders");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is valid JSON");
    let operations = trace["entries"]
        .as_array()
        .expect("trace entries are an array")
        .iter()
        .filter(|entry| entry["event"] == "element_operation" && entry["decision"] == "committed")
        .map(|entry| {
            (
                entry["element_id"].as_str().unwrap_or_default(),
                entry["json_pointer"].as_str().unwrap_or_default(),
                entry["deltas"]["total_work_units"]
                    .as_u64()
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        operations,
        vec![("first", "/elements/0", 8), ("second", "/elements/1", 8),]
    );
}

#[test]
fn rejected_trace_entry_is_exact_and_does_not_commit_partial_output() {
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let limits = ResourceLimits::builder()
        .max_output_bytes(1)
        .build()
        .expect("output limit is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(limits)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace_bytes) = render_with_trace(profile, &scene, &request);
    let error = result.expect_err("output exhaustion returns no artifact");
    assert_eq!(error.code(), ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED);
    assert!(error.report().is_empty());

    let trace: serde_json::Value =
        serde_json::from_slice(&trace_bytes).expect("trace is valid JSON");
    let entries = trace["entries"].as_array().expect("entries are an array");
    let rejection = entries
        .iter()
        .position(|entry| entry["decision"] == "rejected_fatal")
        .expect("trace has a rejected transaction");
    let rejected = &entries[rejection];
    assert_eq!(rejected["event"], "artifact_retention");
    assert_eq!(rejected["decision"], "rejected_fatal");
    assert_eq!(rejected["code"], "resource.output-bytes-exceeded");
    assert_eq!(rejected["measurement"], "exact");
    assert_eq!(rejected["before"], rejected["after"]);
    assert_eq!(rejected["after"]["output_bytes"], 0);
    assert!(rejected["attempted"]["output_bytes"]
        .as_u64()
        .is_some_and(|attempted| attempted > 1));
    assert!(entries[rejection + 1..]
        .iter()
        .all(|entry| entry["decision"] == "released"));
}

#[test]
fn rejected_output_measurement_never_runs_the_emission_pass() {
    let limits = ResourceLimits::builder()
        .max_output_bytes(1)
        .build()
        .expect("output limit is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(limits)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let mut session = RenderSession::new(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );
    let mut passes = 0;

    let error = session
        .measure_and_emit_output(&request, ErrorCode::ENCODE_SVG_FAILED, 0, || {
            passes += 1;
            Ok(b"measured output".to_vec())
        })
        .expect_err("measured output crosses the configured ceiling");

    assert_eq!(error.code(), ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED);
    assert_eq!(passes, 1, "rejected output is measured but never emitted");
    assert!(session.staged_output.is_none());
}

#[test]
fn emission_must_match_the_measured_artifact_bytes() {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let mut session = RenderSession::new(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );

    let error = session
        .emit_premeasured_output(
            &request,
            ErrorCode::ENCODE_SVG_FAILED,
            0,
            b"measured".to_vec(),
            || Ok(b"tampered".to_vec()),
        )
        .expect_err("same-length output from a divergent pass is not publishable");

    assert_eq!(error.code(), ErrorCode::RENDER_INTERNAL_FAILURE);
    assert!(session.staged_output.is_none());
}

#[test]
fn allocation_refusal_preserves_the_committed_trace_and_report_prefix() {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let mut successful = RenderSession::new_with_trace(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );
    let mut refused = RenderSession::new_with_trace(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );
    let diagnostic = test_diagnostic();
    refused
        .retain_diagnostic(
            &request,
            ResourcePhase::ProjectionAndFallback,
            "test_diagnostic",
            diagnostic.clone(),
        )
        .expect("report prefix is retained");
    successful
        .retain_diagnostic(
            &request,
            ResourcePhase::ProjectionAndFallback,
            "test_diagnostic",
            diagnostic.clone(),
        )
        .expect("control report prefix is retained");
    successful
        .emit_premeasured_output(
            &request,
            ErrorCode::ENCODE_SVG_FAILED,
            0,
            b"artifact".to_vec(),
            || Ok(b"artifact".to_vec()),
        )
        .expect("control emission succeeds");
    let expected_trace: serde_json::Value =
        serde_json::from_slice(&successful.meter.trace_bytes()).expect("control trace is JSON");

    let error = refused
        .emit_premeasured_output(
            &request,
            ErrorCode::ENCODE_SVG_FAILED,
            0,
            b"artifact".to_vec(),
            || {
                Err(FormatEncodeError::MemoryUnavailable {
                    subject_kind: "artifact",
                    subject: "svg_output",
                })
            },
        )
        .expect_err("injected physical allocation refusal is fatal");

    assert_eq!(error.code(), ErrorCode::RESOURCE_MEMORY_UNAVAILABLE);
    assert_eq!(error.report().diagnostics().len(), 1);
    assert!(refused.staged_output.is_none());
    let refused_trace: serde_json::Value =
        serde_json::from_slice(&refused.meter.trace_bytes()).expect("refusal trace is JSON");
    assert_eq!(refused_trace["header"], expected_trace["header"]);
    assert_eq!(refused_trace["entries"], expected_trace["entries"]);
}

#[test]
fn scene_carrier_failure_omits_only_the_scene_component() {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .metadata_options(
            crate::MetadataOptions::default()
                .with_embedded_scene(true)
                .with_provenance(true)
                .with_diagnostic_summary(true),
        )
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let mut session = RenderSession::new(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );

    let (metadata, _) = session
        .select_artifact_metadata(
            &scene,
            &request,
            None,
            ErrorCode::ENCODE_SVG_FAILED,
            0,
            |metadata| {
                if metadata.canonical_scene().is_some() {
                    Err(FormatEncodeError::Metadata(MetadataFailure::SceneEmbedding))
                } else {
                    Ok(metadata.manifest().unwrap_or_default().as_bytes().to_vec())
                }
            },
        )
        .expect("scene carrier failure is recoverable");

    assert!(metadata.canonical_scene().is_none());
    let manifest = metadata.manifest().expect("earlier metadata remains");
    assert!(manifest.contains("\"provenance\":{"));
    assert!(manifest.contains("\"diagnostic_summary\":{"));
    assert!(manifest.ends_with("\"embedded_scene\":null}"));
    assert_eq!(session.report.diagnostics.len(), 1);
    assert_eq!(
        session.report.diagnostics[0].code(),
        DiagnosticCode::METADATA_SCENE_EMBEDDING_FAILED
    );
}

#[test]
fn manifest_carrier_failure_without_notices_drops_all_optional_metadata() {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .metadata_options(crate::MetadataOptions::default().with_provenance(true))
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let mut session = RenderSession::new(
        profile,
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        &request,
    );

    let (metadata, artifact) = session
        .select_artifact_metadata(
            &scene,
            &request,
            None,
            ErrorCode::ENCODE_SVG_FAILED,
            0,
            |metadata| {
                if metadata.manifest().is_some() {
                    Err(FormatEncodeError::Metadata(
                        MetadataFailure::ManifestEmbedding,
                    ))
                } else {
                    Ok(b"required artifact".to_vec())
                }
            },
        )
        .expect("optional manifest carrier failure is recoverable");

    assert!(metadata.manifest().is_none());
    assert!(metadata.canonical_scene().is_none());
    assert_eq!(artifact, b"required artifact");
    assert_eq!(session.report.diagnostics.len(), 1);
    assert_eq!(
        session.report.diagnostics[0].code(),
        DiagnosticCode::METADATA_MANIFEST_EMBEDDING_FAILED
    );
}

#[test]
fn metadata_retention_rejection_preserves_measurement_work_in_the_trace() {
    let scene = SceneDocument::parse(EMPTY_SCENE).expect("scene is valid");
    let profile = Renderer::new().profile_identity();
    let metadata = crate::MetadataOptions::default()
        .with_embedded_scene(true)
        .with_provenance(true)
        .with_diagnostic_summary(true);
    let unlimited_request = RenderRequest::builder(OutputFormat::Svg)
        .metadata_options(metadata.clone())
        .build()
        .expect("request is valid");
    let unlimited = Renderer::new()
        .render(&scene, &unlimited_request)
        .expect("control render succeeds");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .metadata_options(metadata)
        .resource_limits(
            ResourceLimits::builder()
                .max_output_bytes(u64::try_from(unlimited.artifact().bytes().len()).unwrap() - 1)
                .build()
                .expect("limits are valid"),
        )
        .build()
        .expect("request is valid");

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("embedded scene retention exhaustion is recoverable");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is JSON");
    let entries = trace["entries"].as_array().expect("entries are an array");
    let measurement = entries
        .iter()
        .position(|entry| entry["event"] == "embedded_scene_metadata_measurement")
        .expect("measurement work is committed");
    let rejection = entries
        .iter()
        .position(|entry| entry["event"] == "embedded_scene_metadata_retention")
        .expect("retention rejection is traced");

    assert!(measurement < rejection);
    assert_eq!(entries[measurement]["decision"], "committed");
    assert_eq!(entries[rejection]["decision"], "rejected_local");
    assert_eq!(entries[rejection]["before"], entries[rejection]["after"]);
}

#[test]
fn diagnosed_unbounded_omission_has_its_own_semantic_transaction() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[null],"appState":{},"files":{}}"#,
    )
    .expect("omission vector is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let profile = Renderer::new().profile_identity();

    let (result, trace) = render_with_trace(profile, &scene, &request);
    result.expect("diagnosed omission retains a meaningful artifact");
    let trace: serde_json::Value = serde_json::from_slice(&trace).expect("trace is valid JSON");
    let omissions = trace["entries"]
        .as_array()
        .expect("trace entries are an array")
        .iter()
        .filter(|entry| entry["event"] == "compatibility_omission")
        .collect::<Vec<_>>();
    let [omission] = omissions.as_slice() else {
        panic!("one omission transaction is retained");
    };
    assert_eq!(omission["json_pointer"], "/elements/0");
    assert_eq!(omission["deltas"]["total_work_units"], 8);
}

/// An invalid-color diagnostic on the first element, as a stage would emit
/// it before the session retains it.
fn test_diagnostic() -> PendingDiagnostic {
    PendingDiagnostic::new(
        DiagnosticKind::InvalidColor,
        OutputFormat::Svg,
        &[DiagnosticEffect::Visual],
        Some("element-1"),
        "/elements/0/color",
        "invalid color substituted",
    )
    .expect("the test diagnostic allocates")
}

#[test]
fn png_raster_distinguishes_every_stroke_style() {
    // tiny-skia treats `Stroke::dash` as inert for the low-level stroker: the
    // path must be dashed before stroking. Skipping that step rendered dashed
    // and dotted elements as solid in PNG while SVG and PDF emitted the correct
    // dash arrays, so this pins all three styles apart in the raster.
    let render_one = |stroke_style: &str| {
        let scene = SceneDocument::parse(
            format!(
                r##"{{"type":"excalidraw","version":2,"elements":[{{"id":"probe","type":"ellipse","x":20,"y":20,"width":200,"height":120,"angle":0,"strokeColor":"#1e1e1e","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"{stroke_style}","roughness":0,"opacity":100,"groupIds":[],"frameId":null,"index":"a1","roundness":null,"seed":4242,"version":1,"versionNonce":4243,"isDeleted":false,"boundElements":null,"updated":1,"link":null,"locked":false}}],"appState":{{"viewBackgroundColor":"#ffffff"}},"files":{{}}}}"##
            )
            .as_bytes(),
        )
        .expect("stroke-style probe scene is admitted");
        let request = RenderRequest::builder(OutputFormat::Png)
            .build()
            .expect("PNG request is valid");
        let artifact = Renderer::new()
            .render(&scene, &request)
            .expect("stroke-style probe renders");
        Sha256::digest(artifact.artifact().bytes()).to_vec()
    };

    let solid = render_one("solid");
    let dashed = render_one("dashed");
    let dotted = render_one("dotted");

    assert_ne!(solid, dashed, "dashed strokes must not rasterize as solid");
    assert_ne!(solid, dotted, "dotted strokes must not rasterize as solid");
    assert_ne!(
        dashed, dotted,
        "dashed and dotted strokes use different dash arrays and must differ"
    );
}

#[test]
fn bundled_faces_carry_their_families_metadata_metrics_and_only_helvetica_differs() {
    // Text is positioned by the source family's metadata metrics (#105). For
    // every bundled family those equal the face's own hhea values, so moving
    // the baseline from the face to the table changes no bundled family's
    // output; Helvetica is shaped with Liberation Sans, whose metrics differ.
    let source_family = |catalog_family: &str| match catalog_family {
        "Virgil" => 1,
        "Cascadia" => 3,
        "Excalifont" => 5,
        "Nunito" => 6,
        "Lilita" => 7,
        "ComicShanns" => 8,
        "Liberation" => 9,
        "Xiaolai" => 100,
        other => panic!("unexpected bundled font family {other}"),
    };
    let mut checked = 0_usize;
    for record in crate::fonts::revision_1()
        .assets
        .iter()
        .filter(|record| record.role == "official-rendering-font")
    {
        let asset = crate::fonts::inspection::load_asset(record.upstream_path)
            .expect("bundled font is intact")
            .expect("bundled font is present");
        let sfnt = wuff::decompress_woff2(asset.bytes()).expect("bundled font decodes");
        let face = crate::shaping::Face::from_slice(&sfnt, 0).expect("bundled font parses");
        let metadata = FamilyMetrics::for_source_family(source_family(record.family))
            .expect("every bundled family has metadata metrics");
        assert_eq!(
            (
                i32::from(metadata.units_per_em),
                metadata.ascender,
                metadata.descender
            ),
            (
                i32::from(face.units_per_em()),
                face.ascender(),
                face.descender()
            ),
            "{} hhea table differs from its family metadata metrics",
            record.upstream_path
        );
        checked += 1;
    }
    assert!(checked >= 8, "every bundled family was checked ({checked})");

    let helvetica = FamilyMetrics::for_source_family(2).expect("Helvetica has metrics");
    let liberation = FamilyMetrics::for_source_family(9).expect("Liberation has metrics");
    assert_eq!(
        (
            helvetica.units_per_em,
            helvetica.ascender,
            helvetica.descender
        ),
        (2_048, 1_577, -471)
    );
    assert_ne!(helvetica, liberation);
}

#[test]
fn an_image_corner_radius_follows_get_corner_radius_of_its_shorter_side() {
    use crate::normalization::NormalizedRoundness;
    // `getCornerRadius(Math.min(element.width, element.height), element)`:
    // ROUNDNESS.LEGACY and PROPORTIONAL_RADIUS give a quarter of the size;
    // ADAPTIVE_RADIUS gives its fixed value until the size drops under four
    // times that value, where the proportional radius takes over.
    assert_eq!(
        geometry::image_corner_radius(100.0, 40.0, Some(NormalizedRoundness::Proportional)),
        10.0
    );
    assert_eq!(
        geometry::image_corner_radius(40.0, 100.0, Some(NormalizedRoundness::Proportional)),
        10.0
    );
    let default_adaptive = Some(NormalizedRoundness::Adaptive { radius: 32.0 });
    assert_eq!(
        geometry::image_corner_radius(200.0, 300.0, default_adaptive),
        32.0
    );
    assert_eq!(
        geometry::image_corner_radius(128.0, 300.0, default_adaptive),
        32.0
    );
    assert_eq!(
        geometry::image_corner_radius(100.0, 300.0, default_adaptive),
        25.0
    );
    assert_eq!(
        geometry::image_corner_radius(
            100.0,
            40.0,
            Some(NormalizedRoundness::Adaptive { radius: 8.0 })
        ),
        8.0
    );
    assert_eq!(geometry::image_corner_radius(100.0, 40.0, None), 0.0);
}

#[test]
fn an_image_corner_clip_exists_only_for_a_positive_radius_and_traces_its_rounded_rectangle() {
    use crate::normalization::NormalizedRoundness;

    let Ok(Some(clip)) =
        ResolvedImageCornerClip::for_image(40.0, 20.0, Some(NormalizedRoundness::Proportional))
    else {
        panic!("a rounded image has a corner clip");
    };
    assert_eq!(clip.radius, 5.0);
    // The same nine-operation outline as the frame clip: four edges joined by
    // four kappa-approximated quarter arcs, starting and ending at the top
    // edge's left arc end.
    assert_eq!(clip.path.operations.len(), 9);
    let corners: Vec<(f64, f64)> = clip
        .path
        .operations
        .iter()
        .filter_map(|operation| match operation {
            geometry::PathOperation::Move(point) | geometry::PathOperation::Line(point) => {
                Some((point.x, point.y))
            }
            geometry::PathOperation::Cubic { .. } => None,
        })
        .collect();
    assert_eq!(
        corners,
        [
            (5.0, 0.0),
            (35.0, 0.0),
            (40.0, 15.0),
            (5.0, 20.0),
            (0.0, 5.0)
        ]
    );
    assert!(matches!(
        clip.path.operations.last(),
        Some(geometry::PathOperation::Cubic { end, .. }) if end.x == 5.0 && end.y == 0.0
    ));

    assert!(
        matches!(
            ResolvedImageCornerClip::for_image(40.0, 20.0, None),
            Ok(None)
        ),
        "an unrounded image has no corner clip"
    );
    assert!(
        matches!(
            ResolvedImageCornerClip::for_image(
                40.0,
                20.0,
                Some(NormalizedRoundness::Adaptive { radius: 0.0 })
            ),
            Ok(None)
        ),
        "a zero radius clips nothing the rectangle does not already clip"
    );
    assert!(
        matches!(
            ResolvedImageCornerClip::for_image(0.0, 20.0, Some(NormalizedRoundness::Proportional)),
            Ok(None)
        ),
        "a degenerate element has no corner to round"
    );
}

#[test]
fn an_image_projection_rounds_only_its_viewport_the_way_the_reference_use_does() {
    use crate::normalization::NormalizedImageCrop;

    // The reference `<use>` is `Math.round(width)` by `Math.round(height)`,
    // and an uncropped image fills it as the reference's 100% symbol image does.
    let projection = ResolvedImageProjection::for_image(40.5, 40.4, None);
    assert_eq!(projection.viewport_width, 41.0);
    assert_eq!(projection.viewport_height, 40.0);
    assert_eq!(projection.crop, None);
    assert_eq!(projection.content_rect(), (0.0, 0.0, 41.0, 40.0));

    // `Math.round` ties go up, not away from zero as `f64::round` would.
    let ties = ResolvedImageProjection::for_image(2.5, 0.5, None);
    assert_eq!(ties.viewport_width, 3.0);
    assert_eq!(ties.viewport_height, 1.0);
    let integer = ResolvedImageProjection::for_image(96.0, 40.0, None);
    assert_eq!(integer.viewport_width, 96.0);
    assert_eq!(integer.viewport_height, 40.0);
    let just_under = ResolvedImageProjection::for_image(0.499_999_999_999_999_94, 1.0, None);
    assert_eq!(just_under.viewport_width, 0.0);

    // A cropped image keeps the reference's unrounded uncropped size and crop
    // offset: `getUncroppedWidthAndHeight` and `normalizedCropX/Y` from the
    // oracle, for a 3x2 source cropped to its right 2x2 in a 20.5x20.4 element.
    let crop = NormalizedImageCrop {
        x: 1.0,
        y: 0.5,
        width: 2.0,
        height: 2.0,
        natural_width: 3.0,
        natural_height: 2.0,
    };
    let projection = ResolvedImageProjection::for_image(20.5, 20.4, Some(crop));
    assert_eq!(projection.viewport_width, 21.0);
    assert_eq!(projection.viewport_height, 20.0);
    assert_eq!(
        projection.crop,
        Some(ResolvedImageCropPlacement {
            offset_x: 10.25,
            offset_y: 5.1,
            uncropped_width: 30.75,
            uncropped_height: 20.4,
        })
    );
    assert_eq!(projection.content_rect(), (-10.25, -5.1, 30.75, 20.4));
}

#[test]
fn a_rounded_image_charges_its_corner_clip_geometry_once_per_target_and_replays() {
    // A 40x40 image with the default adaptive roundness: a 10 px corner clip
    // of nine path operations, charged like a frame clip's geometry in the
    // semantic phase and identically for every format.
    let scene = SceneDocument::parse(br##"{
        "type":"excalidraw","version":2,
        "elements":[{
            "id":"photo","type":"image","x":0,"y":0,"width":40,"height":40,
            "angle":0,"opacity":100,"fileId":"asset","status":"saved","scale":[1,1],
            "roundness":{"type":3},"seed":1,"updated":1
        }],
        "appState":{},
        "files":{"asset":{"id":"asset","mimeType":"image/png","dataURL":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=","created":1}}
    }"##)
    .expect("rounded image scene is admitted");
    let profile = Renderer::new().profile_identity();

    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let request = RenderRequest::builder(format)
            .build()
            .expect("request is valid");
        let cache = new_font_subset_cache();
        let (cold_result, cold_trace) =
            render_with_trace_and_font_subset_cache(profile, &scene, &request, &cache);
        let cold = cold_result.expect("cold rounded image render succeeds");
        let (warm_result, warm_trace) = render_with_trace(profile, &scene, &request);
        let warm = warm_result.expect("warm rounded image render succeeds");
        let (cached_result, cached_trace) =
            render_with_trace_and_font_subset_cache(profile, &scene, &request, &cache);
        let cached = cached_result.expect("request-cache rounded image render succeeds");

        assert_eq!(
            cold.artifact().bytes(),
            warm.artifact().bytes(),
            "{format:?}"
        );
        assert_eq!(
            cold.artifact().bytes(),
            cached.artifact().bytes(),
            "{format:?}"
        );
        assert_eq!(cold_trace, warm_trace, "{format:?}");
        assert_eq!(cold_trace, cached_trace, "{format:?}");

        let trace: serde_json::Value =
            serde_json::from_slice(&cold_trace).expect("trace is valid JSON");
        let entries = trace["entries"].as_array().expect("entries are an array");
        let clips: Vec<&serde_json::Value> = entries
            .iter()
            .filter(|entry| entry["event"] == "image_corner_clip_geometry")
            .collect();
        assert_eq!(clips.len(), 1, "{format:?} charges the corner clip once");
        let clip = clips[0];
        assert_eq!(clip["phase"], "shared_semantic_resolution", "{format:?}");
        assert_eq!(clip["element_id"], "photo", "{format:?}");
        assert_eq!(clip["local_owner"], "photo", "{format:?}");
        assert_eq!(clip["json_pointer"], "/elements/0", "{format:?}");
        assert_eq!(clip["deltas"]["geometry_segments"], 9, "{format:?}");
        assert_eq!(clip["deltas"]["total_work_units"], 9 * 32, "{format:?}");
        assert_eq!(clip["decision"], "committed", "{format:?}");
    }

    let unrounded = SceneDocument::parse(br##"{
        "type":"excalidraw","version":2,
        "elements":[{
            "id":"photo","type":"image","x":0,"y":0,"width":40,"height":40,
            "angle":0,"opacity":100,"fileId":"asset","status":"saved","scale":[1,1],
            "seed":1,"updated":1
        }],
        "appState":{},
        "files":{"asset":{"id":"asset","mimeType":"image/png","dataURL":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=","created":1}}
    }"##)
    .expect("unrounded image scene is admitted");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("request is valid");
    let (result, trace) = render_with_trace(profile, &unrounded, &request);
    result.expect("unrounded image renders");
    assert!(
        !std::str::from_utf8(&trace)
            .expect("trace is UTF-8 JSON")
            .contains("image_corner_clip_geometry"),
        "an unrounded image has no corner clip to charge"
    );
}

// Pinned Rough.js and perfect-freehand path vectors, recomputed from the
// geometry stage alone: normalization, then `rough_drawing` or
// `freedraw::generate`, then the SVG path writers. The digests are the ones
// the SVG renders of these scenes carried when they were pinned, so a change
// here is a change in the generated geometry, not in the document around it.

/// The normalized elements of `source` for a default SVG render.
fn normalized_elements(source: &[u8]) -> Vec<NormalizedElement> {
    let scene = SceneDocument::parse(source).expect("geometry scene is admitted");
    crate::normalization::normalize_scene(
        &scene,
        CompatibilityRevision::REVISION_1,
        OutputFormat::Svg,
        |_| Ok::<_, ()>(()),
    )
    .expect("normalization succeeds")
    .expect("the scene is in the implemented normalization slice")
    .elements
}

/// The default request's generation budget for `source`: the ceilings a
/// default render applies, which none of these scenes approach.
fn default_budget(source: &NormalizedElement) -> rough::GenerationBudget {
    let limits = ResourceLimits::default();
    rough::GenerationBudget::new(
        u64::from(limits.max_geometry_segments_per_element()),
        limits.max_element_work_units(),
        source
            .normalization_work_units
            .saturating_add(8)
            .saturating_add(source.repair_work_units),
    )
}

/// The SVG path data a default render writes for one Rough element, in
/// document order: the fill's path when the element has a background, then the
/// outline. Arrow decorations are not part of these pins.
fn rough_svg_paths(source: &NormalizedElement) -> Vec<String> {
    let drawing = rough_drawing(
        source,
        adjusted_roughness(source),
        &mut default_budget(source),
    )
    .ok()
    .unwrap_or_else(|| panic!("{} generates", source.source.id()));
    let path_data = |path: &geometry::Path| {
        let mut data = String::new();
        svg::push_rough_path(&mut data, path).expect("path data writes");
        data
    };
    drawing
        .fill
        .iter()
        .map(|fill| path_data(&fill.path))
        .chain(std::iter::once(path_data(&drawing.outline)))
        .collect()
}

/// Every SVG path of every element of `source`, in document order.
fn rough_svg_paths_of(source: &[u8]) -> Vec<String> {
    normalized_elements(source)
        .iter()
        .flat_map(rough_svg_paths)
        .collect()
}

/// The SVG path data a default render writes for one freedraw stroke.
fn freedraw_svg_path(source: &NormalizedElement) -> String {
    let NormalizedElementKind::Freedraw(freedraw) = &source.kind else {
        panic!("{} is a freedraw element", source.source.id());
    };
    let path = freedraw::generate(freedraw, source.stroke_width, &mut default_budget(source))
        .ok()
        .unwrap_or_else(|| panic!("{} generates", source.source.id()));
    svg::freedraw_path_data(&path).expect("freedraw path data writes")
}

fn sha256_hex(data: &str) -> String {
    crate::resource_accounting::sha256_hex(data.as_bytes())
}

/// The seven outline families the outline-shape render pins in every format:
/// a rectangle, a rounded rectangle, an ellipse, a diamond, a rounded diamond,
/// a line and a rounded line, none with a background.
const OUTLINE_FAMILIES: &[u8] = br##"{
  "type":"excalidraw","version":2,
  "elements":[
    {"id":"rectangle","type":"rectangle","x":10,"y":10,"width":80,"height":50,"seed":1,"updated":1},
    {"id":"rounded-rectangle","type":"rectangle","x":110,"y":10,"width":80,"height":50,"roundness":{"type":3},"seed":2,"updated":1},
    {"id":"ellipse","type":"ellipse","x":210,"y":10,"width":80,"height":50,"seed":3,"updated":1},
    {"id":"diamond","type":"diamond","x":310,"y":10,"width":80,"height":50,"seed":4,"updated":1},
    {"id":"rounded-diamond","type":"diamond","x":410,"y":10,"width":80,"height":50,"roundness":{"type":2},"seed":5,"updated":1},
    {"id":"line","type":"line","x":10,"y":90,"width":160,"height":40,"points":[[0,0],[80,40],[160,10]],"seed":6,"updated":1},
    {"id":"rounded-line","type":"line","x":210,"y":90,"width":160,"height":40,"points":[[0,0],[80,40],[160,10]],"roundness":{"type":2},"seed":7,"updated":1}
  ],
  "appState":{"viewBackgroundColor":"#ffffff"},"files":{}
}"##;

#[test]
fn outline_operations_match_the_pinned_roughjs_vectors() {
    let paths = rough_svg_paths_of(OUTLINE_FAMILIES);
    assert_eq!(paths.len(), 7);
    assert_eq!(
        paths
            .iter()
            .map(|path| path.matches(['M', 'C']).count())
            .collect::<Vec<_>>(),
        [16, 32, 24, 16, 32, 8, 6]
    );
    assert_eq!(
        sha256_hex(&paths.join("\n")),
        "6aac27aa315f3786f158ba71a73ed6e18537665c5e65335280ea6407aa68c567",
        // Every entry is the pinned npm Rough.js 4.6.4's, derived with
        // `tools/rough-filler-vectors/derive.mjs` over this scene, which
        // draws each shape as the reference's `generateElementShape` does. The
        // digest moved with the rounded diamond's corner radii (#160): they
        // come from the diamond's vertex offsets `floor(extent / 2) + 1`, as
        // its vertices do, not from the exact half extents.
        "outline families derived from the pinned npm Rough.js 4.6.4"
    );
}

/// Rounded diamonds at the sizes and roundness records that distinguish the
/// reference's radius rule (#160): the radii are `getCornerRadius` of the
/// vertex offsets `floor(width / 2) + 1` and `floor(height / 2) + 1`, so an
/// even extent gives a quarter pixel more than its half would, and an odd one
/// half a pixel more. The adaptive (type 3) record's fixed 32 applies only
/// where the vertex offset exceeds its 128 cutoff, so a 300x180 diamond takes
/// 32 horizontally and `91 * 0.25` vertically; the small one takes the
/// proportional branch on both axes, and carries a hachure fill so the fill
/// scan reads the same rounded outline.
const ROUNDED_DIAMOND_FAMILIES: &[u8] = br##"{
  "type":"excalidraw","version":2,
  "elements":[
    {"id":"proportional-even","type":"diamond","x":10,"y":10,"width":120,"height":80,"roundness":{"type":2},"roughness":0,"seed":61,"updated":1},
    {"id":"proportional-odd","type":"diamond","x":150,"y":10,"width":131,"height":77,"roundness":{"type":2},"seed":62,"updated":1},
    {"id":"adaptive-cutoff","type":"diamond","x":300,"y":10,"width":300,"height":180,"roundness":{"type":3},"seed":63,"updated":1},
    {"id":"adaptive-small","type":"diamond","x":620,"y":10,"width":80,"height":50,"roundness":{"type":3,"value":32},"backgroundColor":"#e03131","fillStyle":"hachure","seed":64,"updated":1}
  ],
  "appState":{},"files":{}
}"##;

#[test]
fn rounded_diamond_radii_match_the_pinned_roughjs_vectors() {
    // Every digest here is the pinned npm Rough.js 4.6.4's, computed with
    //
    //   node tools/rough-filler-vectors/derive.mjs \
    //     tools/reference-oracle/node_modules/roughjs/bundled/rough.esm.js \
    //     <<< "$ROUNDED_DIAMOND_FAMILIES"
    //
    // under Node 24.19.0 after `tools/reference-oracle/run-container.sh
    // install`; the script builds the rounded diamond's path from the vertex
    // offsets exactly as `generateElementShape` spells it (#160). The first
    // element is the 120x80 diamond of `focused.legacy-stroke-sharpness.v1`
    // at a different seed: its outline starts at `76.25 10.25`, where the
    // official export starts, not at `76 10`.
    const EXPECTED: &[(&str, &[usize], &[&str])] = &[
        (
            "proportional-even",
            &[32],
            &["0c50847ac1ab2e46ae6385be582994490b9b240848978c35052cbf7561c43b15"],
        ),
        (
            "proportional-odd",
            &[32],
            &["918a24b270e36b18d5b147fec4d3ac93ef6870047416f8e19143e928ddab868e"],
        ),
        (
            "adaptive-cutoff",
            &[32],
            &["52fc37db5a43582c6c53ee3cf730dfe3f4aacd34a9dcf83cf6cf655b206aed8c"],
        ),
        (
            "adaptive-small",
            &[32, 32],
            &[
                "9123e6c501f02751ba23ff438847c3de1ada05aa97f9e8276686b85c93e9e17c",
                "be04099e9c51603cd91f8617864105f734743de2a8180961d1f8d35dd267140d",
            ],
        ),
    ];
    let elements = normalized_elements(ROUNDED_DIAMOND_FAMILIES);
    assert_eq!(elements.len(), EXPECTED.len());
    let mut all_paths = Vec::new();
    for (element, (id, operations, digests)) in elements.iter().zip(EXPECTED) {
        assert_eq!(element.source.id(), *id);
        let paths = rough_svg_paths(element);
        assert_eq!(
            paths
                .iter()
                .map(|path| path.matches(['M', 'C', 'L']).count())
                .collect::<Vec<_>>(),
            *operations,
            "{id} operations"
        );
        assert_eq!(
            paths
                .iter()
                .map(|path| sha256_hex(path))
                .collect::<Vec<_>>(),
            *digests,
            "{id} paths"
        );
        all_paths.extend(paths);
    }
    assert!(
        all_paths[0].starts_with("M76.25 10.25 "),
        "{}",
        all_paths[0]
    );
    assert_eq!(
        sha256_hex(&all_paths.join("\n")),
        "3bed14cd6fd44ce9905e462f050a6f6409f166029eb9b4f4173ffecf761992f2"
    );
}

#[test]
fn architect_ellipse_uses_the_single_core_roughjs_stroke() {
    // Roughness 0 is Excalidraw's "architect" style: Rough.js draws the
    // ellipse's core stroke once instead of the sketchy double pass.
    let source = std::str::from_utf8(OUTLINE_FAMILIES).unwrap().replacen(
        "\"seed\":3,\"updated\"",
        "\"roughness\":0,\"seed\":3,\"updated\"",
        1,
    );
    let paths = rough_svg_paths_of(source.as_bytes());
    let ellipse = &paths[2];
    assert_eq!(ellipse.matches(['M', 'C']).count(), 42);
    assert_eq!(
        sha256_hex(ellipse),
        "d74c0380e58abcacfb14e495406d5f38af852f32f37a0ea528ecb0f0454bec05"
    );
}

#[test]
fn cartoonist_outline_consumes_vertex_offsets_in_roughjs_order() {
    // Roughness 2 is the "cartoonist" style: `preserveVertices` is off, so
    // Rough.js draws the corner offsets from its seeded generator in the
    // order the reference consumes them.
    let paths = rough_svg_paths_of(
        br##"{
        "type":"excalidraw","version":2,
        "elements":[{"id":"cartoonist","type":"rectangle","width":8,"height":5,
            "roughness":2,"seed":8,"updated":1}],
        "appState":{},"files":{}
    }"##,
    );
    assert_eq!(paths.len(), 1);
    assert_eq!(
        sha256_hex(&paths[0]),
        "fe33b396a5e457111305917fe234efdc6f856b635e7a55ab7fc5e5ed3083c19b"
    );
}

#[test]
fn cartoonist_rounded_outline_preserves_continuous_path_vertices() {
    // A rounded shape keeps `preserveVertices` on whatever its roughness, so
    // the path stays continuous through its arcs.
    let source = std::str::from_utf8(OUTLINE_FAMILIES).unwrap().replacen(
        "\"seed\":2,\"updated\"",
        "\"roughness\":2,\"seed\":2,\"updated\"",
        1,
    );
    let paths = rough_svg_paths_of(source.as_bytes());
    assert_eq!(
        sha256_hex(&paths[1]),
        "e9be3be60300e30a0f37ea1c58c8a7b18ddf966d468a0afb6629b3fc5f81aa7f"
    );
}

#[test]
fn rough_fill_vectors_match_the_pinned_geometry_families() {
    // One fill family per shape family: solid on a rounded rectangle,
    // hachure on an ellipse, cross-hatch on a diamond and zigzag on a closed
    // line loop. Each element writes its fill path and then its outline.
    let paths = rough_svg_paths_of(
        br##"{
      "type":"excalidraw","version":2,
      "elements":[
        {"id":"rounded","type":"rectangle","x":10,"y":10,"width":80,"height":50,"roundness":{"type":3},"backgroundColor":"#e03131","fillStyle":"solid","seed":21,"updated":1},
        {"id":"ellipse","type":"ellipse","x":110,"y":10,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"hachure","seed":22,"updated":1},
        {"id":"diamond","type":"diamond","x":210,"y":10,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"cross-hatch","seed":23,"updated":1},
        {"id":"loop","type":"line","x":310,"y":10,"width":80,"height":50,"points":[[0,0],[80,0],[80,50],[0,50],[0,0]],"backgroundColor":"#e03131","fillStyle":"zigzag","seed":24,"updated":1}
      ],
      "appState":{},"files":{}
    }"##,
    );
    assert_eq!(paths.len(), 8);
    assert_eq!(
        sha256_hex(&paths.join("\n")),
        // Moved with the diamond vertex fix: the scene carries two diamonds.
        // `tools/rough-filler-vectors/derive.mjs` reproduces this digest from
        // the pinned npm Rough.js, which is how that derivation was itself
        // checked before it pinned the three fillers below.
        "d4e7528ff1a10f3ebbfbb8f3b404728ce7bef805930d56384652c2be6dc53f6d"
    );
}

/// The three fillers ported for #137 — dots, dashed and zigzag-line — over
/// every shape family and option that changes what they draw: the dot
/// filler's ellipses under an ellipse element's `curveFitting` 1, a dashed
/// stroke style's single stroke and roughness 0's core stroke; the dashed
/// filler under roughness 2, where the fill lines no longer preserve their
/// vertices; the zigzag-line filler over a rounded path's sampled boundary
/// and at roughness 0; and stroke widths 1 and 4, which scale the gap, the
/// fill weight and the dot scatter.
const FILLER_FAMILIES: &[u8] = br##"{
  "type":"excalidraw","version":2,
  "elements":[
    {"id":"dots-rectangle","type":"rectangle","x":10,"y":10,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dots","seed":41,"updated":1},
    {"id":"dashed-ellipse","type":"ellipse","x":110,"y":10,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dashed","seed":42,"updated":1},
    {"id":"zigzag-line-diamond","type":"diamond","x":210,"y":10,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"zigzag-line","seed":43,"updated":1},
    {"id":"dots-loop","type":"line","x":310,"y":10,"width":80,"height":50,"points":[[0,0],[80,0],[80,50],[0,50],[0,0]],"backgroundColor":"#e03131","fillStyle":"dots","seed":44,"updated":1},
    {"id":"dots-ellipse","type":"ellipse","x":10,"y":110,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dots","seed":45,"updated":1},
    {"id":"dots-dashed-stroke","type":"rectangle","x":110,"y":110,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dots","strokeStyle":"dashed","seed":46,"updated":1},
    {"id":"zigzag-line-architect","type":"rectangle","x":210,"y":110,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"zigzag-line","roughness":0,"seed":47,"updated":1},
    {"id":"dots-architect","type":"rectangle","x":310,"y":110,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dots","roughness":0,"seed":48,"updated":1},
    {"id":"dashed-cartoonist","type":"diamond","x":10,"y":210,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dashed","roughness":2,"seed":49,"updated":1},
    {"id":"zigzag-line-rounded","type":"rectangle","x":110,"y":210,"width":80,"height":50,"roundness":{"type":3},"backgroundColor":"#e03131","fillStyle":"zigzag-line","seed":50,"updated":1},
    {"id":"dots-thin","type":"rectangle","x":210,"y":210,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dots","strokeWidth":1,"seed":51,"updated":1},
    {"id":"dashed-thick","type":"rectangle","x":310,"y":210,"width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dashed","strokeWidth":4,"seed":52,"updated":1}
  ],
  "appState":{},"files":{}
}"##;

#[test]
fn dots_dashed_and_zigzag_line_fills_match_the_pinned_roughjs_vectors() {
    // Every digest here is the pinned npm Rough.js 4.6.4's, computed with
    //
    //   node tools/rough-filler-vectors/derive.mjs \
    //     tools/reference-oracle/node_modules/roughjs/bundled/rough.esm.js \
    //     <<< "$FILLER_FAMILIES"
    //
    // under Node 24.19.0 after `tools/reference-oracle/run-container.sh
    // install`. The script drives the generator exactly as Excalidraw
    // 0.18.1's `generateRoughOptions` does and serializes each set with
    // `opsToPath(set, 2)`; it reproduces the digest of the four older fill
    // families above before it derives these. The dot filler's `Math.random()`
    // scatter is stubbed there with a second `Random(seed)`, the sample the
    // renderer draws (`rough::dots_fill`, #137).
    const EXPECTED: &[(&str, usize, &str)] = &[
        (
            "dots-rectangle",
            1188,
            "3e71799dc22c6afd54dfbe1034b6f7104760ebfcf3d2a814f21f923f73cea877",
        ),
        (
            "dashed-ellipse",
            84,
            "86d629cf4d59ca89c11c073e8279b5f6a09eb11f0d47c920f49439f7c614b0cf",
        ),
        (
            "zigzag-line-diamond",
            64,
            "b323939a39be484b021100620064bab064c5bb7ef4fa8121e1182710c52e30b5",
        ),
        (
            "dots-loop",
            1188,
            "76d1235c7c0a3fd46fb490ff13c8ab44134efaaa03a15bb9ba921309e73b8a60",
        ),
        (
            "dots-ellipse",
            880,
            "77df6210a6c16cb8fdde92bf1cc8e423d9f790ef1e5c52b85acf93a643ced89e",
        ),
        (
            "dots-dashed-stroke",
            594,
            "dcfdccfb6ac6856ad5237f2257af7eca8ff6eea0354d67f6c01dbe88e9eef57f",
        ),
        (
            "zigzag-line-architect",
            128,
            "03e2ade75433cf28e905e69618977a8e75522ee991742ca8805a660df7e7d223",
        ),
        (
            "dots-architect",
            2052,
            "42851d1b624c63f7e7706a2276e9e64e33f603269a63ec04a9a76a348570e4a0",
        ),
        (
            "dashed-cartoonist",
            48,
            "1a5f688a815924ae900ac630a78df606a4944419cfb81c6980b57bfa4cf5fd8a",
        ),
        (
            "zigzag-line-rounded",
            112,
            "347d6745eda4b217c930cee4004f5e8cb16775d5eeb65ab4907cbba366981af7",
        ),
        (
            "dots-thin",
            5016,
            "ebe7c571ae6a35408d58e83c8a72817ed5ecc1792f6a31b7889c28ea1bd46d85",
        ),
        (
            "dashed-thick",
            32,
            "7de2fa24211308f4421c2213a8568cd803dea719023bc3f3e78d0cd88dd7651f",
        ),
    ];
    let elements = normalized_elements(FILLER_FAMILIES);
    assert_eq!(elements.len(), EXPECTED.len());
    let mut all_paths = Vec::new();
    for (element, (id, fill_operations, fill_digest)) in elements.iter().zip(EXPECTED) {
        assert_eq!(element.source.id(), *id);
        let style = ["zigzag-line", "dots", "dashed"]
            .into_iter()
            .find(|style| id.starts_with(style))
            .expect("every filler family is named after its filler");
        assert_eq!(
            element.fill_style, style,
            "{id} keeps its fill style through normalization"
        );
        let paths = rough_svg_paths(element);
        assert_eq!(paths.len(), 2, "{id} writes its fill and then its outline");
        assert_eq!(
            paths[0].matches(['M', 'C', 'L']).count(),
            *fill_operations,
            "{id} fill operation count"
        );
        assert_eq!(sha256_hex(&paths[0]), *fill_digest, "{id} fill vector");
        all_paths.extend(paths);
    }
    assert_eq!(
        sha256_hex(&all_paths.join("\n")),
        "a36d0d5ee959954f66696483f640460a68608e36f3c7e214b3b9e4e0bc39dbb3",
        "every fill and outline of the filler families"
    );
}

#[test]
fn dot_scatter_is_seeded_apart_from_the_dot_ellipses() {
    // The reference scatters dot centres with `Math.random()`; revision 1
    // draws that scatter from a second generator seeded with the element
    // seed, so the same element draws the same dots twice, two seeds draw
    // different dots, and the scatter never touches the outline, which is
    // generated before the fill from the same seeded stream.
    let element = |seed: u32| {
        format!(
            r##"{{"type":"excalidraw","version":2,"elements":[{{"id":"dots","type":"rectangle","width":80,"height":50,"backgroundColor":"#e03131","fillStyle":"dots","seed":{seed},"updated":1}}],"appState":{{}},"files":{{}}}}"##
        )
    };
    let first = rough_svg_paths_of(element(41).as_bytes());
    assert_eq!(first, rough_svg_paths_of(element(41).as_bytes()));
    let second = rough_svg_paths_of(element(42).as_bytes());
    assert_ne!(first[0], second[0]);
    let plain = rough_svg_paths_of(
        element(41)
            .replace(r##""backgroundColor":"#e03131","fillStyle":"dots","##, "")
            .as_bytes(),
    );
    assert_eq!(plain.len(), 1);
    assert_eq!(first[1], plain[0], "the outline does not see the scatter");
}

#[test]
fn patterned_curved_boundaries_match_pinned_js_sampling() {
    // Hachure over a curved boundary samples the curve the way the pinned
    // Rough.js run does: the rounded rectangle's arcs and the rounded line's
    // spline are both fill boundaries here.
    let paths = rough_svg_paths_of(
        br##"{
      "type":"excalidraw","version":2,
      "elements":[
        {"id":"rounded-path","type":"rectangle","x":10,"y":10,"width":80,"height":50,"roundness":{"type":3},"backgroundColor":"#e03131","fillStyle":"hachure","seed":31,"updated":1},
        {"id":"rounded-curve","type":"line","x":110,"y":10,"width":80,"height":50,"points":[[0,0],[80,0],[80,50],[0,50],[4,3]],"roundness":{"type":2},"backgroundColor":"#e03131","fillStyle":"hachure","seed":32,"updated":1}
      ],
      "appState":{},"files":{}
    }"##,
    );
    assert_eq!(paths.len(), 4);
    assert_eq!(
        sha256_hex(&paths[0]),
        "c211c4346fa692dc825164e947714aaed5b66e99ae2fcecdfe43ef656683bfb0",
        "rounded path fill was generated independently with pinned Rough.js 4.6.4",
    );
    assert_eq!(
        sha256_hex(&paths[2]),
        "9176fe003ade6804f0f3e9fe2b79aa67835300b4999ac2c72e43e639d0748f06",
        "rounded curve fill was generated independently with pinned Rough.js 4.6.4",
    );
}

#[test]
fn cartoonist_curve_fill_drops_its_vertices_like_the_pinned_roughjs() {
    // The rounded curve of `patterned_curved_boundaries_match_pinned_js_sampling`
    // at roughness 2 (#158). The reference fills a rounded line through
    // `generateRoughOptions(element)` with no `continuousPath`, so its
    // `preserveVertices` is the raw `roughness < 2`: off here, and every
    // hachure line's endpoints jitter. The digests are the pinned npm Rough.js
    // 4.6.4's, computed with
    //
    //   node tools/rough-filler-vectors/derive.mjs \
    //     tools/reference-oracle/node_modules/roughjs/bundled/rough.esm.js \
    //     <<< "$SCENE"
    //
    // under Node 24.19.0 after `tools/reference-oracle/run-container.sh
    // install`; the outline never reads `preserveVertices` and matched before
    // the fix, and the roughness-1 fill above is unchanged by it.
    let paths = rough_svg_paths_of(
        br##"{
      "type":"excalidraw","version":2,
      "elements":[
        {"id":"rounded-curve","type":"line","x":110,"y":10,"width":80,"height":50,"points":[[0,0],[80,0],[80,50],[0,50],[4,3]],"roundness":{"type":2},"backgroundColor":"#e03131","fillStyle":"hachure","roughness":2,"seed":32,"updated":1}
      ],
      "appState":{},"files":{}
    }"##,
    );
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0].matches(['M', 'C', 'L']).count(), 48);
    assert_eq!(
        sha256_hex(&paths[0]),
        "e9c556b28004a9a7c9425d26d770e182af260fcf131b4a03304fd2d8da9c37f5",
        "cartoonist rounded curve fill was generated independently with pinned Rough.js 4.6.4",
    );
    assert_eq!(
        sha256_hex(&paths[1]),
        "96e733b3bd8d647459779543a05f1e8e598f6635ac2def9aa23f8fdf889f36c1",
        "cartoonist rounded curve outline was generated independently with pinned Rough.js 4.6.4",
    );
}

/// The SVG path data a default render writes for one arrow, in the reference's
/// order: the outline, then each arrowhead's sets (a closed head's fill and
/// outline, an open head's line or lines), the start head before the end
/// head; and beside them the `strokeLineDash` each head was generated under.
fn arrow_svg_paths_and_dashes(
    source: &NormalizedElement,
) -> (Vec<String>, Vec<rough::DecorationDash>) {
    let drawing = rough_drawing(
        source,
        adjusted_roughness(source),
        &mut default_budget(source),
    )
    .ok()
    .unwrap_or_else(|| panic!("{} generates", source.source.id()));
    let path_data = |path: &geometry::Path| {
        let mut data = String::new();
        svg::push_rough_path(&mut data, path).expect("path data writes");
        data
    };
    assert!(drawing.fill.is_none(), "an arrow has no fill");
    let mut paths = vec![path_data(&drawing.outline)];
    let mut dashes = Vec::new();
    for decoration in &drawing.decorations {
        paths.extend(
            decoration
                .drawing
                .fill
                .iter()
                .map(|fill| path_data(&fill.path)),
        );
        paths.push(path_data(&decoration.drawing.outline));
        dashes.push(decoration.dash);
    }
    (paths, dashes)
}

/// The four arrows of `focused.cartoonist-arrowheads.v1` with their points
/// normalized as the reference's restore path normalizes them, at roughness 2:
/// an `arrow` start followed by a `dot` end (the control), a `dot` start
/// followed by an `arrow` end, a `circle_outline` start followed by a
/// `triangle` end, and a `crowfoot_one` end alone.
const CARTOONIST_ARROWHEAD_FAMILIES: &[u8] = br##"{
  "type":"excalidraw","version":2,
  "elements":[
    {"id":"arrow-start-then-dot","type":"arrow","x":10,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":"arrow","endArrowhead":"dot","seed":1547013,"updated":1},
    {"id":"dot-start-then-arrow","type":"arrow","x":170,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":"dot","endArrowhead":"arrow","seed":1547026,"updated":1},
    {"id":"circle-outline-start-then-triangle","type":"arrow","x":330,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":"circle_outline","endArrowhead":"triangle","seed":1547039,"updated":1},
    {"id":"crowfoot-one-end","type":"arrow","x":490,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":null,"endArrowhead":"crowfoot_one","seed":1547052,"updated":1}
  ],
  "appState":{},"files":{}
}"##;

#[test]
fn cartoonist_arrowheads_clamp_their_roughness_per_kind_like_the_pinned_roughjs() {
    // Every digest here is the pinned npm Rough.js 4.6.4's, computed with
    //
    //   node tools/rough-filler-vectors/derive.mjs \
    //     tools/reference-oracle/node_modules/roughjs/bundled/rough.esm.js \
    //     <<< "$CARTOONIST_ARROWHEAD_FAMILIES"
    //
    // under Node 24.19.0 after `tools/reference-oracle/run-container.sh
    // install`; the script generates the heads through a verbatim
    // `getArrowheadShapes` over the one options object the outline and both
    // heads share, and over this scene it reproduces all 17 paths of the
    // official export of `focused.cartoonist-arrowheads.v1` byte for byte
    // (#171). A dot-kind head clamps its own roughness to 0.5 and a triangle
    // or diamond its own to 1, each on a spread copy, so the end head after a
    // `dot` or `circle_outline` start is drawn at 1, not 0.5; a `crowfoot_one`
    // is drawn at the element's roughness, 2 here, unclamped.
    const EXPECTED: &[(&str, &[usize], &[&str])] = &[
        (
            "arrow-start-then-dot",
            &[8, 4, 4, 22, 22],
            &[
                "63d6ec6b5d4df4cdf68fcb81c06adf76ebfbd443275b3db7e3afc6360b38fc6f",
                "d92964190e08ce94c33018250a042651f769c52d4108d3ca8361173bc7fd4e81",
                "5477a590dc9b08cbf98eb73fedddac943ec23d8214fef9432723360239c3e26f",
                "7522ef6b2bed77d2ccc77385090e2a12c90129677d55040425952930a7b89815",
                "a91e01e6fef2dec7181789f7b36da79fc71a573c5762fb9d62963f284e73fb68",
            ],
        ),
        (
            "dot-start-then-arrow",
            &[8, 22, 22, 4, 4],
            &[
                "31d22b605da1e2d0b75fc2de90fecb09bc5908ecb3f2111a8ea0e0be787e66c0",
                "d965980132f6badc1f635d7f73a36948537b7271c52f6c4dee8ff578ba27980a",
                "89d4480fc99b9fc23976f9541974ceb4a9f41dc3466533ea117e9517925a5387",
                "9a223af957769bb35b5a6ca27a1e0c563b91b7b8da0f81e342d85b5830f5c929",
                "034cbf834ecf1c8ba54aa3b141dcb5e225e558dc53feb2e5aa135d3337f39672",
            ],
        ),
        (
            "circle-outline-start-then-triangle",
            &[8, 22, 22, 4, 16],
            &[
                "155646a17a2883872d408e4371d9368ff16e4daefcf9ec28f53d94df90096cb9",
                "eeed92a61c3a32cc8043ada8823b769dcc2fbe55c7ad54220d941c5b1cc4dc97",
                "c11fa53245d15afca589dccd458787cc501c301a9f5d0b14532ad16850fe6386",
                "7d0b32501fa21fc5187352e73cb6225347a07eaf23a000c29fbb4064cbce03f5",
                "ce98f244cf03740d7aab172fe3c5d6d5237b1e5a38af7069b6ebee14aa1c2343",
            ],
        ),
        (
            "crowfoot-one-end",
            &[8, 4],
            &[
                "5e428d4053927e86bcb62948854c90cb78fe5e6dd8cc4c487086cc11d22c581a",
                "3b89eb916f4b378ec02a4da616860a5c3a938f588e2059d1deb46c37e3a1c116",
            ],
        ),
    ];
    let elements = normalized_elements(CARTOONIST_ARROWHEAD_FAMILIES);
    assert_eq!(elements.len(), EXPECTED.len());
    let mut all_paths = Vec::new();
    for (element, (id, operations, digests)) in elements.iter().zip(EXPECTED) {
        assert_eq!(element.source.id(), *id);
        let (paths, dashes) = arrow_svg_paths_and_dashes(element);
        assert!(
            dashes
                .iter()
                .all(|dash| *dash == rough::DecorationDash::Solid),
            "{id}: a solid element's heads carry no dash"
        );
        assert_eq!(
            paths
                .iter()
                .map(|path| path.matches(['M', 'C', 'L']).count())
                .collect::<Vec<_>>(),
            *operations,
            "{id}: operation counts"
        );
        assert_eq!(
            paths
                .iter()
                .map(|path| sha256_hex(path))
                .collect::<Vec<_>>(),
            *digests,
            "{id}: the pinned npm Rough.js 4.6.4 arrowhead vectors"
        );
        all_paths.extend(paths);
    }
    assert_eq!(
        sha256_hex(&all_paths.join("\n")),
        "d427a4bb37c9e62549c6b5753988404b2346500d10170362c9a64c765d6e2498",
        "the cartoonist arrowhead scene joined, as the official export's 17 paths join"
    );
}

/// Arrows whose heads read what an earlier head left on the shared options
/// object: the default branch's roughness clamp and dash carried to a
/// `crowfoot_one` that follows, a `crowfoot_one` start keeping the element's
/// own dashed or dotted pattern before an open or closed end head replaces or
/// deletes it, a closed start head deleting the pattern a `crowfoot_one` end
/// would otherwise keep, a `crowfoot_one_or_many` whose crossbar is drawn
/// after its own branch's assignments, and a rounded (curve) arrow with a
/// `bar` and a `crowfoot_many`.
const ARROWHEAD_SHARED_OPTION_FAMILIES: &[u8] = br##"{
  "type":"excalidraw","version":2,
  "elements":[
    {"id":"arrow-then-crowfoot-one","type":"arrow","x":10,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":"arrow","endArrowhead":"crowfoot_one","seed":81,"updated":1},
    {"id":"crowfoot-one-then-arrow","type":"arrow","x":170,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":"crowfoot_one","endArrowhead":"arrow","seed":82,"updated":1},
    {"id":"crowfoot-one-then-triangle","type":"arrow","x":330,"y":40,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roughness":2,"startArrowhead":"crowfoot_one","endArrowhead":"triangle","seed":83,"updated":1},
    {"id":"dotted-arrow-then-crowfoot-one","type":"arrow","x":10,"y":120,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"strokeStyle":"dotted","startArrowhead":"arrow","endArrowhead":"crowfoot_one","seed":84,"updated":1},
    {"id":"dotted-crowfoot-one-then-arrow","type":"arrow","x":170,"y":120,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"strokeStyle":"dotted","startArrowhead":"crowfoot_one","endArrowhead":"arrow","seed":85,"updated":1},
    {"id":"dashed-crowfoot-one-then-diamond","type":"arrow","x":330,"y":120,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"strokeStyle":"dashed","strokeWidth":4,"startArrowhead":"crowfoot_one","endArrowhead":"diamond","seed":86,"updated":1},
    {"id":"dashed-dot-then-crowfoot-one","type":"arrow","x":10,"y":200,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"strokeStyle":"dashed","startArrowhead":"dot","endArrowhead":"crowfoot_one","seed":87,"updated":1},
    {"id":"dotted-crowfoot-one-or-many","type":"arrow","x":170,"y":200,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"strokeStyle":"dotted","roughness":2,"startArrowhead":null,"endArrowhead":"crowfoot_one_or_many","seed":88,"updated":1},
    {"id":"rounded-bar-then-crowfoot-many","type":"arrow","x":330,"y":200,"width":140,"height":30,"points":[[0,0],[70,-30],[140,0]],"roundness":{"type":2},"roughness":2,"startArrowhead":"bar","endArrowhead":"crowfoot_many","seed":89,"updated":1}
  ],
  "appState":{},"files":{}
}"##;

#[test]
fn arrowheads_read_the_shared_options_in_the_pinned_roughjs_order() {
    use rough::DecorationDash::{Dashed, Dotted, DottedHead, Solid};

    // Every digest and dash here is the pinned npm Rough.js 4.6.4's, computed
    // with
    //
    //   node tools/rough-filler-vectors/derive.mjs \
    //     tools/reference-oracle/node_modules/roughjs/bundled/rough.esm.js \
    //     <<< "$ARROWHEAD_SHARED_OPTION_FAMILIES"
    //
    // under Node 24.19.0; the script prints each head's `strokeLineDash` as
    // Rough.js copied it from the shared options at generation time (#171).
    // The default branch assigns `roughness = min(1, roughness)` and the
    // dash on the shared object, so the `crowfoot_one` after an `arrow` start
    // is drawn at 1 where the one before an `arrow` end is drawn at 2, and
    // the one after a dotted `arrow` start takes the shortened `[1.5, 6]`
    // where the one before it keeps the element's `[1.5, 8]`; a closed head
    // deletes the pattern (the `crowfoot_one` after a `dot` start on a dashed
    // element is solid) and leaves the roughness alone (the `arrow` end after
    // a `crowfoot_one` start is still clamped to 1 by its own branch).
    /// One arrow: its id, the operation count of each path, each head's
    /// `strokeLineDash`, and each path's digest.
    type ExpectedArrow = (
        &'static str,
        &'static [usize],
        &'static [rough::DecorationDash],
        &'static [&'static str],
    );
    const EXPECTED: &[ExpectedArrow] = &[
        (
            "arrow-then-crowfoot-one",
            &[8, 4, 4, 4],
            &[Solid, Solid, Solid],
            &[
                "91ed074143383355aaeefe46d6c646c84b9842a8ac84ac722c3ade0ec29db719",
                "d035c0650df83c8fededbf613f7e5bfd1c48459daa42399505732edfc616967b",
                "bc06591c670286d638686998dd2668178b8b8f2c46125efb1f5cd6ab20be15ae",
                "9869288d223566a218162f1624e45da2a628406d5f3600660dbb48bac06c8751",
            ],
        ),
        (
            "crowfoot-one-then-arrow",
            &[8, 4, 4, 4],
            &[Solid, Solid, Solid],
            &[
                "d28baed91b1756207816d1ce96e90ad275087ec7f8a67df01a036518829e69e8",
                "2348038bad60c4097a4f2cad05b485b1bc1caf2ca6af7e52ace01b19c8f853ff",
                "1a6084153037919f446376e31fa04e447da9712d19384c483c23c75a559e96e6",
                "1c61cc00b912227ecc37a7987828bb8be5f1c4058d03adc5edf582bb6af4f23d",
            ],
        ),
        (
            "crowfoot-one-then-triangle",
            &[8, 4, 4, 16],
            &[Solid, Solid],
            &[
                "27dc1a9cb135c0892168a24a9cc635f599843e3c21d15fc713feca174a420e49",
                "a4fcee2db146fd2b1cab9a4d35499898f533d73900166f0f90272d54b426b7d5",
                "2b294f93cbe55f1ceb4c2fe80ebb8f67871c8a2c51c5f0415d9b6506603a7a39",
                "6f84c93ecdc2b4a9f05c85b8d15d7a2baf5aa2c0c2e86b149401a2bb7654673b",
            ],
        ),
        (
            "dotted-arrow-then-crowfoot-one",
            &[4, 2, 2, 2],
            &[DottedHead, DottedHead, DottedHead],
            &[
                "5585ddce561df575da97854cfa1518c826c60c967ac68a176324ac4ad95b7325",
                "3dabf0c11eb4dedafd8666a2eb96482512d7eb4844da929263dad3ff5c59a51d",
                "3da66ba7ea29aae6ca24ec6826d1bcae42eba1658399dc7c01a2b7fc02c5b8a6",
                "acb0e41f99e829e4943af212132802c0e4be1469314c424e631310607c393558",
            ],
        ),
        (
            "dotted-crowfoot-one-then-arrow",
            &[4, 2, 2, 2],
            &[Dotted, DottedHead, DottedHead],
            &[
                "9c8352720b68fb4db6bf21de9200282c5ef362c72042fdecfd9431cbacb52017",
                "9a0b855f17ee7c3413c95c345435f3f92e77ea815bbedc9e860e47e4931fb96c",
                "844be959100f6671f99be191f414711586650356faacc6449ba76b24206a9eba",
                "896269f36081df9fb810b6a2abf4b94a1c9731e2fe806ac1692ad316f22483de",
            ],
        ),
        (
            "dashed-crowfoot-one-then-diamond",
            &[4, 2, 5, 10],
            &[Dashed, Solid],
            &[
                "628558cb5082bccdeae0500abd2e20f45c4cd4607c3ad7acd59801de5eae39c0",
                "c9eafab5e267642e02028a110a989a784723512d0c7331e2040217ff1817cc3b",
                "238c7cecd884a7f4879fa33ed546298b36331cab62d9a1a49f1cbde99d179c7c",
                "8bf580c20d360d3830f7c051a95304bffc78e3a28879c3666d9873fd2bde3594",
            ],
        ),
        (
            "dashed-dot-then-crowfoot-one",
            &[4, 11, 11, 2],
            &[Solid, Solid],
            &[
                "65cb92a55d288d069735cc774b8d3c738d8ce7903e988650d7f043a5169758ca",
                "9e5d7b0e047d9dc4a746a9457ba9f5d4edfc7f676eb44d91488a6b7443e54668",
                "8f9cd326eda9e03d5d05cd16f7679165dbfa72213ad0a1cfbeabffc745704260",
                "594de913cf7d7c719ff19e35bb91df36a4f993e4c6ef7c18531d0a2ceb8642ca",
            ],
        ),
        (
            "dotted-crowfoot-one-or-many",
            &[4, 2, 2, 2],
            &[DottedHead, DottedHead, DottedHead],
            &[
                "3c7f6c089d1bdb4ee803095b4c6e7350566ba8decd88638e3a4262d10dd8f839",
                "cd5039eb442cb95ba4a50944f303530a67e6d416224a6aaaf662ea3e57d3faa7",
                "578a8d9bc68fe9e3b9f73fc2e4e02d19955fb5e0a77b3a84b4b86cd1184355bf",
                "9faf05b6774b9270e898f68b1de63c0b2e42ff4939ab0b544a5ade2c574a673b",
            ],
        ),
        (
            "rounded-bar-then-crowfoot-many",
            &[6, 4, 4, 4, 4],
            &[Solid, Solid, Solid, Solid],
            &[
                "3bd21bd3eb48e6aa2a106238a0432308110d3dbef43c91ece618912aa72f4d9a",
                "7cc61211efae773d734726c9b15d85b45e484fc9dc5f6e556c2792677bd004a7",
                "b05359a809f0fa977d90c85b004cf755ee94bd7e90208fe942dd80c0b44c8aef",
                "693c7d832379be817f236f5152513bddfd7539f34b678f4c10629a4252a71b68",
                "5f5454e1ff9d9aacc98b9f70975ff55b498c0e5004b85d3ec005889190f09054",
            ],
        ),
    ];
    let elements = normalized_elements(ARROWHEAD_SHARED_OPTION_FAMILIES);
    assert_eq!(elements.len(), EXPECTED.len());
    let mut all_paths = Vec::new();
    for (element, (id, operations, expected_dashes, digests)) in elements.iter().zip(EXPECTED) {
        assert_eq!(element.source.id(), *id);
        let (paths, dashes) = arrow_svg_paths_and_dashes(element);
        assert_eq!(dashes, *expected_dashes, "{id}: each head's strokeLineDash");
        assert_eq!(
            paths
                .iter()
                .map(|path| path.matches(['M', 'C', 'L']).count())
                .collect::<Vec<_>>(),
            *operations,
            "{id}: operation counts"
        );
        assert_eq!(
            paths
                .iter()
                .map(|path| sha256_hex(path))
                .collect::<Vec<_>>(),
            *digests,
            "{id}: the pinned npm Rough.js 4.6.4 arrowhead vectors"
        );
        all_paths.extend(paths);
    }
    assert_eq!(
        sha256_hex(&all_paths.join("\n")),
        "c355b2160da0c2c19677b94c78e08d677c95571845f189e96aded11366958375"
    );
}

#[test]
fn line_fill_uses_the_exact_export_zoom_loop_threshold() {
    // The export treats a line as closed, and fills it, when its last point
    // sits within `LINE_CONFIRM_THRESHOLD / zoom` of its first; `[6.4, 4.8]`
    // is exactly 8 away from the origin and `[6.408, 4.806]` just beyond.
    let elements = normalized_elements(
        br##"{
      "type":"excalidraw","version":2,
      "elements":[
        {"id":"at-threshold","type":"line","x":10,"y":10,"width":40,"height":30,"points":[[0,0],[40,0],[40,30],[6.4,4.8]],"backgroundColor":"#e03131","fillStyle":"solid","seed":51,"updated":1},
        {"id":"outside-threshold","type":"line","x":60,"y":10,"width":40,"height":30,"points":[[0,0],[40,0],[40,30],[6.408,4.806]],"backgroundColor":"#e03131","fillStyle":"solid","seed":52,"updated":1}
      ],
      "appState":{},"files":{}
    }"##,
    );
    let drawing = |element: &NormalizedElement| {
        rough_drawing(
            element,
            adjusted_roughness(element),
            &mut default_budget(element),
        )
        .expect("line generates")
    };
    let closed = drawing(&elements[0]);
    assert!(closed.fill.is_some(), "the loop within the threshold fills");
    assert!(closed.shape.svg_uses_even_odd_fill());
    assert_eq!(rough_svg_paths(&elements[0]).len(), 2);
    let open = drawing(&elements[1]);
    assert!(open.fill.is_none(), "the loop beyond the threshold is open");
    assert_eq!(rough_svg_paths(&elements[1]).len(), 1);
}

#[test]
fn repeated_loop_endpoint_keeps_the_polygon_generators_extra_close() {
    // Rough.js closes a polygon by drawing back to its first point; a line
    // whose last point already repeats the first still gets that closing
    // segment, degenerate as it is, in both passes.
    let paths = rough_svg_paths_of(
        br##"{
      "type":"excalidraw","version":2,
      "elements":[
        {"id":"repeated","type":"line","x":10,"y":10,"width":40,"height":30,"points":[[0,0],[40,0],[40,30],[0,0]],"backgroundColor":"#e03131","fillStyle":"solid","seed":61,"updated":1}
      ],
      "appState":{},"files":{}
    }"##,
    );
    let [_fill, outline] = paths.as_slice() else {
        panic!("a filled polygon writes its fill and outline: {paths:?}");
    };
    assert!(
        outline.ends_with("M0 0 C0 0, 0 0, 0 0 M0 0 C0 0, 0 0, 0 0"),
        "unexpected repeated-close outline: {outline}",
    );
}

#[test]
fn freedraw_strokes_match_the_pinned_perfect_freehand_vectors() {
    // perfect-freehand 1.2.0 as Excalidraw v0.18.1 drives it: simulated
    // pressure, explicit pressure samples, a pressureless dot, and omitted
    // samples taking the library's defaults.
    for (source, expected_digest) in [
        (
            br##"{"type":"excalidraw","version":2,"elements":[{"id":"simulated","type":"freedraw","x":10,"y":20,"width":80,"height":30,"points":[[0,0],[20,10],[40,5],[60,30],[80,20]],"pressures":[],"simulatePressure":true,"strokeColor":"#e03131","strokeWidth":2,"opacity":80,"updated":1}],"appState":{},"files":{}}"##.as_slice(),
            "9adff0c560710203718626bc817520baba2819a06898054e0b55d52a7cccd41e",
        ),
        (
            br##"{"type":"excalidraw","version":2,"elements":[{"id":"pressure","type":"freedraw","width":50,"height":30,"points":[[0,0],[15,20],[35,10],[50,30]],"pressures":[0.1,0.4,0.9,0.3],"simulatePressure":false,"strokeWidth":3,"updated":1}],"appState":{},"files":{}}"##.as_slice(),
            "0bb4552d75b1f4b8a39f1d5721b052c288db07d3d51e7b5303ab3c6ffffd0f1e",
        ),
        (
            br##"{"type":"excalidraw","version":2,"elements":[{"id":"dot","type":"freedraw","points":[],"pressures":[],"simulatePressure":false,"strokeWidth":3,"updated":1}],"appState":{},"files":{}}"##.as_slice(),
            "4a84f3104dec0d94b7a702c18b64304d0affeb8c00d21e8e08a4f0731236078a",
        ),
        (
            br##"{"type":"excalidraw","version":2,"elements":[{"id":"defaults","type":"freedraw","width":40,"height":10,"points":[[0,0],[20,10],[40,5]],"simulatePressure":false,"strokeWidth":2,"updated":1}],"appState":{},"files":{}}"##.as_slice(),
            "98f047202f13fe696e5e6c715b343696f61f6460497f110bd2efae09e009289c",
        ),
    ] {
        let elements = normalized_elements(source);
        let path = freedraw_svg_path(&elements[0]);
        assert_eq!(
            sha256_hex(&path),
            expected_digest,
            "{}",
            elements[0].source.id()
        );
    }
}

#[test]
fn freedraw_loop_fills_match_the_pinned_roughjs_vectors() {
    // The four loops of `focused.freedraw-loop-fills.v1`: one 13-point loop
    // filled solid and hachure at roughness 2, hachure at roughness 1 and at
    // roughness 0, each at its own seed. Every digest is the pinned npm
    // Rough.js 4.6.4's, computed with
    //
    //   node tools/rough-filler-vectors/derive.mjs \
    //     tools/reference-oracle/node_modules/roughjs/bundled/rough.esm.js \
    //     < tests/conformance/cases/focused/freedraw-loop-fills/scene.excalidraw
    //
    // under Node 24.19.0 after `tools/reference-oracle/run-container.sh
    // install`; the script draws the loop's background as the reference's
    // `generateElementShape` does, a `generator.curve` over the
    // points-on-curve simplification at 0.75 with `stroke: "none"` and the
    // element's own seed and adjusted roughness, whose outline the generator
    // draws and discards before the fill reads the same seeded stream (#170).
    // The digests are the official export's: the issue records them as
    // `eb9b70f9…`, `29197100…`, `fbcd1653…` and `c230e85a…`.
    const EXPECTED: &[(&str, usize, &str)] = &[
        (
            "loop-solid-cartoonist",
            10,
            "eb9b70f933f01b01d66a3367843285c7d6337889619a92401d0d0ccfa7a694d4",
        ),
        (
            "loop-hachure-cartoonist",
            48,
            "2919710051c9c97a47f25efd7eabc0006b2914375ea275462ec7ddeaa73246b1",
        ),
        (
            "loop-hachure-artist",
            48,
            "fbcd165335e213f07a95e4ea79d994cbe54fc2eab3156ea065b057dc304779d6",
        ),
        (
            "loop-hachure-architect",
            48,
            "c230e85a2e614ad9c3d1561eaea1fe512827d9e862b91c5fff0b790b319d16da",
        ),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source =
        fs::read(root.join("tests/conformance/cases/focused/freedraw-loop-fills/scene.excalidraw"))
            .expect("the freedraw-loop-fills scene is checked in");
    let scene: serde_json::Value = serde_json::from_slice(&source).expect("the scene is JSON");
    let elements = normalized_elements(&source);
    assert_eq!(elements.len(), EXPECTED.len());
    let mut all_paths = Vec::new();
    for ((element, source_element), (id, operations, digest)) in elements
        .iter()
        .zip(scene["elements"].as_array().expect("elements"))
        .zip(EXPECTED)
    {
        assert_eq!(element.source.id(), *id);
        let NormalizedElementKind::Freedraw(freedraw) = &element.kind else {
            panic!("{id} is a freedraw element");
        };
        // The element's own seed and roughness, which the revision-1 scene
        // surface admits for a freedraw element as for every other shape
        // (#170) and resolution hands the fill.
        let seed = i32::try_from(source_element["seed"].as_i64().expect("seed")).expect("i32 seed");
        let roughness = source_element["roughness"].as_f64().expect("roughness");
        assert_eq!(element.seed, seed, "{id} keeps its seed");
        assert_eq!(element.roughness, roughness, "{id} keeps its roughness");
        let mut budget = default_budget(element);
        let fill = freedraw::background_fill(
            &freedraw.points,
            element.seed,
            adjusted_roughness(element),
            element.roughness < 2.0,
            element.stroke_style != "solid",
            rough::FillStyle::from_normalized(&element.fill_style),
            element.stroke_width,
            &mut budget,
        )
        .expect("the loop fills")
        .expect("the loop closes within the threshold");
        let mut path = String::new();
        svg::push_rough_path(&mut path, &fill.path).expect("path data writes");
        assert_eq!(
            path.matches(['M', 'C', 'L']).count(),
            *operations,
            "{id} operations"
        );
        assert_eq!(
            sha256_hex(&path),
            *digest,
            "{id}: the pinned npm Rough.js 4.6.4 fill"
        );
        all_paths.push(path);
    }
    assert_eq!(
        sha256_hex(&all_paths.join("\n")),
        "276ad4e738dfa83c8e77fe1c95ea78676d0505381153f55559733f7811f922e7"
    );
}

#[test]
fn ellipse_ignores_inapplicable_roundness_without_consuming_randomness() {
    // The revision-1 manifest does not read `/roundness` on an ellipse, so a
    // rounded ellipse normalizes as a plain one and draws the same vectors:
    // the seeded generator is consumed identically.
    let rounded_source = std::str::from_utf8(OUTLINE_FAMILIES).unwrap().replacen(
        "\"seed\":3,\"updated\"",
        "\"roundness\":{\"type\":2},\"seed\":3,\"updated\"",
        1,
    );
    let plain = normalized_elements(OUTLINE_FAMILIES);
    let rounded = normalized_elements(rounded_source.as_bytes());
    assert_eq!(rounded[2].roundness, None);
    assert_eq!(rough_svg_paths(&plain[2]), rough_svg_paths(&rounded[2]));
}

#[test]
fn a_legacy_draw_element_normalizes_as_the_equivalent_line() {
    // Scenes saved before freedraw existed carry `type: "draw"`; the official
    // restore turns it into a line with the same points, so the two normalize
    // to the same linear kind and generate the same geometry.
    let source = |kind: &str| {
        format!(
            r#"{{"type":"excalidraw","version":2,"elements":[{{"id":"legacy","type":"{kind}","x":10,"y":20,"width":60,"height":20,"points":[[5,3],[30,20],[65,10]],"strokeWidth":2,"seed":17,"updated":1}}],"appState":{{}},"files":{{}}}}"#
        )
    };
    let legacy = normalized_elements(source("draw").as_bytes());
    let line = normalized_elements(source("line").as_bytes());
    assert_eq!(legacy[0].kind, line[0].kind);
    assert!(matches!(legacy[0].kind, NormalizedElementKind::Linear(_)));
    assert_eq!(
        (legacy[0].x, legacy[0].y, legacy[0].width, legacy[0].height),
        (line[0].x, line[0].y, line[0].width, line[0].height)
    );
    assert_eq!(rough_svg_paths(&legacy[0]), rough_svg_paths(&line[0]));
}

/// A scene with one Excalifont text element at (`x`, `y`) inside a roughness-0
/// rectangle that fixes the scene origin at (0, 0), so the text's device
/// position is the padding plus its own coordinates times the export scale.
fn anchored_text_scene(x: f64, y: f64, angle: f64) -> SceneDocument {
    SceneDocument::parse(
        format!(
            r##"{{"type":"excalidraw","version":2,"elements":[{{"id":"anchor","type":"rectangle","x":0,"y":0,"width":300,"height":120,"angle":0,"strokeColor":"#1e1e1e","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","roughness":0,"opacity":100,"groupIds":[],"frameId":null,"index":"a0","roundness":null,"seed":1,"version":1,"versionNonce":2,"isDeleted":false,"boundElements":null,"updated":1,"link":null,"locked":false}},{{"id":"probe","type":"text","x":{x},"y":{y},"width":200,"height":25,"angle":{angle},"strokeColor":"#1e1e1e","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","roughness":1,"opacity":100,"groupIds":[],"frameId":null,"index":"a1","roundness":null,"seed":1,"version":1,"versionNonce":2,"isDeleted":false,"boundElements":null,"updated":1,"link":null,"locked":false,"text":"Baseline probe","fontSize":20,"fontFamily":5,"textAlign":"left","verticalAlign":"top","containerId":null,"originalText":"Baseline probe","autoResize":true,"lineHeight":1.25}}],"appState":{{"viewBackgroundColor":"#ffffff"}},"files":{{}}}}"##
        )
        .as_bytes(),
    )
    .expect("anchored text scene is admitted")
}

/// The renderer's PNG for `scene` at `scale`, decoded to `(width, height,
/// RGBA8)`.
fn png_rgba(scene: &SceneDocument, scale: f64) -> (u32, u32, Vec<u8>) {
    let request = RenderRequest::builder(OutputFormat::Png)
        .sizing(Sizing::Scale(scale))
        .build()
        .expect("PNG request is valid");
    let outcome = Renderer::new()
        .render(scene, &request)
        .expect("anchored text scene renders");
    let mut decoder = ::png::Decoder::new(std::io::Cursor::new(outcome.artifact().bytes()));
    decoder.set_transformations(::png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().expect("PNG structure is valid");
    let mut rgba = vec![0; reader.output_buffer_size().expect("PNG buffer fits")];
    let frame = reader.next_frame(&mut rgba).expect("PNG pixels decode");
    rgba.truncate(frame.buffer_size());
    (frame.width, frame.height, rgba)
}

/// The ink of every row of `rows` inside the text's columns (the anchor
/// rectangle's horizontal strokes lie outside the rows, its vertical strokes
/// outside the columns): the sum of `255 - min(r, g, b)` per row.
fn row_ink(
    raster: &(u32, u32, Vec<u8>),
    columns: std::ops::Range<u32>,
    rows: std::ops::Range<u32>,
) -> Vec<u64> {
    let (width, _, rgba) = raster;
    rows.map(|y| {
        columns
            .clone()
            .map(|x| {
                let offset = ((y * width + x) * 4) as usize;
                let pixel = &rgba[offset..offset + 4];
                255 - u64::from(pixel[0].min(pixel[1]).min(pixel[2]))
            })
            .sum()
    })
    .collect()
}

#[test]
fn png_snaps_each_text_baseline_to_the_device_pixel_grid_like_the_canvas() {
    // Spec 8.5 (#169): the reference canvas's text rasterization rounds a glyph
    // run's device y to `floor(y + 0.5)` and keeps x fractional, so PNG fills
    // each line's outlines at the snapped baseline. Excalifont at 20 px with
    // line height 1.25 has its first baseline 17.62 below the element's top
    // (12.5 + 20 * (886 - 374) / 1000 / 2), so with the padding of 10 a text
    // at y = 10 has its device baseline at 37.62, which snaps to 38, and every
    // y whose baseline rounds to the same pixel fills the same rows.
    let columns = 20..200;
    let at = |y: f64| png_rgba(&anchored_text_scene(10.0, y, 0.0), 1.0);
    let reference = at(10.0);
    assert_eq!(at(10.3).2, reference.2, "37.92 snaps to 38 as 37.62 does");
    assert_eq!(at(10.5).2, reference.2, "38.12 snaps to 38 as 37.62 does");
    assert_eq!(
        at(10.87).2,
        reference.2,
        "38.49 snaps to 38: the snap is floor(y + 0.5), not ceil"
    );
    // A baseline past the half-pixel snaps to the next row: the same ink one
    // row lower, which is a shift of round(b) - b = +0.5 device pixels of the
    // exact outline against +0.38 for the reference position.
    let lower = at(10.9);
    let reference_rows = row_ink(&reference, columns.clone(), 14..60);
    let lower_rows = row_ink(&lower, columns.clone(), 14..60);
    assert!(reference_rows.iter().any(|ink| *ink > 0));
    assert_ne!(lower.2, reference.2);
    assert_eq!(
        lower_rows[1..],
        reference_rows[..reference_rows.len() - 1],
        "38.52 snaps to 39: the filled rows are the reference's shifted down by one"
    );
    // x stays fractional: a horizontal move of less than a pixel changes the
    // raster, as the canvas positions glyphs with horizontal subpixel precision.
    assert_ne!(
        png_rgba(&anchored_text_scene(10.3, 10.0, 0.0), 1.0).2,
        reference.2,
        "x is not snapped"
    );

    // At export scale 2 the snap is taken in device pixels: the device baseline
    // 2 * 37.62 = 75.24 snaps to 75, as does 75.44 (y = 10.1), while 75.64
    // (y = 10.2) snaps to 76, one device row lower.
    let scaled = |y: f64| png_rgba(&anchored_text_scene(10.0, y, 0.0), 2.0);
    let scaled_reference = scaled(10.0);
    assert_eq!(scaled(10.1).2, scaled_reference.2);
    let scaled_lower = scaled(10.2);
    let scaled_columns = 40..400;
    let scaled_reference_rows = row_ink(&scaled_reference, scaled_columns.clone(), 28..120);
    let scaled_lower_rows = row_ink(&scaled_lower, scaled_columns, 28..120);
    assert!(scaled_reference_rows.iter().any(|ink| *ink > 0));
    assert_eq!(
        scaled_lower_rows[1..],
        scaled_reference_rows[..scaled_reference_rows.len() - 1],
        "at scale 2 a tenth of a logical pixel crosses the device half-pixel"
    );

    // A rotated line is not axis-aligned and keeps the exact baseline, as Skia
    // positions a rotated run with subpixel precision on both axes.
    let rotated = |y: f64| png_rgba(&anchored_text_scene(10.0, y, 0.3), 1.0);
    assert_ne!(
        rotated(10.0).2,
        rotated(10.3).2,
        "rotated text is not snapped"
    );
}

#[test]
fn liberation_sans_substitution_names_ordinary_text_and_generated_labels_apart() {
    // `font.liberation-sans-substituted` is reported once for a Helvetica
    // (`fontFamily` 2) text element and once for a compatibility placeholder's
    // generated label; the two are distinct glossary terms and the message
    // says which one the bundled Liberation Sans shaped (#136).
    let message_for = |scene: &[u8]| {
        let scene = SceneDocument::parse(scene).expect("scene is admitted");
        let request = RenderRequest::builder(OutputFormat::Svg)
            .build()
            .expect("request is valid");
        let outcome = Renderer::new()
            .render(&scene, &request)
            .expect("scene renders");
        let messages: Vec<String> = outcome
            .report()
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.code() == DiagnosticCode::FONT_LIBERATION_SANS_SUBSTITUTED
            })
            .map(|diagnostic| diagnostic.message().to_owned())
            .collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        messages.into_iter().next().expect("one message")
    };
    let helvetica = br##"{"type":"excalidraw","version":2,"elements":[{"id":"h","type":"text","x":0,"y":0,"width":120,"height":23,"angle":0,"strokeColor":"#1e1e1e","backgroundColor":"transparent","fillStyle":"solid","strokeWidth":2,"strokeStyle":"solid","roughness":1,"opacity":100,"groupIds":[],"frameId":null,"index":"a1","roundness":null,"seed":1,"version":1,"versionNonce":2,"isDeleted":false,"boundElements":null,"updated":1,"link":null,"locked":false,"text":"Helvetica","fontSize":20,"fontFamily":2,"textAlign":"left","verticalAlign":"top","containerId":null,"originalText":"Helvetica","autoResize":true,"lineHeight":1.15}],"appState":{},"files":{}}"##;
    let placeholder = br##"{"type":"excalidraw","version":2,"elements":[{"id":"future","type":"future","x":0,"y":0,"width":200,"height":60,"updated":1}],"appState":{},"files":{}}"##;
    assert_eq!(
        message_for(helvetica),
        "Helvetica text used bundled Liberation Sans"
    );
    assert_eq!(
        message_for(placeholder),
        "generated fallback label used bundled Liberation Sans"
    );
}

/// An elbow arrow's geometry transaction charges the shape the reference
/// measures its scene extent with beside the drawn elbow path (#176): the
/// Rough.js `linearPath` over its route points, generated for that measurement
/// alone, counted in the element's geometry segments at 32 work units and a
/// 64-byte command record each, in every format alike and identically on
/// repeated renders. A two-segment `linearPath` at one stroke draws two
/// `move`/`bcurveTo` pairs per segment (the second pair the multi-stroke
/// overlay), so the route's three segments add 12 commands; the same route as
/// an ordinary arrow pays for its drawn outline alone.
#[test]
fn an_elbow_arrow_charges_its_extent_shape_as_generated_geometry() {
    let arrow = |elbowed: &str, roughness: &str| {
        format!(
            r##"{{"type":"excalidraw","version":2,"elements":[
              {{"id":"arrow","type":"arrow","x":50,"y":50,"width":300,"height":90,"angle":0.4,"roughness":{roughness},"points":[[0,0],[150,0],[150,90],[300,90]],"elbowed":{elbowed},"endArrowhead":null,"seed":103,"updated":1}}
            ],"appState":{{}},"files":{{}}}}"##
        )
    };
    let geometry_delta = |source: &str, format: OutputFormat| {
        let scene = SceneDocument::parse(source.as_bytes()).expect("scene is admitted");
        let request = RenderRequest::builder(format)
            .build()
            .expect("request is valid");
        let profile = Renderer::new().profile_identity();
        let (first_result, first_trace) = render_with_trace(profile, &scene, &request);
        let (second_result, second_trace) = render_with_trace(profile, &scene, &request);
        first_result.expect("traced render succeeds");
        second_result.expect("repeated traced render succeeds");
        assert_eq!(first_trace, second_trace, "{format:?} traces agree");
        let trace: serde_json::Value =
            serde_json::from_slice(&first_trace).expect("trace is valid JSON");
        let entry = trace["entries"]
            .as_array()
            .expect("trace entries are an array")
            .iter()
            .find(|entry| entry["event"] == "rough_linear_geometry")
            .expect("the arrow's geometry is one transaction")
            .clone();
        assert_eq!(entry["decision"], "committed");
        (
            entry["deltas"]["geometry_segments"]
                .as_u64()
                .expect("segments"),
            entry["deltas"]["total_work_units"]
                .as_u64()
                .expect("work units"),
            entry["deltas"]["working_bytes"]
                .as_u64()
                .expect("working bytes"),
        )
    };
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let (plain_segments, plain_work, plain_bytes) =
            geometry_delta(&arrow("false", "1"), format);
        let (elbow_segments, elbow_work, elbow_bytes) = geometry_delta(&arrow("true", "1"), format);
        // The drawn shapes differ (a linearPath against the elbow path), so
        // only the extent shape's share is compared, and it is the plain
        // arrow's whole linearPath: 12 commands, 384 work units, 768 bytes.
        assert_eq!(plain_segments, 12, "{format:?}");
        assert_eq!((plain_work, plain_bytes), (12 * 32, 128 + 12 * 64));
        let elbow_source = arrow("true", "1");
        let elbow = &normalized_elements(elbow_source.as_bytes())[0];
        let drawn_elbow_path =
            rough_drawing(elbow, adjusted_roughness(elbow), &mut default_budget(elbow))
                .ok()
                .unwrap_or_else(|| panic!("{} generates", elbow.source.id()))
                .operation_count() as u64;
        assert_eq!(
            elbow_segments,
            drawn_elbow_path + 12,
            "{format:?}: the elbow pays its drawn path and the extent shape's 12 commands"
        );
        assert_eq!(
            (elbow_work, elbow_bytes),
            (elbow_segments * 32, 128 + elbow_segments * 64),
            "{format:?}: the extent shape pays the drawn shape's per-command rates"
        );
        // The charge does not depend on the roughness: the extent shape has
        // the same commands at roughness 0, where it has no jitter to add.
        assert_eq!(
            geometry_delta(&arrow("true", "0"), format).0,
            elbow_segments
        );
    }
}
