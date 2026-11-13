// SPDX-License-Identifier: MPL-2.0
//! Replay original imageasset calls with a read-only captured file boundary.
use base64::Engine as _;
use d2_rs_core::CancellationToken;
use d2_rs_scenebuild::{
    ImageFileReader, ImageHttpClient, ImageHttpResponse, ImageMemoryCache, ImageResolveError,
    RasterImageResolver,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    io::BufRead as _,
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Debug, Default)]
struct Files(Mutex<HashMap<String, Vec<u8>>>);
#[derive(Debug, Default)]
struct Http(Mutex<VecDeque<Value>>);
impl ImageHttpClient for Http {
    fn get(
        &self,
        url: &str,
        _: i64,
        _: Option<&CancellationToken>,
    ) -> Result<ImageHttpResponse, ImageResolveError> {
        let call = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("uncaptured HTTP call");
        assert_eq!(url, call["url"].as_str().unwrap());
        for (name, value) in d2_rs_scenebuild::IMAGE_HTTP_HEADERS {
            assert_eq!(call["request_headers"][name][0], value);
        }
        if let Some(message) = call["error"].as_str() {
            return Err(ImageResolveError {
                message: message.into(),
                unavailable: call["unavailable"].as_bool().unwrap(),
            });
        }
        Ok(ImageHttpResponse {
            status: call["status"].as_u64().unwrap() as u16,
            content_length: call["content_length"].as_i64().unwrap(),
            headers: call["headers"]
                .as_object()
                .unwrap()
                .iter()
                .flat_map(|(key, values)| {
                    values
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| (key.clone(), v.as_str().unwrap().into()))
                })
                .collect(),
            body: base64::engine::general_purpose::STANDARD
                .decode(call["body"].as_str().unwrap())
                .unwrap(),
        })
    }
}
#[derive(Default)]
struct Sessions {
    resolvers: HashMap<i64, (RasterImageResolver, Arc<Files>, Arc<Http>)>,
    caches: HashMap<i64, Arc<ImageMemoryCache>>,
}
impl ImageFileReader for Files {
    fn cache_key(&self, path: &Path) -> Result<Option<String>, ImageResolveError> {
        // Selected original tests explicitly use localfile.Unrestricted.
        Ok(Some(format!("unrestricted:{}", path.display())))
    }
    fn read(
        &self,
        path: &Path,
        max_bytes: i64,
        _: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError> {
        let data = self
            .0
            .lock()
            .unwrap()
            .get(path.to_str().unwrap())
            .expect("uncaptured file read")
            .clone();
        if data.len() as i64 > max_bytes {
            Err(ImageResolveError {
                unavailable: false,
                message: format!("fetched bytes {} exceeds limit {max_bytes}", data.len()),
            })
        } else {
            Ok(data)
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sessions = Sessions::default();
    for line in std::io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line?)?;
        println!(
            "{}",
            serde_json::to_string(&replay(&request, &mut sessions)?)?
        );
    }
    Ok(())
}

fn replay(request: &Value, sessions: &mut Sessions) -> Result<Value, Box<dyn std::error::Error>> {
    let session = request["session"].as_i64().unwrap();
    let cache = request.get("cache").map(|options| {
        sessions
            .caches
            .entry(options["id"].as_i64().unwrap())
            .or_insert_with(|| {
                Arc::new(
                    ImageMemoryCache::new(
                        options["max_entries"].as_i64().unwrap(),
                        options["max_bytes"].as_i64().unwrap(),
                    )
                    .unwrap(),
                )
            })
            .clone()
    });
    let (resolver, files, http) = sessions.resolvers.entry(session).or_insert_with(|| {
        let files = Arc::new(Files::default());
        let http = Arc::new(Http::default());
        let mut resolver = RasterImageResolver::new(
            Path::new(request["base_dir"].as_str().unwrap()),
            serde_json::from_value(request["limits"].clone()).unwrap(),
            files.clone(),
        )
        .unwrap()
        .with_http_client(http.clone());
        if let Some(cache) = &cache {
            // The captured prefix is an opaque namespace token. Rehashing
            // preserves the original namespace/policy equivalence classes.
            resolver = resolver
                .with_shared_cache(
                    cache.clone(),
                    request["cache"]["prefix"].as_str().unwrap(),
                    false,
                )
                .unwrap();
        }
        (resolver, files, http)
    });
    if let Some(calls) = request["http"].as_array() {
        http.0.lock().unwrap().extend(calls.iter().cloned());
    }
    if let Some(inputs) = request["files"].as_object() {
        for (path, data) in inputs {
            files.0.lock().unwrap().insert(
                path.clone(),
                base64::engine::general_purpose::STANDARD.decode(data.as_str().unwrap())?,
            );
        }
    }
    let result = resolver.resolve_with_context(request["source"].as_str().unwrap(), None);
    assert!(http.0.lock().unwrap().is_empty(), "unconsumed HTTP calls");
    let mut output = serde_json::to_value(resolver.usage())?;
    if let Some(cache) = cache {
        output["cache"] = serde_json::to_value(cache.usage())?;
    }
    match result {
        Err(error) => {
            output["error"] = json!(error.message);
            output["unavailable"] = json!(error.unavailable);
        }
        Ok(result) => {
            let uri = String::from_utf8(result.data_uri(None)?)?;
            let r = &result.resource;
            let kind = match r.kind {
                d2_rs_scenebuild::ImageKind::Raster => 1,
                d2_rs_scenebuild::ImageKind::Svg => 2,
                d2_rs_scenebuild::ImageKind::Other(value) => value,
            };
            output["resource"] = json!({"kind":kind,"mime_type":r.mime_type,"data":base64::engine::general_purpose::STANDARD.encode(&r.data),
                    "pixel_width":r.pixel_width,"pixel_height":r.pixel_height,"decoded_bytes":r.decoded_bytes,
                    "fetched_bytes":result.fetched_bytes,"decompressed_bytes":result.decompressed_bytes,"data_uri":uri});
        }
    }
    Ok(output)
}

#[cfg(all(test, feature = "oracle-fixtures"))]
mod tests {
    use super::*;

    #[test]
    fn original_local_data_uri_resources_and_atomic_limit_failures() {
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl"),
            17,
        );
    }

    #[test]
    fn original_svg_resources_and_rejected_envelopes() {
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-svg-master.jsonl"),
            8,
        );
    }

    #[test]
    fn original_svg_compression_and_atomic_limit_failure() {
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-svg-codecs-master.jsonl"),
            7,
        );
    }

    #[test]
    fn original_http_resources_redirect_policy_and_transport_limits() {
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-http-master.jsonl"),
            11,
        );
    }

    #[test]
    fn original_http_headers_and_fragment_canonicalization() {
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-headers-master.jsonl"),
            7,
        );
    }

    #[test]
    fn original_cache_eviction_limits_local_snapshots_and_namespaces() {
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-cache-master.jsonl"),
            11,
        );
        check(
            d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-cache-local-master.jsonl"),
            11,
        );
        check(
            d2_rs_testdata::oracle_fixture!(
                "tests/fixtures/imageasset-cache-namespace-master.jsonl"
            ),
            3,
        );
    }

    fn check(input: &str, count: usize) {
        let mut sessions = Sessions::default();
        let rows: Vec<Value> = input
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(rows.len(), count);
        for row in rows {
            assert_eq!(
                replay(&row["request"], &mut sessions).unwrap(),
                row["expected"],
                "{}",
                row["request"]["test"]
            );
        }
    }
}
