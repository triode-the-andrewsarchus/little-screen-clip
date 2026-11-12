//! Fixed-seed properties and the nightly fuzz over scene admission and
//! compatibility normalization.
//!
//! The fuzz input is the scene document bytes themselves: every case is a
//! plain document that [`SceneDocument::parse`] admits or rejects, so a
//! minimized regression is a readable scene document rather than generator
//! state, and the regression corpus replays through exactly the seam a caller
//! uses. For one input the exercised contract is:
//!
//! * admission returns a typed `scene.*` rejection whose pointer is a
//!   well-formed RFC 6901 pointer and whose parser-ceiling details name the
//!   crossed fixed ceiling and a measurement beyond it (spec §4.1, §13.1);
//! * the canonicalized original scene document is a fixpoint of admission and
//!   agrees with an independent JSON reader on every value (spec §4.2);
//! * compatibility normalization is deterministic for every output format,
//!   replays records and diagnostics in the total source order (spec §11.3),
//!   keeps paint order, points every diagnostic at a location that exists in
//!   the scene or at a missing member of a record that does, and diagnoses
//!   exactly the element members the scene-surface manifest leaves
//!   unmanifested;
//! * re-normalizing the projected rendering view is a fixpoint: no further
//!   repair diagnostics and the same normalized values;
//! * two fresh public-facade SVG renders agree on artifact bytes and the
//!   complete ordered report, whether they succeed or fail.
//!
//! Termination is bounded structurally, not by a clock: inputs are capped at
//! [`MAX_FUZZ_INPUT_BYTES`], generated scenes at [`MAX_ELEMENTS`] elements and
//! [`MAX_POINTS`] points, mutations at [`MAX_MUTATIONS`], and nesting at
//! [`MAX_NESTING`]. The depth, key, and number-token ceilings of spec §13.1 are
//! crossed here; the 64 MiB input and string ceilings, the structure-entry
//! ceiling, and the parser memory ceiling need multi-megabyte inputs and are
//! held by the `N-1`, `N`, `N+1` vectors in `tests/limits_scene_admission.rs`
//! instead. The parser and normalizer take only the byte slice — no path,
//! URL, environment, or clock — and the one place a scene document can name an
//! external resource is checked to be refused with its typed diagnostic. A
//! panic anywhere is a property failure carrying the input for reproduction.
//! The generator is a seeded xorshift with no environment or clock input, so
//! the pull-request smoke and the nightly expansion replay identically on
//! every target.

mod corpus;
mod generator;
mod projection;

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::normalization::{
    normalize_scene, NormalizationEvent, NormalizationOutcome, NormalizedElement,
    NormalizedElementKind,
};
use crate::scene::{member_value, JsonKind, JsonValue};
use crate::{
    CompatibilityDiagnostic, CompatibilityRevision, DiagnosticDetails, ErrorCode, Measurement,
    OutputFormat, RenderRequest, Renderer, SceneDocument, SceneParseError,
};

pub(crate) use corpus::assert_scene_fuzz_regression_corpus;
use corpus::encode_hex;
use generator::{
    nested_arrays_scene, nested_arrays_scene_pointer, push_pointer_segment, Generator,
};
use projection::assert_normalization_is_idempotent;

const FIXED_SEED: u64 = 0xbb67_ae85_84ca_a73b;
/// Kept small enough to gate every pull request; the nightly expansion runs
/// the same generator far deeper.
const PR_CASES: usize = 64;
const MAX_FUZZ_INPUT_BYTES: usize = 65_536;
const MAX_ELEMENTS: usize = 12;
const MAX_POINTS: usize = 24;
const MAX_MUTATIONS: usize = 16;
const MAX_NESTING: usize = 60_000;
const STRATEGIES: usize = 8;

/// The fixed parser envelope of spec §13.1: the code each ceiling rejects
/// with, its canonical field, and the configured value.
const PARSER_CEILINGS: [(ErrorCode, &str, u64); 8] = [
    (
        ErrorCode::SCENE_INPUT_TOO_LARGE,
        "max_input_bytes",
        67_108_864,
    ),
    (ErrorCode::SCENE_JSON_DEPTH_EXCEEDED, "max_json_depth", 128),
    (
        ErrorCode::SCENE_JSON_STRUCTURE_EXCEEDED,
        "max_json_structure_entries",
        1_048_576,
    ),
    (
        ErrorCode::SCENE_STRING_TOO_LARGE,
        "max_total_string_bytes",
        67_108_864,
    ),
    (
        ErrorCode::SCENE_STRING_TOO_LARGE,
        "max_string_bytes",
        67_108_864,
    ),
    (
        ErrorCode::SCENE_OBJECT_KEY_TOO_LARGE,
        "max_object_key_bytes",
        4_096,
    ),
    (
        ErrorCode::SCENE_NUMBER_TOKEN_TOO_LARGE,
        "max_number_token_bytes",
        128,
    ),
    (
        ErrorCode::SCENE_PARSE_MEMORY_EXCEEDED,
        "max_parse_working_bytes",
        268_435_456,
    ),
];

const ENVELOPE_POINTERS: [&str; 6] = ["", "/type", "/version", "/elements", "/appState", "/files"];

/// What one input produced; two replays must agree on every field.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Outcome {
    Rejected {
        code: ErrorCode,
        json_pointer: Option<String>,
    },
    Admitted {
        canonical_sha256: [u8; 32],
        normalized_sha256: [u8; 32],
        report: Vec<(&'static str, String)>,
        render: RenderSummary,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderSummary {
    Artifact([u8; 32]),
    Failure(ErrorCode),
}

/// What the generator knows about a case beyond the generic contract.
#[derive(Clone, Debug)]
enum Expect {
    Any,
    Admitted,
    /// Admitted, and the canonicalized original is exactly these bytes.
    Canonical(Vec<u8>),
    Rejected {
        code: ErrorCode,
        json_pointer: PointerExpectation,
    },
}

/// What a generated rejection's pointer must be.
#[derive(Clone, Debug)]
enum PointerExpectation {
    Unchecked,
    Absent,
    Exactly(String),
}

struct GeneratedCase {
    input: Vec<u8>,
    expect: Expect,
}

#[test]
fn fixed_seed_scene_fuzz_smoke_is_bounded_and_deterministic() {
    assert_scene_fuzz_smoke();
}

#[test]
fn scene_fuzz_regression_corpus_is_manifested_and_replayed() {
    assert_scene_fuzz_regression_corpus();
}

#[test]
fn parser_depth_ceiling_is_exact_around_the_boundary() {
    // The root object is depth 1 and `elements` depth 2, so `nested` arrays
    // inside it reach depth `2 + nested`; the ceiling admits exactly 128.
    for nested in 120..=135 {
        let input = nested_arrays_scene(nested);
        let outcome = replay(&input, &format!("nested {nested}"));
        if 2 + nested <= 128 {
            assert!(
                matches!(outcome, Outcome::Admitted { .. }),
                "nesting {nested} is within the depth ceiling: {outcome:?}"
            );
        } else {
            assert!(
                matches!(
                    outcome,
                    Outcome::Rejected {
                        code: ErrorCode::SCENE_JSON_DEPTH_EXCEEDED,
                        ..
                    }
                ),
                "nesting {nested} crosses the depth ceiling: {outcome:?}"
            );
        }
    }
}

#[test]
fn extreme_nesting_is_rejected_at_the_ceiling_without_exhausting_the_stack() {
    // Recursion is bounded by the depth ceiling, not by the input: sixty
    // thousand opening brackets return the same typed rejection as 129.
    for nested in [129, 4_096, MAX_NESTING] {
        let input = nested_arrays_scene(nested);
        let outcome = replay(&input, &format!("nested {nested}"));
        let Outcome::Rejected { code, json_pointer } = outcome else {
            panic!("nesting {nested} was admitted");
        };
        assert_eq!(code, ErrorCode::SCENE_JSON_DEPTH_EXCEEDED);
        assert_eq!(
            json_pointer.as_deref(),
            Some(nested_arrays_scene_pointer(127).as_str()),
            "nesting {nested} is rejected at the first container past the ceiling"
        );
    }
}

#[test]
fn external_resource_references_are_refused_without_being_read() {
    // A file record is the only place a scene document can name something
    // outside its own bytes. Both a filesystem and a network reference are
    // refused with the typed diagnostic (spec §13.3: public operations never
    // "read external resources"), and the render still completes.
    for data_url in ["file:///etc/hostname", "https://example.com/a.png"] {
        let source = format!(
            r#"{{"type":"excalidraw","version":2,"elements":[{{"id":"i","type":"image","width":10,"height":10,"fileId":"f","seed":1,"updated":1}}],"appState":{{}},"files":{{"f":{{"id":"f","mimeType":"image/png","dataURL":"{data_url}","created":1}}}}}}"#
        );
        let Outcome::Admitted { report, render, .. } = replay(source.as_bytes(), data_url) else {
            panic!("{data_url}: the scene is admitted");
        };
        assert!(
            report
                .iter()
                .any(|(code, pointer)| *code == "image.forbidden-resource"
                    && pointer == "/files/f/dataURL"),
            "{data_url}: the reference is refused: {report:?}"
        );
        assert!(
            matches!(render, RenderSummary::Artifact(_)),
            "{data_url}: the artifact is still meaningful: {render:?}"
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "scheduled deterministic fuzz expansion"]
fn nightly_scene_fuzz_is_bounded_and_deterministic() {
    let cases = std::env::var("EXCALIDRAW_SCENE_FUZZ_CASES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(4_096)
        .min(16_384);
    assert_generated_cases(crate::fuzz_seed::nightly_seed(FIXED_SEED), cases);
}

pub(crate) fn assert_scene_fuzz_smoke() {
    assert_generated_cases(FIXED_SEED, PR_CASES);
}

fn assert_generated_cases(seed: u64, cases: usize) {
    let mut generator = Generator(seed);
    for index in 0..cases {
        let case = generator.case(index);
        let id = format!("seed {seed:016x} case {index}");
        let first = replay(&case.input, &id);
        let second = replay(&case.input, &id);
        assert_eq!(
            first,
            second,
            "{id} is not deterministic\n{}",
            render_input(&case.input)
        );
        assert_expectation(&case, &first, &id);
    }
}

fn assert_expectation(case: &GeneratedCase, outcome: &Outcome, id: &str) {
    let shown = || render_input(&case.input);
    match (&case.expect, outcome) {
        (Expect::Any, _) => {}
        (Expect::Admitted, Outcome::Admitted { .. }) => {}
        (Expect::Canonical(expected), Outcome::Admitted { .. }) => {
            let scene = SceneDocument::parse(&case.input).expect("admitted case re-admits");
            let canonical = scene
                .canonicalized_original_bytes()
                .expect("canonical serialization allocates");
            assert_eq!(
                String::from_utf8_lossy(&canonical),
                String::from_utf8_lossy(expected),
                "{id} canonicalized original differs from the compact spelling\n{}",
                shown()
            );
        }
        (
            Expect::Rejected { code, json_pointer },
            Outcome::Rejected {
                code: actual_code,
                json_pointer: actual_pointer,
            },
        ) => {
            assert_eq!(
                actual_code,
                code,
                "{id} rejected with the wrong code\n{}",
                shown()
            );
            let expected_pointer = match json_pointer {
                PointerExpectation::Unchecked => None,
                PointerExpectation::Absent => Some(None),
                PointerExpectation::Exactly(pointer) => Some(Some(pointer.as_str())),
            };
            if let Some(expected_pointer) = expected_pointer {
                assert_eq!(
                    actual_pointer.as_deref(),
                    expected_pointer,
                    "{id} rejected at the wrong pointer\n{}",
                    shown()
                );
            }
        }
        (expect, outcome) => panic!(
            "{id} expected {expect:?} but produced {outcome:?}\n{}",
            shown()
        ),
    }
}

/// Shows an input for reproduction: as text when it is UTF-8, else as hex.
fn render_input(input: &[u8]) -> String {
    match std::str::from_utf8(input) {
        Ok(text) if text.len() <= 8_192 => format!("input text: {text}"),
        Ok(text) => format!(
            "input text ({} bytes, first 8192 shown): {}",
            text.len(),
            &text[..text.floor_char_boundary(8_192)]
        ),
        Err(_) => format!("input hex: {}", encode_hex(input)),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn replay(input: &[u8], id: &str) -> Outcome {
    std::panic::catch_unwind(|| exercise(input)).unwrap_or_else(|_| {
        panic!(
            "{id} panicked instead of returning a typed outcome\n{}",
            render_input(input)
        )
    })
}

#[cfg(target_arch = "wasm32")]
fn replay(input: &[u8], _id: &str) -> Outcome {
    // A panic becomes a browser-WASM test trap and fails the smoke directly.
    exercise(input)
}

/// Runs one input through admission, preservation, normalization, and the
/// public SVG facade, asserting every property along the way.
fn exercise(input: &[u8]) -> Outcome {
    let input = &input[..input.len().min(MAX_FUZZ_INPUT_BYTES)];
    let scene = match SceneDocument::parse(input) {
        Ok(scene) => scene,
        Err(error) => {
            assert_rejection_contract(&error);
            return Outcome::Rejected {
                code: error.code(),
                json_pointer: error.json_pointer().map(str::to_owned),
            };
        }
    };

    let canonical = assert_canonical_preservation(input, &scene);
    let element_count = scene.element_occurrence_count();

    let mut svg_view = None;
    for format in [OutputFormat::Svg, OutputFormat::Png, OutputFormat::Pdf] {
        let first = normalize(&scene, format);
        let second = normalize(&scene, format);
        assert_eq!(
            first.fingerprint, second.fingerprint,
            "{format:?} normalization is not deterministic"
        );
        assert_eq!(
            first.events, second.events,
            "{format:?} normalization events are not deterministic"
        );
        assert_normalization_order(&first.events, format);
        assert_source_order(
            first.events.iter().filter_map(|event| match event {
                ObservedEvent::Diagnostic { json_pointer, .. } => Some(json_pointer.as_str()),
                _ => None,
            }),
            &scene,
            &format!("{format:?} normalization"),
        );
        assert_paint_order(&first.outcome, element_count, format);
        assert_diagnostic_locations(&first.events, &scene, format);
        assert_manifest_dispositions(&first.events, &scene, format);
        assert_normalization_is_idempotent(&scene, &first, format);
        if format == OutputFormat::Svg {
            svg_view = Some(first);
        }
    }
    let svg_view = svg_view.expect("the SVG rendering view was normalized");
    let (report, render) = assert_render_determinism(&scene);

    Outcome::Admitted {
        canonical_sha256: Sha256::digest(&canonical).into(),
        normalized_sha256: Sha256::digest(svg_view.fingerprint.as_bytes()).into(),
        report,
        render,
    }
}

/// A rejection names a `scene.*` code, carries a well-formed pointer when it
/// carries one, and describes a parser-ceiling crossing exactly.
fn assert_rejection_contract(error: &SceneParseError) {
    let code = error.code();
    assert!(
        code.as_str().starts_with("scene."),
        "admission failed outside the scene error family: {code}"
    );
    if let Some(pointer) = error.json_pointer() {
        assert_pointer_syntax(pointer);
    }
    if code == ErrorCode::SCENE_INVALID_ENVELOPE {
        let pointer = error
            .json_pointer()
            .expect("an envelope rejection locates the failing member");
        assert!(
            ENVELOPE_POINTERS.contains(&pointer),
            "envelope rejection at an unexpected pointer {pointer:?}"
        );
    }
    let ceiling = PARSER_CEILINGS
        .iter()
        .any(|(ceiling_code, _, _)| *ceiling_code == code);
    match error.details() {
        Some(DiagnosticDetails::ParseLimit {
            field,
            configured,
            measurement,
            ..
        }) => {
            assert!(ceiling, "{code} carries parser-ceiling details");
            let expected = PARSER_CEILINGS
                .iter()
                .find(|(ceiling_code, ceiling_field, _)| {
                    *ceiling_code == code && ceiling_field == field
                })
                .unwrap_or_else(|| panic!("{code} names an unmanifested ceiling field {field}"));
            assert_eq!(
                *configured, expected.2,
                "{code} misreports the fixed ceiling"
            );
            let measured = match measurement {
                Measurement::Attempted { value } | Measurement::AtLeast { value } => *value,
                #[allow(unreachable_patterns)]
                _ => panic!("{code} carries an unknown measurement form"),
            };
            assert!(
                measured > *configured,
                "{code} measured {measured} within its ceiling {configured}"
            );
        }
        Some(details) => panic!("{code} carries unexpected details {details:?}"),
        None => assert!(!ceiling, "{code} is a ceiling rejection without details"),
    }
    let descriptor = error.descriptor();
    assert_eq!(descriptor.code(), code);
}

/// The canonicalized original is a fixpoint of admission and reads the same
/// as the source under an independent JSON reader.
fn assert_canonical_preservation(input: &[u8], scene: &SceneDocument) -> Vec<u8> {
    let canonical = scene
        .canonicalized_original_bytes()
        .expect("canonical serialization allocates");
    let mut visited = Vec::new();
    scene.visit_canonicalized_original(&mut |chunk| visited.extend_from_slice(chunk));
    assert_eq!(
        visited, canonical,
        "chunked canonical visit differs from the owned bytes"
    );
    let reparsed = SceneDocument::parse(&canonical).unwrap_or_else(|error| {
        panic!(
            "canonicalized original is not re-admitted: {error} at {:?}",
            error.json_pointer()
        )
    });
    let again = reparsed
        .canonicalized_original_bytes()
        .expect("canonical serialization allocates");
    assert_eq!(
        String::from_utf8_lossy(&again),
        String::from_utf8_lossy(&canonical),
        "canonicalized original is not a fixpoint"
    );
    assert!(
        canonical.len() <= input.len(),
        "canonicalized original grew beyond its source"
    );
    let canonical_text = std::str::from_utf8(&canonical).expect("canonical bytes are UTF-8");
    assert!(
        !canonical_text
            .chars()
            .any(|character| matches!(character, '\u{0000}'..='\u{001f}')),
        "canonicalized original spells a control character directly"
    );

    match (
        serde_json::from_slice::<serde_json::Value>(input),
        serde_json::from_slice::<serde_json::Value>(&canonical),
    ) {
        (Ok(source), Ok(canonical_value)) => assert_eq!(
            source, canonical_value,
            "canonicalized original changed a value under an independent reader"
        ),
        (Err(source_error), Err(canonical_error)) => {
            assert!(
                is_recursion_limit(&source_error) && is_recursion_limit(&canonical_error),
                "independent reader rejected an admitted document: {source_error}; {canonical_error}"
            );
        }
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => {
            panic!("independent reader disagreed with itself on canonicalization: {error}")
        }
    }
    canonical
}

fn is_recursion_limit(error: &serde_json::Error) -> bool {
    error.is_syntax() && error.to_string().contains("recursion limit")
}

/// One observed normalization event, with everything the ordering contract
/// keys on.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ObservedEvent {
    Preflight {
        event: &'static str,
        json_pointer: String,
        working_bytes: u64,
        work_units: u64,
    },
    Commit {
        event: &'static str,
        json_pointer: String,
        element_id: Option<String>,
        local_owner: Option<String>,
        working_bytes: u64,
        work_units: u64,
    },
    Diagnostic {
        source_order: u64,
        source_position: u64,
        json_pointer: String,
        code: &'static str,
        element_id: Option<String>,
        local_owner: Option<String>,
    },
}

struct Normalized {
    outcome: NormalizationOutcome,
    events: Vec<ObservedEvent>,
    fingerprint: String,
}

fn normalize(scene: &SceneDocument, format: OutputFormat) -> Normalized {
    let mut events = Vec::new();
    let outcome = normalize_scene(scene, CompatibilityRevision::REVISION_1, format, |event| {
        events.push(match event {
            NormalizationEvent::PreflightRecord {
                event,
                json_pointer,
                working_bytes,
                work_units,
            } => ObservedEvent::Preflight {
                event,
                json_pointer: json_pointer.to_owned(),
                working_bytes,
                work_units,
            },
            NormalizationEvent::CommitRecord {
                event,
                element_id,
                local_owner,
                json_pointer,
                working_bytes,
                work_units,
            } => ObservedEvent::Commit {
                event,
                json_pointer: json_pointer.to_owned(),
                element_id: element_id.map(str::to_owned),
                local_owner: local_owner.map(str::to_owned),
                working_bytes,
                work_units,
            },
            NormalizationEvent::Diagnostic {
                diagnostic,
                local_owner,
                source_order,
                source_position,
            } => ObservedEvent::Diagnostic {
                source_order,
                source_position,
                json_pointer: diagnostic.json_pointer().to_owned(),
                code: diagnostic.code().as_str(),
                element_id: diagnostic.element_id().map(str::to_owned),
                local_owner: local_owner.map(str::to_owned),
            },
        });
        Ok::<_, ()>(())
    })
    .unwrap_or_else(|error| panic!("{format:?} normalization failed: {error:?}"))
    .expect("revision 1 is in the implemented normalization slice");
    let fingerprint = format!("{outcome:?}\n{events:?}");
    Normalized {
        outcome,
        events,
        fingerprint,
    }
}

/// Diagnostics replay in the total order of spec §11.3: owner source order,
/// then source member position, then pointer bytes, then code bytes. Records
/// for an owner precede its diagnostics.
fn assert_normalization_order(events: &[ObservedEvent], format: OutputFormat) {
    let mut previous: Option<(u64, u64, &str, &str)> = None;
    let mut previous_index = 0;
    for (index, event) in events.iter().enumerate() {
        let ObservedEvent::Diagnostic {
            source_order,
            source_position,
            json_pointer,
            code,
            ..
        } = event
        else {
            continue;
        };
        let key = (
            *source_order,
            *source_position,
            json_pointer.as_str(),
            *code,
        );
        if let Some(previous) = previous {
            assert!(
                previous <= key,
                "{format:?} diagnostic {index} {key:?} replays before {previous:?} at {previous_index}"
            );
        }
        previous = Some(key);
        previous_index = index;
    }

    // Within one owner, every commit record precedes the owner's diagnostics.
    // Element commit records carry their owner through the `/elements/N`
    // pointer; the owner order of `elements` itself is the high half of any
    // element diagnostic's owner order.
    let elements_root = events.iter().find_map(|event| match event {
        ObservedEvent::Diagnostic {
            source_order,
            json_pointer,
            ..
        } if element_index(json_pointer).is_some() => Some(source_order >> 32),
        _ => None,
    });
    let mut diagnosed_owners = BTreeSet::new();
    for event in events {
        match event {
            ObservedEvent::Diagnostic { source_order, .. } => {
                diagnosed_owners.insert(*source_order);
            }
            ObservedEvent::Commit { json_pointer, .. } => {
                if let (Some(index), Some(root)) = (element_index(json_pointer), elements_root) {
                    let owner = (root << 32) | u64::try_from(index).unwrap_or(u64::MAX);
                    assert!(
                        !diagnosed_owners.contains(&owner),
                        "{format:?} element {index} commit record replays after its diagnostics"
                    );
                }
            }
            ObservedEvent::Preflight { .. } => {}
        }
    }
}

/// The element index a `/elements/N...` pointer names.
fn element_index(pointer: &str) -> Option<usize> {
    let rest = pointer.strip_prefix("/elements/")?;
    let digits = rest.split('/').next()?;
    digits.parse().ok()
}

/// Normalized elements keep source paint order and never alias a fallback.
fn assert_paint_order(outcome: &NormalizationOutcome, element_count: usize, format: OutputFormat) {
    let mut seen = BTreeSet::new();
    let mut previous_index = None;
    let mut previous_order = None;
    for element in &outcome.elements {
        let index = element.source.source_index();
        assert!(
            index < element_count,
            "{format:?} element index {index} is out of range"
        );
        assert!(
            previous_index.is_none_or(|previous| previous < index),
            "{format:?} element {index} breaks strictly ascending source order"
        );
        assert!(
            previous_order.is_none_or(|previous| previous <= element.source_order),
            "{format:?} element {index} breaks non-decreasing owner order"
        );
        assert_eq!(
            element.source.pointer(),
            format!("/elements/{index}"),
            "{format:?} element pointer does not name its source index"
        );
        assert!(
            seen.insert(index),
            "{format:?} element {index} normalized twice"
        );
        assert!(
            !element.source.id().is_empty(),
            "{format:?} element {index} has an empty normalized identity"
        );
        assert_finite_element(element, format);
        previous_index = Some(index);
        previous_order = Some(element.source_order);
    }
    let mut ids = BTreeSet::new();
    for element in &outcome.elements {
        assert!(
            ids.insert(element.source.id()),
            "{format:?} normalized identity {} is not unique",
            element.source.id()
        );
    }
    for fallback in &outcome.fallbacks {
        assert!(
            fallback.source_index < element_count,
            "{format:?} fallback index {} is out of range",
            fallback.source_index
        );
        assert!(
            !seen.contains(&fallback.source_index),
            "{format:?} element {} is both normalized and a fallback",
            fallback.source_index
        );
    }
    let mut previous_index = None;
    for index in outcome
        .elements
        .iter()
        .map(|element| element.index.as_str())
    {
        assert!(
            !index.is_empty(),
            "{format:?} normalized fractional index is empty"
        );
        assert!(
            previous_index.is_none_or(|previous: &str| previous < index),
            "{format:?} normalized fractional indexes are not strictly ascending: {previous_index:?} then {index:?}"
        );
        previous_index = Some(index);
    }
}

fn assert_finite_element(element: &NormalizedElement, format: OutputFormat) {
    let id = element.source.id();
    for (name, value) in [
        ("x", element.x),
        ("y", element.y),
        ("width", element.width),
        ("height", element.height),
        ("angle", element.angle),
        ("strokeWidth", element.stroke_width),
        ("roughness", element.roughness),
        ("opacity", element.opacity),
    ] {
        assert!(
            value.is_finite(),
            "{format:?} element {id} normalized {name} is not finite: {value}"
        );
    }
    assert!(
        element.width >= 0.0 && element.height >= 0.0,
        "{format:?} element {id} has negative normalized dimensions"
    );
    assert!(
        element.seed != 0,
        "{format:?} element {id} kept a zero Rough seed"
    );
    let points: &[[f64; 2]] = match &element.kind {
        NormalizedElementKind::Linear(linear) => {
            assert!(
                linear.points.len() >= 2,
                "{format:?} element {id} has fewer than two linear points"
            );
            assert_eq!(
                linear.points[0],
                [0.0, 0.0],
                "{format:?} element {id} first linear point is not the local origin"
            );
            &linear.points
        }
        NormalizedElementKind::Freedraw(freedraw) => &freedraw.points,
        _ => &[],
    };
    for point in points {
        assert!(
            point[0].is_finite() && point[1].is_finite(),
            "{format:?} element {id} normalized a non-finite point"
        );
    }
}

/// Every diagnostic pointer is well formed and names a location that exists
/// in the scene, or a missing member of a record that exists; element
/// attributions name the source identity or a generated one.
fn assert_diagnostic_locations(
    events: &[ObservedEvent],
    scene: &SceneDocument,
    format: OutputFormat,
) {
    for event in events {
        let (pointer, element_id) = match event {
            ObservedEvent::Diagnostic {
                json_pointer,
                element_id,
                ..
            }
            | ObservedEvent::Commit {
                json_pointer,
                element_id,
                ..
            } => (json_pointer.as_str(), element_id.as_deref()),
            ObservedEvent::Preflight { json_pointer, .. } => (json_pointer.as_str(), None),
        };
        assert_pointer_syntax(pointer);
        assert!(
            pointer_locates(scene, pointer),
            "{format:?} pointer {pointer:?} names nothing in the scene"
        );
        if let (Some(element_id), Some(index)) = (element_id, element_index(pointer)) {
            assert_element_attribution(scene, index, element_id, pointer, format);
        }
    }
}

fn assert_element_attribution(
    scene: &SceneDocument,
    index: usize,
    element_id: &str,
    pointer: &str,
    format: OutputFormat,
) {
    let source_id = source_element_id(scene, index);
    let generated = element_id.strip_prefix("generated-").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    });
    assert!(
        source_id.as_deref() == Some(element_id) || generated,
        "{format:?} diagnostic at {pointer} attributes {element_id:?} to element {index} whose identity is {source_id:?}"
    );
}

/// The `id` string of the element at `index`, whatever its kind.
fn source_element_id(scene: &SceneDocument, index: usize) -> Option<String> {
    let JsonValue::Object(members) = scene.element_value(index)? else {
        return None;
    };
    match member_value(members, "id")? {
        JsonValue::String(id) => Some(id.to_string()),
        _ => None,
    }
}

/// The source member keys of the element at `index`, in source order, when
/// the element is an object.
fn source_element_members(scene: &SceneDocument, index: usize) -> Option<Vec<String>> {
    let JsonValue::Object(members) = scene.element_value(index)? else {
        return None;
    };
    Some(
        members
            .iter()
            .map(|member| member.key.to_string())
            .collect(),
    )
}

fn unescape_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// Where a diagnostic pointer sits in the source document, read from the
/// parsed scene rather than from any ordering key the implementation
/// computed: the root member, the element or file record inside it, and the
/// member of that record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SourceLocation {
    root_position: usize,
    record_index: usize,
    member_position: Option<usize>,
}

fn source_location(scene: &SceneDocument, pointer: &str) -> Option<SourceLocation> {
    let mut segments = pointer.strip_prefix('/')?.split('/');
    let root = unescape_segment(segments.next()?);
    let root_position = scene
        .root_members()
        .iter()
        .position(|member| *member.key == *root)?;
    let member_position_in = |members: &[crate::scene::JsonMember], key: &str| {
        members.iter().position(|member| *member.key == *key)
    };
    match root.as_str() {
        "elements" => {
            let index = segments.next()?.parse::<usize>().ok()?;
            let member = segments.next().map(unescape_segment);
            let JsonValue::Object(members) = scene.element_value(index)? else {
                return Some(SourceLocation {
                    root_position,
                    record_index: index,
                    member_position: None,
                });
            };
            Some(SourceLocation {
                root_position,
                record_index: index,
                member_position: member.and_then(|member| member_position_in(members, &member)),
            })
        }
        "files" => {
            let key = unescape_segment(segments.next()?);
            let member = segments.next().map(unescape_segment);
            let record_index = (0..scene.file_record_count()).find(|index| {
                scene
                    .file_record_at(*index)
                    .is_some_and(|record| record.key() == key)
            })?;
            let record = scene.file_record_at(record_index)?;
            let members = record.members();
            Some(SourceLocation {
                root_position,
                record_index,
                member_position: member.and_then(|member| member_position_in(members?, &member)),
            })
        }
        "appState" => {
            let member = segments.next().map(unescape_segment);
            Some(SourceLocation {
                root_position,
                record_index: 0,
                member_position: member
                    .and_then(|member| member_position_in(scene.app_state_members(), &member)),
            })
        }
        _ => Some(SourceLocation {
            root_position,
            record_index: 0,
            member_position: None,
        }),
    }
}

/// Diagnostics follow the source document: root members in source order,
/// elements and file records by index inside theirs, and members in source
/// order inside each record (spec §11.3: "Original owners use depth-first
/// source order, arrays by index, objects by source member order"). The
/// expectation is read from the parsed scene, not from the owner and position
/// keys the implementation sorts by. Pointers that name no source location
/// (the document root, a missing member) leave the cursor where it was.
fn assert_source_order<'a>(
    pointers: impl IntoIterator<Item = &'a str>,
    scene: &SceneDocument,
    context: &str,
) {
    let mut previous: Option<SourceLocation> = None;
    for pointer in pointers {
        let Some(location) = source_location(scene, pointer) else {
            continue;
        };
        let record = (location.root_position, location.record_index);
        if let Some(previous) = previous {
            let previous_record = (previous.root_position, previous.record_index);
            assert!(
                previous_record <= record,
                "{context} diagnostic at {pointer} precedes an earlier record's diagnostics"
            );
            if let (true, Some(previous_position), Some(position)) = (
                previous_record == record,
                previous.member_position,
                location.member_position,
            ) {
                assert!(
                    previous_position <= position,
                    "{context} diagnostic at {pointer} precedes an earlier source member's diagnostics"
                );
            }
        }
        previous = Some(SourceLocation {
            member_position: location.member_position.or(previous
                .filter(|previous| (previous.root_position, previous.record_index) == record)
                .and_then(|previous| previous.member_position)),
            ..location
        });
    }
}

/// Every unmanifested top-level element member is diagnosed as an unsupported
/// property at its own pointer, and no manifested or inherited-ignored member
/// is, so the scene-surface manifest rather than a property-name heuristic
/// decides participation (issue #49).
fn assert_manifest_dispositions(
    events: &[ObservedEvent],
    scene: &SceneDocument,
    format: OutputFormat,
) {
    let policy = crate::catalog::REVISION_1.scene_surface();
    let unsupported = events
        .iter()
        .filter_map(|event| match event {
            ObservedEvent::Diagnostic {
                code: "compatibility.unsupported-property",
                json_pointer,
                ..
            } => Some(json_pointer.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for index in 0..scene.element_occurrence_count() {
        if scene.element_is_silent(index) {
            continue;
        }
        let Some(members) = source_element_members(scene, index) else {
            continue;
        };
        let Some(JsonValue::Object(element)) = scene.element_value(index) else {
            continue;
        };
        let Some(JsonValue::String(kind)) = member_value(element, "type") else {
            continue;
        };
        let (record_kind, ignored): (&str, &[&str]) = match policy
            .element_kinds
            .iter()
            .find(|element_policy| element_policy.kind == &**kind)
        {
            Some(element_policy) if element_policy.status == "static-export-irrelevant" => {
                continue;
            }
            Some(element_policy) => (
                element_policy.record_kind,
                element_policy.ignored_inherited_paths,
            ),
            None if &**kind == "draw" => ("element.legacy_draw", &[]),
            None => continue,
        };
        for member in &members {
            let mut path = String::from("/");
            push_pointer_segment(&mut path, member);
            let pointer = format!("/elements/{index}{path}");
            let manifested = ignored.contains(&path.as_str())
                || crate::scene_surface::is_manifested_path(policy, record_kind, &path);
            assert_eq!(
                !unsupported.contains(pointer.as_str()),
                manifested,
                "{format:?} member {pointer} manifested={manifested} but its unsupported-property diagnosis disagrees"
            );
        }
    }
}

fn assert_pointer_syntax(pointer: &str) {
    if pointer.is_empty() {
        return;
    }
    assert!(
        pointer.starts_with('/'),
        "pointer {pointer:?} does not start with '/'"
    );
    let bytes = pointer.as_bytes();
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte == b'~' {
            assert!(
                matches!(bytes.get(offset + 1), Some(b'0' | b'1')),
                "pointer {pointer:?} has an unescaped '~'"
            );
        }
    }
}

/// True when `pointer` resolves, or when it names the first absent position
/// of an existing record: any member of an object, or the index just past
/// the end of an array (a point with a missing component). An index further
/// past the end names nothing.
fn pointer_locates(scene: &SceneDocument, pointer: &str) -> bool {
    if scene.json_kind_at(pointer).is_some() {
        return true;
    }
    let Some(split) = pointer.rfind('/') else {
        return false;
    };
    match scene.json_kind_at(&pointer[..split]) {
        Some(JsonKind::Object(_)) => true,
        Some(JsonKind::Array(length)) => pointer[split + 1..].parse::<usize>() == Ok(length),
        _ => false,
    }
}

/// Two fresh public-facade SVG renders agree on the artifact and the complete
/// ordered report, and the report replays in the total diagnostic order.
fn assert_render_determinism(
    scene: &SceneDocument,
) -> (Vec<(&'static str, String)>, RenderSummary) {
    let request = RenderRequest::builder(OutputFormat::Svg)
        .build()
        .expect("default SVG request is valid");
    let first = Renderer::new().render(scene, &request);
    let second = Renderer::new().render(scene, &request);
    let (report, summary) = match (&first, &second) {
        (Ok(first), Ok(second)) => {
            assert_eq!(
                first.artifact().bytes(),
                second.artifact().bytes(),
                "repeated SVG render changed the artifact bytes"
            );
            assert_eq!(
                first.report(),
                second.report(),
                "repeated SVG render changed the report"
            );
            (
                first.report(),
                RenderSummary::Artifact(Sha256::digest(first.artifact().bytes()).into()),
            )
        }
        (Err(first), Err(second)) => {
            assert_eq!(
                first.code(),
                second.code(),
                "repeated SVG render failed differently"
            );
            assert_eq!(
                (first.json_pointer(), first.element_id()),
                (second.json_pointer(), second.element_id()),
                "repeated SVG render failed at a different location"
            );
            assert_eq!(
                first.report(),
                second.report(),
                "repeated SVG render accumulated a different report"
            );
            assert_pointer_syntax(first.json_pointer());
            assert!(
                !first.code().as_str().starts_with("scene."),
                "an admitted scene failed to render with an admission code {}",
                first.code()
            );
            (first.report(), RenderSummary::Failure(first.code()))
        }
        (first, second) => panic!(
            "repeated SVG render changed its outcome kind: {:?} then {:?}",
            first
                .as_ref()
                .map(|outcome| outcome.artifact().bytes().len())
                .map_err(|error| error.code()),
            second
                .as_ref()
                .map(|outcome| outcome.artifact().bytes().len())
                .map_err(|error| error.code()),
        ),
    };

    let mut previous: Option<(u64, u64, &str, &str, u64)> = None;
    for diagnostic in report.diagnostics() {
        assert_pointer_syntax(diagnostic.json_pointer());
        assert!(
            pointer_locates(scene, diagnostic.json_pointer()),
            "report pointer {:?} names nothing in the scene",
            diagnostic.json_pointer()
        );
        assert!(
            !diagnostic.effects().is_empty(),
            "report diagnostic {} has an empty effect list",
            diagnostic.code()
        );
        let key = (
            diagnostic.order().owner_index,
            diagnostic.order().source_position,
            diagnostic.json_pointer(),
            diagnostic.code().as_str(),
            diagnostic.order().retention_ordinal,
        );
        if let Some(previous) = previous {
            assert!(
                previous <= key,
                "report diagnostic {key:?} is ordered before {previous:?}"
            );
        }
        previous = Some(key);
    }
    assert_source_order(
        report
            .diagnostics()
            .iter()
            .map(CompatibilityDiagnostic::json_pointer),
        scene,
        "report",
    );
    let report = report
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code().as_str(),
                diagnostic.json_pointer().to_owned(),
            )
        })
        .collect();
    (report, summary)
}
