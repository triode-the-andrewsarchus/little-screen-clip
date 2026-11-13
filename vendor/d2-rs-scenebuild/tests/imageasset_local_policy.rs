// SPDX-License-Identifier: MPL-2.0
// ImageLocalFiles::rooted is Unix-only, like the descriptor-relative opens it needs.
#![cfg(unix)]
use d2_rs_scenebuild::{ImageAssetLimits, ImageLocalFiles, RasterImageResolver};
use std::path::Path;
use std::sync::Arc;

#[test]
fn omitted_base_uses_policy_root_and_explicit_base_takes_precedence() {
    let root = std::env::temp_dir().join(format!(
        "d2-imageasset-root-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let policy = Arc::new(ImageLocalFiles::rooted(&root).unwrap());
    // A missing file's diagnostic identifies which directory was resolved,
    // without depending on decoding or any generated image fixture.
    let implicit = RasterImageResolver::new(Path::new(""), limits(), policy.clone()).unwrap();
    let error = implicit
        .resolve_with_context("missing.png", None)
        .unwrap_err();
    assert!(error.unavailable);
    assert!(
        error
            .message
            .contains(&root.join("missing.png").to_string_lossy().to_string())
    );
    assert!(error.message.contains("open local file"));
    let explicit = RasterImageResolver::new(Path::new("."), limits(), policy.clone()).unwrap();
    let error = explicit
        .resolve_with_context("missing.png", None)
        .unwrap_err();
    assert!(!error.unavailable);
    assert!(error.message.contains("outside configured root"));
    policy.close();
    std::fs::remove_dir(root).unwrap();
}

fn limits() -> ImageAssetLimits {
    ImageAssetLimits {
        max_fetched_bytes: 1 << 20,
        max_encoded_bytes: 1 << 20,
        max_decompressed_bytes: 1 << 20,
        max_svg_bytes: 1 << 20,
        max_decoded_width: 1000,
        max_decoded_height: 1000,
        max_decoded_pixels: 1_000_000,
        max_assets: 10,
        max_cumulative_encoded_bytes: 1 << 24,
        max_cumulative_decoded_bytes: 1 << 24,
    }
}
