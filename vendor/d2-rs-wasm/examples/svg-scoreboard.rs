// SPDX-License-Identifier: MPL-2.0
//! Run the WASM target-render boundary on the d2svg golden manifest.
//! Native execution measures the identical Rust path without JS error glue.
use serde_json::{Value, json};

fn main() {
    let manifest = std::env::args()
        .nth(1)
        .expect("usage: svg-scoreboard <manifest.json>");
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    for row in rows {
        let output = row["output"].as_str().unwrap();
        let result = (|| {
            let input = std::fs::read_to_string(row["input"].as_str().unwrap())
                .map_err(|error| error.to_string())?;
            let diagram: Value = serde_json::from_str(&input).map_err(|error| error.to_string())?;
            let config = &diagram["config"];
            let mut options = json!({"pad":0});
            for name in [
                "themeID",
                "darkThemeID",
                "sketch",
                "center",
                "themeOverrides",
                "darkThemeOverrides",
            ] {
                if !config[name].is_null() {
                    options[name] = config[name].clone();
                }
            }
            if !row["theme_id"].is_null() {
                options["themeID"] = row["theme_id"].clone();
            }
            if ["layers", "scenarios", "steps"].iter().any(|key| {
                diagram[key]
                    .as_array()
                    .is_some_and(|children| !children.is_empty())
            }) {
                options["target"] = json!("*");
                options["animateInterval"] = json!(1000);
            }
            d2_rs_wasm::render_d2_target_json_native(&input, &options.to_string())
        })();
        match result {
            Ok(svg) => std::fs::write(output, svg).unwrap(),
            Err(error) => std::fs::write(format!("{output}.err"), error).unwrap(),
        }
    }
}
