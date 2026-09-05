use image::GenericImageView;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PayloadKind {
    Text,
    Markdown,
    Url,
    Image,
    MultiImage,
    Audio,
    Animation,
    FileBatch,
    Directory,
    Color,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageThumbnail {
    pub width: u32,
    pub height: u32,
    pub premultiplied_rgba: Vec<u8>,
}

impl ImageThumbnail {
    pub fn generate(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Self> {
        if let Ok(dyn_img) = image::load_from_memory(bytes) {
            let thumb = dyn_img.thumbnail(max_w, max_h);
            let (tw, th) = thumb.dimensions();
            let rgba = thumb.to_rgba8();

            let mut premultiplied = Vec::with_capacity(rgba.len());
            for pixel in rgba.chunks_exact(4) {
                let a = pixel[3] as u32;
                let r = ((pixel[0] as u32 * a) / 255) as u8;
                let g = ((pixel[1] as u32 * a) / 255) as u8;
                let b = ((pixel[2] as u32 * a) / 255) as u8;
                premultiplied.extend_from_slice(&[r, g, b, pixel[3]]);
            }

            return Some(Self {
                width: tw,
                height: th,
                premultiplied_rgba: premultiplied,
            });
        }

        // If raster loading fails, try rendering as SVG
        if is_svg_bytes(bytes) {
            return Self::generate_from_svg(bytes, max_w, max_h);
        }

        None
    }

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

pub fn is_svg_bytes(bytes: &[u8]) -> bool {
    let header = std::str::from_utf8(bytes.get(..1024).unwrap_or(bytes)).unwrap_or("");
    let trimmed = header.trim_start();
    trimmed.starts_with("<svg")
        || (trimmed.starts_with("<?xml") && header.contains("<svg"))
        || header.contains("<svg")
}

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
            let num: String = val.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
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

pub fn read_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    use std::io::Cursor;
    if let Ok(reader) = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format() {
        if let Ok(dims) = reader.into_dimensions() {
            return Some(dims);
        }
    }
    if is_svg_bytes(bytes) {
        return read_svg_dimensions(bytes);
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClipPayload {
    Text(String),
    Markdown(String),
    Url(url::Url),
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
    AudioRef {
        path: PathBuf,
        duration_ms: u64,
        sample_rate: u32,
        channels: u16,
        format: String,
        #[serde(default)]
        waveform: Option<crate::audio::AudioWaveform>,
    },
    Animation {
        path: PathBuf,
        format: String,
        width: u32,
        height: u32,
        duration_ms: u64,
        #[serde(default)]
        thumbnail: Option<crate::animation::AnimationThumbnail>,
    },
    FileBatch(Vec<PathBuf>),
    MultiImage {
        paths: Vec<PathBuf>,
        total_count: usize,
        preview_indices: Vec<usize>,
        #[serde(default)]
        thumbnails: Vec<ImageThumbnail>,
    },
    Directory(PathBuf),
    Color {
        hex: String,
        r: u8,
        g: u8,
        b: u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipItem {
    pub id: Uuid,
    pub created_at: u64,
    pub payload: ClipPayload,
    pub title: String,
    pub subtitle: String,
    pub details: Vec<String>,
    pub handler_tag: String,
}

impl ClipItem {
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
            ClipPayload::AudioRef { path, .. } => {
                Some(path.to_string_lossy().to_string())
            }
            _ => None,
        }
    }

    pub fn with_handler(mut self, handler: impl Into<String>) -> Self {
        self.handler_tag = handler.into();
        self
    }

    pub fn from_image_bytes(bytes: Vec<u8>, mime: impl Into<String>) -> Option<Self> {
        let mime = mime.into();
        // Fast header-only dimension extraction in ~0.05ms (does NOT decode pixels)
        let (width, height) = read_image_dimensions(&bytes).unwrap_or((0, 0));
        let size_kb = bytes.len() as f64 / 1024.0;
        let title = if width > 0 && height > 0 {
            format!("Image ({}×{})", width, height)
        } else {
            "Image".to_string()
        };
        let subtitle = format!("{:.1} KB • {}", size_kb, mime.replace("image/", "").to_uppercase());

        let ext = if mime.contains("png") {
            "png"
        } else if mime.contains("jpeg") || mime.contains("jpg") {
            "jpg"
        } else if mime.contains("webp") {
            "webp"
        } else if mime.contains("svg") {
            "svg"
        } else {
            "png"
        };
        let staging_dir = std::env::temp_dir().join("little_clip_staging");
        let _ = std::fs::create_dir_all(&staging_dir);
        let staging_path = staging_dir.join(format!("{}.{}", Uuid::new_v4(), ext));
        let _ = std::fs::write(&staging_path, &bytes);

        Some(Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes,
                path: Some(staging_path),
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

    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let trimmed = text.trim();

        // 1. Check if copied text represents one or more local file paths / URIs (e.g. from file manager)
        let candidate_paths = extract_local_file_paths(trimmed);
        if !candidate_paths.is_empty() {
            if candidate_paths.len() == 1 {
                let path = &candidate_paths[0];
                if path.is_dir() {
                    return Self::from_directory(path.clone());
                }

                // Check if it's a video file on disk
                if crate::animation::is_video_path(path) {
                    if let Some(item) = Self::from_video(path.clone()) {
                        return item;
                    }
                }

                // Check if it's an animated image file (GIF or animated WebP) on disk
                if crate::animation::is_animated_image_path(path) {
                    if let Some(item) = Self::from_animated_image(path.clone()) {
                        return item;
                    }
                }

                // Check if it's an image file on disk
                if is_image_path(path) {
                    if let Ok(bytes) = std::fs::read(path) {
                        let (w, h) = read_image_dimensions(&bytes).unwrap_or((0, 0));
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

                        let thumbnail = ImageThumbnail::generate(&bytes, 320, 220);

                        let handler_tag = if ext == "SVG" {
                            "SVG (VECTOR)".to_string()
                        } else {
                            "IMAGE (FILE)".to_string()
                        };

                        let mime = if ext == "SVG" {
                            "image/svg+xml".to_string()
                        } else {
                            format!("image/{}", ext.to_lowercase())
                        };

                        return Self {
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
                        };
                    }
                }

                // Check if it's an audio file on disk
                if crate::audio::is_audio_path(path) {
                    if let Some(item) = Self::from_audio(path.clone()) {
                        return item;
                    }
                }

                // Check if Markdown file
                let ext_lower = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if matches!(ext_lower.as_str(), "md" | "markdown" | "mdown") {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "document.md".to_string());
                        let lines_count = content.lines().count();
                        let size_str = format_bytes(content.len() as u64);
                        return Self {
                            id: Uuid::new_v4(),
                            created_at: current_unix_timestamp(),
                            payload: ClipPayload::Markdown(content),
                            title: file_name,
                            subtitle: format!("Markdown • {} lines • {}", lines_count, size_str),
                            details: vec!["Markdown".to_string(), format!("{} lines", lines_count), size_str],
                            handler_tag: "MARKDOWN (FILE)".to_string(),
                        };
                    }
                }

                // Check if Text file (.txt, .text, .log, .ini, .yaml, .yml, .toml, code files)
                if matches!(
                    ext_lower.as_str(),
                    "txt" | "text" | "log" | "ini" | "yaml" | "yml" | "toml" | "rs" | "py" | "c"
                        | "cpp" | "h" | "js" | "ts" | "html" | "css" | "sh" | "conf" | "env"
                ) {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "document.txt".to_string());
                        let lines_count = content.lines().count();
                        let size_str = format_bytes(content.len() as u64);
                        let ext_upper = if ext_lower.is_empty() {
                            "TXT".to_string()
                        } else {
                            ext_lower.to_uppercase()
                        };
                        return Self {
                            id: Uuid::new_v4(),
                            created_at: current_unix_timestamp(),
                            payload: ClipPayload::Text(content),
                            title: file_name,
                            subtitle: format!("{} • {} lines • {}", ext_upper, lines_count, size_str),
                            details: vec![ext_upper.clone(), format!("{} lines", lines_count), size_str],
                            handler_tag: format!("{} (FILE)", ext_upper),
                        };
                    }
                }

                // Check if JSON file
                if ext_lower == "json" {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        if serde_json::from_str::<serde_json::Value>(&content).is_ok() {
                            let file_name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "data.json".to_string());
                            let size_str = format_bytes(content.len() as u64);
                            return Self {
                                id: Uuid::new_v4(),
                                created_at: current_unix_timestamp(),
                                payload: ClipPayload::Text(content),
                                title: file_name,
                                subtitle: format!("JSON • {}", size_str),
                                details: vec!["JSON".to_string(), size_str],
                                handler_tag: "JSON (FILE)".to_string(),
                            };
                        }
                    }
                }

                // General single file
                return Self::from_single_file(path);
            } else {
                // Multiple files
                if candidate_paths.iter().all(|p| is_image_path(p)) {
                    return Self::from_multi_image(candidate_paths);
                }
                return Self::from_files(candidate_paths);
            }
        }

        // 2. Check if color hex (#RRGGBB or #RGB)
        if let Some(color) = parse_hex_color(trimmed) {
            return Self::from_color(color);
        }

        // 3. Check if raw SVG markup in clipboard
        if is_svg_bytes(trimmed.as_bytes()) && trimmed.contains("</svg>") {
            let bytes = trimmed.as_bytes().to_vec();
            let (w, h) = read_image_dimensions(&bytes).unwrap_or((0, 0));
            let size_kb = bytes.len() as f64 / 1024.0;
            let thumbnail = ImageThumbnail::generate(&bytes, 320, 220);
            let staging_dir = std::env::temp_dir().join("little_clip_staging");
            let _ = std::fs::create_dir_all(&staging_dir);
            let staging_path = staging_dir.join(format!("{}.svg", Uuid::new_v4()));
            let _ = std::fs::write(&staging_path, &bytes);
            return Self {
                id: Uuid::new_v4(),
                created_at: current_unix_timestamp(),
                payload: ClipPayload::Image {
                    bytes,
                    path: Some(staging_path),
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
            };
        }

        // 3. Check if JSON string
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                let kind_desc = match &val {
                    serde_json::Value::Object(map) => format!("Object ({} keys)", map.len()),
                    serde_json::Value::Array(arr) => format!("Array ({} items)", arr.len()),
                    _ => "JSON Value".to_string(),
                };
                let lines_count = text.lines().count();
                let chars_count = text.chars().count();
                return Self {
                    id: Uuid::new_v4(),
                    created_at: current_unix_timestamp(),
                    payload: ClipPayload::Text(text),
                    title: kind_desc,
                    subtitle: format!("{} lines • {} chars", lines_count, chars_count),
                    details: vec!["JSON".to_string(), format!("{}L", lines_count)],
                    handler_tag: "JSON".to_string(),
                };
            }
        }

        // 4. Check if URL
        if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
            && !trimmed.contains('\n')
        {
            if let Ok(parsed_url) = url::Url::parse(trimmed) {
                return Self::from_url(parsed_url);
            }
        }

        // 5. Check if Markdown text
        let is_markdown = trimmed.starts_with("# ")
            || trimmed.starts_with("## ")
            || trimmed.starts_with("### ")
            || (trimmed.contains("\n# ") || trimmed.contains("\n## "))
            || (trimmed.starts_with("- ") && trimmed.contains("\n- "))
            || (trimmed.starts_with("* ") && trimmed.contains("\n* "));

        let lines_count = text.lines().count();
        let chars_count = text.chars().count();
        let preview_title = text
            .lines()
            .next()
            .unwrap_or("")
            .trim_start_matches('#')
            .trim()
            .chars()
            .take(40)
            .collect::<String>();

        let title = if preview_title.is_empty() {
            if is_markdown { "Markdown note".to_string() } else { "Empty text".to_string() }
        } else {
            preview_title
        };

        if is_markdown {
            return Self {
                id: Uuid::new_v4(),
                created_at: current_unix_timestamp(),
                payload: ClipPayload::Markdown(text),
                title,
                subtitle: format!("Markdown • {} lines • {} chars", lines_count, chars_count),
                details: vec!["Markdown".to_string(), format!("{} lines", lines_count), format!("{}c", chars_count)],
                handler_tag: "MARKDOWN".to_string(),
            };
        }

        // 6. Default Plain Text
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

    pub fn from_image(bytes: Vec<u8>, width: u32, height: u32, mime: impl Into<String>) -> Self {
        let mime = mime.into();
        let size_kb = bytes.len() as f64 / 1024.0;
        let title = format!("{}×{}", width, height);
        let subtitle = format!("{:.1} KB • {}", size_kb, mime);

        let ext = if mime.contains("png") {
            "png"
        } else if mime.contains("jpeg") || mime.contains("jpg") {
            "jpg"
        } else if mime.contains("webp") {
            "webp"
        } else if mime.contains("svg") {
            "svg"
        } else {
            "png"
        };
        let staging_dir = std::env::temp_dir().join("little_clip_staging");
        let _ = std::fs::create_dir_all(&staging_dir);
        let staging_path = staging_dir.join(format!("{}.{}", Uuid::new_v4(), ext));
        let _ = std::fs::write(&staging_path, &bytes);

        Self {
            id: Uuid::new_v4(),
            created_at: current_unix_timestamp(),
            payload: ClipPayload::Image {
                bytes,
                path: Some(staging_path),
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
        let subtitle = format!("{} • {:.1} kHz • {} ({})", dur_str, khz, ch_str, meta.format);

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

    pub fn from_video(path: PathBuf) -> Option<Self> {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Video file".to_string());
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "VIDEO".to_string());

        let (width, height, duration_ms) = crate::animation::read_video_metadata(&path)
            .unwrap_or((0, 0, 0));

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

    pub fn from_animated_image(path: PathBuf) -> Option<Self> {
        let bytes = std::fs::read(&path).ok()?;
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_else(|| "ANIM".to_string());

        let is_anim = if ext == "GIF" {
            use image::codecs::gif::GifDecoder;
            use image::AnimationDecoder;
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

    pub fn from_single_file(p: &Path) -> Self {
        if is_image_path(p) {
            if let Ok(bytes) = std::fs::read(p) {
                let ext = p
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
            details: vec![format!("{} pictures", total_count), format_bytes(total_size)],
            handler_tag: "MULTI-PICTURE".to_string(),
        }
    }

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

    pub fn audio_path(&self) -> Option<PathBuf> {
        match &self.payload {
            ClipPayload::AudioRef { path, .. } => Some(path.clone()),
            ClipPayload::FileBatch(files) if files.len() == 1 && crate::audio::is_audio_path(&files[0]) => {
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
}

pub fn is_image_path(p: &Path) -> bool {
    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
        matches!(
            ext.to_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "tiff" | "ico" | "avif"
        )
    } else {
        false
    }
}

pub fn extract_local_file_paths(text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let decoded = if let Some(stripped) = line.strip_prefix("file://") {
            percent_encoding::percent_decode_str(stripped)
                .decode_utf8()
                .ok()
                .map(|s| s.into_owned())
        } else if line.starts_with('/') {
            Some(line.to_string())
        } else {
            None
        };

        if let Some(path_str) = decoded {
            let p = PathBuf::from(path_str);
            if p.exists() {
                paths.push(p);
            }
        }
    }
    paths
}

fn parse_hex_color(s: &str) -> Option<(u8, u8, u8, String)> {
    let s = s.trim();
    if !s.starts_with('#') {
        return None;
    }
    let hex = &s[1..];
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some((r, g, b, format!("#{}", hex.to_uppercase())));
    }
    if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
        let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
        let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
        return Some((r, g, b, format!("#{}", hex.to_uppercase())));
    }
    None
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
        let item_4 = ClipItem::from_multi_image(vec![p1.clone(), p2.clone(), p3.clone(), p4.clone()]);
        if let ClipPayload::MultiImage { total_count, preview_indices, .. } = item_4.payload {
            assert_eq!(total_count, 4);
            assert_eq!(preview_indices, vec![0, 1, 2, 3]);
        } else {
            panic!("Expected MultiImage payload");
        }

        // Test > 4 files (first 2 and last 2 selected)
        let item_6 = ClipItem::from_multi_image(vec![p1, p2, p3, p4, p5, p6]);
        if let ClipPayload::MultiImage { total_count, preview_indices, .. } = item_6.payload {
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
        if let ClipPayload::Image { width, height, thumbnail, mime, .. } = item.payload {
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
