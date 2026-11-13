//! Bounded Markdown front matter with source coordinates and explicit failures.
use serde_json::Value;
use std::ops::Range;

const MAX_BYTES: usize = 64 * 1024;

/// Identifies the front-matter syntax, independently of its delimiter spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Toml,
    Json,
}
impl Format {
    /// Returns the syntax name used in metadata presentation.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Json => "JSON",
        }
    }
}

/// Parsed metadata and its byte coordinates in the original, unchanged source.
#[derive(Clone, Debug, PartialEq)]
pub struct FrontMatter {
    pub format: Format,
    pub source: Range<usize>,
    /// Start of the Markdown body. Zero preserves source after a framing error.
    pub body_start: usize,
    pub value: Result<Value, String>,
}
impl FrontMatter {
    /// Returns bounded display fields, retaining an explicit overflow summary.
    pub fn display_fields(&self) -> Vec<(String, String)> {
        let Ok(Value::Object(fields)) = &self.value else {
            return Vec::new();
        };
        let bounded = |text: &str| {
            let mut value: String = text.chars().take(160).collect();
            if text.chars().count() > 160 {
                value.push('…');
            }
            value
        };
        let mut result: Vec<_> = fields
            .iter()
            .take(32)
            .map(|(key, value)| {
                let text = match value {
                    Value::String(text) => text.clone(),
                    _ => value.to_string(),
                };
                (bounded(key), bounded(&text))
            })
            .collect();
        if fields.len() > 32 {
            result.push(("…".into(), format!("{} more fields", fields.len() - 32)));
        }
        result
    }
}

/// Reads YAML `---`, TOML `+++`, JSON `;;;`, or a leading JSON object.
///
/// Framing and decoded metadata are bounded. This never resolves includes,
/// environment values, tags or network resources. A framing failure leaves
/// `body_start` at zero so callers can show the source rather than lose text.
/// A closed, malformed block retains its source range and a parse diagnostic.
///
/// # Examples
/// ```
/// let source = "+++\ntitle = 'Example'\n+++\n# Body";
/// let metadata = little_clip_core::markdown_metadata::parse(source).unwrap();
/// assert_eq!(&source[metadata.body_start..], "# Body");
/// assert_eq!(metadata.value.unwrap()["title"], "Example");
/// ```
pub fn parse(input: &str) -> Option<FrontMatter> {
    let start = if input.starts_with('\u{feff}') { 3 } else { 0 };
    let mut end = input.len().min(start + MAX_BYTES);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &input[start..end];
    let first = prefix.split_inclusive('\n').next()?;
    let delimiter = first.trim_end_matches(['\r', '\n']);
    let format = match delimiter {
        "---" => Format::Yaml,
        "+++" => Format::Toml,
        ";;;" => Format::Json,
        _ if prefix.starts_with('{') => return bare_json(input, prefix, start),
        _ => return None,
    };
    let mut offset = first.len();
    for line in prefix[first.len()..].split_inclusive('\n') {
        let closing = line.trim_end_matches(['\r', '\n']);
        let complete_line = line.ends_with('\n') || start + offset + line.len() == input.len();
        if complete_line && (closing == delimiter || (format == Format::Yaml && closing == "...")) {
            return Some(FrontMatter {
                format,
                source: start..start + offset + line.len(),
                body_start: start + offset + line.len(),
                value: decode(&prefix[first.len()..offset], format),
            });
        }
        offset += line.len();
    }
    Some(FrontMatter {
        format,
        source: start..start + first.len(),
        body_start: 0,
        value: Err("Front matter is unclosed or exceeds the 64 KiB limit".into()),
    })
}

fn bare_json(input: &str, prefix: &str, start: usize) -> Option<FrontMatter> {
    // A prose brace is not enough to classify a document as JSON metadata.
    let after_brace = prefix[1..].trim_start();
    if !after_brace.starts_with(['"', '}']) {
        return None;
    }
    let mut stream = serde_json::Deserializer::from_str(prefix).into_iter::<Value>();
    let value = stream.next()?;
    let consumed = stream.byte_offset();
    if value.is_ok() && !input[start + consumed..].is_empty() {
        let rest = input[start + consumed..].trim_start_matches([' ', '\t', '\r']);
        if !rest.starts_with('\n') {
            return None;
        }
    }
    let valid = value.is_ok();
    Some(FrontMatter {
        format: Format::Json,
        source: start..start + consumed,
        body_start: if valid { start + consumed } else { 0 },
        value: value.map_err(|error| error.to_string()),
    })
}

fn decode(source: &str, format: Format) -> Result<Value, String> {
    let value = match format {
        Format::Yaml => {
            let options = serde_saphyr::options! {
                budget: serde_saphyr::budget! {
                    max_depth: 32,
                    flow_nesting_limit: 32,
                    max_events: 16_384,
                    max_nodes: 4_096,
                    max_aliases: 64,
                    max_anchors: 64,
                    max_recorded_anchor_events: 4_096,
                    max_recorded_anchor_bytes: MAX_BYTES,
                    max_total_scalar_bytes: MAX_BYTES,
                    max_documents: 1,
                    max_inclusion_depth: 0,
                },
                reject_unsupported_tags: true,
                strict_booleans: true,
            };
            serde_saphyr::from_str_with_options(source, options)
                .map_err(|error| error.to_string())?
        }
        Format::Toml => {
            let value: toml::Value = toml::from_str(source).map_err(|error| error.to_string())?;
            toml_metadata(value)?
        }
        Format::Json => serde_json::from_str(source).map_err(|error| error.to_string())?,
    };
    if value.is_null() && format == Format::Yaml {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    if !matches!(value, Value::Object(_)) {
        return Err("Front matter must contain a metadata object".into());
    }
    Ok(value)
}

// TOML's Serde datetime encoding is an implementation-specific struct, not a
// metadata object. Represent its ISO value as text in the common JSON model.
fn toml_metadata(value: toml::Value) -> Result<Value, String> {
    match value {
        toml::Value::Datetime(value) => Ok(Value::String(value.to_string())),
        toml::Value::Array(values) => values
            .into_iter()
            .map(toml_metadata)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        toml::Value::Table(values) => values
            .into_iter()
            .map(|(key, value)| toml_metadata(value).map(|value| (key, value)))
            .collect::<Result<_, _>>()
            .map(Value::Object),
        value => serde_json::to_value(value).map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_dates_remain_readable_typed_metadata_in_nested_tables_and_arrays() {
        let source = "+++\ndate = 2026-11-12\nlastmod = 2026-11-12T12:30:00-06:00\n[params]\nrevisions = [2026-11-11, 2026-11-12]\ndraft = false\n+++\n# Body";
        let value = parse(source).unwrap().value.unwrap();
        assert_eq!(value["date"], "2026-11-12");
        assert_eq!(value["lastmod"], "2026-11-12T12:30:00-06:00");
        assert_eq!(value["params"]["revisions"][1], "2026-11-12");
        assert_eq!(value["params"]["draft"], false);
        assert!(!value.to_string().contains("toml_private"));
    }

    #[test]
    fn delimiters_preserve_original_body_and_unicode_offsets() {
        for (format, text) in [
            (
                Format::Yaml,
                "\u{feff}---\r\ntitle: café\r\n---\r\n# Body\r\n",
            ),
            (Format::Toml, "+++\ntitle = 'café'\n+++\n# Body\r\n"),
            (Format::Json, ";;;\n{\"title\":\"café\"}\n;;;\n# Body\r\n"),
        ] {
            let metadata = parse(text).unwrap();
            assert_eq!(metadata.format, format);
            assert_eq!(&text[metadata.body_start..], "# Body\r\n");
            assert_eq!(metadata.value.unwrap()["title"], "café");
        }
    }

    #[test]
    fn json_object_prefix_handles_nested_braces_strings_and_body() {
        let source = "{\"nested\":{\"value\":\"} \\\" {\"}}\n\n# Body";
        let metadata = parse(source).unwrap();
        assert!(metadata.value.is_ok());
        assert_eq!(&source[metadata.body_start..], "\n\n# Body");
        assert!(parse("{ordinary prose}").is_none());
        assert!(parse("{} ordinary prose").is_none());
        assert!(parse("# Title\n---\ntitle: x\n---").is_none());
    }

    #[test]
    fn malformed_or_oversized_metadata_keeps_diagnostics_and_source() {
        let source = "---\ntitle: [\n---\n# Body";
        let metadata = parse(source).unwrap();
        assert!(metadata.value.is_err());
        assert_eq!(&source[metadata.body_start..], "# Body");
        for source in [
            "+++\ntitle = 'x'\n# Body".to_owned(),
            format!("---\n{}\n---\nbody", "x".repeat(MAX_BYTES)),
        ] {
            let metadata = parse(&source).unwrap();
            assert!(metadata.value.is_err());
            assert_eq!(metadata.body_start, 0);
        }
    }

    #[test]
    fn limits_do_not_turn_partial_delimiters_into_closed_blocks() {
        let padding = " ".repeat(MAX_BYTES - 8);
        let source = format!("---\n{padding}\n---oops\nBody");
        assert_eq!(parse(&source).unwrap().body_start, 0);
        assert!(parse("---\n---\nbody").unwrap().value.unwrap().is_object());
        assert!(parse(";;;\n[]\n;;;\nbody").unwrap().value.is_err());
    }

    #[test]
    fn yaml_cannot_include_or_amplify_unbounded_aliases() {
        for value in [
            "secret: !include /etc/passwd".to_owned(),
            format!("a: &a hello\nb: [{}]", vec!["*a"; 100].join(",")),
            format!("deep: {}0{}", "[".repeat(64), "]".repeat(64)),
        ] {
            assert!(
                parse(&format!("---\n{value}\n---\nbody"))
                    .unwrap()
                    .value
                    .is_err()
            );
        }
        let source = "---\nbase: &name Example\nalias: *name\n---\nbody";
        assert_eq!(parse(source).unwrap().value.unwrap()["alias"], "Example");
    }
}
