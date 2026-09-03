use super::types::{NodeGraph, NodeInstance, NodeKind, PortValue};
use base64::Engine;
use little_clip_core::item::ClipItem;
use little_clip_core::slot::SlotManager;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;
use uuid::Uuid;


#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("Graph contains a cycle")]
    CycleDetected,
    #[error("Missing required input: {0}")]
    MissingInput(String),
    #[error("Node {0} execution failed: {1}")]
    NodeFailed(String, String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default)]
pub struct PipelineExecutionResult {
    pub left_hand_item: Option<ClipItem>,
    pub right_hand_item: Option<ClipItem>,
    pub saved_files: Vec<PathBuf>,
    pub should_paste: bool,
    pub node_previews: HashMap<Uuid, String>,
}

pub struct PipelineExecutor<'a> {
    graph: &'a NodeGraph,
    slot_mgr: &'a SlotManager,
    temp_dir: PathBuf,
}

impl<'a> PipelineExecutor<'a> {
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
        }
    }

    pub fn execute(&self) -> Result<PipelineExecutionResult, PipelineError> {
        let ordered_nodes = self.topological_sort()?;
        let mut outputs: HashMap<(Uuid, String), PortValue> = HashMap::new();
        let mut result = PipelineExecutionResult::default();

        for node in ordered_nodes {
            let mut inputs = HashMap::new();
            for pin in &node.inputs {
                if let Some(conn) = self.graph.connections.iter().find(|c| {
                    c.to_node == node.id && c.to_pin.eq_ignore_ascii_case(&pin.name)
                }) {
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
                    return Err(PipelineError::MissingInput("Left Hand is empty".to_string()));
                }
            }
            NodeKind::RightHandSource => {
                if let Some(item) = self.slot_mgr.right_hand() {
                    let val = PortValue::from_clip_item(item);
                    preview = Some(item.title.clone());
                    out.push(("Out".to_string(), val));
                } else {
                    return Err(PipelineError::MissingInput("Right Hand is empty".to_string()));
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
                let text = node
                    .props
                    .get("text")
                    .cloned()
                    .unwrap_or_default();
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
                let res = run_command_with_stdin("python3", &["-c", script, pattern, replacement], &text)
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
                let filter = node
                    .props
                    .get("filter")
                    .map(|s| s.as_str())
                    .unwrap_or(".");
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
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), format!("Base64 decode error: {}", e)))?;
                    String::from_utf8(bytes)
                        .map_err(|e| PipelineError::NodeFailed(node.title.clone(), format!("Invalid UTF-8: {}", e)))?
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
                let out_path = self.temp_dir.join(format!("cropped_{}.png", Uuid::new_v4()));

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
                let out_path = self.temp_dir.join(format!("resized_{}.png", Uuid::new_v4()));

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
                let q = node.props.get("quality").map(|s| s.as_str()).unwrap_or("90");
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
                let deg = node.props.get("degrees").map(|s| s.as_str()).unwrap_or("90");
                let out_path = self.temp_dir.join(format!("rotated_{}.png", Uuid::new_v4()));

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
                let out_path = self.temp_dir.join(format!("stitched_{}.png", Uuid::new_v4()));

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
                let out_path = self.temp_dir.join(format!("stripped_{}.png", Uuid::new_v4()));

                run_magick(&[
                    in_path.to_str().unwrap(),
                    "-strip",
                    out_path.to_str().unwrap(),
                ])
                .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some("EXIF stripped".to_string());
                out.push(("Image Out".to_string(), PortValue::ImagePath(out_path)));
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
                let out_path = self.temp_dir.join(format!("trimmed_{}.wav", Uuid::new_v4()));

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
                let db = node.props.get("gain_db").map(|s| s.as_str()).unwrap_or("3.0");
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
                        out.split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string()
                    }
                    PortValue::Text(t) => {
                        let out = run_command_with_stdin("sha256sum", &[], t)
                            .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;
                        out.split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string()
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
                    Some(PortValue::FilePath(p)) | Some(PortValue::ImagePath(p)) | Some(PortValue::AudioPath(p)) => vec![p.clone()],
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
                let in_val = inputs.values().next().cloned();
                let stdin_data = match in_val {
                    Some(PortValue::Text(t)) => t,
                    Some(PortValue::FilePath(p)) | Some(PortValue::ImagePath(p)) | Some(PortValue::AudioPath(p)) => {
                        p.display().to_string()
                    }
                    Some(PortValue::FileBatch(paths)) => paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    None => String::new(),
                };

                let script = node
                    .props
                    .get("script")
                    .map(|s| s.as_str())
                    .unwrap_or("import sys\nsys.stdout.write(sys.stdin.read())");

                let stdout = run_python_snippet(script, &stdin_data)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

                preview = Some(stdout.clone());
                out.push(("Output".to_string(), PortValue::Text(stdout)));
            }
            NodeKind::BashCommand => {
                let in_val = inputs.values().next().cloned();
                let stdin_data = match in_val {
                    Some(PortValue::Text(t)) => t,
                    Some(PortValue::FilePath(p)) | Some(PortValue::ImagePath(p)) | Some(PortValue::AudioPath(p)) => {
                        p.display().to_string()
                    }
                    Some(PortValue::FileBatch(paths)) => paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    None => String::new(),
                };

                let cmd = node
                    .props
                    .get("command")
                    .map(|s| s.as_str())
                    .unwrap_or("cat");

                let stdout = run_command_with_stdin("bash", &["-c", cmd], &stdin_data)
                    .map_err(|e| PipelineError::NodeFailed(node.title.clone(), e))?;

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
            }
        }
        if let Some(v) = inputs.values().next() {
            if let Some(p) = v.as_path() {
                return Ok(p.to_path_buf());
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

fn run_command(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to spawn {}: {}", cmd, e))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{} error (exit {:?}): {}", cmd, output.status.code(), err));
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
        let sorted = executor.topological_sort().expect("Should sort DAG cleanly");
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
        assert!(matches!(cyclic_executor.topological_sort(), Err(PipelineError::CycleDetected)));
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
        n_const.props.insert("text".to_string(), "hello world".to_string());

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
        mgr.push_left(ClipItem::from_text("  Rust Edition 2024 -- Little Screen Clip!  "));


        let recipe = make_clean_and_slugify_recipe();
        let executor = PipelineExecutor::new(&recipe.graph, &mgr);
        let res = executor.execute().expect("Slugify pipeline failed");

        assert!(res.left_hand_item.is_some());
        let item = res.left_hand_item.unwrap();
        assert_eq!(item.title, "rust-edition-2024-little-screen-clip");
    }
}

