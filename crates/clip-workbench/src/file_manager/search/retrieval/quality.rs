//! Explicit fixture evaluation using production storage, filters and rank fusion.
use super::*;
use crate::file_manager::search::{policy::Root, store::Segment};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Digest;

const PIPELINE: &str = "frozen-quality-v1";

#[derive(Deserialize)]
struct PreparedSegment {
    file: String,
    modality: String,
    location: String,
    text: String,
    f32: Vec<f32>,
}
#[derive(Deserialize)]
struct Inputs {
    segments: Vec<PreparedSegment>,
    queries: HashMap<String, Vec<f32>>,
}
struct Corpus {
    _cache: tempfile::TempDir,
    source: PathBuf,
    store: Store,
    inputs: Inputs,
}
impl Corpus {
    fn load(folder: &Path) -> Self {
        let source = folder.join("fixture");
        let origin = Root::open(&source).unwrap();
        let inputs: Inputs =
            serde_json::from_value(read_json(&folder.join("vectors.json"))).unwrap();
        let cache = tempfile::tempdir_in(folder).unwrap();
        let mut store = Store::open(cache.path()).unwrap();
        let root = store.enroll(&origin).unwrap();
        let generation = store.begin_scan(root).unwrap();
        let directory = filesystem::read_directory(&source, &AtomicBool::new(false)).unwrap();
        assert_eq!(directory.entries.len(), 60);
        for entry in directory.entries {
            let (id, _) = store
                .observe(
                    root,
                    generation,
                    &entry,
                    &std::fs::metadata(&entry.path).unwrap(),
                    PIPELINE,
                )
                .unwrap();
            let segments = inputs
                .segments
                .iter()
                .filter(|segment| segment.file == entry.name)
                .map(|segment| {
                    let bytes = segment
                        .f32
                        .iter()
                        .flat_map(|value| value.to_le_bytes())
                        .collect::<Vec<_>>();
                    Segment {
                        modality: segment.modality.clone(),
                        location: segment.location.clone(),
                        text: segment.text.clone(),
                        input_hash: sha2::Sha256::digest(&bytes).to_vec(),
                        vector: Some(segment.f32.clone()),
                    }
                })
                .collect::<Vec<_>>();
            assert!(!segments.is_empty(), "missing prepared file {}", entry.name);
            store
                .publish(
                    id,
                    entry.revision,
                    PIPELINE,
                    &segments,
                    "quality-fixture prepared coverage",
                )
                .unwrap();
        }
        store.finish_scan(root, None).unwrap();
        origin.revalidate().unwrap();
        Self {
            _cache: cache,
            source,
            store,
            inputs,
        }
    }

    fn install_f32_reference(&self) {
        let references = self.inputs.segments.iter().map(|segment| {
            let path = self.source.join(&segment.file);
            let (id, count): (i64, usize) = self.store.connection.query_row(
                "SELECT min(s.id),count(*) FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE p.path=?1 AND s.modality=?2 AND s.location=?3",
                rusqlite::params![path.as_os_str().as_encoded_bytes(), segment.modality, segment.location],
                |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
            assert_eq!(count, 1, "reference segment identity must be unambiguous");
            (id, segment.f32.clone())
        }).collect::<Vec<_>>();
        let stored: usize = self
            .store
            .connection
            .query_row("SELECT count(*) FROM segments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            stored,
            references.len(),
            "every segment needs a float32 reference"
        );
        self.store.install_f32_reference(&references).unwrap();
        assert_eq!(self.store.vector_cache_usage().0, references.len());
    }

    fn rank(&self, case: &Value, floors: &BTreeMap<String, f32>) -> Value {
        let text = case["query"].as_str().unwrap();
        let query = Query::parse(&format!(
            ":emb:{};scope:recursive",
            serde_json::to_string(text).unwrap()
        ))
        .unwrap();
        let id = case["id"]
            .as_str()
            .expect("quality query needs a stable identifier");
        let vector = self
            .inputs
            .queries
            .get(id)
            .expect("quality query has no real model embedding");
        let hits = Retrieval {
            progress: None,
            store: &self.store,
            pipeline: PIPELINE,
            vector,
            path: &self.source,
            query: &query,
            text,
            hidden: false,
            index_hidden: false,
            show_offline: false,
            exclusions: &[],
            floors,
            cancel: &Arc::new(AtomicBool::new(false)),
        }
        .search()
        .unwrap();
        let expected = case["file"].as_str();
        let rank = expected
            .and_then(|name| hits.iter().position(|hit| hit.entry.name == name))
            .map(|rank| rank + 1);
        let semantic_only = hits.iter().any(|hit| {
            hit.segments.iter().any(|segment| !segment.keyword)
                && !hit.segments.iter().any(|segment| segment.keyword)
                && crate::file_manager::type_navigation::name_rank(
                    &hit.entry.name,
                    &text.to_lowercase(),
                )
                .is_none()
        });
        json!({"id":case["id"],"kind":case["kind"],"split":case["split"],"query":text,
            "expected_file":expected,"rank":rank,"has_results":!hits.is_empty(),"semantic_only_result":semantic_only,
            "top":hits.iter().take(10).map(|hit| json!({"file":hit.entry.name,"segments":hit.segments.iter().map(|segment| json!({"id":segment.id,"score":segment.score,"keyword":segment.keyword})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
    }
}
fn read_json(path: &Path) -> Value {
    let bytes = crate::file_source::read_file_bytes(path, 32 * 1024 * 1024, true, false).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
#[ignore = "explicit frozen real-model quality evaluation; requires owned generated fixtures"]
fn production_quality_fixture() {
    let folder = PathBuf::from(std::env::var_os("CLIP_SEARCH_QUALITY_FOLDER").unwrap());
    let report_path = PathBuf::from(std::env::var_os("CLIP_SEARCH_QUALITY_REPORT").unwrap());
    let manifest = read_json(&folder.join("fixture-manifest.json"));
    let component = read_json(&folder.join("report.json"));
    let validation = read_json(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/search_model/retrieval_validation.json"),
    );
    let corpus = Corpus::load(&folder);
    let mut cases = manifest["positives"].as_array().unwrap().clone();
    cases.extend(manifest["negatives"].as_array().unwrap().iter().cloned());
    for case in validation["positives"].as_array().unwrap() {
        let target = manifest["positives"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["id"] == case["target"])
            .unwrap();
        let mut case = case.clone();
        case["file"] = target["file"].clone();
        case["kind"] = target["kind"].clone();
        cases.push(case);
    }
    cases.extend(validation["negatives"].as_array().unwrap().iter().cloned());
    let mut results = Vec::new();
    for (name, floors) in [
        ("baseline", json!({"text":0.25,"image":0.25,"audio":0.25})),
        ("defaults", json!({})),
        ("candidate", component["candidate_floors"].clone()),
    ] {
        let floors: BTreeMap<String, f32> = serde_json::from_value(floors).unwrap();
        results.push(json!({"name":name,"floors":floors,"cases":cases.iter().map(|case| corpus.rank(case, &floors)).collect::<Vec<_>>()}));
    }
    let exact_cases = manifest["exact_queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let mut case = case.clone();
            case["file"] = case["query"].clone();
            case
        })
        .collect::<Vec<_>>();
    let exact = exact_cases
        .iter()
        .map(|case| corpus.rank(case, &BTreeMap::new()))
        .collect::<Vec<_>>();
    assert!(
        exact.iter().all(|case| case["rank"] == 1),
        "exact filename tier failed"
    );
    corpus.install_f32_reference();
    let reference = json!({
        "method":"original model outputs with f32 encode/decode normalization, seeded via test-only cache; same SQL admission, Rust f32 accumulation, cutoffs and fusion",
        "cases":cases.iter().map(|case|corpus.rank(case,&BTreeMap::new())).collect::<Vec<_>>(),
        "exact":exact_cases.iter().map(|case|corpus.rank(case,&BTreeMap::new())).collect::<Vec<_>>()
    });
    let report = json!({"scope":"production SQLite/f16 scoring/lexical/RRF component over frozen real-model inputs; excludes coordinator/UI freshness and BF16 reference","files":60,"segments":corpus.inputs.segments.len(),"model_pipeline":component["pipeline"],"results":results,"exact":exact,"float32_reference":reference,"fixture_sha256":component["fixture_sha256"],"full_phase6_acceptance_proven":false});
    std::fs::write(report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
