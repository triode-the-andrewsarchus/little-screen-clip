// SPDX-License-Identifier: MPL-2.0
//! d2wasm.CompileResponse.RenderOptions after d2lib configuration/defaults.
use d2_rs_language::compiler::DiagramConfig;
use serde_json::{Map, Value, json};

pub(super) const FIELDS: [&str; 16] = [
    "pad",
    "sketch",
    "isometric",
    "center",
    "themeID",
    "darkThemeID",
    "themeOverrides",
    "darkThemeOverrides",
    "scale",
    "forceAppendix",
    "target",
    "animateInterval",
    "salt",
    "noXMLTag",
    "ascii",
    "asciiMode",
];

fn overrides(values: &std::collections::BTreeMap<String, String>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    [
        "N1", "N2", "N3", "N4", "N5", "N6", "N7", "B1", "B2", "B3", "B4", "B5", "B6", "AA2", "AA4",
        "AA5", "AB4", "AB5",
    ]
    .into_iter()
    .map(|key| {
        (
            key.to_ascii_lowercase(),
            values.get(key).map_or(Value::Null, |value| json!(value)),
        )
    })
    .collect::<Map<String, Value>>()
    .into()
}

pub(super) fn project(options: &Map<String, Value>, config: Option<&DiagramConfig>) -> Value {
    let mut output: Map<String, Value> = FIELDS
        .into_iter()
        .map(|key| {
            (
                key.to_owned(),
                options.get(key).cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    // Compile forwards only these render values to d2lib. Configuration fills
    // nil pointers; defaults then fill pad/sketch/center/themeID.
    let config = config.cloned().unwrap_or_default();
    for (key, value) in [
        ("pad", json!(config.pad.unwrap_or(100))),
        ("sketch", json!(config.sketch.unwrap_or(false))),
        ("isometric", json!(config.isometric.unwrap_or(false))),
        ("center", json!(config.center.unwrap_or(false))),
        ("themeID", json!(config.theme_id.unwrap_or(0))),
        ("darkThemeID", json!(config.dark_theme_id)),
    ] {
        if output[key].is_null() {
            output.insert(key.to_owned(), value);
        }
    }
    // d2wasm Compile never forwards input.Opts.ThemeOverrides into renderOpts.
    output.insert("themeOverrides".into(), overrides(&config.theme_overrides));
    output.insert(
        "darkThemeOverrides".into(),
        overrides(&config.dark_theme_overrides),
    );
    Value::Object(output)
}
