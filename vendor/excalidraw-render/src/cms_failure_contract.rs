//! Dependency failure seams exercise the same parse/construct/execute error
//! mapping and real owned-LUT allocation wrapper used by the public facade.
use crate::codec_fixtures as fixtures;
use crate::render::{with_cms_failure, CmsSite};
use crate::{ErrorCode, OutputFormat, RenderRequest, Renderer, SceneDocument};
use base64::Engine as _;
use serde_json::json;

#[test]
pub(crate) fn cms_out_of_memory_is_fatal_at_every_operation_boundary() {
    sampled_curve_allocations_are_fallible_for_each_channel();
    let profile = fixtures::linear_rgb_icc_profile();
    let png = fixtures::png_with_chunks_before_idat(
        &fixtures::solid_png(1, 1, [128, 128, 128, 255]),
        &[(
            *b"iCCP",
            [
                b"authored\0\0".as_slice(),
                &miniz_oxide::deflate::compress_to_vec_zlib(&profile, 6),
            ]
            .concat(),
        )],
    );
    let jpeg =
        fixtures::jpeg_with_icc_profile(&fixtures::jpeg_rgb(1, 1, &[[128, 128, 128]]), &profile);
    let webp = fixtures::webp_with_metadata(1, 1, &[[128, 128, 128, 255]], Some(profile), None);
    for (mime, bytes) in [
        ("image/png", png),
        ("image/jpeg", jpeg),
        ("image/webp", webp),
    ] {
        let source = json!({"type":"excalidraw","version":2,"elements":[{"id":"image","type":"image","width":1,"height":1,"fileId":"asset","status":"saved","scale":[1,1],"updated":1}],"appState":{},"files":{"asset":{"id":"asset","mimeType":mime,"dataURL":format!("data:{mime};base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes)),"created":1}}});
        let scene = SceneDocument::parse(&serde_json::to_vec(&source).unwrap()).unwrap();
        for site in [
            CmsSite::Parse,
            CmsSite::Construct,
            CmsSite::Execute,
            CmsSite::Allocation,
        ] {
            for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
                let request = RenderRequest::builder(format).padding(0.).build().unwrap();
                let healthy = Renderer::new().render(&scene, &request).unwrap();
                let actual = with_cms_failure(site, || Renderer::new().render(&scene, &request));
                if format == OutputFormat::Svg {
                    let actual = actual.unwrap();
                    assert_eq!(actual.artifact().bytes(), healthy.artifact().bytes());
                    assert_eq!(actual.report(), healthy.report());
                } else {
                    assert!(
                        actual.is_err(),
                        "CMS {site:?} allocation refusal must be fatal in {format:?}"
                    );
                    let error = actual.unwrap_err();
                    assert_eq!(error.format(), format);
                    assert_eq!(error.category(), crate::RenderErrorCategory::ResourceLimit);
                    assert_eq!(error.element_id(), None);
                    assert_eq!(error.json_pointer(), "");
                    assert_eq!(
                        serde_json::to_value(error.descriptor().details()).unwrap(),
                        json!({"kind":"memory_unavailable","subject_kind":"asset_resolution","subject":"raster_asset"})
                    );
                    assert_eq!(
                        error.descriptor().code(),
                        ErrorCode::RESOURCE_MEMORY_UNAVAILABLE
                    );
                    assert_eq!(
                        serde_json::to_value(error.report()).unwrap(),
                        json!({"schema_version":1,"diagnostics":[]})
                    );
                    let again = with_cms_failure(site, || Renderer::new().render(&scene, &request))
                        .unwrap_err();
                    assert_eq!(error.descriptor(), again.descriptor());
                    assert_eq!(error.report(), again.report());
                }
                let recovered = Renderer::new().render(&scene, &request).unwrap();
                assert_eq!(healthy.artifact().bytes(), recovered.artifact().bytes());
            }
        }
    }
    let mut evidence: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../tests/oracle/png-color/stage4-reference.json"
    ))
    .unwrap();
    evidence["cases"].as_array_mut().unwrap().extend(
        crate::png_stage4_review::gray_lut_equivalence()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .cloned(),
    );
    for row in evidence["cases"].as_array().unwrap().iter().filter(|r| {
        r["expected_outcome"] == "accepted"
            && (r["group"].as_str().unwrap().starts_with("icc-lut") || r["group"] == "icc-gray-lut")
            && r["signal"] != "lut-only-a2b2"
    }) {
        let scene = SceneDocument::parse(&serde_json::to_vec(&row["scene"]).unwrap()).unwrap();
        for site in [CmsSite::Allocation, CmsSite::ProgramAllocation] {
            for format in [OutputFormat::Png, OutputFormat::Pdf] {
                let request = RenderRequest::builder(format).build().unwrap();
                let result = with_cms_failure(site, || Renderer::new().render(&scene, &request))
                    .unwrap_err();
                assert_eq!(
                    result.descriptor().code(),
                    ErrorCode::RESOURCE_MEMORY_UNAVAILABLE,
                    "{}",
                    row["id"]
                );
            }
        }
    }
}

fn sampled_curve_allocations_are_fallible_for_each_channel() {
    let evidence = crate::png_icc_safety::sampled_evidence();
    for row in evidence["cases"].as_array().unwrap() {
        let scene = SceneDocument::parse(&serde_json::to_vec(&row["scene"]).unwrap()).unwrap();
        let channels = if row["layout"]["color_type"] == 4 {
            1
        } else {
            3
        };
        for skip in 0..channels {
            for format in [OutputFormat::Png, OutputFormat::Pdf] {
                let request = RenderRequest::builder(format).padding(0.).build().unwrap();
                let healthy = Renderer::new().render(&scene, &request).unwrap();
                assert!(healthy.report().is_empty());
                let error =
                    crate::render::with_cms_failure_after(CmsSite::Allocation, skip, || {
                        Renderer::new().render(&scene, &request)
                    })
                    .unwrap_err();
                assert_eq!(
                    error.descriptor().code(),
                    ErrorCode::RESOURCE_MEMORY_UNAVAILABLE,
                    "{} channel {skip}",
                    row["id"]
                );
                assert_eq!(
                    serde_json::to_value(error.report()).unwrap(),
                    json!({"schema_version":1,"diagnostics":[]})
                );
            }
        }
    }
}
