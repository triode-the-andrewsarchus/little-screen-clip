// SPDX-License-Identifier: MPL-2.0
//! Original sequence label/scalar mutation, including retained measured sizes.
//! Complete original diagrams plus source scalar mutations and measured sizes.
use super::*;

#[test]
fn sequence_span_target_and_graph_scalars_match_original_go() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/sequence-span-export-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 26);
    let mut spans = 0;
    for case in cases {
        let options = serde_json::json!({"path":"index.d2", "engine":case["engine"], "utf16":true});
        let actual: serde_json::Value = serde_json::from_str(
            &compile_d2_target_json_with_policy(
                case["source"].as_str().unwrap(),
                &options.to_string(),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        sequence_target_export_tests::assert_target(
            &case["diagram"],
            &actual["diagram"],
            case["name"].as_str().unwrap(),
        );
        sequence_target_export_tests::assert_target(
            &case["graph"],
            &actual["source_graph"],
            &format!("{}:complete Graph", case["name"]),
        );
        let expected_shapes = case["diagram"]["shapes"].as_array().unwrap();
        let shapes = actual["diagram"]["shapes"].as_array().unwrap();
        assert_eq!(
            shapes.len(),
            expected_shapes.len(),
            "{} inventory",
            case["name"]
        );
        for (expected, shape) in expected_shapes.iter().zip(shapes) {
            assert_eq!(shape["id"], expected["id"], "{} order", case["name"]);
            for key in [
                "type",
                "label",
                "labelWidth",
                "labelHeight",
                "labelPosition",
            ] {
                sequence_target_export_tests::assert_target(
                    &serde_json::json!({key:expected.get(key)}),
                    &serde_json::json!({key:shape.get(key)}),
                    &format!("{}:{}:{key}", case["name"], expected["id"]),
                );
            }
        }
        let expected_objects = case["graph"]["objects"].as_array().unwrap();
        let objects = actual["source_graph"]["objects"].as_array().unwrap();
        assert_eq!(objects.len(), expected_objects.len());
        for (expected, object) in expected_objects.iter().zip(objects) {
            assert_eq!(object["id"], expected["id"], "{} graph order", case["name"]);
            if expected["zIndex"] == 2 {
                spans += 1;
                for key in ["label", "shape", "labelDimensions"] {
                    sequence_target_export_tests::assert_target(
                        &expected["attributes"][key],
                        &object["attributes"][key],
                        &format!("{}:{}:attributes.{key}", case["name"], expected["id"]),
                    );
                }
            }
        }
    }
    assert_eq!(spans, 45);
}
