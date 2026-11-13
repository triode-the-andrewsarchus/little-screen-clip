// SPDX-License-Identifier: MPL-2.0
use super::*;
use serde_json::Value;

fn public_request(json: &str) -> Result<Value, String> {
    let prepared: Value = serde_json::from_str(&public_compile_request::prepare(json)?).unwrap();
    let path = prepared["inputPath"].as_str().unwrap();
    let source = prepared["fs"][path].as_str().unwrap();
    compile_d2_target_json_with_policy(source, &prepared["sourceOptions"].to_string(), true)
        .map(|json| serde_json::from_str(&json).unwrap())
        .map_err(SourceCompileError::public_message)
}

fn flatten_expected(diagram: &Value, path: String, rows: &mut Vec<(String, Value)>) {
    let mut root = diagram.clone();
    for wrapper in ["layers", "scenarios", "steps"] {
        root.as_object_mut().unwrap().remove(wrapper);
    }
    rows.push((path.clone(), root));
    for wrapper in ["layers", "scenarios", "steps"] {
        if let Some(children) = diagram[wrapper].as_array() {
            for child in children {
                let child_path = format!("{wrapper}.{}", child["name"].as_str().unwrap());
                let child_path = if path.is_empty() {
                    child_path
                } else {
                    format!("{path}.{child_path}")
                };
                flatten_expected(child, child_path, rows);
            }
        }
    }
}

fn assert_complete_board_targets(expected: &Value, actual: &Value, name: &str) {
    let mut expected_rows = Vec::new();
    flatten_expected(expected, String::new(), &mut expected_rows);
    let mut actual_rows = vec![(String::new(), actual["diagram"].clone())];
    for board in actual["boards"].as_array().unwrap() {
        actual_rows.push((
            board["path"].as_str().unwrap().to_owned(),
            board["graph"]["diagram"].clone(),
        ));
    }
    assert_eq!(
        actual_rows.len(),
        expected_rows.len(),
        "{name}: full board inventory"
    );
    for (path, expected) in &expected_rows {
        let (_, actual) = actual_rows
            .iter()
            .find(|(p, _)| p == path)
            .expect("missing expected board");
        super::sequence_target_export_tests::assert_target(
            expected,
            actual,
            &format!("{name}:{path}"),
        );
    }
    for wrapper in ["layers", "scenarios", "steps"] {
        for (parent, _) in &expected_rows {
            let prefix = if parent.is_empty() {
                format!("{wrapper}.")
            } else {
                format!("{parent}.{wrapper}.")
            };
            let children = |rows: &Vec<(String, Value)>| {
                rows.iter()
                    .filter(|(p, _)| p.starts_with(&prefix) && !p[prefix.len()..].contains('.'))
                    .map(|(p, _)| p.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                children(&expected_rows),
                children(&actual_rows),
                "{name}: sibling order"
            );
        }
    }
}

#[test]
fn complete_public_requests_and_stateful_font_failures_follow_original_go() {
    let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-option-boundaries-source-v090.json"
    ))
    .unwrap();
    let requests = fixture["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 355);
    let (mut faults, mut probe_errors) = (0, 0);
    for rows in fixture["repeats"].as_array().unwrap() {
        assert_eq!(rows.as_array().unwrap().len(), requests.len());
        for (request, row) in requests.iter().zip(rows.as_array().unwrap()) {
            let name = request["name"].as_str().unwrap();
            assert_eq!(row["name"], name);
            let actual = public_request(request["request_json"].as_str().unwrap());
            if let Some(error) = row["error"]["message"].as_str() {
                let actual = actual.unwrap_err();
                if error.starts_with("panic recovered:") {
                    assert_eq!(actual.lines().next(), error.lines().next(), "{name}");
                    assert_ne!(actual, error, "Go runtime stacks remain unmatched");
                    faults += 1;
                } else {
                    assert_eq!(actual, error, "{name}");
                }
            } else {
                assert_complete_board_targets(&row["result"], &actual.unwrap(), name);
            }
            let probe =
                public_request(r#"{"fs":{"index":"a -> b\n"},"options":{"layout":"dagre"}}"#);
            if let Some(error) = row["healthyError"].as_str() {
                assert_eq!(probe.unwrap_err(), error, "{name}: poisoned ruler state");
                probe_errors += 1;
            } else {
                assert_complete_board_targets(&row["healthy"], &probe.unwrap(), name);
            }
        }
    }
    assert_eq!(faults, 48);
    assert_eq!(probe_errors, 96);
}

#[test]
fn public_font_checksum_failure_precedes_parsing_and_valid_registration_restores_ruler() {
    let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-font-registry-source-v090.json"
    ))
    .unwrap();
    let requests = fixture["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 4);
    for rows in fixture["repeats"].as_array().unwrap() {
        for (request, row) in requests.iter().zip(rows.as_array().unwrap()) {
            let name = request["name"].as_str().unwrap();
            let actual = public_request(request["request_json"].as_str().unwrap());
            if let Some(error) = row["error"]["message"].as_str() {
                assert_eq!(actual.unwrap_err(), error, "{name}");
            } else {
                assert_complete_board_targets(&row["result"], &actual.unwrap(), name);
            }
            let probe =
                public_request(r#"{"fs":{"index":"a -> b\n"},"options":{"layout":"dagre"}}"#);
            if let Some(error) = row["healthyError"].as_str() {
                assert_eq!(probe.unwrap_err(), error, "{name}: persistent font state");
            } else {
                assert_complete_board_targets(&row["healthy"], &probe.unwrap(), name);
            }
        }
    }
}

#[test]
fn complete_public_render_options_and_targets_follow_original_go() {
    let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-render-options-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 147);
    for case in cases {
        let actual = public_request(case["requestWire"].as_str().unwrap()).unwrap();
        // Master adds a resolved Isometric pointer (default false). Preserve
        // every v0.9.0 value and compare the complete current object shape.
        let mut expected_options = case["renderOptions"].clone();
        expected_options["isometric"] = false.into();
        sequence_target_export_tests::assert_target(
            &expected_options,
            &actual["source_render_options"],
            &format!("{}.renderOptions", case["name"]),
        );
        assert_complete_board_targets(&case["diagram"], &actual, case["name"].as_str().unwrap());
    }
}

#[test]
fn public_request_unicode_decoding_matches_original_go() {
    let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/request-unicode-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 220);
    for row in cases {
        let actual: Value = serde_json::from_str(&get_elk_graph(Some(
            row["request"].as_str().unwrap().to_owned(),
        )))
        .unwrap();
        sequence_target_export_tests::assert_target(
            &row["expected"],
            &actual,
            row["name"].as_str().unwrap(),
        );
    }
    let cases = fixture["public_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 60);
    for row in cases {
        let result = public_request(row["request"].as_str().unwrap());
        if row["resolved"] == true {
            assert_complete_board_targets(
                &row["result"]["diagram"],
                &result.unwrap(),
                row["name"].as_str().unwrap(),
            );
        } else {
            assert_eq!(
                result.unwrap_err(),
                row["error"]["message"],
                "{}",
                row["name"]
            );
        }
    }
}
