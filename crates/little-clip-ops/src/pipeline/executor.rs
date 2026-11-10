//! Graph topological evaluation and DAG execution engine.
//!
//! Performs cycle detection via Kahn's topological sort algorithm, dispatches nodes in
//! topological order, handles inter-process communication for CLI tools (FFmpeg, ImageMagick, jq, Python),
//! manages prompt inputs, and collects output payloads for slot state commitment or file saving.

use super::artifacts::Workspace;
use super::compiler::CompiledPipeline;
use super::types::{NodeGraph, NodeInstance, NodeKind, PortValue};
use base64::Engine;
use little_clip_core::CommandRequest;
use little_clip_core::item::{ClipItem, ClipPayload};
use little_clip_core::slot::SlotManager;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use uuid::Uuid;

use super::EXTERNAL_COMMAND_TIMEOUT;

/// Errors that can occur during graph compilation or node execution.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Graph contains a directed dependency cycle.
    #[error("Graph contains a cycle")]
    CycleDetected,
    /// A node required an input on a pin that was not connected or produced.
    #[error("Missing required input: {0}")]
    MissingInput(String),
    /// Subprocess or operation failed within a specific node.
    #[error("Node {0} execution failed: {1}")]
    NodeFailed(String, String),
    /// Underlying filesystem I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Graph structure does not match the node catalog or has invalid wires.
    #[error("Invalid graph: {0}")]
    InvalidGraph(String),
    /// Execution stopped after creating these files; completed writes are not rolled back.
    #[error("{cause}; files already saved: {saved_files:?}")]
    PartialExecution {
        /// Original execution or publication failure.
        #[source]
        cause: Box<PipelineError>,
        /// Files installed successfully before the failure.
        saved_files: Vec<PathBuf>,
    },
}

impl PipelineError {
    fn after_saves(self, saved_files: Vec<PathBuf>) -> Self {
        if saved_files.is_empty() {
            self
        } else {
            Self::PartialExecution {
                cause: Box::new(self),
                saved_files,
            }
        }
    }
}

pub use super::types::PipelineExecutionResult;

/// Evaluator that topologically executes a `NodeGraph` against current slot contents.
pub struct PipelineExecutor<'a> {
    graph: &'a NodeGraph,
    slot_mgr: &'a SlotManager,
    /// Node-specific prompt overrides mapped by node UUID.
    pub prompt_overrides: HashMap<Uuid, String>,
    /// Single prompt override applied to the first interactive prompt node.
    pub single_prompt_override: Option<String>,
}

impl<'a> PipelineExecutor<'a> {
    /// Constructs an evaluator that creates isolated scratch space for each execution.
    pub fn new(graph: &'a NodeGraph, slot_mgr: &'a SlotManager) -> Self {
        Self {
            graph,
            slot_mgr,
            prompt_overrides: HashMap::new(),
            single_prompt_override: None,
        }
    }

    /// Sets node-specific text prompt overrides.
    pub fn with_prompts(mut self, prompts: HashMap<Uuid, String>) -> Self {
        self.prompt_overrides = prompts;
        self
    }

    /// Sets a single prompt override applied to the first interactive prompt node.
    pub fn with_single_prompt(mut self, prompt: Option<String>) -> Self {
        self.single_prompt_override = prompt;
        self
    }

    /// Topologically sorts the graph nodes and executes all operations in dependency order.
    ///
    /// Scratch files are removed on return. Files referenced by the final hand items
    /// are retained independently of the executor and survive serialization.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError`] if graph compilation fails, if a required input is missing at
    /// runtime, if an external subprocess exits with an error code, if an artifact cannot be staged,
    /// or if script evaluation fails.
    /// [`PipelineError::PartialExecution`] retains paths of files already saved if
    /// a later step fails. Hand assignments and paste are committed only on success.
    pub fn execute(&self) -> Result<PipelineExecutionResult, PipelineError> {
        let compiled = CompiledPipeline::compile(self.graph)?;
        let workspace = Workspace::new()?;
        let mut outputs: HashMap<(Uuid, String), PortValue> = HashMap::new();
        let mut result = PipelineExecutionResult::default();

        for node in compiled.nodes() {
            let mut inputs = HashMap::new();
            for pin in &node.inputs {
                if let Some(conn) = compiled.input_for(node.id, &pin.name) {
                    if let Some(val) = outputs.get(&(conn.from_node, conn.from_pin.clone())) {
                        inputs.insert(pin.name.clone(), val.clone());
                    }
                }
            }

            let (node_outputs, preview) = self
                .execute_node(node, &inputs, &mut result, &workspace)
                .map_err(|error| error.after_saves(std::mem::take(&mut result.saved_files)))?;
            if let Some(p) = preview {
                result.node_previews.insert(node.id, p);
            }
            for (pin_name, val) in node_outputs {
                outputs.insert((node.id, pin_name), val);
            }
        }

        workspace.retain_outputs(&mut result).map_err(|error| {
            PipelineError::Io(error).after_saves(std::mem::take(&mut result.saved_files))
        })?;
        Ok(result)
    }

    fn extract_script_stdin(inputs: &HashMap<String, PortValue>) -> String {
        let in_val = inputs.values().next().cloned();
        match in_val {
            Some(PortValue::Text(t)) => t,
            Some(PortValue::FilePath(p))
            | Some(PortValue::ImagePath(p))
            | Some(PortValue::AudioPath(p)) => p.display().to_string(),
            Some(PortValue::FileBatch(paths)) => paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            None => String::new(),
        }
    }

    fn resolve_recipe_script_path(fname: &str) -> std::path::PathBuf {
        let recipes_dir = super::recipes::get_recipes_dir();
        if std::path::Path::new(fname).is_absolute() {
            std::path::PathBuf::from(fname)
        } else {
            recipes_dir.join(fname)
        }
    }

    fn execute_node(
        &self,
        node: &'a NodeInstance,
        inputs: &HashMap<String, PortValue>,
        result: &mut PipelineExecutionResult,
        workspace: &Workspace,
    ) -> Result<(Vec<(String, PortValue)>, Option<String>), PipelineError> {
        let temp_dir = workspace.path();
        let mut out = Vec::new();
        #[allow(unused_assignments)]
        let mut preview = None;
        macro_rules! path_arg {
            ($path:expr) => {
                $path.to_str().ok_or_else(|| {
                    PipelineError::NodeFailed(
                        node.title.clone(),
                        format!("Path is not valid UTF-8: {}", $path.display()),
                    )
                })?
            };
        }

        match &node.kind {
            NodeKind::LeftHandSource => {
                if let Some(item) = self.slot_mgr.left_hand() {
                    let val = source_port_value(node, item, workspace)?;
                    preview = Some(item.title.clone());
                    out.push(("Out".to_string(), val));
                } else {
                    return Err(PipelineError::MissingInput(
                        "Left Hand is empty".to_string(),
                    ));
                }
            }
            NodeKind::RightHandSource => {
                if let Some(item) = self.slot_mgr.right_hand() {
                    let val = source_port_value(node, item, workspace)?;
                    preview = Some(item.title.clone());
                    out.push(("Out".to_string(), val));
                } else {
                    return Err(PipelineError::MissingInput(
                        "Right Hand is empty".to_string(),
                    ));
                }
            }
            NodeKind::SlotSource(slot_id) => {
                if let Some(item) = self.slot_mgr.get(*slot_id) {
                    let val = source_port_value(node, item, workspace)?;
                    preview = Some(item.title.clone());
                    out.push(("Out".to_string(), val));
                } else {
                    return Err(PipelineError::MissingInput(format!(
                        "Slot {} is empty",
                        slot_id.label()
                    )));
                }
            }
            NodeKind::ConstantText => {
                let text = node.props.get("text").cloned().unwrap_or_default();
                preview = Some(text.clone());
                out.push(("Text".to_string(), PortValue::Text(text)));
            }
            NodeKind::ConstantNumber => {
                let val = node
                    .props
                    .get("value")
                    .cloned()
                    .unwrap_or_else(|| "0".to_string());
                preview = Some(val.clone());
                out.push(("Value".to_string(), PortValue::Text(val)));
            }
            NodeKind::PromptOrHandText => {
                let hand_prop = node
                    .props
                    .get("hand")
                    .map(|s| s.as_str())
                    .unwrap_or("right");
                let msg = node
                    .props
                    .get("prompt_message")
                    .map(|s| s.as_str())
                    .unwrap_or("Enter text:");
                let def = node
                    .props
                    .get("default_text")
                    .map(|s| s.as_str())
                    .unwrap_or("");

                let text = if let Some(t) = self.prompt_overrides.get(&node.id) {
                    t.clone()
                } else if let Some(t) = &self.single_prompt_override {
                    t.clone()
                } else if let Some(t) = node.props.get("provided_text") {
                    t.clone()
                } else {
                    let hand_item = if hand_prop.eq_ignore_ascii_case("left") {
                        self.slot_mgr.left_hand()
                    } else {
                        self.slot_mgr.right_hand()
                    };

                    let hand_text = hand_item.and_then(|item| match &item.payload {
                        ClipPayload::Text(t) | ClipPayload::Markdown(t) if !t.trim().is_empty() => {
                            Some(t.clone())
                        }
                        ClipPayload::Url(u) => Some(u.to_string()),
                        _ => None,
                    });

                    if let Some(t) = hand_text {
                        t
                    } else {
                        prompt_desktop_entry(&node.title, msg, def)
                            .unwrap_or_else(|| def.to_string())
                    }
                };

                preview = Some(text.clone());
                out.push(("Text".to_string(), PortValue::Text(text)));
            }

            // --- Text Operators ---
            NodeKind::TextTrim => {
                let text = self.get_input_text(inputs, &["In"])?;
                let trimmed = text.trim().to_string();
                preview = Some(trimmed.clone());
                out.push(("Out".to_string(), PortValue::Text(trimmed)));
            }
            NodeKind::TextSlugify => {
                let text = self.get_input_text(inputs, &["In"])?;
                let slug = run_python_snippet(
                    r#"import re, sys
text = sys.stdin.read().strip()
slug = re.sub(r'[\W_]+', '-', text).strip('-').lower()
sys.stdout.write(slug)
"#,
                    &text,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                preview = Some(slug.clone());
                out.push(("Out".to_string(), PortValue::Text(slug)));
            }
            NodeKind::TextCase => {
                let text = self.get_input_text(inputs, &["In"])?;
                let target_case = node
                    .props
                    .get("target_case")
                    .map(|s| s.as_str())
                    .unwrap_or("snake");
                let script = format!(
                    r#"import re, sys
text = sys.stdin.read()
case = "{}"
if case == "upper":
    sys.stdout.write(text.upper())
elif case == "lower":
    sys.stdout.write(text.lower())
elif case == "snake":
    s = re.sub(r'[\W_]+', '_', text).strip('_').lower()
    sys.stdout.write(s)
elif case == "camel":
    words = re.split(r'[\W_]+', text)
    words = [w for w in words if w]
    if words:
        res = words[0].lower() + ''.join(w.capitalize() for w in words[1:])
        sys.stdout.write(res)
    else:
        sys.stdout.write(text)
else:
    sys.stdout.write(text)
"#,
                    target_case
                );
                let cased = run_python_snippet(&script, &text)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                preview = Some(cased.clone());
                out.push(("Out".to_string(), PortValue::Text(cased)));
            }
            NodeKind::TextRegexReplace => {
                let text = self.get_input_text(inputs, &["In"])?;
                let pattern = node
                    .props
                    .get("pattern")
                    .map(|s| s.as_str())
                    .unwrap_or(r"\s+");
                let replacement = node
                    .props
                    .get("replacement")
                    .map(|s| s.as_str())
                    .unwrap_or(" ");
                let script = "import re, sys; sys.stdout.write(re.sub(sys.argv[1], sys.argv[2], sys.stdin.read()))";
                let res =
                    run_command_with_stdin("python3", &["-c", script, pattern, replacement], &text)
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                preview = Some(res.clone());
                out.push(("Out".to_string(), PortValue::Text(res)));
            }

            NodeKind::TextSplitLines => {
                let text = self.get_input_text(inputs, &["In"])?;
                let delim = node
                    .props
                    .get("delimiter")
                    .map(|s| s.as_str())
                    .unwrap_or(r"\n");
                let delim_clean = if delim == r"\n" { "\n" } else { delim };
                let lines: Vec<&str> = text.split(delim_clean).collect();
                preview = Some(format!("{} items", lines.len()));
                let joined = lines.join("\n");
                out.push(("Out".to_string(), PortValue::Text(joined)));
            }
            NodeKind::TextJoin => {
                let text = self.get_input_text(inputs, &["In"])?;
                let sep = node
                    .props
                    .get("separator")
                    .map(|s| s.as_str())
                    .unwrap_or(", ");
                let lines: Vec<&str> = text.lines().collect();
                let joined = lines.join(sep);
                preview = Some(joined.clone());
                out.push(("Out".to_string(), PortValue::Text(joined)));
            }
            NodeKind::TextJsonQuery => {
                let text = self.get_input_text(inputs, &["JSON In", "In"])?;
                let filter = node.props.get("filter").map(|s| s.as_str()).unwrap_or(".");
                let res = run_command_with_stdin("jq", &[filter], &text)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                preview = Some(res.clone());
                out.push(("JSON Out".to_string(), PortValue::Text(res)));
            }
            NodeKind::TextBase64 => {
                let text = self.get_input_text(inputs, &["In"])?;
                let mode = node
                    .props
                    .get("mode")
                    .map(|s| s.as_str())
                    .unwrap_or("encode");
                let res = if mode.eq_ignore_ascii_case("decode") {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(text.trim().as_bytes())
                        .map_err(|e| {
                            PipelineError::NodeFailed(
                                node.title.clone(),
                                format!("Base64 decode error: {}", e),
                            )
                        })?;
                    String::from_utf8(bytes).map_err(|e| {
                        PipelineError::NodeFailed(
                            node.title.clone(),
                            format!("Invalid UTF-8: {}", e),
                        )
                    })?
                } else {
                    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
                };
                preview = Some(res.clone());
                out.push(("Out".to_string(), PortValue::Text(res)));
            }

            // --- Image Transforms (ImageMagick) ---
            NodeKind::ImagePadFrame => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let border_color = node
                    .props
                    .get("border_color")
                    .map(|s| s.as_str())
                    .unwrap_or("#1e293b");
                let border_size = node
                    .props
                    .get("border_size")
                    .map(|s| s.as_str())
                    .unwrap_or("24");
                let out_path = temp_dir.join(format!("framed_{}.png", Uuid::new_v4()));

                super::content::magick(
                    workspace,
                    &[&in_path],
                    &["-bordercolor", border_color, "-border", border_size],
                    &out_path,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Framed (+{}px)", border_size));
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageAddTitle => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let caption = inputs
                    .get("Caption")
                    .and_then(|v| v.as_text())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        node.props
                            .get("caption")
                            .cloned()
                            .unwrap_or_else(|| "Caption".to_string())
                    });
                let bar_color = node
                    .props
                    .get("bar_color")
                    .map(|s| s.as_str())
                    .unwrap_or("#0f172a");
                let text_color = node
                    .props
                    .get("text_color")
                    .map(|s| s.as_str())
                    .unwrap_or("#f8fafc");
                let bar_height = node
                    .props
                    .get("bar_height")
                    .map(|s| s.as_str())
                    .unwrap_or("48");
                let font_size = node
                    .props
                    .get("font_size")
                    .map(|s| s.as_str())
                    .unwrap_or("24");
                let out_path = temp_dir.join(format!("titled_{}.png", Uuid::new_v4()));

                super::content::magick(
                    workspace,
                    &[&in_path],
                    &[
                        "-background",
                        bar_color,
                        "-fill",
                        text_color,
                        "-gravity",
                        "south",
                        "-splice",
                        &format!("0x{}", bar_height),
                        "-pointsize",
                        font_size,
                        "-annotate",
                        "+0+10",
                        &caption,
                    ],
                    &out_path,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Titled: {}", caption));
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageCrop => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let geom = node
                    .props
                    .get("geometry")
                    .map(|s| s.as_str())
                    .unwrap_or("1000x1000+0+0");
                let out_path = temp_dir.join(format!("cropped_{}.png", Uuid::new_v4()));

                super::content::magick(
                    workspace,
                    &[&in_path],
                    &["-crop", geom, "+repage"],
                    &out_path,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Crop {}", geom));
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageResize => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let max_w = node
                    .props
                    .get("max_width")
                    .map(|s| s.as_str())
                    .unwrap_or("1920");
                let max_h = node
                    .props
                    .get("max_height")
                    .map(|s| s.as_str())
                    .unwrap_or("1080");
                let out_path = temp_dir.join(format!("resized_{}.png", Uuid::new_v4()));

                super::content::magick(
                    workspace,
                    &[&in_path],
                    &["-resize", &format!("{}x{}>", max_w, max_h)],
                    &out_path,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Resize {}x{}", max_w, max_h));
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageConvertWebp => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let q = node
                    .props
                    .get("quality")
                    .map(|s| s.as_str())
                    .unwrap_or("90");
                let out_path = temp_dir.join(format!("{}.webp", Uuid::new_v4()));

                super::content::magick(workspace, &[&in_path], &["-quality", q], &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some("Converted to WebP".to_string());
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageRotate => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let deg = node
                    .props
                    .get("degrees")
                    .map(|s| s.as_str())
                    .unwrap_or("90");
                let out_path = temp_dir.join(format!("rotated_{}.png", Uuid::new_v4()));

                super::content::magick(workspace, &[&in_path], &["-rotate", deg], &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Rotated {}°", deg));
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageStitch => {
                let left_path = self.get_input_file(inputs, &["Left Image", "In 1", "In"])?;
                let right_path = self.get_input_file(inputs, &["Right Image", "In 2"])?;
                let out_path = temp_dir.join(format!("stitched_{}.png", Uuid::new_v4()));

                super::content::magick(
                    workspace,
                    &[&left_path, &right_path],
                    &["+append"],
                    &out_path,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some("Stitched 2 Images".to_string());
                out.push(("Combined".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageStripExif => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let out_path = temp_dir.join(format!("stripped_{}.png", Uuid::new_v4()));

                super::content::magick(workspace, &[&in_path], &["-strip"], &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some("EXIF stripped".to_string());
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageDiff => {
                let left_path =
                    self.get_input_file(inputs, &["Base Image", "Left", "In 1", "In"])?;
                let right_path =
                    self.get_input_file(inputs, &["Compare Image", "Right", "In 2"])?;
                let threshold: f32 = node
                    .props
                    .get("threshold")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.10);
                let b1 = super::content::image_bytes(&left_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let b2 = super::content::image_bytes(&right_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let opts = crate::image_diff::ImageDiffOptions {
                    threshold,
                    ..Default::default()
                };
                let res = crate::image_diff::compare_image_bytes_with_mime(
                    &b1,
                    Some(&little_clip_core::item::guess_image_mime(&left_path)),
                    &b2,
                    Some(&little_clip_core::item::guess_image_mime(&right_path)),
                    &opts,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let mask_png = res
                    .encode_mask_png()
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let out_mask = temp_dir.join(format!("diff_mask_{}.png", Uuid::new_v4()));
                workspace
                    .write_output(&out_mask, &mask_png)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;

                let overlay_png = res
                    .encode_overlay_png()
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let out_overlay = temp_dir.join(format!("diff_overlay_{}.png", Uuid::new_v4()));
                workspace
                    .write_output(&out_overlay, &overlay_png)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;

                preview = Some(format!(
                    "Diff: {} altered pixels ({:.2}%)",
                    res.diff_count, res.diff_percentage
                ));
                out.push(("Mask".to_string(), PortValue::ImagePath(out_mask)));
                out.push(("Overlay".to_string(), PortValue::ImagePath(out_overlay)));
            }

            // --- Audio Transforms (FFmpeg) ---
            NodeKind::AudioNormalize => {
                let in_path = self.get_input_file(inputs, &["Audio In", "In"])?;
                let target = node
                    .props
                    .get("target_lufs")
                    .map(|s| s.as_str())
                    .unwrap_or("-14");
                let tp = node.props.get("tp").map(|s| s.as_str()).unwrap_or("-1.5");
                let out_path = temp_dir.join(format!("norm_{}.wav", Uuid::new_v4()));

                let filter = format!("loudnorm=I={}:LRA=11:TP={}", target, tp);
                super::content::audio(workspace, &[&in_path], &["-af", &filter], &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Normalized ({} LUFS)", target));
                out.push(("Audio Out".to_string(), PortValue::AudioPath(out_path)));
            }
            NodeKind::AudioTrimSilence => {
                let in_path = self.get_input_file(inputs, &["Audio In", "In"])?;
                let db = node
                    .props
                    .get("threshold_db")
                    .map(|s| s.as_str())
                    .unwrap_or("-50");
                let out_path = temp_dir.join(format!("trimmed_{}.wav", Uuid::new_v4()));

                let filter = format!(
                    "silenceremove=start_periods=1:start_duration=0.1:start_threshold={}dB:stop_periods=1:stop_duration=0.1:stop_threshold={}dB",
                    db, db
                );
                super::content::audio(workspace, &[&in_path], &["-af", &filter], &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Trimmed Silence ({}dB)", db));
                out.push(("Audio Out".to_string(), PortValue::AudioPath(out_path)));
            }
            NodeKind::AudioAdjustGain => {
                let in_path = self.get_input_file(inputs, &["Audio In", "In"])?;
                let db = node
                    .props
                    .get("gain_db")
                    .map(|s| s.as_str())
                    .unwrap_or("3.0");
                let out_path = temp_dir.join(format!("gain_{}.wav", Uuid::new_v4()));

                let filter = format!("volume={}dB", db);
                super::content::audio(workspace, &[&in_path], &["-af", &filter], &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Gain {}dB", db));
                out.push(("Audio Out".to_string(), PortValue::AudioPath(out_path)));
            }
            NodeKind::AudioConcat => {
                let in1 = self.get_input_file(inputs, &["Clip 1", "In 1", "In"])?;
                let in2 = self.get_input_file(inputs, &["Clip 2", "In 2"])?;
                let out_path = temp_dir.join(format!("concat_{}.wav", Uuid::new_v4()));

                let filter = "[0:0][1:0]concat=n=2:v=0:a=1[out]";
                super::content::audio(
                    workspace,
                    &[&in1, &in2],
                    &["-filter_complex", filter, "-map", "[out]"],
                    &out_path,
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some("Concatenated Audio".to_string());
                out.push(("Audio Out".to_string(), PortValue::AudioPath(out_path)));
            }

            // --- File Transforms ---
            NodeKind::FileSha256 => {
                let in_val = inputs
                    .values()
                    .next()
                    .ok_or_else(|| PipelineError::MissingInput("In".to_string()))?;
                let hash = match in_val {
                    PortValue::FilePath(p) | PortValue::ImagePath(p) | PortValue::AudioPath(p) => {
                        super::content::checksum(p)
                            .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?
                    }
                    PortValue::Text(t) => {
                        let out = run_command_with_stdin("sha256sum", &[], t)
                            .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                        out.split_whitespace().next().unwrap_or("").to_string()
                    }
                    PortValue::FileBatch(paths) if paths.len() == 1 => {
                        super::content::checksum(&paths[0])
                            .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?
                    }
                    PortValue::FileBatch(paths) => {
                        let mut hashes = Vec::new();
                        for p in paths {
                            let h = super::content::checksum(p)
                                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                            hashes.push(format!("{}: {}", p.display(), h));
                        }
                        hashes.join("\n")
                    }
                };
                preview = Some(hash.clone());
                out.push(("Checksum".to_string(), PortValue::Text(hash)));
            }
            NodeKind::FileZip => {
                let paths = match inputs.values().next() {
                    Some(PortValue::FileBatch(p)) => p.clone(),
                    Some(PortValue::FilePath(p))
                    | Some(PortValue::ImagePath(p))
                    | Some(PortValue::AudioPath(p)) => vec![p.clone()],
                    _ => return Err(PipelineError::MissingInput("Files In".to_string())),
                };
                let archive_name = node
                    .props
                    .get("archive_name")
                    .map(|s| s.as_str())
                    .unwrap_or("archive.zip");
                let mut components = Path::new(archive_name).components();
                if !matches!(
                    (components.next(), components.next()),
                    (Some(Component::Normal(name)), None) if name == std::ffi::OsStr::new(archive_name)
                ) {
                    return Err(PipelineError::NodeFailed(
                        node.title.clone(),
                        "Archive name must be a filename within the workspace".into(),
                    ));
                }
                let out_path = temp_dir.join(archive_name);

                super::archive::write(workspace, &paths, &out_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;

                preview = Some(format!("Zipped {} files", paths.len()));
                out.push(("Zip Archive".to_string(), PortValue::FilePath(out_path)));
            }
            NodeKind::FileExtractName => {
                let p = self.get_input_file(inputs, &["File In", "In"])?;
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                preview = Some(name.clone());
                out.push(("Filename".to_string(), PortValue::Text(name)));
            }

            // --- Scripting Escape Hatch ---
            NodeKind::PythonScript => {
                let stdin_data = Self::extract_script_stdin(inputs);
                let script_file = node.props.get("script_file");
                let stdout = if let Some(fname) = script_file {
                    let fpath = Self::resolve_recipe_script_path(fname);
                    if !fpath.exists() {
                        return Err(PipelineError::NodeFailed(
                            node.title.clone(),
                            format!("Script file not found: {}", fpath.display()),
                        ));
                    }
                    run_command_with_stdin("python3", &[path_arg!(fpath)], &stdin_data)
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?
                } else {
                    let script = node
                        .props
                        .get("script")
                        .map(|s| s.as_str())
                        .unwrap_or("import sys\nsys.stdout.write(sys.stdin.read())");
                    run_python_snippet(script, &stdin_data)
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?
                };

                preview = Some(stdout.clone());
                out.push(("Output".to_string(), PortValue::Text(stdout)));
            }
            NodeKind::BashCommand => {
                let stdin_data = Self::extract_script_stdin(inputs);
                let script_file = node.props.get("script_file");
                let stdout = if let Some(fname) = script_file {
                    let fpath = Self::resolve_recipe_script_path(fname);
                    if !fpath.exists() {
                        return Err(PipelineError::NodeFailed(
                            node.title.clone(),
                            format!("Script file not found: {}", fpath.display()),
                        ));
                    }
                    run_command_with_stdin("bash", &[path_arg!(fpath)], &stdin_data)
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?
                } else {
                    let cmd = node
                        .props
                        .get("command")
                        .map(|s| s.as_str())
                        .unwrap_or("cat");
                    run_command_with_stdin("bash", &["-c", cmd], &stdin_data)
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?
                };

                preview = Some(stdout.clone());
                out.push(("Output".to_string(), PortValue::Text(stdout)));
            }

            // --- Sinks ---
            NodeKind::SetLeftHand => {
                let in_val = inputs
                    .values()
                    .next()
                    .ok_or_else(|| PipelineError::MissingInput("In".to_string()))?;
                let clip = in_val.clone().into_clip_item("Pipeline Result");
                preview = Some(format!("Set [LH]: {}", clip.title));
                result.left_hand_item = Some(clip);
            }
            NodeKind::SetRightHand => {
                let in_val = inputs
                    .values()
                    .next()
                    .ok_or_else(|| PipelineError::MissingInput("In".to_string()))?;
                let clip = in_val.clone().into_clip_item("Pipeline Result");
                preview = Some(format!("Set [RH]: {}", clip.title));
                result.right_hand_item = Some(clip);
            }
            NodeKind::SaveToDisk => {
                let in_val = inputs
                    .values()
                    .next()
                    .ok_or_else(|| PipelineError::MissingInput("In".to_string()))?;
                let dest_str = node
                    .props
                    .get("destination")
                    .map(|s| s.as_str())
                    .unwrap_or("~/Downloads");
                let dest_expanded = shellexpand(dest_str);
                let before = result.saved_files.len();
                super::save::save(in_val, &dest_expanded, &mut result.saved_files).map_err(
                    |error| PipelineError::NodeFailed(node.title.clone(), error.to_string()),
                )?;
                preview = Some(format!("Saved {} files", result.saved_files.len() - before));
            }
            NodeKind::PasteToApp => {
                let in_val = inputs
                    .values()
                    .next()
                    .ok_or_else(|| PipelineError::MissingInput("In".to_string()))?;
                let clip = in_val.clone().into_clip_item("Pipeline Result");
                preview = Some(format!("Paste: {}", clip.title));
                result.left_hand_item = Some(clip);
                result.should_paste = true;
            }
        }

        Ok((out, preview))
    }

    fn get_input_text(
        &self,
        inputs: &HashMap<String, PortValue>,
        preferred_names: &[&str],
    ) -> Result<String, PipelineError> {
        for &name in preferred_names {
            if let Some(v) = inputs.get(name) {
                if let Some(t) = v.as_text() {
                    return Ok(t.to_string());
                }
            }
        }
        if let Some(v) = inputs.values().next() {
            if let Some(t) = v.as_text() {
                return Ok(t.to_string());
            }
        }
        Err(PipelineError::MissingInput(preferred_names.join(" or ")))
    }

    fn get_input_file(
        &self,
        inputs: &HashMap<String, PortValue>,
        preferred_names: &[&str],
    ) -> Result<PathBuf, PipelineError> {
        for &name in preferred_names {
            if let Some(v) = inputs.get(name) {
                if let Some(p) = v.as_path() {
                    return Ok(p.to_path_buf());
                }
                if let Some(t) = v.as_text() {
                    let p = PathBuf::from(t.trim());
                    if p.is_file() {
                        return Ok(p);
                    }
                }
            }
        }
        if let Some(v) = inputs.values().next() {
            if let Some(p) = v.as_path() {
                return Ok(p.to_path_buf());
            }
            if let Some(t) = v.as_text() {
                let p = PathBuf::from(t.trim());
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
        Err(PipelineError::MissingInput(preferred_names.join(" or ")))
    }
}

fn source_port_value(
    node: &NodeInstance,
    item: &ClipItem,
    workspace: &Workspace,
) -> Result<PortValue, PipelineError> {
    workspace
        .source_value(item)
        .map_err(|error| PipelineError::NodeFailed(node.title.clone(), error.to_string()))
}

fn shellexpand(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

fn prompt_desktop_entry(title: &str, message: &str, default_val: &str) -> Option<String> {
    // Only attempt GUI dialogs if an X11 or Wayland display session is active
    let has_display = std::env::var("DISPLAY")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    if !has_display {
        return None;
    }

    if let Ok(out) = Command::new("zenity")
        .args([
            "--entry",
            &format!("--title={}", title),
            &format!("--text={}", message),
            &format!("--entry-text={}", default_val),
            "--timeout=30",
        ])
        .output()
    {
        if out.status.success() {
            let res = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !res.is_empty() {
                return Some(res);
            }
        }
    }

    if let Ok(out) = Command::new("yad")
        .args([
            "--entry",
            &format!("--title={}", title),
            &format!("--text={}", message),
            &format!("--entry-text={}", default_val),
            "--timeout=30",
        ])
        .output()
    {
        if out.status.success() {
            let res = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !res.is_empty() {
                return Some(res);
            }
        }
    }

    None
}

fn run_command_with_stdin(cmd: &str, args: &[&str], stdin_data: &str) -> Result<String, String> {
    let request = args.iter().fold(
        CommandRequest::new(cmd)
            .timeout(EXTERNAL_COMMAND_TIMEOUT)
            .stdin_bytes(stdin_data.as_bytes()),
        |request, arg| request.arg(*arg),
    );
    let output = request.run().map_err(|error| error.to_string())?;

    if !output.status.success() {
        return Err(format!("{} error: {}", cmd, output.stderr_lossy()));
    }

    Ok(output.stdout_lossy())
}

fn run_python_snippet(code: &str, stdin_data: &str) -> Result<String, String> {
    run_command_with_stdin("python3", &["-c", code], stdin_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::catalog::{find_template, instantiate_template};
    use crate::pipeline::recipes::{make_clean_and_slugify_recipe, make_pad_and_caption_recipe};

    #[test]
    fn checksum_batch_reports_protected_input_instead_of_an_empty_digest() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("ordinary.txt");
        std::fs::write(&source, b"ordinary bytes").unwrap();
        let node = instantiate_template(find_template("file_sha256").unwrap(), [0.0, 0.0]);
        let graph = NodeGraph {
            nodes: vec![node],
            ..Default::default()
        };
        let slots = SlotManager::default();
        let executor = PipelineExecutor::new(&graph, &slots);
        let workspace = Workspace::new().unwrap();
        let single = HashMap::from([("In".into(), PortValue::FileBatch(vec![source.clone()]))]);
        let (values, _) = executor
            .execute_node(
                &graph.nodes[0],
                &single,
                &mut PipelineExecutionResult::default(),
                &workspace,
            )
            .unwrap();
        assert!(matches!(&values[0].1, PortValue::Text(hash) if hash.len() == 64));
        let inputs = HashMap::from([(
            "In".into(),
            PortValue::FileBatch(vec![source, "/proc/version".into()]),
        )]);
        let error = executor
            .execute_node(
                &graph.nodes[0],
                &inputs,
                &mut PipelineExecutionResult::default(),
                &workspace,
            )
            .unwrap_err();
        assert!(matches!(error, PipelineError::NodeFailed(_, _)));
        assert!(error.to_string().contains("/proc/version"));
    }
    use crate::pipeline::types::Connection;

    fn save_graph(destination: &Path) -> NodeGraph {
        let source = instantiate_template(find_template("source_lh").unwrap(), [0.0, 0.0]);
        let mut sink = instantiate_template(find_template("sink_save_disk").unwrap(), [100.0, 0.0]);
        sink.props
            .insert("destination".into(), destination.to_str().unwrap().into());
        NodeGraph {
            connections: vec![Connection {
                from_node: source.id,
                from_pin: "Out".into(),
                to_node: sink.id,
                to_pin: "In".into(),
            }],
            nodes: vec![source, sink],
            ..Default::default()
        }
    }

    #[test]
    fn saving_pipeline_files_refuses_to_overwrite_an_existing_destination() {
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("input.bin");
        let destination = fixture.path().join("output");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&input, b"new bytes").unwrap();
        std::fs::write(destination.join("input.bin"), b"existing bytes").unwrap();
        let graph = save_graph(&destination);
        let mut slots = SlotManager::default();
        slots.push_left(ClipItem::from_files(vec![input]));
        assert!(PipelineExecutor::new(&graph, &slots).execute().is_err());
        assert_eq!(
            std::fs::read(destination.join("input.bin")).unwrap(),
            b"existing bytes"
        );
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 1);
    }

    #[test]
    fn pipeline_failure_reports_files_already_saved_in_the_batch() {
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("first.bin");
        std::fs::write(&input, b"completed bytes").unwrap();
        let destination = fixture.path().join("output");
        let graph = save_graph(&destination);
        let mut slots = SlotManager::default();
        slots.push_left(ClipItem::from_files(vec![
            input,
            fixture.path().join("missing.bin"),
        ]));
        let error = PipelineExecutor::new(&graph, &slots).execute().unwrap_err();
        let completed = destination.join("first.bin");
        assert_eq!(std::fs::read(&completed).unwrap(), b"completed bytes");
        assert!(
            error.to_string().contains(completed.to_str().unwrap()),
            "{error}"
        );
        assert!(
            matches!(error, PipelineError::PartialExecution { saved_files, .. } if saved_files == [completed])
        );
    }

    #[test]
    fn later_node_failure_keeps_previously_completed_save_results() {
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("input.bin");
        std::fs::write(&input, b"completed bytes").unwrap();
        let destination = fixture.path().join("output");
        let invalid_destination = fixture.path().join("not-a-directory");
        std::fs::write(&invalid_destination, b"untouched").unwrap();
        let mut graph = save_graph(&destination);
        let mut failing_sink = graph.nodes[1].clone();
        failing_sink.id = Uuid::new_v4();
        failing_sink.props.insert(
            "destination".into(),
            invalid_destination.to_str().unwrap().into(),
        );
        graph.connections.push(Connection {
            from_node: graph.nodes[0].id,
            from_pin: "Out".into(),
            to_node: failing_sink.id,
            to_pin: "In".into(),
        });
        graph.nodes.push(failing_sink);
        let mut slots = SlotManager::default();
        slots.push_left(ClipItem::from_files(vec![input]));
        let error = PipelineExecutor::new(&graph, &slots).execute().unwrap_err();
        assert!(
            matches!(error, PipelineError::PartialExecution { saved_files, .. } if saved_files == [destination.join("input.bin")])
        );
        assert_eq!(
            std::fs::read(destination.join("input.bin")).unwrap(),
            b"completed bytes"
        );
        assert_eq!(std::fs::read(invalid_destination).unwrap(), b"untouched");
    }

    #[test]
    fn archive_install_failure_reports_previously_completed_saves() {
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("source.bin");
        std::fs::write(&input, b"completed save").unwrap();
        let destination = fixture.path().join("output");
        let mut graph = save_graph(&destination);
        let mut archive = instantiate_template(find_template("file_zip").unwrap(), [200.0, 0.0]);
        archive
            .props
            .insert("archive_name".into(), format!("{}.zip", "a".repeat(300)));
        // Both depend on the sole source; connection order enqueues save before ZIP.
        graph.connections.push(Connection {
            from_node: graph.nodes[0].id,
            from_pin: "Out".into(),
            to_node: archive.id,
            to_pin: "Files In".into(),
        });
        graph.nodes.push(archive);
        let mut slots = SlotManager::default();
        slots.push_left(ClipItem::from_files(vec![input]));
        let error = PipelineExecutor::new(&graph, &slots).execute().unwrap_err();
        let completed = destination.join("source.bin");
        assert_eq!(std::fs::read(&completed).unwrap(), b"completed save");
        assert!(
            matches!(error, PipelineError::PartialExecution { saved_files, .. } if saved_files == [completed])
        );
    }

    #[test]
    fn generated_archive_survives_executor_and_result_serialization() {
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("notes.txt");
        std::fs::write(&input, b"pipeline retention fixture").unwrap();
        let source = instantiate_template(find_template("source_lh").unwrap(), [0.0, 0.0]);
        let archive = instantiate_template(find_template("file_zip").unwrap(), [100.0, 0.0]);
        let left = instantiate_template(find_template("sink_lh").unwrap(), [200.0, 0.0]);
        let right = instantiate_template(find_template("sink_rh").unwrap(), [200.0, 100.0]);
        let mut graph = NodeGraph {
            connections: vec![
                Connection {
                    from_node: source.id,
                    from_pin: "Out".into(),
                    to_node: archive.id,
                    to_pin: "Files In".into(),
                },
                Connection {
                    from_node: archive.id,
                    from_pin: "Zip Archive".into(),
                    to_node: left.id,
                    to_pin: "In".into(),
                },
                Connection {
                    from_node: archive.id,
                    from_pin: "Zip Archive".into(),
                    to_node: right.id,
                    to_pin: "In".into(),
                },
            ],
            nodes: vec![source, archive, left, right],
            ..Default::default()
        };
        let mut slots = SlotManager::default();
        slots.push_left(ClipItem::from_files(vec![input]));
        // Matches the daemon's temporary-executor handoff to another task.
        let result = PipelineExecutor::new(&graph, &slots).execute().unwrap();
        let item: ClipItem = serde_json::from_slice(
            &serde_json::to_vec(result.left_hand_item.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        let path = item.file_path().unwrap();
        assert_eq!(
            result
                .right_hand_item
                .as_ref()
                .unwrap()
                .file_path()
                .unwrap(),
            path
        );
        drop(result);
        assert!(std::fs::read(&path).unwrap().starts_with(b"PK"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        for invalid in [
            "../escaped.zip",
            "nested/file.zip",
            "/tmp/escaped.zip",
            "trailing.zip/",
            "trailing.zip/.",
            "",
            ".",
        ] {
            graph.nodes[1]
                .props
                .insert("archive_name".into(), invalid.into());
            assert!(matches!(
                PipelineExecutor::new(&graph, &slots).execute(),
                Err(PipelineError::NodeFailed(_, _))
            ));
        }
    }

    #[test]
    fn test_topological_sort_and_cycle_detection() {
        let t_const = find_template("source_const_text").unwrap();
        let t_trim = find_template("text_trim").unwrap();

        let n1 = instantiate_template(t_const, [0.0, 0.0]);
        let n2 = instantiate_template(t_trim, [100.0, 0.0]);

        // Valid DAG: n1 -> n2
        let graph = NodeGraph {
            nodes: vec![n1.clone(), n2.clone()],
            connections: vec![Connection {
                from_node: n1.id,
                from_pin: "Text".to_string(),
                to_node: n2.id,
                to_pin: "In".to_string(),
            }],
            ..Default::default()
        };

        let sorted = CompiledPipeline::compile(&graph)
            .expect("Should compile DAG cleanly")
            .nodes()
            .to_vec();
        assert_eq!(sorted[0].id, n1.id);
        assert_eq!(sorted[1].id, n2.id);

        // Cyclic graph: two valid transform nodes wired into each other.
        let n3 = instantiate_template(t_trim, [200.0, 0.0]);
        let cyclic_graph = NodeGraph {
            nodes: vec![n2.clone(), n3.clone()],
            connections: vec![
                Connection {
                    from_node: n2.id,
                    from_pin: "Out".to_string(),
                    to_node: n3.id,
                    to_pin: "In".to_string(),
                },
                Connection {
                    from_node: n3.id,
                    from_pin: "Out".to_string(),
                    to_node: n2.id,
                    to_pin: "In".to_string(),
                },
            ],
            ..Default::default()
        };

        assert!(matches!(
            CompiledPipeline::compile(&cyclic_graph),
            Err(PipelineError::CycleDetected)
        ));
    }

    #[test]
    fn validation_rejects_dangling_and_duplicate_input_connections() {
        let source = instantiate_template(find_template("source_const_text").unwrap(), [0.0, 0.0]);
        let sink = instantiate_template(find_template("sink_lh").unwrap(), [100.0, 0.0]);
        let missing_node = Uuid::new_v4();
        let dangling = NodeGraph {
            nodes: vec![source.clone(), sink.clone()],
            connections: vec![Connection {
                from_node: missing_node,
                from_pin: "Out".to_string(),
                to_node: sink.id,
                to_pin: "In".to_string(),
            }],
            ..Default::default()
        };
        assert!(matches!(
            CompiledPipeline::compile(&dangling),
            Err(PipelineError::InvalidGraph(_))
        ));

        let duplicate_input = NodeGraph {
            nodes: vec![source.clone(), sink.clone()],
            connections: vec![
                Connection {
                    from_node: source.id,
                    from_pin: "Text".to_string(),
                    to_node: sink.id,
                    to_pin: "In".to_string(),
                },
                Connection {
                    from_node: source.id,
                    from_pin: "Text".to_string(),
                    to_node: sink.id,
                    to_pin: "In".to_string(),
                },
            ],
            ..Default::default()
        };
        assert!(matches!(
            CompiledPipeline::compile(&duplicate_input),
            Err(PipelineError::InvalidGraph(_))
        ));
    }

    #[test]
    fn test_recipe_yaml_roundtrip() {
        let recipe = make_pad_and_caption_recipe();
        let yaml = serde_yaml::to_string(&recipe).expect("Serialization failed");
        assert!(yaml.contains("Pad and Caption"));
        assert!(yaml.contains("ImagePadFrame"));

        let deserialized: crate::pipeline::types::Recipe =
            serde_yaml::from_str(&yaml).expect("Deserialization failed");
        assert_eq!(deserialized.name, recipe.name);
        assert_eq!(deserialized.graph.nodes.len(), 5);
        assert_eq!(deserialized.graph.connections.len(), 4);
    }

    #[test]
    fn test_python_script_execution() {
        let t_const = find_template("source_const_text").unwrap();
        let t_py = find_template("script_python").unwrap();
        let t_sink = find_template("sink_lh").unwrap();

        let mut n_const = instantiate_template(t_const, [0.0, 0.0]);
        n_const
            .props
            .insert("text".to_string(), "hello world".to_string());

        let mut n_py = instantiate_template(t_py, [150.0, 0.0]);
        n_py.props.insert(
            "script".to_string(),
            "import sys\nsys.stdout.write(sys.stdin.read().upper())".to_string(),
        );

        let n_sink = instantiate_template(t_sink, [300.0, 0.0]);

        let graph = NodeGraph {
            nodes: vec![n_const.clone(), n_py.clone(), n_sink.clone()],
            connections: vec![
                Connection {
                    from_node: n_const.id,
                    from_pin: "Text".to_string(),
                    to_node: n_py.id,
                    to_pin: "Input".to_string(),
                },
                Connection {
                    from_node: n_py.id,
                    from_pin: "Output".to_string(),
                    to_node: n_sink.id,
                    to_pin: "In".to_string(),
                },
            ],
            ..Default::default()
        };

        let mgr = SlotManager::default();
        let executor = PipelineExecutor::new(&graph, &mgr);
        let res = executor.execute().expect("Pipeline execution failed");

        assert!(res.left_hand_item.is_some());
        let item = res.left_hand_item.unwrap();
        assert_eq!(item.title, "HELLO WORLD");
    }

    #[test]
    fn test_clean_and_slugify_execution() {
        let mut mgr = SlotManager::default();
        mgr.push_left(ClipItem::from_text(
            "  Rust Edition 2024 -- Little Screen Clip!  ",
        ));

        let recipe = make_clean_and_slugify_recipe();
        let executor = PipelineExecutor::new(&recipe.graph, &mgr);
        let res = executor.execute().expect("Slugify pipeline failed");

        assert!(res.left_hand_item.is_some());
        let item = res.left_hand_item.unwrap();
        assert_eq!(item.title, "rust-edition-2024-little-screen-clip");
    }

    #[test]
    fn test_prompt_or_hand_text_with_override_leaves_slot_empty() {
        use crate::pipeline::catalog::find_template;

        let t_prompt = find_template("source_prompt_or_hand").unwrap();
        let t_sink = find_template("sink_lh").unwrap();

        let node_src = instantiate_template(t_prompt, [0.0, 0.0]);
        let node_sink = instantiate_template(t_sink, [200.0, 0.0]);

        let graph = NodeGraph {
            nodes: vec![node_src.clone(), node_sink.clone()],
            connections: vec![Connection {
                from_node: node_src.id,
                from_pin: "Text".to_string(),
                to_node: node_sink.id,
                to_pin: "In".to_string(),
            }],
            pan: [0.0, 0.0],
            zoom: 1.0,
        };

        let mgr = SlotManager::default();
        // Right hand is EMPTY
        assert!(mgr.right_hand().is_none());

        let executor = PipelineExecutor::new(&graph, &mgr)
            .with_single_prompt(Some("Prompted Caption Text".to_string()));

        let res = executor.execute().expect("Pipeline execution failed");

        // The pipeline produced an item with the prompted text
        assert!(res.left_hand_item.is_some());
        assert_eq!(res.left_hand_item.unwrap().title, "Prompted Caption Text");

        // CRUCIAL: The slot manager's right hand remains completely empty!
        assert!(mgr.right_hand().is_none());
    }

    #[test]
    fn test_script_file_execution() {
        use crate::pipeline::recipes::{create_script_file_scaffold, get_recipes_dir};
        use std::fs;

        let py_fname = create_script_file_scaffold("Test Exec Recipe", true).unwrap();
        let sh_fname = create_script_file_scaffold("Test Exec Recipe", false).unwrap();

        let recipes_dir = get_recipes_dir();
        let py_path = recipes_dir.join(&py_fname);
        let sh_path = recipes_dir.join(&sh_fname);

        // Customize python script to reverse the string
        let py_custom = r#"# Recipe: Test Exec Recipe
import sys
data = sys.stdin.read().strip()
sys.stdout.write(data[::-1])
"#;
        fs::write(&py_path, py_custom).unwrap();

        // Customize bash script to append "!BASH!"
        let sh_custom = r#"#!/usr/bin/env bash
# Recipe: Test Exec Recipe
data=$(cat)
printf "%s!BASH!" "$data"
"#;
        fs::write(&sh_path, sh_custom).unwrap();

        let t_const = find_template("source_const_text").unwrap();
        let t_py = find_template("script_python").unwrap();
        let t_sh = find_template("script_bash").unwrap();
        let t_sink = find_template("sink_lh").unwrap();

        let mut n_const = instantiate_template(t_const, [0.0, 0.0]);
        n_const
            .props
            .insert("text".to_string(), "hello".to_string());

        let mut n_py = instantiate_template(t_py, [150.0, 0.0]);
        n_py.props
            .insert("script_file".to_string(), py_fname.clone());

        let mut n_sh = instantiate_template(t_sh, [300.0, 0.0]);
        n_sh.props
            .insert("script_file".to_string(), sh_fname.clone());

        let n_sink = instantiate_template(t_sink, [450.0, 0.0]);

        // n_const -> n_py -> n_sh -> n_sink
        let graph = NodeGraph {
            nodes: vec![n_const.clone(), n_py.clone(), n_sh.clone(), n_sink.clone()],
            connections: vec![
                Connection {
                    from_node: n_const.id,
                    from_pin: "Text".to_string(),
                    to_node: n_py.id,
                    to_pin: "Input".to_string(),
                },
                Connection {
                    from_node: n_py.id,
                    from_pin: "Output".to_string(),
                    to_node: n_sh.id,
                    to_pin: "Input".to_string(),
                },
                Connection {
                    from_node: n_sh.id,
                    from_pin: "Output".to_string(),
                    to_node: n_sink.id,
                    to_pin: "In".to_string(),
                },
            ],
            ..Default::default()
        };

        let mgr = SlotManager::default();
        let executor = PipelineExecutor::new(&graph, &mgr);
        let res = executor
            .execute()
            .expect("Script file execution pipeline failed");

        assert!(res.left_hand_item.is_some());
        let item = res.left_hand_item.unwrap();
        // "hello" reversed is "olleh", then appended "!BASH!" => "olleh!BASH!"
        assert_eq!(item.title, "olleh!BASH!");

        // Clean up
        let _ = fs::remove_file(py_path);
        let _ = fs::remove_file(sh_path);
    }

    #[test]
    fn test_image_diff_node_execution() {
        use image::{Rgba, RgbaImage};
        use std::fs;

        let temp_dir = std::env::temp_dir().join(format!("test_img_diff_{}", Uuid::new_v4()));
        let _ = fs::create_dir_all(&temp_dir);

        let img1_path = temp_dir.join("img1.png");
        let img2_path = temp_dir.join("img2.png");

        let mut img1 = RgbaImage::new(16, 16);
        for p in img1.pixels_mut() {
            *p = Rgba([200u8, 200, 200, 255]);
        }
        img1.save(&img1_path).unwrap();

        let mut img2 = RgbaImage::new(16, 16);
        for (x, _y, p) in img2.enumerate_pixels_mut() {
            if x < 8 {
                *p = Rgba([10u8, 10, 10, 255]);
            } else {
                *p = Rgba([200u8, 200, 200, 255]);
            }
        }
        img2.save(&img2_path).unwrap();

        let t_left = find_template("source_lh").unwrap();
        let t_right = find_template("source_rh").unwrap();
        let t_diff = find_template("img_diff").unwrap();
        let t_sink = find_template("sink_lh").unwrap();

        let n_src1 = instantiate_template(t_left, [0.0, 0.0]);
        let n_src2 = instantiate_template(t_right, [0.0, 100.0]);

        let mut n_diff = instantiate_template(t_diff, [200.0, 50.0]);
        n_diff
            .props
            .insert("threshold".to_string(), "0.10".to_string());

        let n_sink = instantiate_template(t_sink, [400.0, 50.0]);

        let graph = NodeGraph {
            nodes: vec![
                n_src1.clone(),
                n_src2.clone(),
                n_diff.clone(),
                n_sink.clone(),
            ],
            connections: vec![
                Connection {
                    from_node: n_src1.id,
                    from_pin: "Out".to_string(),
                    to_node: n_diff.id,
                    to_pin: "Base Image".to_string(),
                },
                Connection {
                    from_node: n_src2.id,
                    from_pin: "Out".to_string(),
                    to_node: n_diff.id,
                    to_pin: "Compare Image".to_string(),
                },
                Connection {
                    from_node: n_diff.id,
                    from_pin: "Mask".to_string(),
                    to_node: n_sink.id,
                    to_pin: "In".to_string(),
                },
            ],
            ..Default::default()
        };

        let mut mgr = SlotManager::default();
        mgr.push_left(
            ClipItem::from_image_bytes(fs::read(&img1_path).unwrap(), "image/png").unwrap(),
        );
        mgr.push_right(
            ClipItem::from_image_bytes(fs::read(&img2_path).unwrap(), "image/png").unwrap(),
        );
        let executor = PipelineExecutor::new(&graph, &mgr);
        let res = executor
            .execute()
            .expect("ImageDiff pipeline execution failed");

        assert!(res.left_hand_item.is_some());
        let item = res.left_hand_item.unwrap();
        match item.payload {
            ClipPayload::Image {
                width,
                height,
                mime,
                ..
            } => {
                assert_eq!(width, 16);
                assert_eq!(height, 16);
                assert_eq!(mime, "image/png");
            }
            _ => panic!("Expected image payload from ImageDiff node output"),
        }

        let _ = fs::remove_file(img1_path);
        let _ = fs::remove_file(img2_path);
        let _ = fs::remove_dir_all(temp_dir);
    }
}
