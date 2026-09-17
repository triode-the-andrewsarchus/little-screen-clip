//! Data models, enums, and port types for pipeline graphs.
//!
//! Defines graph topology types (`NodeGraph`, `NodeInstance`, `Connection`, `Pin`),
//! data flow representations (`PortType`, `PortValue`), and recipe structures (`Recipe`, `RecipeInputType`).

use little_clip_core::ArtifactStore;
use little_clip_core::item::{ClipItem, ClipPayload};
use little_clip_core::slot::SlotId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Data type classification for node graph input and output pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortType {
    /// Accepts any data type.
    Any,
    /// UTF-8 plain text or string data.
    Text,
    /// Single image file or memory buffer.
    Image,
    /// Single audio file or audio stream.
    Audio,
    /// Single filesystem path.
    FilePath,
    /// Collection of filesystem paths.
    FileBatch,
}

impl PortType {
    /// Returns `true` if this output port type can connect to `other` input port type.
    ///
    /// `Any` is deliberately compatible with every port type; all other connections require an
    /// exact type match.
    ///
    /// ```
    /// use little_clip_ops::PortType;
    ///
    /// assert!(PortType::Text.is_compatible_with(&PortType::Text));
    /// assert!(PortType::Any.is_compatible_with(&PortType::Image));
    /// assert!(!PortType::Text.is_compatible_with(&PortType::Audio));
    /// ```
    pub fn is_compatible_with(&self, other: &PortType) -> bool {
        match (self, other) {
            (PortType::Any, _) | (_, PortType::Any) => true,
            (a, b) => a == b,
        }
    }

    /// Returns the RGBA color used to render pin dots and wires in the workbench UI.
    pub fn color_rgba(&self) -> [u8; 4] {
        match self {
            PortType::Any => [203, 213, 225, 255],       // Slate
            PortType::Text => [251, 191, 36, 255],       // Amber
            PortType::Image => [56, 189, 248, 255],      // Sky blue
            PortType::Audio => [168, 85, 247, 255],      // Purple
            PortType::FilePath => [52, 211, 153, 255],   // Emerald
            PortType::FileBatch => [244, 114, 182, 255], // Pink
        }
    }
}

/// Dynamic runtime value passed between node ports during graph execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PortValue {
    /// Text or string payload.
    Text(String),
    /// Path to an image file.
    ImagePath(PathBuf),
    /// Path to an audio file.
    AudioPath(PathBuf),
    /// Path to a generic file.
    FilePath(PathBuf),
    /// Batch of file paths.
    FileBatch(Vec<PathBuf>),
}

impl PortValue {
    /// Returns the underlying `PortType` for this value.
    pub fn port_type(&self) -> PortType {
        match self {
            PortValue::Text(_) => PortType::Text,
            PortValue::ImagePath(_) => PortType::Image,
            PortValue::AudioPath(_) => PortType::Audio,
            PortValue::FilePath(_) => PortType::FilePath,
            PortValue::FileBatch(_) => PortType::FileBatch,
        }
    }

    /// Returns string slice if this is a text value or path string.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            PortValue::Text(s) => Some(s.as_str()),
            PortValue::FilePath(p) | PortValue::ImagePath(p) | PortValue::AudioPath(p) => {
                p.to_str()
            }
            PortValue::FileBatch(_) => None,
        }
    }

    /// Returns filesystem path reference if this value is a path.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            PortValue::ImagePath(p) | PortValue::AudioPath(p) | PortValue::FilePath(p) => {
                Some(p.as_path())
            }
            _ => None,
        }
    }

    /// Converts a `ClipItem` into its corresponding `PortValue`.
    pub fn from_clip_item(item: &ClipItem) -> Self {
        match &item.payload {
            ClipPayload::Text(t) => PortValue::Text(t.clone()),
            ClipPayload::Markdown(m) => PortValue::Text(m.clone()),
            ClipPayload::Url(u) => PortValue::Text(u.to_string()),
            ClipPayload::Color { hex, .. } => PortValue::Text(hex.clone()),
            ClipPayload::AudioRef { path, .. } => PortValue::AudioPath(path.clone()),
            ClipPayload::Animation { path, .. } => PortValue::FilePath(path.clone()),
            ClipPayload::FileBatch(paths) => {
                if paths.len() == 1 {
                    PortValue::FilePath(paths[0].clone())
                } else {
                    PortValue::FileBatch(paths.clone())
                }
            }
            ClipPayload::Directory(p) => PortValue::FilePath(p.clone()),
            ClipPayload::MultiImage { paths, .. } => {
                if paths.len() == 1 {
                    PortValue::ImagePath(paths[0].clone())
                } else {
                    PortValue::FileBatch(paths.clone())
                }
            }
            ClipPayload::Image {
                path: Some(path), ..
            } => PortValue::ImagePath(path.clone()),
            ClipPayload::Image {
                bytes,
                mime,
                path: None,
                ..
            } => {
                // Memory-only images need a materialized pipeline input file.
                let ext = if mime.contains("png") {
                    "png"
                } else if mime.contains("jpeg") || mime.contains("jpg") {
                    "jpg"
                } else if mime.contains("webp") {
                    "webp"
                } else if mime.contains("svg") {
                    "svg"
                } else {
                    "bin"
                };
                let path = ArtifactStore::new()
                    .and_then(|store| store.stage_bytes("pipeline-input", "image", ext, bytes))
                    .unwrap_or_else(|_| {
                        PathBuf::from(format!("pipeline-input-{}.{}", Uuid::new_v4(), ext))
                    });
                PortValue::ImagePath(path)
            }
        }
    }

    /// Materializes the port value into a new `ClipItem`.
    pub fn into_clip_item(self, title_hint: &str) -> ClipItem {
        match self {
            PortValue::Text(text) => ClipItem::from_text(text),
            PortValue::ImagePath(path) => {
                if let Ok(bytes) = std::fs::read(&path) {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("png")
                        .to_lowercase();
                    let mime = if ext == "jpg" || ext == "jpeg" {
                        "image/jpeg"
                    } else if ext == "webp" {
                        "image/webp"
                    } else if ext == "svg" {
                        "image/svg+xml"
                    } else {
                        "image/png"
                    };
                    if let Some(mut item) = ClipItem::from_image_bytes(bytes, mime) {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| title_hint.to_string());
                        item.title = file_name;
                        return item;
                    }
                }
                ClipItem::from_single_file(&path)
            }
            PortValue::AudioPath(path) => ClipItem::from_audio(path.clone())
                .unwrap_or_else(|| ClipItem::from_single_file(&path)),
            PortValue::FilePath(path) => {
                if let Ok(bytes) = std::fs::read(&path) {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if ext == "png"
                        || ext == "jpg"
                        || ext == "jpeg"
                        || ext == "webp"
                        || ext == "svg"
                    {
                        let mime = if ext == "jpg" || ext == "jpeg" {
                            "image/jpeg"
                        } else if ext == "webp" {
                            "image/webp"
                        } else if ext == "svg" {
                            "image/svg+xml"
                        } else {
                            "image/png"
                        };
                        if let Some(mut item) = ClipItem::from_image_bytes(bytes, mime) {
                            let file_name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| title_hint.to_string());
                            item.title = file_name;
                            return item;
                        }
                    }
                }
                ClipItem::from_single_file(&path)
            }
            PortValue::FileBatch(paths) => {
                if paths.is_empty() {
                    ClipItem::from_text(title_hint.to_string())
                } else if paths.len() == 1 {
                    PortValue::FilePath(paths[0].clone()).into_clip_item(title_hint)
                } else {
                    ClipItem::from_files(paths)
                }
            }
        }
    }
}

/// Declares acceptable input formats for recipe triggering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecipeInputType {
    #[default]
    Any,
    Text,
    #[serde(
        rename = "text | prompt",
        alias = "text_or_prompt",
        alias = "text_prompt"
    )]
    TextOrPrompt,
    Image,
    Audio,
    MultiText,
    MultiImage,
    None,
}

impl RecipeInputType {
    /// Human-friendly display label for the input requirement.
    pub fn label(&self) -> &'static str {
        match self {
            RecipeInputType::Any => "Any",
            RecipeInputType::Text => "Text",
            RecipeInputType::TextOrPrompt => "Text | Prompt",
            RecipeInputType::Image => "Image",
            RecipeInputType::Audio => "Audio",
            RecipeInputType::MultiText => "Multi-Text",
            RecipeInputType::MultiImage => "Multi-Image",
            RecipeInputType::None => "None",
        }
    }
}

/// Identifies the node operation and functional category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    // Sources
    LeftHandSource,
    RightHandSource,
    SlotSource(SlotId),
    ConstantText,
    ConstantNumber,
    PromptOrHandText,

    // Text transforms
    TextTrim,
    TextSlugify,
    TextCase,
    TextRegexReplace,
    TextSplitLines,
    TextJoin,
    TextJsonQuery,
    TextBase64,

    // Image transforms
    ImagePadFrame,
    ImageAddTitle,
    ImageCrop,
    ImageResize,
    ImageConvertWebp,
    ImageRotate,
    ImageStitch,
    ImageStripExif,
    ImageDiff,

    // Audio transforms
    AudioNormalize,
    AudioTrimSilence,
    AudioAdjustGain,
    AudioConcat,

    // File transforms
    FileSha256,
    FileZip,
    FileExtractName,

    // Scripting Escape Hatch
    PythonScript,
    BashCommand,

    // Sinks
    SetLeftHand,
    SetRightHand,
    SaveToDisk,
    PasteToApp,
}

/// An input or output connection pin on a node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pin {
    /// Pin identifier name (e.g. "image", "text", "output").
    pub name: String,
    /// Data type accepted or emitted by this pin.
    pub port_type: PortType,
}

/// An instantiated node within a node graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeInstance {
    /// Unique node identifier.
    pub id: Uuid,
    /// The node's functional category and operation.
    pub kind: NodeKind,
    /// Display title shown on the node header.
    pub title: String,
    /// Canvas position `[x, y]` for UI rendering.
    pub pos: [f32; 2],
    /// Key-value property map for node parameters (e.g. padding, regex, quality).
    #[serde(default)]
    pub props: HashMap<String, String>,
    /// Input pins.
    #[serde(default)]
    pub inputs: Vec<Pin>,
    /// Output pins.
    #[serde(default)]
    pub outputs: Vec<Pin>,
}

/// A directed wire connecting an output pin of one node to an input pin of another.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Connection {
    /// Source node ID.
    pub from_node: Uuid,
    /// Source output pin name.
    pub from_pin: String,
    /// Target node ID.
    pub to_node: Uuid,
    /// Target input pin name.
    pub to_pin: String,
}

/// Complete node graph containing nodes, connections, and viewport pan/zoom state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeGraph {
    /// List of nodes in the graph.
    #[serde(default)]
    pub nodes: Vec<NodeInstance>,
    /// List of connections (wires) between nodes.
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// Viewport pan offset `[x, y]`.
    #[serde(default = "default_pan")]
    pub pan: [f32; 2],
    /// Viewport zoom level.
    #[serde(default = "default_zoom")]
    pub zoom: f32,
}

fn default_pan() -> [f32; 2] {
    [0.0, 0.0]
}

fn default_zoom() -> f32 {
    1.0
}

impl Default for NodeGraph {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            connections: Vec::new(),
            pan: [0.0, 0.0],
            zoom: 1.0,
        }
    }
}

/// A named, reusable automation pipeline preset with input criteria and graph topology.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    /// Unique recipe name.
    pub name: String,
    /// Short summary of what the recipe accomplishes.
    pub description: String,
    /// Input type filter determining when this recipe is applicable.
    #[serde(default)]
    pub input_type: RecipeInputType,
    /// Descriptive tags for searching and categorizing recipes.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Underlying node graph definition.
    pub graph: NodeGraph,
}

impl Recipe {
    /// Returns `true` if this recipe can be executed on the currently selected clipboard items.
    pub fn is_applicable(&self, items: &[&ClipItem]) -> bool {
        match self.input_type {
            RecipeInputType::Any => !items.is_empty(),
            RecipeInputType::None => true,
            RecipeInputType::Text => items.len() == 1 && is_text_like(items[0]),
            RecipeInputType::TextOrPrompt => {
                // Applicable if single text item, or if slot is empty / non-text (which triggers popup prompt)
                items.len() <= 1
            }
            RecipeInputType::Image => items.len() == 1 && is_image_like(items[0]),
            RecipeInputType::Audio => items.len() == 1 && is_audio_like(items[0]),
            RecipeInputType::MultiText => {
                items.len() >= 2 && items.iter().all(|it| is_text_like(it))
            }
            RecipeInputType::MultiImage => {
                items.len() >= 2 && items.iter().all(|it| is_image_like(it))
            }
        }
    }
}

fn is_text_like(item: &ClipItem) -> bool {
    matches!(
        item.payload,
        ClipPayload::Text(_) | ClipPayload::Markdown(_) | ClipPayload::Url(_)
    )
}

fn is_image_like(item: &ClipItem) -> bool {
    matches!(
        item.payload,
        ClipPayload::Image { .. } | ClipPayload::MultiImage { .. }
    )
}

fn is_audio_like(item: &ClipItem) -> bool {
    matches!(item.payload, ClipPayload::AudioRef { .. })
}
