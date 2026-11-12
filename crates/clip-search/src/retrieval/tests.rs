use super::*;
use crate::{
    policy::Root,
    store::{DIMENSION, Segment},
};
use little_clip_core::file_inventory::read_directory;
use rusqlite::OptionalExtension;
struct Fixture {
    _cache: tempfile::TempDir,
    source: tempfile::TempDir,
    store: Store,
    root: i64,
    generation: i64,
    exclusions: Vec<PathBuf>,
    index_hidden: bool,
    show_offline: bool,
}
impl Fixture {
    fn new() -> Self {
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let mut store = Store::open(cache.path()).unwrap();
        let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(root).unwrap();
        Self {
            _cache: cache,
            source,
            store,
            root,
            generation,
            exclusions: Vec::new(),
            index_hidden: true,
            show_offline: false,
        }
    }
    fn add(
        &mut self,
        name: impl AsRef<Path>,
        text: &str,
        score: Option<f32>,
        duplicates: usize,
    ) -> Arc<Entry> {
        let path = self.source.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let entry = read_directory(path.parent().unwrap(), &AtomicBool::new(false))
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.path == path)
            .unwrap();
        let (id, _) = self
            .store
            .observe(
                self.root,
                self.generation,
                &entry,
                &std::fs::metadata(&path).unwrap(),
                "model",
            )
            .unwrap();
        let segments = score
            .map(|score| {
                let mut vector = vec![0.0; DIMENSION];
                vector[0] = score;
                vector[1] = (1.0 - score * score).sqrt();
                (0..duplicates)
                    .map(|i| Segment {
                        modality: "text".into(),
                        location: format!("passage {i}"),
                        text: text.into(),
                        input_hash: vec![1; 32],
                        vector: Some(vector.clone()),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.store
            .publish(id, entry.revision, "model", &segments, "complete")
            .unwrap();
        Arc::new(entry)
    }
    fn find(&self, description: &str, scope: &Path, selectors: &str, hidden: bool) -> Vec<String> {
        let (head, options) = selectors.split_once(';').unwrap_or((selectors, ""));
        let command = format!(":{head};emb:\"{description}\";{options}");
        let query = Query::parse(command.trim_end_matches(';')).unwrap();
        let mut vector = vec![0.0; DIMENSION];
        vector[0] = 1.0;
        Retrieval {
            progress: None,
            store: &self.store,
            pipeline: "model",
            vector: &vector,
            path: scope,
            query: &query,
            text: description,
            hidden,
            index_hidden: self.index_hidden,
            show_offline: self.show_offline,
            exclusions: &self.exclusions,
            // Synthetic scores test ranking/admission independently of
            // empirical model cutoffs. Default behavior is covered below.
            floors: &BTreeMap::from([("text".into(), 0.25)]),
            cancel: &Arc::new(AtomicBool::new(false)),
        }
        .search()
        .unwrap()
        .into_iter()
        .map(|hit| hit.entry.name)
        .collect()
    }
}
#[test]
fn coarse_names_count_distinct_paths_and_retire_the_last_owner() {
    let mut fixture = Fixture::new();
    let seed = fixture.add("same.txt", "ordinary words", Some(1.0), 1);
    let mut top = Top {
        by_name: Some(HashMap::new()),
        ..Top::default()
    };
    for index in 0..LIMIT {
        let mut entry = (*seed).clone();
        entry.path = fixture.source.path().join(format!("dir_{index}/same.txt"));
        top.offer(Ranked {
            entry: Arc::new(entry),
            tier: 3,
            score: 1.0,
            segments: Vec::new(),
        });
    }
    assert_eq!(top.by_name.as_ref().unwrap()["same.txt"], LIMIT);
    assert!(top.accepts_name("same.txt", -1.0));
    assert!(!top.accepts_name("z.txt", -1.0));
    let paths = top.by_path.keys().cloned().collect::<Vec<_>>();
    for (index, path) in paths.iter().enumerate() {
        top.remove(path).unwrap();
        assert_eq!(
            top.by_name
                .as_ref()
                .unwrap()
                .get("same.txt")
                .copied()
                .unwrap_or(0),
            LIMIT - index - 1
        );
    }
    assert!(top.by_name.as_ref().unwrap().is_empty());
    let mut ordinary = Top::default();
    ordinary.offer(Ranked {
        entry: seed,
        tier: 3,
        score: 1.0,
        segments: Vec::new(),
    });
    assert!(
        ordinary.by_name.is_none(),
        "semantic scoring must not allocate this lexical-only accelerator"
    );
}

#[test]
fn broad_lexical_hints_match_alias_fallback_order_and_every_retained_location() {
    let mut fixture = Fixture::new();
    for index in 0..520 {
        fixture.add(
            format!("record_{index}.txt"),
            "shared ordinary words",
            Some(0.9),
            4,
        );
    }
    let query = Query::parse(":file").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let floors = BTreeMap::new();
    let retrieval = Retrieval {
        store: &fixture.store,
        pipeline: "model",
        vector: &[],
        path: fixture.source.path(),
        query: &query,
        text: "shared",
        hidden: false,
        index_hidden: true,
        show_offline: false,
        exclusions: &[],
        floors: &floors,
        cancel: &cancel,
        progress: None,
    };
    let signature = |hits: Vec<Hit>| {
        hits.into_iter()
            .map(|hit| {
                (
                    hit.entry.path,
                    hit.segments
                        .into_iter()
                        .map(|segment| (segment.id, segment.score.to_bits(), segment.keyword))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = signature(retrieval.search().unwrap());
    assert_eq!(expected.len(), LIMIT);
    assert!(
        expected
            .iter()
            .all(|(_, segments)| segments.len() == explanation::MAX_PASSAGES)
    );
    assert_eq!(
        fixture
            .store
            .prepare_lexical_names(&cancel)
            .unwrap()
            .1
            .len(),
        2080
    );
    // All aliases are outside Here scope, but must disable singleton hints
    // for their file. This exercises the real SQL fallback without a test
    // switch or changing the candidate set/ranking.
    use std::os::unix::ffi::OsStrExt;
    let alias_parent = fixture.source.path().join("nested");
    let aliases: Vec<(i64, Vec<u8>)> = {
        let mut statement = fixture
            .store
            .connection
            .prepare("SELECT file_id,path FROM paths")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    for (id, bytes) in aliases {
        let name = Path::new(std::ffi::OsStr::from_bytes(&bytes))
            .file_name()
            .unwrap();
        let alias = alias_parent.join(name);
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO paths(path,parent,file_id) VALUES(?1,?2,?3)",
                rusqlite::params![
                    alias.as_os_str().as_encoded_bytes(),
                    alias_parent.as_os_str().as_encoded_bytes(),
                    id
                ],
            )
            .unwrap();
    }
    assert_eq!(signature(retrieval.search().unwrap()), expected);
    assert!(
        fixture
            .store
            .prepare_lexical_names(&cancel)
            .unwrap()
            .1
            .is_empty()
    );
}

#[test]
fn warm_vectors_follow_local_external_and_policy_changes_and_idle_release() {
    let mut fixture = Fixture::new();
    fixture.add("a.txt", "ordinary text", Some(0.9), 1);
    fixture.add("b.txt", "ordinary text", Some(0.8), 1);
    let path = fixture.source.path().to_owned();
    assert_eq!(
        fixture.find("abstract meaning", &path, "file", false),
        ["a.txt", "b.txt"]
    );
    assert_eq!(fixture.store.vector_cache_usage().0, 2);
    fixture.exclusions.push(path.join("a.txt"));
    assert_eq!(
        fixture.find("abstract meaning", &path, "file", false),
        ["b.txt"]
    );
    fixture.exclusions.clear();
    let mut low = vec![0.0; DIMENSION];
    low[1] = 1.0;
    let low = super::super::store::encode_vector(&low).unwrap();
    fixture
        .store
        .connection
        .execute(
            "UPDATE segments SET vector=?1 WHERE id=(SELECT min(id) FROM segments)",
            [&low],
        )
        .unwrap();
    assert_eq!(
        fixture.find("abstract meaning", &path, "file", false),
        ["b.txt"]
    );
    let external = rusqlite::Connection::open(fixture._cache.path().join("index.sqlite")).unwrap();
    external
        .execute("UPDATE segments SET vector=?1", [&low])
        .unwrap();
    assert!(
        fixture
            .find("abstract meaning", &path, "file", false)
            .is_empty()
    );
    assert_eq!(fixture.store.vector_cache_usage().0, 2);
    let epoch = fixture.store.prepare_vector_cache().unwrap();
    external
        .execute("UPDATE roots SET available=0", [])
        .unwrap();
    assert!(fixture.store.finish_vector_cache(epoch).is_err());
    assert_eq!(fixture.store.vector_cache_usage().0, 0);
    assert!(
        fixture
            .find("abstract meaning", &path, "file", false)
            .is_empty()
    );
    fixture
        .store
        .release_idle_vectors(std::time::Duration::ZERO);
    assert_eq!(fixture.store.vector_cache_usage(), (0, 0));
}

#[test]
fn warmed_cache_keeps_hard_filters_before_corrupt_vector_decoding() {
    let mut fixture = Fixture::new();
    fixture.add("a.txt", "ordinary text", Some(0.9), 1);
    let path = fixture.source.path().to_owned();
    assert_eq!(
        fixture.find("abstract meaning", &path, "file", false),
        ["a.txt"]
    );
    let nan = half::f16::NAN.to_le_bytes().repeat(DIMENSION);
    fixture
        .store
        .connection
        .execute("UPDATE segments SET vector=?1", [&nan])
        .unwrap();
    let filtered = Query::parse(r#":image;emb:"abstract meaning""#).unwrap();
    let unfiltered = Query::parse(r#":emb:"abstract meaning""#).unwrap();
    let mut vector = vec![0.0; DIMENSION];
    vector[0] = 1.0;
    let mut retrieval = Retrieval {
        progress: None,
        store: &fixture.store,
        pipeline: "model",
        vector: &vector,
        path: &path,
        query: &filtered,
        text: "abstract meaning",
        hidden: false,
        index_hidden: false,
        show_offline: false,
        exclusions: &[],
        floors: &BTreeMap::new(),
        cancel: &Arc::new(AtomicBool::new(false)),
    };
    assert!(retrieval.search().unwrap().is_empty());
    assert_eq!(fixture.store.vector_cache_usage().0, 0);
    retrieval.query = &unfiltered;
    assert!(retrieval.search().is_err());
    // NULL vector rows remain lexical-only, even if unrelated metadata is
    // malformed. The covering scan must not add semantic metadata reads.
    fixture
        .store
        .connection
        .execute_batch("UPDATE segments SET vector=NULL; UPDATE files SET metadata='invalid JSON';")
        .unwrap();
    retrieval.query = &filtered;
    assert!(retrieval.search().unwrap().is_empty());
}

#[test]
fn lexical_sql_threshold_preserves_ties_retained_paths_and_error_cleanup() {
    let mut fixture = Fixture::new();
    let seed = fixture.add("seed.txt", "ordinary words", Some(1.0), 1);
    let mut top = Top::default();
    for index in 1000..1500 {
        let mut entry = (*seed).clone();
        entry.name = format!("foo_{index}.txt");
        entry.path = fixture.source.path().join(&entry.name);
        top.offer(Ranked {
            entry: Arc::new(entry),
            tier: 3,
            score: 1.0,
            segments: Vec::new(),
        });
    }
    let connection = &fixture.store.connection;
    let accepts = |name: &str, score: f64| {
        let path = fixture.source.path().join(name);
        connection
            .query_row(
                "SELECT clip_lexical_might_rank(?1,?2)",
                rusqlite::params![path.as_os_str().as_encoded_bytes(), -score],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    };
    let result = (|| -> Result<(), String> {
        let paths = Arc::new(HashMap::from([
            (1, Box::<str>::from("foo_2.txt")),
            (2, Box::<str>::from("foo_2000.txt")),
            (3, Box::<str>::from("foo_1499.txt")),
        ]));
        let filter = LexicalRankFilter::with_paths(connection, &mut top, Some(paths))?;
        let segment_accepts = |id: i64, score: f64| {
            connection
                .query_row(
                    "SELECT clip_segment_might_rank(?1,?2)",
                    rusqlite::params![id, -score],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
        };
        assert!(segment_accepts(1, 1.0));
        assert!(!segment_accepts(2, 1.0));
        assert!(segment_accepts(3, 0.1));
        assert!(
            segment_accepts(999, 0.0),
            "uncached and aliased segments must fall through to ordinary SQL admission"
        );
        assert!(accepts("foo_2.txt", 1.0));
        assert!(!accepts("foo_2000.txt", 1.0));
        assert!(
            accepts("foo_1499.txt", 0.1),
            "retained paths still merge lower-scoring locations"
        );
        let mut earlier = (*seed).clone();
        earlier.name = "foo_2.txt".into();
        earlier.path = fixture.source.path().join(&earlier.name);
        filter.top.lock().unwrap().offer(Ranked {
            entry: Arc::new(earlier),
            tier: 3,
            score: 1.0,
            segments: Vec::new(),
        });
        assert!(
            !accepts("foo_1499.txt", 0.1),
            "evicted paths must obey the current boundary"
        );
        assert!(!segment_accepts(3, 0.1));
        connection
            .query_row(
                "SELECT clip_lexical_might_rank('invalid TEXT path',0)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    assert!(result.is_err());
    assert!(
        connection
            .query_row("SELECT clip_segment_might_rank(1,0)", [], |row| row
                .get::<_, bool>(0))
            .is_err()
    );
    assert_eq!(top.by_path.len(), LIMIT);
    assert!(
        top.by_path
            .contains_key(&fixture.source.path().join("foo_2.txt"))
    );
    assert!(
        connection
            .query_row("SELECT clip_lexical_might_rank(x'2f',0)", [], |row| row
                .get::<_, bool>(0))
            .is_err(),
        "the query-local callback must be removed on error"
    );
}

#[test]
fn default_cutoffs_reject_weak_scores_and_explicit_overrides_remain_available() {
    let mut fixture = Fixture::new();
    fixture.add("candidate.txt", "unrelated source words", Some(0.5), 1);
    let query = Query::parse(r#":emb:"abstract meaning""#).unwrap();
    let mut vector = vec![0.0; DIMENSION];
    vector[0] = 1.0;
    let defaults = BTreeMap::new();
    let mut retrieval = Retrieval {
        progress: None,
        store: &fixture.store,
        pipeline: "model",
        vector: &vector,
        path: fixture.source.path(),
        query: &query,
        text: "abstract meaning",
        hidden: false,
        index_hidden: false,
        show_offline: false,
        exclusions: &[],
        floors: &defaults,
        cancel: &Arc::new(AtomicBool::new(false)),
    };
    assert!(retrieval.search().unwrap().is_empty());
    let override_floors = BTreeMap::from([("text".into(), 0.4)]);
    retrieval.floors = &override_floors;
    assert_eq!(retrieval.search().unwrap()[0].entry.name, "candidate.txt");
}

#[test]
fn incidental_words_in_long_descriptions_do_not_crowd_out_semantic_matches() {
    let mut fixture = Fixture::new();
    for index in 0..24 {
        fixture.add(
            format!("noise-{index}.txt"),
            "A document with unrelated inventory notes.",
            Some(0.1),
            1,
        );
    }
    fixture.add("visual.png", "", Some(0.95), 1);
    assert_eq!(
        fixture.find(
            "a picture with three large blue circles",
            fixture.source.path(),
            "",
            false
        ),
        vec!["visual.png"]
    );
}

#[test]
fn partial_words_in_short_queries_do_not_crowd_out_semantic_matches() {
    let mut fixture = Fixture::new();
    for index in 0..24 {
        fixture.add(
            format!("noise-{index}.txt"),
            "A document with unrelated inventory notes.",
            Some(0.1),
            1,
        );
    }
    fixture.add("recording.wav", "", Some(0.95), 1);
    let names = fixture.find("A dog barking", fixture.source.path(), "", false);
    assert_eq!(names[0], "recording.wav");
    assert_eq!(names.len(), 25, "Partial keyword results remain available");
    // Single-letter and non-English tokens keep literal FTS behavior.
    fixture.add("language.txt", "C भाषा", Some(0.1), 1);
    assert_eq!(
        fixture.find("C भाषा", fixture.source.path(), "", false)[0],
        "language.txt"
    );
}

#[test]
fn short_keyword_queries_keep_the_broad_keyword_fallback() {
    let mut fixture = Fixture::new();
    fixture.add(
        "payment.txt",
        "Invoice for replacing panes of glass.",
        Some(0.1),
        1,
    );
    assert_eq!(
        fixture.find("window invoice", fixture.source.path(), "", false),
        vec!["payment.txt"]
    );
}

#[test]
fn sqlite_name_prefilter_keeps_unicode_and_escaped_unix_filenames() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    register_name_filter(&connection).unwrap();
    let cases: &[(&[u8], &str, bool)] = &[
        (b"/root/Report.txt", "report", true),
        ("/root/Résumé.pdf".as_bytes(), "résumé", true),
        (b"/root/take_\xFF.wav", r"\xff", true),
        (b"/root/take\\part.wav", r"take\\part", true),
        (b"/root/take\npart.wav", r"\x0a", true),
        (b"/root/report.txt", "unrelated", false),
    ];
    for (path, query, expected) in cases {
        let actual: bool = connection
            .query_row(
                "SELECT clip_filename_matches(?1,?2)",
                rusqlite::params![path, query],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actual, *expected, "path={path:?}, query={query:?}");
    }
}

#[test]
fn cancellation_interrupts_sql_without_a_result_row_and_releases_the_connection() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let started = std::time::Instant::now();
    let result = std::thread::scope(|scope| {
        let _guard = SqlCancellation::new(&connection, &cancel);
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            cancel.store(true, AtomicOrdering::Relaxed);
        });
        connection.query_row(
            "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n",
            [], |row| row.get::<_, i64>(0),
        )
    });
    assert!(
        matches!(result, Err(rusqlite::Error::SqliteFailure(error, _))
        if error.code == rusqlite::ErrorCode::OperationInterrupted)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    // Cancellation belongs to the finished read, never to later accepted writes.
    assert!(cancel.load(AtomicOrdering::Relaxed));
    connection
        .execute_batch("CREATE TABLE later(value); INSERT INTO later VALUES(1)")
        .unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM later", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn invalid_query_vectors_cannot_silently_change_semantic_ranking() {
    let fixture = Fixture::new();
    let query = Query::parse(r#":emb:"meaning""#).unwrap();
    let invalid = [
        vec![1.0],
        vec![0.0; DIMENSION],
        vec![f32::NAN; DIMENSION],
        vec![f32::INFINITY; DIMENSION],
    ];
    for vector in invalid {
        let result = Retrieval {
            progress: None,
            store: &fixture.store,
            pipeline: "model",
            vector: &vector,
            path: fixture.source.path(),
            query: &query,
            text: "meaning",
            hidden: false,
            index_hidden: false,
            show_offline: false,
            exclusions: &[],
            floors: &BTreeMap::new(),
            cancel: &Arc::new(AtomicBool::new(false)),
        }
        .search();
        assert!(
            result
                .err()
                .unwrap()
                .contains("finite nonzero 768-dimensional")
        );
    }
}

#[test]
fn cached_credentials_cannot_supply_lexical_or_semantic_results() {
    let mut fixture = Fixture::new();
    fixture.add("credentials.json", "violet lighthouse", Some(1.0), 1);
    fixture.add("secret.key", "violet lighthouse", Some(1.0), 1);
    fixture.add(".env", "violet lighthouse", Some(1.0), 1);
    fixture.add("ordinary.txt", "violet lighthouse", Some(0.8), 1);
    assert_eq!(
        fixture.find("violet", fixture.source.path(), "", true),
        vec!["ordinary.txt"]
    );
}

#[test]
fn sql_scope_keeps_component_boundaries_and_binary_path_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let mut fixture = Fixture::new();
    let directory = PathBuf::from(std::ffi::OsString::from_vec(b"group\xff".to_vec()));
    fixture.add(directory.join("a.txt"), "needle content", Some(0.9), 1);
    fixture.add(directory.join("deep/b.txt"), "needle content", Some(0.8), 1);
    fixture.add("group-neighbor/c.txt", "needle content", Some(0.7), 1);
    let scope = fixture.source.path().join(&directory);
    assert_eq!(fixture.find("needle", &scope, "", false), vec!["a.txt"]);
    assert_eq!(
        fixture.find("needle", &scope.join(""), ";scope:recursive", false),
        vec!["a.txt", "b.txt"]
    );
    assert_eq!(
        fixture.find("needle", Path::new("/"), ";scope:recursive", true),
        vec!["a.txt", "b.txt", "c.txt"]
    );
}

#[test]
fn live_and_offline_rows_cannot_cross_a_known_source_context() {
    let mut fixture = Fixture::new();
    fixture.add("note.txt", "violet lighthouse", Some(0.9), 1);
    fixture
        .store
        .connection
        .execute("UPDATE files SET scope='different-source'", [])
        .unwrap();
    assert!(
        fixture
            .find("violet", fixture.source.path(), "", false)
            .is_empty()
    );
    fixture
        .store
        .unavailable(fixture.source.path(), "disconnected")
        .unwrap();
    fixture.show_offline = true;
    assert!(
        fixture
            .find("violet", fixture.source.path(), "", false)
            .is_empty()
    );
}

#[test]
fn offline_results_require_opt_in_and_do_not_revive_excluded_or_disabled_roots() {
    let mut fixture = Fixture::new();
    let entry = fixture.add("old.txt", "violet lighthouse", Some(0.8), 1);
    fixture.add(".git/secret.txt", "violet lighthouse", Some(1.0), 1);
    fixture
        .store
        .unavailable(fixture.source.path(), "disconnected")
        .unwrap();
    assert!(
        fixture
            .find("violet", fixture.source.path(), ";scope:indexed", false)
            .is_empty()
    );
    fixture.show_offline = true;
    assert_eq!(
        fixture.find("violet", fixture.source.path(), ";scope:indexed", false),
        ["old.txt"]
    );
    assert!(
        fixture
            .store
            .last_known_paths(&[(*entry).clone()])
            .unwrap()
            .contains(&entry.path)
    );
    fixture.exclusions.push(entry.path.clone());
    assert!(
        fixture
            .find("violet", fixture.source.path(), ";scope:indexed", false)
            .is_empty()
    );
    fixture.exclusions.clear();
    fixture.store.disable_unconfigured(&[]).unwrap();
    assert!(
        fixture
            .find("violet", fixture.source.path(), ";scope:indexed", false)
            .is_empty()
    );
    assert!(
        fixture
            .store
            .last_known_paths(&[(*entry).clone()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_available_overlapping_membership_cannot_fall_back_to_stale_offline_evidence() {
    let mut fixture = Fixture::new();
    let entry = fixture.add("child/old.txt", "violet lighthouse", Some(0.8), 1);
    let child = fixture
        .store
        .enroll(&Root::open(&fixture.source.path().join("child")).unwrap())
        .unwrap();
    let generation = fixture.store.begin_scan(child).unwrap();
    fixture
        .store
        .observe(
            child,
            generation,
            &entry,
            &std::fs::metadata(&entry.path).unwrap(),
            "model",
        )
        .unwrap();
    fixture
        .store
        .unavailable(fixture.source.path(), "disconnected parent source")
        .unwrap();
    assert!(
        fixture
            .store
            .last_known_paths(&[(*entry).clone()])
            .unwrap()
            .is_empty()
    );
    fixture
        .store
        .unavailable(
            &fixture.source.path().join("child"),
            "disconnected child source",
        )
        .unwrap();
    assert_eq!(
        fixture
            .store
            .last_known_paths(&[(*entry).clone()])
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn semantic_progress_covers_all_admitted_references_and_partial_tail() {
    let mut fixture = Fixture::new();
    let a = fixture.add("a.txt", "ordinary content", Some(0.9), 1500);
    fixture.add("b.txt", "ordinary content", Some(0.9), 1500);
    let c = fixture.add("c.txt", "ordinary content", Some(0.9), 1500);
    for (source, name) in [
        (&a.path, "a-copy.txt"),
        (&c.path, "c-stale.txt"),
        (&c.path, ".c-hidden.txt"),
    ] {
        std::fs::hard_link(source, fixture.source.path().join(name)).unwrap();
        fixture.add(name, "ordinary content", Some(0.9), 1500);
    }
    let stale = fixture.source.path().join("c-stale.txt");
    fixture
        .store
        .connection
        .execute(
            "UPDATE memberships SET verified=0 WHERE path=?1",
            [stale.as_os_str().as_encoded_bytes()],
        )
        .unwrap();
    fixture.add("name-only.zip", "opaque inventory", None, 0);
    let query = Query::parse(":emb:\"abstract meaning\"").unwrap();
    let vector = std::iter::once(1.0)
        .chain(std::iter::repeat_n(0.0, DIMENSION - 1))
        .collect::<Vec<_>>();
    let progress = AtomicU64::new(0);
    let cancel = Arc::new(AtomicBool::new(false));
    let floors = BTreeMap::new();
    let mut retrieval = Retrieval {
        store: &fixture.store,
        pipeline: "model",
        vector: &vector,
        path: fixture.source.path(),
        query: &query,
        text: "abstract meaning",
        hidden: false,
        index_hidden: false,
        show_offline: false,
        exclusions: &[],
        floors: &floors,
        cancel: &cancel,
        progress: Some(&progress),
    };
    assert_eq!(retrieval.search().unwrap().len(), 4);
    // One verified alias adds references, not unique vectors. A stale or
    // hidden alias must not contribute references or leak shared metadata.
    assert_eq!(progress.load(AtomicOrdering::Relaxed), 6000);
    let indexed = Query::parse(":emb:\"abstract meaning\";scope:indexed").unwrap();
    retrieval.query = &indexed;
    assert_eq!(retrieval.search().unwrap().len(), 4);
    assert_eq!(progress.load(AtomicOrdering::Relaxed), 6000);
    retrieval.query = &query;
    // SQL scope filtering precedes the counter; neither the result window
    // nor a prior cache hit ends scanning early.
    let here = fixture.source.path().join("empty");
    retrieval.path = &here;
    assert!(retrieval.search().unwrap().is_empty());
    assert_eq!(progress.load(AtomicOrdering::Relaxed), 0);
    retrieval.path = fixture.source.path();
    cancel.store(true, AtomicOrdering::Relaxed);
    assert!(retrieval.search().is_err());
    assert_eq!(progress.load(AtomicOrdering::Relaxed), 0);
}

#[test]
fn lexical_search_needs_no_model_but_rejects_obsolete_extraction_records() {
    let mut fixture = Fixture::new();
    fixture.add("current.txt", "violet lighthouse", Some(0.0), 1);
    fixture.add("old.txt", "violet lighthouse", Some(1.0), 1);
    fixture.store.connection.execute("UPDATE files SET extraction_version=0 WHERE id=(SELECT file_id FROM paths WHERE path=?1)", [fixture.source.path().join("old.txt").as_os_str().as_encoded_bytes()]).unwrap();
    let query = Query::parse(":emb:\"violet lighthouse\"").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let floors = BTreeMap::new();
    let mut retrieval = Retrieval {
        progress: None,
        store: &fixture.store,
        pipeline: "",
        vector: &[],
        path: fixture.source.path(),
        query: &query,
        text: "violet lighthouse",
        hidden: false,
        index_hidden: true,
        show_offline: false,
        exclusions: &[],
        floors: &floors,
        cancel: &cancel,
    };
    let hits = retrieval.search().unwrap();
    assert_eq!(
        hits.iter()
            .map(|hit| hit.entry.name.as_str())
            .collect::<Vec<_>>(),
        ["current.txt"]
    );
    assert!(hits[0].segments.iter().all(|segment| segment.keyword));
    fixture
        .store
        .connection
        .execute("UPDATE memberships SET verified=0", [])
        .unwrap();
    assert_eq!(
        retrieval.search().unwrap().len(),
        1,
        "checking inventory can supply lexical candidates for fresh source verification"
    );
    let mut vector = vec![0.0; DIMENSION];
    vector[0] = 1.0;
    retrieval.vector = &vector;
    assert!(retrieval.search().is_err());
    retrieval.pipeline = "model";
    assert!(
        retrieval.search().unwrap().is_empty(),
        "unverified inventory must not contribute semantic scoring"
    );
}

#[test]
fn batched_keyword_evidence_preserves_fts_prefix_and_unicode_snippets() {
    let mut fixture = Fixture::new();
    let entry = fixture.add(
        "evidence.txt",
        &format!(
            "{}Violet lighthouse café guides boats.{}",
            "context ".repeat(100),
            " tail".repeat(100)
        ),
        Some(0.9),
        1,
    );
    let id: i64 = fixture
        .store
        .connection
        .query_row("SELECT id FROM segments LIMIT 1", [], |row| row.get(0))
        .unwrap();
    let hits = vec![Hit {
        entry: (*entry).clone(),
        segments: vec![SegmentMatch {
            id,
            score: 0.9,
            keyword: true,
        }],
    }];
    let cancel = Arc::new(AtomicBool::new(false));
    for text in [
        "violet",
        "CAFÉ",
        "cafe",
        "lighthou",
        "absent",
        "violet lighthouse",
    ] {
        let expression = terms(text).unwrap().join(" OR ");
        let expected: Option<String> = fixture.store.connection.query_row(
            "SELECT snippet(segment_fts,0,'','',' … ',32) FROM segment_fts WHERE rowid=?1 AND segment_fts MATCH ?2",
            rusqlite::params![id, expression], |row| row.get(0),
        ).optional().unwrap();
        let details = explanation::read(
            &fixture.store,
            &hits,
            &[(*entry).clone()],
            "model",
            text,
            &cancel,
        )
        .unwrap();
        let expected = expected.map(|value| {
            explanation::clipped(&value.split_whitespace().collect::<Vec<_>>().join(" "), 320)
        });
        assert_eq!(
            details[&entry.path]
                .passages
                .first()
                .map(|passage| &passage.text),
            expected.as_ref(),
            "{text}"
        );
    }
}

#[test]
fn explanations_read_only_verified_files_and_their_own_matching_segments() {
    let mut fixture = Fixture::new();
    let text = format!(
        "{}Violet lighthouse guides boats.{}",
        "ordinary context ".repeat(100),
        " trailing".repeat(100)
    );
    let first = fixture.add("one.txt", &text, Some(0.9), 5);
    let other = fixture.add("two.txt", "Unrelated private passage", Some(0.8), 1);
    let ids = fixture.store.connection.prepare("SELECT s.id FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE p.path=?1 ORDER BY s.id")
        .unwrap().query_map([first.path.as_os_str().as_encoded_bytes()], |row| row.get::<_, i64>(0)).unwrap()
        .collect::<Result<Vec<_>, _>>().unwrap();
    let mut hits = vec![
        Hit {
            entry: (*first).clone(),
            segments: ids
                .iter()
                .map(|id| SegmentMatch {
                    id: *id,
                    score: 0.9,
                    keyword: true,
                })
                .collect(),
        },
        Hit {
            entry: (*other).clone(),
            segments: vec![SegmentMatch {
                id: ids[0],
                score: 0.8,
                keyword: false,
            }],
        },
    ];
    let cancel = Arc::new(AtomicBool::new(false));
    let details = explanation::read(
        &fixture.store,
        &hits,
        &[(*first).clone()],
        "model",
        "violet lighthouse",
        &cancel,
    )
    .unwrap();
    assert_eq!(details.len(), 1);
    let detail = &details[&first.path];
    assert_eq!(detail.coverage, "complete");
    assert_eq!(detail.passages.len(), explanation::MAX_PASSAGES);
    assert_eq!(detail.passages[0].location, "passage 0");
    assert_eq!(detail.passages[0].kind, "Keyword match");
    assert!(detail.passages[0].text.contains("Violet lighthouse"));
    assert!(detail.passages[0].text.chars().count() <= 321);
    for keyword in [false, true] {
        hits[1].segments[0].keyword = keyword;
        let details = explanation::read(
            &fixture.store,
            &hits,
            &[(*other).clone()],
            "model",
            "violet",
            &cancel,
        )
        .unwrap();
        assert!(
            details[&other.path].passages.is_empty(),
            "a foreign segment must not be displayed (keyword={keyword})"
        );
    }
    let details = explanation::read(
        &fixture.store,
        &hits,
        &[(*first).clone()],
        "another model",
        "violet",
        &cancel,
    )
    .unwrap();
    assert!(details[&first.path].passages.is_empty());
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        explanation::read(
            &fixture.store,
            &hits,
            &[(*first).clone()],
            "model",
            "violet",
            &cancel
        )
        .is_err()
    );
}

#[test]
fn changed_exclusions_filter_cached_rows_before_candidate_caps() {
    let mut fixture = Fixture::new();
    for number in 0..501 {
        fixture.add(
            format!("ignore/entry_{number}.txt"),
            "old cache",
            Some(1.0),
            2,
        );
    }
    fixture.add("public.txt", "ordinary content", Some(0.8), 2);
    // Populate the vector cache before changing the query's policy. Both
    // accepted and excluded paths have adjacent multiple segment rows.
    let before = fixture.find("meaning", fixture.source.path(), ";scope:recursive", true);
    assert_eq!(before.len(), LIMIT);
    assert!(!before.iter().any(|name| name == "public.txt"));
    fixture
        .exclusions
        .push(fixture.source.path().join("ignore"));
    assert_eq!(
        fixture.find("meaning", fixture.source.path(), ";scope:recursive", true),
        ["public.txt"]
    );
    fixture
        .exclusions
        .push(fixture.source.path().join("public.txt"));
    assert!(
        fixture
            .find("public.txt", fixture.source.path(), ";scope:indexed", true)
            .is_empty()
    );
}

#[test]
fn hidden_index_policy_filters_cached_memberships_even_when_browser_shows_hidden() {
    let mut fixture = Fixture::new();
    fixture.add(".private/needle.txt", "needle", Some(1.0), 1);
    assert_eq!(
        fixture.find("needle", fixture.source.path(), ";scope:indexed", true),
        ["needle.txt"]
    );
    fixture.index_hidden = false;
    for scope in [";scope:indexed", ";scope:recursive"] {
        assert!(
            fixture
                .find("needle", fixture.source.path(), scope, true)
                .is_empty()
        );
    }
}

#[test]
fn recursive_and_indexed_visibility_excludes_hidden_ancestors() {
    let mut fixture = Fixture::new();
    fixture.add("visible.txt", "needle", Some(1.0), 1);
    fixture.add(".archive/inside.txt", "needle", Some(1.0), 1);
    let root = fixture.source.path();
    assert_eq!(
        fixture.find("needle", root, ";scope:recursive", false),
        ["visible.txt"]
    );
    assert_eq!(
        fixture.find("needle", root, ";scope:indexed", false),
        ["visible.txt"]
    );
    assert_eq!(
        fixture.find("needle", root, ";scope:recursive", true).len(),
        2
    );
    assert_eq!(
        fixture.find("needle", root, ";scope:indexed", true).len(),
        2
    );
    assert_eq!(
        fixture.find("needle", &root.join(".archive"), ";scope:here", false),
        ["inside.txt"]
    );
}

#[test]
fn hidden_root_visibility_requires_its_verified_membership_and_survives_parent_retirement() {
    let mut fixture = Fixture::new();
    let entry = fixture.add(".archive/inside.txt", "needle", Some(1.0), 1);
    let hidden = fixture.source.path().join(".archive");
    fixture
        .store
        .directory_complete(fixture.root, fixture.generation, &hidden)
        .unwrap();
    let root = Root::open(&hidden).unwrap();
    let nested = fixture.store.enroll(&root).unwrap();
    let generation = fixture.store.begin_scan(nested).unwrap();
    assert!(
        fixture
            .find("needle", fixture.source.path(), ";scope:indexed", false)
            .is_empty()
    );
    // Availability of a containing root alone cannot expose another root's
    // hidden inventory. The path needs that root's actual verified membership.
    fixture
        .store
        .observe(
            nested,
            generation,
            &entry,
            &std::fs::metadata(&entry.path).unwrap(),
            "model",
        )
        .unwrap();
    assert_eq!(
        fixture.find("needle", fixture.source.path(), ";scope:indexed", false),
        ["inside.txt"]
    );
    fixture.store.begin_scan(fixture.root).unwrap();
    fixture.store.retire_path(fixture.root, &hidden).unwrap();
    fixture.store.finish_scan(fixture.root, None).unwrap();
    let parent = fixture
        .store
        .roots()
        .unwrap()
        .into_iter()
        .find(|r| r.id == fixture.root)
        .unwrap();
    assert_eq!(parent.state, "ready");
    assert_eq!(parent.files, 0);
    assert_eq!(
        fixture.find("needle", fixture.source.path(), ";scope:indexed", false),
        ["inside.txt"]
    );
    assert!(
        fixture
            .find("needle", fixture.source.path(), ";scope:recursive", false)
            .is_empty()
    );
    assert_eq!(std::fs::read(&entry.path).unwrap(), b"needle");
    fixture.store.begin_scan(nested).unwrap();
    assert!(
        fixture
            .find("needle", fixture.source.path(), ";scope:indexed", false)
            .is_empty()
    );
}

#[test]
fn unconfigured_cached_roots_cannot_supply_matches_or_scope_after_reopen_or_clear() {
    let mut fixture = Fixture::new();
    fixture.add("one.txt", "needle", Some(1.0), 1);
    assert_eq!(
        fixture.find("needle", fixture.source.path(), "", false),
        ["one.txt"]
    );
    fixture.store.disable_unconfigured(&[]).unwrap();
    assert!(
        fixture
            .find("needle", fixture.source.path(), "", false)
            .is_empty()
    );
    assert!(fixture.store.roots().unwrap().is_empty());
    assert!(fixture.store.source_roots().unwrap().is_empty());
    let count: u64 = fixture
        .store
        .connection
        .query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 1,
        "Removing configuration must retain generated work without authorizing it"
    );
    fixture
        .store
        .unavailable(fixture.source.path(), "still unavailable")
        .unwrap();
    assert!(fixture.store.roots().unwrap().is_empty());
    fixture
        .store
        .disable_unconfigured(&[fixture.source.path().into()])
        .unwrap();
    assert_eq!(fixture.store.roots().unwrap().len(), 1);
    assert!(
        fixture.store.source_roots().unwrap().is_empty(),
        "Configured intent must still pass fresh source admission"
    );
    fixture.store.disable_unconfigured(&[]).unwrap();
    fixture.store.clear_generated().unwrap();
    assert!(
        fixture.store.roots().unwrap().is_empty(),
        "Clear must not reactivate cached enrollments"
    );
    assert!(fixture.store.source_roots().unwrap().is_empty());
}

#[test]
fn exact_filenames_outrank_meaning_even_for_name_only_files() {
    let mut fixture = Fixture::new();
    fixture.add("report.rs", "code is name-only", None, 0);
    fixture.add("REPORT.RS", "other code", None, 0);
    fixture.add("word.txt", "report rs", Some(1.0), 1);
    assert_eq!(
        fixture.find("report.rs", fixture.source.path(), "", false),
        ["report.rs", "REPORT.RS", "word.txt"]
    );
}
#[test]
fn no_summing_of_segments_and_low_semantic_scores_do_not_make_results() {
    let mut fixture = Fixture::new();
    fixture.add("short.txt", "unrelated literal text", Some(0.8), 1);
    fixture.add("long.txt", "unrelated literal text", Some(0.6), 40);
    fixture.add("weak.txt", "unrelated literal text", Some(0.1), 1);
    assert_eq!(
        fixture.find("different", fixture.source.path(), "", false),
        ["short.txt", "long.txt"]
    );
}
#[test]
fn lexical_or_fallback_is_scoped_and_hidden_and_stale_sources_are_excluded() {
    let mut fixture = Fixture::new();
    fixture.add("outside.txt", "red missing", Some(0.0), 1);
    fixture.add("inner/red.txt", "red", Some(0.0), 1);
    fixture.add("inner/.red.txt", "red missing", Some(1.0), 1);
    let scope = fixture.source.path().join("inner");
    assert_eq!(fixture.find("red missing", &scope, "", false), ["red.txt"]);
    assert_eq!(fixture.find("red missing", &scope, "", true), [".red.txt"]);
    fixture
        .store
        .invalidate_directory(fixture.root, Some(&scope))
        .unwrap();
    assert!(fixture.find("red missing", &scope, "", false).is_empty());
}
#[test]
fn hard_link_visibility_comes_from_each_path_not_shared_object_metadata() {
    let mut fixture = Fixture::new();
    fixture.add("visible.txt", "needle", Some(1.0), 1);
    std::fs::hard_link(
        fixture.source.path().join("visible.txt"),
        fixture.source.path().join(".hidden.txt"),
    )
    .unwrap();
    fixture.add(".hidden.txt", "needle", Some(1.0), 1);
    assert_eq!(
        fixture.find("needle", fixture.source.path(), "", false),
        ["visible.txt"]
    );
    assert_eq!(
        fixture
            .find("needle", fixture.source.path(), "", true)
            .len(),
        2
    );
}

#[test]
fn hard_filters_run_before_each_component_limit() {
    let mut fixture = Fixture::new();
    for i in 0..=LIMIT {
        fixture.add(format!("tiny{i}.txt"), "needle", Some(1.0), 1);
    }
    fixture.add("large.txt", &"needle ".repeat(200), Some(0.4), 1);
    assert_eq!(
        fixture.find("needle", fixture.source.path(), ">=1KB", false),
        ["large.txt"]
    );
}

#[test]
fn keyword_window_preserves_natural_ties_after_exclusions_and_unverified_memberships() {
    let mut fixture = Fixture::new();
    // Reverse insertion order and repeated passages must not determine
    // ties, displace eligible files, or reward files with more segments.
    for index in (0..LIMIT + 32).rev() {
        fixture.add(
            format!("item{index}.txt"),
            "needle",
            Some(0.0),
            1 + index % 3,
        );
    }
    for index in 0..10 {
        fixture
            .exclusions
            .push(fixture.source.path().join(format!("item{index}.txt")));
    }
    for index in 10..20 {
        let path = fixture.source.path().join(format!("item{index}.txt"));
        fixture
            .store
            .connection
            .execute(
                "UPDATE memberships SET verified=0 WHERE path=?1",
                [path.as_os_str().as_encoded_bytes()],
            )
            .unwrap();
    }
    let expected = (20..20 + LIMIT)
        .map(|index| format!("item{index}.txt"))
        .collect::<Vec<_>>();
    assert_eq!(
        fixture.find("needle", fixture.source.path(), "", false),
        expected
    );
}
#[test]
fn combining_marks_and_literals_use_safe_fts_tokens() {
    let mut fixture = Fixture::new();
    fixture.add("words.txt", "café किताब", Some(0.0), 1);
    assert_eq!(
        fixture.find("cafe", fixture.source.path(), "", false),
        ["words.txt"]
    );
    assert_eq!(
        fixture.find("किताब", fixture.source.path(), "", false),
        ["words.txt"]
    );
    assert_eq!(
        terms("foo\" OR text:bar*").unwrap(),
        ["\"foo\"", "\"OR\"", "\"text\"", "\"bar\"*"]
    );
}
#[test]
fn component_storage_stays_bounded_when_files_gain_better_segments() {
    let mut fixture = Fixture::new();
    let base = fixture.add("base.txt", "text", None, 0);
    let mut top = Top::default();
    for i in 0..=LIMIT {
        let mut entry = (*base).clone();
        entry.name = format!("file{i}.txt");
        entry.path = fixture.source.path().join(&entry.name);
        top.offer(Ranked {
            entry: Arc::new(entry),
            tier: 0,
            score: i as f64,
            segments: Vec::new(),
        });
    }
    assert_eq!(top.by_path.len(), LIMIT);
    assert_eq!(top.ordered.len(), LIMIT);
    let mut improved = top.best().last().unwrap().clone();
    improved.score = 10000.0;
    top.offer(improved.clone());
    assert_eq!(top.best().next().unwrap().entry.path, improved.entry.path);
    assert_eq!(top.by_path.len(), LIMIT);
    assert_eq!(top.ordered.len(), LIMIT);
}

#[test]
fn modality_count_cannot_overpower_the_lexical_weight() {
    let mut fixture = Fixture::new();
    let keyword = fixture.add("keyword.txt", "text", None, 0);
    let meaning = fixture.add("meaning.txt", "text", None, 0);
    let mut lexical = Top::default();
    lexical.offer(Ranked {
        entry: keyword,
        tier: 3,
        score: 1.0,
        segments: Vec::new(),
    });
    let mut modalities = BTreeMap::new();
    for name in ["text", "image", "audio"] {
        let mut list = Top::default();
        list.offer(Ranked {
            entry: meaning.clone(),
            tier: 0,
            score: 1.0,
            segments: Vec::new(),
        });
        modalities.insert(name.into(), list);
    }
    let names = fuse(&lexical, &modalities, "different", false)
        .into_iter()
        .map(|hit| hit.entry.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["keyword.txt", "meaning.txt"]);
    // A partial match must remain weaker even when only one of several
    // modalities supports the semantic candidate.
    modalities.get_mut("text").unwrap().by_path.clear();
    modalities.get_mut("text").unwrap().ordered.clear();
    modalities.get_mut("image").unwrap().by_path.clear();
    modalities.get_mut("image").unwrap().ordered.clear();
    let names = fuse(&lexical, &modalities, "different", true)
        .into_iter()
        .map(|hit| hit.entry.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["meaning.txt", "keyword.txt"]);
}
