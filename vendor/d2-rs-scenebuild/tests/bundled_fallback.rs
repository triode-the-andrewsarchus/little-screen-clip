// SPDX-License-Identifier: MPL-2.0
//! Original master Build contracts from noto_color_emoji_fallback_native_test.go.
//! Fixture: recipe `scenebuild-fallback` (tools/fixtures/scenebuild-oracle.toml),
//! complete scenes with font assets as hashes, regenerated into oracle-cache.
use d2_rs_scenebuild::{
    BundledFallbackLimits, BundledFallbackResolver, FallbackRequest, FallbackResolver,
};
use std::sync::Arc;

fn resolver(max_requested_runes: i64, max_bundled_bytes: i64) -> BundledFallbackResolver {
    BundledFallbackResolver::new(
        None,
        BundledFallbackLimits {
            max_requested_runes,
            max_bundled_bytes,
            max_resolved_bytes: max_bundled_bytes,
        },
    )
    .unwrap()
}

// Replays the regenerated `scenebuild-fallback` capture (docs/FIXTURES.md).
#[cfg(feature = "oracle-fixtures")]
mod capture {
    use d2_rs_scene::{Asset, Document};
    use d2_rs_scenebuild::{
        BundledFallbackLimits, BundledFallbackResolver, FallbackFont, FallbackRequest,
        FallbackResolver, FontFallbackOptions, Options, build,
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Debug)]
    struct CountingResolver {
        inner: BundledFallbackResolver,
        calls: AtomicUsize,
    }
    impl FallbackResolver for CountingResolver {
        fn resolve_fallbacks(
            &self,
            request: &FallbackRequest,
        ) -> Result<Vec<FallbackFont>, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.inner.resolve_fallbacks(request)
        }
    }

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

    // Go's JSON emits integral float fields as integer tokens. Preserve every
    // value and type, while permitting the same numeric spelling as the gate.
    fn same_json(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_json(x, y))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.get(key).is_some_and(|other| same_json(value, other)))
            }
            _ => left == right,
        }
    }

    #[test]
    fn original_bundled_build_scenes_and_error() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/bundled-fallback-master.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let request = &case["input"];
            let counting = Arc::new(CountingResolver {
                inner: BundledFallbackResolver::new(
                    None,
                    BundledFallbackLimits {
                        max_requested_runes: request["resolver_limits"]["MaxRequestedRunes"]
                            .as_i64()
                            .unwrap(),
                        max_bundled_bytes: request["resolver_limits"]["MaxBundledBytes"]
                            .as_i64()
                            .unwrap(),
                        max_resolved_bytes: request["resolver_limits"]["MaxResolvedBytes"]
                            .as_i64()
                            .unwrap(),
                    },
                )
                .unwrap(),
                calls: AtomicUsize::new(0),
            });
            let fonts = &request["fonts"];
            let number = |name: &str| fonts[name].as_i64().unwrap_or_default();
            let options = Options {
                fonts: Some(FontFallbackOptions {
                    resolver_impl: Some(counting.clone()),
                    max_assets: number("max_assets"),
                    max_bytes: number("max_bytes"),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let result = build(
                &serde_json::from_value(request["diagram"].clone()).unwrap(),
                &options,
            );
            if case["error"] == true {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    case["expected"].as_str().unwrap()
                );
            } else {
                assert!(
                    same_json(&encode(&result.unwrap()), &case["expected"]),
                    "{}",
                    request["test"]
                );
            }
            // The upstream variation-selector and style-bucket tests require
            // one resolver call, rather than resolving each node or glyph.
            assert_eq!(counting.calls.load(Ordering::Relaxed), 1);
        }
    }
}

#[test]
fn resolver_cumulative_requests_and_retained_bytes_are_bounded() {
    let resolver = resolver(2, 4_991_984);
    let request = FallbackRequest {
        runes: vec!['✅'],
        family: "HandDrawn".into(),
        style: "regular".into(),
        weight: 400,
    };
    let first = resolver.resolve_fallbacks(&request).unwrap();
    let second = resolver.resolve_fallbacks(&request).unwrap();
    assert!(Arc::ptr_eq(&first[0].data, &second[0].data));
    assert_eq!(
        resolver.resolve_fallbacks(&request).unwrap_err(),
        "d2fonts: requested fallback rune count exceeds cumulative resolver limit 2"
    );
    let limited = self::resolver(2, 4_991_983);
    assert_eq!(
        limited.resolve_fallbacks(&request).unwrap_err(),
        "d2fonts: bundled fallback font bytes exceed limit 4991983 while retaining Noto Color Emoji"
    );
}

#[test]
fn resolver_limits_metadata_and_noncharacters_follow_source() {
    assert_eq!(
        BundledFallbackResolver::new(
            None,
            BundledFallbackLimits {
                max_requested_runes: 0,
                max_bundled_bytes: 0,
                max_resolved_bytes: 1
            }
        )
        .unwrap_err(),
        "d2fonts: every bundled fallback limit must be positive"
    );
    let resolver = resolver(5, 1);
    let mut request = FallbackRequest {
        runes: vec!['\u{fdd0}', '\u{10ffff}', 'あ'],
        family: "SourceSansPro".into(),
        style: "regular".into(),
        weight: 400,
    };
    assert!(resolver.resolve_fallbacks(&request).unwrap().is_empty());
    request.family = "x".repeat(1025);
    assert_eq!(
        resolver.resolve_fallbacks(&request).unwrap_err(),
        "d2fonts: fallback request font metadata exceeds 1024 bytes"
    );
}
