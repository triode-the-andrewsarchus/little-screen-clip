//! Full facade accounting over the profile/transparency/container classes.
//! Trace balances locate a boundary; its ingredient weights, decisions, reports
//! and recovery artifacts are independently asserted, never snapshot-refreshed.
use crate::render::stage4_render_trace;
use crate::{
    ErrorCode, FrameOptions, OutputFormat, RenderRequest, Renderer, ResourceLimits, SceneDocument,
};
use base64::Engine as _;
use serde_json::{json, Value};
fn parse(v: &Value) -> SceneDocument {
    SceneDocument::parse(&serde_json::to_vec(v).unwrap()).unwrap()
}
fn request(format: OutputFormat, limits: ResourceLimits) -> RenderRequest {
    RenderRequest::builder(format)
        .padding(0.)
        .include_background(false)
        .frame_options(
            FrameOptions::default()
                .with_show_name(false)
                .with_outline(false),
        )
        .resource_limits(limits)
        .build()
        .unwrap()
}
fn report(
    actual: &crate::CompatibilityReport,
    format: OutputFormat,
    frames: u64,
    rejected: Option<(u64, u64)>,
) {
    let mut actual = serde_json::to_value(actual).unwrap();
    for row in actual["diagnostics"].as_array_mut().unwrap() {
        assert!(!row["message"].as_str().unwrap().is_empty());
        row.as_object_mut().unwrap().remove("message");
    }
    let rows: Vec<_> = if let Some((configured, attempted)) = rejected {
        (0..2).map(|i|json!({"code":"resource.asset-work-exceeded","severity":"warning","format":format.as_str(),"effects":["visual"],"element_id":if i==0 {"image"}else{"repeat"},"json_pointer":"/files/asset/dataURL","details":{"kind":"resource_limit","field":"max_asset_work_units","configured":configured,"value_kind":"work_units","measurement":{"kind":"attempted","value":attempted},"recovery":if format==OutputFormat::Png {"placeholder"}else{"omitted"}}})).collect()
    } else if frames > 1 && format != OutputFormat::Svg {
        (0..2).map(|i|json!({"code":"image.animation-flattened","severity":"info","format":format.as_str(),"effects":["visual"],"element_id":if i==0 {"image"}else{"repeat"},"json_pointer":format!("/elements/{i}/fileId"),"details":{"kind":"asset","asset_id":"asset","claimed_media_type":null,"validated_media_type":"image/png","recovery":"flattened","recipe_id":null}})).collect()
    } else {
        vec![]
    };
    assert_eq!(actual, json!({"schema_version":1,"diagnostics":rows}));
}
#[test]
pub(crate) fn profile_workspace_masks_sharing_and_recovery_have_exact_boundaries() {
    profile_limits(None);
}
pub(crate) fn profile_limits(selected_mask: Option<usize>) {
    let bytes = include_bytes!("../../../tests/oracle/png-color/stage4-oracle-reference.json");
    let mut evidence: Value = serde_json::from_slice(bytes).unwrap();
    let mut gray = crate::png_stage4_review::gray_lut_equivalence();
    for row in gray["cases"].as_array_mut().unwrap() {
        row["animation_frames"] = json!(1);
    }
    evidence["cases"]
        .as_array_mut()
        .unwrap()
        .extend(gray["cases"].as_array().unwrap().iter().cloned());
    let rows: Vec<_> = evidence["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| {
            r["group"] == "icc-gray-lut"
                || r["id"].as_str().unwrap().ends_with("-o6")
                    && r["id"].as_str().unwrap().contains("-false-")
        })
        .collect();
    assert_eq!(rows.len(), 504);
    for row in rows {
        let (w, h) = (
            row["width"].as_u64().unwrap(),
            row["height"].as_u64().unwrap(),
        );
        let pixels = w * h;
        let payload = base64::engine::general_purpose::STANDARD
            .decode(row["png"].as_str().unwrap())
            .unwrap();
        let b = payload.len() as u64;
        let frames = row["animation_frames"].as_u64().unwrap();
        let extra = if row["id"].as_str().unwrap().contains("-subframe-") {
            8 * pixels
        } else if frames > 1 {
            4 * pixels
        } else {
            0
        };
        for mask in 0..3 {
            if selected_mask.is_some_and(|selected| selected != mask) {
                continue;
            }
            let mut source = row["scene"].clone();
            let mut repeated = source["elements"][0].clone();
            repeated["id"] = json!("repeat");
            source["elements"].as_array_mut().unwrap().push(repeated);
            if mask == 1 {
                for e in source["elements"].as_array_mut().unwrap() {
                    e["roundness"] = json!({"type":3});
                }
            }
            if mask == 2 {
                for e in source["elements"].as_array_mut().unwrap() {
                    e["frameId"] = json!("clip");
                }
                source["elements"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"id":"clip","type":"frame","width":w,"height":h,"updated":1}));
            }
            let scene = parse(&source);
            for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
                let base_request = request(format, ResourceLimits::default());
                let (base, trace) = stage4_render_trace(&scene, &base_request);
                let base = base.unwrap();
                report(base.report(), format, frames, None);
                let trace: Value = serde_json::from_slice(&trace).unwrap();
                let entries = trace["entries"].as_array().unwrap();
                let events = |name: &str| {
                    entries
                        .iter()
                        .filter(|e| e["event"] == name && e["decision"] == "committed")
                        .collect::<Vec<_>>()
                };
                if format == OutputFormat::Svg {
                    assert!(events("png_color_normalize").is_empty());
                    assert!(events("decoded_png_rgba").is_empty());
                    let work = b + extra;
                    for configured in [work - 1, work, work + 1] {
                        let req = request(
                            format,
                            ResourceLimits::builder()
                                .max_asset_work_units(configured)
                                .max_decoded_pixels_per_asset(1)
                                .max_total_decoded_pixels(1)
                                .build()
                                .unwrap(),
                        );
                        let actual = Renderer::new().render(&scene, &req).unwrap();
                        if configured < work {
                            report(actual.report(), format, frames, Some((configured, work)));
                            let mut missing = source.clone();
                            missing["files"] = json!({});
                            let control = Renderer::new()
                                .render(&parse(&missing), &base_request)
                                .unwrap();
                            assert_eq!(actual.artifact().bytes(), control.artifact().bytes());
                        } else {
                            assert_eq!(actual.artifact().bytes(), base.artifact().bytes());
                            assert_eq!(actual.report(), base.report());
                        }
                    }
                    continue;
                }
                let color = events("png_color_normalize");
                let decode = events("decoded_png_rgba");
                assert_eq!(color.len(), 1);
                assert_eq!(decode.len(), 1);
                let workspace = 1_048_576 + 32 * w.max(h);
                assert_eq!(color[0]["deltas"]["working_bytes"], workspace);
                assert_eq!(color[0]["deltas"]["total_work_units"], b);
                assert_eq!(
                    decode[0]["deltas"]["working_bytes"],
                    workspace + 256 + 4 * pixels
                );
                assert_eq!(decode[0]["deltas"]["total_work_units"], 3 * b + 4 * pixels);
                // Global peak bounds and the earlier source-profile workspace
                // are separate gates: PNG encoding has a larger later envelope.
                let peak = entries
                    .iter()
                    .filter_map(|e| e["after"]["working_bytes"].as_u64())
                    .max()
                    .unwrap();
                for configured in [peak - 1, peak, peak + 1] {
                    let req = request(
                        format,
                        ResourceLimits::builder()
                            .max_working_bytes(configured)
                            .build()
                            .unwrap(),
                    );
                    let (actual, limited) = stage4_render_trace(&scene, &req);
                    if configured < peak {
                        let error = actual.unwrap_err();
                        assert_eq!(
                            error.descriptor().code(),
                            ErrorCode::RESOURCE_WORKING_BYTES_EXCEEDED
                        );
                        let limited: Value = serde_json::from_slice(&limited).unwrap();
                        let rejected = limited["entries"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|e| e["decision"] == "rejected_fatal")
                            .unwrap();
                        assert_eq!(rejected["attempted"]["working_bytes"], peak);
                        assert_eq!(
                            serde_json::to_value(error.descriptor().details()).unwrap(),
                            json!({"kind":"resource_limit","field":"max_working_bytes","configured":configured,"value_kind":"bytes","measurement":{"kind":"attempted","value":peak},"recovery":null})
                        );
                        let after_content = rejected["phase"] == "encoding_and_outcome_assembly";
                        report(
                            error.report(),
                            format,
                            if after_content { frames } else { 0 },
                            None,
                        );
                        let repeated = Renderer::new().render(&scene, &req).unwrap_err();
                        assert_eq!(error.descriptor(), repeated.descriptor());
                        assert_eq!(error.report(), repeated.report());
                    } else {
                        let actual = actual.unwrap();
                        assert_eq!(actual.artifact().bytes(), base.artifact().bytes());
                        assert_eq!(actual.report(), base.report());
                    }
                }
                let decode_peak = decode[0]["after"]["working_bytes"].as_u64().unwrap();
                for configured in [decode_peak - 1, decode_peak, decode_peak + 1] {
                    let req = request(
                        format,
                        ResourceLimits::builder()
                            .max_working_bytes(configured)
                            .build()
                            .unwrap(),
                    );
                    let (result, limited) = stage4_render_trace(&scene, &req);
                    let limited: Value = serde_json::from_slice(&limited).unwrap();
                    let event = limited["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|e| e["event"] == "decoded_png_rgba")
                        .unwrap();
                    assert_eq!(
                        event["decision"],
                        if configured < decode_peak {
                            "rejected_fatal"
                        } else {
                            "committed"
                        }
                    );
                    if let Err(error) = result {
                        assert_eq!(
                            error.descriptor().code(),
                            ErrorCode::RESOURCE_WORKING_BYTES_EXCEEDED
                        );
                        let rejected = limited["entries"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|e| e["decision"] == "rejected_fatal")
                            .unwrap();
                        report(
                            error.report(),
                            format,
                            if rejected["phase"] == "encoding_and_outcome_assembly" {
                                frames
                            } else {
                                0
                            },
                            None,
                        );
                    }
                }
                // Local intrinsic validation/decode crossings recover both
                // occurrences, with masks and partial alpha still participating.
                let work = 5 * b + 4 * pixels + extra;
                for configured in [b + extra - 1, b + extra, b + extra + 1] {
                    let req = request(
                        format,
                        ResourceLimits::builder()
                            .max_asset_work_units(configured)
                            .build()
                            .unwrap(),
                    );
                    let actual = Renderer::new().render(&scene, &req).unwrap();
                    report(
                        actual.report(),
                        format,
                        frames,
                        Some((
                            configured,
                            if configured < b + extra {
                                b + extra
                            } else {
                                work
                            },
                        )),
                    );
                    let mut missing = source.clone();
                    missing["files"] = json!({});
                    let control = Renderer::new()
                        .render(&parse(&missing), &base_request)
                        .unwrap();
                    assert_eq!(actual.artifact().bytes(), control.artifact().bytes());
                }
                for configured in [work - 1, work, work + 1] {
                    let req = request(
                        format,
                        ResourceLimits::builder()
                            .max_asset_work_units(configured)
                            .build()
                            .unwrap(),
                    );
                    let actual = Renderer::new().render(&scene, &req).unwrap();
                    if configured < work {
                        report(actual.report(), format, frames, Some((configured, work)));
                        let mut missing = source.clone();
                        missing["files"] = json!({});
                        let control = Renderer::new()
                            .render(&parse(&missing), &base_request)
                            .unwrap();
                        assert_eq!(actual.artifact().bytes(), control.artifact().bytes());
                    } else {
                        assert_eq!(actual.artifact().bytes(), base.artifact().bytes());
                        assert_eq!(actual.report(), base.report());
                    }
                }
                if format == OutputFormat::Png && mask == 2 {
                    // Main surface + one shared frame mask + one image-owned
                    // mask per occurrence. Sharing source pixels shares neither
                    // occurrence's masking/projection work.
                    for configured in [4 * pixels - 1, 4 * pixels, 4 * pixels + 1] {
                        let req = request(
                            format,
                            ResourceLimits::builder()
                                .max_raster_surface_pixels(pixels)
                                .max_total_rasterized_pixels(configured)
                                .build()
                                .unwrap(),
                        );
                        let actual = Renderer::new().render(&scene, &req);
                        if configured < 4 * pixels {
                            let error = actual.unwrap_err();
                            assert_eq!(
                                error.descriptor().code(),
                                ErrorCode::RESOURCE_TOTAL_RASTERIZED_PIXELS_EXCEEDED
                            );
                            // Required raster-mask planning fails before any
                            // occurrence is actually rendered/flattened.
                            report(error.report(), format, 0, None);
                            assert_eq!(
                                serde_json::to_value(error.descriptor().details()).unwrap(),
                                json!({"kind":"resource_limit","field":"max_total_rasterized_pixels","configured":configured,"value_kind":"pixels","measurement":{"kind":"attempted","value":4*pixels},"recovery":null})
                            );
                        } else {
                            assert_eq!(actual.unwrap().artifact().bytes(), base.artifact().bytes());
                        }
                    }
                }
                let limits = ResourceLimits::builder()
                    .max_decoded_pixels_per_asset(pixels)
                    .max_total_decoded_pixels(pixels)
                    .build()
                    .unwrap();
                let actual = Renderer::new()
                    .render(&scene, &request(format, limits))
                    .unwrap();
                assert_eq!(actual.artifact().bytes(), base.artifact().bytes());
            }
        }
        if selected_mask.is_none() || selected_mask == Some(0) {
            alias_boundaries(row, &payload, pixels, frames);
        }
    }
    if selected_mask.is_none() {
        crate::cms_failure_contract::cms_out_of_memory_is_fatal_at_every_operation_boundary();
    }
}

fn alias_boundaries(row: &Value, payload: &[u8], pixels: u64, frames: u64) {
    for distinct in [false, true] {
        let second = if distinct {
            crate::codec_fixtures::png_with_chunk_before_iend(payload, *b"vpAg", b"identity")
        } else {
            payload.to_vec()
        };
        let mut source = row["scene"].clone();
        source["files"]["second"] = source["files"]["asset"].clone();
        source["files"]["second"]["id"] = json!("second");
        source["files"]["second"]["dataURL"] = json!(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&second)
        ));
        let mut repeated = source["elements"][0].clone();
        repeated["id"] = json!("repeat");
        repeated["fileId"] = json!("second");
        source["elements"].as_array_mut().unwrap().push(repeated);
        let scene = parse(&source);
        let report_rows = |format: OutputFormat, count: usize| {
            if frames > 1 {
                (0..count).map(|i|json!({"code":"image.animation-flattened","severity":"info","format":format.as_str(),"effects":["visual"],"element_id":if i==0 {"image"}else{"repeat"},"json_pointer":format!("/elements/{i}/fileId"),"details":{"kind":"asset","asset_id":if i==0 {"asset"}else{"second"},"claimed_media_type":null,"validated_media_type":"image/png","recovery":"flattened","recipe_id":null}})).collect::<Vec<_>>()
            } else {
                vec![]
            }
        };
        let exact_report = |actual: &crate::CompatibilityReport, expected: Vec<Value>| {
            let mut v = serde_json::to_value(actual).unwrap();
            for d in v["diagnostics"].as_array_mut().unwrap() {
                assert!(!d["message"].as_str().unwrap().is_empty());
                d.as_object_mut().unwrap().remove("message");
            }
            assert_eq!(v, json!({"schema_version":1,"diagnostics":expected}));
        };
        for format in [OutputFormat::Png, OutputFormat::Pdf] {
            let healthy = Renderer::new()
                .render(&scene, &request(format, ResourceLimits::default()))
                .unwrap();
            exact_report(healthy.report(), report_rows(format, 2));
            let n = if distinct { 2 * pixels } else { pixels };
            for configured in [n - 1, n, n + 1] {
                if configured < pixels {
                    assert!(ResourceLimits::builder()
                        .max_decoded_pixels_per_asset(pixels)
                        .max_total_decoded_pixels(configured)
                        .build()
                        .is_err());
                    continue;
                }
                let req = request(
                    format,
                    ResourceLimits::builder()
                        .max_decoded_pixels_per_asset(pixels)
                        .max_total_decoded_pixels(configured)
                        .build()
                        .unwrap(),
                );
                let actual = Renderer::new().render(&scene, &req);
                if configured < n {
                    let error = actual.unwrap_err();
                    assert_eq!(
                        error.descriptor().code(),
                        ErrorCode::RESOURCE_TOTAL_DECODED_PIXELS_EXCEEDED
                    );
                    assert_eq!(
                        serde_json::to_value(error.descriptor().details()).unwrap(),
                        json!({"kind":"resource_limit","field":"max_total_decoded_pixels","configured":configured,"value_kind":"pixels","measurement":{"kind":"attempted","value":n},"recovery":null})
                    );
                    exact_report(
                        error.report(),
                        report_rows(format, if format == OutputFormat::Pdf { 1 } else { 0 }),
                    );
                } else {
                    let actual = actual.unwrap();
                    assert_eq!(actual.artifact().bytes(), healthy.artifact().bytes());
                    assert_eq!(actual.report(), healthy.report());
                }
            }
            let n = (payload.len() + second.len()) as u64;
            for configured in [n - 1, n, n + 1] {
                let req = request(
                    format,
                    ResourceLimits::builder()
                        .max_asset_bytes(second.len().max(payload.len()) as u64)
                        .max_total_asset_bytes(configured)
                        .build()
                        .unwrap(),
                );
                let actual = Renderer::new().render(&scene, &req);
                if configured < n {
                    let error = actual.unwrap_err();
                    assert_eq!(
                        error.descriptor().code(),
                        ErrorCode::RESOURCE_TOTAL_ASSET_BYTES_EXCEEDED
                    );
                    assert_eq!(
                        serde_json::to_value(error.descriptor().details()).unwrap(),
                        json!({"kind":"resource_limit","field":"max_total_asset_bytes","configured":configured,"value_kind":"bytes","measurement":{"kind":"attempted","value":n},"recovery":null})
                    );
                    exact_report(error.report(), vec![]);
                } else {
                    let actual = actual.unwrap();
                    assert_eq!(actual.artifact().bytes(), healthy.artifact().bytes());
                    assert_eq!(actual.report(), healthy.report());
                }
            }
        }
    }
}
