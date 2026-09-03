use super::types::{NodeInstance, NodeKind, Pin, PortType};
use little_clip_core::slot::SlotId;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NodeTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub tags: &'static [&'static str],
    pub kind: NodeKind,
    pub inputs: &'static [(&'static str, PortType)],
    pub outputs: &'static [(&'static str, PortType)],
    pub default_props: &'static [(&'static str, &'static str)],
}

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
        inputs: &[("Image In", PortType::Image)],
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
        inputs: &[("Image In", PortType::Image)],
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
        inputs: &[("Image In", PortType::Image)],
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
        inputs: &[("Left Image", PortType::Image), ("Right Image", PortType::Image)],
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
        inputs: &[("Audio In", PortType::Audio)],
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
        inputs: &[("Audio In", PortType::Audio)],
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
        inputs: &[("Audio In", PortType::Audio)],
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
        default_props: &[
            (
                "script",
                "import sys\ndata = sys.stdin.read()\nsys.stdout.write(data.upper())",
            ),
        ],
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
];

pub fn find_template(id: &str) -> Option<&'static NodeTemplate> {
    NODE_CATALOG.iter().find(|t| t.id == id)
}

pub fn search_templates(query: &str, category_filter: Option<&str>) -> Vec<&'static NodeTemplate> {
    let q = query.trim().to_lowercase();
    NODE_CATALOG
        .iter()
        .filter(|t| {
            if let Some(cat) = category_filter {
                if !cat.eq_ignore_ascii_case("all") && !t.category.eq_ignore_ascii_case(cat) {
                    return false;
                }
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
