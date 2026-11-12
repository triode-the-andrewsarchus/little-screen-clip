// SPDX-License-Identifier: MPL-2.0
// Every test here starts from the regenerated imageasset capture (docs/FIXTURES.md).
#![cfg(feature = "oracle-fixtures")]
use base64::Engine as _;
use d2_rs_core::CancellationToken;
use d2_rs_scenebuild::{
    ImageAssetLimits, ImageFileReader, ImageMemoryCache, ImageResolveError, RasterImageResolver,
    UnrestrictedImageFiles,
};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn source() -> (ImageAssetLimits, Vec<u8>) {
    let row: serde_json::Value = serde_json::from_str(
        d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    (
        serde_json::from_value(row["request"]["limits"].clone()).unwrap(),
        base64::engine::general_purpose::STANDARD
            .decode(row["expected"]["resource"]["data"].as_str().unwrap())
            .unwrap(),
    )
}

#[derive(Debug)]
struct Files {
    scope: usize,
    reads: AtomicUsize,
    data: Vec<u8>,
}
impl ImageFileReader for Files {
    fn cache_key(&self, path: &Path) -> Result<Option<String>, ImageResolveError> {
        Ok(Some(format!(
            "test-reader:{}:{}",
            self.scope,
            path.display()
        )))
    }
    fn read(
        &self,
        _: &Path,
        _: i64,
        _: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(self.data.clone())
    }
}

#[test]
fn snapshot_ownership_per_resolver_charges_and_atomic_failed_hits() {
    let (limits, png) = source();
    let files = Arc::new(Files {
        scope: 1,
        reads: AtomicUsize::new(0),
        data: png.clone(),
    });
    let cache = Arc::new(ImageMemoryCache::new(2, 1 << 20).unwrap());
    let make = |limits| {
        RasterImageResolver::new(Path::new("."), limits, files.clone())
            .unwrap()
            .with_shared_cache(cache.clone(), "tenant", false)
            .unwrap()
    };
    let first = make(limits);
    let mut returned = first.resolve_with_context("icon.png", None).unwrap();
    returned.resource.mime_type = "tampered".into();
    Arc::make_mut(&mut returned.resource.data)[0] = 0;
    let second = make(limits);
    let hit = second.resolve_with_context("icon.png", None).unwrap();
    assert_eq!(hit.resource.data.as_ref(), png);
    assert_eq!(hit.resource.mime_type, "image/png");
    assert_eq!(second.usage().assets, 1);
    assert_eq!(files.reads.load(Ordering::Relaxed), 1);
    let canceled = CancellationToken::new();
    canceled.cancel();
    let third = make(limits);
    assert!(
        third
            .resolve_with_context("icon.png", Some(&canceled))
            .is_err()
    );
    assert_eq!(third.usage().assets, 0);
    third.resolve_with_context("icon.png", None).unwrap();
    assert_eq!(third.usage().assets, 1);
    let mut strict = limits;
    strict.max_cumulative_encoded_bytes = png.len() as i64 - 1;
    let fourth = make(strict);
    for _ in 0..2 {
        let error = fourth.resolve_with_context("icon.png", None).unwrap_err();
        assert!(error.message.contains("reserve resource"));
        assert_eq!(fourth.usage().assets, 0);
        assert_eq!(fourth.usage().encoded, 0);
    }
    assert_eq!(files.reads.load(Ordering::Relaxed), 1);
}

#[test]
fn cache_limits_namespace_network_and_reader_identity_are_explicit() {
    for (entries, bytes) in [(0, 1), (1, 0), (-1, 1), (1, -1)] {
        assert_eq!(
            ImageMemoryCache::new(entries, bytes).unwrap_err().message,
            "imageasset: cache limits must be positive"
        );
    }
    let (limits, png) = source();
    let files = Arc::new(Files {
        scope: 2,
        reads: AtomicUsize::new(0),
        data: png.clone(),
    });
    let cache = Arc::new(ImageMemoryCache::new(8, 1 << 20).unwrap());
    assert_eq!(
        RasterImageResolver::new(Path::new("."), limits, files.clone())
            .unwrap()
            .with_shared_cache(cache.clone(), "", false)
            .unwrap_err()
            .message,
        "imageasset: CacheNamespace is required when Cache is set"
    );
    for (namespace, private) in [
        ("one", false),
        ("one", true),
        ("two", false),
        ("one", false),
    ] {
        let resolver = RasterImageResolver::new(Path::new("."), limits, files.clone())
            .unwrap()
            .with_shared_cache(cache.clone(), namespace, private)
            .unwrap();
        resolver.resolve_with_context("icon.png", None).unwrap();
    }
    assert_eq!(files.reads.load(Ordering::Relaxed), 3);
    let separate = Arc::new(Files {
        scope: 3,
        reads: AtomicUsize::new(0),
        data: png,
    });
    RasterImageResolver::new(Path::new("."), limits, separate.clone())
        .unwrap()
        .with_shared_cache(cache, "one", false)
        .unwrap()
        .resolve_with_context("icon.png", None)
        .unwrap();
    assert_eq!(separate.reads.load(Ordering::Relaxed), 1);
}

#[test]
fn unrestricted_cache_survives_file_removal_but_not_new_base_directory() {
    let (limits, png) = source();
    let root = std::env::temp_dir().join(format!("d2rs-shared-cache-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("icon.png"), png).unwrap();
    let cache = Arc::new(ImageMemoryCache::new(2, 1 << 20).unwrap());
    let make = |base: &Path| {
        RasterImageResolver::new(base, limits, Arc::new(UnrestrictedImageFiles::new()))
            .unwrap()
            .with_shared_cache(cache.clone(), "local", false)
            .unwrap()
    };
    make(&root).resolve_with_context("icon.png", None).unwrap();
    std::fs::remove_file(root.join("icon.png")).unwrap();
    assert!(make(&root).resolve_with_context("icon.png", None).is_ok());
    assert!(
        make(&root.join("other"))
            .resolve_with_context("icon.png", None)
            .is_err()
    );
    std::fs::remove_dir(root).unwrap();
}

#[derive(Debug)]
struct UnscopedFiles(Files);
impl ImageFileReader for UnscopedFiles {
    fn read(
        &self,
        path: &Path,
        limit: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError> {
        self.0.read(path, limit, cancel)
    }
}

#[test]
fn custom_readers_skip_shared_file_caching_until_they_supply_a_policy_key() {
    let (limits, png) = source();
    let files = Arc::new(UnscopedFiles(Files {
        scope: 4,
        reads: AtomicUsize::new(0),
        data: png,
    }));
    let cache = Arc::new(ImageMemoryCache::new(2, 1 << 20).unwrap());
    for _ in 0..2 {
        RasterImageResolver::new(Path::new("."), limits, files.clone())
            .unwrap()
            .with_shared_cache(cache.clone(), "tenant", false)
            .unwrap()
            .resolve_with_context("icon.png", None)
            .unwrap();
    }
    assert_eq!(files.0.reads.load(Ordering::Relaxed), 2);
    assert_eq!(cache.usage().entries, 0);
}

#[test]
fn concurrent_shared_hits_each_charge_their_own_session_once() {
    let (limits, png) = source();
    let files = Arc::new(Files {
        scope: 5,
        reads: AtomicUsize::new(0),
        data: png,
    });
    let cache = Arc::new(ImageMemoryCache::new(2, 1 << 20).unwrap());
    let make = || {
        RasterImageResolver::new(Path::new("."), limits, files.clone())
            .unwrap()
            .with_shared_cache(cache.clone(), "tenant", false)
            .unwrap()
    };
    make().resolve_with_context("icon.png", None).unwrap();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let resolver = make();
            scope.spawn(move || {
                for _ in 0..2 {
                    resolver.resolve_with_context("icon.png", None).unwrap();
                }
                assert_eq!(resolver.usage().assets, 1);
                assert_eq!(resolver.usage().decoded, 24);
            });
        }
    });
    assert_eq!(files.reads.load(Ordering::Relaxed), 1);
}
