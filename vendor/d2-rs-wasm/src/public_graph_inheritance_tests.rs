// SPDX-License-Identifier: MPL-2.0
//! Original complete board graphs and targets, including literal board names.
use super::*;
use serde_json::Value;

fn expected_boards(
    graph: &Value,
    diagram: &Value,
    path: Vec<String>,
    rows: &mut Vec<(Vec<String>, Value, Value)>,
) {
    let mut graph_body = graph.clone();
    let mut diagram_body = diagram.clone();
    for wrapper in ["layers", "scenarios", "steps"] {
        graph_body.as_object_mut().unwrap().remove(wrapper);
        diagram_body.as_object_mut().unwrap().remove(wrapper);
    }
    rows.push((path.clone(), graph_body, diagram_body));
    for wrapper in ["layers", "scenarios", "steps"] {
        if let Some(children) = graph[wrapper].as_array() {
            for child in children {
                let name = child["name"].as_str().unwrap();
                let child_diagram = diagram[wrapper]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v["name"] == name)
                    .unwrap();
                let mut child_path = path.clone();
                child_path.extend([wrapper.to_owned(), name.to_owned()]);
                expected_boards(child, child_diagram, child_path, rows);
            }
        }
    }
}

#[test]
fn complete_inherited_graphs_and_targets_match_original_go() {
    let fixture: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-inheritance-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 72);
    let mut errors = 0;
    let more: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-data-source-v090.json"
    ))
    .unwrap();
    let additional = more["cases"].as_array().unwrap();
    assert_eq!(additional.len(), 15);
    let contexts: Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-class-context-source-v090.json"
    ))
    .unwrap();
    let contexts = contexts["cases"].as_array().unwrap();
    assert_eq!(contexts.len(), 9);
    for case in cases.iter().chain(additional).chain(contexts) {
        let prepared: Value = serde_json::from_str(
            &public_compile_request::prepare(&case["request"].to_string()).unwrap(),
        )
        .unwrap();
        let result = compile_d2_target_json_with_policy(
            case["request"]["fs"]["index.d2"].as_str().unwrap(),
            &prepared["sourceOptions"].to_string(),
            true,
        )
        .map_err(SourceCompileError::public_message);
        if let Some(message) = case["error"]["message"].as_str() {
            assert_eq!(result.unwrap_err(), message, "{}", case["name"]);
            errors += 1;
            continue;
        }
        let actual: Value = serde_json::from_str(&result.unwrap()).unwrap();
        let mut expected = Vec::new();
        expected_boards(
            &case["expected"]["graph"],
            &case["expected"]["diagram"],
            Vec::new(),
            &mut expected,
        );
        let mut actual_rows = vec![(Vec::<String>::new(), &actual)];
        for entry in actual["boards"].as_array().unwrap() {
            let parts: Vec<String> = serde_json::from_value(entry["path_parts"].clone()).unwrap();
            actual_rows.push((parts, &entry["graph"]));
        }
        assert_eq!(
            actual_rows.len(),
            expected.len(),
            "{} board inventory",
            case["name"]
        );
        let paths: std::collections::BTreeSet<_> = actual_rows.iter().map(|(p, _)| p).collect();
        assert_eq!(paths.len(), actual_rows.len(), "duplicate board path");
        for (path, graph, diagram) in expected {
            let (_, actual) = actual_rows.iter().find(|(p, _)| p == &path).unwrap();
            let label = format!("{}:{path:?}", case["name"]);
            sequence_target_export_tests::assert_target(
                &graph,
                &actual["source_graph"],
                &format!("{label}.graph"),
            );
            sequence_target_export_tests::assert_target(
                &diagram,
                &actual["diagram"],
                &format!("{label}.diagram"),
            );
        }
    }
    assert_eq!(errors, 6);
}
