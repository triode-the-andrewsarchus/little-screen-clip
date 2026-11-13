// SPDX-License-Identifier: MPL-2.0
//! Original source observations for actor label/icon defaults and lifelines.
use super::*;

#[test]
fn sequence_actor_label_policy_matches_original_successes_and_errors() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/sequence-actor-label-policy-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 128);
    let (mut successes, mut errors, mut pending_panics) = (0, 0, 0);
    let mut observations = Vec::new();
    for case in cases {
        let expected = &case["expected"];
        let actual = source_layout_impl_with_cancellation(
            case["source"].as_str().unwrap(),
            r#"{"path":"index.d2","engine":"dagre"}"#,
            None,
        );
        if expected["panic"] == true {
            // The source API's recovered nil-pointer faults remain an open
            // contract. Retain every observation without counting it as a match.
            pending_panics += 1;
            observations.push(serde_json::json!({"name":case["name"],"source_panic_pending":true,"native_error":actual.err()}));
            continue;
        }
        if let Some(error) = expected["error"].as_str() {
            assert_eq!(actual.err().as_deref(), Some(error), "{}", case["name"]);
            errors += 1;
            observations.push(serde_json::json!({"name":case["name"],"error":error}));
            continue;
        }
        let (graph, layout, _, _, _) = actual.unwrap();
        let shapes = expected["diagram"]["shapes"].as_array().unwrap();
        assert_eq!(graph.nodes().count(), shapes.len(), "{}", case["name"]);
        for shape in shapes {
            let (id, _) = graph
                .nodes()
                .find(|(_, n)| n.external_id == shape["id"].as_str().unwrap())
                .unwrap();
            let rect = layout.boxes[&id];
            for (field, actual, expected) in [
                (
                    "x",
                    rect.origin.x.trunc(),
                    shape["pos"]["x"].as_f64().unwrap(),
                ),
                (
                    "y",
                    rect.origin.y.trunc(),
                    shape["pos"]["y"].as_f64().unwrap(),
                ),
                (
                    "width",
                    rect.size.width.trunc(),
                    shape["width"].as_f64().unwrap(),
                ),
                (
                    "height",
                    rect.size.height.trunc(),
                    shape["height"].as_f64().unwrap(),
                ),
            ] {
                assert_eq!(actual, expected, "{} {} {field}", case["name"], shape["id"]);
            }
            let label = &layout.node_labels[&id];
            assert_eq!(
                label.position.d2_name(),
                shape["labelPosition"].as_str().unwrap(),
                "{} {} label position",
                case["name"],
                shape["id"]
            );
            assert_eq!(
                label.icon_position.map(|p| p.d2_name()).unwrap_or(""),
                shape["iconPosition"].as_str().unwrap(),
                "{} {} icon position",
                case["name"],
                shape["id"]
            );
        }
        let raw_edges = expected["raw_connections"].as_array().unwrap();
        assert_eq!(
            layout.sequence_lifelines.len(),
            raw_edges
                .iter()
                .filter(|e| e["dst"].as_str().unwrap().contains("-lifeline-end-"))
                .count(),
            "{} complete lifeline inventory",
            case["name"]
        );
        for line in &layout.sequence_lifelines {
            let id = &graph.node(line.actor).unwrap().external_id;
            let source: Vec<_> = raw_edges
                .iter()
                .filter(|e| {
                    e["src"].as_str() == Some(id.as_str())
                        && e["dst"].as_str().unwrap().contains("-lifeline-end-")
                })
                .collect();
            assert_eq!(source.len(), 1);
            let route = source[0]["route"].as_array().unwrap();
            assert_eq!(line.route.len(), route.len());
            for (point, expected) in line.route.iter().zip(route) {
                for (axis, value) in [("x", point.x), ("y", point.y)] {
                    assert_eq!(
                        value.to_bits(),
                        expected[axis].as_f64().unwrap().to_bits(),
                        "{} {id} raw lifeline {axis}",
                        case["name"]
                    );
                }
            }
        }
        successes += 1;
        observations.push(serde_json::json!({"name":case["name"],"boxes":layout.boxes,"labels":layout.node_labels,"lifelines":layout.sequence_lifelines}));
    }
    assert_eq!((successes, errors, pending_panics), (100, 16, 12));
    if let Ok(path) = std::env::var("D2RS_SEQUENCE_ACTOR_LABEL_RECORDS") {
        std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({"successful_geometry_label_lifeline_matches":successes,"error_matches":errors,"source_panics_pending":pending_panics,"whole_scope_passed":false,"observations":observations})).unwrap()).unwrap();
    }
}
