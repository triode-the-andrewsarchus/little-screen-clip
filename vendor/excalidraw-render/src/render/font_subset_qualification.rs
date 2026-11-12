use super::{font_subset, svg_fonts};
use crate::resource_accounting::lower_hex;
#[cfg(not(target_arch = "wasm32"))]
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use write_fonts::read::tables::cmap::CmapIterLimits;
use write_fonts::read::{types::GlyphId, FontRef, TableProvider};

#[cfg(not(target_arch = "wasm32"))]
const CORPUS_PATH: &str = concat!(
    "tests/oracle/compatibility-artifacts/",
    "revision-1-svg-font-subset-qualification-corpus.json"
);
#[cfg(not(target_arch = "wasm32"))]
const PACKAGED_CORPUS_PATH: &str =
    "crates/excalidraw-render/tests/qualification/font-subset-corpus.json";
const CORPUS_SCHEMA: &str = "excalidraw-render/svg-font-subset-qualification-corpus/v1";
/// The revision's asset manifest digest, as its sidecar names it.
const SOURCE_MANIFEST_SIDECAR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/oracle/reference-profiles/revision-1-assets.sha256"
));
const PROFILE_SHA256: &str = "0b6ec496a92a1aa2d0fdcca9ab0ca57c7ff5f18300473da8e46755d67572fca4";
/// Per-cell trace digests are not pinned: cross-target equality of the
/// aggregate `resourceTracesSha256` is the determinism property (ADR-0002).
/// Building with `EXCALIDRAW_FONT_SUBSET_TRACE_DIGESTS` set dumps one digest
/// per traced cell so an aggregate mismatch can be bisected on any target,
/// including `wasm32-unknown-unknown`, where run-time environment variables are
/// unavailable.
const TRACE_DIGEST_DIAGNOSTICS: bool =
    option_env!("EXCALIDRAW_FONT_SUBSET_TRACE_DIGESTS").is_some();
const CLASS_IDS: [&str; 9] = [
    "mapped-scalar",
    "range-boundary",
    "empty-intersection",
    "full-intersection",
    "non-bmp",
    "combining-mark",
    "composite-closure",
    "layout-closure-heavy",
    "default-ignorable-no-space",
];
/// The qualified sources: the 230 official chunks in catalog order, then the
/// Noto Emoji fallback face (#150), which direct SVG subsets like a chunk.
const SOURCE_COUNT: usize = 231;

/// One source the corpus qualifies, as the subsetter sees it plus the catalog
/// facts the corpus records beside it.
#[derive(Clone, Copy)]
struct QualificationSource {
    source: svg_fonts::SubsetSource,
    upstream_path: &'static str,
    /// The catalog family: the notice table's key and the qualifier's family
    /// for the Xiaolai-specific empty-intersection candidates.
    family: &'static str,
    license_identity: &'static str,
    reserved_font_names: &'static [&'static str],
}

impl QualificationSource {
    fn chunk(record: &'static crate::fonts::inspection::AssetRecord) -> Self {
        Self {
            source: svg_fonts::SubsetSource::chunk(record),
            upstream_path: record.upstream_path,
            family: record.family,
            license_identity: record.license_identity,
            reserved_font_names: record.reserved_font_names,
        }
    }

    /// The Noto Emoji fallback: an sfnt rather than a WOFF2 chunk, its own
    /// one-face family declaring all of Unicode, OFL without a Reserved Font
    /// Name, so its alias is its canonical name.
    fn noto_emoji() -> Self {
        Self {
            source: svg_fonts::SubsetSource::noto_emoji(),
            upstream_path: "scripts/woff2/assets/NotoEmoji-Regular.ttf",
            family: crate::fonts::inspection::NOTO_EMOJI_FAMILY,
            license_identity: "font.noto-emoji.ofl-1.1",
            reserved_font_names: &[],
        }
    }

    fn identity(self) -> &'static str {
        self.source.identity()
    }

    fn alias(self) -> &'static str {
        effective_alias(self.family)
    }

    /// The resolved family name the demand and the trace scene name.
    fn family_name(self) -> &'static str {
        family_name(self.family)
    }

    /// The corpus spelling of the source container.
    fn container_name(self) -> &'static str {
        match self.source.container() {
            font_subset::SourceContainer::Woff2 => "woff2",
            font_subset::SourceContainer::Sfnt => "sfnt",
        }
    }

    fn ranges(self) -> &'static [(u32, u32)] {
        self.source
            .unicode_ranges()
            .expect("qualified source has declared ranges")
    }

    fn load(self) -> crate::fonts::inspection::VerifiedAsset {
        self.source
            .load()
            .expect("qualified source verifies")
            .expect("qualified source exists")
    }

    /// The sfnt the subsetter plans over: the chunk's decode, the fallback
    /// itself.
    fn decode(self, bytes: &[u8]) -> Vec<u8> {
        match self.source.container() {
            font_subset::SourceContainer::Woff2 => {
                wuff::decompress_woff2(bytes).expect("qualified source decodes")
            }
            font_subset::SourceContainer::Sfnt => bytes.to_vec(),
        }
    }

    /// The WOFF2 the canonical viewers, which admit only that container, load
    /// as the source face: the chunk's own bytes, or the fallback sfnt encoded
    /// by the subsetter's encoder. The corpus pins the digest of the encoding
    /// beside the sfnt's, so the viewer manifest's source role stays bound to
    /// the manifested face.
    fn viewer_source_woff2(self, bytes: &[u8], sfnt: &[u8]) -> Vec<u8> {
        match self.source.container() {
            font_subset::SourceContainer::Woff2 => bytes.to_vec(),
            font_subset::SourceContainer::Sfnt => {
                font_subset::encode_woff2(sfnt).expect("qualified fallback source encodes")
            }
        }
    }

    fn semantic_identity(self, scalars: &[u32]) -> String {
        svg_fonts::source_semantic_identity(self.source, scalars)
            .expect("qualified semantic identity can be represented")
    }

    fn derive(
        self,
        bytes: &[u8],
        scalars: &[u32],
        identity: &str,
    ) -> Result<font_subset::Derived, font_subset::Error> {
        font_subset::derive(
            bytes,
            self.source.container(),
            scalars,
            self.alias(),
            identity,
        )
    }
}

/// Every qualified source in corpus order.
fn qualification_sources() -> Vec<QualificationSource> {
    let catalog = crate::fonts::revision_1();
    let mut sources = catalog
        .assets
        .iter()
        .map(QualificationSource::chunk)
        .collect::<Vec<_>>();
    sources.push(QualificationSource::noto_emoji());
    assert_eq!(sources.len(), SOURCE_COUNT);
    sources
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Corpus {
    schema: String,
    compatibility_revision: u64,
    originating_issue: u64,
    source_manifest_sha256: String,
    subset_profile_sha256: String,
    classes: Vec<ClassDefinition>,
    sources: Vec<SourceQualification>,
    failures: Vec<FailureQualification>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClassDefinition {
    id: String,
    definition: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceQualification {
    source_identity: String,
    upstream_path: String,
    source_sha256: String,
    /// `woff2` for an official chunk, `sfnt` for the Noto Emoji fallback.
    source_container: String,
    /// For an sfnt source, the digest of its WOFF2 encoding, the container the
    /// canonical viewers load the source face through.
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_source_woff2_sha256: Option<String>,
    effective_alias: String,
    license_identity: String,
    reserved_font_names: Vec<String>,
    cells: Vec<QualificationCell>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualificationCell {
    class: String,
    status: CellStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    ordered_scalars: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_subset_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generated_woff2_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transformation_notice_sha256: Option<String>,
    expected_validation_outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CellStatus {
    Success,
    NotSelected,
    NotApplicable,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FailureQualification {
    id: String,
    expected_error_code: String,
    expected_stage: String,
    max_source_bytes: usize,
    max_scalars: usize,
    expected_generated_woff2_bytes: usize,
    complete_chunk_fallback: bool,
    partial_artifact: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QualificationMeasurementReport {
    generated_bytes_sha256: String,
    resource_traces_sha256: String,
    successful_cells: usize,
    traced_cells: usize,
}

struct QualificationMeasurement {
    generated_bytes: Sha256,
    resource_traces: Sha256,
    successful_cells: usize,
    traced_cells: usize,
}

impl QualificationMeasurement {
    fn new() -> Self {
        Self {
            generated_bytes: Sha256::new(),
            resource_traces: Sha256::new(),
            successful_cells: 0,
            traced_cells: 0,
        }
    }

    fn record_generated(&mut self, source: &str, class: &str, bytes: &[u8]) {
        update_framed(&mut self.generated_bytes, source.as_bytes());
        update_framed(&mut self.generated_bytes, class.as_bytes());
        update_framed(&mut self.generated_bytes, bytes);
        self.successful_cells += 1;
    }

    fn record_trace(&mut self, source: &str, class: &str, bytes: &[u8]) {
        update_framed(&mut self.resource_traces, source.as_bytes());
        update_framed(&mut self.resource_traces, class.as_bytes());
        update_framed(&mut self.resource_traces, bytes);
        self.traced_cells += 1;
        if TRACE_DIGEST_DIAGNOSTICS {
            #[cfg(target_arch = "wasm32")]
            wasm_bindgen_test::console_log!(
                "SVG_FONT_SUBSET_CELL_TRACE={source}\t{class}\t{}",
                sha256(bytes)
            );
            #[cfg(not(target_arch = "wasm32"))]
            println!(
                "SVG_FONT_SUBSET_CELL_TRACE={source}\t{class}\t{}",
                sha256(bytes)
            );
        }
    }

    fn report(&self) -> QualificationMeasurementReport {
        QualificationMeasurementReport {
            generated_bytes_sha256: lower_hex(&self.generated_bytes.clone().finalize()),
            resource_traces_sha256: lower_hex(&self.resource_traces.clone().finalize()),
            successful_cells: self.successful_cells,
            traced_cells: self.traced_cells,
        }
    }
}

fn update_framed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
    hasher.update(bytes);
}

pub(crate) fn assert_complete_font_subset_qualification_corpus() {
    let corpus_bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/qualification/font-subset-corpus.json"
    ));
    let corpus: Corpus = serde_json::from_slice(corpus_bytes)
        .expect("the font subset qualification corpus is valid JSON");

    assert_eq!(corpus.schema, CORPUS_SCHEMA);
    assert_eq!(corpus.compatibility_revision, 1);
    assert_eq!(corpus.originating_issue, 98);
    assert_eq!(
        corpus.source_manifest_sha256,
        SOURCE_MANIFEST_SIDECAR.trim()
    );
    assert_eq!(corpus.subset_profile_sha256, PROFILE_SHA256);
    assert_eq!(corpus.classes.len(), CLASS_IDS.len());
    for (definition, expected) in corpus.classes.iter().zip(CLASS_IDS) {
        assert_eq!(definition.id, expected);
        assert!(!definition.definition.is_empty());
    }
    let sources = qualification_sources();
    assert_eq!(corpus.sources.len(), SOURCE_COUNT);
    assert_eq!(
        corpus.sources.len(),
        crate::fonts::revision_1().assets.len() + 1
    );

    let mut measurement = QualificationMeasurement::new();
    let mut source_identities = BTreeSet::new();
    let mut cell_identities = BTreeSet::new();
    for (qualified, source) in corpus.sources.iter().zip(&sources) {
        assert_eq!(qualified.source_identity, source.identity());
        assert_eq!(qualified.upstream_path, source.upstream_path);
        assert_eq!(qualified.source_sha256, source.source.complete_sha256());
        assert_eq!(qualified.source_container, source.container_name());
        assert_eq!(qualified.effective_alias, source.alias());
        assert_eq!(qualified.license_identity, source.license_identity);
        assert_eq!(
            qualified.reserved_font_names,
            source
                .reserved_font_names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        );
        assert!(source_identities.insert(qualified.source_identity.as_str()));
        assert_eq!(qualified.cells.len(), CLASS_IDS.len());

        let asset = source.load();
        let sfnt = source.decode(asset.bytes());
        let viewer_source = source.viewer_source_woff2(asset.bytes(), &sfnt);
        assert_eq!(
            qualified.viewer_source_woff2_sha256.as_deref(),
            (source.source.container() == font_subset::SourceContainer::Sfnt)
                .then(|| sha256(&viewer_source))
                .as_deref(),
            "{} viewer source container",
            source.upstream_path
        );
        if source.source.container() == font_subset::SourceContainer::Sfnt {
            assert_eq!(
                wuff::decompress_woff2(&viewer_source)
                    .map(|decoded| canonical_tables(&decoded))
                    .expect("viewer source WOFF2 decodes"),
                canonical_tables(&sfnt),
                "{} viewer source WOFF2 reconstructs the manifested face",
                source.upstream_path
            );
        }
        let expected_cells = classify_source(source.family, source.ranges(), &sfnt);
        for ((cell, expected), class) in qualified.cells.iter().zip(expected_cells).zip(CLASS_IDS) {
            assert_eq!(cell.class, class);
            assert!(
                cell_identities.insert((qualified.source_identity.as_str(), cell.class.as_str()))
            );
            assert_cell(
                *source,
                asset.bytes(),
                qualified,
                cell,
                expected,
                &mut measurement,
            );
        }
    }
    assert_eq!(source_identities.len(), SOURCE_COUNT);
    assert_eq!(cell_identities.len(), SOURCE_COUNT * CLASS_IDS.len());
    assert_failure_cells(&corpus.failures);
    let report = serde_json::to_string(&measurement.report())
        .expect("qualification measurement report is serializable");
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_test::console_log!("SVG_FONT_SUBSET_TARGET_MEASUREMENT={report}");
    #[cfg(not(target_arch = "wasm32"))]
    println!("SVG_FONT_SUBSET_TARGET_MEASUREMENT={report}");
}

/// The tables of an sfnt by tag, with `head.checkSumAdjustment` zeroed: what a
/// lossless container round trip preserves. `glyf` and `loca` are compared
/// through the canonical serializer since the WOFF2 transform re-serializes
/// them, and `hmtx` likewise since the transform may reconstruct it.
fn canonical_tables(sfnt: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let font = FontRef::new(sfnt).expect("sfnt parses");
    let mut tables = BTreeMap::new();
    for record in font.table_directory().table_records() {
        let tag = record.tag();
        let mut bytes = font
            .data_for_tag(tag)
            .expect("table directory entry has data")
            .as_bytes()
            .to_vec();
        if tag == write_fonts::read::types::Tag::new(b"head") {
            bytes[8..12].fill(0);
        }
        tables.insert(tag.to_string(), bytes);
    }
    for tag in ["glyf", "loca", "hmtx"] {
        tables.remove(tag);
    }
    let face = crate::shaping::Face::from_slice(sfnt, 0).expect("sfnt shapes");
    let glyph_count = face.number_of_glyphs();
    let mut glyphs = Vec::new();
    for glyph in 0..glyph_count {
        let glyph = crate::shaping::GlyphId(glyph);
        let mut outline = OutlineDigest::default();
        let bounds = face.outline_glyph(glyph, &mut outline);
        glyphs.extend_from_slice(
            &face
                .glyph_hor_advance(glyph)
                .unwrap_or_default()
                .to_le_bytes(),
        );
        glyphs.extend_from_slice(
            &face
                .glyph_hor_side_bearing(glyph)
                .unwrap_or_default()
                .to_le_bytes(),
        );
        glyphs.extend_from_slice(&outline.bytes);
        if let Some(bounds) = bounds {
            for value in [bounds.x_min, bounds.y_min, bounds.x_max, bounds.y_max] {
                glyphs.extend_from_slice(&value.to_le_bytes());
            }
        }
        glyphs.push(b'|');
    }
    tables.insert("glyphs".to_owned(), glyphs);
    tables
}

#[derive(Default)]
struct OutlineDigest {
    bytes: Vec<u8>,
}

impl crate::shaping::OutlineBuilder for OutlineDigest {
    fn move_to(&mut self, x: f32, y: f32) {
        self.bytes.push(b'M');
        self.bytes.extend_from_slice(&x.to_bits().to_le_bytes());
        self.bytes.extend_from_slice(&y.to_bits().to_le_bytes());
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.bytes.push(b'L');
        self.bytes.extend_from_slice(&x.to_bits().to_le_bytes());
        self.bytes.extend_from_slice(&y.to_bits().to_le_bytes());
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.bytes.push(b'Q');
        for value in [x1, y1, x, y] {
            self.bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }

    fn close(&mut self) {
        self.bytes.push(b'Z');
    }
}

fn assert_cell(
    source: QualificationSource,
    source_bytes: &[u8],
    qualified: &SourceQualification,
    cell: &QualificationCell,
    expected: ClassifiedCell,
    measurement: &mut QualificationMeasurement,
) {
    assert_eq!(cell.status, expected.status);
    assert_eq!(cell.ordered_scalars.as_deref(), expected.scalars.as_deref());
    match expected.status {
        CellStatus::Success => {
            assert_eq!(cell.expected_validation_outcome, "valid");
            assert!(cell.reason.is_none());
            let scalars = expected.scalars.as_deref().expect("success has scalars");
            let semantic_identity = source.semantic_identity(scalars);
            assert_eq!(
                cell.semantic_subset_identity.as_deref(),
                Some(semantic_identity.as_str())
            );
            let mut session = font_subset::Session::new();
            let derived = session
                .derive(
                    source.identity(),
                    source_bytes,
                    source.source.container(),
                    scalars,
                    &qualified.effective_alias,
                    &semantic_identity,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{} {} derives at {:?}",
                        source.upstream_path, cell.class, error.stage
                    )
                });
            let cached = session
                .derive(
                    source.identity(),
                    source_bytes,
                    source.source.container(),
                    scalars,
                    &qualified.effective_alias,
                    &semantic_identity,
                )
                .expect("qualified request-cache replay succeeds");
            assert_eq!(derived.woff2, cached.woff2);
            assert_eq!(derived.decoded_sfnt_bytes, cached.decoded_sfnt_bytes);
            assert_eq!(derived.subset_sfnt_bytes, cached.subset_sfnt_bytes);
            assert_eq!(derived.subset_tables, cached.subset_tables);
            let output_sha256 = sha256(derived.woff2.as_ref());
            if cell.generated_woff2_sha256.as_deref() != Some(output_sha256.as_str()) {
                let decoded = wuff::decompress_woff2(derived.woff2.as_ref())
                    .expect("divergent WOFF2 still decodes");
                panic!(
                    "{} {} generated WOFF2 diverged: expected {:?}, actual {}, decoded sfnt {}",
                    source.upstream_path,
                    cell.class,
                    cell.generated_woff2_sha256,
                    output_sha256,
                    sha256(&decoded)
                );
            }
            measurement.record_generated(source.identity(), &cell.class, derived.woff2.as_ref());
            let trace = canonical_trace(source, scalars, Some(&derived));
            measurement.record_trace(source.identity(), &cell.class, &trace);
            let notice = svg_fonts::subset_notice(
                source.source,
                source.family_name(),
                &qualified.effective_alias,
                &semantic_identity,
                &derived,
            )
            .unwrap_or_else(|error| panic!("qualified subset notice failed at {:?}", error.stage));
            assert_eq!(
                notice.provenance.license_identity,
                qualified.license_identity
            );
            assert_eq!(
                cell.transformation_notice_sha256.as_deref(),
                Some(sha256(notice.resource_text.as_bytes()).as_str())
            );
        }
        CellStatus::NotSelected => {
            assert_eq!(cell.expected_validation_outcome, "not_selected");
            assert!(cell.semantic_subset_identity.is_none());
            assert!(cell.generated_woff2_sha256.is_none());
            assert!(cell.transformation_notice_sha256.is_none());
            let scalars = expected
                .scalars
                .as_deref()
                .expect("not-selected has scalars");
            let trace = canonical_trace(source, scalars, None);
            measurement.record_trace(source.identity(), &cell.class, &trace);
        }
        CellStatus::NotApplicable => {
            assert_eq!(cell.expected_validation_outcome, "not_applicable");
            assert!(cell.ordered_scalars.is_none());
            assert!(cell.semantic_subset_identity.is_none());
            assert!(cell.generated_woff2_sha256.is_none());
            assert!(cell.transformation_notice_sha256.is_none());
            assert_eq!(cell.reason.as_deref(), expected.reason);
        }
    }
}

fn assert_failure_cells(failures: &[FailureQualification]) {
    let expected = [
        ("empty-source", "decode"),
        ("short-semantic-identity", "sfnt"),
        ("empty-generated-sfnt", "validate"),
    ];
    assert_eq!(failures.len(), expected.len());
    let record = &crate::fonts::revision_1().assets[0];
    let asset = crate::fonts::inspection::load_asset(record.upstream_path)
        .expect("failure fixture source verifies")
        .expect("failure fixture source exists");
    for (failure, (id, stage)) in failures.iter().zip(expected) {
        assert_eq!(failure.id, id);
        assert_eq!(failure.expected_error_code, "encode.svg-font-subset-failed");
        assert_eq!(failure.expected_stage, stage);
        assert!(!failure.complete_chunk_fallback);
        assert!(!failure.partial_artifact);
        let (source_bytes, scalar_count) = match id {
            "empty-source" => (0, 1),
            "short-semantic-identity" => (asset.bytes().len(), 1),
            "empty-generated-sfnt" => (0, 0),
            _ => panic!("unknown failure fixture"),
        };
        assert!(source_bytes <= failure.max_source_bytes);
        assert!(scalar_count <= failure.max_scalars);
        let (actual, generated_woff2_bytes) = match id {
            "empty-source" => {
                match font_subset::derive(
                    &[],
                    font_subset::SourceContainer::Woff2,
                    &[u32::from('A')],
                    "Test",
                    &"0".repeat(64),
                ) {
                    Ok(derived) => (None, Some(derived.woff2.len())),
                    Err(error) => (Some(error.stage), None),
                }
            }
            "short-semantic-identity" => {
                match font_subset::derive(
                    asset.bytes(),
                    font_subset::SourceContainer::Woff2,
                    &[u32::from('A')],
                    "Test",
                    "short",
                ) {
                    Ok(derived) => (None, Some(derived.woff2.len())),
                    Err(error) => (Some(error.stage), None),
                }
            }
            "empty-generated-sfnt" => match font_subset::validate_checksums(&[]) {
                Ok(()) => panic!("empty generated sfnt passed validation"),
                Err(error) => (Some(error.stage), None),
            },
            _ => panic!("unknown failure fixture"),
        };
        let actual = actual.expect("failure fixture returns no successful subset");
        assert_eq!(stage_name(actual), stage);
        assert_eq!(
            generated_woff2_bytes.unwrap_or_default(),
            failure.expected_generated_woff2_bytes
        );
        assert!(generated_woff2_bytes.is_none());
        let public_error =
            super::font_subset_qualification_error(actual, "Cascadia Code", record.identity);
        assert_eq!(
            public_error.code(),
            crate::ErrorCode::ENCODE_SVG_FONT_SUBSET_FAILED
        );
        let crate::DiagnosticDetails::SvgFontSubsetFailure {
            stage: public_stage,
            ..
        } = public_error
            .details()
            .expect("font subset error has details")
        else {
            panic!("font subset error has typed subset details");
        };
        assert_eq!(*public_stage, actual);
    }
}

struct ClassifiedCell {
    status: CellStatus,
    scalars: Option<Vec<u32>>,
    reason: Option<&'static str>,
}

fn classify_source(family: &str, ranges: &[(u32, u32)], sfnt: &[u8]) -> Vec<ClassifiedCell> {
    let face = crate::shaping::Face::from_slice(sfnt, 0).expect("qualified source parses");
    let font = FontRef::new(sfnt).expect("qualified source is an sfnt");

    let mapped = first_mapped(&face, ranges, |_| true).expect("source declares a mapped scalar");
    let boundaries = range_boundaries(&face, ranges);
    let outside = first_outside(family, ranges);
    let full = full_intersection(&face, ranges);
    let non_bmp = first_mapped(&face, ranges, |scalar| scalar > 0xffff);
    let combining = first_mapped(&face, ranges, is_combining_mark);
    let composite = first_mapped(&face, ranges, |scalar| {
        mapped_glyph_is_composite(&face, &font, scalar)
    });
    let layout = layout_probe(&face, &font);

    vec![
        success(vec![mapped]),
        success(boundaries),
        outside.map_or_else(
            || not_applicable("declared ranges cover every Unicode scalar"),
            |scalar| not_selected(vec![scalar]),
        ),
        success(full),
        non_bmp.map_or_else(
            || not_applicable("source has no mapped non-BMP scalar"),
            |scalar| success(vec![scalar]),
        ),
        combining.map_or_else(
            || not_applicable("source has no mapped combining mark"),
            |scalar| success(vec![scalar]),
        ),
        composite.map_or_else(
            || not_applicable("source has no mapped composite glyph"),
            |scalar| success(vec![scalar]),
        ),
        layout.map_or_else(
            || not_applicable("source has no mapped layout-referenced glyph"),
            success,
        ),
        first_mapped(&face, ranges, |scalar| {
            char::from_u32(scalar).is_some_and(super::text::is_default_ignorable)
        })
        .map_or_else(
            || {
                not_applicable(
                    "source has no mapped default-ignorable scalar in its declared ranges",
                )
            },
            |scalar| success(vec![scalar]),
        ),
    ]
}

fn success(scalars: Vec<u32>) -> ClassifiedCell {
    ClassifiedCell {
        status: CellStatus::Success,
        scalars: Some(scalars),
        reason: None,
    }
}

fn not_selected(scalars: Vec<u32>) -> ClassifiedCell {
    ClassifiedCell {
        status: CellStatus::NotSelected,
        scalars: Some(scalars),
        reason: None,
    }
}

fn not_applicable(reason: &'static str) -> ClassifiedCell {
    ClassifiedCell {
        status: CellStatus::NotApplicable,
        scalars: None,
        reason: Some(reason),
    }
}

fn first_mapped(
    face: &crate::shaping::Face,
    ranges: &[(u32, u32)],
    predicate: impl Fn(u32) -> bool,
) -> Option<u32> {
    for (start, end) in ranges {
        for scalar in *start..=*end {
            if predicate(scalar)
                && qualification_character(scalar)
                    .is_some_and(|character| face.glyph_index(character).is_some())
            {
                return Some(scalar);
            }
        }
    }
    None
}

fn range_boundaries(face: &crate::shaping::Face, ranges: &[(u32, u32)]) -> Vec<u32> {
    let mut scalars = Vec::new();
    let range = ranges.first().copied().expect("source declares a range");
    if let Some(scalar) = first_mapped(face, &[range], |scalar| !is_combining_mark(scalar)) {
        push_scalar(&mut scalars, scalar);
    }
    if let Some(scalar) = (range.0..=range.1).rev().find(|scalar| {
        !is_combining_mark(*scalar)
            && qualification_character(*scalar)
                .is_some_and(|character| face.glyph_index(character).is_some())
    }) {
        push_scalar(&mut scalars, scalar);
    }
    if scalars.is_empty() {
        push_scalar(
            &mut scalars,
            first_mapped(face, ranges, |_| true).expect("source declares a mapped scalar"),
        );
    }
    scalars
}

fn full_intersection(face: &crate::shaping::Face, ranges: &[(u32, u32)]) -> Vec<u32> {
    let mut scalars =
        vec![first_mapped(face, ranges, |_| true).expect("source declares a mapped scalar")];
    for (start, end) in ranges {
        let scalar = (*start..=*end)
            .find(|scalar| {
                qualification_character(*scalar)
                    .is_some_and(|character| face.glyph_index(character).is_none())
            })
            .or_else(|| {
                (*start..=*end).find(|scalar| {
                    qualification_character(*scalar)
                        .is_some_and(|character| face.glyph_index(character).is_some())
                })
            })
            .or_else(|| (*start..=*end).find(|scalar| char::from_u32(*scalar).is_some()))
            .expect("declared range includes a Unicode scalar");
        push_scalar(&mut scalars, scalar);
    }
    scalars.sort_by_key(|scalar| !is_combining_mark(*scalar));
    scalars
}

fn push_scalar(scalars: &mut Vec<u32>, scalar: u32) {
    if char::from_u32(scalar).is_some() && !scalars.contains(&scalar) {
        scalars.push(scalar);
    }
}

fn first_outside(family: &str, ranges: &[(u32, u32)]) -> Option<u32> {
    let candidates: Box<dyn Iterator<Item = u32>> = if family == "Xiaolai" {
        Box::new(
            crate::fonts::inspection::CJK_SCALAR_RANGES_V1
                .iter()
                .flat_map(|(start, end)| *start..=*end),
        )
    } else {
        Box::new((0x20..=0x10ffff).chain(0..0x20))
    };
    candidates.into_iter().find(|scalar| {
        char::from_u32(*scalar).is_some()
            && !ranges
                .iter()
                .any(|(start, end)| start <= scalar && scalar <= end)
    })
}

fn is_combining_mark(scalar: u32) -> bool {
    char::from_u32(scalar).is_some_and(|character| {
        unicode_normalization::char::canonical_combining_class(character) != 0
    })
}

fn qualification_character(scalar: u32) -> Option<char> {
    let character = char::from_u32(scalar)?;
    let low = scalar & 0xffff;
    if character.is_control() || matches!(scalar, 0xfdd0..=0xfdef) || matches!(low, 0xfffe | 0xffff)
    {
        None
    } else {
        Some(character)
    }
}

fn mapped_glyph_is_composite(face: &crate::shaping::Face, font: &FontRef<'_>, scalar: u32) -> bool {
    let Some(character) = char::from_u32(scalar) else {
        return false;
    };
    let Some(glyph_id) = face.glyph_index(character) else {
        return false;
    };
    let Ok(loca) = font.loca(None) else {
        return false;
    };
    let Ok(glyf) = font.glyf() else {
        return false;
    };
    matches!(
        loca.get_glyf(GlyphId::new(u32::from(glyph_id.0)), &glyf),
        Ok(Some(write_fonts::read::tables::glyf::Glyph::Composite(_)))
    )
}

fn layout_probe(face: &crate::shaping::Face, font: &FontRef<'_>) -> Option<Vec<u32>> {
    if font.gsub().is_err() && font.gpos().is_err() {
        return None;
    }
    let manifested = [
        "office", "ffi", "fl", "AV", "To", "Ta", "Wa", "Yo", "لا", "سلام",
    ];
    if let Some(probe) = manifested.iter().find(|probe| {
        probe
            .chars()
            .all(|character| face.glyph_index(character).is_some())
            && probe_exercises_layout(face, probe)
    }) {
        return Some(probe_scalars(probe));
    }

    const MAX_LAYOUT_CANDIDATES: usize = 48;
    let referenced = font_subset::layout_glyph_ids(font)?;
    let cmap = font.cmap().ok()?;
    let (_, _, subtable) = cmap.best_subtable()?;
    let mapped = subtable
        .iter_with_limits(CmapIterLimits::default_for_font(font))
        .filter_map(|(scalar, glyph)| {
            u16::try_from(glyph.to_u32())
                .ok()
                .filter(|glyph| {
                    referenced
                        .get(usize::from(*glyph))
                        .copied()
                        .unwrap_or(false)
                })
                .filter(|_| qualification_character(scalar).is_some())
                .map(|_| scalar)
        })
        .collect::<Vec<_>>();
    let candidates = (0..mapped.len().min(MAX_LAYOUT_CANDIDATES))
        .filter_map(|index| {
            let sampled = if mapped.len() <= MAX_LAYOUT_CANDIDATES {
                index
            } else {
                index * (mapped.len() - 1) / (MAX_LAYOUT_CANDIDATES - 1)
            };
            mapped.get(sampled).copied()
        })
        .filter_map(char::from_u32)
        .collect::<Vec<_>>();

    for character in &candidates {
        let probe = character.to_string();
        if probe_exercises_layout(face, &probe) {
            return Some(probe_scalars(&probe));
        }
        let probe = [*character, *character].iter().collect::<String>();
        if probe_exercises_layout(face, &probe) {
            return Some(probe_scalars(&probe));
        }
    }
    for first in &candidates {
        for second in &candidates {
            let probe = [*first, *second].iter().collect::<String>();
            if probe_exercises_layout(face, &probe) {
                return Some(probe_scalars(&probe));
            }
        }
    }
    for window in candidates.windows(3) {
        let probe = window.iter().collect::<String>();
        if probe_exercises_layout(face, &probe) {
            return Some(probe_scalars(&probe));
        }
        let probe = window.iter().rev().collect::<String>();
        if probe_exercises_layout(face, &probe) {
            return Some(probe_scalars(&probe));
        }
    }
    let carrier = mapped.into_iter().take(64).collect::<Vec<_>>();
    (!carrier.is_empty()).then_some(carrier)
}

fn probe_scalars(probe: &str) -> Vec<u32> {
    let mut scalars = Vec::new();
    for scalar in probe.chars().map(u32::from) {
        push_scalar(&mut scalars, scalar);
    }
    scalars
}

fn probe_exercises_layout(face: &crate::shaping::Face, probe: &str) -> bool {
    let mapped = probe
        .chars()
        .filter_map(|character| face.glyph_index(character))
        .collect::<Vec<_>>();
    let shaped = crate::shaping::shape(face, &[], probe, None);
    if shaped.glyph_infos().len() != mapped.len()
        || shaped
            .glyph_infos()
            .iter()
            .zip(&mapped)
            .any(|(info, glyph)| info.glyph_id != u32::from(glyph.0))
    {
        return true;
    }
    shaped
        .glyph_positions()
        .iter()
        .zip(mapped)
        .any(|(position, glyph)| {
            position.x_advance != i32::from(face.glyph_hor_advance(glyph).unwrap_or_default())
                || position.y_advance != 0
                || position.x_offset != 0
                || position.y_offset != 0
        })
}

fn canonical_trace(
    source: QualificationSource,
    scalars: &[u32],
    derived: Option<&font_subset::Derived>,
) -> Vec<u8> {
    let selected = derived.is_some();
    let (outcome, trace) =
        super::render_font_subset_qualification_trace(source.family_name(), scalars);
    outcome.unwrap_or_else(|error| {
        panic!(
            "{} {scalars:?} canonical trace render fails with {error:?} / {:?}; {}",
            source.upstream_path,
            error.code(),
            family_failure_diagnosis(source, scalars)
        )
    });
    let trace_value: serde_json::Value =
        serde_json::from_slice(&trace).expect("canonical trace is JSON");
    let entries = trace_value["entries"]
        .as_array()
        .expect("canonical trace entries are an array");
    let has_event = |event: &str| {
        entries
            .iter()
            .any(|entry| entry["event"] == event && entry["semantic_identity"] == source.identity())
    };
    assert!(has_event("svg_font_chunk_considered"));
    assert_eq!(has_event("svg_font_chunk_selected"), selected);
    if selected {
        for event in [
            "svg_font_woff2_decode",
            "svg_font_subset_plan",
            "svg_font_subset_sfnt",
            "svg_font_woff2_encode",
            "svg_font_validate",
            "svg_font_css_data_url",
        ] {
            assert!(has_event(event), "{} lacks {event}", source.upstream_path);
        }
    }
    trace
}

fn family_failure_diagnosis(target: QualificationSource, scalars: &[u32]) -> String {
    for source in qualification_sources()
        .into_iter()
        .filter(|source| source.family == target.family)
    {
        if source.source.selected_by(scalars) != Some(true) {
            continue;
        }
        let asset = source.load();
        let identity = source.semantic_identity(scalars);
        if let Err(error) = source.derive(asset.bytes(), scalars, &identity) {
            let prefix = (1..=scalars.len()).find(|length| {
                let values = &scalars[..*length];
                let identity = source.semantic_identity(values);
                source.derive(asset.bytes(), values, &identity).is_err()
            });
            return format!(
                "{} fails at {:?}, first bad prefix {prefix:?}",
                source.upstream_path, error.stage
            );
        }
    }
    "no selected source reproduces the failure".to_owned()
}

fn sha256(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

fn effective_alias(family: &str) -> &'static str {
    svg_fonts::Bundle::effective_family(family_name(family))
}

/// The resolved family name of a catalog family: the inverse of
/// `svg_fonts::catalog_family`.
fn family_name(family: &str) -> &'static str {
    match family {
        "Cascadia" => "Cascadia Code",
        "ComicShanns" => "Comic Shanns",
        "Liberation" => "Liberation Sans",
        "Lilita" => "Lilita One",
        "Excalifont" => "Excalifont",
        "Nunito" => "Nunito",
        "Virgil" => "Virgil",
        "Xiaolai" => "Xiaolai",
        "NotoEmoji" => "Noto Emoji",
        _ => panic!("unknown qualification source family"),
    }
}

fn stage_name(stage: crate::SvgFontSubsetStage) -> &'static str {
    match stage {
        crate::SvgFontSubsetStage::Decode => "decode",
        crate::SvgFontSubsetStage::Plan => "plan",
        crate::SvgFontSubsetStage::Sfnt => "sfnt",
        crate::SvgFontSubsetStage::Encode => "encode",
        crate::SvgFontSubsetStage::Validate => "validate",
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn build_corpus() -> Corpus {
    let catalog = crate::fonts::revision_1();
    let mut sources = Vec::with_capacity(SOURCE_COUNT);
    for source in qualification_sources() {
        let asset = source.load();
        let sfnt = source.decode(asset.bytes());
        let viewer_source_woff2_sha256 = (source.source.container()
            == font_subset::SourceContainer::Sfnt)
            .then(|| sha256(&source.viewer_source_woff2(asset.bytes(), &sfnt)));
        let cells = CLASS_IDS
            .into_iter()
            .zip(classify_source(source.family, source.ranges(), &sfnt))
            .map(|(class, classified)| materialize_cell(source, asset.bytes(), class, classified))
            .collect();
        sources.push(SourceQualification {
            source_identity: source.identity().to_owned(),
            upstream_path: source.upstream_path.to_owned(),
            source_sha256: source.source.complete_sha256().to_owned(),
            source_container: source.container_name().to_owned(),
            viewer_source_woff2_sha256,
            effective_alias: source.alias().to_owned(),
            license_identity: source.license_identity.to_owned(),
            reserved_font_names: source
                .reserved_font_names
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            cells,
        });
    }
    Corpus {
        schema: CORPUS_SCHEMA.to_owned(),
        compatibility_revision: 1,
        originating_issue: 98,
        source_manifest_sha256: SOURCE_MANIFEST_SIDECAR.trim().to_owned(),
        subset_profile_sha256: PROFILE_SHA256.to_owned(),
        classes: class_definitions(),
        sources,
        failures: vec![
            failure("empty-source", "decode", 0, 1),
            failure(
                "short-semantic-identity",
                "sfnt",
                catalog.assets[0].complete_length,
                1,
            ),
            failure("empty-generated-sfnt", "validate", 0, 0),
        ],
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn materialize_cell(
    source: QualificationSource,
    source_bytes: &[u8],
    class: &str,
    classified: ClassifiedCell,
) -> QualificationCell {
    match classified.status {
        CellStatus::Success => {
            let scalars = classified.scalars.expect("success has scalars");
            let semantic_identity = source.semantic_identity(&scalars);
            let derived = source
                .derive(source_bytes, &scalars, &semantic_identity)
                .unwrap_or_else(|error| {
                    let first_bad_prefix = (1..=scalars.len()).find(|length| {
                        let prefix = &scalars[..*length];
                        let identity = source.semantic_identity(prefix);
                        source.derive(source_bytes, prefix, &identity).is_err()
                    });
                    panic!(
                        "{} {class} {scalars:?}, first bad prefix {first_bad_prefix:?}: {:?}",
                        source.upstream_path, error.stage
                    )
                });
            let output_sha256 = sha256(derived.woff2.as_ref());
            let notice = svg_fonts::subset_notice(
                source.source,
                source.family_name(),
                source.alias(),
                &semantic_identity,
                &derived,
            )
            .unwrap_or_else(|error| panic!("qualified subset notice failed at {:?}", error.stage));
            QualificationCell {
                class: class.to_owned(),
                status: CellStatus::Success,
                ordered_scalars: Some(scalars),
                semantic_subset_identity: Some(semantic_identity),
                generated_woff2_sha256: Some(output_sha256),
                transformation_notice_sha256: Some(sha256(notice.resource_text.as_bytes())),
                expected_validation_outcome: "valid".to_owned(),
                reason: None,
            }
        }
        CellStatus::NotSelected => {
            let scalars = classified.scalars.expect("not-selected has scalars");
            QualificationCell {
                class: class.to_owned(),
                status: CellStatus::NotSelected,
                ordered_scalars: Some(scalars),
                semantic_subset_identity: None,
                generated_woff2_sha256: None,
                transformation_notice_sha256: None,
                expected_validation_outcome: "not_selected".to_owned(),
                reason: None,
            }
        }
        CellStatus::NotApplicable => QualificationCell {
            class: class.to_owned(),
            status: CellStatus::NotApplicable,
            ordered_scalars: None,
            semantic_subset_identity: None,
            generated_woff2_sha256: None,
            transformation_notice_sha256: None,
            expected_validation_outcome: "not_applicable".to_owned(),
            reason: classified.reason.map(str::to_owned),
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn failure(
    id: &str,
    stage: &str,
    max_source_bytes: usize,
    max_scalars: usize,
) -> FailureQualification {
    FailureQualification {
        id: id.to_owned(),
        expected_error_code: "encode.svg-font-subset-failed".to_owned(),
        expected_stage: stage.to_owned(),
        max_source_bytes,
        max_scalars,
        expected_generated_woff2_bytes: 0,
        complete_chunk_fallback: false,
        partial_artifact: false,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn class_definitions() -> Vec<ClassDefinition> {
    [
        "first source-mapped Unicode scalar inside the chunk's declared ranges",
        "ordered unique source-mapped scalars nearest the first declared range boundaries",
        "first Unicode scalar outside every declared range; the chunk must not be selected",
        "ordered unique scalar representatives intersecting every declared range, preferring unmapped scalars to isolate selection",
        "first source-mapped scalar above U+FFFF",
        "first source-mapped scalar with a nonzero canonical combining class",
        "first source-mapped scalar whose TrueType glyph is composite",
        "complete bounded carrier for source-mapped glyphs referenced by GSUB/GPOS/GDEF closure, preferring a shaping probe that changes glyph choice, count, advance, or offset",
        "first source-mapped default-ignorable scalar without a literal space; verifies the shaper's implicit space-glyph dependency",
    ]
    .into_iter()
    .zip(CLASS_IDS)
    .map(|(definition, id)| ClassDefinition {
        id: id.to_owned(),
        definition: definition.to_owned(),
    })
    .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn materialize_viewer_inputs() {
    use std::fs;
    use std::path::PathBuf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = root.join("target/svg-font-subset-viewer-qualification");
    if output.exists() {
        fs::remove_dir_all(&output).expect("stale viewer qualification inputs can be removed");
    }
    fs::create_dir_all(output.join("inputs"))
        .expect("viewer qualification input directory can be created");

    let corpus: Corpus = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/qualification/font-subset-corpus.json"
    )))
    .expect("the font subset qualification corpus is valid JSON");
    let sources = qualification_sources();
    assert_eq!(corpus.sources.len(), sources.len());
    let mut executions = BTreeMap::<String, serde_json::Value>::new();

    for (source, qualified) in sources.iter().zip(&corpus.sources) {
        assert_eq!(qualified.source_identity, source.identity());
        assert_eq!(qualified.upstream_path, source.upstream_path);
        assert_eq!(qualified.source_sha256, source.source.complete_sha256());
        assert_eq!(qualified.source_container, source.container_name());
        let asset = source.load();
        let source_sfnt = source.decode(asset.bytes());
        // The canonical viewers admit only WOFF2: a chunk is loaded as itself,
        // the fallback sfnt through the subsetter's encoder, whose digest the
        // corpus pins beside the sfnt's.
        let source_woff2 = source.viewer_source_woff2(asset.bytes(), &source_sfnt);
        assert_eq!(
            qualified.viewer_source_woff2_sha256.as_deref(),
            (source.source.container() == font_subset::SourceContainer::Sfnt)
                .then(|| sha256(&source_woff2))
                .as_deref()
        );
        for cell in &qualified.cells {
            if cell.status != CellStatus::Success {
                continue;
            }
            let scalars = cell
                .ordered_scalars
                .as_deref()
                .expect("success has scalars");
            let identity = cell
                .semantic_subset_identity
                .as_deref()
                .expect("success has semantic identity");
            let coverage = serde_json::json!({
                "class": cell.class,
                "ordered_scalars": scalars,
                "source_identity": qualified.source_identity,
            });
            // Cases are keyed and ordered by what they render, so a subset
            // profile change rewrites their subset fields, not their order.
            let glyph_set = glyph_set(&qualified.source_identity, scalars);
            if let Some(execution) = executions.get_mut(&glyph_set) {
                assert_eq!(
                    execution["semantic_subset_identity"], identity,
                    "one glyph set has one semantic subset identity"
                );
                execution["coverage"]
                    .as_array_mut()
                    .expect("execution coverage is an array")
                    .push(coverage);
                continue;
            }

            let derived = source
                .derive(asset.bytes(), scalars, identity)
                .unwrap_or_else(|error| {
                    panic!(
                        "{} {} viewer subset derives at {:?}",
                        source.upstream_path, cell.class, error.stage
                    )
                });
            let subset_sfnt =
                wuff::decompress_woff2(derived.woff2.as_ref()).expect("qualified subset decodes");
            let mapped_scalars = scalars
                .iter()
                .copied()
                .filter(|scalar| {
                    char::from_u32(*scalar).is_some_and(|character| {
                        crate::shaping::Face::from_slice(&source_sfnt, 0)
                            .is_some_and(|face| face.glyph_index(character).is_some())
                    })
                })
                .collect::<Vec<_>>();
            assert!(
                !mapped_scalars.is_empty(),
                "successful cell has mapped text"
            );

            let case_dir = output.join("inputs").join(identity);
            fs::create_dir_all(&case_dir).expect("viewer case directory can be created");
            let source_woff2_path = case_dir.join("source.woff2");
            let subset_woff2_path = case_dir.join("subset.woff2");
            let source_sfnt_path = case_dir.join("source.sfnt");
            let subset_sfnt_path = case_dir.join("subset.sfnt");
            fs::write(&source_woff2_path, &source_woff2).expect("source WOFF2 can be staged");
            fs::write(&subset_woff2_path, derived.woff2.as_ref())
                .expect("subset WOFF2 can be staged");
            fs::write(&source_sfnt_path, &source_sfnt).expect("source sfnt can be staged");
            fs::write(&subset_sfnt_path, &subset_sfnt).expect("subset sfnt can be staged");

            let family = format!("Qualification-{}", &identity[..16]);
            let (source_family, source_properties) = fontdb_face(&source_sfnt);
            let (subset_family, subset_properties) = fontdb_face(&subset_sfnt);
            assert_eq!(
                source_properties, subset_properties,
                "subset preserves face properties"
            );
            let lines = qualification_lines(&mapped_scalars);
            let width = 384_u32;
            let height = 24_u32
                .checked_add(
                    u32::try_from(lines.len())
                        .expect("line count is a u32")
                        .checked_mul(28)
                        .expect("line height does not overflow"),
                )
                .expect("SVG height does not overflow");
            let source_svg = qualification_svg(
                &family,
                &source_woff2,
                &lines,
                width,
                height,
                identity,
                &source_properties,
            );
            let subset_svg = qualification_svg(
                &family,
                derived.woff2.as_ref(),
                &lines,
                width,
                height,
                identity,
                &source_properties,
            );
            fs::write(case_dir.join("source.svg"), source_svg.as_bytes())
                .expect("source SVG can be staged");
            fs::write(case_dir.join("subset.svg"), subset_svg.as_bytes())
                .expect("subset SVG can be staged");

            let source_assets = qualification_assets(
                &root,
                &source_woff2,
                &source_sfnt_path,
                &source_sfnt,
                &family,
                &source_family,
                &source_properties,
            );
            let subset_assets = qualification_assets(
                &root,
                derived.woff2.as_ref(),
                &subset_sfnt_path,
                &subset_sfnt,
                &family,
                &subset_family,
                &subset_properties,
            );
            write_json(&case_dir.join("source.assets.json"), &source_assets);
            write_json(&case_dir.join("subset.assets.json"), &subset_assets);

            let relative = |name: &str| {
                format!("target/svg-font-subset-viewer-qualification/inputs/{identity}/{name}")
            };
            executions.insert(
                glyph_set.clone(),
                serde_json::json!({
                    "coverage": [coverage],
                    "dimensions": {
                        "byte_length": u64::from(width) * u64::from(height) * 4,
                        "height": height,
                        "layout": "top-left-row-major",
                        "pixel_format": "straight-alpha-srgb-rgba8",
                        "width": width,
                    },
                    "effective_alias": qualified.effective_alias,
                    "glyph_set": glyph_set,
                    "mapped_scalars": mapped_scalars,
                    "semantic_subset_identity": identity,
                    "source_assets": relative("source.assets.json"),
                    "source_assets_sha256": sha256(&json_bytes(&source_assets)),
                    "source_sfnt": relative("source.sfnt"),
                    "source_sfnt_sha256": sha256(&source_sfnt),
                    "source_svg": relative("source.svg"),
                    "source_svg_sha256": sha256(source_svg.as_bytes()),
                    "source_woff2": relative("source.woff2"),
                    "source_woff2_sha256": sha256(&source_woff2),
                    "subset_assets": relative("subset.assets.json"),
                    "subset_assets_sha256": sha256(&json_bytes(&subset_assets)),
                    "subset_sfnt": relative("subset.sfnt"),
                    "subset_sfnt_sha256": sha256(&subset_sfnt),
                    "subset_svg": relative("subset.svg"),
                    "subset_svg_sha256": sha256(subset_svg.as_bytes()),
                    "subset_woff2": relative("subset.woff2"),
                    "subset_woff2_sha256": sha256(derived.woff2.as_ref()),
                }),
            );
        }
    }

    let cases = serde_json::json!({
        "cases": executions.into_values().collect::<Vec<_>>(),
        "corpus_sha256": sha256(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/qualification/font-subset-corpus.json"
        ))),
        "schema": "excalidraw-render/svg-font-subset-viewer-inputs/v2",
        "subset_profile_sha256": PROFILE_SHA256,
    });
    write_json(&output.join("inputs.json"), &cases);
}

/// The key of a viewer qualification case: the SHA-256 of the source font's
/// identity (which names its bytes' digest) and the ordered scalars set in it.
/// It is the semantic subset identity's preimage without the subset profile,
/// so it names what is rendered and survives a profile change;
/// `xtask conformance verify-svg-font-subsets` recomputes it from the case's
/// coverage.
#[cfg(not(target_arch = "wasm32"))]
fn glyph_set(source_identity: &str, scalars: &[u32]) -> String {
    let scalars = scalars
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    sha256(
        format!("excalidraw-render:svg-font-subset-glyph-set:v1\n{source_identity}\n{scalars}")
            .as_bytes(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn qualification_lines(scalars: &[u32]) -> Vec<String> {
    scalars
        .chunks(12)
        .map(|line| {
            line.iter()
                .map(|scalar| char::from_u32(*scalar).expect("manifested scalar is Unicode"))
                .collect()
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn qualification_svg(
    family: &str,
    woff2: &[u8],
    lines: &[String],
    width: u32,
    height: u32,
    identity: &str,
    properties: &FaceProperties,
) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(woff2);
    let mut text = String::new();
    for (index, line) in lines.iter().enumerate() {
        let y = 24 + u32::try_from(index).expect("line index is a u32") * 28;
        text.push_str(&format!(
            "<tspan x=\"12\" y=\"{y}\">{}</tspan>",
            xml_escape(line)
        ));
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\"><style>@font-face{{font-family:'{family}';font-style:{};font-weight:{};font-stretch:{};src:url(data:font/woff2;base64,{encoded}) format(woff2);unicode-range:U+0-10FFFF}}</style><text data-semantic-subset-identity=\"{identity}\" font-family=\"{family}\" font-style=\"{}\" font-weight=\"{}\" font-stretch=\"{}\" font-size=\"20\" fill=\"#111111\">{text}</text></svg>",
        properties.style,
        properties.weight,
        properties.stretch,
        properties.style,
        properties.weight,
        properties.stretch,
    )
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Eq, PartialEq)]
struct FaceProperties {
    stretch: &'static str,
    style: &'static str,
    weight: u16,
}

#[cfg(not(target_arch = "wasm32"))]
fn fontdb_face(sfnt: &[u8]) -> (String, FaceProperties) {
    use std::sync::Arc;

    let mut database = usvg::fontdb::Database::new();
    let ids = database.load_font_source(usvg::fontdb::Source::Binary(Arc::new(sfnt.to_vec())));
    let face = ids
        .iter()
        .filter_map(|id| database.face(*id))
        .find(|face| face.index == 0)
        .expect("qualified sfnt has face index zero");
    let stretch = match face.stretch {
        usvg::fontdb::Stretch::UltraCondensed => "ultra-condensed",
        usvg::fontdb::Stretch::ExtraCondensed => "extra-condensed",
        usvg::fontdb::Stretch::Condensed => "condensed",
        usvg::fontdb::Stretch::SemiCondensed => "semi-condensed",
        usvg::fontdb::Stretch::Normal => "normal",
        usvg::fontdb::Stretch::SemiExpanded => "semi-expanded",
        usvg::fontdb::Stretch::Expanded => "expanded",
        usvg::fontdb::Stretch::ExtraExpanded => "extra-expanded",
        usvg::fontdb::Stretch::UltraExpanded => "ultra-expanded",
    };
    let style = match face.style {
        usvg::fontdb::Style::Normal => "normal",
        usvg::fontdb::Style::Italic => "italic",
        usvg::fontdb::Style::Oblique => "oblique",
    };
    (
        face.families
            .first()
            .map(|(family, _)| family.clone())
            .expect("qualified font has a family name"),
        FaceProperties {
            stretch,
            style,
            weight: face.weight.0,
        },
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn qualification_assets(
    root: &std::path::Path,
    woff2: &[u8],
    sfnt_path: &std::path::Path,
    sfnt: &[u8],
    css_family: &str,
    sfnt_family: &str,
    properties: &FaceProperties,
) -> serde_json::Value {
    let relative = |path: &std::path::Path| {
        path.strip_prefix(root)
            .expect("qualification input is under repository root")
            .to_string_lossy()
            .replace('\\', "/")
    };
    serde_json::json!({
        "fonts": [{
            "css": {
                "family": css_family,
                "stretch": properties.stretch,
                "style": properties.style,
                "unicode_range": ["U+0-10FFFF"],
                "weight": properties.weight,
            },
            "sfnt": {
                "face_index": 0,
                "family": sfnt_family,
                "media_type": "font/ttf",
                "path": relative(sfnt_path),
                "sha256": sha256(sfnt),
                "stretch": properties.stretch,
                "style": properties.style,
                "weight": properties.weight,
            },
            "source": {
                "media_type": "font/woff2",
                "sha256": sha256(woff2),
            }
        }],
        "images": [],
        "schema": "excalidraw-render/svg-viewer-asset-declaration/v1",
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(not(target_arch = "wasm32"))]
fn write_json(path: &std::path::Path, value: &serde_json::Value) {
    std::fs::write(path, json_bytes(value)).expect("qualification JSON can be written");
}

#[cfg(not(target_arch = "wasm32"))]
fn json_bytes(value: &serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("qualification JSON serializes");
    bytes.push(b'\n');
    bytes
}

#[test]
fn layout_probe_detects_horizontal_advance_only_gpos() {
    for record in crate::fonts::revision_1()
        .assets
        .iter()
        .filter(|record| record.family == "Liberation")
    {
        let asset = crate::fonts::inspection::load_asset(record.upstream_path)
            .expect("Liberation source verifies")
            .expect("Liberation source exists");
        let sfnt = wuff::decompress_woff2(asset.bytes()).expect("Liberation source decodes");
        let face = crate::shaping::Face::from_slice(&sfnt, 0).expect("Liberation source parses");
        for probe in ["AV", "To", "Ta", "Wa", "Yo"] {
            let mapped = probe
                .chars()
                .filter_map(|character| face.glyph_index(character))
                .collect::<Vec<_>>();
            if mapped.len() != probe.chars().count() {
                continue;
            }
            let shaped = crate::shaping::shape(&face, &[], probe, None);
            let glyphs_unchanged = shaped.glyph_infos().len() == mapped.len()
                && shaped
                    .glyph_infos()
                    .iter()
                    .zip(&mapped)
                    .all(|(info, glyph)| info.glyph_id == u32::from(glyph.0));
            let positions = shaped.glyph_positions();
            let horizontal_advance_changed =
                positions.iter().zip(&mapped).any(|(position, glyph)| {
                    position.x_advance
                        != i32::from(face.glyph_hor_advance(*glyph).unwrap_or_default())
                });
            let no_other_positioning = positions.iter().all(|position| {
                position.y_advance == 0 && position.x_offset == 0 && position.y_offset == 0
            });
            if glyphs_unchanged && horizontal_advance_changed && no_other_positioning {
                assert!(probe_exercises_layout(&face, probe));
                return;
            }
        }
    }
    panic!("manifest contains advance-only GPOS evidence");
}

/// The release-mode qualification set (#138, #141): every cell of the corpus
/// derives, matches its pinned bytes and replays its canonical trace. It takes
/// minutes even optimized, so the push gate skips it and the packaged-facade
/// workflow lifts the gate with `EXCALIDRAW_RENDER_RELEASE_QUALIFICATION=1`
/// and no name filter, so a rename cannot drop it out of the run. Target
/// evidence also requires the measurement it prints, so a skipped run cannot
/// pass for a qualified one.
#[test]
#[cfg_attr(
    not(release_qualification),
    ignore = "release-mode qualification set; run with EXCALIDRAW_RENDER_RELEASE_QUALIFICATION=1"
)]
fn complete_font_subset_qualification_corpus_matches_generated_bytes_and_traces() {
    assert_complete_font_subset_qualification_corpus();
}

/// Regenerates the qualification corpus and stages it, with its digest sidecar
/// and its packaged copy, under Cargo's scratch space rather than in the
/// source tree (#140): the printed `cp` lines refresh the committed copies,
/// so running the ignored test leaves the checkout clean until the refresh is
/// a deliberate step.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "explicitly refreshes immutable qualification evidence"]
fn materialize_complete_font_subset_qualification_corpus() {
    use std::fs;
    use std::path::PathBuf;

    let value = serde_json::to_value(build_corpus()).expect("corpus is serializable");
    let bytes = serde_json::to_vec(&value).expect("corpus value is serializable");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = fs::canonicalize(root).expect("repository root resolves");
    let staged = scratch_directory("svg-font-subset-qualification-corpus");
    let committed = root.join(CORPUS_PATH);
    let sidecar = committed.with_extension("sha256");
    let packaged = root.join(PACKAGED_CORPUS_PATH);
    let staged_corpus = staged.join(committed.file_name().expect("corpus has a file name"));
    let staged_sidecar = staged.join(sidecar.file_name().expect("sidecar has a file name"));
    fs::write(&staged_corpus, &bytes).expect("corpus can be staged");
    fs::write(&staged_sidecar, format!("{}\n", sha256(&bytes))).expect("sidecar can be staged");
    println!(
        "staged the SVG font subset qualification corpus ({} bytes, sha256 {}); refresh the \
         committed copies with:\n  cp {} {}\n  cp {} {}\n  cp {} {}",
        bytes.len(),
        sha256(&bytes),
        staged_corpus.display(),
        committed.display(),
        staged_sidecar.display(),
        sidecar.display(),
        staged_corpus.display(),
        packaged.display(),
    );
}

/// A fresh `<target>/tmp/<name>-<pid>` directory. Lib tests have no
/// `CARGO_TARGET_TMPDIR`, so the target directory is the one this test binary
/// runs from (`<target>/<profile>/deps/`), which keeps the staging inside the
/// checkout whenever the target directory is.
#[cfg(not(target_arch = "wasm32"))]
fn scratch_directory(name: &str) -> std::path::PathBuf {
    let executable = std::env::current_exe().expect("test executable path is known");
    let target = executable
        .ancestors()
        .nth(3)
        .expect("test executable lives under <target>/<profile>/deps");
    let path = target
        .join("tmp")
        .join(format!("{name}-{}", std::process::id()));
    if path.exists() {
        std::fs::remove_dir_all(&path).expect("stale scratch directory is removable");
    }
    std::fs::create_dir_all(&path).expect("scratch directory is creatable");
    path
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "explicitly materializes canonical viewer qualification inputs"]
fn materialize_complete_font_subset_viewer_inputs() {
    materialize_viewer_inputs();
}
