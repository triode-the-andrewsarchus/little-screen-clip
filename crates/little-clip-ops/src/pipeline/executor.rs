//! Graph topological evaluation and DAG execution engine.
//!
//! Performs cycle detection via Kahn's topological sort algorithm, dispatches nodes in
//! topological order, handles inter-process communication for CLI tools (FFmpeg, ImageMagick, jq, Python),
//! manages prompt inputs, and collects output payloads for slot state commitment or file saving.

use super::types::{NodeGraph, NodeInstance, NodeKind, PortValue};
use base64::Engine;
use little_clip_core::item::{ClipItem, ClipPayload};
use little_clip_core::slot::SlotManager;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;
use uuid::Uuid;

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
}

/// Consolidated outputs and side-effects produced by a pipeline graph execution.
#[derive(Debug, Clone, Default)]
pub struct PipelineExecutionResult {
    /// Resulting item to assign to Left Hand if a `SetLeftHand` or `PasteToApp` node fired.
    pub left_hand_item: Option<ClipItem>,
    /// Resulting item to assign to Right Hand if a `SetRightHand` node fired.
    pub right_hand_item: Option<ClipItem>,
    /// Paths to files saved to disk during execution.
    pub saved_files: Vec<PathBuf>,
    /// True if the output item should be simulated pasted into the active application.
    pub should_paste: bool,
    /// Per-node intermediate output summary previews.
    pub node_previews: HashMap<Uuid, String>,
}

/// Evaluator that topologically executes a `NodeGraph` against current slot contents.
pub struct PipelineExecutor<'a> {
    graph: &'a NodeGraph,
    slot_mgr: &'a SlotManager,
    temp_dir: PathBuf,
    /// Node-specific prompt overrides mapped by node UUID.
    pub prompt_overrides: HashMap<Uuid, String>,
    /// Single prompt override applied to the first interactive prompt node.
    pub single_prompt_override: Option<String>,
}

impl<'a> PipelineExecutor<'a> {
    /// Constructs a new `PipelineExecutor` with an isolated temporary working directory.
    pub fn new(graph: &'a NodeGraph, slot_mgr: &'a SlotManager) -> Self {
        let run_id = Uuid::new_v4();
        let temp_dir = std::env::temp_dir()
            .join("little_clip_pipeline_runs")
            .join(run_id.to_string());
        let _ = std::fs::create_dir_all(&temp_dir);

        Self {
            graph,
            slot_mgr,
            temp_dir,
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
    pub fn execute(&self) -> Result<PipelineExecutionResult, PipelineError> {
        let ordered_nodes = self.topological_sort()?;
        let mut outputs: HashMap<(Uuid, String), PortValue> = HashMap::new();
        let mut result = PipelineExecutionResult::default();

        for node in ordered_nodes {
            let mut inputs = HashMap::new();
            for pin in &node.inputs {
                if let Some(conn) = self
                    .graph
                    .connections
                    .iter()
                    .find(|c| c.to_node == node.id && c.to_pin.eq_ignore_ascii_case(&pin.name))
                {
                    if let Some(val) = outputs.get(&(conn.from_node, conn.from_pin.clone())) {
                        inputs.insert(pin.name.clone(), val.clone());
                    }
                }
            }

            let (node_outputs, preview) = self.execute_node(node, &inputs, &mut result)?;
            if let Some(p) = preview {
                result.node_previews.insert(node.id, p);
            }
            for (pin_name, val) in node_outputs {
                outputs.insert((node.id, pin_name), val);
            }
        }

        Ok(result)
    }

    fn topological_sort(&self) -> Result<Vec<&'a NodeInstance>, PipelineError> {
        let mut in_degree: HashMap<Uuid, usize> = HashMap::new();
        let mut adj: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

        for node in &self.graph.nodes {
            in_degree.insert(node.id, 0);
            adj.insert(node.id, Vec::new());
        }

        for conn in &self.graph.connections {
            if in_degree.contains_key(&conn.to_node) && adj.contains_key(&conn.from_node) {
                *in_degree.entry(conn.to_node).or_insert(0) += 1;
                adj.entry(conn.from_node).or_default().push(conn.to_node);
            }
        }

        let mut queue: VecDeque<Uuid> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut sorted = Vec::new();
        while let Some(node_id) = queue.pop_front() {
            if let Some(node) = self.graph.nodes.iter().find(|n| n.id == node_id) {
                sorted.push(node);
            }
            if let Some(neighbors) = adj.get(&node_id) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(&neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        if sorted.len() != self.graph.nodes.len() {
            return Err(PipelineError::CycleDetected);
        }

        Ok(sorted)
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
    ) -> Result<(Vec<(String, PortValue)>, Option<String>), PipelineError> {
        let mut out = Vec::new();
        #[allow(unused_assignments)]
        let mut preview = None;

        match &node.kind {
            NodeKind::LeftHandSource => {
                if let Some(item) = self.slot_mgr.left_hand() {
                    let val = PortValue::from_clip_item(item);
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
                    let val = PortValue::from_clip_item(item);
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
                    let val = PortValue::from_clip_item(item);
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
                let out_path = self.temp_dir.join(format!("framed_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-bordercolor",
                    border_color,
                    "-border",
                    border_size,
                    out_path.to_str().unwrap(),
                ])
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
                let out_path = self.temp_dir.join(format!("titled_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
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
                    out_path.to_str().unwrap(),
                ])
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
                let out_path = self
                    .temp_dir
                    .join(format!("cropped_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-crop",
                    geom,
                    "+repage",
                    out_path.to_str().unwrap(),
                ])
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
                let out_path = self
                    .temp_dir
                    .join(format!("resized_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-resize",
                    &format!("{}x{}>", max_w, max_h),
                    out_path.to_str().unwrap(),
                ])
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
                let out_path = self.temp_dir.join(format!("{}.webp", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-quality",
                    q,
                    out_path.to_str().unwrap(),
                ])
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
                let out_path = self
                    .temp_dir
                    .join(format!("rotated_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-rotate",
                    deg,
                    out_path.to_str().unwrap(),
                ])
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Rotated {}°", deg));
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageStitch => {
                let left_path = self.get_input_file(inputs, &["Left Image", "In 1", "In"])?;
                let right_path = self.get_input_file(inputs, &["Right Image", "In 2"])?;
                let out_path = self
                    .temp_dir
                    .join(format!("stitched_{}.png", Uuid::new_v4()));

                run_magick(&[
                    left_path.to_str().unwrap(),
                    right_path.to_str().unwrap(),
                    "+append",
                    out_path.to_str().unwrap(),
                ])
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some("Stitched 2 Images".to_string());
                out.push(("Combined".to_string(), PortValue::ImagePath(out_path)));
            }
            NodeKind::ImageStripExif => {
                let in_path = self.get_input_file(inputs, &["Image In", "In"])?;
                let out_path = self
                    .temp_dir
                    .join(format!("stripped_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-strip",
                    out_path.to_str().unwrap(),
                ])
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
                let b1 = std::fs::read(&left_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let b2 = std::fs::read(&right_path)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let opts = crate::image_diff::ImageDiffOptions {
                    threshold,
                    ..Default::default()
                };
                let res = crate::image_diff::compare_image_bytes(&b1, &b2, &opts)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let mask_png = res
                    .encode_mask_png()
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let out_mask = self
                    .temp_dir
                    .join(format!("diff_mask_{}.png", Uuid::new_v4()));
                std::fs::write(&out_mask, mask_png)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;

                let overlay_png = res
                    .encode_overlay_png()
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e.to_string()))?;
                let out_overlay = self
                    .temp_dir
                    .join(format!("diff_overlay_{}.png", Uuid::new_v4()));
                std::fs::write(&out_overlay, overlay_png)
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
                let out_path = self.temp_dir.join(format!("norm_{}.wav", Uuid::new_v4()));

                let filter = format!("loudnorm=I={}:LRA=11:TP={}", target, tp);
                run_command(
                    "ffmpeg",
                    &[
                        "-y",
                        "-i",
                        in_path.to_str().unwrap(),
                        "-af",
                        &filter,
                        out_path.to_str().unwrap(),
                    ],
                )
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
                let out_path = self
                    .temp_dir
                    .join(format!("trimmed_{}.wav", Uuid::new_v4()));

                let filter = format!(
                    "silenceremove=start_periods=1:start_duration=0.1:start_threshold={}dB:stop_periods=1:stop_duration=0.1:stop_threshold={}dB",
                    db, db
                );
                run_command(
                    "ffmpeg",
                    &[
                        "-y",
                        "-i",
                        in_path.to_str().unwrap(),
                        "-af",
                        &filter,
                        out_path.to_str().unwrap(),
                    ],
                )
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
                let out_path = self.temp_dir.join(format!("gain_{}.wav", Uuid::new_v4()));

                let filter = format!("volume={}dB", db);
                run_command(
                    "ffmpeg",
                    &[
                        "-y",
                        "-i",
                        in_path.to_str().unwrap(),
                        "-af",
                        &filter,
                        out_path.to_str().unwrap(),
                    ],
                )
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(format!("Gain {}dB", db));
                out.push(("Audio Out".to_string(), PortValue::AudioPath(out_path)));
            }
            NodeKind::AudioConcat => {
                let in1 = self.get_input_file(inputs, &["Clip 1", "In 1", "In"])?;
                let in2 = self.get_input_file(inputs, &["Clip 2", "In 2"])?;
                let out_path = self.temp_dir.join(format!("concat_{}.wav", Uuid::new_v4()));

                let filter = "[0:0][1:0]concat=n=2:v=0:a=1[out]";
                run_command(
                    "ffmpeg",
                    &[
                        "-y",
                        "-i",
                        in1.to_str().unwrap(),
                        "-i",
                        in2.to_str().unwrap(),
                        "-filter_complex",
                        filter,
                        "-map",
                        "[out]",
                        out_path.to_str().unwrap(),
                    ],
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
                        let out = run_command("sha256sum", &[p.to_str().unwrap()])
                            .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                        out.split_whitespace().next().unwrap_or("").to_string()
                    }
                    PortValue::Text(t) => {
                        let out = run_command_with_stdin("sha256sum", &[], t)
                            .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                        out.split_whitespace().next().unwrap_or("").to_string()
                    }
                    PortValue::FileBatch(paths) => {
                        let mut hashes = Vec::new();
                        for p in paths {
                            let out = run_command("sha256sum", &[p.to_str().unwrap()])
                                .unwrap_or_default();
                            let h = out.split_whitespace().next().unwrap_or("").to_string();
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
                let out_path = self.temp_dir.join(archive_name);

                let mut args = vec!["-r", out_path.to_str().unwrap()];
                for p in &paths {
                    args.push(p.to_str().unwrap());
                }
                run_command("zip", &args)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

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
                    run_command_with_stdin("python3", &[fpath.to_str().unwrap()], &stdin_data)
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
                    run_command_with_stdin("bash", &[fpath.to_str().unwrap()], &stdin_data)
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
                let _ = std::fs::create_dir_all(&dest_expanded);

                let saved = match in_val {
                    PortValue::FilePath(p) | PortValue::ImagePath(p) | PortValue::AudioPath(p) => {
                        let filename = p.file_name().unwrap_or_default();
                        let target = dest_expanded.join(filename);
                        std::fs::copy(p, &target)?;
                        target
                    }
                    PortValue::Text(t) => {
                        let target = dest_expanded.join(format!("clip_{}.txt", Uuid::new_v4()));
                        std::fs::write(&target, t)?;
                        target
                    }
                    PortValue::FileBatch(paths) => {
                        for p in paths {
                            let filename = p.file_name().unwrap_or_default();
                            let target = dest_expanded.join(filename);
                            let _ = std::fs::copy(p, &target);
                        }
                        dest_expanded
                    }
                };

                preview = Some(format!("Saved: {}", saved.display()));
                result.saved_files.push(saved);
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

fn run_command(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to spawn {}: {}", cmd, e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} error (exit {:?}): {}",
            cmd,
            output.status.code(),
            err
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_command_with_stdin(cmd: &str, args: &[&str], stdin_data: &str) -> Result<String, String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn {}: {}", cmd, e))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for {}: {}", cmd, e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{} error: {}", cmd, err));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_python_snippet(code: &str, stdin_data: &str) -> Result<String, String> {
    run_command_with_stdin("python3", &["-c", code], stdin_data)
}

fn run_magick(args: &[&str]) -> Result<String, String> {
    // Try 'magick' first (ImageMagick 7), fallback to 'convert' (ImageMagick 6)
    if let Ok(out) = run_command("magick", args) {
        return Ok(out);
    }
    run_command("convert", args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::catalog::{find_template, instantiate_template};
    use crate::pipeline::recipes::{make_clean_and_slugify_recipe, make_pad_and_caption_recipe};
    use crate::pipeline::types::Connection;

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

        let mgr = SlotManager::default();
        let executor = PipelineExecutor::new(&graph, &mgr);
        let sorted = executor
            .topological_sort()
            .expect("Should sort DAG cleanly");
        assert_eq!(sorted[0].id, n1.id);
        assert_eq!(sorted[1].id, n2.id);

        // Cyclic graph: n1 -> n2 -> n1
        let cyclic_graph = NodeGraph {
            nodes: vec![n1.clone(), n2.clone()],
            connections: vec![
                Connection {
                    from_node: n1.id,
                    from_pin: "Text".to_string(),
                    to_node: n2.id,
                    to_pin: "In".to_string(),
                },
                Connection {
                    from_node: n2.id,
                    from_pin: "Out".to_string(),
                    to_node: n1.id,
                    to_pin: "Text".to_string(),
                },
            ],
            ..Default::default()
        };

        let cyclic_executor = PipelineExecutor::new(&cyclic_graph, &mgr);
        assert!(matches!(
            cyclic_executor.topological_sort(),
            Err(PipelineError::CycleDetected)
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

        let t_const = find_template("source_const_text").unwrap();
        let t_diff = find_template("img_diff").unwrap();
        let t_sink = find_template("sink_lh").unwrap();

        let mut n_src1 = instantiate_template(t_const, [0.0, 0.0]);
        n_src1
            .props
            .insert("text".to_string(), img1_path.to_str().unwrap().to_string());

        let mut n_src2 = instantiate_template(t_const, [0.0, 100.0]);
        n_src2
            .props
            .insert("text".to_string(), img2_path.to_str().unwrap().to_string());

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
                    from_pin: "Text".to_string(),
                    to_node: n_diff.id,
                    to_pin: "Base Image".to_string(),
                },
                Connection {
                    from_node: n_src2.id,
                    from_pin: "Text".to_string(),
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

        let mgr = SlotManager::default();
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
