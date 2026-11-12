// SPDX-License-Identifier: MPL-2.0
//! Replay captured imageasset policy calls against real files. Requests contain
//! only source operations; no expected resource, error or cache decision.
// The replay drives the rooted local-file policy, which is Unix-only.
#[cfg(not(unix))]
fn main() {
    eprintln!("imageasset-policy-scoreboard needs a Unix host");
    std::process::exit(2);
}

#[cfg(unix)]
use base64::Engine as _;
#[cfg(unix)]
use d2_rs_scenebuild::{ImageKind, ImageLocalFiles, ImageMemoryCache, RasterImageResolver};
#[cfg(unix)]
use serde_json::{Value, json};
#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::path::{Component, Path, PathBuf};
#[cfg(unix)]
use std::sync::Arc;

#[cfg(unix)]
fn replay(case: &Value, fixture_root: &Path) -> Value {
    let mut sessions = HashMap::new();
    let mut caches = HashMap::<u64, Arc<ImageMemoryCache>>::new();
    let mut session_caches = HashMap::new();
    let mut top_dirs = Vec::new();
    let mut observations = Vec::new();
    let fixture_path = |op: &Value, field: &str| {
        let path = PathBuf::from(op[field].as_str().unwrap());
        assert!(path.is_absolute() && path.starts_with(fixture_root) && path != fixture_root);
        assert!(!path.components().any(|c| c == Component::ParentDir));
        path
    };
    for op in case["operations"].as_array().unwrap() {
        let id = op["id"].as_u64().unwrap_or(0);
        observations.push(match op["op"].as_str().unwrap() {
            "mkdir" => {
                use std::os::unix::fs::PermissionsExt;
                let path = fixture_path(op, "path");
                assert!(!path.exists() && !path.is_symlink());
                // testing.T.TempDir returns .../TestName<random>/001; its
                // private parent is created by testing, outside the observed
                // os.Mkdir calls. Recreate that same private fixture tree.
                std::fs::create_dir_all(&path).unwrap();
                std::fs::set_permissions(
                    &path,
                    std::fs::Permissions::from_mode(op["mode"].as_u64().unwrap() as u32),
                )
                .unwrap();
                let top = fixture_root.join(path.strip_prefix(fixture_root).unwrap().components().next().unwrap());
                if !top_dirs.contains(&top) {
                    top_dirs.push(top);
                }
                json!({"error":""})
            }
            "write" => {
                use std::os::unix::fs::OpenOptionsExt;
                let path = fixture_path(op, "path");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(op["body"].as_str().unwrap())
                    .unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(op["mode"].as_u64().unwrap() as u32)
                    .open(path)
                    .unwrap()
                    .write_all(&bytes)
                    .unwrap();
                json!({"error":""})
            }
            "symlink" => {
                let target = fixture_path(op, "target");
                std::os::unix::fs::symlink(target, fixture_path(op, "path")).unwrap();
                json!({"error":""})
            }
            "new" => {
                let policy = match op["policy"].as_str().unwrap() {
                    "denied" => ImageLocalFiles::default(),
                    "unrestricted" => ImageLocalFiles::Unrestricted,
                    "rooted" => ImageLocalFiles::rooted(&fixture_path(op, "root")).unwrap(),
                    _ => panic!("unknown file policy"),
                };
                let mut result = RasterImageResolver::new(
                    Path::new(op["base_dir"].as_str().unwrap()),
                    serde_json::from_value(op["limits"].clone()).unwrap(),
                    Arc::new(policy),
                );
                let cache_id = op["cache"].as_u64().unwrap();
                if cache_id != 0 {
                    let cache = caches.entry(cache_id).or_insert_with(|| {
                        Arc::new(
                            ImageMemoryCache::new(
                                op["cache_entries"].as_i64().unwrap(),
                                op["cache_bytes"].as_i64().unwrap(),
                            )
                            .unwrap(),
                        )
                    });
                    result = result.and_then(|r| {
                        r.with_shared_cache(cache.clone(), op["namespace"].as_str().unwrap(), false)
                    });
                    session_caches.insert(id, cache.clone());
                }
                let error = result.as_ref().err().map(ToString::to_string).unwrap_or_default();
                if let Ok(resolver) = result {
                    sessions.insert(id, resolver);
                }
                json!({"error":error})
            }
            "resolve" => {
                let result = sessions[&id].resolve_with_context(op["source"].as_str().unwrap(), None);
                let mut got = match result {
                    Ok(r) => json!({"error":"", "unavailable":false, "resource":{
                        "kind":match r.resource.kind {ImageKind::Raster=>"raster".to_owned(),ImageKind::Svg=>"svg".to_owned(),ImageKind::Other(k)=>format!("Kind({k})")},
                        "mime":r.resource.mime_type,
                        "width":r.resource.pixel_width,"height":r.resource.pixel_height,
                        "decoded":r.resource.decoded_bytes,
                        "data":base64::engine::general_purpose::STANDARD.encode(&r.resource.data),
                    }}),
                    Err(e) => json!({"error":e.message,"unavailable":e.unavailable,"resource":null}),
                };
                if let Some(cache) = session_caches.get(&id) {
                    got["cache_entries"] = json!(cache.usage().entries);
                    got["cache_bytes"] = json!(cache.usage().encoded);
                }
                got
            }
            other => panic!("unknown operation {other}"),
        });
    }
    drop(sessions);
    drop(caches);
    for path in top_dirs {
        std::fs::remove_dir_all(path).unwrap();
    }
    json!(observations)
}

#[cfg(unix)]
fn main() {
    let request: Value =
        serde_json::from_slice(&std::fs::read(std::env::args().nth(1).unwrap()).unwrap()).unwrap();
    let fixture_root = Path::new(request["fixture_root"].as_str().unwrap());
    assert!(fixture_root.is_absolute() && fixture_root.is_dir());
    let output: Vec<_> = request["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| replay(c, fixture_root))
        .collect();
    println!("{}", json!(output));
}
