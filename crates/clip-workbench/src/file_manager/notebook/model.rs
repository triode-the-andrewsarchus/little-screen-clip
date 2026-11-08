//! Passive nbformat 4 content. No kernel, HTML execution, or external resource loading.
use crate::file_source::FileSource;
use serde_json::Value;
use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Code,
    Markdown,
    Raw,
    Other,
}
impl Kind {
    pub const ALL: [Self; 4] = [Self::Code, Self::Markdown, Self::Raw, Self::Other];
    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Markdown => "Markdown",
            Self::Raw => "Raw",
            Self::Other => "Other",
        }
    }
}
pub(super) enum Output {
    Text {
        label: String,
        text: Arc<String>,
        error: bool,
    },
    Image {
        mime: &'static str,
        encoded: String,
    },
    Unsupported(String),
}
pub(super) struct Cell {
    pub kind: Kind,
    pub source: Arc<String>,
    pub title: String,
    pub execution: Option<i64>,
    pub metadata: String,
    pub outputs: Vec<Output>,
    pub output_count: usize,
}
impl Cell {
    pub fn has_error(&self) -> bool {
        self.outputs
            .iter()
            .any(|output| matches!(output, Output::Text { error: true, .. }))
    }
}
pub(super) struct Document {
    pub language: String,
    pub kernel: String,
    pub metadata: String,
    pub cells: Vec<Cell>,
}

pub(super) fn load(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let mut file = crate::file_source::open_regular_file(path, true).map_err(|e| e.to_string())?;
    let source = FileSource::capture_file(&file)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let document = parse(&bytes, token)?;
    if FileSource::capture_file(&file)? != source {
        return Err("Notebook changed while reading".into());
    }
    source.check()?;
    Ok(document)
}
fn parse(bytes: &[u8], token: &AtomicBool) -> Result<Document, String> {
    if token.load(Ordering::Relaxed) {
        return Err("Notebook loading cancelled".into());
    }
    if bytes.len() > MAX_BYTES {
        return Err("Notebook opening is limited to 32 MiB".into());
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid notebook JSON: {e}"))?;
    if value["nbformat"].as_u64() != Some(4) {
        return Err("This viewer supports nbformat 4 notebooks".into());
    }
    let cells = value["cells"]
        .as_array()
        .ok_or("Notebook cells must be an array")?;
    if cells.len() > 10_000 {
        return Err("Notebook opening is limited to 10,000 cells".into());
    }
    let mut document = Document {
        language: value["metadata"]["language_info"]["name"]
            .as_str()
            .unwrap_or("text")
            .chars()
            .take(80)
            .collect(),
        kernel: value["metadata"]["kernelspec"]["display_name"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect(),
        metadata: value.get("metadata").unwrap_or(&Value::Null).to_string(),
        cells: Vec::with_capacity(cells.len()),
    };
    for (index, cell) in cells.iter().enumerate() {
        if token.load(Ordering::Relaxed) {
            return Err("Notebook loading cancelled".into());
        }
        document
            .cells
            .push(parse_cell(cell, token).map_err(|error| format!("Cell {}: {error}", index + 1))?);
    }
    Ok(document)
}
fn parse_cell(cell: &Value, token: &AtomicBool) -> Result<Cell, String> {
    let kind = match cell["cell_type"].as_str().ok_or("Missing cell_type")? {
        "code" => Kind::Code,
        "markdown" => Kind::Markdown,
        "raw" => Kind::Raw,
        _ => Kind::Other,
    };
    let source = Arc::new(multiline(cell.get("source").ok_or("Missing cell source")?)?);
    let title = source
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Empty cell")
        .chars()
        .take(120)
        .collect();
    let mut parsed = Cell {
        kind,
        source,
        title,
        execution: cell["execution_count"].as_i64(),
        metadata: cell.get("metadata").unwrap_or(&Value::Null).to_string(),
        outputs: Vec::new(),
        output_count: 0,
    };
    if let Some(outputs) = cell.get("outputs") {
        let outputs = outputs.as_array().ok_or("Cell outputs must be an array")?;
        parsed.output_count = outputs.len();
        for output in outputs {
            if token.load(Ordering::Relaxed) {
                return Err("Notebook loading cancelled".into());
            }
            parse_output(output, &mut parsed.outputs)?;
        }
    }
    Ok(parsed)
}
fn multiline(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => parts
            .iter()
            .map(|part| part.as_str().ok_or("Text fragments must be strings".into()))
            .collect::<Result<Vec<_>, String>>()
            .map(|parts| parts.concat()),
        _ => Err("Cell text must be a string or an array of strings".into()),
    }
}
fn parse_output(value: &Value, outputs: &mut Vec<Output>) -> Result<(), String> {
    match value["output_type"].as_str().unwrap_or("unknown") {
        "stream" => outputs.push(Output::Text {
            label: value["name"]
                .as_str()
                .unwrap_or("stream")
                .chars()
                .take(80)
                .collect(),
            text: Arc::new(multiline(&value["text"])?),
            error: false,
        }),
        "error" => {
            let mut text = format!(
                "{}: {}",
                value["ename"].as_str().unwrap_or("Error"),
                value["evalue"].as_str().unwrap_or("")
            );
            if let Some(traceback) = value["traceback"].as_array() {
                for line in traceback.iter().filter_map(Value::as_str) {
                    text.push('\n');
                    text.push_str(line);
                }
            }
            outputs.push(Output::Text {
                label: "Saved error".into(),
                text: Arc::new(text),
                error: true,
            });
        }
        "display_data" | "execute_result" => parse_bundle(&value["data"], outputs)?,
        other => outputs.push(Output::Unsupported(format!(
            "Saved output: {}",
            other.chars().take(80).collect::<String>()
        ))),
    }
    Ok(())
}
fn parse_bundle(value: &Value, outputs: &mut Vec<Output>) -> Result<(), String> {
    let mut displayed = false;
    for mime in ["image/png", "image/jpeg"] {
        if let Some(image) = value.get(mime) {
            outputs.push(Output::Image {
                mime,
                encoded: multiline(image)?,
            });
            displayed = true;
            break;
        }
    }
    if let Some(text) = value.get("text/plain") {
        outputs.push(Output::Text {
            label: "Saved result".into(),
            text: Arc::new(multiline(text)?),
            error: false,
        });
        displayed = true;
    } else if let Some(json) = value.get("application/json") {
        outputs.push(Output::Text {
            label: "JSON result".into(),
            text: Arc::new(json.to_string()),
            error: false,
        });
        displayed = true;
    }
    if !displayed {
        let types = value
            .as_object()
            .map(|object| {
                object
                    .keys()
                    .take(16)
                    .map(|key| key.chars().take(80).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        outputs.push(Output::Unsupported(format!(
            "Saved output ({types}); no supported representation"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v4_cells_join_fragments_and_preserve_saved_results_and_metadata() {
        let value = serde_json::json!({"nbformat":4,"cells":[
            {"cell_type":"markdown","source":["# Heading\n","Body"],"metadata":{"tags":["intro"]}},
            {"cell_type":"code","source":"print('hello')","execution_count":7,"outputs":[
                {"output_type":"stream","name":"stdout","text":["hel","lo\n"]},
                {"output_type":"error","ename":"ValueError","evalue":"bad","traceback":["frame one","frame two"]},
                {"output_type":"display_data","data":{"image/png":["YW","Jj"],"text/plain":"A figure"}},
                {"output_type":"display_data","data":{"text/html":"<script>never run()</script>"}}
            ]}],"metadata":{"language_info":{"name":"python"},"authors":[{"name":"Ada"}]}});
        let notebook = parse(
            &serde_json::to_vec(&value).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(notebook.language, "python");
        assert!(notebook.metadata.contains("Ada"));
        assert_eq!(notebook.cells[0].source.as_str(), "# Heading\nBody");
        assert!(notebook.cells[0].metadata.contains("intro"));
        let cell = &notebook.cells[1];
        assert_eq!(cell.execution, Some(7));
        assert_eq!(cell.output_count, 4);
        assert!(cell.has_error());
        assert!(
            matches!(&cell.outputs[0], Output::Text { text, .. } if text.as_str() == "hello\n")
        );
        assert!(
            matches!(&cell.outputs[1], Output::Text { text, .. } if text.contains("frame one\nframe two"))
        );
        assert!(matches!(&cell.outputs[2], Output::Image { encoded, .. } if encoded == "YWJj"));
        assert!(matches!(&cell.outputs[4], Output::Unsupported(_)));
    }
    #[test]
    fn invalid_versions_text_shapes_and_cell_counts_fail_explicitly() {
        for value in [
            serde_json::json!({"nbformat":3,"cells":[]}),
            serde_json::json!({"nbformat":4,"cells":false}),
            serde_json::json!({"nbformat":4,"cells":[{"cell_type":"code","source":[42]}]}),
        ] {
            assert!(
                parse(
                    &serde_json::to_vec(&value).unwrap(),
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
        let too_many = serde_json::json!({"nbformat":4,"cells":vec![serde_json::json!({"cell_type":"raw","source":""});10_001]});
        assert!(
            parse(
                &serde_json::to_vec(&too_many).unwrap(),
                &AtomicBool::new(false)
            )
            .err()
            .unwrap()
            .contains("10,000")
        );
        assert!(
            parse(&vec![b' '; MAX_BYTES + 1], &AtomicBool::new(false))
                .err()
                .unwrap()
                .contains("32 MiB")
        );
        let value = serde_json::json!({"nbformat":4,"cells":[{"cell_type":"raw","source":"data"}]});
        assert!(parse(&serde_json::to_vec(&value).unwrap(), &AtomicBool::new(true)).is_err());
    }
}
