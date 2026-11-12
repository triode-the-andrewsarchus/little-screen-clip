// SPDX-License-Identifier: MPL-2.0
//! Full source targets and recovered source faults through native API compilation.
use super::*;

#[test]
fn malformed_math_faults_preserve_complete_successes_and_next_compile() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::load_str(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-math"),
        "tests/fixtures/source-nil-child-v090.json",
    ))
    .unwrap();
    let (mut faults, mut successes) = (0, 0);
    for case in fixture["cases"].as_array().unwrap() {
        let actual = compile_d2_target_json_impl(
            case["source"].as_str().unwrap(),
            r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
        );
        if !case["source_error"].is_null() {
            let message = actual.unwrap_err();
            assert_eq!(
                message.lines().next(),
                case["source_error"]["message"]
                    .as_str()
                    .unwrap()
                    .lines()
                    .next(),
                "{}",
                case["name"]
            );
            assert!(
                message.contains("native source context: mathjax-go/internal/svg/renderer.wrap:61")
            );
            assert_ne!(message, case["source_error"]["message"].as_str().unwrap());
            faults += 1;
        } else {
            let actual: serde_json::Value = serde_json::from_str(&actual.unwrap()).unwrap();
            super::sequence_target_export_tests::assert_target(
                &case["expected_diagram"],
                &actual["diagram"],
                case["name"].as_str().unwrap(),
            );
            successes += 1;
        }
    }
    // The fixture records the pinned Go inventory; do not transcribe the
    // v0.1.0 97/51 split after master changed it to 0/148 (brief Update 1).
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 148);
    let expected_faults = cases
        .iter()
        .filter(|case| !case["source_error"].is_null())
        .count();
    assert_eq!(
        (faults, successes),
        (expected_faults, cases.len() - expected_faults)
    );
    let healthy = compile_d2_target_json_impl("a -> b\n", r#"{"engine":"dagre"}"#).unwrap();
    let healthy: serde_json::Value = serde_json::from_str(&healthy).unwrap();
    assert_eq!(healthy["diagram"]["shapes"].as_array().unwrap().len(), 2);
}
