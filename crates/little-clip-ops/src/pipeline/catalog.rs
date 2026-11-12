//! Node template catalog for the pipeline graph builder.
//!
//! Provides predefined operator templates (`NODE_CATALOG`) spanning sources, text filters,
//! ImageMagick transforms, FFmpeg audio filters, file compressors, scripting escape hatches (Python/Bash),
//! and clipboard/file sinks, with keyword/tag search and node instantiation helpers.

use super::types::{NodeInstance, NodeKind, Pin, PortType};
use little_clip_core::slot::SlotId;
use std::collections::HashMap;
use uuid::Uuid;

/// Template definition for a node available in the node palette.
#[derive(Debug, Clone)]
pub struct NodeTemplate {
    /// Unique catalog identifier.
    pub id: &'static str,
    /// Human-readable display name.
    pub name: &'static str,
    /// Functional category (e.g. "Source", "Text", "Image", "Audio", "Files", "Script", "Sink").
    pub category: &'static str,
    /// Descriptive summary of node purpose and tools invoked.
    pub description: &'static str,
    /// Search tags and keywords.
    pub tags: &'static [&'static str],
    /// Underlying node kind enum variant.
    pub kind: NodeKind,
    /// Static input pin definitions `(name, port_type)`.
    pub inputs: &'static [(&'static str, PortType)],
    /// Static output pin definitions `(name, port_type)`.
    pub outputs: &'static [(&'static str, PortType)],
    /// Default property key-value pairs.
    pub default_props: &'static [(&'static str, &'static str)],
}

impl NodeTemplate {
    /// Returns the corresponding clipboard [`OpDescriptor`](crate::OpDescriptor) if this node template represents
    /// a built-in clipboard operation.
    pub fn matching_op_descriptor(&self) -> Option<&'static crate::OpDescriptor> {
        let target_op = match self.id {
            "text_trim" => Some("trim"),
            "text_slugify" => Some("slugify"),
            "text_case" => Some("snake-case"),
            "text_base64" => Some("base64-encode"),
            "text_join" => Some("join-commas"),
            "img_diff" => Some("diff"),
            _ => None,
        };
        target_op.and_then(crate::find_op_by_name)
    }

    /// Returns the hierarchical menu path (e.g. `"Audio/Filtering"` or `"Audio/Spectral/Combine"`)
    /// used to group this node in cascading menus.
    pub fn menu_path(&self) -> &'static str {
        if self.id.starts_with("cdp_") {
            return super::cdp_catalog::cdp_menu_path(self.id);
        }
        match self.id {
            "source_lh" | "source_rh" | "source_lp1" | "source_lp2" | "source_rp1"
            | "source_undo" => "Source/Clipboard",
            "source_const_text" | "source_const_num" => "Source/Constants",
            "source_prompt_or_hand" => "Source/Interactive",
            "text_trim" | "text_slugify" | "text_case" => "Text/Clean & Case",
            "text_regex_replace" | "text_split_lines" | "text_join" | "text_json_query"
            | "text_base64" => "Text/Transform",
            "img_pad_frame" | "img_add_title" | "img_crop" | "img_stitch" => "Image/Compose",
            "img_resize" | "img_rotate" | "img_convert_webp" | "img_strip_exif" | "img_diff" => {
                "Image/Transform"
            }
            "audio_normalize" | "audio_trim_silence" | "audio_adjust_gain" | "audio_concat" => {
                "Audio/Core"
            }
            "file_sha256" | "file_zip" | "file_extract_name" => "Files",
            "script_python" | "script_bash" => "Script",
            "sink_lh"
            | "sink_rh"
            | "sink_save_disk"
            | "sink_paste_app"
            | "sink_external_program" => "Sink",
            "sink_audio_studio"
            | "sink_image_studio"
            | "sink_text_studio"
            | "sink_spreadsheet_studio"
            | "sink_midi_studio"
            | "sink_pdf_studio"
            | "sink_model3d_studio" => "Sink/Studios",
            _ => self.category,
        }
    }

    /// Returns the first input pin name compatible with `source_out`, if any.
    pub fn first_compatible_input(&self, source_out: PortType) -> Option<&'static str> {
        self.inputs
            .iter()
            .find(|(_, pt)| source_out.is_compatible_with(pt))
            .map(|(name, _)| *name)
    }

    /// Returns the first output pin name compatible with `target_in`, if any.
    pub fn first_compatible_output(&self, target_in: PortType) -> Option<&'static str> {
        self.outputs
            .iter()
            .find(|(_, pt)| pt.is_compatible_with(&target_in))
            .map(|(name, _)| *name)
    }
}

/// Static catalog of all available pipeline node templates.
pub const NODE_CATALOG: &[NodeTemplate] = &[
    // -------------------------------------------------------------
    // Sources
    // -------------------------------------------------------------
    NodeTemplate {
        id: "source_lh",
        name: "Left Hand Source",
        category: "Source",
        description: "Emits active ClipItem currently held in Left Hand [LH]",
        tags: &["source", "input", "hand", "primary", "left"],
        kind: NodeKind::LeftHandSource,
        inputs: &[],
        outputs: &[("Out", PortType::Any)],
        default_props: &[],
    },
    NodeTemplate {
        id: "source_rh",
        name: "Right Hand Source",
        category: "Source",
        description: "Emits active ClipItem currently held in Right Hand [RH]",
        tags: &["source", "input", "hand", "secondary", "right"],
        kind: NodeKind::RightHandSource,
        inputs: &[],
        outputs: &[("Out", PortType::Any)],
        default_props: &[],
    },
    NodeTemplate {
        id: "source_lp1",
        name: "Left Prior 1 (LP1)",
        category: "Source",
        description: "Emits clip item from Left Prior 1 history slot",
        tags: &["source", "history", "left", "prior"],
        kind: NodeKind::SlotSource(SlotId::LeftPrior1),
        inputs: &[],
        outputs: &[("Out", PortType::Any)],
        default_props: &[],
    },
    NodeTemplate {
        id: "source_lp2",
        name: "Left Prior 2 (LP2)",
        category: "Source",
        description: "Emits clip item from Left Prior 2 history slot",
        tags: &["source", "history", "left", "prior"],
        kind: NodeKind::SlotSource(SlotId::LeftPrior2),
        inputs: &[],
        outputs: &[("Out", PortType::Any)],
        default_props: &[],
    },
    NodeTemplate {
        id: "source_rp1",
        name: "Right Prior 1 (RP1)",
        category: "Source",
        description: "Emits clip item from Right Prior 1 history slot",
        tags: &["source", "history", "right", "prior"],
        kind: NodeKind::SlotSource(SlotId::RightPrior1),
        inputs: &[],
        outputs: &[("Out", PortType::Any)],
        default_props: &[],
    },
    NodeTemplate {
        id: "source_undo",
        name: "Undo Slot (Z)",
        category: "Source",
        description: "Emits last displaced clip item from Undo slot",
        tags: &["source", "undo", "last"],
        kind: NodeKind::SlotSource(SlotId::Undo),
        inputs: &[],
        outputs: &[("Out", PortType::Any)],
        default_props: &[],
    },
    NodeTemplate {
        id: "source_const_text",
        name: "Constant Text",
        category: "Source",
        description: "Emits a user-defined literal string or delimiter",
        tags: &["source", "string", "literal", "constant"],
        kind: NodeKind::ConstantText,
        inputs: &[],
        outputs: &[("Text", PortType::Text)],
        default_props: &[("text", "Hello World")],
    },
    NodeTemplate {
        id: "source_const_num",
        name: "Constant Number",
        category: "Source",
        description: "Emits a numerical constant for parameters",
        tags: &["source", "number", "size", "constant"],
        kind: NodeKind::ConstantNumber,
        inputs: &[],
        outputs: &[("Value", PortType::Text)],
        default_props: &[("value", "24")],
    },
    NodeTemplate {
        id: "source_prompt_or_hand",
        name: "Prompt or Hand Text",
        category: "Source",
        description: "Emits text from Hand if present; otherwise prompts via popup input when executed",
        tags: &["source", "hand", "prompt", "text", "input", "interactive"],
        kind: NodeKind::PromptOrHandText,
        inputs: &[],
        outputs: &[("Text", PortType::Text)],
        default_props: &[
            ("hand", "right"),
            ("prompt_message", "Enter text:"),
            ("default_text", ""),
        ],
    },
    // -------------------------------------------------------------
    // Text Operators (CLI / External Python / jq)
    // -------------------------------------------------------------
    NodeTemplate {
        id: "text_trim",
        name: "Trim Whitespace",
        category: "Text",
        description: "Trim leading and trailing whitespace from string",
        tags: &["text", "trim", "clean", "whitespace"],
        kind: NodeKind::TextTrim,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[],
    },
    NodeTemplate {
        id: "text_slugify",
        name: "Slugify Text",
        category: "Text",
        description: "Convert string into URL-safe lowercase kebab-case slug",
        tags: &["text", "slugify", "url", "kebab", "python"],
        kind: NodeKind::TextSlugify,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[],
    },
    NodeTemplate {
        id: "text_case",
        name: "Change Case",
        category: "Text",
        description: "Convert casing: UPPER, lower, snake_case, or camelCase",
        tags: &["text", "case", "upper", "lower", "snake", "camel"],
        kind: NodeKind::TextCase,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[("target_case", "snake")],
    },
    NodeTemplate {
        id: "text_regex_replace",
        name: "Regex Replace",
        category: "Text",
        description: "Search and replace text using Python regular expressions",
        tags: &["text", "regex", "replace", "pattern", "python"],
        kind: NodeKind::TextRegexReplace,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[("pattern", r"\s+"), ("replacement", " ")],
    },
    NodeTemplate {
        id: "text_split_lines",
        name: "Split Lines",
        category: "Text",
        description: "Split multiline text by custom delimiter",
        tags: &["text", "split", "delimiter", "lines"],
        kind: NodeKind::TextSplitLines,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[("delimiter", r"\n")],
    },
    NodeTemplate {
        id: "text_join",
        name: "Join Lines",
        category: "Text",
        description: "Join lines with custom separator string",
        tags: &["text", "join", "separator", "merge"],
        kind: NodeKind::TextJoin,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[("separator", ", ")],
    },
    NodeTemplate {
        id: "text_json_query",
        name: "JSON Query (jq)",
        category: "Text",
        description: "Query or format JSON structure via jq CLI",
        tags: &["text", "json", "jq", "query", "filter"],
        kind: NodeKind::TextJsonQuery,
        inputs: &[("JSON In", PortType::Text)],
        outputs: &[("JSON Out", PortType::Text)],
        default_props: &[("filter", ".")],
    },
    NodeTemplate {
        id: "text_base64",
        name: "Base64 Encode/Decode",
        category: "Text",
        description: "Encode text to Base64 or decode Base64 to text",
        tags: &["text", "base64", "encode", "decode"],
        kind: NodeKind::TextBase64,
        inputs: &[("In", PortType::Text)],
        outputs: &[("Out", PortType::Text)],
        default_props: &[("mode", "encode")],
    },
    // -------------------------------------------------------------
    // Image Operators (ImageMagick CLI: magick / convert)
    // -------------------------------------------------------------
    NodeTemplate {
        id: "img_pad_frame",
        name: "Pad with Frame",
        category: "Image",
        description: "Add a solid or accent border around the image (ImageMagick)",
        tags: &["image", "frame", "border", "pad", "magick"],
        kind: NodeKind::ImagePadFrame,
        inputs: &[("Image In", PortType::Image), ("Mod", PortType::Automation)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[("border_color", "#1e293b"), ("border_size", "24")],
    },
    NodeTemplate {
        id: "img_add_title",
        name: "Add Title / Caption",
        category: "Image",
        description: "Add a bottom caption bar with title text (ImageMagick)",
        tags: &["image", "caption", "title", "text", "magick"],
        kind: NodeKind::ImageAddTitle,
        inputs: &[("Image In", PortType::Image), ("Caption", PortType::Text)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[
            ("bar_color", "#0f172a"),
            ("text_color", "#f8fafc"),
            ("bar_height", "48"),
            ("font_size", "24"),
        ],
    },
    NodeTemplate {
        id: "img_crop",
        name: "Crop Image",
        category: "Image",
        description: "Crop image by geometry (e.g. 500x500+0+0) (ImageMagick)",
        tags: &["image", "crop", "dimensions", "cut", "magick"],
        kind: NodeKind::ImageCrop,
        inputs: &[("Image In", PortType::Image)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[("geometry", "1000x1000+0+0")],
    },
    NodeTemplate {
        id: "img_resize",
        name: "Resize Image",
        category: "Image",
        description: "Resize image bounding box (e.g. 1920x1080) (ImageMagick)",
        tags: &["image", "resize", "scale", "dimensions", "magick"],
        kind: NodeKind::ImageResize,
        inputs: &[("Image In", PortType::Image)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[("max_width", "1920"), ("max_height", "1080")],
    },
    NodeTemplate {
        id: "img_convert_webp",
        name: "Convert to WebP",
        category: "Image",
        description: "Convert image to modern high-efficiency WebP format",
        tags: &["image", "webp", "convert", "compress", "magick"],
        kind: NodeKind::ImageConvertWebp,
        inputs: &[("Image In", PortType::Image), ("Mod", PortType::Automation)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[("quality", "90")],
    },
    NodeTemplate {
        id: "img_rotate",
        name: "Rotate / Flip",
        category: "Image",
        description: "Rotate image degrees (90, 180, 270) or flip (ImageMagick)",
        tags: &["image", "rotate", "flip", "orientation", "magick"],
        kind: NodeKind::ImageRotate,
        inputs: &[("Image In", PortType::Image), ("Mod", PortType::Automation)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[("degrees", "90")],
    },
    NodeTemplate {
        id: "img_stitch",
        name: "Stitch Side-by-Side",
        category: "Image",
        description: "Stitch two images horizontally (+append) (ImageMagick)",
        tags: &["image", "stitch", "append", "dual", "magick"],
        kind: NodeKind::ImageStitch,
        inputs: &[
            ("Left Image", PortType::Image),
            ("Right Image", PortType::Image),
        ],
        outputs: &[("Combined", PortType::Image)],
        default_props: &[],
    },
    NodeTemplate {
        id: "img_strip_exif",
        name: "Strip EXIF Data",
        category: "Image",
        description: "Strip metadata and GPS EXIF privacy markers (ImageMagick)",
        tags: &["image", "exif", "privacy", "strip", "clean", "magick"],
        kind: NodeKind::ImageStripExif,
        inputs: &[("Image In", PortType::Image)],
        outputs: &[("Image Out", PortType::Image)],
        default_props: &[],
    },
    NodeTemplate {
        id: "img_diff",
        name: "Picture Diff (ODiff)",
        category: "Image",
        description: "Perceptual pixel comparison highlighting differences (ODiff-style)",
        tags: &["image", "diff", "compare", "odiff", "mask", "visual"],
        kind: NodeKind::ImageDiff,
        inputs: &[
            ("Base Image", PortType::Image),
            ("Compare Image", PortType::Image),
        ],
        outputs: &[("Mask", PortType::Image), ("Overlay", PortType::Image)],
        default_props: &[("threshold", "0.10")],
    },
    // -------------------------------------------------------------
    // Audio Operators (FFmpeg CLI)
    // -------------------------------------------------------------
    NodeTemplate {
        id: "audio_normalize",
        name: "Normalize (-14 LUFS)",
        category: "Audio",
        description: "Standard broadcast loudness normalization (-14 LUFS via FFmpeg)",
        tags: &["audio", "normalize", "lufs", "broadcast", "ffmpeg"],
        kind: NodeKind::AudioNormalize,
        inputs: &[("Audio In", PortType::Audio), ("Mod", PortType::Automation)],
        outputs: &[("Audio Out", PortType::Audio)],
        default_props: &[("target_lufs", "-14"), ("tp", "-1.5")],
    },
    NodeTemplate {
        id: "audio_trim_silence",
        name: "Trim Silence",
        category: "Audio",
        description: "Remove leading and trailing silence (FFmpeg silenceremove)",
        tags: &["audio", "silence", "trim", "cut", "ffmpeg"],
        kind: NodeKind::AudioTrimSilence,
        inputs: &[("Audio In", PortType::Audio), ("Mod", PortType::Automation)],
        outputs: &[("Audio Out", PortType::Audio)],
        default_props: &[("threshold_db", "-50")],
    },
    NodeTemplate {
        id: "audio_adjust_gain",
        name: "Adjust Gain (dB)",
        category: "Audio",
        description: "Boost or cut audio volume by decibels (FFmpeg volume)",
        tags: &["audio", "gain", "volume", "db", "ffmpeg"],
        kind: NodeKind::AudioAdjustGain,
        inputs: &[("Audio In", PortType::Audio), ("Mod", PortType::Automation)],
        outputs: &[("Audio Out", PortType::Audio)],
        default_props: &[("gain_db", "3.0")],
    },
    NodeTemplate {
        id: "audio_concat",
        name: "Concatenate Audio",
        category: "Audio",
        description: "Join two audio clips sequentially (FFmpeg concat)",
        tags: &["audio", "concat", "join", "merge", "ffmpeg"],
        kind: NodeKind::AudioConcat,
        inputs: &[("Clip 1", PortType::Audio), ("Clip 2", PortType::Audio)],
        outputs: &[("Audio Out", PortType::Audio)],
        default_props: &[],
    },
    // -------------------------------------------------------------
    // File Operators
    // -------------------------------------------------------------
    NodeTemplate {
        id: "file_sha256",
        name: "Compute SHA-256",
        category: "Files",
        description: "Calculate cryptographic SHA-256 checksum of input file/text",
        tags: &["file", "sha256", "hash", "checksum"],
        kind: NodeKind::FileSha256,
        inputs: &[("In", PortType::Any)],
        outputs: &[("Checksum", PortType::Text)],
        default_props: &[],
    },
    NodeTemplate {
        id: "file_zip",
        name: "Compress to ZIP",
        category: "Files",
        description: "Package input files into a compressed .zip archive",
        tags: &["file", "zip", "compress", "archive"],
        kind: NodeKind::FileZip,
        inputs: &[("Files In", PortType::FileBatch)],
        outputs: &[("Zip Archive", PortType::FilePath)],
        default_props: &[("archive_name", "archive.zip")],
    },
    NodeTemplate {
        id: "file_extract_name",
        name: "Extract Filename",
        category: "Files",
        description: "Extract filename and extension from full path",
        tags: &["file", "path", "filename", "basename"],
        kind: NodeKind::FileExtractName,
        inputs: &[("File In", PortType::FilePath)],
        outputs: &[("Filename", PortType::Text)],
        default_props: &[],
    },
    // -------------------------------------------------------------
    // Scripting Escape Hatch (Python / Bash)
    // -------------------------------------------------------------
    NodeTemplate {
        id: "script_python",
        name: "Python Script Node",
        category: "Script",
        description: "Execute custom Python script. Input received in stdin, output captured from stdout.",
        tags: &["script", "python", "custom", "code", "escape hatch"],
        kind: NodeKind::PythonScript,
        inputs: &[("Input", PortType::Any)],
        outputs: &[("Output", PortType::Any)],
        default_props: &[(
            "script",
            "import sys\ndata = sys.stdin.read()\nsys.stdout.write(data.upper())",
        )],
    },
    NodeTemplate {
        id: "script_bash",
        name: "Bash Command Node",
        category: "Script",
        description: "Pipe input through arbitrary shell command (e.g. jq, sed, awk)",
        tags: &["script", "bash", "shell", "pipe", "command"],
        kind: NodeKind::BashCommand,
        inputs: &[("Input", PortType::Any)],
        outputs: &[("Output", PortType::Any)],
        default_props: &[("command", "tr '[:lower:]' '[:upper:]'")],
    },
    // -------------------------------------------------------------
    // Parameter Automation
    // -------------------------------------------------------------
    NodeTemplate {
        id: "auto_lfo",
        name: "LFO Modulator",
        category: "Automation",
        description: "Periodic sine/triangle/square/saw LFO curve for modulating node parameters",
        tags: &["automation", "lfo", "mod", "sine", "oscillator"],
        kind: NodeKind::AutomationLfo,
        inputs: &[],
        outputs: &[("Curve", PortType::Automation)],
        default_props: &[
            ("shape", "sine"),
            ("rate_hz", "2.0"),
            ("depth", "1.0"),
            ("offset", "0.5"),
        ],
    },
    NodeTemplate {
        id: "auto_envelope",
        name: "ADSR Envelope",
        category: "Automation",
        description: "Attack-Decay-Sustain-Release automation envelope",
        tags: &["automation", "envelope", "adsr", "mod"],
        kind: NodeKind::AutomationEnvelope,
        inputs: &[],
        outputs: &[("Curve", PortType::Automation)],
        default_props: &[
            ("attack", "0.15"),
            ("decay", "0.25"),
            ("sustain", "0.65"),
            ("release", "0.25"),
        ],
    },
    NodeTemplate {
        id: "auto_draw",
        name: "Freehand Curve",
        category: "Automation",
        description: "Drawn breakpoint automation curve for parameter modulation",
        tags: &["automation", "draw", "curve", "breakfile", "mod"],
        kind: NodeKind::AutomationDraw,
        inputs: &[],
        outputs: &[("Curve", PortType::Automation)],
        default_props: &[("points", "[[0.0,0.2],[0.35,0.85],[0.7,0.4],[1.0,0.9]]")],
    },
    NodeTemplate {
        id: "auto_steps",
        name: "Step Sequencer",
        category: "Automation",
        description: "8-step quantized automation sequencer for rhythmic modulation",
        tags: &["automation", "steps", "sequencer", "mod"],
        kind: NodeKind::AutomationSteps,
        inputs: &[],
        outputs: &[("Curve", PortType::Automation)],
        default_props: &[("steps", "[0.2,0.8,0.4,0.9,0.1,0.6,0.5,0.7]")],
    },
    // -------------------------------------------------------------
    // Sinks / Actions
    // -------------------------------------------------------------
    NodeTemplate {
        id: "sink_lh",
        name: "Set in Left Hand [LH]",
        category: "Sink",
        description: "Commits output to Left Hand and moves previous [LH] to Undo [Z]",
        tags: &["sink", "hand", "primary", "output", "commit", "left"],
        kind: NodeKind::SetLeftHand,
        inputs: &[("In", PortType::Any)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_rh",
        name: "Set in Right Hand [RH]",
        category: "Sink",
        description: "Commits output to Right Hand [RH]",
        tags: &["sink", "hand", "secondary", "output", "commit", "right"],
        kind: NodeKind::SetRightHand,
        inputs: &[("In", PortType::Any)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_save_disk",
        name: "Save to Disk",
        category: "Sink",
        description: "Saves result to designated folder on disk",
        tags: &["sink", "save", "disk", "export", "file"],
        kind: NodeKind::SaveToDisk,
        inputs: &[("In", PortType::Any)],
        outputs: &[],
        default_props: &[("destination", "~/Downloads")],
    },
    NodeTemplate {
        id: "sink_paste_app",
        name: "Paste to Active App",
        category: "Sink",
        description: "Commits to Left Hand and simulates Ctrl+V paste to active window",
        tags: &["sink", "paste", "clipboard", "action"],
        kind: NodeKind::PasteToApp,
        inputs: &[("In", PortType::Any)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_audio_studio",
        name: "Open in Audio Studio",
        category: "Sink",
        description: "Automatically opens the generated audio in the built-in Audio Studio on run",
        tags: &["sink", "studio", "audio", "wav", "preview", "open"],
        kind: NodeKind::OpenAudioStudio,
        inputs: &[("Audio", PortType::Audio)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_image_studio",
        name: "Open in Image Studio",
        category: "Sink",
        description: "Automatically opens the generated image in the built-in Image Studio on run",
        tags: &["sink", "studio", "image", "preview", "open"],
        kind: NodeKind::OpenImageStudio,
        inputs: &[("Image", PortType::Image)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_text_studio",
        name: "Open in Text Studio",
        category: "Sink",
        description: "Automatically opens the generated text in the built-in Text Studio on run",
        tags: &["sink", "studio", "text", "preview", "open"],
        kind: NodeKind::OpenTextStudio,
        inputs: &[("Text", PortType::Text)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_spreadsheet_studio",
        name: "Open in Spreadsheet Studio",
        category: "Sink",
        description: "Automatically opens CSV/table data in the built-in Spreadsheet Studio on run",
        tags: &["sink", "studio", "spreadsheet", "csv", "table", "open"],
        kind: NodeKind::OpenSpreadsheetStudio,
        inputs: &[("In", PortType::Any)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_midi_studio",
        name: "Open in MIDI Studio",
        category: "Sink",
        description: "Automatically opens a MIDI file in the built-in MIDI Studio on run",
        tags: &["sink", "studio", "midi", "preview", "open"],
        kind: NodeKind::OpenMidiStudio,
        inputs: &[("File", PortType::FilePath)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_pdf_studio",
        name: "Open in PDF Studio",
        category: "Sink",
        description: "Automatically opens a PDF file in the built-in PDF Studio on run",
        tags: &["sink", "studio", "pdf", "preview", "open"],
        kind: NodeKind::OpenPdfStudio,
        inputs: &[("File", PortType::FilePath)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_model3d_studio",
        name: "Open in 3D Studio",
        category: "Sink",
        description: "Automatically opens a 3D model or Gaussian splat in the built-in 3D Studio on run",
        tags: &["sink", "studio", "3d", "model", "splat", "mesh", "open"],
        kind: NodeKind::OpenModel3dStudio,
        inputs: &[("File", PortType::FilePath)],
        outputs: &[],
        default_props: &[],
    },
    NodeTemplate {
        id: "sink_external_program",
        name: "External Program",
        category: "Sink",
        description: "Opens the output file with the system default handler or a custom binary with pre/post args",
        tags: &[
            "sink", "external", "program", "open", "xdg-open", "cli", "app",
        ],
        kind: NodeKind::OpenExternalProgram,
        inputs: &[("In", PortType::Any)],
        outputs: &[],
        default_props: &[("program", ""), ("pre_args", ""), ("post_args", "")],
    },
];

/// Finds a node template in `NODE_CATALOG` or the CDP8 catalog by its identifier.
pub fn find_template(id: &str) -> Option<&'static NodeTemplate> {
    NODE_CATALOG
        .iter()
        .find(|t| t.id == id)
        .or_else(|| super::cdp_catalog::find_cdp_template(id))
}

/// Searches catalog templates by name, description, tags, or category filter.
pub fn search_templates(query: &str, category_filter: Option<&str>) -> Vec<&'static NodeTemplate> {
    let q = query.trim().to_lowercase();
    NODE_CATALOG
        .iter()
        .chain(super::cdp_catalog::all_cdp_templates().iter())
        .filter(|t| {
            if let Some(cat) = category_filter
                && !cat.eq_ignore_ascii_case("all")
                && !t.category.eq_ignore_ascii_case(cat)
            {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            t.name.to_lowercase().contains(&q)
                || t.description.to_lowercase().contains(&q)
                || t.tags.iter().any(|tag| tag.to_lowercase().contains(&q))
                || t.category.to_lowercase().contains(&q)
        })
        .collect()
}

/// Creates a new `NodeInstance` from a `NodeTemplate` at the specified canvas position.
pub fn instantiate_template(template: &NodeTemplate, pos: [f32; 2]) -> NodeInstance {
    let mut props = HashMap::new();
    for &(k, v) in template.default_props {
        props.insert(k.to_string(), v.to_string());
    }

    let inputs = template
        .inputs
        .iter()
        .map(|&(name, port_type)| Pin {
            name: name.to_string(),
            port_type,
        })
        .collect();

    let outputs = template
        .outputs
        .iter()
        .map(|&(name, port_type)| Pin {
            name: name.to_string(),
            port_type,
        })
        .collect();

    NodeInstance {
        id: Uuid::new_v4(),
        kind: template.kind.clone(),
        title: template.name.to_string(),
        pos,
        props,
        inputs,
        outputs,
    }
}
