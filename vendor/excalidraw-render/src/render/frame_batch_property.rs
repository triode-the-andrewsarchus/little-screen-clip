//! Batch-versus-standalone frame equivalence.
//!
//! Rendering one frame through a shared [`PreparedFrameRender`] must produce
//! the same artifact bytes, the same compatibility report, and the same
//! canonical resource trace as rendering it with no batch preparation at all.
//! That equivalence is what lets the batch path replay a journal of
//! normalization and relationship events instead of re-deriving them, and it is
//! the property any simplification of that replay has to keep.
//!
//! Unlike a fixed scene list, the corpus here is generated: scenes vary in
//! frame count, element mix, placement class, and text demand, and each scene
//! is exercised against resource limits probed at the `N-1`, `N`, and `N+1`
//! boundaries of the values it actually consumes. Near-limit scenes matter
//! because `resource.*-exceeded` diagnostics are embedded in the artifact, so a
//! change in *when* a charge lands can change output bytes even when the total
//! charge is unchanged.

use super::*;
use crate::{FrameRenderRequest, ResourceLimits};

const FIXED_SEED: u64 = 0x2545_f491_4f6c_dd1d;
/// The release-mode sample: small enough to run on every push in `--release`
/// beside the font subset qualification corpus.
/// `nightly_frame_batch_property_runs_a_deeper_sample` draws a longer sample
/// from the same seed, so these cases are its prefix.
const RELEASE_CASES: usize = 16;
const MAX_FRAMES: usize = 4;
const MAX_ELEMENTS: usize = 12;

/// Deterministic xorshift generator; the corpus must be reproducible on every
/// target, so no environment or clock input is admitted.
struct Generator(u64);

impl Generator {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }

    fn coordinate(&mut self, span: f64) -> f64 {
        f64::from(u32::try_from(self.below(1_000)).unwrap_or(0)) / 1_000.0 * span
    }
}

fn assert_generated_cases(seed: u64, cases: usize) {
    let mut generator = Generator(seed);
    for case in 0..cases {
        let source = generate_scene(&mut generator);
        let id = format!("seed {seed:016x} case {case}");
        let scene = SceneDocument::parse(source.as_bytes())
            .unwrap_or_else(|error| panic!("{id} is admissible: {error:?}\n{source}"));
        let frames = scene.frames().collect::<Vec<_>>();
        if frames.is_empty() {
            continue;
        }
        for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
            // Each frame sets its own boundaries: a ladder derived from one
            // frame says nothing about what the others consume, and it is the
            // frame sitting on its own limit whose charge timing matters.
            for frame in &frames {
                for limits in limit_ladder(format, *frame, &id) {
                    assert_batch_matches_standalone(&scene, *frame, format, limits, &id);
                }
            }
        }
    }
}

/// Renders every frame twice — once from a shared batch preparation, once cold
/// — and requires the two to be indistinguishable in artifact, report, and
/// trace, whether they succeed or fail.
fn assert_batch_matches_standalone(
    scene: &SceneDocument,
    frame: crate::FrameRef<'_>,
    format: OutputFormat,
    limits: ResourceLimits,
    id: &str,
) {
    let request = FrameRenderRequest::builder(format)
        .resource_limits(limits)
        .build()
        .unwrap_or_else(|error| panic!("{id} frame request is valid: {error:?}"));
    let prepared = PreparedFrameRender::new(
        crate::catalog::REVISION_1.frame_profile(),
        crate::catalog::REVISION_1.scene_surface(),
        scene,
        request.as_render_request(),
    );
    let Ok(prepared) = prepared else {
        // Preparation failure is a batch-only outcome with no standalone
        // counterpart; the batch API surfaces it before any frame renders.
        return;
    };

    {
        let (batch, batch_trace) = render_frame_with_trace(
            &crate::catalog::REVISION_1,
            frame,
            &request,
            Some(&prepared),
        );
        let (cold, cold_trace) =
            render_frame_with_trace(&crate::catalog::REVISION_1, frame, &request, None);
        let ordinal = frame.ordinal();
        match (batch, cold) {
            (Ok(batch), Ok(cold)) => {
                assert_eq!(
                    batch.artifact().bytes(),
                    cold.artifact().bytes(),
                    "{id} frame {ordinal} {format:?} artifact bytes diverge"
                );
                assert_eq!(
                    batch.report(),
                    cold.report(),
                    "{id} frame {ordinal} {format:?} report diverges"
                );
            }
            (Err(batch), Err(cold)) => {
                assert_eq!(
                    batch.code(),
                    cold.code(),
                    "{id} frame {ordinal} {format:?} failure code diverges"
                );
                // The record a crossing lands on is contract-visible, so the
                // first crossing must be identical and not merely the same
                // code: a charge that moves earlier or later would change it.
                assert_eq!(
                    (batch.element_id(), batch.json_pointer()),
                    (cold.element_id(), cold.json_pointer()),
                    "{id} frame {ordinal} {format:?} first crossing location diverges"
                );
                assert_eq!(
                    batch.report(),
                    cold.report(),
                    "{id} frame {ordinal} {format:?} failure report diverges"
                );
            }
            (batch, cold) => panic!(
                "{id} frame {ordinal} {format:?} outcome kind diverges: batch {:?}, cold {:?}",
                batch.map(|outcome| outcome.artifact().bytes().len()).err(),
                cold.map(|outcome| outcome.artifact().bytes().len()).err(),
            ),
        }
        assert_eq!(
            batch_trace, cold_trace,
            "{id} frame {ordinal} {format:?} canonical resource trace diverges"
        );
    }
}

/// Probes what this frame actually consumes under default limits, then returns
/// limits sitting exactly on, just under, and just over those values. A frame
/// that already fails under default limits is itself the near-limit case, so it
/// still contributes the default profile even though no boundary can be derived
/// from it.
fn limit_ladder(format: OutputFormat, frame: crate::FrameRef<'_>, id: &str) -> Vec<ResourceLimits> {
    let mut ladder = vec![ResourceLimits::default()];
    let probe = FrameRenderRequest::builder(format)
        .build()
        .unwrap_or_else(|error| panic!("{id} probe request is valid: {error:?}"));
    let (outcome, trace) =
        render_frame_with_trace(&crate::catalog::REVISION_1, frame, &probe, None);
    let Ok(outcome) = outcome else {
        return ladder;
    };
    let output_bytes = outcome.artifact().bytes().len() as u64;
    let working_bytes = peak_working_bytes(&trace);

    for boundary in [
        output_bytes.saturating_sub(1),
        output_bytes,
        output_bytes + 1,
    ] {
        if let Ok(limits) = ResourceLimits::builder()
            .max_output_bytes(boundary.max(1))
            .build()
        {
            ladder.push(limits);
        }
    }
    if let Some(working_bytes) = working_bytes {
        for boundary in [
            working_bytes.saturating_sub(1),
            working_bytes,
            working_bytes + 1,
        ] {
            if let Ok(limits) = ResourceLimits::builder()
                .max_working_bytes(boundary.max(1))
                .build()
            {
                ladder.push(limits);
            }
        }
    }
    ladder
}

/// Reads the highest committed `working_bytes` value out of a canonical trace.
fn peak_working_bytes(trace: &[u8]) -> Option<u64> {
    let trace: serde_json::Value = serde_json::from_slice(trace).ok()?;
    trace["entries"]
        .as_array()?
        .iter()
        .filter_map(|entry| entry["after"]["working_bytes"].as_u64())
        .max()
}

#[test]
fn nested_resource_recovery_replays_cached_descendants_and_frame_traces() {
    use base64::Engine;
    let url = |value: &str| {
        format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(value)
        )
    };
    let leaf = url(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>"#,
    );
    let child = url(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><svg><image width="50%" height="50%" href="{leaf}"/></svg></svg>"#
    ));
    let parent = url(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80"><image width="40px" height="40px" href="{child}"/><image x="40" width="40" height="40" href="{child}"/></svg>"#
    ));
    let outer = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="80"><image width="80" height="80" href="{parent}"/><image x="80" width="80" height="80" href="{parent}"/></svg>"#
    );
    let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[
        {"id":"frame","type":"frame","width":160,"height":80,"updated":0},
        {"id":"image","type":"image","width":160,"height":80,"fileId":"asset","updated":0}],
        "appState":{},"files":{"asset":{"id":"asset","created":0,"dataURL":url(&outer)}}});
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let limits = ResourceLimits::builder()
            .max_nested_asset_depth(2)
            .build()
            .unwrap();
        let frame = scene.frames().next().unwrap();
        let request = FrameRenderRequest::builder(format)
            .resource_limits(limits.clone())
            .build()
            .unwrap();
        let (outcome, trace) =
            render_frame_with_trace(&crate::catalog::REVISION_1, frame, &request, None);
        let outcome = outcome.unwrap();
        assert_eq!(outcome.report().diagnostics().len(), 4);
        for diagnostic in outcome.report().diagnostics() {
            assert!(matches!(
                diagnostic.details(),
                Some(crate::DiagnosticDetails::NestedResourceLimit {
                    recovery: crate::Recovery::Placeholder,
                    ..
                })
            ));
        }
        let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
        let recoveries: Vec<_> = trace["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["event"] == "nested_resource_placeholder" && e["decision"] == "committed")
            .collect();
        assert_eq!(
            recoveries.len(),
            4,
            "every occurrence pays even through two cache levels"
        );
        assert_batch_matches_standalone(
            &scene,
            frame,
            format,
            limits,
            "nested recovery cache/frame replay",
        );
    }
}

#[test]
fn rejected_frame_coordinates_replay_identical_failure_and_release_traces() {
    let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[
        {"id":"bad","type":"frame","x":21,"width":10,"height":10,"updated":0},
        {"id":"child","type":"rectangle","width":5,"height":5,"frameId":"bad","seed":1,"updated":0},
        {"id":"good","type":"frame","width":10,"height":10,"updated":0}
    ],"appState":{},"files":{}});
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        for frame in scene.frames() {
            assert_batch_matches_standalone(
                &scene,
                frame,
                format,
                ResourceLimits::builder()
                    .max_coordinate_magnitude(20.0)
                    .build()
                    .unwrap(),
                "coordinate rejected frame",
            );
        }
    }
}

#[test]
fn derived_viewport_crossings_replay_the_same_fatal_trace() {
    let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[
        {"id":"bad","type":"frame","x":-40,"y":-40,"width":80,"height":80,"angle":std::f64::consts::FRAC_PI_4,"updated":0}
    ],"appState":{},"files":{}});
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        assert_batch_matches_standalone(
            &scene,
            scene.frames().next().unwrap(),
            format,
            ResourceLimits::builder()
                .max_coordinate_magnitude(100.0)
                .build()
                .unwrap(),
            "derived viewport crossing",
        );
    }
}

#[test]
fn forbidden_resource_recovery_replays_target_local_bounds_and_traces() {
    let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[
        {"id":"zero","type":"image","width":0,"height":0,"fileId":"outer","updated":1},
        {"id":"bounded","type":"image","x":110,"width":30,"height":20,"fileId":"outer","updated":1},
        {"id":"first","type":"frame","width":80,"height":60,"updated":1},
        {"id":"second","type":"frame","x":100,"width":80,"height":60,"updated":1}
    ],"appState":{},"files":{"outer":{"id":"outer","created":1,"dataURL":"https://example.invalid/image.png"}}});
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        for frame in scene.frames() {
            assert_batch_matches_standalone(
                &scene,
                frame,
                format,
                ResourceLimits::default(),
                "forbidden resource target-local recovery",
            );
        }
    }
}

#[test]
fn forbidden_nested_recovery_replays_cached_descendant_work() {
    use base64::Engine;
    let url = |text: &str| {
        format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(text)
        )
    };
    let leaf = url(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image width="20" height="20" href="https://example.invalid/unfetched"/></svg>"#,
    );
    let parent = url(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><image width="20" height="20" href="{leaf}"/><image x="20" width="20" height="20" href="{leaf}"/></svg>"#
    ));
    let outer = url(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="20"><image width="40" height="20" href="{parent}"/><image x="40" width="40" height="20" href="{parent}"/></svg>"#
    ));
    let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[
        {"id":"frame","type":"frame","width":80,"height":20,"updated":1},
        {"id":"image","type":"image","width":80,"height":20,"fileId":"outer","updated":1}
    ],"appState":{},"files":{"outer":{"id":"outer","created":1,"dataURL":outer}}});
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        for work in [8191, 8192] {
            let limits = ResourceLimits::builder()
                .max_asset_work_units(work)
                .build()
                .unwrap();
            let request = FrameRenderRequest::builder(format)
                .resource_limits(limits.clone())
                .build()
                .unwrap();
            let frame = scene.frames().next().unwrap();
            let (outcome, trace) =
                render_frame_with_trace(&crate::catalog::REVISION_1, frame, &request, None);
            let outcome = outcome.unwrap();
            assert_eq!(outcome.report().diagnostics().len(), 4);
            for diagnostic in outcome.report().diagnostics() {
                assert_eq!(diagnostic.code().as_str(), "image.forbidden-resource");
                assert!(
                    matches!(diagnostic.details(),Some(crate::DiagnosticDetails::NestedSource {recovery,..}) if *recovery==if work==8192 {crate::Recovery::Placeholder}else{crate::Recovery::Omitted})
                );
            }
            let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
            let count = trace["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| {
                    entry["event"] == "nested_resource_placeholder"
                        && entry["decision"] == "committed"
                })
                .count();
            assert_eq!(
                count,
                if work == 8192 { 4 } else { 0 },
                "cached descendants each replay their own recovery"
            );
            assert_batch_matches_standalone(
                &scene,
                frame,
                format,
                limits,
                "forbidden nested cached recovery",
            );
        }
    }
}

#[test]
fn depth_variant_sanitization_shares_raster_validation_work() {
    use base64::Engine;
    let mut png = Vec::new();
    {
        let mut encoder = ::png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(::png::ColorType::Rgba);
        encoder.set_depth(::png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255])
            .unwrap();
    }
    let url = |svg: &str| {
        format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(svg)
        )
    };
    let shared = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image width="2" height="2" href="data:image/png;base64,{}"/></svg>"#,
        base64::engine::general_purpose::STANDARD.encode(&png)
    );
    let wrapper = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image width="2" height="2" href="{}"/></svg>"#,
        url(&shared)
    );
    for deep_first in [false, true] {
        let (a, b) = if deep_first {
            (&wrapper, &shared)
        } else {
            (&shared, &wrapper)
        };
        let outer = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><image width="2" height="2" href="{}"/><image x="2" width="2" height="2" href="{}"/></svg>"#,
            url(a),
            url(b)
        );
        let source = serde_json::json!({"type":"excalidraw","version":2,"elements":[{"id":"frame","type":"frame","width":4,"height":2,"updated":0},{"id":"image","type":"image","width":4,"height":2,"fileId":"asset","updated":0}],"appState":{},"files":{"asset":{"id":"asset","created":0,"dataURL":url(&outer)}}});
        let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
        let request = FrameRenderRequest::builder(OutputFormat::Svg)
            .resource_limits(
                ResourceLimits::builder()
                    .max_nested_asset_depth(3)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let (outcome, trace) = render_frame_with_trace(
            &crate::catalog::REVISION_1,
            scene.frames().next().unwrap(),
            &request,
            None,
        );
        assert!(outcome.unwrap().report().is_empty());
        let trace: serde_json::Value = serde_json::from_slice(&trace).unwrap();
        let charges: Vec<_> = trace["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["event"] == "nested_asset_intrinsic_work")
            .collect();
        assert_eq!(
            charges.len(),
            3,
            "one work charge each for shared SVG, wrapper SVG and PNG: {trace}"
        );
    }
}

/// The duplicate-ID effect correction happens after relationship resolution.
/// Crossings before/during/after that boundary must replay the same accumulated
/// report and ledger, even for a target that excludes the duplicate occurrence.
#[test]
fn duplicate_identity_cycle_and_resource_boundaries_replay_exact_traces() {
    let source = br#"{"type":"excalidraw","version":2,"elements":[
        {"id":"duplicate","type":"frame","width":80,"height":60,"updated":0},
        {"id":"duplicate","type":"magicframe","x":20,"width":80,"height":60,"updated":0},
        {"id":"child","type":"rectangle","x":5,"y":5,"width":30,"height":20,"seed":1,"frameId":"duplicate"},
        {"id":"cycle","type":"rectangle","x":10,"y":10,"width":10,"height":10,"seed":1,"updated":0,"frameId":"cycle"}
    ],"appState":{},"files":{}}"#;
    let scene = SceneDocument::parse(source).unwrap();
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        for frame in scene.frames().collect::<Vec<_>>().into_iter().rev() {
            let request = FrameRenderRequest::builder(format).build().unwrap();
            let (outcome, bytes) =
                render_frame_with_trace(&crate::catalog::REVISION_1, frame, &request, None);
            assert!(outcome.is_ok());
            let trace: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let mut work_boundaries = std::collections::BTreeSet::new();
            let mut seen_normalization = false;
            let mut seen_relationship = false;
            for entry in trace["entries"].as_array().unwrap() {
                let event = entry["event"].as_str().unwrap_or_default();
                if event == "normalization_repair" || event.starts_with("relationship_") {
                    seen_normalization |= event == "normalization_repair";
                    seen_relationship |= event.starts_with("relationship_");
                    work_boundaries.insert(entry["after"]["total_work_units"].as_u64().unwrap());
                }
            }
            assert!(
                seen_normalization && seen_relationship,
                "trace must exercise both boundaries"
            );
            for n in work_boundaries {
                for boundary in [n.saturating_sub(1).max(1), n, n + 1] {
                    let limits = ResourceLimits::builder()
                        .max_total_work_units(boundary)
                        .max_element_work_units(boundary)
                        .max_asset_work_units(boundary)
                        .build()
                        .unwrap();
                    assert_batch_matches_standalone(
                        &scene,
                        frame,
                        format,
                        limits,
                        "duplicate/cycle work boundary",
                    );
                }
            }
            for diagnostics in [1, 2, 3] {
                assert_batch_matches_standalone(
                    &scene,
                    frame,
                    format,
                    ResourceLimits::builder()
                        .max_diagnostics(diagnostics)
                        .build()
                        .unwrap(),
                    "duplicate/cycle diagnostic boundary",
                );
            }
            for limits in limit_ladder(format, frame, "duplicate/cycle") {
                assert_batch_matches_standalone(
                    &scene,
                    frame,
                    format,
                    limits,
                    "duplicate/cycle output and memory boundary",
                );
            }
        }
    }
}

/// Builds one scene of frames plus elements distributed over the placement
/// classes that frame selection distinguishes: inside, crossing, outside, and
/// bound to a frame by `frameId`.
fn generate_scene(generator: &mut Generator) -> String {
    let frame_count = 1 + generator.below(MAX_FRAMES as u64) as usize;
    let mut elements = String::new();
    let mut updated = 0_u32;
    for index in 0..frame_count {
        let x = 200.0 * index as f64;
        let kind = if generator.below(4) == 0 {
            "magicframe"
        } else {
            "frame"
        };
        let name = match generator.below(3) {
            0 => "null".to_owned(),
            1 => format!("\"Frame {index}\""),
            _ => "\"\"".to_owned(),
        };
        updated += 1;
        push_element(
            &mut elements,
            &format!(
                r#"{{"id":"frame-{index}","type":"{kind}","name":{name},"x":{x},"y":0,"width":160,"height":120,"updated":{updated}}}"#
            ),
        );
    }

    let element_count = generator.below(MAX_ELEMENTS as u64 + 1) as usize;
    for index in 0..element_count {
        let owner = generator.below(frame_count as u64) as usize;
        let origin = 200.0 * owner as f64;
        let (x, y) = match generator.below(4) {
            // Inside the frame box.
            0 => (origin + 10.0, 10.0),
            // Crossing the left edge.
            1 => (origin - 20.0, generator.coordinate(100.0)),
            // Entirely outside every frame.
            2 => (origin + 400.0, 400.0),
            // Randomly placed near the frame.
            _ => (
                origin + generator.coordinate(180.0),
                generator.coordinate(140.0),
            ),
        };
        let frame_id = if generator.below(2) == 0 {
            format!(r#","frameId":"frame-{owner}""#)
        } else {
            String::new()
        };
        updated += 1;
        let element = match generator.below(4) {
            0 => format!(
                r#"{{"id":"rect-{index}","type":"rectangle","x":{x},"y":{y},"width":40,"height":30,"updated":{updated}{frame_id}}}"#
            ),
            1 => format!(
                r#"{{"id":"ellipse-{index}","type":"ellipse","x":{x},"y":{y},"width":50,"height":20,"updated":{updated}{frame_id}}}"#
            ),
            2 => format!(
                r#"{{"id":"text-{index}","type":"text","x":{x},"y":{y},"width":80,"height":25,"fontSize":20,"fontFamily":{},"text":{},"updated":{updated}{frame_id}}}"#,
                1 + generator.below(6),
                text_literal(generator),
            ),
            _ => format!(
                r#"{{"id":"diamond-{index}","type":"diamond","x":{x},"y":{y},"width":36,"height":36,"updated":{updated}{frame_id}}}"#
            ),
        };
        push_element(&mut elements, &element);
    }

    format!(
        r#"{{"type":"excalidraw","version":2,"elements":[{elements}],"appState":{{}},"files":{{}}}}"#
    )
}

/// Text demand drives font subsetting, which is the costliest charge a frame
/// makes; the alternatives span Latin, CJK, and emoji chunk selection.
fn text_literal(generator: &mut Generator) -> &'static str {
    match generator.below(5) {
        0 => "\"A\"",
        1 => "\"Roadmap\"",
        2 => "\"\\u4e2d\\u6587\"",
        3 => "\"\\ud83d\\ude00\"",
        _ => "\"One two\\nthree\"",
    }
}

fn push_element(elements: &mut String, element: &str) {
    if !elements.is_empty() {
        elements.push(',');
    }
    elements.push_str(element);
}

/// Every generated case renders each frame, in each format, once cold and once
/// through a shared batch preparation across a ladder of resource limits: several
/// hundred renders that take minutes unoptimized. The push gate therefore skips
/// it; `just qualification <target>` runs it in `--release` with
/// `EXCALIDRAW_RENDER_RELEASE_QUALIFICATION=1` and no name filter, so a rename
/// cannot drop it silently.
#[test]
#[cfg_attr(
    not(release_qualification),
    ignore = "release-mode qualification set; run with EXCALIDRAW_RENDER_RELEASE_QUALIFICATION=1"
)]
fn frame_batch_rendering_matches_standalone_around_resource_limits() {
    assert_generated_cases(FIXED_SEED, RELEASE_CASES);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "scheduled deterministic frame batch expansion"]
fn nightly_frame_batch_property_runs_a_deeper_sample() {
    let cases = std::env::var("EXCALIDRAW_FRAME_BATCH_CASES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(2_048)
        .min(16_384);
    assert_generated_cases(crate::fuzz_seed::nightly_seed(FIXED_SEED), cases);
}
