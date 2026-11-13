//! Metadata operation fault contracts through the public scene/frame facade.
use crate::render::test_faults::{self, Fault};
use crate::render::MetadataStage as Stage;
use crate::resource_accounting::lower_hex;
use crate::{
    FrameRenderRequest, MetadataOptions, OutputFormat, RenderErrorCategory, RenderRequest,
    Renderer, ResourceLimits, SceneDocument,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[allow(dead_code)]
#[path = "../tests/common/pdf_inspect.rs"]
mod pdf_inspect;

const FORMATS: [OutputFormat; 3] = [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf];
const SOURCE:&[u8]=br#"{"type":"excalidraw","version":2,"source":"metadata-fault-contract","elements":[{"id":"shape","type":"future","x":5,"y":5,"width":10,"height":10,"updated":1,"opaque":{"number":1.00e+02}}],"appState":{},"files":{}}"#;
#[derive(Clone, Copy)]
struct Case {
    stage: Stage,
    code: &'static str,
    component: &'static str,
    operation: &'static str,
    retained: Parts,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Parts {
    provenance: bool,
    diagnostic_summary: bool,
    embedded_scene: bool,
}
impl Parts {
    const ALL: Self = Self {
        provenance: true,
        diagnostic_summary: true,
        embedded_scene: true,
    };
    const NONE: Self = Self {
        provenance: false,
        diagnostic_summary: false,
        embedded_scene: false,
    };
    const SUMMARY_AND_SCENE: Self = Self {
        provenance: false,
        diagnostic_summary: true,
        embedded_scene: true,
    };
    const PROVENANCE_AND_SCENE: Self = Self {
        provenance: true,
        diagnostic_summary: false,
        embedded_scene: true,
    };
    const PROVENANCE_AND_SUMMARY: Self = Self {
        provenance: true,
        diagnostic_summary: true,
        embedded_scene: false,
    };
    const SUMMARY_ONLY: Self = Self {
        provenance: false,
        diagnostic_summary: true,
        embedded_scene: false,
    };
}
const CASES: [Case; 8] = [
    Case {
        stage: Stage::ProvenanceSerialization,
        code: "metadata.provenance-serialization-failed",
        component: "provenance",
        operation: "serialization",
        retained: Parts::SUMMARY_AND_SCENE,
    },
    Case {
        stage: Stage::DiagnosticSummarySerialization,
        code: "metadata.diagnostic-summary-serialization-failed",
        component: "diagnostic_summary",
        operation: "serialization",
        retained: Parts::PROVENANCE_AND_SCENE,
    },
    Case {
        stage: Stage::SceneSerialization,
        code: "metadata.scene-serialization-failed",
        component: "embedded_scene",
        operation: "serialization",
        retained: Parts::PROVENANCE_AND_SUMMARY,
    },
    Case {
        stage: Stage::SceneCompression,
        code: "metadata.scene-compression-failed",
        component: "embedded_scene",
        operation: "compression",
        retained: Parts::PROVENANCE_AND_SUMMARY,
    },
    Case {
        stage: Stage::SceneWrapper,
        code: "metadata.scene-embedding-failed",
        component: "embedded_scene",
        operation: "embedding",
        retained: Parts::PROVENANCE_AND_SUMMARY,
    },
    Case {
        stage: Stage::SceneEmbedding,
        code: "metadata.scene-embedding-failed",
        component: "embedded_scene",
        operation: "embedding",
        retained: Parts::PROVENANCE_AND_SUMMARY,
    },
    Case {
        stage: Stage::ManifestSerialization,
        code: "metadata.manifest-serialization-failed",
        component: "manifest",
        operation: "serialization",
        retained: Parts::NONE,
    },
    Case {
        stage: Stage::ManifestEmbedding,
        code: "metadata.manifest-embedding-failed",
        component: "manifest",
        operation: "embedding",
        retained: Parts::NONE,
    },
];
fn options(parts: Parts) -> MetadataOptions {
    MetadataOptions::default()
        .with_embedded_scene(parts.embedded_scene)
        .with_provenance(parts.provenance)
        .with_diagnostic_summary(parts.diagnostic_summary)
}
fn request(format: OutputFormat, parts: Parts) -> RenderRequest {
    RenderRequest::builder(format)
        .metadata_options(options(parts))
        .build()
        .unwrap()
}
fn document(notices: bool, frames: bool) -> (SceneDocument, Vec<u8>) {
    if !notices && !frames {
        return (SceneDocument::parse(SOURCE).unwrap(), SOURCE.to_vec());
    }
    let mut source: Value = serde_json::from_slice(SOURCE).unwrap();
    if notices {
        source["elements"].as_array_mut().unwrap().push(json!({"id":"text","type":"text","x":20,"width":80,"height":25,"fontSize":20,"fontFamily":3,"text":"Code","updated":1}));
    }
    if frames {
        source["elements"][0]["frameId"] = json!("first");
        source["elements"].as_array_mut().unwrap().extend([
            json!({"id":"first","type":"frame","width":40,"height":40,"updated":1}),
            json!({"id":"second","type":"frame","x":100,"width":40,"height":40,"updated":1}),
        ]);
    }
    let bytes = serde_json::to_vec(&source).unwrap();
    (SceneDocument::parse(&bytes).unwrap(), bytes)
}
fn prefix() -> Value {
    json!({"code":"compatibility.unsupported-element-type","severity":"warning","effects":["visual","semantic"],"element_id":"shape","json_pointer":"/elements/0","details":{"kind":"compatibility","recovery":"placeholder","recipe_id":"renderer.unknown-element.v1"}})
}
fn notice_document(format: OutputFormat) -> (SceneDocument, Vec<u8>) {
    if format != OutputFormat::Png {
        return document(true, false);
    }
    // PNG does not embed font programs. Its rendered official error-image
    // recipe supplies the format's real mandatory-notice case instead.
    let mut source: Value = serde_json::from_slice(SOURCE).unwrap();
    source["elements"] = json!([{"id":"shape","type":"image","width":20,"height":20,"fileId":"missing","status":"error","updated":1}]);
    let bytes = serde_json::to_vec(&source).unwrap();
    (SceneDocument::parse(&bytes).unwrap(), bytes)
}
fn notice_prefix(format: OutputFormat) -> Value {
    if format != OutputFormat::Png {
        return prefix();
    }
    json!({"code":"image.missing-file","severity":"warning","effects":["visual"],"element_id":"shape","json_pointer":"/elements/0/fileId","details":{"kind":"asset","asset_id":"missing","claimed_media_type":null,"validated_media_type":null,"recovery":"placeholder","recipe_id":"official.image-error@sha256:f3c63c8600147961d006c0eea92ea6aff87bdc54711a2368af895a78154919e9"}})
}
fn warning(case: Case) -> Value {
    json!({"code":case.code,"severity":"warning","effects":["metadata"],"element_id":null,"json_pointer":"","details":{"kind":"metadata","component":case.component,"operation":case.operation,"recovery":"metadata_omitted"}})
}
fn report(actual: &crate::CompatibilityReport, format: OutputFormat, mut rows: Value) {
    let mut actual = serde_json::to_value(actual).unwrap();
    for row in actual["diagnostics"].as_array_mut().unwrap() {
        assert!(!row["message"].as_str().unwrap().is_empty());
        row.as_object_mut().unwrap().remove("message");
    }
    for row in rows.as_array_mut().unwrap() {
        row["format"] = json!(format.as_str());
    }
    assert_eq!(actual, json!({"schema_version":1,"diagnostics":rows}));
}
fn manifest(format: OutputFormat, bytes: &[u8]) -> Option<Value> {
    let json = match format {
        OutputFormat::Svg => {
            let text = std::str::from_utf8(bytes).unwrap();
            let doc = roxmltree::Document::parse(text).unwrap();
            doc.descendants()
                .find(|n| n.tag_name().name() == "manifest")
                .and_then(|n| n.text())
                .map(str::to_owned)
        }
        OutputFormat::Pdf => pdf_inspect::pdf_manifest(bytes),
        OutputFormat::Png => {
            let mut cursor = 8;
            let mut found = None;
            while cursor < bytes.len() {
                let n = u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().unwrap()) as usize;
                let kind = &bytes[cursor + 4..cursor + 8];
                let data = &bytes[cursor + 8..cursor + 8 + n];
                if kind == b"iTXt" && data.starts_with(b"excalidraw-render:artifact-manifest\0") {
                    assert!(found.is_none());
                    let offset = data.iter().position(|b| *b == 0).unwrap() + 5;
                    found = Some(std::str::from_utf8(&data[offset..]).unwrap().to_owned());
                }
                cursor += n + 12;
            }
            found
        }
    };
    json.map(|json| serde_json::from_str(&json).unwrap())
}
fn assert_manifest(
    outcome: &crate::RenderOutcome,
    format: OutputFormat,
    parts: Parts,
    canonical: &[u8],
    notices: bool,
    frame: Option<u32>,
    has_prefix: bool,
) {
    let actual = manifest(format, outcome.artifact().bytes());
    if parts == Parts::NONE && !notices {
        assert!(actual.is_none());
        return;
    }
    let actual = actual.unwrap();
    let mut expected = json!({"schema_version":if frame.is_some(){2}else{1},"provenance":null,"diagnostic_summary":null,"notices":[],"embedded_scene":null});
    if let Some(ordinal) = frame {
        expected["target"] = json!({"kind":"frame","ordinal":ordinal});
    }
    if parts.provenance {
        expected["provenance"] = json!({"renderer_version":env!("CARGO_PKG_VERSION"),"compatibility_revision":1,"excalidraw_release":"v0.18.1","reference_profile_digest":outcome.profile_identity().reference_profile_digest()});
    }
    if parts.diagnostic_summary {
        expected["diagnostic_summary"] = json!({"snapshot":"pre_metadata","total":u8::from(has_prefix),"by_severity":{"info":0,"warning":u8::from(has_prefix)},"by_code":if has_prefix{json!([{"code":if notices&&format==OutputFormat::Png {"image.missing-file"}else{"compatibility.unsupported-element-type"},"count":1}])}else{json!([])}});
    }
    if parts.embedded_scene {
        expected["embedded_scene"] = json!({"name":"scene.excalidraw","media_type":"application/vnd.excalidraw+json","sha256":format!("sha256:{}", lower_hex(&Sha256::digest(canonical)))});
    }
    if notices {
        assert!(!actual["notices"].as_array().unwrap().is_empty());
        expected["notices"] = actual["notices"].clone();
    }
    assert_eq!(actual, expected);
}

#[test]
fn metadata_failures_have_exact_recovery_in_every_format() {
    every_operation_preserves_unaffected_metadata_and_artifacts();
    manifest_failures_drop_prepared_scene_or_fail_required_notices();
    allocation_refusal_is_fatal_and_disabled_operations_are_silent();
    multiple_failures_preserve_priority_and_the_frozen_summary();
    frame_failures_are_target_local_and_batch_replayed();
    long_frame_names_keep_exact_manifests_after_component_failure();
    generation_limits_precede_metadata_operation_faults();
    fault_scopes_restore_state_after_panics_and_across_threads();
}

#[test]
fn every_operation_preserves_unaffected_metadata_and_artifacts() {
    for format in FORMATS {
        for case in CASES {
            for notices in [false, true] {
                if notices && case.component == "manifest" {
                    continue;
                }
                let (scene, canonical) = if notices {
                    notice_document(format)
                } else {
                    document(false, false)
                };
                let control = Renderer::new()
                    .render(&scene, &request(format, case.retained))
                    .unwrap();
                let healthy = Renderer::new()
                    .render(&scene, &request(format, Parts::ALL))
                    .unwrap();
                for _ in 0..2 {
                    let (actual, observed) = test_faults::run(&[Fault::at(case.stage)], || {
                        Renderer::new().render(&scene, &request(format, Parts::ALL))
                    });
                    let actual = actual.unwrap();
                    assert_eq!(observed.failures, [case.stage]);
                    assert!(observed.visits.contains(&case.stage));
                    report(
                        actual.report(),
                        format,
                        json!([
                            if notices {
                                notice_prefix(format)
                            } else {
                                prefix()
                            },
                            warning(case)
                        ]),
                    );
                    assert_eq!(
                        actual.artifact().bytes(),
                        control.artifact().bytes(),
                        "{} {format:?}",
                        case.code
                    );
                    assert_manifest(
                        &actual,
                        format,
                        case.retained,
                        &canonical,
                        notices,
                        None,
                        true,
                    );
                    if notices {
                        assert_eq!(
                            manifest(format, actual.artifact().bytes()).unwrap()["notices"],
                            manifest(format, healthy.artifact().bytes()).unwrap()["notices"]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn manifest_failures_drop_prepared_scene_or_fail_required_notices() {
    for format in FORMATS {
        for case in CASES
            .into_iter()
            .filter(|case| case.component == "manifest")
        {
            for notices in [false, true] {
                for late in [false, true] {
                    let (scene, canonical) = if notices {
                        notice_document(format)
                    } else {
                        document(false, false)
                    };
                    let fault = if late {
                        Fault::at(case.stage).with_scene()
                    } else {
                        Fault::at(case.stage)
                    };
                    let (result, observed) = test_faults::run(&[fault], || {
                        Renderer::new().render(&scene, &request(format, Parts::ALL))
                    });
                    assert_eq!(observed.failures, [case.stage]);
                    if late {
                        assert!(observed.visits.contains(&Stage::SceneCompression));
                        assert!(observed.visits.contains(&Stage::SceneWrapper));
                    }
                    if notices {
                        let error = result.unwrap_err();
                        assert_eq!(error.category(), RenderErrorCategory::Encoding);
                        assert_eq!(
                            error.code().as_str(),
                            format!("encode.{}-failed", format.as_str())
                        );
                        assert_eq!(error.format(), format);
                        assert_eq!(error.element_id(), None);
                        assert_eq!(error.json_pointer(), "");
                        assert!(error.details().is_none());
                        report(error.report(), format, json!([notice_prefix(format)]));
                    } else {
                        let actual = result.unwrap();
                        report(actual.report(), format, json!([prefix(), warning(case)]));
                        let control = Renderer::new()
                            .render(&scene, &request(format, Parts::NONE))
                            .unwrap();
                        assert_eq!(actual.artifact().bytes(), control.artifact().bytes());
                        assert_manifest(
                            &actual,
                            format,
                            Parts::NONE,
                            &canonical,
                            false,
                            None,
                            true,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn allocation_refusal_is_fatal_and_disabled_operations_are_silent() {
    for format in FORMATS {
        for case in CASES {
            let (scene, _) = document(false, false);
            let (result, observed) =
                test_faults::run(&[Fault::at(case.stage).allocation()], || {
                    Renderer::new().render(&scene, &request(format, Parts::ALL))
                });
            let error = result.unwrap_err();
            assert_eq!(observed.failures, [case.stage]);
            assert_eq!(error.code().as_str(), "resource.memory-unavailable");
            assert_eq!(error.category(), RenderErrorCategory::ResourceLimit);
            assert_eq!(error.element_id(), None);
            assert_eq!(error.json_pointer(), "");
            assert_eq!(
                serde_json::to_value(error.details()).unwrap(),
                json!({"kind":"memory_unavailable","subject_kind":"metadata","subject":"test_operation_refusal"})
            );
            report(error.report(), format, json!([prefix()]));
            let (result, observed) = test_faults::run(&[Fault::at(case.stage)], || {
                Renderer::new().render(&scene, &request(format, case.retained))
            });
            report(result.unwrap().report(), format, json!([prefix()]));
            assert!(observed.failures.is_empty());
            assert!(!observed.visits.contains(&case.stage));
            if case.component == "manifest" {
                let (notice_scene, _) = notice_document(format);
                for fault in [
                    Fault::at(case.stage).allocation(),
                    Fault::at(case.stage).with_scene().allocation(),
                ] {
                    let (error, observed) = test_faults::run(&[fault], || {
                        Renderer::new().render(&notice_scene, &request(format, Parts::ALL))
                    });
                    let error = error.unwrap_err();
                    assert_eq!(observed.failures, [case.stage]);
                    assert_eq!(error.code().as_str(), "resource.memory-unavailable");
                    report(error.report(), format, json!([notice_prefix(format)]));
                }
            }
        }
    }
}

#[test]
fn multiple_failures_preserve_priority_and_the_frozen_summary() {
    let (scene, canonical) = document(false, false);
    for format in FORMATS {
        let (result, observed) = test_faults::run(
            &[
                Fault::at(Stage::ProvenanceSerialization),
                Fault::at(Stage::SceneCompression),
            ],
            || Renderer::new().render(&scene, &request(format, Parts::ALL)),
        );
        let actual = result.unwrap();
        assert_eq!(
            observed.failures,
            [Stage::ProvenanceSerialization, Stage::SceneCompression]
        );
        report(
            actual.report(),
            format,
            json!([prefix(), warning(CASES[0]), warning(CASES[3])]),
        );
        assert_manifest(
            &actual,
            format,
            Parts::SUMMARY_ONLY,
            &canonical,
            false,
            None,
            true,
        );
        let rules = [
            Fault::at(Stage::ProvenanceSerialization),
            Fault::at(Stage::DiagnosticSummarySerialization),
            Fault::at(Stage::SceneSerialization),
        ];
        let (result, observed) = test_faults::run(&rules, || {
            Renderer::new().render(&scene, &request(format, Parts::ALL))
        });
        let actual = result.unwrap();
        assert_eq!(
            observed.failures,
            [
                Stage::ProvenanceSerialization,
                Stage::DiagnosticSummarySerialization,
                Stage::SceneSerialization
            ]
        );
        report(
            actual.report(),
            format,
            json!([
                prefix(),
                warning(CASES[0]),
                warning(CASES[1]),
                warning(CASES[2])
            ]),
        );
        let limited = RenderRequest::builder(format)
            .metadata_options(options(Parts::ALL))
            .resource_limits(
                ResourceLimits::builder()
                    .max_diagnostics(2)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let (error, _) = test_faults::run(&rules, || Renderer::new().render(&scene, &limited));
        let error = error.unwrap_err();
        assert_eq!(error.code().as_str(), "resource.diagnostic-count-exceeded");
        report(error.report(), format, json!([prefix(), warning(CASES[0])]));
    }
}

#[test]
fn frame_failures_are_target_local_and_batch_replayed() {
    let (scene, canonical) = document(false, true);
    for format in FORMATS {
        for case in CASES {
            let frames: Vec<_> = scene.frames().collect();
            let request = FrameRenderRequest::builder(format)
                .metadata_options(options(Parts::ALL))
                .build()
                .unwrap();
            let renderer = Renderer::new();
            let (batch, observed) = test_faults::run(&[Fault::at(case.stage).frame(1)], || {
                renderer
                    .render_frames(&scene, frames.iter().copied(), &request)
                    .unwrap()
                    .map(|entry| entry.into_result().unwrap())
                    .collect::<Vec<_>>()
            });
            assert_eq!(observed.failures, [case.stage]);
            for (i, frame) in frames.into_iter().enumerate() {
                let (direct, _) = test_faults::run(&[Fault::at(case.stage).frame(1)], || {
                    Renderer::new().render_frame(frame, &request)
                });
                let direct = direct.unwrap();
                report(
                    direct.report(),
                    format,
                    if i == 0 {
                        json!([prefix(), warning(case)])
                    } else {
                        json!([])
                    },
                );
                assert_eq!(direct.report(), batch[i].report());
                assert_eq!(direct.artifact().bytes(), batch[i].artifact().bytes());
                let parts = if i == 0 { case.retained } else { Parts::ALL };
                let clean = Renderer::new()
                    .render_frame(
                        frame,
                        &FrameRenderRequest::builder(format)
                            .metadata_options(options(parts))
                            .build()
                            .unwrap(),
                    )
                    .unwrap();
                assert_eq!(direct.artifact().bytes(), clean.artifact().bytes());
                assert_manifest(
                    &direct,
                    format,
                    parts,
                    &canonical,
                    false,
                    Some(i as u32 + 1),
                    i == 0,
                );
            }
        }
    }
}

#[test]
fn fault_scopes_restore_state_after_panics_and_across_threads() {
    let panic = std::panic::catch_unwind(|| {
        test_faults::run(&[Fault::at(Stage::SceneSerialization)], || {
            panic!("scope unwind probe")
        })
    });
    assert!(panic.is_err());
    let (scene, _) = document(false, false);
    let request = request(OutputFormat::Png, Parts::ALL);
    report(
        Renderer::new().render(&scene, &request).unwrap().report(),
        OutputFormat::Png,
        json!([prefix()]),
    );
    test_faults::run(&[Fault::at(Stage::SceneSerialization)], || {
        std::thread::scope(|scope| {
            let outcome = scope
                .spawn(|| Renderer::new().render(&scene, &request))
                .join()
                .unwrap()
                .unwrap();
            report(outcome.report(), OutputFormat::Png, json!([prefix()]));
        });
        let outcome = Renderer::new().render(&scene, &request).unwrap();
        report(
            outcome.report(),
            OutputFormat::Png,
            json!([prefix(), warning(CASES[2])]),
        );
    });
}

#[test]
fn long_frame_names_keep_exact_manifests_after_component_failure() {
    let (_, source) = document(false, true);
    let mut source: Value = serde_json::from_slice(&source).unwrap();
    let name = "Frame <&> \"Ω\"\n".repeat(128);
    source["elements"][1]["name"] = json!(name);
    let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
    for format in FORMATS {
        let frame = scene.frames().next().unwrap();
        let full = FrameRenderRequest::builder(format)
            .metadata_options(options(Parts::ALL))
            .build()
            .unwrap();
        let (actual, observed) = test_faults::run(&[Fault::at(Stage::SceneCompression)], || {
            Renderer::new().render_frame(frame, &full)
        });
        let actual = actual.unwrap();
        assert_eq!(observed.failures, [Stage::SceneCompression]);
        report(
            actual.report(),
            format,
            json!([prefix(), warning(CASES[3])]),
        );
        let clean = Renderer::new()
            .render_frame(
                frame,
                &FrameRenderRequest::builder(format)
                    .metadata_options(options(Parts::PROVENANCE_AND_SUMMARY))
                    .build()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(actual.artifact().bytes(), clean.artifact().bytes());
        let manifest = manifest(format, actual.artifact().bytes()).unwrap();
        assert_eq!(
            manifest["target"],
            json!({"kind":"frame","ordinal":1,"name":name})
        );
        assert_eq!(manifest["diagnostic_summary"]["total"], json!(1));
        assert_eq!(manifest["embedded_scene"], Value::Null);
    }
}

#[test]
fn generation_limits_precede_metadata_operation_faults() {
    let mut source: Value = serde_json::from_slice(SOURCE).unwrap();
    source["elements"][0]["opaque"] = json!("a".repeat(65536));
    let bytes = serde_json::to_vec(&source).unwrap();
    let scene = SceneDocument::parse(&bytes).unwrap();
    // Required drawing and the two small metadata fields fit. Generating the
    // full original-scene wrapper alone costs more than this remaining budget.
    let configured = 2 * bytes.len() as u64;
    for format in FORMATS {
        let request = RenderRequest::builder(format)
            .metadata_options(options(Parts::ALL))
            .resource_limits(
                ResourceLimits::builder()
                    .max_total_work_units(configured)
                    .max_element_work_units(configured)
                    .max_asset_work_units(configured)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let (outcome, observed) = test_faults::run(&[Fault::at(Stage::SceneSerialization)], || {
            Renderer::new().render(&scene, &request)
        });
        let outcome = outcome.unwrap();
        assert!(observed.failures.is_empty());
        assert!(!observed.visits.contains(&Stage::SceneSerialization));
        let diagnostics = outcome.report().diagnostics();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            diagnostics[1].code().as_str(),
            "resource.optional-metadata-omitted"
        );
        assert_eq!(diagnostics[1].element_id(), None);
        assert_eq!(diagnostics[1].json_pointer(), "");
        let details = serde_json::to_value(diagnostics[1].details()).unwrap();
        assert_eq!(details["field"], "max_total_work_units");
        assert_eq!(details["configured"], configured);
        assert_eq!(details["recovery"], "metadata_omitted");
        assert!(details["measurement"]["value"].as_u64().unwrap() > configured);
        let control = Renderer::new()
            .render(
                &scene,
                &self::request(format, Parts::PROVENANCE_AND_SUMMARY),
            )
            .unwrap();
        assert_eq!(outcome.artifact().bytes(), control.artifact().bytes());
        assert_manifest(
            &outcome,
            format,
            Parts::PROVENANCE_AND_SUMMARY,
            &bytes,
            false,
            None,
            true,
        );
    }
}
