//! Canonical resource traces are held stable as a cross-target equality
//! property, not as pinned digests (ADR-0002). These tests assert the relative
//! equalities — cold, warm, request-cache, and frame-batch paths agree — and
//! spot-check absolute envelope deltas; the cross-target digest itself is
//! compared by `xtask conformance aggregate-svg-font-subset-targets`.

use super::*;
use crate::{Renderer, ResourceLimits};

pub(crate) fn assert_font_subset_resource_trace_matrix() {
    assert_scene_trace_replay();
    assert_frame_batch_trace_replay();
    assert_failure_trace_replay();
}

#[test]
fn null_image_identity_releases_normalized_storage_before_encoding() {
    for field in ["", ",\"fileId\":null", ",\"fileId\":false"] {
        let source = format!(
            r#"{{"type":"excalidraw","version":2,"elements":[{{"id":"image","type":"image","width":20,"height":20,"updated":0{field}}}],"appState":{{}},"files":{{}}}}"#
        );
        let scene = SceneDocument::parse(source.as_bytes()).unwrap();
        for format in [OutputFormat::Svg, OutputFormat::Pdf] {
            let request = RenderRequest::builder(format).build().unwrap();
            let (result, trace) =
                render_with_trace(crate::catalog::REVISION_1.profile(), &scene, &request);
            result.unwrap();
            let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
            let entries = trace["entries"].as_array().unwrap();
            let normalized = entries
                .iter()
                .find(|e| e["event"] == "normalized_image")
                .expect("normalized image charge");
            let bytes = normalized["deltas"]["working_bytes"].as_u64().unwrap();
            let release = entries
                .iter()
                .position(|e| {
                    e["event"] == "resolved_image_fallback"
                        && e["decision"] == "released"
                        && e["deltas"]["working_bytes"].as_u64() == Some(bytes)
                })
                .expect("normalized image record must be released");
            let workspace = entries
                .iter()
                .position(|e| {
                    e["event"]
                        == if format == OutputFormat::Svg {
                            "svg_serializer_workspace"
                        } else {
                            "pdf_stream_workspace"
                        }
                })
                .unwrap();
            assert!(release < workspace);
        }
    }
}

fn assert_scene_trace_replay() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","width":80,"height":30,"fontSize":20,"fontFamily":5,"text":"A","updated":1}],"appState":{},"files":{}}"#,
    )
    .expect("font subset trace scene is valid");
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("SVG request is valid");
    let profile = Renderer::new().profile_identity();

    let cache = new_font_subset_cache();
    let (cold_result, cold_trace) =
        render_with_trace_and_font_subset_cache(profile, &scene, &request, &cache);
    let cold = cold_result.expect("cold font subset render succeeds");
    assert_eq!(font_subset_cache_stats(&cache), (0, 1));

    let (warm_result, warm_trace) = render_with_trace(profile, &scene, &request);
    let warm = warm_result.expect("warm font subset render succeeds");

    let (cached_result, cached_trace) =
        render_with_trace_and_font_subset_cache(profile, &scene, &request, &cache);
    let cached = cached_result.expect("request-cache font subset render succeeds");
    assert_eq!(font_subset_cache_stats(&cache), (1, 1));

    assert_eq!(cold.artifact().bytes(), warm.artifact().bytes());
    assert_eq!(cold.artifact().bytes(), cached.artifact().bytes());
    assert_eq!(cold.report(), warm.report());
    assert_eq!(cold.report(), cached.report());
    assert_eq!(cold_trace, warm_trace);
    assert_eq!(cold_trace, cached_trace);
    let trace_text = std::str::from_utf8(&cold_trace).expect("trace is UTF-8 JSON");
    assert!(!trace_text.contains("cache"));

    let trace: serde_json::Value =
        serde_json::from_slice(&cold_trace).expect("trace is valid JSON");
    let entries = trace["entries"].as_array().expect("entries are an array");
    let envelope = entries
        .iter()
        .find(|entry| entry["event"] == "svg_font_dependency_envelope")
        .expect("font dependency envelope is traced");
    assert_eq!(envelope["deltas"]["working_bytes"], 13_942_016);
    assert_eq!(envelope["deltas"]["total_work_units"], 8_397_412);
}

fn assert_frame_batch_trace_replay() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"first-frame","type":"frame","x":0,"y":0,"width":100,"height":60,"updated":1},{"id":"first-text","type":"text","x":10,"y":10,"width":40,"height":30,"fontSize":20,"fontFamily":5,"text":"A","frameId":"first-frame","updated":2},{"id":"second-frame","type":"frame","x":120,"y":0,"width":100,"height":60,"updated":3},{"id":"second-text","type":"text","x":130,"y":10,"width":40,"height":30,"fontSize":20,"fontFamily":5,"text":"A","frameId":"second-frame","updated":4}],"appState":{},"files":{}}"#,
    )
    .expect("frame-batch font subset scene is valid");
    let frames = scene.frames().collect::<Vec<_>>();
    let [first_frame, second_frame] = frames.as_slice() else {
        panic!("font subset scene has two frames");
    };
    let request = crate::FrameRenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("frame SVG request is valid");
    let prepared = PreparedFrameRender::new(
        crate::catalog::REVISION_1.frame_profile(),
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        request.as_render_request(),
    )
    .unwrap_or_else(|_| panic!("frame batch preparation succeeds"));

    let (first_result, _) = render_frame_with_trace(
        &crate::catalog::REVISION_1,
        *first_frame,
        &request,
        Some(&prepared),
    );
    first_result.expect("first frame renders and populates the batch cache");
    assert_eq!(prepared_font_subset_cache_stats(&prepared), (0, 1));

    let (batch_result, batch_trace) = render_frame_with_trace(
        &crate::catalog::REVISION_1,
        *second_frame,
        &request,
        Some(&prepared),
    );
    let batch = batch_result.expect("second frame renders from the batch cache");
    assert_eq!(prepared_font_subset_cache_stats(&prepared), (1, 1));

    let (cold_result, cold_trace) =
        render_frame_with_trace(&crate::catalog::REVISION_1, *second_frame, &request, None);
    let cold = cold_result.expect("second frame renders without batch preparation");
    assert_eq!(batch.artifact().bytes(), cold.artifact().bytes());
    assert_eq!(batch.report(), cold.report());
    assert_eq!(batch_trace, cold_trace);
    assert!(!std::str::from_utf8(&batch_trace)
        .expect("frame trace is UTF-8 JSON")
        .contains("cache"));

    let failure_request = crate::FrameRenderRequest::builder(OutputFormat::Svg)
        .resource_limits(
            ResourceLimits::builder()
                .max_output_bytes(1)
                .build()
                .expect("frame output crossing limit is valid"),
        )
        .build()
        .expect("output-limited frame request is valid");
    let failure_prepared = PreparedFrameRender::new(
        crate::catalog::REVISION_1.frame_profile(),
        crate::catalog::REVISION_1.scene_surface(),
        &scene,
        failure_request.as_render_request(),
    )
    .unwrap_or_else(|_| panic!("output-limited frame batch preparation succeeds"));
    render_frame_with_trace(
        &crate::catalog::REVISION_1,
        *first_frame,
        &failure_request,
        Some(&failure_prepared),
    )
    .0
    .expect_err("first frame output crossing is fatal");
    assert_eq!(prepared_font_subset_cache_stats(&failure_prepared), (0, 1));
    let (batch_failure, batch_failure_trace) = render_frame_with_trace(
        &crate::catalog::REVISION_1,
        *second_frame,
        &failure_request,
        Some(&failure_prepared),
    );
    assert_eq!(prepared_font_subset_cache_stats(&failure_prepared), (1, 1));
    let (cold_failure, cold_failure_trace) = render_frame_with_trace(
        &crate::catalog::REVISION_1,
        *second_frame,
        &failure_request,
        None,
    );
    let batch_failure = batch_failure.expect_err("cached frame output crossing is fatal");
    let cold_failure = cold_failure.expect_err("cold frame output crossing is fatal");
    assert_eq!(batch_failure.code(), cold_failure.code());
    assert_eq!(batch_failure.report(), cold_failure.report());
    assert_eq!(batch_failure_trace, cold_failure_trace);
}

fn assert_failure_trace_replay() {
    let scene = SceneDocument::parse(
        br#"{"type":"excalidraw","version":2,"elements":[{"id":"text","type":"text","width":80,"height":30,"fontSize":20,"fontFamily":5,"text":"A","updated":1}],"appState":{},"files":{}}"#,
    )
    .expect("font subset failure scene is valid");
    let profile = Renderer::new().profile_identity();
    let default_request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("default SVG request is valid");
    let cache = new_font_subset_cache();
    render_with_trace_and_font_subset_cache(profile, &scene, &default_request, &cache)
        .0
        .expect("cache prewarming succeeds");

    let (_, default_trace) = render_with_trace(profile, &scene, &default_request);
    let default_trace: serde_json::Value =
        serde_json::from_slice(&default_trace).expect("default trace is valid JSON");
    let envelope_attempt = default_trace["entries"]
        .as_array()
        .expect("default entries are an array")
        .iter()
        .find(|entry| entry["event"] == "svg_font_dependency_envelope")
        .and_then(|entry| entry["attempted"]["working_bytes"].as_u64())
        .expect("dependency envelope has attempted working bytes");
    let limited_request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(
            ResourceLimits::builder()
                .max_working_bytes(envelope_attempt - 1)
                .build()
                .expect("envelope crossing limit is valid"),
        )
        .build()
        .expect("limited SVG request is valid");
    let (cold_limited, cold_limited_trace) = render_with_trace(profile, &scene, &limited_request);
    let (cached_limited, cached_limited_trace) =
        render_with_trace_and_font_subset_cache(profile, &scene, &limited_request, &cache);
    let cold_limited = cold_limited.expect_err("cold envelope crossing is fatal");
    let cached_limited = cached_limited.expect_err("cached envelope crossing is fatal");
    assert_eq!(
        cold_limited.code(),
        ErrorCode::RESOURCE_WORKING_BYTES_EXCEEDED
    );
    assert_eq!(cold_limited.code(), cached_limited.code());
    assert_eq!(cold_limited.report(), cached_limited.report());
    assert_eq!(cold_limited_trace, cached_limited_trace);
    assert_eq!(font_subset_cache_stats(&cache), (0, 1));

    let output_request = RenderRequest::builder(OutputFormat::Svg)
        .resource_limits(
            ResourceLimits::builder()
                .max_output_bytes(1)
                .build()
                .expect("output crossing limit is valid"),
        )
        .build()
        .expect("output-limited SVG request is valid");
    let (cold_output, cold_output_trace) = render_with_trace(profile, &scene, &output_request);
    let (cached_output, cached_output_trace) =
        render_with_trace_and_font_subset_cache(profile, &scene, &output_request, &cache);
    let cold_output = cold_output.expect_err("cold output crossing is fatal");
    let cached_output = cached_output.expect_err("cached output crossing is fatal");
    assert_eq!(
        cold_output.code(),
        ErrorCode::RESOURCE_OUTPUT_BYTES_EXCEEDED
    );
    assert_eq!(cold_output.code(), cached_output.code());
    assert_eq!(cold_output.report(), cached_output.report());
    assert_eq!(cold_output_trace, cached_output_trace);
    assert_eq!(font_subset_cache_stats(&cache), (1, 1));
}

#[test]
fn font_subset_resource_trace_matrix() {
    assert_font_subset_resource_trace_matrix();
}
