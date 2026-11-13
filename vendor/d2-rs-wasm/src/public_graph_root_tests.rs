// SPDX-License-Identifier: MPL-2.0
//! Original public Graph AST/root and target-root projections across engines.
use super::*;

#[test]
fn public_graph_document_and_board_roots_match_original_go() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-roots-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 87);
    let mut boards = 0;
    for case in cases {
        let options = serde_json::json!({
            "path": "index.d2", "engine": case["engine"], "utf16": true
        });
        let actual: serde_json::Value = serde_json::from_str(
            &compile_d2_target_json_impl(case["source"].as_str().unwrap(), &options.to_string())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            actual["boards"].as_array().unwrap().len() + 1,
            case["boards"].as_array().unwrap().len()
        );
        for board in case["boards"].as_array().unwrap() {
            let path = board["path"].as_str().unwrap();
            let metadata = if path.is_empty() {
                &actual
            } else {
                &actual["boards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["path"] == path)
                    .unwrap_or_else(|| panic!("missing board {path}"))["graph"]
            };
            for (expected, actual_key) in [("ast", "ast"), ("root", "source_graph_root")] {
                sequence_target_export_tests::assert_target(
                    &board[expected],
                    &metadata[actual_key],
                    &format!("{}:{path}.{expected}", case["name"]),
                );
            }
            sequence_target_export_tests::assert_target(
                &board["target_root"],
                &metadata["diagram"]["root"],
                &format!("{}:{path}.target_root", case["name"]),
            );
            boards += 1;
        }
    }
    assert_eq!(boards, 99);
}

/// The target response is serialized directly from borrowed records. Its
/// bytes must equal the former `serde_json::Value` encoding: a decode and
/// re-encode through `Value` (ordered maps, shortest f64 spelling) must be an
/// identity, so no f32 spelling, duplicate key or raw number differs.
#[test]
fn public_target_json_is_value_encoding_identity() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-graph-roots-source-v090.json"
    ))
    .unwrap();
    let mut sources: Vec<(String, String, bool)> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["source"].as_str().unwrap().to_owned(),
                case["engine"].as_str().unwrap_or("dagre").to_owned(),
                false,
            )
        })
        .collect();
    let extra = [
        "vars: { d2-config: { theme-id: 4; pad: 7; center: true; theme-overrides: { B1: \"#123456\" }; data: { k: [1, 2.5] } } }\na.b -> c: hi { style.stroke-dash: 3; source-arrowhead.shape: diamond }\nc.shape: circle\nd.icon: https://icons.terrastruct.com/essentials/004-picture.svg\n",
        "x: { shape: sql_table; id: int { constraint: primary_key }; name: string }\ny: { shape: class; +f: int; -m(a int): string }\nx.id -> y\n",
        "s: { shape: sequence_diagram; a -> b: m1; b -> a: m2; a.t: { b -> a } }\n",
        "g: { grid-rows: 2; grid-columns: 3; a; b; c; d; e: { style.opacity: 0.35 }; f }\n",
        "vars: { d2-legend: { a: Legend { shape: circle }; a -> b: flows } }\na -> b\n",
        "a -> b: { style.animated: true; label.near: top-center }\nlayers: { l1: { c -> d } }\nscenarios: { s1: { a.style.fill: red } }\nsteps: { st1: { e } }\n",
    ];
    for source in extra {
        for engine in ["dagre", "elk", "tala"] {
            sources.push((source.to_owned(), engine.to_owned(), true));
        }
    }
    let mut checked = 0;
    for (source, engine, must_compile) in sources {
        for public_render_options in [false, true] {
            let mut options = serde_json::json!({
                "path": "index.d2", "engine": engine, "utf16_positions": true, "theme_id": 0
            });
            if public_render_options {
                options["public_render_options"] = serde_json::json!({ "pad": 20 });
            }
            let direct =
                match compile_d2_target_json_with_policy(&source, &options.to_string(), true) {
                    Ok(direct) => direct,
                    Err(error) if !must_compile => {
                        let _ = error;
                        continue;
                    }
                    Err(error) => panic!("{engine}: {source}: {}", error.public_message()),
                };
            checked += 1;
            let value: serde_json::Value = serde_json::from_str(&direct).unwrap();
            assert_eq!(
                serde_json::to_string(&value).unwrap(),
                direct,
                "{engine}: {source}"
            );
        }
    }
    assert!(checked >= 87 * 2 + 36, "only {checked} responses checked");
}
