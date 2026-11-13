// SPDX-License-Identifier: MPL-2.0
// Every test here starts from the regenerated imageasset capture (docs/FIXTURES.md).
#![cfg(feature = "oracle-fixtures")]
use base64::Engine as _;
use d2_rs_core::CancellationToken;
use d2_rs_scenebuild::{ImageAssetLimits, RasterImageResolver, UnrestrictedImageFiles};
use serde_json::Value;
use std::sync::Arc;

fn source() -> (ImageAssetLimits, Vec<u8>) {
    let fixture: Value = serde_json::from_str(
        d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    (
        serde_json::from_value(fixture["request"]["limits"].clone()).unwrap(),
        base64::engine::general_purpose::STANDARD
            .decode(fixture["expected"]["resource"]["data"].as_str().unwrap())
            .unwrap(),
    )
}

#[test]
fn malformed_image_preserves_declared_mime_or_reports_sniffed_type() {
    // TestIsometricPagedFailurePreservesOutput's second-board icon.
    let (limits, _) = source();
    let resolver = RasterImageResolver::new(
        std::path::Path::new("."),
        limits,
        Arc::new(UnrestrictedImageFiles::new()),
    )
    .unwrap();
    for (uri, expected) in [
        (
            "data:image/png;base64,YmFk",
            "imageasset: probe \"data:image/png\": unsupported or malformed image with MIME type \"image/png\"",
        ),
        (
            "data:IMAGE/X-PNG;base64,YmFk",
            "imageasset: probe \"data:IMAGE/X-PNG\": unsupported or malformed image with MIME type \"image/png\"",
        ),
        (
            "data:;base64,YmFk",
            "imageasset: probe \"data URI\": unsupported or malformed image (sniffed \"text/plain\")",
        ),
    ] {
        assert_eq!(
            resolver
                .resolve_with_context(uri, None)
                .unwrap_err()
                .message,
            expected
        );
        assert_eq!(resolver.usage().assets, 0);
    }
}

#[test]
fn local_absence_retries_canonical_hits_and_owned_bytes() {
    let (limits, png) = source();
    let root = std::env::temp_dir().join(format!("d2rs-raster-resolver-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let resolver =
        RasterImageResolver::new(&root, limits, Arc::new(UnrestrictedImageFiles::new())).unwrap();
    let missing = resolver
        .resolve_with_context("photo.png", None)
        .unwrap_err();
    assert!(missing.unavailable);
    assert_eq!(resolver.usage().assets, 0);
    std::fs::write(root.join("photo.png"), &png).unwrap();
    let result = resolver.resolve_with_context("./photo.png", None).unwrap();
    assert_eq!(result.resource.data.as_ref(), png);
    let mut uri = result.data_uri(None).unwrap();
    uri[0] = b'X';
    std::fs::remove_file(root.join("photo.png")).unwrap();
    let cached = resolver
        .resolve_with_context(root.join("photo.png").to_str().unwrap(), None)
        .unwrap();
    assert_eq!(cached.resource.data.as_ref(), png);
    assert!(
        cached
            .data_uri(None)
            .unwrap()
            .starts_with(b"data:image/png;base64,")
    );
    assert_eq!(resolver.usage().assets, 1);
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn cancellation_precedes_io_and_base64_growth_is_bounded() {
    let (mut limits, png) = source();
    limits.max_decompressed_bytes = 1;
    let resolver = RasterImageResolver::new(
        std::path::Path::new("."),
        limits,
        Arc::new(UnrestrictedImageFiles::new()),
    )
    .unwrap();
    let token = CancellationToken::new();
    token.expire();
    assert_eq!(
        resolver
            .resolve_with_context("data:image/png;base64,secret", Some(&token))
            .unwrap_err()
            .message,
        "imageasset: resolve \"data:image/png\": context deadline exceeded"
    );
    let source = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    assert_eq!(
        resolver
            .resolve_with_context(&source, None)
            .unwrap_err()
            .message,
        "imageasset: load \"data:image/png\": decode data URI base64: decompressed bytes 2 exceeds limit 1"
    );
    assert_eq!(resolver.usage().assets, 0);
    let mut invalid = limits;
    invalid.max_assets = 0;
    assert_eq!(
        RasterImageResolver::new(
            std::path::Path::new("."),
            invalid,
            Arc::new(UnrestrictedImageFiles::new())
        )
        .unwrap_err()
        .message,
        "imageasset: every limit must be positive"
    );
}

#[test]
fn svg_limits_are_inclusive_and_failed_admission_does_not_charge_the_session() {
    let (mut limits, _) = source();
    limits.max_svg_bytes = 6;
    let resolver = RasterImageResolver::new(
        std::path::Path::new("."),
        limits,
        Arc::new(UnrestrictedImageFiles::new()),
    )
    .unwrap();
    let too_large = "data:image/svg+xml,%3Csvg%20/%3E";
    assert_eq!(
        resolver
            .resolve_with_context(too_large, None)
            .unwrap_err()
            .message,
        "imageasset: probe \"data:image/svg+xml\": SVG bytes 7 exceeds limit 6"
    );
    assert_eq!(resolver.usage().assets, 0);
    let exact = resolver
        .resolve_with_context("data:image/svg+xml,%3Csvg/%3E", None)
        .unwrap();
    assert_eq!(exact.resource.kind, d2_rs_scenebuild::ImageKind::Svg);
    assert_eq!(exact.resource.data.as_ref(), b"<svg/>");
    assert_eq!(exact.resource.decoded_bytes, 6);
    assert_eq!(
        (
            resolver.usage().assets,
            resolver.usage().encoded,
            resolver.usage().decoded
        ),
        (1, 6, 6)
    );
    assert!(
        !resolver
            .resolve_with_context(too_large, None)
            .unwrap_err()
            .unavailable
    );
    assert_eq!(
        (
            resolver.usage().assets,
            resolver.usage().encoded,
            resolver.usage().decoded
        ),
        (1, 6, 6)
    );
}

#[test]
fn svg_depth_is_bounded_before_import_and_compressed_limits_charge_final_xml() {
    let (limits, _) = source();
    let resolver = RasterImageResolver::new(
        std::path::Path::new("."),
        limits,
        Arc::new(UnrestrictedImageFiles::new()),
    )
    .unwrap();
    let xml = format!("<svg>{}{}</svg>", "<g>".repeat(256), "</g>".repeat(256));
    let uri = format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(xml)
    );
    assert_eq!(
        resolver
            .resolve_with_context(&uri, None)
            .unwrap_err()
            .message,
        "imageasset: probe \"data:image/svg+xml\": SVG XML depth 257 exceeds limit 256"
    );
    assert_eq!(resolver.usage().assets, 0);
    let row: Value = serde_json::from_str(
        d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-svg-codecs-master.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let expected = &row["expected"]["resource"];
    let mut limits = limits;
    limits.max_encoded_bytes = expected["decoded_bytes"].as_i64().unwrap();
    let bounded = RasterImageResolver::new(
        std::path::Path::new("."),
        limits,
        Arc::new(UnrestrictedImageFiles::new()),
    )
    .unwrap();
    let result = bounded
        .resolve_with_context(row["request"]["source"].as_str().unwrap(), None)
        .unwrap();
    assert_eq!(
        result.resource.data.as_ref(),
        base64::engine::general_purpose::STANDARD
            .decode(expected["data"].as_str().unwrap())
            .unwrap()
    );
    assert_eq!(
        bounded.usage().encoded,
        expected["decoded_bytes"].as_i64().unwrap()
    );
}
