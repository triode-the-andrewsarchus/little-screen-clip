// SPDX-License-Identifier: MPL-2.0
//! Native replay of the original isometric WASM test calls.
use serde_json::{Value, json};

fn main() {
    let path = std::env::args().nth(1).expect("request.json");
    let request: String = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let request: Value = serde_json::from_str(&request).unwrap();
    let input = &request["input"];
    let response = if request["operation"] == "compile" {
        let prepared: Value = serde_json::from_str(
            &d2_rs_wasm::prepare_d2_public_compile_request_json(&input.to_string()).unwrap(),
        )
        .unwrap();
        let source = prepared["fs"][prepared["inputPath"].as_str().unwrap()]
            .as_str()
            .unwrap();
        let compiled: Value = serde_json::from_str(
            &d2_rs_wasm::compile_d2_public_target_json(
                source,
                &prepared["sourceOptions"].to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        json!({"data":{"diagram":compiled["diagram"], "renderOptions":compiled["source_render_options"]}})
    } else {
        serde_json::from_str(&d2_rs_wasm::render_d2_target_response_json(
            &input["diagram"].to_string(),
            &input["options"].to_string(),
        ))
        .unwrap()
    };
    println!("{response}");
}
