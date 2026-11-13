// SPDX-License-Identifier: MPL-2.0
//! Replay the original upstream image/icon Build requests without network I/O.
use base64::Engine as _;
use d2_rs_scene::{Asset, Document};
use d2_rs_scenebuild::{
    AssetOptions, ImageKind, ImageResolveError, ImageResolver, ImageResource, Options,
    SvgImportBudget, SvgImportLimits, build,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

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

#[derive(Debug)]
struct CapturedResolver {
    resources: Value,
    calls: Mutex<Vec<String>>,
}

impl ImageResolver for CapturedResolver {
    fn resolve(&self, source: &str) -> Result<Option<ImageResource>, ImageResolveError> {
        self.calls.lock().unwrap().push(source.to_owned());
        let Some(value) = self.resources.get(source) else {
            panic!("unrecorded resolver call: {source}");
        };
        if let Some(message) = value["error"].as_str() {
            return Err(ImageResolveError {
                message: message.to_owned(),
                unavailable: value["unavailable"].as_bool().unwrap(),
            });
        }
        if value.as_object().is_some_and(|v| v.is_empty()) {
            return Ok(None);
        }
        Ok(Some(ImageResource {
            kind: match value["kind"].as_u64().unwrap() {
                1 => ImageKind::Raster,
                2 => ImageKind::Svg,
                n => ImageKind::Other(n as u8),
            },
            mime_type: value["mime_type"].as_str().unwrap().to_owned(),
            data: base64::engine::general_purpose::STANDARD
                .decode(value["data"].as_str().unwrap())
                .unwrap()
                .into(),
            pixel_width: value["pixel_width"].as_i64().unwrap(),
            pixel_height: value["pixel_height"].as_i64().unwrap(),
            decoded_bytes: value["decoded_bytes"].as_i64().unwrap(),
        }))
    }
}

fn options(request: &Value, resolver: &Arc<CapturedResolver>, legacy: bool) -> Options {
    Options {
        pad: request["pad"].as_i64(),
        assets: request["configured"]
            .as_bool()
            .unwrap()
            .then(|| AssetOptions {
                resolver: true,
                resolver_impl: (!legacy).then(|| resolver.clone() as Arc<dyn ImageResolver>),
                svg_import_limits: {
                    let limits = &request["assets"]["SVGImportLimits"];
                    let n = |key: &str| limits[key].as_i64().unwrap();
                    SvgImportLimits {
                        max_bytes: n("MaxBytes"),
                        max_depth: n("MaxDepth"),
                        max_elements: n("MaxElements"),
                        max_attributes: n("MaxAttributes"),
                        max_attribute_bytes: n("MaxAttributeBytes"),
                        max_path_commands: n("MaxPathCommands"),
                        max_transform_functions: n("MaxTransformFunctions"),
                        max_use_depth: n("MaxUseDepth"),
                        max_resources: n("MaxResources"),
                    }
                },
                svg_import_budget: {
                    let budget = &request["assets"]["SVGImportBudget"];
                    let n = |key: &str| budget[key].as_i64().unwrap();
                    SvgImportBudget {
                        max_source_bytes: n("MaxSourceBytes"),
                        max_elements: n("MaxElements"),
                        max_attributes: n("MaxAttributes"),
                        max_attribute_bytes: n("MaxAttributeBytes"),
                        max_path_commands: n("MaxPathCommands"),
                        max_transform_functions: n("MaxTransformFunctions"),
                        max_declared_resources: n("MaxDeclaredResources"),
                        max_expanded_use_instances: n("MaxExpandedUseInstances"),
                    }
                },
            }),
        ..Default::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let request: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let resolver = Arc::new(CapturedResolver {
        resources: request["resources"].clone(),
        calls: Mutex::default(),
    });
    let options = options(
        &request,
        &resolver,
        args.iter().any(|arg| arg == "--legacy"),
    );
    match build(
        &serde_json::from_value(request["diagram"].clone())?,
        &options,
    ) {
        Ok(document) => std::fs::write(&args[2], serde_json::to_vec(&encode(&document))?)?,
        Err(error) => std::fs::write(&args[2], error.to_string())?,
    }
    // The source resolver handoff is reached only once per distinct source,
    // after all layout admission. Also reject I/O that Go never performed.
    let calls = resolver.calls.lock().unwrap();
    let unique: std::collections::BTreeSet<_> = calls.iter().collect();
    assert_eq!(calls.len(), unique.len(), "repeated source resolution");
    // Require every recorded resolver call, including SVG failures.
    if !args.iter().any(|arg| arg == "--legacy") {
        assert_eq!(
            calls.len(),
            request["resources"].as_object().unwrap().len(),
            "missing resolver I/O"
        );
    }
    Ok(())
}

#[cfg(all(test, feature = "oracle-fixtures"))]
mod tests {
    use super::*;

    fn same_json(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_json(x, y))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|other| same_json(v, other)))
            }
            _ => left == right,
        }
    }

    #[test]
    fn original_image_asset_build_contracts() {
        check_fixture(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/image-assets-master.json"),
            28,
        );
    }
    #[test]
    fn original_latex_build_contracts() {
        check_fixture(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/latex-master.json"),
            8,
        );
    }
    fn check_fixture(input: &str, count: usize) {
        let fixture: Value = serde_json::from_str(input).unwrap();
        let mut checked = 0;
        for case in fixture["cases"].as_array().unwrap() {
            let request = &case["input"];
            let resolver = Arc::new(CapturedResolver {
                resources: request["resources"].clone(),
                calls: Mutex::default(),
            });
            let options = options(request, &resolver, false);
            let result = build(
                &serde_json::from_value(request["diagram"].clone()).unwrap(),
                &options,
            );
            if case["error"] == true {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    case["expected"].as_str().unwrap(),
                    "{}",
                    request["test"]
                );
            } else {
                assert!(
                    same_json(&encode(&result.unwrap()), &case["expected"]),
                    "{}",
                    request["test"]
                );
            }
            let calls = resolver.calls.lock().unwrap();
            let unique: std::collections::BTreeSet<_> = calls.iter().collect();
            assert_eq!(calls.len(), unique.len(), "{}", request["test"]);
            assert_eq!(
                calls.len(),
                request["resources"].as_object().unwrap().len(),
                "{}",
                request["test"]
            );
            checked += 1;
        }
        assert_eq!(checked, count);
    }

    #[derive(Debug)]
    struct StaticResolver(Option<ImageResource>);
    impl ImageResolver for StaticResolver {
        fn resolve(&self, _: &str) -> Result<Option<ImageResource>, ImageResolveError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn nil_kind_deduplication_and_document_ownership() {
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/image-assets-master.json"
        ))
        .unwrap();
        let request = &fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| {
                v["input"]["test"] == "TestBuildShapeImageAndOrdinaryIconUseResolvedOwnedAssets"
            })
            .unwrap()["input"];
        let original = CapturedResolver {
            resources: request["resources"].clone(),
            calls: Mutex::default(),
        };
        let source = request["resources"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap();
        let resource = original.resolve(source).unwrap().unwrap();
        let mut diagram = request["diagram"].clone();
        // A second locator returning the same probed resource must share one
        // content ID even when a caller's resolver does not cache sources.
        diagram["shapes"][1]["icon"] =
            json!({"Scheme":"https", "Host":"example.com", "Path":"/alias.png"});
        let diagram = serde_json::from_value(diagram).unwrap();
        let make_options = |r| Options {
            assets: Some(AssetOptions {
                resolver_impl: Some(Arc::new(StaticResolver(r))),
                ..Default::default()
            }),
            ..Default::default()
        };
        let options = make_options(Some(resource.clone()));
        let mut first = build(&diagram, &options).unwrap();
        let second = build(&diagram, &options).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.assets.len(), 1);
        let Asset::RasterAsset { data, .. } = first.assets.values_mut().next().unwrap() else {
            panic!("raster")
        };
        data[0] ^= 255;
        assert_ne!(first, second);
        assert_eq!(build(&diagram, &options).unwrap(), second);
        assert_eq!(
            build(&diagram, &make_options(None))
                .unwrap_err()
                .to_string(),
            "scene: shape \"photo\" image asset resolver returned nil"
        );
        let mut invalid = resource;
        invalid.kind = ImageKind::Other(99);
        assert_eq!(
            build(&diagram, &make_options(Some(invalid)))
                .unwrap_err()
                .to_string(),
            "scene: shape \"photo\" image asset has unsupported resolved kind Kind(99)"
        );
    }

    #[test]
    fn native_data_uri_resolution_builds_the_complete_original_go_scene() {
        use d2_rs_scenebuild::{RasterImageResolver, UnrestrictedImageFiles};
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/image-assets-master.json"
        ))
        .unwrap();
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| {
                case["input"]["test"] == "TestBuildShapeImageAndOrdinaryIconUseResolvedOwnedAssets"
            })
            .unwrap();
        let request = &case["input"];
        let resource_case: Value = serde_json::from_str(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let resolver = Arc::new(
            RasterImageResolver::new(
                std::path::Path::new("."),
                serde_json::from_value(resource_case["request"]["limits"].clone()).unwrap(),
                Arc::new(UnrestrictedImageFiles::new()),
            )
            .unwrap(),
        );
        let document = build(
            &serde_json::from_value(request["diagram"].clone()).unwrap(),
            &Options {
                pad: request["pad"].as_i64(),
                assets: Some(AssetOptions {
                    resolver_impl: Some(resolver.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(same_json(&encode(&document), &case["expected"]));
        assert_eq!(resolver.usage().assets, 1);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn native_http_scene_remains_owned_and_rebuilds_after_origin_shutdown() {
        use d2_rs_scenebuild::{
            NativeImageHttpClient, RasterImageResolver, UnrestrictedImageFiles,
        };
        use std::io::{Read, Write};
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/image-assets-master.json"
        ))
        .unwrap();
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| {
                case["input"]["test"]
                    == "TestResolvedRasterDocumentRendersAfterRemoteOriginShutdown"
            })
            .unwrap();
        let request = &case["input"];
        let (_, resource) = request["resources"]
            .as_object()
            .unwrap()
            .iter()
            .next()
            .unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(resource["data"].as_str().unwrap())
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut diagram = request["diagram"].clone();
        diagram["shapes"][0]["icon"]["Host"] = json!(listener.local_addr().unwrap().to_string());
        let diagram = serde_json::from_value(diagram).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let (mut socket, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(5))
                    }
                    Err(e) => panic!("HTTP scene fixture accept failed: {e}"),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() < 8192);
            }
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", png.len()).unwrap();
            socket.write_all(&png).unwrap();
        });
        let limits: Value = serde_json::from_str(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let resolver = Arc::new(
            RasterImageResolver::new(
                std::path::Path::new("."),
                serde_json::from_value(limits["request"]["limits"].clone()).unwrap(),
                Arc::new(UnrestrictedImageFiles::new()),
            )
            .unwrap()
            .with_http_client(Arc::new(NativeImageHttpClient {
                allow_private_networks: true,
            })),
        );
        let captured = Arc::new(CapturedResolver {
            resources: request["resources"].clone(),
            calls: Mutex::default(),
        });
        let mut options = options(request, &captured, false);
        options.assets.as_mut().unwrap().resolver_impl = Some(resolver.clone());
        let first = build(&diagram, &options).unwrap();
        server.join().unwrap();
        assert!(same_json(&encode(&first), &case["expected"]));
        let again = build(&diagram, &options).unwrap();
        assert!(same_json(&encode(&again), &case["expected"]));
        assert!(same_json(&encode(&first), &case["expected"]));
        assert_eq!(resolver.usage().assets, 1);
    }

    #[test]
    fn native_svg_resolution_preserves_original_scenes_and_shared_import_limits() {
        use d2_rs_scenebuild::{RasterImageResolver, UnrestrictedImageFiles};
        let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/image-assets-master.json"
        ))
        .unwrap();
        let limits: Value = serde_json::from_str(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl")
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let mut checked = 0;
        for case in fixture["cases"].as_array().unwrap() {
            let request = &case["input"];
            let resources = request["resources"].as_object().unwrap();
            if resources.is_empty() || !resources.values().all(|r| r["kind"] == 2) {
                continue;
            }
            let captured = Arc::new(CapturedResolver {
                resources: request["resources"].clone(),
                calls: Mutex::default(),
            });
            let mut options = options(request, &captured, false);
            let resolver = Arc::new(
                RasterImageResolver::new(
                    std::path::Path::new("."),
                    serde_json::from_value(limits["request"]["limits"].clone()).unwrap(),
                    Arc::new(UnrestrictedImageFiles::new()),
                )
                .unwrap(),
            );
            options.assets.as_mut().unwrap().resolver_impl = Some(resolver);
            let result = build(
                &serde_json::from_value(request["diagram"].clone()).unwrap(),
                &options,
            );
            if case["error"] == true {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    case["expected"].as_str().unwrap(),
                    "{}",
                    request["test"]
                );
            } else {
                assert!(
                    same_json(&encode(&result.unwrap()), &case["expected"]),
                    "{}",
                    request["test"]
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 10);
    }
}
