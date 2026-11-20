//! Opt-in production-path terrain measurements; no instrumentation in GUI builds.
use super::*;
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

#[derive(Default)]
struct ReadTiming {
    file: String,
    bytes: usize,
    read: Duration,
    decode: Duration,
}
static READS: Mutex<Option<Vec<ReadTiming>>> = Mutex::new(None);
pub(super) fn record(file: &OsStr, bytes: usize, read: Duration, decode: Duration) {
    if let Some(reads) = READS.lock().unwrap().as_mut() {
        reads.push(ReadTiming {
            file: file.to_string_lossy().into_owned(),
            bytes,
            read,
            decode,
        });
    }
}

#[test]
#[ignore = "requires downloaded assets; reports read/decode/cache timings without flushing system caches"]
fn terrain_loading_profile() {
    let root = std::env::var_os("LITTLE_CLIP_GEOGRAPHY_ACCEPTANCE_DIR")
        .expect("set acceptance dataset directory");
    let started = Instant::now();
    let assets = BasemapAssets::load(Path::new(&root), None);
    eprintln!(
        "TERRAIN_PROFILE inventory_ms={:.2} hgt_files={}",
        started.elapsed().as_secs_f64() * 1000.,
        assets.status().local_hgt_count
    );
    if let Ok(expected) = std::env::var("LITTLE_CLIP_GEOGRAPHY_PROFILE_EXPECT_HGT") {
        assert_eq!(
            assets.status().local_hgt_count,
            expected.parse::<usize>().unwrap(),
            "Concurrent migration must preserve the complete inventory"
        );
    }
    let selected = std::env::var("LITTLE_CLIP_GEOGRAPHY_PROFILE_CASE").unwrap_or_default();
    for (name, lat, lon, zoom) in [
        ("omaha-close", 41.265, -95.90, 3500.),
        ("omaha-medium", 41.265, -95.90, 400.),
        ("rockies-close", 39.50, -105.60, 3500.),
    ] {
        if !selected.is_empty() && selected != name {
            continue;
        }
        for pass in 0..2 {
            *READS.lock().unwrap() = Some(Vec::new());
            let camera = GlobeCamera {
                center_lat: lat,
                center_lon: lon,
                zoom,
                mode: ProjectionMode::Map2D,
            };
            let ids = tiles::visible_tiles(&camera, 1280., 800., 1.);
            let next = AtomicUsize::new(0);
            let cancel = AtomicBool::new(false);
            let started = Instant::now();
            std::thread::scope(|scope| {
                for _ in 0..2 {
                    scope.spawn(|| {
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(&id) = ids.get(index) else {
                                break;
                            };
                            std::hint::black_box(
                                tiles::SourceTile::prepare(&assets, id, &cancel).unwrap(),
                            );
                        }
                    });
                }
            });
            let elapsed = started.elapsed();
            let records = READS.lock().unwrap().take().unwrap();
            let mut files = BTreeMap::new();
            for record in &records {
                *files.entry(&record.file).or_insert(0usize) += 1;
            }
            eprintln!(
                "TERRAIN_PROFILE case={name} pass={pass} tiles={} elapsed_ms={:.2} reads={} bytes={} read_ms={:.2} decode_ms={:.2} files={files:?}",
                ids.len(),
                elapsed.as_secs_f64() * 1000.,
                records.len(),
                records.iter().map(|r| r.bytes).sum::<usize>(),
                records
                    .iter()
                    .map(|r| r.read.as_secs_f64() * 1000.)
                    .sum::<f64>(),
                records
                    .iter()
                    .map(|r| r.decode.as_secs_f64() * 1000.)
                    .sum::<f64>()
            );
        }
    }
}
