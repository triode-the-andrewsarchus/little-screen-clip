// SPDX-License-Identifier: MPL-2.0
//! Shared Go URL codec, including the 16 MiB decoded script limit.

pub(super) use d2_rs_lib::urlenc::decode;

#[cfg(all(test, feature = "oracle-fixtures"))]
mod tests {
    use super::*;
    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn original_go_url_decode_streams_and_errors_match() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/url-codec-source-v090.json"
        ))
        .unwrap();
        let rows = fixture["cases"].as_array().unwrap();
        assert_eq!(rows.len(), 987);
        let mut failures = Vec::new();
        for row in rows {
            let actual = decode(row["encoded"].as_str().unwrap());
            let expected = &row["expected"];
            let matches = match actual {
                Ok(ref text) => {
                    expected.get("error").is_none() && expected["result"] == text.as_str()
                }
                Err(ref message) => expected["error"]["message"] == message.as_str(),
            };
            if !matches {
                failures.push(
                    serde_json::json!({"name":row["name"],"expected":expected,"actual":actual}),
                );
            }
        }
        assert!(
            failures.is_empty(),
            "source codec differences: {}",
            serde_json::to_string(&failures).unwrap()
        );
    }
}
