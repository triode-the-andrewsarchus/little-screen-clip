use crate::file_manager::search::{answer::Value as TypedValue, query::AnswerType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_HEADER: usize = 4096;
const MAX_FIELDS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager::search) struct Field {
    pub path: Vec<String>,
    pub value: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager::search) struct Header {
    pub schema: String,
    pub format: String,
    pub fields: Vec<Field>,
    pub raw: String,
}

pub(super) fn inspect(source: &str) -> Result<Option<Header>, String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    let marker = first.trim_end_matches(['\r', '\n']);
    let format = match marker {
        "+++" => "TOML",
        "---" => "YAML",
        _ => return Ok(None),
    };
    let mut end = first.len();
    let mut closed = false;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == marker {
            closed = true;
            break;
        }
        end += line.len();
        if end > MAX_HEADER {
            return Ok(None);
        }
    }
    if !closed {
        return Ok(None);
    }
    let raw = &source[first.len()..end];
    let value = if format == "TOML" {
        toml_json(
            toml::from_str(raw).map_err(|_| "Invalid TOML front matter")?,
            0,
        )
    } else {
        // Deserialize through YAML's value visitor first: JSON's generic map
        // visitor overwrites duplicate keys instead of rejecting ambiguity.
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(raw).map_err(|_| "Invalid YAML front matter")?;
        serde_json::to_value(yaml).map_err(|_| "Unsupported YAML front matter")?
    };
    if !value.is_object() {
        return Ok(None);
    }
    let mut fields = Vec::new();
    if !flatten(&value, &mut Vec::new(), &mut fields) || fields.is_empty() {
        return Ok(None);
    }
    let shape = fields
        .iter()
        .map(|field| (&field.path, &field.kind))
        .collect::<Vec<_>>();
    let shape = serde_json::to_vec(&(format, shape)).map_err(|_| "Invalid front-matter shape")?;
    Ok(Some(Header {
        schema: format!("{:x}", Sha256::digest(shape)),
        format: format.into(),
        fields,
        raw: raw.into(),
    }))
}

fn toml_json(value: toml::Value, depth: usize) -> Value {
    if depth > 8 {
        return Value::Null;
    }
    match value {
        toml::Value::Datetime(date) => Value::String(date.to_string()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, toml_json(value, depth + 1)))
                .collect(),
        ),
        // Floating literals require lexical preservation before exact numeric
        // extraction. Do not silently accept a rounded binary float as a fact.
        toml::Value::Float(_) | toml::Value::Array(_) => Value::Null,
        value => serde_json::to_value(value).unwrap_or(Value::Null),
    }
}

fn flatten(value: &Value, path: &mut Vec<String>, fields: &mut Vec<Field>) -> bool {
    if path.len() > 8 || fields.len() > MAX_FIELDS {
        return false;
    }
    if let Value::Object(object) = value {
        if object.is_empty() && !path.is_empty() {
            fields.push(Field {
                path: path.clone(),
                value: String::new(),
                kind: "unsupported".into(),
            });
        }
        for (key, child) in object {
            if key.is_empty() || key.len() > 256 {
                return false;
            }
            path.push(key.clone());
            let complete = flatten(child, path, fields);
            path.pop();
            if !complete {
                return false;
            }
        }
        return fields.len() <= MAX_FIELDS;
    }
    let (value, kind) = scalar(value);
    fields.push(Field {
        path: path.clone(),
        value,
        kind: kind.into(),
    });
    fields.len() <= MAX_FIELDS
}

fn scalar(value: &Value) -> (String, &'static str) {
    match value {
        Value::String(value) => {
            // Expose validated format, not the value, to the schema planner.
            // Include it in the schema hash so an invalid date cannot poison
            // the plan cache for later files with a valid date in that field.
            let kind = if TypedValue::parse(value, None, AnswerType::Date).is_ok() {
                "date"
            } else {
                "text"
            };
            (value.clone(), kind)
        }
        Value::Bool(value) => (value.to_string(), "boolean"),
        Value::Number(value) if value.is_i64() || value.is_u64() => (value.to_string(), "integer"),
        // Keep unsupported labels visible to planning. Hiding a null or
        // floating publishDate/balance could make another field appear to be
        // the best match. It is not an empty-string answer.
        _ => (String::new(), "unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_and_toml_preserve_dates_and_ignore_body_dates() {
        for input in [
            "---\r\ntitle: Example\r\ndate: 2021-11-18\r\n---\r\nBody date: 2030-01-01",
            "+++\ntitle = 'Example'\ndate = 2021-11-18\n+++\nBody date: 2030-01-01",
        ] {
            let header = inspect(input).unwrap().unwrap();
            let date = header
                .fields
                .iter()
                .find(|field| field.path == ["date"])
                .unwrap();
            assert_eq!(date.value, "2021-11-18");
            assert_eq!(date.kind, "date");
            assert!(!header.raw.contains("2030"));
        }
    }

    #[test]
    fn schema_tracks_structure_and_types_instead_of_values() {
        let first = inspect("---\ndate: 2021-01-01\ndraft: false\n---\n")
            .unwrap()
            .unwrap();
        let second = inspect("---\ndraft: true\ndate: 2025-02-01\n---\n")
            .unwrap()
            .unwrap();
        let changed = inspect("---\ndate: 12\ndraft: false\n---\n")
            .unwrap()
            .unwrap();
        assert_eq!(first.schema, second.schema);
        assert_ne!(first.schema, changed.schema);
        let invalid = inspect("---\ndate: unknown\ndraft: false\n---\n")
            .unwrap()
            .unwrap();
        assert_ne!(first.schema, invalid.schema);
        assert!(inspect("---\ndate: first\ndate: second\n---\n").is_err());
    }

    #[test]
    fn incomplete_or_large_headers_decline_and_unsupported_numbers_preserve_labels() {
        assert!(inspect("---\ndate: 2021-01-01\n").unwrap().is_none());
        assert!(
            inspect(&format!("---\ntitle: {}\n---\n", "a".repeat(MAX_HEADER)))
                .unwrap()
                .is_none()
        );
        let deep = format!(
            "---\ndate: 2021-11-18\nmetadata: {}\n---\n",
            "{\"a\":".repeat(9) + "null" + &"}".repeat(9)
        );
        let long_key = format!("---\ndate: 2021-11-18\n{}: null\n---\n", "a".repeat(257));
        assert!(inspect(&long_key).unwrap().is_none());
        assert!(inspect(&deep).unwrap().is_none());
        for source in [
            "+++\namount = 12.1234567890123456789\n+++\n",
            "---\namount: 12.1234567890123456789\n---\n",
            "---\npublishDate: null\n---\n",
            "---\npublishDate: {}\n---\n",
            "---\npublishDate: []\n---\n",
        ] {
            let header = inspect(source).unwrap().unwrap();
            assert_eq!(header.fields.len(), 1);
            assert_eq!(header.fields[0].kind, "unsupported");
            assert!(header.fields[0].value.is_empty());
        }
    }
}
