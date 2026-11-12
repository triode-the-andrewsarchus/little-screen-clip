// SPDX-License-Identifier: MPL-2.0
//! Synthetic-root object identity and complete original target publication.
use super::*;
use base64::engine::general_purpose::STANDARD;

#[test]
fn root_structured_targets_and_native_helper_diagnostics_follow_source() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/root-structured-source-v090.json"
    ))
    .unwrap();
    let (mut successful, mut errors) = (0, 0);
    for case in fixture["cases"].as_array().unwrap() {
        let options =
            serde_json::json!({"path":"index.d2","engine":case["engine"],"utf16":true}).to_string();
        let actual = compile_d2_target_json_impl(case["source"].as_str().unwrap(), &options);
        let expected = &case["response"];
        if expected["error"].is_object() {
            // The native helper returns compiler text. The original standard
            // Go WASM facade serializes diagnostic objects into its message;
            // that separate public transport is not normalized into a match.
            let diagnostics: serde_json::Value =
                serde_json::from_str(expected["error"]["message"].as_str().unwrap()).unwrap();
            let text = diagnostics
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["errmsg"].as_str().unwrap())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(actual.unwrap_err(), text, "{}", case["name"]);
            errors += 1;
        } else {
            let actual: serde_json::Value = serde_json::from_str(&actual.unwrap()).unwrap();
            super::sequence_target_export_tests::assert_target(
                &expected["data"]["diagram"],
                &actual["diagram"],
                case["name"].as_str().unwrap(),
            );
            successful += 1;
        }
    }
    assert_eq!((successful, errors), (95, 30));
}

#[test]
fn root_connection_render_faults_follow_successful_compile() {
    // Preserve the v0.9.0 inputs; master's target admission now rejects these
    // malformed routes before the old renderer panic. Compare the full current
    // Go result rather than retaining obsolete panic/context expectations.
    let master: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/render-regressions-master.json"
    ))
    .unwrap();
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/root-structured-source-v090.json"
    ))
    .unwrap();
    let (mut faults, mut successful) = (0, 0);
    for case in fixture["cases"].as_array().unwrap().iter() {
        if case["response"]["error"].is_object() {
            continue;
        }
        let options =
            serde_json::json!({"path":"index.d2","engine":case["engine"],"utf16":true}).to_string();
        let actual = render_d2_source_impl(case["source"].as_str().unwrap(), &options);
        let expected = &master["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "direct" && row["name"] == case["name"])
            .unwrap()["got"];
        if !expected["error"].as_str().unwrap().is_empty() {
            assert_eq!(
                actual.unwrap_err(),
                expected["error"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            faults += 1;
        } else {
            let bytes = STANDARD
                .decode(expected["output"].as_str().unwrap())
                .unwrap();
            assert_eq!(actual.unwrap().as_bytes(), bytes, "{}", case["name"]);
            successful += 1;
        }
    }
    assert_eq!((faults, successful), (72, 23));
}
