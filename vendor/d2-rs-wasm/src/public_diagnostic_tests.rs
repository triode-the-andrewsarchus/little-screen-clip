// SPDX-License-Identifier: MPL-2.0
use super::*;

#[cfg(feature = "oracle-fixtures")]
#[test]
fn public_compiler_diagnostics_keep_ordered_ranges_and_native_helper_text() {
    let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
        "tests/fixtures/public-compiler-diagnostics-source-v090.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 68);
    for case in cases {
        let path = case["inputPath"].as_str().unwrap_or("index.d2");
        let mut files = case["files"].as_object().cloned().unwrap_or_default();
        files.insert(path.to_owned(), case["source"].clone());
        let options = serde_json::json!({
            "path": path, "files": files, "utf16_positions": true,
            "engine": case["options"]["layout"].as_str().unwrap_or("dagre"),
        })
        .to_string();
        let source = case["source"].as_str().unwrap();
        let error = compile_d2_target_json_typed(source, &options).unwrap_err();
        assert!(
            matches!(error, SourceCompileError::Diagnostics(_)),
            "{}",
            case["name"]
        );
        assert_eq!(
            error.public_message(),
            case["error"]["message"],
            "{}",
            case["name"]
        );
        let native_text = case["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["errmsg"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            compile_d2_target_json_impl(source, &options).unwrap_err(),
            native_text,
            "{}",
            case["name"]
        );
    }
}

#[test]
fn public_compile_preserves_noncompiler_error_identity() {
    for options in [r#"{"engine":"bogus"}"#, r#"{"font_regular":"AQID"}"#, "{"] {
        let public = compile_d2_target_json_typed("a\n", options).unwrap_err();
        assert!(matches!(public, SourceCompileError::Other(_)));
        assert_eq!(
            public.public_message(),
            compile_d2_target_json_impl("a\n", options).unwrap_err()
        );
    }
    let diagnostics = SourceCompileError::Diagnostics(None);
    assert_eq!(diagnostics.to_string(), "");
    assert_eq!(diagnostics.public_message(), "null");
}
