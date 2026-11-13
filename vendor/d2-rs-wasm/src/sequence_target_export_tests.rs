// SPDX-License-Identifier: MPL-2.0
//! Complete original target objects; no field projection or exclusions.
use super::*;

pub(super) fn assert_target(expected: &serde_json::Value, actual: &serde_json::Value, path: &str) {
    use serde_json::Value;
    match (expected, actual) {
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(
                a.keys().collect::<std::collections::BTreeSet<_>>(),
                b.keys().collect::<std::collections::BTreeSet<_>>(),
                "{path} keys"
            );
            for (key, value) in a {
                assert_target(value, &b[key], &format!("{path}.{key}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path} inventory");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                assert_target(a, b, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(a), Value::Number(b)) => {
            if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
                assert_eq!(a, b, "{path}");
            } else if let (Some(a), Some(b)) = (a.as_u64(), b.as_u64()) {
                assert_eq!(a, b, "{path}");
            } else {
                assert_eq!(
                    a.as_f64().unwrap().to_bits(),
                    b.as_f64().unwrap().to_bits(),
                    "{path} exact number"
                );
            }
        }
        _ => assert_eq!(expected, actual, "{path}"),
    }
}

#[cfg(feature = "oracle-fixtures")]
#[test]
fn complete_successful_sequence_targets_match_original_go() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/sequence-actor-label-policy-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 128);
    let (mut successful, mut errors, mut panics_open) = (0, 0, 0);
    let mut records = Vec::new();
    for case in cases {
        let expected = &case["expected"];
        if expected["panic"] == true {
            panics_open += 1;
            continue;
        }
        let actual = compile_d2_target_json_impl(
            case["source"].as_str().unwrap(),
            r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
        );
        if let Some(error) = expected["error"].as_str() {
            assert_eq!(actual.err().as_deref(), Some(error), "{}", case["name"]);
            errors += 1;
            continue;
        }
        let actual: serde_json::Value = serde_json::from_str(&actual.unwrap()).unwrap();
        assert_target(
            &expected["diagram"],
            &actual["diagram"],
            case["name"].as_str().unwrap(),
        );
        successful += 1;
        records.push(serde_json::json!({"name":case["name"],"diagram":actual["diagram"]}));
    }
    assert_eq!((successful, errors, panics_open), (100, 16, 12));
    if let Ok(path) = std::env::var("D2RS_SEQUENCE_TARGET_EXPORT_RECORDS") {
        std::fs::write(path,serde_json::to_vec_pretty(&serde_json::json!({"complete_diagrams":successful,"errors":errors,"source_panics_open":panics_open,"whole_scope_passed":false,"records":records})).unwrap()).unwrap();
    }
}

#[cfg(feature = "oracle-fixtures")]
#[test]
fn complete_expanded_target_objects_match_original_go() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/sequence-target-export-expanded-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 44);
    let (mut successes, mut errors) = (0, 0);
    for case in cases {
        let expected = &case["expected"];
        assert_eq!(expected["panic"], false);
        let actual = compile_d2_target_json_impl(
            case["source"].as_str().unwrap(),
            r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
        );
        if let Some(error) = expected["error"].as_str() {
            assert_eq!(actual.err().as_deref(), Some(error), "{}", case["name"]);
            errors += 1;
        } else {
            let actual: serde_json::Value = serde_json::from_str(&actual.unwrap()).unwrap();
            assert_target(
                &expected["diagram"],
                &actual["diagram"],
                case["name"].as_str().unwrap(),
            );
            successes += 1;
        }
    }
    assert_eq!((successes, errors), (42, 2));
}

#[cfg(feature = "oracle-fixtures")]
#[test]
fn grid_extraction_preserves_complete_source_target_and_label_owners() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/target-projection-label-source-v090.json"
    ))
    .unwrap();
    let actual = compile_d2_target_json_impl(
        fixture["source"].as_str().unwrap(),
        r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
    )
    .unwrap();
    let actual: serde_json::Value = serde_json::from_str(&actual).unwrap();
    assert_target(
        &fixture["expected"],
        &actual["diagram"],
        "flipt full target",
    );
}
