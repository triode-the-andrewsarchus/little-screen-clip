// SPDX-License-Identifier: MPL-2.0
//! Replay original upstream bundled-fallback Build calls; compare full scenes.
use d2_rs_scene::{Asset, Document};
use d2_rs_scenebuild::{
    BundledFallbackLimits, BundledFallbackResolver, FontFallbackOptions, Options, build,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn encode(document: &Document) -> Value {
    let mut value = serde_json::to_value(document).unwrap();
    for (id, asset) in &document.assets {
        if let Asset::FontAsset {
            mime_type,
            data,
            face_index,
            ..
        } = asset
        {
            value["Assets"][id] = json!({"FontAsset": {"MIMEType": mime_type, "FaceIndex": face_index, "DataSHA256": format!("{:x}", Sha256::digest(data))}});
        }
    }
    value
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let request: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let fonts = &request["fonts"];
    let number = |name: &str| fonts[name].as_i64().unwrap_or_default();
    let limits = &request["resolver_limits"];
    let options = Options {
        fonts: Some(FontFallbackOptions {
            resolver: true,
            resolver_impl: Some(std::sync::Arc::new(BundledFallbackResolver::new(
                None,
                BundledFallbackLimits {
                    max_requested_runes: limits["MaxRequestedRunes"].as_i64().unwrap(),
                    max_bundled_bytes: limits["MaxBundledBytes"].as_i64().unwrap(),
                    max_resolved_bytes: limits["MaxResolvedBytes"].as_i64().unwrap(),
                },
            )?)),
            max_assets: number("max_assets"),
            max_bytes: number("max_bytes"),
            max_runes_per_text: number("max_runes_per_text"),
            max_total_runes: number("max_total_runes"),
            max_coverage_checks: number("max_coverage_checks"),
            max_font_faces_per_text: number("max_font_faces_per_text"),
            max_shaping_runs: number("max_shaping_runs"),
            max_shaped_glyphs: number("max_shaped_glyphs"),
        }),
        ..Default::default()
    };
    match build(
        &serde_json::from_value(request["diagram"].clone())?,
        &options,
    ) {
        Ok(document) => std::fs::write(&args[2], serde_json::to_vec(&encode(&document))?)?,
        Err(error) => std::fs::write(&args[2], error.to_string())?,
    }
    Ok(())
}
