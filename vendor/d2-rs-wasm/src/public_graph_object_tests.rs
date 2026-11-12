// SPDX-License-Identifier: MPL-2.0
//! Complete original board Graph records, including authored IR references.
use super::*;

fn compare_board(expected: &serde_json::Value, actual: &serde_json::Value, path: &str) {
    let mut expected_body = expected.clone();
    for wrapper in ["layers", "scenarios", "steps"] {
        expected_body.as_object_mut().unwrap().remove(wrapper);
    }
    sequence_target_export_tests::assert_target(&expected_body, &actual["source_graph"], path);
}

#[test]
fn compiled_graph_board_records_match_original_go() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-objects-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 207);
    let storage_fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-edge-storage-source-v090.json"
    ))
    .unwrap();
    let storage_cases = storage_fixture["cases"].as_array().unwrap();
    assert_eq!(storage_cases.len(), 18);
    for case in cases.iter().chain(storage_cases) {
        let prepared: serde_json::Value = serde_json::from_str(
            &public_compile_request::prepare(&case["request"].to_string()).unwrap(),
        )
        .unwrap();
        let actual: serde_json::Value = serde_json::from_str(
            &compile_d2_target_json_with_policy(
                case["request"]["fs"]["index.d2"].as_str().unwrap(),
                &prepared["sourceOptions"].to_string(),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        compare_board(&case["graph"], &actual, case["name"].as_str().unwrap());
        fn descendant_count(graph: &serde_json::Value) -> usize {
            ["layers", "scenarios", "steps"]
                .into_iter()
                .map(|kind| {
                    graph[kind].as_array().map_or(0, |children| {
                        children
                            .iter()
                            .map(|child| 1 + descendant_count(child))
                            .sum()
                    })
                })
                .sum()
        }
        let boards = actual["boards"].as_array().unwrap();
        assert_eq!(
            boards.len(),
            descendant_count(&case["graph"]),
            "{}: complete board inventory",
            case["name"]
        );
        let unique_paths: std::collections::BTreeSet<_> = boards
            .iter()
            .map(|board| board["path"].as_str().unwrap())
            .collect();
        assert_eq!(unique_paths.len(), boards.len(), "duplicate board path");
        for board in boards {
            let path = board["path"].as_str().unwrap();
            let parts = path.split('.').collect::<Vec<_>>();
            let mut expected = &case["graph"];
            for part in parts.chunks_exact(2) {
                expected = expected[part[0]]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|b| b["name"] == part[1])
                    .unwrap();
            }
            compare_board(
                expected,
                &board["graph"],
                &format!("{}:{path}", case["name"]),
            );
        }
    }
}
