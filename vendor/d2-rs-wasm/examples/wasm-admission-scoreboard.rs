// SPDX-License-Identifier: MPL-2.0
//! Replay captured raw WASM Render requests through the same Rust boundary.
use serde_json::Value;

fn main() {
    let path = std::env::args().nth(1).expect("request.json");
    let request: Option<String> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let input: Value = serde_json::from_str(request.as_deref().unwrap()).unwrap();
    let response = d2_rs_wasm::render_d2_target_response_json(
        &input["diagram"].to_string(),
        &input["options"].to_string(),
    );
    println!("{response}");
}
