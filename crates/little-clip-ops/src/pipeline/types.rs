//! Data models, enums, and port types for pipeline graphs.
//!
//! Defines graph topology types (`NodeGraph`, `NodeInstance`, `Connection`, `Pin`),
//! data flow representations (`PortType`, `PortValue`), and recipe structures (`Recipe`, `RecipeInputType`).

use little_clip_core::ArtifactStore;
use little_clip_core::item::{ClipItem, ClipPayload};
use little_clip_core::slot::SlotId;
use little_clip_core::staging::StagingError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

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

    /// Returns text or a single path string, including a one-reference batch.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            PortValue::Text(s) => Some(s.as_str()),
            PortValue::FilePath(p) | PortValue::ImagePath(p) | PortValue::AudioPath(p) => {
                p.to_str()
            }
            PortValue::FileBatch(paths) if paths.len() == 1 => paths[0].to_str(),
            PortValue::FileBatch(_) => None,
        }
    }

    /// Returns a filesystem path for a path value or one-reference batch.
    pub fn as_path(&self) -> Option<&Path> {
        match self {
            PortValue::ImagePath(p) | PortValue::AudioPath(p) | PortValue::FilePath(p) => {
                Some(p.as_path())
            }
            PortValue::FileBatch(paths) if paths.len() == 1 => Some(&paths[0]),
            _ => None,
        }
    }

    /// Converts a `ClipItem` into its corresponding `PortValue`.
    ///
    /// # Errors
    ///
    /// Returns a staging error when a memory-only image cannot be materialized for a path-based
    /// pipeline operation. It never returns a path that was not successfully written.
    pub fn from_clip_item(item: &ClipItem) -> Result<Self, StagingError> {
        Ok(match &item.payload {
            ClipPayload::Text(t) => PortValue::Text(t.clone()),
            ClipPayload::Markdown(m) => PortValue::Text(m.clone()),
            ClipPayload::Url(u) => PortValue::Text(u.to_string()),
            ClipPayload::Color { hex, .. } => PortValue::Text(hex.clone()),
            ClipPayload::AudioRef { path, .. } => PortValue::AudioPath(path.clone()),
            ClipPayload::Animation { path, .. } => PortValue::FilePath(path.clone()),
            ClipPayload::FileBatch(paths) => PortValue::FileBatch(paths.clone()),
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
                let ext = little_clip_core::item::image_extension_for_mime(mime).unwrap_or("bin");
                let path =
                    ArtifactStore::new()?.stage_bytes("pipeline-input", "image", ext, bytes)?;
                PortValue::ImagePath(path)
            }
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_image_file_reference_remains_a_reference_until_a_content_node_consumes_it() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("reference.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let reference = ClipItem::from_files(vec![path.clone()]);
        let port = PortValue::from_clip_item(&reference).unwrap();
        assert_eq!(port, PortValue::FileBatch(vec![path.clone()]));
        assert_eq!(port.as_path(), Some(path.as_path()));
        assert_eq!(port.as_text(), path.to_str());
        assert_eq!(
            port.into_clip_item("reference").payload,
            ClipPayload::FileBatch(vec![path])
        );
    }

    #[test]
    fn oversized_explicit_image_output_falls_back_to_a_reference_before_payload_allocation() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("large.png");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(128 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            PortValue::ImagePath(path.clone())
                .into_clip_item("large")
                .payload,
            ClipPayload::FileBatch(vec![path])
        );
    }

    #[test]
    fn file_materialization_preserves_image_bytes_and_ordinary_references() {
        let fixture = tempfile::tempdir().unwrap();
        let image = fixture.path().join("image.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]))
            .save(&image)
            .unwrap();
        for value in [
            PortValue::ImagePath(image.clone()),
            PortValue::FilePath(image.clone()),
        ] {
            let item = value.into_clip_item("result");
            assert!(matches!(
                item.payload,
                ClipPayload::Image {
                    width: 2,
                    height: 2,
                    ..
                }
            ));
            if let ClipPayload::Image { bytes, path, .. } = item.payload {
                assert_eq!(bytes, std::fs::read(&image).unwrap());
                let path =
                    path.expect("materialized image bytes retain an owned disk representation");
                assert_ne!(
                    path, image,
                    "the result must survive cleanup of the input workspace"
                );
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                std::fs::remove_file(path).unwrap();
            }
        }
        let binary = fixture.path().join("ordinary.bin");
        std::fs::copy(&image, &binary).unwrap();
        let protected = fixture.path().join("protected.png");
        std::os::unix::fs::symlink("/dev/null", &protected).unwrap();
        for path in [binary, protected] {
            assert_eq!(
                PortValue::FilePath(path.clone())
                    .into_clip_item("result")
                    .payload,
                ClipPayload::FileBatch(vec![path])
            );
        }
    }
}
