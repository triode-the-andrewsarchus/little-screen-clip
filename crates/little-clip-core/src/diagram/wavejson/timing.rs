//! Lossless timing edits over a validated JSON5 document.
use json_five::rt::parser::{JSONText, JSONValue};
use serde_json::Value;

/// One signal lane and its stable declaration path in the parsed document.
#[derive(Clone, Debug)]
pub struct Lane {
    pub name: String,
    pub wave: String,
    pub period: f64,
    pub phase: f64,
    path: Vec<usize>,
    wave_field: usize,
    name_field: Option<usize>,
}

/// A source snapshot that can change individual string literals losslessly.
///
/// Parsing belongs on a bounded worker. Applying a replacement does not parse or
/// render source; callers must discard this snapshot when their source changes.
pub struct Document {
    pub lanes: Vec<Lane>,
    source: String,
    ast: JSONText,
    signal_field: usize,
}
impl Document {
    /// Parses timing data and verifies that the concrete syntax round-trips.
    ///
    /// # Errors
    /// Returns an error for invalid data, non-timing figures, ambiguous keys or
    /// syntax the concrete-syntax parser cannot preserve byte for byte.
    pub fn parse(source: &str) -> Result<Self, String> {
        let value = super::parse(source)?;
        if value.get("signal").is_none() {
            return Err("Register and logic figures use the source editor.".into());
        }
        let bare = source.trim_start_matches('\u{feff}');
        let ast = parse_syntax(bare)?;
        if ast.to_string() != bare {
            return Err(
                "This JSON5 syntax cannot be changed losslessly; use the source editor.".into(),
            );
        }
        let signal_field = property(&ast.value, "signal")?.ok_or("Missing signal array")?;
        let mut lanes = Vec::new();
        let signals = field(&ast.value, signal_field).ok_or("Missing signal array")?;
        collect_lanes(&value["signal"], signals, &mut Vec::new(), &mut lanes)?;
        if lanes.is_empty() {
            return Err("No wave strings to paint; edit the figure in source.".into());
        }
        Ok(Self {
            source: source.into(),
            ast,
            signal_field,
            lanes,
        })
    }

    /// Returns the exact source revision represented by this model.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Replaces one lane's waveform or existing name, retaining all other bytes.
    ///
    /// # Errors
    /// Rejects stale source, missing fields, invalid lane indices and wave states
    /// outside the direct editor's supported cell alphabet or size budget.
    pub fn replace(
        &self,
        source: &str,
        lane: usize,
        name: bool,
        text: &str,
    ) -> Result<String, String> {
        if source != self.source {
            return Err("Source changed; wait for the timing editor to refresh.".into());
        }
        let lane = self.lanes.get(lane).ok_or("Unknown signal lane")?;
        let index = lane.edit_field(name, text)?;
        let mut ast = JSONText {
            value: self.ast.value.clone(),
            context: self.ast.context.clone(),
        };
        let mut node =
            field_mut(&mut ast.value, self.signal_field).ok_or("Missing signal array")?;
        for index in &lane.path {
            let JSONValue::JSONArray { values, .. } = node else {
                return Err("Signal path changed".into());
            };
            node = &mut values.get_mut(*index).ok_or("Signal path changed")?.value;
        }
        let literal = field_mut(node, index).ok_or("Signal field changed")?;
        let encoded = serde_json::to_string(text).map_err(|error| error.to_string())?;
        let inner = encoded[1..encoded.len() - 1].to_owned();
        *literal = if matches!(literal, JSONValue::SingleQuotedString(_)) {
            JSONValue::SingleQuotedString(inner.replace("\\\"", "\"").replace('\'', "\\'"))
        } else {
            JSONValue::DoubleQuotedString(inner)
        };
        let bom = &source[..source.len() - source.trim_start_matches('\u{feff}').len()];
        let result = format!("{bom}{ast}");
        if result.len() > super::MAX_SOURCE {
            return Err("Edited source exceeds the WaveJSON source budget.".into());
        }
        Ok(result)
    }

    /// Whether a lane's existing name can be changed without inventing structure.
    pub fn can_rename(&self, lane: usize) -> bool {
        self.lanes
            .get(lane)
            .is_some_and(|lane| lane.name_field.is_some())
    }
}

fn parse_syntax(source: &str) -> Result<JSONText, String> {
    let mut tokens =
        json_five::tokenize::tokenize_rt_str(source).map_err(|error| error.to_string())?;
    // json-five 0.3.1 reports a block comment's closing slash as its exclusive
    // end. Correct that public span before the concrete parser captures trivia.
    // The exact-roundtrip check still rejects every other representation drift.
    for (start, kind, end) in &mut tokens.tok_spans {
        if *kind == json_five::tokenize::TokType::BlockComment
            && source
                .get(*start..*end)
                .is_some_and(|text| text.ends_with('*'))
            && source.as_bytes().get(*end) == Some(&b'/')
        {
            *end += 1;
        }
    }
    json_five::rt::parser::from_tokens(&tokens).map_err(|error| error.to_string())
}

impl Lane {
    fn edit_field(&self, name: bool, text: &str) -> Result<usize, String> {
        let index = if name {
            self.name_field
                .ok_or("Add a name in the source editor first.")?
        } else {
            self.wave_field
        };
        if !name
            && (text.len() > 8192
                || !text
                    .chars()
                    .all(|ch| "01.xz=23456789pPnNhHlLuUdD|".contains(ch)))
        {
            return Err(
                "Use the source editor for subcycles or other extended wave states.".into(),
            );
        }
        if name && text.len() > 4096 {
            return Err("Signal name is too long.".into());
        }
        Ok(index)
    }
}

fn property(value: &JSONValue, name: &str) -> Result<Option<usize>, String> {
    let JSONValue::JSONObject {
        key_value_pairs, ..
    } = value
    else {
        return Ok(None);
    };
    let mut found = None;
    for (index, pair) in key_value_pairs.iter().enumerate() {
        // Delegate escaped identifier and string-key semantics to the JSON5 parser.
        let key: Value = json5::from_str(&format!("{{{}:null}}", pair.key))
            .map_err(|error| error.to_string())?;
        if key.get(name).is_some() && found.replace(index).is_some() {
            return Err(format!(
                "Duplicate {name} field; resolve it in source before graphical editing."
            ));
        }
    }
    Ok(found)
}
fn field(value: &JSONValue, index: usize) -> Option<&JSONValue> {
    let JSONValue::JSONObject {
        key_value_pairs, ..
    } = value
    else {
        return None;
    };
    key_value_pairs.get(index).map(|pair| &pair.value)
}
fn field_mut(value: &mut JSONValue, index: usize) -> Option<&mut JSONValue> {
    let JSONValue::JSONObject {
        key_value_pairs, ..
    } = value
    else {
        return None;
    };
    key_value_pairs.get_mut(index).map(|pair| &mut pair.value)
}
fn collect_lanes(
    value: &Value,
    syntax: &JSONValue,
    path: &mut Vec<usize>,
    lanes: &mut Vec<Lane>,
) -> Result<(), String> {
    match (value, syntax) {
        (Value::Array(items), JSONValue::JSONArray { values, .. }) => {
            for (index, item) in items.iter().enumerate() {
                path.push(index);
                let syntax = values
                    .get(index)
                    .ok_or("JSON5 parsers disagreed on an array; use source editing.")?;
                collect_lanes(item, &syntax.value, path, lanes)?;
                path.pop();
            }
        }
        (Value::Object(_), JSONValue::JSONObject { .. }) => {
            if let Some(wave) = value.get("wave").and_then(Value::as_str) {
                lanes.push(Lane {
                    name: value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Unnamed signal")
                        .into(),
                    wave: wave.into(),
                    period: value.get("period").and_then(Value::as_f64).unwrap_or(1.0),
                    phase: value.get("phase").and_then(Value::as_f64).unwrap_or(0.0),
                    path: path.clone(),
                    wave_field: property(syntax, "wave")?.ok_or("Missing wave literal")?,
                    name_field: if value.get("name").is_some_and(Value::is_string) {
                        property(syntax, "name")?
                    } else {
                        None
                    },
                });
            }
        }
        _ => (),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_preserve_comments_grouping_bom_crlf_unknown_fields_and_quote_style() {
        let source = "\u{feff}{\r\n // untouched 日本語\r\n signal: ['ignored', ['Group', { name:'a', wave:'01..', unknown:0x20, }, {}]], extra: 'keep',\r\n}\r\n";
        let doc = Document::parse(source).unwrap();
        assert_eq!(doc.lanes.len(), 1);
        let edited = doc.replace(source, 0, false, "10..").unwrap();
        assert_eq!(edited, source.replace("'01..'", "'10..'"));
        let renamed = doc
            .replace(source, 0, true, "quote ' and \\\" 日本語\n")
            .unwrap();
        assert_eq!(
            super::super::parse(&renamed).unwrap()["signal"][1][1]["name"],
            "quote ' and \\\" 日本語\n"
        );
        assert!(doc.replace(&edited, 0, false, "11..").is_err());
    }
    #[test]
    fn inline_comment_roundtrip() {
        let source = "{signal:[{name:'request',wave:'000000'}], /* preserve */ extra:42}";
        assert_eq!(parse_syntax(source).unwrap().to_string(), source);
    }
    #[test]
    fn ambiguous_fields_and_unpreserved_syntax_do_not_produce_silent_edits() {
        assert!(Document::parse("{signal:[{wave:'01',wave:'10'}]}").is_err());
        assert!(Document::parse("{signal:[{name:'a',name:'b',wave:'10'}]}").is_err());
        let source = "{signal:[{wave:'01'}]}";
        let doc = Document::parse(source).unwrap();
        assert!(!doc.can_rename(0));
        assert!(doc.replace(source, 0, true, "name").is_err());
        assert!(doc.replace(source, 0, false, "<01>").is_err());
    }
}
