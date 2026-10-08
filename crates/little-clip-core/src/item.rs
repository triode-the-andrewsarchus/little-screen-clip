//! Clipboard items, payloads, thumbnail generators, and content classifiers.
//!
//! Handles extraction, MIME detection, dimension inspection, and thumbnail generation
//! across text, markdown, URLs, raster/vector images, camera RAW, PDFs, audio clips,
//! animations/video, directories, and file batches.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// High-level discriminator for the kind of payload stored in a `ClipItem`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PayloadKind {
    /// Plain unformatted text.
    Text,
    /// Markdown-formatted text.
    Markdown,
    /// Web or network URL.
    Url,
    /// Raster or vector single image.
    Image,
    /// Multiple image paths / gallery.
    MultiImage,
    /// Audio file reference with waveform/metadata.
    Audio,
    /// Looping animation (GIF, animated WebP) or video clip preview.
    Animation,
    /// Batch of arbitrary filesystem files.
    FileBatch,
    /// Single filesystem directory.
    Directory,
    /// Hex/RGB color representation.
    Color,
    /// Unrecognized or generic binary format.
    Unknown,
}

/// Pre-rendered thumbnail bitmap with premultiplied RGBA pixels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageThumbnail {
    /// Width of the rendered thumbnail in pixels.
    pub width: u32,
    /// Height of the rendered thumbnail in pixels.
    pub height: u32,
    /// Premultiplied RGBA byte buffer (`[R, G, B, A, ...]`).
    pub premultiplied_rgba: Vec<u8>,
}

impl ImageThumbnail {
    /// Generates a downscaled thumbnail from raw image bytes, fitting within `(max_w, max_h)`.
    ///
    /// Supports enabled raster formats, including HEIF/HEIC and AVIF, and falls back to
    /// SVG conversion via `rsvg-convert` if the payload contains vector markup.
    pub fn generate(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Self> {
        Self::generate_with_mime(bytes, None, max_w, max_h)
    }

    /// Generates a thumbnail with a MIME hint for formats such as TGA.
    pub fn generate_with_mime(
        bytes: &[u8],
        mime: Option<&str>,
        max_w: u32,
        max_h: u32,
    ) -> Option<Self> {
        Self::generate_with_limits(bytes, mime, max_w, max_h, image::Limits::default())
    }

    /// Generates a thumbnail while enforcing the caller's raster decode limits.
    ///
    /// Uses the shared decoder hooks and optional MIME hint, preserving aspect
    /// ratio within `max_w` and `max_h`. Output pixels use premultiplied RGBA.
    /// If raster decoding fails, SVG input uses the existing SVG renderer;
    /// `limits` applies only to raster decoding. Returns `None` when neither
    /// decoder can produce a thumbnail.
    pub fn generate_with_limits(
        bytes: &[u8],
        mime: Option<&str>,
        max_w: u32,
        max_h: u32,
        limits: image::Limits,
    ) -> Option<Self> {
        if let Ok(dyn_img) = image_reader(bytes, mime).and_then(|mut reader| {
            reader.limits(limits);
            reader.decode()
        }) {
            let thumb = dyn_img.thumbnail(max_w, max_h);
            return Some(Self::from_rgba(thumb.to_rgba8()));
        }

        // If raster loading fails, try rendering as SVG
        if is_svg_bytes(bytes) {
            return Self::generate_from_svg(bytes, max_w, max_h);
        }

        None
    }

    /// Converts straight-alpha pixels into the shared premultiplied preview format.
    pub fn from_rgba(image: image::RgbaImage) -> Self {
        let (width, height) = image.dimensions();
        let mut pixels = image.into_raw();
        for pixel in pixels.chunks_exact_mut(4) {
            for channel in 0..3 {
                pixel[channel] = (u16::from(pixel[channel]) * u16::from(pixel[3]) / 255) as u8;
            }
        }
        Self {
            width,
            height,
            premultiplied_rgba: pixels,
        }
    }

    /// Renders vector SVG bytes to raster pixels using the `rsvg-convert` command-line tool.
    pub fn generate_from_svg(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Self> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("rsvg-convert")
            .arg("-w")
            .arg(max_w.to_string())
            .arg("-h")
            .arg(max_h.to_string())
            .arg("-a")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(bytes);
        }

        let output = child.wait_with_output().ok()?;
        if !output.status.success() || output.stdout.is_empty() {
            return None;
        }

        Self::generate(&output.stdout, max_w, max_h)
    }
}

/// Returns `true` if the byte slice starts with an SVG XML declaration or `<svg` tag.
pub fn is_svg_bytes(bytes: &[u8]) -> bool {
    let header = std::str::from_utf8(bytes.get(..1024).unwrap_or(bytes)).unwrap_or("");
    let trimmed = header.trim_start();
    trimmed.starts_with("<svg")
        || (trimmed.starts_with("<?xml") && header.contains("<svg"))
        || header.contains("<svg")
}

/// Parses width and height attributes or `viewBox` bounds from raw SVG bytes without full XML parsing.
pub fn read_svg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(bytes.get(..4096).unwrap_or(bytes)).ok()?;

    // 1. Try viewBox="min-x min-y width height"
    if let Some(pos) = text.find("viewBox=") {
        let after = &text[pos + 8..];
        let quote = after.chars().next()?;
        if quote == '"' || quote == '\'' {
            let end_quote = after[1..].find(quote)?;
            let parts: Vec<&str> = after[1..=end_quote]
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() == 4 {
                if let (Ok(w), Ok(h)) = (parts[2].parse::<f32>(), parts[3].parse::<f32>()) {
                    if w > 0.0 && h > 0.0 {
                        return Some((w.round() as u32, h.round() as u32));
                    }
                }
            }
        }
    }

    // 2. Try width="..." height="..."
    let extract_attr = |attr: &str| -> Option<u32> {
        let pos = text.find(attr)?;
        let after = &text[pos + attr.len()..];
        let quote = after.chars().next()?;
        if quote == '"' || quote == '\'' {
            let end = after[1..].find(quote)?;
            let val = &after[1..=end];
            let num: String = val
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let f: f32 = num.parse().ok()?;
            Some(f.round() as u32)
        } else {
            None
        }
    };

    if let (Some(w), Some(h)) = (extract_attr("width="), extract_attr("height=")) {
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }

    None
}

/// Returns a supported image file extension for a MIME type.
pub fn image_extension_for_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/svg+xml" => Some("svg"),
        "image/heif" => Some("heif"),
        "image/heic" => Some("heic"),
        "image/x-farbfeld" => Some("ff"),
        "image/jpg" => Some("jpg"),
        _ => image::ImageFormat::from_mime_type(mime)
            .filter(|format| format.reading_enabled() || *format == image::ImageFormat::Avif)
            .map(|format| format.extensions_str()[0]),
    }
}

/// Creates a raster reader with HEIF/HEIC/AVIF decoding and an optional format hint.
///
/// Byte signatures take precedence over the hint. TGA has no reliable signature,
/// so callers reading files should supply their MIME type.
///
/// # Errors
///
/// Returns an error if reading the format signature fails. Unsupported formats
/// and invalid image data are reported when decoding or inspecting dimensions.
pub fn image_reader<'a>(
    bytes: &'a [u8],
    mime: Option<&str>,
) -> image::ImageResult<image::ImageReader<std::io::Cursor<&'a [u8]>>> {
    static CODECS: std::sync::Once = std::sync::Once::new();
    CODECS.call_once(libheif_rs::integration::image::register_all_decoding_hooks);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    let mime = match mime {
        Some("image/jpg") => Some("image/jpeg"),
        Some("image/x-farbfeld") => {
            reader.set_format(image::ImageFormat::Farbfeld);
            None
        }
        other => other,
    };
    if let Some(format) = mime.and_then(image::ImageFormat::from_mime_type) {
        reader.set_format(format);
    }
    Ok(reader.with_guessed_format()?)
}

/// Fast header-only image dimension reader for raster and vector formats (~0.05ms, skips pixel decode).
pub fn read_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    read_image_dimensions_with_mime(bytes, None)
}

/// Reads raster or SVG dimensions, using a MIME hint for formats such as TGA.
pub fn read_image_dimensions_with_mime(bytes: &[u8], mime: Option<&str>) -> Option<(u32, u32)> {
    if let Ok(dims) = image_reader(bytes, mime).and_then(|reader| reader.into_dimensions()) {
        return Some(dims);
    }
    if is_svg_bytes(bytes) {
        return read_svg_dimensions(bytes);
    }
    None
}

fn image_mime_matches_bytes(mime: &str, bytes: &[u8]) -> bool {
    if mime == "image/svg+xml" {
        return is_svg_bytes(bytes);
    }

    if matches!(mime, "image/heif" | "image/heic") {
        return bytes.get(4..8) == Some(b"ftyp")
            && image_reader(bytes, None)
                .and_then(|reader| reader.into_dimensions())
                .is_ok();
    }
    if matches!(mime, "image/x-tga" | "image/x-targa") {
        return image_reader(bytes, Some(mime))
            .and_then(|reader| reader.into_dimensions())
            .is_ok();
    }
    let Ok(format) = image::guess_format(bytes) else {
        return false;
    };
    match mime {
        "image/jpg" => format == image::ImageFormat::Jpeg,
        "image/x-farbfeld" => format == image::ImageFormat::Farbfeld,
        _ => image::ImageFormat::from_mime_type(mime) == Some(format),
    }
}

/// Concrete typed payload data contained within a clipboard item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClipPayload {
    /// Raw text string.
    Text(String),
    /// Markdown formatted content.
    Markdown(String),
    /// Validated web URL.
    Url(url::Url),
    /// Raster image bytes with dimensions, MIME, and optional thumbnail.
    Image {
        #[serde(skip)]
        bytes: Vec<u8>,
        #[serde(default)]
        path: Option<PathBuf>,
        width: u32,
        height: u32,
        mime: String,
        #[serde(default)]
        thumbnail: Option<ImageThumbnail>,
    },
    /// Audio file reference with duration, sample rate, channels, and optional waveform analysis.
    AudioRef {
        path: PathBuf,
        duration_ms: u64,
        sample_rate: u32,
        channels: u16,
        format: String,
        #[serde(default)]
        waveform: Option<crate::audio::AudioWaveform>,
    },
    /// Looping animated image or video clip.
    Animation {
        path: PathBuf,
        format: String,
        width: u32,
        height: u32,
        duration_ms: u64,
        #[serde(default)]
        thumbnail: Option<crate::animation::AnimationThumbnail>,
    },
    /// Multiple files selected or copied together.
    FileBatch(Vec<PathBuf>),
    /// Multiple image files with thumbnail preview indices.
    MultiImage {
        paths: Vec<PathBuf>,
        total_count: usize,
        preview_indices: Vec<usize>,
        #[serde(default)]
        thumbnails: Vec<ImageThumbnail>,
    },
    /// Filesystem directory reference.
    Directory(PathBuf),
    /// Hex and RGB color definition.
    Color { hex: String, r: u8, g: u8, b: u8 },
}

/// Represents a single clipboard entry stored in a slot, including metadata and display labels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipItem {
    /// Unique identifier for the item.
    pub id: Uuid,
    /// Unix timestamp (seconds) when the item was captured.
    pub created_at: u64,
    /// Stored clipboard payload.
    pub payload: ClipPayload,
    /// Primary title for UI display.
    pub title: String,
    /// Subtitle or secondary metadata string (dimensions, size, duration).
    pub subtitle: String,
    /// Detailed badges / chips displayed on the card.
    pub details: Vec<String>,
    /// Active handler / processor tag (e.g. `"IMAGE (FILE)"`, `"TEXT"`, `"SVG"`).
    pub handler_tag: String,
}

impl ClipItem {
    /// Serializes the item to plain text suitable for system clipboard export.
    pub fn to_clipboard_text(&self) -> Option<String> {
        match &self.payload {
            ClipPayload::Text(t) => Some(t.clone()),
            ClipPayload::Markdown(m) => Some(m.clone()),
            ClipPayload::Url(u) => Some(u.to_string()),
            ClipPayload::Color { hex, .. } => Some(hex.clone()),
            ClipPayload::MultiImage { paths, .. } => Some(
                paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            ClipPayload::FileBatch(files) => Some(
                files
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            ClipPayload::AudioRef { path, .. } => Some(path.to_string_lossy().to_string()),
            _ => None,
        }
    }

    /// Returns the local filesystem path if this item refers to a file on disk.
    pub fn file_path(&self) -> Option<PathBuf> {
        match &self.payload {
            ClipPayload::Image { path: Some(p), .. } => Some(p.clone()),
            ClipPayload::AudioRef { path, .. } => Some(path.clone()),
            ClipPayload::Directory(p) => Some(p.clone()),
            ClipPayload::FileBatch(files) if files.len() == 1 => files.first().cloned(),
            ClipPayload::Text(t) | ClipPayload::Markdown(t) => {
                let p = Path::new(t.trim());
                if p.is_file() {
                    Some(p.to_path_buf())
                } else {
                    None
                }
            }
            ClipPayload::Url(u) => {
                if let Ok(p) = u.to_file_path() {
                    if p.is_file() { Some(p) } else { None }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Sets a custom handler tag badge for the item.
    pub fn with_handler(mut self, handler: impl Into<String>) -> Self {
        self.handler_tag = handler.into();
        self
    }

    /// Creates an image `ClipItem` directly from in-memory bytes and a MIME type string.
    pub fn from_image_bytes(bytes: Vec<u8>, mime: impl Into<String>) -> Option<Self> {
        let mime = mime.into().to_ascii_lowercase();
        if !image_mime_matches_bytes(&mime, &bytes) {
            return None;
        }
        // Fast header-only dimension extraction in ~0.05ms (does NOT decode pixels)
        let (width, height) =
            read_image_dimensions_with_mime(&bytes, Some(&mime)).unwrap_or((0, 0));
        let size_kb = bytes.len() as f64 / 1024.0;
        let title = if width > 0 && height > 0 {
            format!("Image ({}×{})", width, height)
        } else {
            "Image".to_string()
        };
        let subtitle = format!(
            "{:.1} KB • {}",
            size_kb,
            mime.replace("image/", "").to_uppercase()
        );

        let ext = image_extension_for_mime(&mime).unwrap_or("png");
        let staging_path = crate::staging::ArtifactStore::new()
            .and_then(|store| store.stage_bytes("clipboard", "img", ext, &bytes))
            .ok();

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes,
                path: staging_path,
                width,
                height,
                mime: mime.clone(),
                thumbnail: None,
            },
            title,
            subtitle,
            details: vec![format!("{}×{}", width, height), mime],
            handler_tag: "IMAGE (CLIPBOARD)".to_string(),
        })
    }

    /// Parses arbitrary clipboard text, classifying it into files, URLs, colors, SVGs, JSON, Markdown, or plain text.
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
        let trimmed = text.trim();

        // 1. Check if copied text represents one or more local file paths / URIs (e.g. from file manager)
        let candidate_paths = extract_local_file_paths(trimmed);
        if !candidate_paths.is_empty() {
            if let Some(item) = Self::from_candidate_paths(&candidate_paths) {
                return item;
            }
        }

        // 2. Check if color hex (#RRGGBB or #RGB)
        if let Some(color) = crate::color::parse_hex_color(trimmed) {
            return Self::from_color(color);
        }

        // 3. Check if raw SVG markup in clipboard
        if is_svg_bytes(trimmed.as_bytes()) && trimmed.contains("</svg>") {
            return Self::from_svg_clipboard_text(trimmed);
        }

        // 4. Check if JSON string
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            if let Some(item) = Self::from_json_text(&text, trimmed) {
                return item;
            }
        }

        // 5. Check if URL
        if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
            && !trimmed.contains('\n')
        {
            if let Ok(parsed_url) = url::Url::parse(trimmed) {
                return Self::from_url(parsed_url);
            }
        }

        // 6. Check if Markdown text
        if crate::syntax::detect_markdown_text(trimmed) {
            return Self::from_markdown_text(text);
        }

        // 7. Default Plain Text
        let lines_count = text.lines().count();
        let chars_count = text.chars().count();
        let title = Self::preview_title_for_text(&text, "Empty text");
        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Text(text),
            title,
            subtitle: format!("{} lines • {} chars", lines_count, chars_count),
            details: vec![format!("{}L", lines_count), format!("{}c", chars_count)],
            handler_tag: "TEXT".to_string(),
        }
    }

    fn from_candidate_paths(paths: &[PathBuf]) -> Option<Self> {
        if paths.len() == 1 {
            Some(Self::from_single_local_path(&paths[0]))
        } else if paths.iter().all(|p| is_image_path(p)) {
            Some(Self::from_multi_image(paths.to_vec()))
        } else {
            Some(Self::from_files(paths.to_vec()))
        }
    }

    fn from_single_local_path(path: &Path) -> Self {
        if path.is_dir() {
            return Self::from_directory(path.to_path_buf());
        }
        if crate::animation::is_video_path(path) {
            if let Some(item) = Self::from_video(path.to_path_buf()) {
                return item;
            }
        }
        if crate::animation::is_animated_image_path(path) {
            if let Some(item) = Self::from_animated_image(path.to_path_buf()) {
                return item;
            }
        }
        if is_image_path(path) {
            if let Some(item) = Self::from_image_file(path) {
                return item;
            }
        }
        if crate::raw_image::is_raw_image_path(path) {
            if let Some(item) = Self::from_raw_image_file(path) {
                return item;
            }
        }
        if crate::pdf::is_pdf_path(path) {
            if let Some(item) = Self::from_pdf_file(path) {
                return item;
            }
        }
        if crate::audio::is_audio_path(path) {
            if let Some(item) = Self::from_audio(path.to_path_buf()) {
                return item;
            }
        }
        if let Some(item) = Self::from_text_or_code_file(path) {
            return item;
        }

        Self::from_single_file(path)
    }

    fn from_image_file(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let mime = guess_image_mime(path);
        let (w, h) = read_image_dimensions_with_mime(&bytes, Some(&mime)).unwrap_or((0, 0));
        let size_kb = bytes.len() as f64 / 1024.0;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Image file".to_string());
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "IMG".to_string());

        let subtitle = if ext == "SVG" {
            if w > 0 && h > 0 {
                format!("Vector ({}×{}) • {:.1} KB • SVG", w, h, size_kb)
            } else {
                format!("Vector • {:.1} KB • SVG", size_kb)
            }
        } else if w > 0 && h > 0 {
            format!("{}×{} • {:.1} KB • {}", w, h, size_kb, ext)
        } else {
            format!("{:.1} KB • {}", size_kb, ext)
        };

        let thumbnail = ImageThumbnail::generate_with_mime(&bytes, Some(&mime), 320, 220);
        let handler_tag = if ext == "SVG" {
            "SVG (VECTOR)".to_string()
        } else {
            "IMAGE (FILE)".to_string()
        };

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes,
                path: Some(path.to_path_buf()),
                width: w,
                height: h,
                mime,
                thumbnail,
            },
            title: file_name,
            subtitle,
            details: vec![format!("{}×{}", w, h), ext],
            handler_tag,
        })
    }

    fn from_raw_image_file(path: &Path) -> Option<Self> {
        let (preview_bytes, w, h) = crate::raw_image::extract_raw_image_preview(path).ok()?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "RAW Image".to_string());
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "RAW".to_string());
        let file_size_mb = std::fs::metadata(path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(0.0);
        let subtitle = format!(
            "Camera RAW ({}×{}) • {:.1} MB • {}",
            w, h, file_size_mb, ext
        );
        let thumbnail = ImageThumbnail::generate(&preview_bytes, 320, 220);

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes: preview_bytes,
                path: Some(path.to_path_buf()),
                width: w,
                height: h,
                mime: "image/x-camera-raw".to_string(),
                thumbnail,
            },
            title: file_name,
            subtitle,
            details: vec![format!("{}×{}", w, h), ext, "CAMERA RAW".to_string()],
            handler_tag: "RAW (CAMERA)".to_string(),
        })
    }

    fn from_pdf_file(path: &Path) -> Option<Self> {
        let (page1_bytes, w, h) = crate::pdf::render_pdf_page_preview(path, 1, 800).ok()?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());
        let page_count = crate::pdf::get_pdf_info(path)
            .map(|i| i.page_count)
            .unwrap_or(1);
        let file_size_kb = std::fs::metadata(path)
            .map(|m| m.len() as f64 / 1024.0)
            .unwrap_or(0.0);
        let subtitle = format!("PDF ({} pg) • {:.1} KB", page_count, file_size_kb);
        let thumbnail = ImageThumbnail::generate(&page1_bytes, 320, 220);

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes: page1_bytes,
                path: Some(path.to_path_buf()),
                width: w,
                height: h,
                mime: "application/pdf".to_string(),
                thumbnail,
            },
            title: file_name,
            subtitle,
            details: vec![format!("{} pages", page_count), "PDF".to_string()],
            handler_tag: "PDF (DOCUMENT)".to_string(),
        })
    }

    fn from_text_or_code_file(path: &Path) -> Option<Self> {
        let ext_lower = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let size_str = format_bytes(size_bytes);

        if matches!(ext_lower.as_str(), "md" | "markdown" | "mdown") {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "document.md".to_string());
            let subtitle = format!("Markdown • {}", size_str);
            return Some(Self {
                id: Uuid::new_v4(),
                created_at: current_unix_timestamp(),
                payload: ClipPayload::FileBatch(vec![path.to_path_buf()]),
                title: file_name,
                subtitle,
                details: vec!["Markdown".to_string(), size_str],
                handler_tag: "MARKDOWN (FILE)".to_string(),
            });
        }

        if ext_lower == "json" {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "data.json".to_string());
            return Some(Self {
                id: Uuid::new_v4(),
                created_at: current_unix_timestamp(),
                payload: ClipPayload::FileBatch(vec![path.to_path_buf()]),
                title: file_name,
                subtitle: format!("JSON • {}", size_str),
                details: vec!["JSON".to_string(), size_str],
                handler_tag: "JSON (FILE)".to_string(),
            });
        }

        if matches!(
            ext_lower.as_str(),
            "txt"
                | "text"
                | "log"
                | "ini"
                | "yaml"
                | "yml"
                | "toml"
                | "rs"
                | "py"
                | "c"
                | "cpp"
                | "h"
                | "js"
                | "ts"
                | "html"
                | "css"
                | "sh"
                | "conf"
                | "env"
        ) {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "document.txt".to_string());
            let ext_upper = if ext_lower.is_empty() {
                "TXT".to_string()
            } else {
                ext_lower.to_uppercase()
            };
            return Some(Self {
                id: Uuid::new_v4(),
                created_at: current_unix_timestamp(),
                payload: ClipPayload::FileBatch(vec![path.to_path_buf()]),
                title: file_name,
                subtitle: format!("{} • {}", ext_upper, size_str),
                details: vec![ext_upper.clone(), size_str],
                handler_tag: format!("{} (FILE)", ext_upper),
            });
        }

        None
    }

    fn from_svg_clipboard_text(trimmed: &str) -> Self {
        let bytes = trimmed.as_bytes().to_vec();
        let (w, h) = read_image_dimensions(&bytes).unwrap_or((0, 0));
        let size_kb = bytes.len() as f64 / 1024.0;
        let thumbnail = ImageThumbnail::generate(&bytes, 320, 220);
        let staging_path = crate::staging::ArtifactStore::new()
            .and_then(|store| store.stage_bytes("clipboard", "svg", "svg", &bytes))
            .ok();
        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes,
                path: staging_path,
                width: w,
                height: h,
                mime: "image/svg+xml".to_string(),
                thumbnail,
            },
            title: "Vector Graphic".to_string(),
            subtitle: if w > 0 && h > 0 {
                format!("Vector ({}×{}) • {:.1} KB • SVG", w, h, size_kb)
            } else {
                format!("Vector • {:.1} KB • SVG", size_kb)
            },
            details: vec!["SVG".to_string(), format!("{:.1} KB", size_kb)],
            handler_tag: "SVG (CLIPBOARD)".to_string(),
        }
    }

    fn from_json_text(text: &str, trimmed: &str) -> Option<Self> {
        let val = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
        let kind_desc = match &val {
            serde_json::Value::Object(map) => format!("Object ({} keys)", map.len()),
            serde_json::Value::Array(arr) => format!("Array ({} items)", arr.len()),
            _ => "JSON Value".to_string(),
        };
        let lines_count = text.lines().count();
        let chars_count = text.chars().count();
        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Text(text.to_string()),
            title: kind_desc,
            subtitle: format!("{} lines • {} chars", lines_count, chars_count),
            details: vec!["JSON".to_string(), format!("{}L", lines_count)],
            handler_tag: "JSON".to_string(),
        })
    }

    fn from_markdown_text(text: String) -> Self {
        let lines_count = text.lines().count();
        let chars_count = text.chars().count();
        let title = Self::preview_title_for_text(&text, "Markdown note");
        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Markdown(text),
            title,
            subtitle: format!("Markdown • {} lines • {}", lines_count, chars_count),
            details: vec![
                "Markdown".to_string(),
                format!("{} lines", lines_count),
                format!("{}c", chars_count),
            ],
            handler_tag: "MARKDOWN".to_string(),
        }
    }

    fn preview_title_for_text(text: &str, default_title: &str) -> String {
        let preview = text
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches('#')
            .trim()
            .chars()
            .take(40)
            .collect::<String>();
        if preview.is_empty() {
            default_title.to_string()
        } else {
            preview
        }
    }

    /// Creates a URL `ClipItem` from a parsed `Url`.
    pub fn from_url(parsed: url::Url) -> Self {
        let host = parsed.host_str().unwrap_or("URL").to_string();
        let path = parsed.path().to_string();
        let title = host.clone();
        let subtitle = path.chars().take(50).collect::<String>();

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Url(parsed),
            title,
            subtitle,
            details: vec!["URL".to_string(), host],
            handler_tag: "URL".to_string(),
        }
    }

    /// Creates a Color `ClipItem` from RGB bytes and hex string.
    pub fn from_color((r, g, b, hex): (u8, u8, u8, String)) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Color {
                hex: hex.clone(),
                r,
                g,
                b,
            },
            title: hex.clone(),
            subtitle: format!("rgb({}, {}, {})", r, g, b),
            details: vec!["Color".to_string(), hex],
            handler_tag: "COLOR".to_string(),
        }
    }

    /// Creates an Image `ClipItem` with known dimensions and MIME string.
    pub fn from_image(bytes: Vec<u8>, width: u32, height: u32, mime: impl Into<String>) -> Self {
        let mime = mime.into();
        let size_kb = bytes.len() as f64 / 1024.0;
        let title = format!("{}×{}", width, height);
        let subtitle = format!("{:.1} KB • {}", size_kb, mime);

        let ext = image_extension_for_mime(&mime).unwrap_or("png");
        let staging_path = crate::staging::ArtifactStore::new()
            .and_then(|store| store.stage_bytes("clipboard", "img", ext, &bytes))
            .ok();

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes,
                path: staging_path,
                width,
                height,
                mime: mime.clone(),
                thumbnail: None,
            },
            title,
            subtitle,
            details: vec![format!("{}×{}", width, height), mime],
            handler_tag: "IMAGE".to_string(),
        }
    }

    /// Reads audio metadata from a file path and creates an `AudioRef` `ClipItem`.
    pub fn from_audio(path: PathBuf) -> Option<Self> {
        let meta = crate::audio::read_audio_metadata(&path)?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Audio file".to_string());

        let mins = meta.duration_ms / 60_000;
        let secs = (meta.duration_ms % 60_000) / 1000;
        let dur_str = format!("{:02}:{:02}", mins, secs);

        let ch_str = match meta.channels {
            1 => "Mono".to_string(),
            2 => "Stereo".to_string(),
            6 => "5.1 (6 ch)".to_string(),
            8 => "7.1 (8 ch)".to_string(),
            n => format!("{} ch", n),
        };

        let khz = meta.sample_rate as f64 / 1000.0;
        let subtitle = format!(
            "{} • {:.1} kHz • {} ({})",
            dur_str, khz, ch_str, meta.format
        );

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::AudioRef {
                path,
                duration_ms: meta.duration_ms,
                sample_rate: meta.sample_rate,
                channels: meta.channels,
                format: meta.format,
                waveform: None,
            },
            title: file_name,
            subtitle,
            details: vec![dur_str, ch_str],
            handler_tag: "AUDIO (FILE)".to_string(),
        })
    }

    /// Reads video metadata via `ffprobe` and creates an `Animation` `ClipItem`.
    pub fn from_video(path: PathBuf) -> Option<Self> {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Video file".to_string());
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "VIDEO".to_string());

        let (width, height, duration_ms) =
            crate::animation::read_video_metadata(&path).unwrap_or((0, 0, 0));

        let mins = duration_ms / 60_000;
        let secs = (duration_ms % 60_000) / 1000;
        let dur_text = format!("{:02}:{:02}", mins, secs);

        let subtitle = if width > 0 && height > 0 {
            format!("{} • {}×{} ({})", dur_text, width, height, ext)
        } else {
            format!("{} ({})", dur_text, ext)
        };

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Animation {
                path,
                format: ext.clone(),
                width,
                height,
                duration_ms,
                thumbnail: None,
            },
            title: file_name,
            subtitle,
            details: vec![dur_text, format!("{}×{}", width, height), ext],
            handler_tag: "VIDEO (FILE)".to_string(),
        })
    }

    /// Verifies animated image headers (GIF/WebP) and creates an `Animation` `ClipItem`.
    pub fn from_animated_image(path: PathBuf) -> Option<Self> {
        let bytes = std::fs::read(&path).ok()?;
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "ANIM".to_string());

        let is_anim = if ext == "GIF" {
            use image::AnimationDecoder;
            use image::codecs::gif::GifDecoder;
            if let Ok(decoder) = GifDecoder::new(std::io::Cursor::new(&bytes)) {
                let mut iter = decoder.into_frames();
                iter.next().is_some() && iter.next().is_some()
            } else {
                false
            }
        } else if ext == "WEBP" {
            use image::codecs::webp::WebPDecoder;
            WebPDecoder::new(std::io::Cursor::new(&bytes))
                .map(|d| d.has_animation())
                .unwrap_or(false)
        } else {
            false
        };

        if !is_anim {
            return None;
        }

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Animated image".to_string());

        let (width, height) = read_image_dimensions(&bytes).unwrap_or((0, 0));
        let size_kb = bytes.len() as f64 / 1024.0;
        let subtitle = format!("{}×{} • {:.1} KB • {}", width, height, size_kb, ext);

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Animation {
                path,
                format: ext.clone(),
                width,
                height,
                duration_ms: 0,
                thumbnail: None,
            },
            title: file_name,
            subtitle,
            details: vec![format!("{}×{}", width, height), ext.clone()],
            handler_tag: format!("{} (ANIM)", ext),
        })
    }

    /// Creates a Directory `ClipItem` referencing a filesystem folder.
    pub fn from_directory(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Directory(path),
            title: name,
            subtitle: "Directory".to_string(),
            details: vec!["Directory".to_string()],
            handler_tag: "DIRECTORY".to_string(),
        }
    }

    /// Creates a `ClipItem` for a single file on disk, attempting format-specific dispatch.
    pub fn from_single_file(p: &Path) -> Self {
        if is_image_path(p) {
            if let Ok(bytes) = std::fs::read(p) {
                let mime = guess_image_mime(p);
                if let Some(mut item) = Self::from_image_bytes(bytes, mime) {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.to_string_lossy().to_string());
                    item.title = name;
                    if let ClipPayload::Image { ref mut path, .. } = item.payload {
                        *path = Some(p.to_path_buf());
                    }
                    return item;
                }
            }
        }

        if crate::audio::is_audio_path(p) {
            if let Some(item) = Self::from_audio(p.to_path_buf()) {
                return item;
            }
        }

        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string_lossy().to_string());
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let size_bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let size_str = format_bytes(size_bytes);

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::FileBatch(vec![p.to_path_buf()]),
            title: name,
            subtitle: format!("{} • {}", ext.to_uppercase(), size_str),
            details: vec![ext.to_uppercase(), size_str],
            handler_tag: "BINARY (FILE)".to_string(),
        }
    }

    /// Creates a `ClipItem` representing a collection of files.
    pub fn from_files(paths: Vec<PathBuf>) -> Self {
        let count = paths.len();
        let first_name = paths
            .first()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Batch".to_string());

        let total_size: u64 = paths
            .iter()
            .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum();

        let all_audio = !paths.is_empty() && paths.iter().all(|p| crate::audio::is_audio_path(p));
        let (title, handler_tag) = if all_audio {
            (
                if count > 1 {
                    format!("{} (+{} clips)", first_name, count - 1)
                } else {
                    first_name
                },
                format!("AUDIO BATCH ({})", count),
            )
        } else {
            (
                if count > 1 {
                    format!("{} (+{} files)", first_name, count - 1)
                } else {
                    first_name
                },
                "FILE BATCH".to_string(),
            )
        };

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::FileBatch(paths),
            title,
            subtitle: format!("{} items • {}", count, format_bytes(total_size)),
            details: vec![format!("{} items", count), format_bytes(total_size)],
            handler_tag,
        }
    }

    /// Creates a MultiImage `ClipItem` representing multiple image files for gallery preview.
    pub fn from_multi_image(paths: Vec<PathBuf>) -> Self {
        let total_count = paths.len();
        let preview_indices = if total_count <= 4 {
            (0..total_count).collect::<Vec<usize>>()
        } else {
            vec![0, 1, total_count - 2, total_count - 1]
        };

        let thumbnails = Vec::new();

        let first_name = paths[0]
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_string());

        let total_size: u64 = paths
            .iter()
            .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum();

        let title = if total_count == 1 {
            first_name
        } else {
            format!("{} (+{} pics)", first_name, total_count - 1)
        };
        let subtitle = format!("{} pictures • {}", total_count, format_bytes(total_size));

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::MultiImage {
                paths,
                total_count,
                preview_indices,
                thumbnails,
            },
            title,
            subtitle,
            details: vec![
                format!("{} pictures", total_count),
                format_bytes(total_size),
            ],
            handler_tag: "MULTI-PICTURE".to_string(),
        }
    }

    /// Returns the high-level `PayloadKind` of this item.
    pub fn kind(&self) -> PayloadKind {
        match &self.payload {
            ClipPayload::Text(_) => PayloadKind::Text,
            ClipPayload::Markdown(_) => PayloadKind::Markdown,
            ClipPayload::Url(_) => PayloadKind::Url,
            ClipPayload::Image { .. } => PayloadKind::Image,
            ClipPayload::MultiImage { .. } => PayloadKind::MultiImage,
            ClipPayload::AudioRef { .. } => PayloadKind::Audio,
            ClipPayload::Animation { .. } => PayloadKind::Animation,
            ClipPayload::FileBatch(_) => PayloadKind::FileBatch,
            ClipPayload::Directory(_) => PayloadKind::Directory,
            ClipPayload::Color { .. } => PayloadKind::Color,
        }
    }

    /// Returns an audio file path if the payload contains or references audio.
    pub fn audio_path(&self) -> Option<PathBuf> {
        match &self.payload {
            ClipPayload::AudioRef { path, .. } => Some(path.clone()),
            ClipPayload::FileBatch(files)
                if files.len() == 1 && crate::audio::is_audio_path(&files[0]) =>
            {
                Some(files[0].clone())
            }
            ClipPayload::Text(text) => {
                let trimmed = text.trim();
                let path = Path::new(trimmed);
                if path.is_file() && crate::audio::is_audio_path(path) {
                    Some(path.to_path_buf())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Returns an image file path if the payload contains or references an image.
    pub fn image_path(&self) -> Option<PathBuf> {
        match &self.payload {
            ClipPayload::Image { path: Some(p), .. } if p.is_file() => Some(p.clone()),
            ClipPayload::FileBatch(files)
                if files.len() == 1 && is_image_path(&files[0]) && files[0].is_file() =>
            {
                Some(files[0].clone())
            }
            ClipPayload::Text(text) => {
                let trimmed = text.trim();
                let path = Path::new(trimmed);
                if path.is_file() && is_image_path(path) {
                    Some(path.to_path_buf())
                } else {
                    let extracted = extract_local_file_paths(trimmed);
                    if extracted.len() == 1 && is_image_path(&extracted[0]) {
                        Some(extracted[0].clone())
                    } else {
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// Returns `true` if this item is an image payload or points to an image file.
    pub fn is_image(&self) -> bool {
        match &self.payload {
            ClipPayload::Image { .. } => true,
            _ => self.image_path().is_some(),
        }
    }

    /// Resolves the raw image bytes, MIME type string, and optional source path.
    pub fn resolve_image_data(&self) -> Option<(Vec<u8>, String, Option<PathBuf>)> {
        match &self.payload {
            ClipPayload::Image {
                bytes, path, mime, ..
            } => {
                let actual_bytes = if !bytes.is_empty() {
                    bytes.clone()
                } else if let Some(p) = path {
                    std::fs::read(p).ok()?
                } else {
                    return None;
                };
                Some((actual_bytes, mime.clone(), path.clone()))
            }
            _ => {
                if let Some(p) = self.image_path() {
                    if let Ok(bytes) = std::fs::read(&p) {
                        let mime = guess_image_mime(&p);
                        return Some((bytes, mime, Some(p)));
                    }
                }
                None
            }
        }
    }
}

/// Returns `true` if the path has a standard raster or vector image extension.
pub fn is_image_path(p: &Path) -> bool {
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        matches!(ext.to_lowercase().as_str(), "svg" | "heif" | "heic")
            || image::ImageFormat::from_extension(ext).is_some_and(|format| {
                format.reading_enabled() || format == image::ImageFormat::Avif
            })
    } else {
        false
    }
}

/// Guesses the MIME type string from a file path's extension.
pub fn guess_image_mime(p: &Path) -> String {
    let ext = p
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "svg" => "image/svg+xml".to_string(),
        "heif" => "image/heif".to_string(),
        "heic" => "image/heic".to_string(),
        "ff" => "image/x-farbfeld".to_string(),
        _ => image::ImageFormat::from_extension(&ext)
            .map(|format| format.to_mime_type())
            .unwrap_or("image/png")
            .to_string(),
    }
}

/// Parses local file paths or `file://` URIs from clipboard text, expanding tildes and validating existence.
pub fn extract_local_file_paths(text: &str) -> Vec<PathBuf> {
    let home = std::env::var("HOME").ok();
    let home_ref = home.as_deref();
    let non_empty_lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if non_empty_lines.is_empty() {
        return Vec::new();
    }

    if non_empty_lines.len() > 1 {
        extract_from_multiline(&non_empty_lines, home_ref)
    } else {
        extract_from_single_line(non_empty_lines[0], home_ref)
    }
}

fn extract_from_multiline(lines: &[&str], home: Option<&str>) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(lines.len());
    for line in lines {
        if let Some(p) = resolve_candidate_path(line, home, false) {
            if !paths.contains(&p) {
                paths.push(p);
            }
        } else {
            // If not all lines are valid paths, the multiline text is code/prose, not a file batch
            return Vec::new();
        }
    }
    paths
}

fn extract_from_single_line(line: &str, home: Option<&str>) -> Vec<PathBuf> {
    if let Some(p) = resolve_candidate_path(line, home, true) {
        return vec![p];
    }

    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() > 1 {
        let mut paths = Vec::with_capacity(tokens.len());
        for token in tokens {
            if let Some(p) = resolve_candidate_path(token, home, false) {
                if !paths.contains(&p) {
                    paths.push(p);
                }
            } else {
                return Vec::new();
            }
        }
        return paths;
    }

    Vec::new()
}

fn resolve_candidate_path(s: &str, home: Option<&str>, is_sole_input: bool) -> Option<PathBuf> {
    let clean = s.trim().trim_matches('"').trim_matches('\'');
    if is_disallowed_path_string(clean, is_sole_input) {
        return None;
    }

    let raw = parse_raw_path_string(clean, home)?;
    let p = PathBuf::from(raw);
    if p.exists() { Some(p) } else { None }
}

fn is_disallowed_path_string(clean: &str, is_sole_input: bool) -> bool {
    if clean.is_empty() || clean.starts_with('#') {
        return true;
    }
    if clean.starts_with("//") || clean.starts_with("/*") || clean.starts_with("*/") {
        return true;
    }
    if clean == "." || clean == ".." {
        return true;
    }
    if !is_sole_input && clean == "/" {
        return true;
    }
    false
}

fn parse_raw_path_string(clean: &str, home: Option<&str>) -> Option<String> {
    if let Some(stripped) = clean.strip_prefix("file://") {
        percent_encoding::percent_decode_str(stripped)
            .decode_utf8()
            .ok()
            .map(|s| s.into_owned())
    } else if clean.starts_with('/') {
        Some(clean.to_string())
    } else if let Some(h) = home {
        clean
            .strip_prefix("~/")
            .map(|rest| format!("{}/{}", h, rest))
    } else {
        None
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn current_unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_multi_image_selection() {
        let p1 = PathBuf::from("img1.png");
        let p2 = PathBuf::from("img2.jpg");
        let p3 = PathBuf::from("img3.webp");
        let p4 = PathBuf::from("img4.png");
        let p5 = PathBuf::from("img5.jpg");
        let p6 = PathBuf::from("img6.png");

        // Test <= 4 files (all 4 selected)
        let item_4 =
            ClipItem::from_multi_image(vec![p1.clone(), p2.clone(), p3.clone(), p4.clone()]);
        if let ClipPayload::MultiImage {
            total_count,
            preview_indices,
            ..
        } = item_4.payload
        {
            assert_eq!(total_count, 4);
            assert_eq!(preview_indices, vec![0, 1, 2, 3]);
        } else {
            panic!("Expected MultiImage payload");
        }

        // Test > 4 files (first 2 and last 2 selected)
        let item_6 = ClipItem::from_multi_image(vec![p1, p2, p3, p4, p5, p6]);
        if let ClipPayload::MultiImage {
            total_count,
            preview_indices,
            ..
        } = item_6.payload
        {
            assert_eq!(total_count, 6);
            assert_eq!(preview_indices, vec![0, 1, 4, 5]); // First 2 (0, 1) and last 2 (4, 5)
        } else {
            panic!("Expected MultiImage payload");
        }
    }

    #[test]
    fn test_svg_detection_and_thumbnail() {
        let svg_markup = r#"<svg width="200" height="100" viewBox="0 0 200 100" xmlns="http://www.w3.org/2000/svg"><rect width="200" height="100" fill="red"/></svg>"#;
        assert!(is_svg_bytes(svg_markup.as_bytes()));
        let dims = read_svg_dimensions(svg_markup.as_bytes());
        assert_eq!(dims, Some((200, 100)));

        let item = ClipItem::from_text(svg_markup);
        assert_eq!(item.handler_tag, "SVG (CLIPBOARD)");
        if let ClipPayload::Image {
            width,
            height,
            thumbnail,
            mime,
            ..
        } = item.payload
        {
            assert_eq!(width, 200);
            assert_eq!(height, 100);
            assert_eq!(mime, "image/svg+xml");
            assert!(thumbnail.is_some());
            let thumb = thumbnail.unwrap();
            assert!(thumb.width > 0 && thumb.height > 0);
        } else {
            panic!("Expected Image payload with SVG");
        }
    }

    #[test]
    fn test_from_files_audio_batch() {
        let paths = vec![
            PathBuf::from("/tmp/sample_01.wav"),
            PathBuf::from("/tmp/sample_02.wav"),
            PathBuf::from("/tmp/sample_03.wav"),
        ];
        let item = ClipItem::from_files(paths.clone());
        assert_eq!(item.handler_tag, "AUDIO BATCH (3)");
        assert_eq!(item.title, "sample_01.wav (+2 clips)");
        if let ClipPayload::FileBatch(files) = item.payload {
            assert_eq!(files.len(), 3);
            assert_eq!(files, paths);
        } else {
            panic!("Expected FileBatch payload");
        }
    }

    #[test]
    fn test_image_detection_and_resolve() {
        let fake_png_bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let item = ClipItem::from_image(fake_png_bytes.clone(), 100, 200, "image/png");
        assert!(item.is_image());
        let resolved = item.resolve_image_data();
        assert!(resolved.is_some());
        let (bytes, mime, path) = resolved.unwrap();
        assert_eq!(bytes, fake_png_bytes);
        assert_eq!(mime, "image/png");
        assert!(path.is_some());
    }

    #[test]
    fn test_code_with_comments_not_treated_as_directory() {
        let snippet = r#"
        egui::Frame::none()
            .fill(Color32::from_rgb(15, 23, 42)) // Slate 900
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                // 1. Header Toolbar
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Table")
                            .size(24.0),
                    );
                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(18.0)
                            .color(Color32::WHITE),
                    );

                    ui.add_space(8.0);
                    let info_badge = format!("{} rows × {} cols", total_rows, total_cols);
                    ui.label(
                        egui::RichText::new(info_badge)
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    // Search box
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new("Search").small());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Filter cells...")
                            .desired_width(180.0),
                    );
        "#;
        let paths = extract_local_file_paths(snippet);
        assert!(paths.is_empty(), "Expected empty paths, got: {:?}", paths);

        let item = ClipItem::from_text(snippet);
        assert_eq!(item.handler_tag, "TEXT");
        match item.payload {
            ClipPayload::Text(content) => {
                assert!(content.contains("Header Toolbar"));
            }
            other => panic!("Expected ClipPayload::Text, got: {:?}", other),
        }
    }

    #[test]
    fn test_copied_markdown_file_preserves_file_batch() {
        let temp_dir = std::env::temp_dir().join("little_clip_test_md");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("sample_interview.md");
        std::fs::write(&file_path, "# Interview with Soatok\n\nThis is content.\n").unwrap();

        let uri = format!("file://{}", file_path.to_string_lossy());
        let item = ClipItem::from_text(uri);

        assert_eq!(item.handler_tag, "MARKDOWN (FILE)");
        assert_eq!(item.title, "sample_interview.md");
        assert_eq!(item.file_path(), Some(file_path.clone()));

        // Crucial: payload MUST be FileBatch, and clipboard text MUST be the file path (not contents)
        match &item.payload {
            ClipPayload::FileBatch(files) => {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0], file_path);
            }
            other => panic!("Expected FileBatch payload, got: {:?}", other),
        }
        assert_eq!(
            item.to_clipboard_text(),
            Some(file_path.to_string_lossy().to_string())
        );

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn test_copied_code_file_preserves_file_batch() {
        let temp_dir = std::env::temp_dir().join("little_clip_test_c");
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("main.c");
        std::fs::write(&file_path, "#include <stdio.h>\nint main() { return 0; }\n").unwrap();

        let path_str = file_path.to_string_lossy().to_string();
        let item = ClipItem::from_text(&path_str);

        assert_eq!(item.handler_tag, "C (FILE)");
        assert_eq!(item.title, "main.c");
        assert_eq!(item.file_path(), Some(file_path.clone()));

        match &item.payload {
            ClipPayload::FileBatch(files) => {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0], file_path);
            }
            other => panic!("Expected FileBatch payload, got: {:?}", other),
        }
        assert_eq!(item.to_clipboard_text(), Some(path_str));

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn invalid_or_mismatched_image_bytes_are_rejected() {
        assert!(ClipItem::from_image_bytes(b"not an image".to_vec(), "image/png").is_none());
        assert!(
            ClipItem::from_image_bytes(b"not an image".to_vec(), "application/octet-stream")
                .is_none()
        );
    }
}
