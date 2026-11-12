// SPDX-License-Identifier: MPL-2.0
//! Recovered source fault behavior; source runtime stack strings remain open.
use super::*;

#[test]
fn source_nil_pointer_faults_reject_without_poisoning_next_compile() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/sequence-api-fault-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 12);
    let mut records = Vec::new();
    for case in cases {
        assert_eq!(case["source_raw_error"]["code"], 500);
        let source_message = case["source_public_error"]["message"].as_str().unwrap();
        let actual = compile_d2_target_json_impl(
            case["source"].as_str().unwrap(),
            r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
        )
        .unwrap_err();
        assert_eq!(
            actual.lines().next(),
            source_message.lines().next(),
            "{}",
            case["name"]
        );
        assert!(
            actual
                .contains("native source context: d2sequence.sequenceDiagram.addLifelineEdges:425")
        );
        // Native context is deliberately distinguished from the Go runtime
        // backtrace. Do not count this as full diagnostic-string parity.
        assert_ne!(actual, source_message);
        let healthy = compile_d2_target_json_impl(
            "a -> b\n",
            r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
        )
        .unwrap();
        let healthy: serde_json::Value = serde_json::from_str(&healthy).unwrap();
        assert_eq!(healthy["diagram"]["shapes"].as_array().unwrap().len(), 2);
        records.push(serde_json::json!({"name":case["name"],"error":actual,"healthy_diagram":healthy["diagram"],"full_message_exact":false}));
    }
    if let Ok(path) = std::env::var("D2RS_SEQUENCE_FAULT_RECORDS") {
        std::fs::write(path, serde_json::to_vec_pretty(&records).unwrap()).unwrap();
    }
}

#[test]
fn grid_actor_faults_and_complete_successes_follow_original_go() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/sequence-grid-fault-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 24);
    let (mut successes, mut faults) = (0, 0);
    for case in cases {
        let expected = &case["expected"];
        let actual = compile_d2_target_json_impl(
            case["source"].as_str().unwrap(),
            r#"{"path":"index.d2","engine":"dagre","utf16":true}"#,
        );
        if expected["panic"] == true {
            assert!(actual.unwrap_err().starts_with("panic recovered: runtime error: invalid memory address or nil pointer dereference\n"),"{}",case["name"]);
            faults += 1;
        } else {
            let actual: serde_json::Value = serde_json::from_str(&actual.unwrap()).unwrap();
            super::sequence_target_export_tests::assert_target(
                &expected["diagram"],
                &actual["diagram"],
                case["name"].as_str().unwrap(),
            );
            successes += 1;
        }
    }
    assert_eq!((successes, faults), (16, 8));
}
