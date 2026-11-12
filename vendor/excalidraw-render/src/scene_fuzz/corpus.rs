//! The permanent minimized regression corpus: every confirmed fuzz failure,
//! replayed through the same seam on every target.

use std::collections::BTreeSet;

use serde::Deserialize;

use super::{
    replay, Outcome, RenderSummary, FIXED_SEED, MAX_ELEMENTS, MAX_FUZZ_INPUT_BYTES, MAX_MUTATIONS,
    MAX_NESTING, MAX_POINTS,
};

const REGRESSION_SCHEMA: &str = "excalidraw-render/scene-fuzz-regressions/v1";
const REQUIRED_RISKS: [Risk; 10] = [
    Risk::MalformedJson,
    Risk::InvalidUtf8,
    Risk::Envelope,
    Risk::DuplicateMember,
    Risk::ParserCeiling,
    Risk::CanonicalPreservation,
    Risk::NormalizationRepair,
    Risk::SurfacePolicy,
    Risk::PointerEscaping,
    Risk::PublicFacadeRender,
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegressionCorpus {
    schema: String,
    compatibility_revision: u64,
    originating_issue: u64,
    broader_fuzz_issue: u64,
    fixed_seed_hex: String,
    bounds: Bounds,
    cases: Vec<RegressionCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Bounds {
    max_input_bytes: usize,
    max_elements: usize,
    max_points: usize,
    max_mutations: usize,
    max_nesting: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegressionCase {
    id: String,
    risk: Risk,
    input: RegressionInput,
    minimized: bool,
    expected: Expected,
}

/// A scene document spelled as text when it is UTF-8, else as lowercase hex.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum RegressionInput {
    Text(String),
    Hex(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
enum Risk {
    MalformedJson,
    InvalidUtf8,
    Envelope,
    DuplicateMember,
    ParserCeiling,
    CanonicalPreservation,
    NormalizationRepair,
    SurfacePolicy,
    PointerEscaping,
    PublicFacadeRender,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Expected {
    #[serde(rename_all = "camelCase")]
    Rejected {
        code: String,
        json_pointer: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Admitted {
        canonical_sha256: String,
        report: Vec<ExpectedDiagnostic>,
        render: ExpectedRender,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedDiagnostic {
    code: String,
    json_pointer: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedRender {
    Artifact,
    Failure { code: String },
}

/// Replays every manifested case and requires its pinned admission outcome:
/// the canonicalized original digest and the ordered `(code, pointer)` report
/// through the public SVG facade. The normalized-view fingerprint is compared
/// between replays but not pinned here, because it is a `Debug` rendering of
/// the private normalized types rather than a contract.
pub(crate) fn assert_scene_fuzz_regression_corpus() {
    let corpus: RegressionCorpus = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/qualification/scene-fuzz-regressions.json"
    )))
    .expect("the scene fuzz regression corpus is valid JSON");
    assert_eq!(corpus.schema, REGRESSION_SCHEMA);
    assert_eq!(corpus.compatibility_revision, 1);
    assert_eq!(corpus.originating_issue, 81);
    assert_eq!(corpus.broader_fuzz_issue, 83);
    assert_eq!(corpus.fixed_seed_hex, format!("{FIXED_SEED:016x}"));
    assert_eq!(corpus.bounds.max_input_bytes, MAX_FUZZ_INPUT_BYTES);
    assert_eq!(corpus.bounds.max_elements, MAX_ELEMENTS);
    assert_eq!(corpus.bounds.max_points, MAX_POINTS);
    assert_eq!(corpus.bounds.max_mutations, MAX_MUTATIONS);
    assert_eq!(corpus.bounds.max_nesting, MAX_NESTING);

    let mut ids = BTreeSet::new();
    let mut risks = BTreeSet::new();
    for case in corpus.cases {
        assert!(
            ids.insert(case.id.clone()),
            "duplicate regression id {}",
            case.id
        );
        assert!(REQUIRED_RISKS.contains(&case.risk));
        risks.insert(case.risk);
        assert!(case.minimized, "{} is not minimized", case.id);
        let input = match &case.input {
            RegressionInput::Text(text) => text.as_bytes().to_vec(),
            RegressionInput::Hex(hex) => {
                let bytes = decode_hex(hex).expect("regression input is lowercase hex");
                assert!(
                    std::str::from_utf8(&bytes).is_err(),
                    "{} spells UTF-8 input as hex",
                    case.id
                );
                bytes
            }
        };
        assert!(
            input.len() <= MAX_FUZZ_INPUT_BYTES,
            "{} exceeds the input bound",
            case.id
        );
        let first = replay(&input, &case.id);
        let second = replay(&input, &case.id);
        assert_eq!(first, second, "{} is not deterministic", case.id);
        match (&case.expected, &first) {
            (
                Expected::Rejected { code, json_pointer },
                Outcome::Rejected {
                    code: actual_code,
                    json_pointer: actual_pointer,
                },
            ) => {
                assert_eq!(
                    actual_code.as_str(),
                    code,
                    "{} rejected with the wrong code",
                    case.id
                );
                assert_eq!(
                    actual_pointer, json_pointer,
                    "{} rejected at the wrong pointer",
                    case.id
                );
            }
            (
                Expected::Admitted {
                    canonical_sha256,
                    report,
                    render,
                },
                Outcome::Admitted {
                    canonical_sha256: actual_canonical,
                    report: actual_report,
                    render: actual_render,
                    ..
                },
            ) => {
                assert_eq!(
                    encode_hex(actual_canonical),
                    *canonical_sha256,
                    "{} canonicalized differently",
                    case.id
                );
                let expected_report = report
                    .iter()
                    .map(|diagnostic| (diagnostic.code.as_str(), diagnostic.json_pointer.as_str()))
                    .collect::<Vec<_>>();
                let actual_report = actual_report
                    .iter()
                    .map(|(code, pointer)| (*code, pointer.as_str()))
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual_report, expected_report,
                    "{} reported differently",
                    case.id
                );
                match (render, actual_render) {
                    (ExpectedRender::Artifact, RenderSummary::Artifact(_)) => {}
                    (ExpectedRender::Failure { code }, RenderSummary::Failure(actual)) => {
                        assert_eq!(
                            actual.as_str(),
                            code,
                            "{} failed to render differently",
                            case.id
                        );
                    }
                    (_, actual) => {
                        panic!("{} rendered with the wrong outcome: {actual:?}", case.id)
                    }
                }
            }
            (_, actual) => panic!("{} returned the wrong outcome: {actual:?}", case.id),
        }
    }
    assert_eq!(
        risks,
        REQUIRED_RISKS.into_iter().collect(),
        "the regression corpus does not cover every required risk"
    );
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(value.get(offset..offset + 2)?, 16).ok())
        .collect()
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
