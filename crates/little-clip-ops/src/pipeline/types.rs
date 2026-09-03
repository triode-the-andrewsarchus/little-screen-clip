use little_clip_core::item::{ClipItem, ClipPayload};
use little_clip_core::slot::SlotId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PortType {
    Any,
    Text,
    Image,
    Audio,
    FilePath,
    FileBatch,
}

impl PortType {
    pub fn is_compatible_with(&self, other: &PortType) -> bool {
        match (self, other) {
            (PortType::Any, _) | (_, PortType::Any) => true,
            (a, b) => a == b,
        }
    }

    pub fn color_rgba(&self) -> [u8; 4] {
        match self {
            PortType::Any => [203, 213, 225, 255],     // Slate
            PortType::Text => [251, 191, 36, 255],    // Amber
            PortType::Image => [56, 189, 248, 255],   // Sky blue
            PortType::Audio => [168, 85, 247, 255],   // Purple
            PortType::FilePath => [52, 211, 153, 255],// Emerald
            PortType::FileBatch => [244, 114, 182, 255], // Pink
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PortValue {
    Text(String),
    ImagePath(PathBuf),
    AudioPath(PathBuf),
    FilePath(PathBuf),
    FileBatch(Vec<PathBuf>),
}

impl PortValue {
    pub fn port_type(&self) -> PortType {
        match self {
            PortValue::Text(_) => PortType::Text,
            PortValue::ImagePath(_) => PortType::Image,
            PortValue::AudioPath(_) => PortType::Audio,
            PortValue::FilePath(_) => PortType::FilePath,
            PortValue::FileBatch(_) => PortType::FileBatch,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            PortValue::Text(s) => Some(s.as_str()),
            PortValue::FilePath(p) | PortValue::ImagePath(p) | PortValue::AudioPath(p) => {
                p.to_str()
            }
            PortValue::FileBatch(_) => None,
        }
    }

    pub fn as_path(&self) -> Option<&Path> {
        match self {
            PortValue::ImagePath(p) | PortValue::AudioPath(p) | PortValue::FilePath(p) => {
                Some(p.as_path())
            }
            _ => None,
        }
    }

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
            ClipPayload::Image { bytes, mime, .. } => {
                // For raw image in memory, we save to staging or temp path
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
                let cache_dir = std::env::temp_dir().join("little_clip_pipeline_staging");
                let _ = std::fs::create_dir_all(&cache_dir);
                let path = cache_dir.join(format!("{}.{}", Uuid::new_v4(), ext));
                let _ = std::fs::write(&path, bytes);
                PortValue::ImagePath(path)
            }
        }
    }

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
            PortValue::AudioPath(path) => {
                ClipItem::from_audio(path.clone()).unwrap_or_else(|| ClipItem::from_single_file(&path))
            }
            PortValue::FilePath(path) => {
                if let Ok(bytes) = std::fs::read(&path) {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if ext == "png" || ext == "jpg" || ext == "jpeg" || ext == "webp" || ext == "svg" {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecipeInputType {
    #[default]
    Any,
    Text,
    #[serde(rename = "text | prompt", alias = "text_or_prompt", alias = "text_prompt")]
    TextOrPrompt,
    Image,
    Audio,
    MultiText,
    MultiImage,
    None,
}

impl RecipeInputType {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pin {
    pub name: String,
    pub port_type: PortType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeInstance {
    pub id: Uuid,
    pub kind: NodeKind,
    pub title: String,
    pub pos: [f32; 2],
    #[serde(default)]
    pub props: HashMap<String, String>,
    #[serde(default)]
    pub inputs: Vec<Pin>,
    #[serde(default)]
    pub outputs: Vec<Pin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Connection {
    pub from_node: Uuid,
    pub from_pin: String,
    pub to_node: Uuid,
    pub to_pin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeGraph {
    #[serde(default)]
    pub nodes: Vec<NodeInstance>,
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default = "default_pan")]
    pub pan: [f32; 2],
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_type: RecipeInputType,
    #[serde(default)]
    pub tags: Vec<String>,
    pub graph: NodeGraph,
}

impl Recipe {
    pub fn is_applicable(&self, items: &[&ClipItem]) -> bool {
        match self.input_type {
            RecipeInputType::Any => !items.is_empty(),
            RecipeInputType::None => true,
            RecipeInputType::Text => {
                items.len() == 1 && is_text_like(items[0])
            }
            RecipeInputType::TextOrPrompt => {
                // Applicable if single text item, or if slot is empty / non-text (which triggers popup prompt)
                items.len() <= 1
            }
            RecipeInputType::Image => {
                items.len() == 1 && is_image_like(items[0])
            }
            RecipeInputType::Audio => {
                items.len() == 1 && is_audio_like(items[0])
            }
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


