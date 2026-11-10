//! Component benchmark: real SQLite and ranking, synthetic embeddings, no inference.
use super::*;
use crate::file_manager::search::{
    policy::Root,
    store::{DIMENSION, EXTRACTION_VERSION, encode_vector},
};
use rusqlite::params;
use serde_json::json;
use std::{fs::File, io::Write, os::unix::fs::MetadataExt, time::Instant};

struct Corpus {
    _cache: tempfile::TempDir,
    source: tempfile::TempDir,
    store: Store,
    files: usize,
    segments: usize,
}
impl Corpus {
    fn create(files: usize, extra_segments: usize) -> Self {
        assert!(extra_segments <= 2);
        let workspace = std::env::var_os("CLIP_RETRIEVAL_BENCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let cache = tempfile::tempdir_in(&workspace).unwrap();
        let source = tempfile::tempdir_in(&workspace).unwrap();
        for index in 0..files {
            let mut file =
                File::create(source.path().join(format!("item_{index:06}.txt"))).unwrap();
            file.write_all(body(index, files).as_bytes()).unwrap();
        }
        let entries = filesystem::read_directory(source.path(), &AtomicBool::new(false))
            .unwrap()
            .entries;
        assert_eq!(entries.len(), files);
        let mut store = Store::open(cache.path()).unwrap();
        let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(root).unwrap();
        let scope: String = store
            .connection
            .query_row(
                "SELECT scope FROM source_sessions WHERE root_id=?1",
                [root],
                |row| row.get(0),
            )
            .unwrap();
        let vectors = vectors();
        let segments;
        {
            let tx = store.connection.transaction().unwrap();
            let mut file = tx.prepare("INSERT INTO files(id,device,inode,revision,pipeline,metadata,state,coverage,extraction_version,scope,format) VALUES(?1,?2,?3,?4,'benchmark-v1',?5,'ready','complete',?6,?7,?8)").unwrap();
            let mut path = tx
                .prepare("INSERT INTO paths(path,parent,file_id) VALUES(?1,?2,?3)")
                .unwrap();
            let mut membership = tx
                .prepare(
                    "INSERT INTO memberships(root_id,path,generation,verified) VALUES(?1,?2,?3,1)",
                )
                .unwrap();
            let mut segment = tx.prepare("INSERT INTO segments(file_id,modality,location,text,input_hash,pipeline,vector) VALUES(?1,'text',?2,?3,?4,'benchmark-v1',?5)").unwrap();
            let mut count = 0;
            for (index, entry) in entries.iter().enumerate() {
                let id = index as i64 + 1;
                let metadata = std::fs::metadata(&entry.path).unwrap();
                file.execute(params![
                    id,
                    metadata.dev() as i64,
                    metadata.ino() as i64,
                    entry.revision as i64,
                    serde_json::to_string(entry).unwrap(),
                    EXTRACTION_VERSION,
                    scope,
                    crate::file_manager::search::store::source_format(entry),
                ])
                .unwrap();
                let bytes = entry.path.as_os_str().as_encoded_bytes();
                path.execute(params![
                    bytes,
                    source.path().as_os_str().as_encoded_bytes(),
                    id
                ])
                .unwrap();
                membership
                    .execute(params![root, bytes, generation])
                    .unwrap();
                let vector = &vectors[if index % 10_000 == 0 || index + 1 == files {
                    0
                } else {
                    1 + index * 37 % 1024
                }];
                for ordinal in 0..1 + index % 2 + extra_segments {
                    segment
                        .execute(params![
                            id,
                            format!("passage {ordinal}"),
                            body(index, files),
                            vec![1_u8; 32],
                            vector
                        ])
                        .unwrap();
                    count += 1;
                }
            }
            drop((file, path, membership, segment));
            tx.commit().unwrap();
            segments = count;
        }
        store.finish_scan(root, None).unwrap();
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM segments WHERE length(vector)=1536",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            segments
        );
        // Bulk fixture seeding deliberately bypasses Store::publish. Reopen as
        // a real persisted index so its vector-count admission is reconstructed.
        drop(store);
        let store = Store::open(cache.path()).unwrap();
        // Benchmark-only override: measure pager working-set pressure before
        // changing the application's memory budget. Record the actual setting.
        if let Ok(value) = std::env::var("CLIP_RETRIEVAL_BENCH_SQLITE_CACHE_MIB") {
            let mib: i64 = value.parse().unwrap();
            assert!((1..=512).contains(&mib));
            store
                .connection
                .pragma_update(None, "cache_size", -mib * 1024)
                .unwrap();
        }
        Self {
            _cache: cache,
            source,
            store,
            files,
            segments,
        }
    }
    fn diagnostics(&self, retrieval: &Retrieval<'_>) -> serde_json::Value {
        let expression = terms(retrieval.text).unwrap().join(" AND ");
        let mut scans = serde_json::Map::new();
        // Diagnostic scans keep an empty threshold: count the full SQL scope.
        let mut diagnostic_top = Top::default();
        let _rank_filter =
            LexicalRankFilter::new(&self.store.connection, &mut diagnostic_top).unwrap();
        for (name, select, expression) in [
            ("names", NAMES_SELECT, None),
            ("keywords_and", FTS_SELECT, Some(expression.as_str())),
            ("meaning", retrieval.semantic_select(), None),
        ] {
            if (name == "keywords_and" && expression == Some(""))
                || (name == "meaning" && retrieval.vector.is_empty())
            {
                continue;
            }
            let started = Instant::now();
            let mut statement = retrieval
                .statement(&format!("{select}{}", retrieval.current()), expression)
                .unwrap();
            let mut rows = statement.raw_query();
            let mut count = 0;
            while rows.next().unwrap().is_some() {
                count += 1;
            }
            scans.insert(
                name.into(),
                json!({"rows":count,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0}),
            );
        }
        let scoring = if retrieval.vector.is_empty() {
            serde_json::Value::Null
        } else {
            let prototypes = vectors();
            let started = Instant::now();
            let mut checksum = 0.0_f64;
            for index in 0..self.segments {
                checksum += f64::from(
                    cosine_score(
                        std::hint::black_box(retrieval.vector),
                        std::hint::black_box(&prototypes[index % prototypes.len()]),
                    )
                    .unwrap(),
                );
            }
            std::hint::black_box(checksum);
            json!({"vectors":self.segments,"mode":"uncached f16 decoding/normalization and scoring","elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
        };
        json!({"scope":"independent single warm SQL scans and in-memory scoring probe after latency sampling; not an additive decomposition or end-to-end latency", "sql_scans":scans,"scoring_probe":scoring,"vector_cache_usage":self.store.vector_cache_usage()})
    }
    fn verify_cancelled_stream(
        &self,
        retrieval: &Retrieval<'_>,
        expected: &[Hit],
    ) -> Option<serde_json::Value> {
        let progress = retrieval.progress?;
        if self.segments < 187_500 {
            return None;
        }
        let (cached, reserved) = self.store.vector_cache_usage();
        // Every fixture segment has one path and vector. Passing the total
        // cache population proves some uncached vectors have been visited,
        // without assuming which IDs the cache retained or the SQL row order.
        let threshold = cached as u64 + 2048;
        assert!(threshold + 2048 < self.segments as u64);
        progress.store(0, AtomicOrdering::Relaxed);
        let cancel = retrieval.cancel;
        let done = AtomicBool::new(false);
        let started = Instant::now();
        let result = std::thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + std::time::Duration::from_secs(30);
                while progress.load(AtomicOrdering::Relaxed) < threshold
                    && !done.load(AtomicOrdering::Acquire)
                    && Instant::now() < deadline
                {
                    std::thread::park_timeout(std::time::Duration::from_millis(1));
                }
                if progress.load(AtomicOrdering::Relaxed) >= threshold {
                    cancel.store(true, AtomicOrdering::Release);
                }
            });
            let result = retrieval.search();
            done.store(true, AtomicOrdering::Release);
            result
        });
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let checked = progress.load(AtomicOrdering::Relaxed);
        let error = result.err().expect("An active streaming scan must cancel");
        assert!(
            error.contains("cancel") || error.contains("interrupt"),
            "{error}"
        );
        assert!(checked >= threshold && checked < self.segments as u64);
        assert_eq!(self.store.vector_cache_usage(), (cached, reserved));
        retrieval.cancel.store(false, AtomicOrdering::Release);
        let recovered = retrieval.search().unwrap();
        assert_eq!(progress.load(AtomicOrdering::Relaxed), self.segments as u64);
        assert!(
            recovered
                .iter()
                .map(|hit| &hit.entry.path)
                .eq(expected.iter().map(|hit| &hit.entry.path))
        );
        Some(
            json!({"scope":"active component scan beyond cache population, not native UI cancellation",
            "threshold_references":threshold,"cancelled_checked_references":checked,
            "total_references":self.segments,"cached_vectors":cached,"reserved_bytes":reserved,
            "scan_until_cancelled_ms":elapsed_ms,"error":error,"recovered_ordered_paths":true}),
        )
    }
    fn measure(
        &self,
        name: &str,
        text: &str,
        semantic: bool,
        floor: f32,
        samples: usize,
    ) -> serde_json::Value {
        let query = Query::parse(&format!(":emb:\"{text}\";scope:indexed")).unwrap();
        let mut vector = vec![0.0; DIMENSION];
        vector[0] = 1.0;
        let floors = BTreeMap::from([("text".into(), floor)]);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = AtomicU64::new(0);
        let retrieval = Retrieval {
            progress: semantic.then_some(&progress),
            store: &self.store,
            pipeline: if semantic { "benchmark-v1" } else { "" },
            vector: if semantic { &vector } else { &[] },
            path: self.source.path(),
            query: &query,
            text,
            hidden: false,
            index_hidden: false,
            show_offline: false,
            exclusions: &[],
            floors: &floors,
            cancel: &cancel,
        };
        register_name_filter(&self.store.connection).unwrap();
        self.store.prepare_vector_cache().unwrap();
        let mut plans = serde_json::Map::new();
        let mut plan_top = Top::default();
        let plan_filter = LexicalRankFilter::new(&self.store.connection, &mut plan_top).unwrap();
        for (label, select, expression) in [
            ("names", NAMES_SELECT, None),
            ("keywords", FTS_SELECT, Some("\"commonterm\"*")),
            ("meaning", retrieval.semantic_select(), None),
        ] {
            let mut statement = retrieval
                .statement(
                    &format!("EXPLAIN QUERY PLAN {select}{}", retrieval.current()),
                    expression,
                )
                .unwrap();
            let mut rows = statement.raw_query();
            let mut details = Vec::new();
            while let Some(row) = rows.next().unwrap() {
                details.push(row.get::<_, String>(3).unwrap());
            }
            plans.insert(label.into(), json!(details));
        }
        drop(plan_filter);
        let first = Instant::now();
        let first_hits = retrieval.search().unwrap();
        let first_ms = first.elapsed().as_secs_f64() * 1000.0;
        if name == "exact_filename" {
            assert_eq!(first_hits.first().unwrap().entry.name, text);
        }
        if name == "rare_keyword" {
            assert_eq!(first_hits.len(), 1);
        }
        if semantic {
            assert!(!first_hits.is_empty());
            assert_eq!(progress.load(AtomicOrdering::Relaxed), self.segments as u64);
            let (cached, reserved) = self.store.vector_cache_usage();
            if self.segments <= 150_000 {
                assert_eq!(
                    cached, self.segments,
                    "the acceptance workload must exercise a warmed vector cache"
                );
            } else if self.segments >= 187_500 {
                assert!(
                    cached > 0 && cached < self.segments,
                    "the larger workload must exercise both cached and uncached rows"
                );
            }
            assert!(
                first_hits
                    .iter()
                    .any(|hit| hit.entry.name == format!("item_{:06}.txt", self.files - 1)),
                "the final high-scoring file must remain searchable beyond the cache limit"
            );
            if self.segments >= 187_500 {
                let last = self
                    .source
                    .path()
                    .join(format!("item_{:06}.txt", self.files - 1));
                let cached_last: i64 = self.store.connection.query_row(
                    "SELECT max(clip_vector_cached(s.id)) FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE p.path=?1",
                    [last.as_os_str().as_encoded_bytes()], |row| row.get(0)).unwrap();
                assert_eq!(
                    cached_last, 0,
                    "the late winning file must exercise uncached scoring"
                );
            }
            assert!(reserved <= 512 * 1024 * 1024);
        }
        let mut timings = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            let hits = std::hint::black_box(retrieval.search().unwrap());
            assert_eq!(hits.len(), first_hits.len());
            timings.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        timings.sort_by(f64::total_cmp);
        eprintln!(
            "{name}: {} samples, p95 {:.3} ms",
            samples,
            timings[(samples * 95).div_ceil(100) - 1]
        );
        // Run outside latency samples for above-cap semantic workloads.
        let cancellation = self.verify_cancelled_stream(&retrieval, &first_hits);
        json!({"name":name,"diagnostics":self.diagnostics(&retrieval),"sqlite_query_plans":plans,"first_query_ms":first_ms,"samples":samples,"median_ms":timings[samples/2],
            "cosine_floor":floor,"progress_checked_references":semantic.then(||progress.load(AtomicOrdering::Relaxed)),"stream_cancellation":cancellation,"p95_ms":timings[(samples*95).div_ceil(100)-1],"min_ms":timings[0],"max_ms":timings[samples-1],"hits":first_hits.len()})
    }
}
fn body(index: usize, files: usize) -> &'static str {
    if index == files / 2 {
        "commonterm raresignalidentifier"
    } else {
        "commonterm ordinarydocument"
    }
}
fn vectors() -> Vec<Vec<u8>> {
    std::iter::once(0.95_f32)
        .chain((0..1024).map(|index| (index as f32 - 512.0) / 2560.0))
        .map(|score| {
            let mut vector = vec![0.0; DIMENSION];
            vector[0] = score;
            vector[1] = (1.0 - score * score).sqrt();
            encode_vector(&vector).unwrap()
        })
        .collect()
}

#[test]
#[ignore = "exports an owned synthetic index for explicit private-compositor scale measurements"]
fn export_native_scale_fixture() {
    let workspace = PathBuf::from(
        std::env::var_os("CLIP_RETRIEVAL_BENCH_ROOT").expect("Set a driver-owned workspace"),
    );
    assert!(workspace.is_absolute() && workspace.is_dir());
    let files: usize = std::env::var("CLIP_RETRIEVAL_BENCH_FILES")
        .unwrap()
        .parse()
        .unwrap();
    assert!((100..=100_000).contains(&files));
    let extra_segments = std::env::var("CLIP_RETRIEVAL_UI_EXTRA_SEGMENTS")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .unwrap();
    let pipeline = std::env::var("CLIP_RETRIEVAL_UI_PIPELINE").unwrap();
    assert!(pipeline.starts_with("search-v2:") && pipeline.len() < 8192);
    let corpus = Corpus::create(files, extra_segments);
    // Reuse actual metadata, source sessions and the canonical Store schema.
    // Only the vectors are synthetic: this fixture is for latency, not quality.
    // Match the installed model revision so production admission can run intact.
    let tx = corpus.store.connection.unchecked_transaction().unwrap();
    for table in ["files", "segments"] {
        tx.execute(&format!("UPDATE {table} SET pipeline=?1"), [&pipeline])
            .unwrap();
    }
    tx.commit().unwrap();
    let Corpus {
        _cache: cache,
        source,
        store,
        files,
        segments,
    } = corpus;
    drop(store);
    // The driver owns the enclosing TemporaryDirectory and removes the whole
    // fixture after the private application has stopped, including failures.
    let source = source.keep();
    let cache = cache.keep();
    std::fs::write(
        workspace.join("native-fixture.json"),
        serde_json::to_vec_pretty(&json!({
            "source":source,"cache":cache,"files":files,"vectors":segments,
            "pipeline":pipeline,"synthetic_vectors":true,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "explicit local retrieval-scale benchmark; run optimized for acceptance measurements"]
fn retrieval_scale_benchmark() {
    let files = std::env::var("CLIP_RETRIEVAL_BENCH_FILES")
        .ok()
        .map(|value| value.parse().unwrap())
        .unwrap_or(100_000);
    let samples = std::env::var("CLIP_RETRIEVAL_BENCH_SAMPLES")
        .ok()
        .map(|value| value.parse().unwrap())
        .unwrap_or(21);
    assert!((100..=200_000).contains(&files) && (1..=100).contains(&samples));
    let started = Instant::now();
    eprintln!("Preparing {files} ordinary fixture files and their synthetic-vector index");
    let corpus = Corpus::create(files, 0);
    let preparation = started.elapsed().as_secs_f64();
    let default_floor = little_clip_core::config::FileSearchConfig::default_semantic_floor("text");
    let cases = vec![
        corpus.measure(
            "exact_filename",
            &format!("item_{:06}.txt", files / 2),
            false,
            default_floor,
            samples,
        ),
        corpus.measure(
            "rare_keyword",
            "raresignalidentifier",
            false,
            default_floor,
            samples,
        ),
        corpus.measure(
            "common_keyword",
            "commonterm",
            false,
            default_floor,
            samples,
        ),
        corpus.measure(
            "hybrid_default_floor",
            "abstract meaning",
            true,
            default_floor,
            samples,
        ),
        corpus.measure(
            "hybrid_broad_floor",
            "abstract meaning",
            true,
            -1.0,
            samples,
        ),
    ];
    let report = json!({"profile":if cfg!(debug_assertions) {"debug"} else {"release"},"files":corpus.files,
        "vectors":corpus.segments,"sqlite_cache_kib":-corpus.store.connection.pragma_query_value(None,"cache_size", |row| row.get::<_,i64>(0)).unwrap(),"dimensions":DIMENSION,"storage":"f16","preparation_seconds":preparation,
        "scope":"SQLite/ranking component only; synthetic unit vectors; excludes model inference, source verification, explanations and UI publication; first-query timing is not disk-cold", "cases":cases});
    let path = std::env::var_os("CLIP_RETRIEVAL_BENCH_REPORT")
        .expect("Set an owned benchmark report path");
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("{report}");
}
