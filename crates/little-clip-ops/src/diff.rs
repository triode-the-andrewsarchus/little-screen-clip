use little_clip_core::{ClipItem, ClipPayload};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use uuid::Uuid;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffDisplayMode {
    Inline,
    SideBySide,
}

impl DiffDisplayMode {
    pub fn label(&self) -> &'static str {
        match self {
            DiffDisplayMode::Inline => "Inline",
            DiffDisplayMode::SideBySide => "Side-by-Side",
        }
    }

    pub fn toggled(&self) -> Self {
        match self {
            DiffDisplayMode::Inline => DiffDisplayMode::SideBySide,
            DiffDisplayMode::SideBySide => DiffDisplayMode::Inline,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DiffTarget {
    ClipboardText {
        label: String,
        content: String,
        ext_hint: Option<String>,
    },
    File {
        label: String,
        path: PathBuf,
        content: String,
    },
    Image {
        label: String,
        path: Option<PathBuf>,
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        mime: String,
    },
}

impl DiffTarget {
    pub fn label(&self) -> &str {
        match self {
            DiffTarget::ClipboardText { label, .. } => label,
            DiffTarget::File { label, .. } => label,
            DiffTarget::Image { label, .. } => label,
        }
    }

    pub fn content(&self) -> &str {
        match self {
            DiffTarget::ClipboardText { content, .. } => content,
            DiffTarget::File { content, .. } => content,
            DiffTarget::Image { .. } => "",
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, DiffTarget::File { .. })
    }

    pub fn is_image(&self) -> bool {
        matches!(self, DiffTarget::Image { .. })
    }

    pub fn image_bytes(&self) -> Option<&[u8]> {
        match self {
            DiffTarget::Image { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            DiffTarget::File { path, .. } => Some(path.as_path()),
            DiffTarget::Image { path, .. } => path.as_deref(),
            DiffTarget::ClipboardText { .. } => None,
        }
    }

    pub fn ext_hint(&self) -> Option<&str> {
        match self {
            DiffTarget::File { path, .. } => path.extension().and_then(|s| s.to_str()),
            DiffTarget::Image { path, mime, .. } => {
                path.as_ref()
                    .and_then(|p| p.extension().and_then(|s| s.to_str()))
                    .or_else(|| {
                        if mime.contains("png") {
                            Some("png")
                        } else if mime.contains("jpeg") || mime.contains("jpg") {
                            Some("jpg")
                        } else if mime.contains("webp") {
                            Some("webp")
                        } else {
                            None
                        }
                    })
            }
            DiffTarget::ClipboardText { ext_hint, .. } => ext_hint.as_deref(),
        }
    }

    pub fn line_count(&self) -> usize {
        match self {
            DiffTarget::ClipboardText { content, .. } | DiffTarget::File { content, .. } => {
                content.lines().count().max(1)
            }
            DiffTarget::Image { .. } => 1,
        }
    }

    pub fn byte_size(&self) -> usize {
        match self {
            DiffTarget::ClipboardText { content, .. } | DiffTarget::File { content, .. } => {
                content.len()
            }
            DiffTarget::Image { bytes, .. } => bytes.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiffResult {
    pub engine_name: String,
    pub raw_ansi: String,
    pub is_identical: bool,
    pub mode: DiffDisplayMode,
}

pub fn find_difft_bin() -> Option<PathBuf> {
    // 1. Check user local bin
    if let Ok(home) = std::env::var("HOME") {
        let local_bin = PathBuf::from(home).join(".local/bin/difft");
        if local_bin.is_file() {
            return Some(local_bin);
        }
    }

    // 2. Check standard system locations
    for p in &["/usr/bin/difft", "/usr/local/bin/difft", "/opt/homebrew/bin/difft"] {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }

    // 3. Test running `difft --version` in PATH
    if let Ok(output) = Command::new("difft").arg("--version").output() {
        if output.status.success() {
            return Some(PathBuf::from("difft"));
        }
    }

    None
}

pub fn guess_extension_from_content(content: &str) -> Option<&'static str> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return Some("json");
    }

    if trimmed.contains("fn ") || trimmed.contains("pub fn ") || trimmed.contains("use std::") {
        return Some("rs");
    }

    if trimmed.contains("def ") || trimmed.contains("import ") || trimmed.contains("class ") {
        return Some("py");
    }

    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") {
        return Some("html");
    }

    if trimmed.starts_with("---") || (trimmed.contains(": ") && trimmed.lines().count() > 2) {
        return Some("yaml");
    }

    if trimmed.starts_with("#!/bin/sh") || trimmed.starts_with("#!/bin/bash") {
        return Some("sh");
    }

    None
}

pub fn is_image_path(p: &Path) -> bool {
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "svg" | "gif" | "tiff" | "tif")
}

pub fn guess_image_mime(p: &Path) -> String {
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "webp" => "image/webp".to_string(),
        "bmp" => "image/bmp".to_string(),
        "svg" => "image/svg+xml".to_string(),
        "gif" => "image/gif".to_string(),
        _ => "image/png".to_string(),
    }
}

pub fn resolve_diff_target_from_path(p: &Path) -> Option<DiffTarget> {
    if !p.is_file() {
        return None;
    }

    if is_image_path(p) {
        if let Ok(bytes) = std::fs::read(p) {
            let (width, height) = little_clip_core::item::read_image_dimensions(&bytes).unwrap_or((0, 0));
            let mime = guess_image_mime(p);
            let label = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Image".to_string());
            return Some(DiffTarget::Image {
                label,
                path: Some(p.to_path_buf()),
                bytes,
                width,
                height,
                mime,
            });
        }
    }

    if let Ok(content) = std::fs::read_to_string(p) {
        let label = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "File".to_string());
        return Some(DiffTarget::File {
            label,
            path: p.to_path_buf(),
            content,
        });
    }

    None
}

/// If a single item contains 2 or more images or files (e.g. MultiImage, FileBatch, or Text with paths),
/// resolve the first two as a comparison pair.
pub fn resolve_diff_pair(item: &ClipItem) -> Option<(DiffTarget, DiffTarget)> {
    match &item.payload {
        ClipPayload::MultiImage { paths, .. } if paths.len() >= 2 => {
            let t1 = resolve_diff_target_from_path(&paths[0])?;
            let t2 = resolve_diff_target_from_path(&paths[1])?;
            Some((t1, t2))
        }
        ClipPayload::FileBatch(paths) if paths.len() >= 2 => {
            let img_paths: Vec<&PathBuf> = paths.iter().filter(|p| is_image_path(p)).collect();
            if img_paths.len() >= 2 {
                let t1 = resolve_diff_target_from_path(img_paths[0])?;
                let t2 = resolve_diff_target_from_path(img_paths[1])?;
                Some((t1, t2))
            } else {
                let t1 = resolve_diff_target_from_path(&paths[0])?;
                let t2 = resolve_diff_target_from_path(&paths[1])?;
                Some((t1, t2))
            }
        }
        ClipPayload::Text(text) => {
            let paths = little_clip_core::item::extract_local_file_paths(text);
            let img_paths: Vec<&PathBuf> = paths.iter().filter(|p| is_image_path(p)).collect();
            if img_paths.len() >= 2 {
                let t1 = resolve_diff_target_from_path(img_paths[0])?;
                let t2 = resolve_diff_target_from_path(img_paths[1])?;
                Some((t1, t2))
            } else if paths.len() >= 2 {
                let t1 = resolve_diff_target_from_path(&paths[0])?;
                let t2 = resolve_diff_target_from_path(&paths[1])?;
                Some((t1, t2))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn resolve_diff_target(item: &ClipItem) -> Option<DiffTarget> {
    match &item.payload {
        ClipPayload::Image { bytes, path, width, height, mime, .. } => {
            let mut actual_path = path.clone();
            let mut actual_bytes = if !bytes.is_empty() {
                bytes.clone()
            } else if let Some(p) = path {
                std::fs::read(p).unwrap_or_default()
            } else {
                Vec::new()
            };

            // If bytes are empty, aggressively search common locations and item metadata
            if actual_bytes.is_empty() {
                // Check if any details entry is a valid file
                for d in &item.details {
                    let p = Path::new(d);
                    if p.is_file() {
                        if let Ok(b) = std::fs::read(p) {
                            actual_bytes = b;
                            actual_path = Some(p.to_path_buf());
                            break;
                        }
                    }
                }
            }

            if actual_bytes.is_empty() {
                let p = PathBuf::from(&item.title);
                if p.is_file() {
                    if let Ok(b) = std::fs::read(&p) {
                        actual_bytes = b;
                        actual_path = Some(p);
                    }
                }
            }

            if actual_bytes.is_empty() {
                if let Ok(home) = std::env::var("HOME") {
                    let candidate = PathBuf::from(&home).join("Documents/type_testing").join(&item.title);
                    if candidate.is_file() {
                        if let Ok(b) = std::fs::read(&candidate) {
                            actual_bytes = b;
                            actual_path = Some(candidate);
                        }
                    }
                }
            }

            if actual_bytes.is_empty() {
                let staging = std::env::temp_dir().join("little_clip_staging").join(&item.title);
                if staging.is_file() {
                    if let Ok(b) = std::fs::read(&staging) {
                        actual_bytes = b;
                        actual_path = Some(staging);
                    }
                }
            }

            let (actual_w, actual_h) = if *width > 0 && *height > 0 {
                (*width, *height)
            } else if !actual_bytes.is_empty() {
                little_clip_core::item::read_image_dimensions(&actual_bytes).unwrap_or((0, 0))
            } else {
                (0, 0)
            };

            let label = if !item.title.is_empty() && item.title != "Image" {
                item.title.clone()
            } else {
                format!("Image ({}×{})", actual_w, actual_h)
            };

            Some(DiffTarget::Image {
                label,
                path: actual_path,
                bytes: actual_bytes,
                width: actual_w,
                height: actual_h,
                mime: mime.clone(),
            })
        }
        ClipPayload::MultiImage { paths, .. } if !paths.is_empty() => {
            resolve_diff_target_from_path(&paths[0])
        }
        ClipPayload::FileBatch(paths) if !paths.is_empty() => {
            resolve_diff_target_from_path(&paths[0])
        }
        ClipPayload::Text(s) | ClipPayload::Markdown(s) => {
            let trimmed = s.trim();
            let path_candidate = Path::new(trimmed);
            if path_candidate.is_file() {
                if is_image_path(path_candidate) {
                    if let Ok(bytes) = std::fs::read(path_candidate) {
                        let (width, height) = little_clip_core::item::read_image_dimensions(&bytes).unwrap_or((0, 0));
                        let mime = guess_image_mime(path_candidate);
                        let label = path_candidate.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Image File".to_string());
                        return Some(DiffTarget::Image {
                            label,
                            path: Some(path_candidate.to_path_buf()),
                            bytes,
                            width,
                            height,
                            mime,
                        });
                    }
                }

                if let Ok(content) = std::fs::read_to_string(path_candidate) {
                    let label = path_candidate
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "File".to_string());
                    return Some(DiffTarget::File {
                        label,
                        path: path_candidate.to_path_buf(),
                        content,
                    });
                }
            }

            let ext_hint = guess_extension_from_content(s).map(|e| e.to_string());
            let line_count = s.lines().count();
            let label = if !item.title.is_empty() && item.title != "Text" {
                item.title.clone()
            } else {
                format!("Clipboard ({} lines)", line_count)
            };

            Some(DiffTarget::ClipboardText {
                label,
                content: s.clone(),
                ext_hint,
            })
        }
        ClipPayload::Url(u) => Some(DiffTarget::ClipboardText {
            label: "URL".to_string(),
            content: u.to_string(),
            ext_hint: None,
        }),
        _ => None,
    }
}

pub fn compute_diff(
    lhs: &DiffTarget,
    rhs: &DiffTarget,
    mode: DiffDisplayMode,
    width: usize,
) -> DiffResult {
    // Fast path: if text is byte-identical
    if lhs.content() == rhs.content() {
        let raw_ansi = "\x1b[2m--- No changes (Contents are identical) ---\x1b[0m\n".to_string();
        return DiffResult {
            engine_name: "Identical Check".to_string(),
            raw_ansi,
            is_identical: true,
            mode,
        };
    }

    match mode {
        // First-class unified diff with dual line numbers, +/- gutter, and word-level diffing
        DiffDisplayMode::Inline => generate_unified_inline_diff(lhs, rhs),
        // First-class split side-by-side diff with syntax highlighting, column alignment, and word diffing
        DiffDisplayMode::SideBySide => generate_split_diff(lhs, rhs, width),
    }
}

pub fn run_difft(
    difft_bin: &Path,
    lhs: &DiffTarget,
    rhs: &DiffTarget,
    mode: DiffDisplayMode,
    width: usize,
) -> std::io::Result<DiffResult> {
    let run_id = Uuid::new_v4();
    let temp_dir = std::env::temp_dir().join(format!("little_clip_diff_{}", &run_id.to_string()[..8]));
    let _ = std::fs::create_dir_all(&temp_dir);

    // Determine extension to assist tree-sitter language detection
    let ext_hint = lhs.ext_hint().or_else(|| rhs.ext_hint());
    let ext = little_clip_core::syntax::detect_language(
        if !lhs.content().is_empty() { lhs.content() } else { rhs.content() },
        ext_hint,
    );

    let (lhs_path, lhs_is_temp) = match lhs {
        DiffTarget::File { path, .. } => (path.clone(), false),
        DiffTarget::ClipboardText { content, .. } => {
            let path = temp_dir.join(format!("lhs.{}", ext));
            std::fs::write(&path, content)?;
            (path, true)
        }
        DiffTarget::Image { path: Some(p), .. } => (p.clone(), false),
        DiffTarget::Image { bytes, .. } => {
            let img_ext = lhs.ext_hint().unwrap_or("png");
            let path = temp_dir.join(format!("lhs.{}", img_ext));
            std::fs::write(&path, bytes)?;
            (path, true)
        }
    };

    let (rhs_path, rhs_is_temp) = match rhs {
        DiffTarget::File { path, .. } => (path.clone(), false),
        DiffTarget::ClipboardText { content, .. } => {
            let path = temp_dir.join(format!("rhs.{}", ext));
            std::fs::write(&path, content)?;
            (path, true)
        }
        DiffTarget::Image { path: Some(p), .. } => (p.clone(), false),
        DiffTarget::Image { bytes, .. } => {
            let img_ext = rhs.ext_hint().unwrap_or("png");
            let path = temp_dir.join(format!("rhs.{}", img_ext));
            std::fs::write(&path, bytes)?;
            (path, true)
        }
    };

    let display_label = if lhs.label().ends_with(&format!(".{}", ext))
        || rhs.label().ends_with(&format!(".{}", ext))
    {
        format!("{} ⟷ {}", lhs.label(), rhs.label())
    } else {
        format!("{} ⟷ {}.{}", lhs.label(), rhs.label(), ext)
    };

    let display_arg = match mode {
        DiffDisplayMode::Inline => "inline",
        DiffDisplayMode::SideBySide => "side-by-side",
    };

    // Invoke difft using GIT_EXTERNAL_DIFF 7-argument format to avoid printing /tmp paths
    let output = Command::new(difft_bin)
        .arg("--display")
        .arg(display_arg)
        .arg("--color")
        .arg("always")
        .arg("--width")
        .arg(width.to_string())
        .arg(&display_label)
        .arg(&lhs_path)
        .arg(".")
        .arg(".")
        .arg(&rhs_path)
        .arg(".")
        .arg(".")
        .output();

    if lhs_is_temp {
        let _ = std::fs::remove_file(&lhs_path);
    }
    if rhs_is_temp {
        let _ = std::fs::remove_file(&rhs_path);
    }
    let _ = std::fs::remove_dir(&temp_dir);

    let output = output?;
    let raw_ansi = String::from_utf8_lossy(&output.stdout).to_string();
    let is_identical = raw_ansi.trim().contains("No changes.") || raw_ansi.trim().is_empty();

    Ok(DiffResult {
        engine_name: "Difftastic 0.70.0 (AST)".to_string(),
        raw_ansi,
        is_identical,
        mode,
    })
}

pub fn generate_unified_inline_diff(lhs: &DiffTarget, rhs: &DiffTarget) -> DiffResult {
    let old_lines: Vec<&str> = lhs.content().lines().collect();
    let new_lines: Vec<&str> = rhs.content().lines().collect();

    let diff = similar::TextDiff::from_lines(lhs.content(), rhs.content());
    let groups = diff.grouped_ops(3);

    let ext_hint = lhs.ext_hint().or_else(|| rhs.ext_hint());
    let lang_id = little_clip_core::syntax::detect_language(
        if !lhs.content().is_empty() { lhs.content() } else { rhs.content() },
        ext_hint,
    );
    let lang_name = little_clip_core::syntax::language_display_name(lang_id);

    if groups.is_empty() {
        return DiffResult {
            engine_name: format!("Unified Diff ({})", lang_name),
            raw_ansi: "\x1b[2m--- No changes (Contents are identical) ---\x1b[0m\n".to_string(),
            is_identical: true,
            mode: DiffDisplayMode::Inline,
        };
    }

    let syntax = SYNTAX_SET
        .find_syntax_by_extension(lang_id)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang_id))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = THEME_SET
        .themes
        .get("base16-ocean.dark")
        .or_else(|| THEME_SET.themes.values().next())
        .expect("default theme");

    let mut highlighter = HighlightLines::new(syntax, theme);

    let max_line = old_lines.len().max(new_lines.len());
    let num_w = max_line.to_string().len().max(3);

    let mut out = String::new();

    // Clean, readable header with language badge
    out.push_str(&format!(
        "\x1b[1;91m--- LHS: {}\x1b[0m\n\x1b[1;92m+++ RHS: {}\x1b[0m  \x1b[2;36m[{}]\x1b[0m\n\n",
        lhs.label(),
        rhs.label(),
        lang_name,
    ));

    for group in &groups {
        if let Some(header) = format_hunk_header(group) {
            out.push_str(&header);
        } else {
            continue;
        }

        for op in group {
            match op {
                similar::DiffOp::Equal { old_index, new_index, len } => {
                    for offset in 0..*len {
                        let o_num = old_index + offset + 1;
                        let n_num = new_index + offset + 1;
                        let line = old_lines.get(old_index + offset).copied().unwrap_or("");
                        let hl_line = highlight_line_ansi(line, &mut highlighter, None);
                        out.push_str(&format!(
                            "\x1b[2;90m{:>w$} {:>w$} │\x1b[0m   {}\n",
                            o_num,
                            n_num,
                            hl_line,
                            w = num_w
                        ));
                    }
                }
                similar::DiffOp::Delete { old_index, old_len, .. } => {
                    for offset in 0..*old_len {
                        let o_num = old_index + offset + 1;
                        let line = old_lines.get(old_index + offset).copied().unwrap_or("");
                        let hl_line = highlight_line_ansi(line, &mut highlighter, Some("\x1b[41m"));
                        out.push_str(&format!(
                            "\x1b[91m{:>w$}\x1b[0m \x1b[2;90m{:>w$} │\x1b[0m \x1b[91m- \x1b[0m{}\n",
                            o_num,
                            "",
                            hl_line,
                            w = num_w
                        ));
                    }
                }
                similar::DiffOp::Insert { new_index, new_len, .. } => {
                    for offset in 0..*new_len {
                        let n_num = new_index + offset + 1;
                        let line = new_lines.get(new_index + offset).copied().unwrap_or("");
                        let hl_line = highlight_line_ansi(line, &mut highlighter, Some("\x1b[42m"));
                        out.push_str(&format!(
                            "\x1b[2;90m{:>w$}\x1b[0m \x1b[92m{:>w$}\x1b[0m \x1b[2;90m│\x1b[0m \x1b[92m+ \x1b[0m{}\n",
                            "",
                            n_num,
                            hl_line,
                            w = num_w
                        ));
                    }
                }
                similar::DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                    let min_len = (*old_len).min(*new_len);

                    // Print deleted lines (all '-' lines)
                    for offset in 0..*old_len {
                        let o_num = old_index + offset + 1;
                        let old_line = old_lines.get(old_index + offset).copied().unwrap_or("");
                        let hl_old = if offset < min_len {
                            let new_line = new_lines.get(new_index + offset).copied().unwrap_or("");
                            let (h_old, _) = format_word_diff_syntax(old_line, new_line, &mut highlighter);
                            h_old
                        } else {
                            highlight_line_ansi(old_line, &mut highlighter, Some("\x1b[41m"))
                        };
                        out.push_str(&format!(
                            "\x1b[91m{:>w$}\x1b[0m \x1b[2;90m{:>w$} │\x1b[0m \x1b[91m- \x1b[0m{}\n",
                            o_num,
                            "",
                            hl_old,
                            w = num_w
                        ));
                    }

                    // Print added lines (all '+' lines)
                    for offset in 0..*new_len {
                        let n_num = new_index + offset + 1;
                        let new_line = new_lines.get(new_index + offset).copied().unwrap_or("");
                        let hl_new = if offset < min_len {
                            let old_line = old_lines.get(old_index + offset).copied().unwrap_or("");
                            let (_, h_new) = format_word_diff_syntax(old_line, new_line, &mut highlighter);
                            h_new
                        } else {
                            highlight_line_ansi(new_line, &mut highlighter, Some("\x1b[42m"))
                        };
                        out.push_str(&format!(
                            "\x1b[2;90m{:>w$}\x1b[0m \x1b[92m{:>w$}\x1b[0m \x1b[2;90m│\x1b[0m \x1b[92m+ \x1b[0m{}\n",
                            "",
                            n_num,
                            hl_new,
                            w = num_w
                        ));
                    }
                }
            }
        }
    }

    DiffResult {
        engine_name: format!("Unified Diff ({})", lang_name),
        raw_ansi: out,
        is_identical: false,
        mode: DiffDisplayMode::Inline,
    }
}

fn format_hunk_header(group: &[similar::DiffOp]) -> Option<String> {
    let first_op = group.first()?;
    let last_op = group.last()?;

    let old_start = first_op.old_range().start + 1;
    let new_start = first_op.new_range().start + 1;
    let old_count = last_op.old_range().end - first_op.old_range().start;
    let new_count = last_op.new_range().end - first_op.new_range().start;

    Some(format!(
        "\x1b[36m@@ -{},{} +{},{} @@\x1b[0m\n",
        old_start, old_count, new_start, new_count
    ))
}

fn append_highlighted_tokens(
    out: &mut String,
    ranges: &[(syntect::highlighting::Style, &str)],
) {
    for (style, token_str) in ranges {
        let (r, g, b) = (style.foreground.r, style.foreground.g, style.foreground.b);
        out.push_str(&format!("\x1b[38;2;{};{};{}m{}", r, g, b, token_str));
    }
}

fn compute_word_diff_spans<'a>(
    word_diff: &similar::TextDiff<'a, 'a, 'a, str>,
) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    let mut old_spans = Vec::new();
    let mut new_spans = Vec::new();
    let mut cur_old = 0;
    let mut cur_new = 0;

    for change in word_diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            similar::ChangeTag::Delete => {
                old_spans.push((cur_old, cur_old + len));
                cur_old += len;
            }
            similar::ChangeTag::Insert => {
                new_spans.push((cur_new, cur_new + len));
                cur_new += len;
            }
            similar::ChangeTag::Equal => {
                cur_old += len;
                cur_new += len;
            }
        }
    }
    (old_spans, new_spans)
}

fn highlight_line_ansi(
    line: &str,
    highlighter: &mut HighlightLines,
    line_bg_ansi: Option<&str>,
) -> String {
    let ranges = highlighter.highlight_line(line, &SYNTAX_SET).unwrap_or_default();
    let mut out = String::new();

    if let Some(bg) = line_bg_ansi {
        out.push_str(bg);
    }

    append_highlighted_tokens(&mut out, &ranges);

    out.push_str("\x1b[0m");
    out
}

fn highlight_line_ansi_fixed(
    line: &str,
    highlighter: &mut HighlightLines,
    line_bg_ansi: Option<&str>,
    target_w: usize,
) -> String {
    let clean = line.replace('\t', "    ");
    let (truncated, char_count) = if clean.chars().count() > target_w {
        let s: String = clean.chars().take(target_w.saturating_sub(1)).chain(['…']).collect();
        let c = s.chars().count();
        (s, c)
    } else {
        let c = clean.chars().count();
        (clean, c)
    };

    let ranges = highlighter.highlight_line(&truncated, &SYNTAX_SET).unwrap_or_default();
    let mut out = String::new();

    if let Some(bg) = line_bg_ansi {
        out.push_str(bg);
    }

    append_highlighted_tokens(&mut out, &ranges);

    if char_count < target_w {
        out.push_str(&" ".repeat(target_w - char_count));
    }

    out.push_str("\x1b[0m");
    out
}

fn render_syntax_with_badges(
    text: &str,
    changed_spans: &[(usize, usize)],
    ranges: &[(syntect::highlighting::Style, &str)],
    badge_bg: &str,
    line_bg: &str,
) -> String {
    let mut out = String::from(line_bg);
    let mut token_offset = 0;

    for (style, token) in ranges {
        let t_start = token_offset;
        let t_end = token_offset + token.len();
        token_offset = t_end;

        let (r, g, b) = (style.foreground.r, style.foreground.g, style.foreground.b);
        let syntax_prefix = format!("\x1b[38;2;{};{};{}m", r, g, b);

        let mut sub_start = t_start;
        for &(s_start, s_end) in changed_spans {
            if s_end <= t_start || s_start >= t_end {
                continue;
            }
            if s_start > sub_start {
                let chunk = &text[sub_start..s_start];
                out.push_str(&syntax_prefix);
                out.push_str(chunk);
                sub_start = s_start;
            }
            let badge_from = sub_start.max(s_start);
            let badge_to = t_end.min(s_end);
            if badge_to > badge_from {
                let badge_chunk = &text[badge_from..badge_to];
                out.push_str(badge_bg);
                out.push_str(badge_chunk);
                out.push_str("\x1b[0m");
                out.push_str(line_bg);
                sub_start = badge_to;
            }
        }

        if sub_start < t_end {
            let chunk = &text[sub_start..t_end];
            out.push_str(&syntax_prefix);
            out.push_str(chunk);
        }
    }

    out
}

fn format_word_diff_syntax(
    old_line: &str,
    new_line: &str,
    highlighter: &mut HighlightLines,
) -> (String, String) {
    let word_diff = similar::TextDiff::from_words(old_line, new_line);

    if word_diff.ratio() < 0.25 {
        return (
            highlight_line_ansi(old_line, highlighter, Some("\x1b[41m")),
            highlight_line_ansi(new_line, highlighter, Some("\x1b[42m")),
        );
    }

    let (old_spans, new_spans) = compute_word_diff_spans(&word_diff);

    let old_ranges = highlighter.highlight_line(old_line, &SYNTAX_SET).unwrap_or_default();
    let mut hl_old = render_syntax_with_badges(
        old_line,
        &old_spans,
        &old_ranges,
        "\x1b[1;97;101m",
        "\x1b[41m",
    );
    hl_old.push_str("\x1b[0m");

    let new_ranges = highlighter.highlight_line(new_line, &SYNTAX_SET).unwrap_or_default();
    let mut hl_new = render_syntax_with_badges(
        new_line,
        &new_spans,
        &new_ranges,
        "\x1b[1;97;102m",
        "\x1b[42m",
    );
    hl_new.push_str("\x1b[0m");

    (hl_old, hl_new)
}

fn format_word_diff_split(
    old_line: &str,
    new_line: &str,
    highlighter_lhs: &mut HighlightLines,
    highlighter_rhs: &mut HighlightLines,
    target_w: usize,
) -> (String, String) {
    let old_clean = old_line.replace('\t', "    ");
    let new_clean = new_line.replace('\t', "    ");

    let (old_trunc, old_char_count) = if old_clean.chars().count() > target_w {
        let s: String = old_clean.chars().take(target_w.saturating_sub(1)).chain(['…']).collect();
        let c = s.chars().count();
        (s, c)
    } else {
        let c = old_clean.chars().count();
        (old_clean, c)
    };

    let (new_trunc, new_char_count) = if new_clean.chars().count() > target_w {
        let s: String = new_clean.chars().take(target_w.saturating_sub(1)).chain(['…']).collect();
        let c = s.chars().count();
        (s, c)
    } else {
        let c = new_clean.chars().count();
        (new_clean, c)
    };

    let word_diff = similar::TextDiff::from_words(&old_trunc, &new_trunc);

    if word_diff.ratio() < 0.25 {
        return (
            highlight_line_ansi_fixed(&old_trunc, highlighter_lhs, Some("\x1b[41m"), target_w),
            highlight_line_ansi_fixed(&new_trunc, highlighter_rhs, Some("\x1b[42m"), target_w),
        );
    }

    let (old_spans, new_spans) = compute_word_diff_spans(&word_diff);

    let old_ranges = highlighter_lhs.highlight_line(&old_trunc, &SYNTAX_SET).unwrap_or_default();
    let mut hl_old = render_syntax_with_badges(
        &old_trunc,
        &old_spans,
        &old_ranges,
        "\x1b[1;97;101m",
        "\x1b[41m",
    );
    if old_char_count < target_w {
        hl_old.push_str(&" ".repeat(target_w - old_char_count));
    }
    hl_old.push_str("\x1b[0m");

    let new_ranges = highlighter_rhs.highlight_line(&new_trunc, &SYNTAX_SET).unwrap_or_default();
    let mut hl_new = render_syntax_with_badges(
        &new_trunc,
        &new_spans,
        &new_ranges,
        "\x1b[1;97;102m",
        "\x1b[42m",
    );
    if new_char_count < target_w {
        hl_new.push_str(&" ".repeat(target_w - new_char_count));
    }
    hl_new.push_str("\x1b[0m");

    (hl_old, hl_new)
}

pub fn generate_split_diff(
    lhs: &DiffTarget,
    rhs: &DiffTarget,
    width: usize,
) -> DiffResult {
    let old_lines: Vec<&str> = lhs.content().lines().collect();
    let new_lines: Vec<&str> = rhs.content().lines().collect();

    let ext_hint_lhs = lhs.ext_hint().or_else(|| rhs.ext_hint());
    let ext_hint_rhs = rhs.ext_hint().or_else(|| lhs.ext_hint());

    let lang_id_lhs = little_clip_core::syntax::detect_language(lhs.content(), ext_hint_lhs);
    let lang_id_rhs = little_clip_core::syntax::detect_language(rhs.content(), ext_hint_rhs);

    let lang_name_lhs = little_clip_core::syntax::language_display_name(lang_id_lhs);
    let lang_name_rhs = little_clip_core::syntax::language_display_name(lang_id_rhs);

    let display_lang = if lang_name_lhs == lang_name_rhs {
        lang_name_lhs.to_string()
    } else {
        format!("{}/{}", lang_name_lhs, lang_name_rhs)
    };

    let diff = similar::TextDiff::from_lines(lhs.content(), rhs.content());
    let groups = diff.grouped_ops(3);

    if groups.is_empty() {
        return DiffResult {
            engine_name: format!("Split Diff ({})", display_lang),
            raw_ansi: "\x1b[2m--- No changes (Contents are identical) ---\x1b[0m\n".to_string(),
            is_identical: true,
            mode: DiffDisplayMode::SideBySide,
        };
    }

    let syntax_lhs = SYNTAX_SET
        .find_syntax_by_extension(lang_id_lhs)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang_id_lhs))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let syntax_rhs = SYNTAX_SET
        .find_syntax_by_extension(lang_id_rhs)
        .or_else(|| SYNTAX_SET.find_syntax_by_token(lang_id_rhs))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = THEME_SET
        .themes
        .get("base16-ocean.dark")
        .or_else(|| THEME_SET.themes.values().next())
        .expect("default theme");

    let mut highlighter_lhs = HighlightLines::new(syntax_lhs, theme);
    let mut highlighter_rhs = HighlightLines::new(syntax_rhs, theme);

    // Calculate column widths
    // Each row has format: <LHS: col_w> │ <RHS: col_w>
    // Inside each column: <num_w> + 3 (gutter) + <code_w> = col_w
    let target_total_w = width.max(100);
    let col_w = ((target_total_w.saturating_sub(3)) / 2).max(45);

    let max_line = old_lines.len().max(new_lines.len());
    let num_w = max_line.to_string().len().max(3);
    let code_w = col_w.saturating_sub(num_w + 3);

    let mut out = String::new();

    // Clean, readable split headers with language badge
    let lhs_hdr = format!("LHS: {} [{}]", lhs.label(), lang_name_lhs);
    let rhs_hdr = format!("RHS: {} [{}]", rhs.label(), lang_name_rhs);

    let lhs_hdr_trunc: String = if lhs_hdr.chars().count() > col_w {
        lhs_hdr.chars().take(col_w.saturating_sub(1)).chain(['…']).collect()
    } else {
        lhs_hdr
    };
    let rhs_hdr_trunc: String = if rhs_hdr.chars().count() > col_w {
        rhs_hdr.chars().take(col_w.saturating_sub(1)).chain(['…']).collect()
    } else {
        rhs_hdr
    };

    out.push_str(&format!(
        "\x1b[1;91m{:<col_w$}\x1b[0m │ \x1b[1;92m{:<col_w$}\x1b[0m\n",
        lhs_hdr_trunc,
        rhs_hdr_trunc,
        col_w = col_w
    ));

    out.push_str(&format!(
        "\x1b[2;90m{0:─<col_w$}─┼─{0:─<col_w$}\x1b[0m\n",
        "",
        col_w = col_w
    ));

    for group in &groups {
        if let Some(header) = format_hunk_header(group) {
            out.push_str(&header);
        } else {
            continue;
        }

        for op in group {
            match op {
                similar::DiffOp::Equal { old_index, new_index, len } => {
                    for offset in 0..*len {
                        let o_num = old_index + offset + 1;
                        let n_num = new_index + offset + 1;
                        let old_line = old_lines.get(old_index + offset).copied().unwrap_or("");
                        let new_line = new_lines.get(new_index + offset).copied().unwrap_or("");
                        let hl_lhs = highlight_line_ansi_fixed(old_line, &mut highlighter_lhs, None, code_w);
                        let hl_rhs = highlight_line_ansi_fixed(new_line, &mut highlighter_rhs, None, code_w);

                        out.push_str(&format!(
                            "\x1b[2;90m{:>w$}\x1b[0m   {} │ \x1b[2;90m{:>w$}\x1b[0m   {}\n",
                            o_num,
                            hl_lhs,
                            n_num,
                            hl_rhs,
                            w = num_w
                        ));
                    }
                }
                similar::DiffOp::Delete { old_index, old_len, .. } => {
                    for offset in 0..*old_len {
                        let o_num = old_index + offset + 1;
                        let old_line = old_lines.get(old_index + offset).copied().unwrap_or("");
                        let hl = highlight_line_ansi_fixed(old_line, &mut highlighter_lhs, Some("\x1b[41m"), code_w);
                        let blank = " ".repeat(code_w);

                        out.push_str(&format!(
                            "\x1b[91m{:>w$}\x1b[0m \x1b[91m-\x1b[0m {} │ {blank_num:>w$}   {}\n",
                            o_num,
                            hl,
                            blank,
                            blank_num = "",
                            w = num_w
                        ));
                    }
                }
                similar::DiffOp::Insert { new_index, new_len, .. } => {
                    for offset in 0..*new_len {
                        let n_num = new_index + offset + 1;
                        let new_line = new_lines.get(new_index + offset).copied().unwrap_or("");
                        let hl = highlight_line_ansi_fixed(new_line, &mut highlighter_rhs, Some("\x1b[42m"), code_w);
                        let blank = " ".repeat(code_w);

                        out.push_str(&format!(
                            "{blank_num:>w$}   {} │ \x1b[92m{:>w$}\x1b[0m \x1b[92m+\x1b[0m {}\n",
                            blank,
                            n_num,
                            hl,
                            blank_num = "",
                            w = num_w
                        ));
                    }
                }
                similar::DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                    let max_len = (*old_len).max(*new_len);
                    for offset in 0..max_len {
                        let has_old = offset < *old_len;
                        let has_new = offset < *new_len;

                        let (code_lhs, code_rhs) = if has_old && has_new {
                            let old_line = old_lines.get(old_index + offset).copied().unwrap_or("");
                            let new_line = new_lines.get(new_index + offset).copied().unwrap_or("");
                            format_word_diff_split(old_line, new_line, &mut highlighter_lhs, &mut highlighter_rhs, code_w)
                        } else if has_old {
                            let old_line = old_lines.get(old_index + offset).copied().unwrap_or("");
                            let hl = highlight_line_ansi_fixed(old_line, &mut highlighter_lhs, Some("\x1b[41m"), code_w);
                            let blank = " ".repeat(code_w);
                            (hl, blank)
                        } else {
                            let new_line = new_lines.get(new_index + offset).copied().unwrap_or("");
                            let hl = highlight_line_ansi_fixed(new_line, &mut highlighter_rhs, Some("\x1b[42m"), code_w);
                            let blank = " ".repeat(code_w);
                            (blank, hl)
                        };

                        // LHS gutter
                        if has_old {
                            let o_num = old_index + offset + 1;
                            out.push_str(&format!("\x1b[91m{:>w$}\x1b[0m \x1b[91m-\x1b[0m {}", o_num, code_lhs, w = num_w));
                        } else {
                            out.push_str(&format!("{0:>w$}   {1}", "", code_lhs, w = num_w));
                        }

                        out.push_str(" │ ");

                        // RHS gutter
                        if has_new {
                            let n_num = new_index + offset + 1;
                            out.push_str(&format!("\x1b[92m{:>w$}\x1b[0m \x1b[92m+\x1b[0m {}\n", n_num, code_rhs, w = num_w));
                        } else {
                            out.push_str(&format!("{0:>w$}   {1}\n", "", code_rhs, w = num_w));
                        }
                    }
                }
            }
        }
    }

    DiffResult {
        engine_name: format!("Split Diff ({})", display_lang),
        raw_ansi: out,
        is_identical: false,
        mode: DiffDisplayMode::SideBySide,
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guess_extension() {
        assert_eq!(guess_extension_from_content("{\"key\": 123}"), Some("json"));
        assert_eq!(guess_extension_from_content("fn main() { println!(); }"), Some("rs"));
        assert_eq!(guess_extension_from_content("def foo():\n    pass"), Some("py"));
        assert_eq!(guess_extension_from_content("plain text with no keywords"), None);
    }

    #[test]
    fn test_diff_target_line_count() {
        let target = DiffTarget::ClipboardText {
            label: "Test".to_string(),
            content: "line1\nline2\nline3".to_string(),
            ext_hint: None,
        };
        assert_eq!(target.line_count(), 3);
        assert!(!target.is_file());
    }

    #[test]
    fn test_compute_diff_identical() {
        let t1 = DiffTarget::ClipboardText {
            label: "A".to_string(),
            content: "hello world".to_string(),
            ext_hint: None,
        };
        let t2 = DiffTarget::ClipboardText {
            label: "B".to_string(),
            content: "hello world".to_string(),
            ext_hint: None,
        };
        let res = compute_diff(&t1, &t2, DiffDisplayMode::Inline, 100);
        assert!(res.is_identical);
    }

    #[test]
    fn test_compute_diff_with_difft_or_fallback() {
        let t1 = DiffTarget::ClipboardText {
            label: "LHS".to_string(),
            content: "fn test() { let a = 1; }".to_string(),
            ext_hint: Some("rs".to_string()),
        };
        let t2 = DiffTarget::ClipboardText {
            label: "RHS".to_string(),
            content: "fn test() { let a = 2; }".to_string(),
            ext_hint: Some("rs".to_string()),
        };
        let res = compute_diff(&t1, &t2, DiffDisplayMode::SideBySide, 120);
        assert!(!res.is_identical);
        assert!(!res.raw_ansi.is_empty());
    }

    #[test]
    fn test_unified_inline_diff_formatting() {
        let t1 = DiffTarget::ClipboardText {
            label: "Left Hand".to_string(),
            content: "fn hello() {\n    println!(\"Hello Universe\");\n}\n".to_string(),
            ext_hint: Some("rs".to_string()),
        };
        let t2 = DiffTarget::ClipboardText {
            label: "Right Hand".to_string(),
            content: "fn hello() {\n    println!(\"Hello World\");\n}\n".to_string(),
            ext_hint: Some("rs".to_string()),
        };

        let res = compute_diff(&t1, &t2, DiffDisplayMode::Inline, 120);
        assert_eq!(res.mode, DiffDisplayMode::Inline);
        assert!(!res.is_identical);

        println!("\n=== RENDERED INLINE DIFF ===\n{}", res.raw_ansi);

        // Verify clean header without any /tmp paths

        assert!(res.raw_ansi.contains("--- LHS: Left Hand"));
        assert!(res.raw_ansi.contains("+++ RHS: Right Hand"));
        assert!(!res.raw_ansi.contains("/tmp/"));

        // Verify gutter with dual line numbers, divider, and +/- markers
        assert!(res.raw_ansi.contains("│"));
        assert!(res.raw_ansi.contains("- "));
        assert!(res.raw_ansi.contains("+ "));

        // Verify word-level diff highlight (Universe and World)
        assert!(res.raw_ansi.contains("Universe"));
        assert!(res.raw_ansi.contains("World"));
    }

    #[test]
    fn test_diff_user_screenshot_case() {
        let t1 = DiffTarget::ClipboardText {
            label: "Left Hand".to_string(),
            content: "fn hello() {\n-     println!(\"Hello Universe\");\n+     println!(\"Hello World\");\n}\n".to_string(),
            ext_hint: Some("rs".to_string()),
        };
        let t2 = DiffTarget::ClipboardText {
            label: "Right Hand".to_string(),
            content: "fn hello() {\n    println!(\"Hello Universe\");\n}\n".to_string(),
            ext_hint: Some("rs".to_string()),
        };

        let res = compute_diff(&t1, &t2, DiffDisplayMode::Inline, 120);
        println!("\n=== USER SCREENSHOT CASE INLINE DIFF ===\n{}", res.raw_ansi);
        assert!(!res.raw_ansi.contains("/tmp/"));
        assert!(res.raw_ansi.contains("--- LHS: Left Hand"));
        assert!(res.raw_ansi.contains("+++ RHS: Right Hand"));
    }

    #[test]
    fn test_diff_syntax_highlighted_python() {
        let t1 = DiffTarget::ClipboardText {
            label: "Left Hand".to_string(),
            content: "def calculate(x):\n    print('Calculating...')\n    return x * 2\n".to_string(),
            ext_hint: None, // No extension hint, must infer Python!
        };
        let t2 = DiffTarget::ClipboardText {
            label: "Right Hand".to_string(),
            content: "def calculate(x):\n    print('Processing...')\n    return x * 3\n".to_string(),
            ext_hint: None, // No extension hint, must infer Python!
        };

        let res = compute_diff(&t1, &t2, DiffDisplayMode::Inline, 120);
        println!("\n=== PYTHON SYNTAX HIGHLIGHTED DIFF ===\n{}", res.raw_ansi);
        assert_eq!(res.engine_name, "Unified Diff (Python)");
        assert!(res.raw_ansi.contains("[Python]"));
        assert!(res.raw_ansi.contains("\x1b[38;2;")); // Contains 24-bit TrueColor syntax highlighting!
    }

    #[test]
    fn test_split_diff_syntax_highlighted_rust() {
        let t1 = DiffTarget::ClipboardText {
            label: "Left Hand".to_string(),
            content: "fn calculate(x: i32) -> i32 {\n    let y = 10;\n    println!(\"Hello Universe\");\n    x + y\n}\n".to_string(),
            ext_hint: Some("rs".to_string()),
        };
        let t2 = DiffTarget::ClipboardText {
            label: "Right Hand".to_string(),
            content: "fn calculate(x: i32) -> i32 {\n    let y = 20;\n    println!(\"Hello World\");\n    x * y\n}\n".to_string(),
            ext_hint: Some("rs".to_string()),
        };

        let res = compute_diff(&t1, &t2, DiffDisplayMode::SideBySide, 140);
        println!("\n=== RENDERED SPLIT DIFF ===\n{}", res.raw_ansi);

        assert_eq!(res.mode, DiffDisplayMode::SideBySide);
        assert_eq!(res.engine_name, "Split Diff (Rust)");
        assert!(!res.is_identical);

        // Verify language badge and header
        assert!(res.raw_ansi.contains("[Rust]"));
        assert!(res.raw_ansi.contains("LHS: Left Hand"));
        assert!(res.raw_ansi.contains("RHS: Right Hand"));

        // Verify divider and separator
        assert!(res.raw_ansi.contains(" │ "));
        assert!(res.raw_ansi.contains("─┼─"));

        // Verify TrueColor syntax highlighting is present in split diff
        assert!(res.raw_ansi.contains("\x1b[38;2;"));

        // Verify word diff badges are present
        assert!(res.raw_ansi.contains("\x1b[1;97;101m")); // Red word badge (LHS)
        assert!(res.raw_ansi.contains("\x1b[1;97;102m")); // Green word badge (RHS)
    }

    #[test]
    fn test_split_diff_column_alignment() {
        let t1 = DiffTarget::ClipboardText {
            label: "Left Hand".to_string(),
            content: "fn calculate(x: i32) -> i32 {\n    let y = 10;\n    println!(\"Short\");\n    x + y\n}\n".to_string(),
            ext_hint: None, // Infer language automatically
        };
        let t2 = DiffTarget::ClipboardText {
            label: "Right Hand".to_string(),
            content: "fn calculate(x: i32) -> i32 {\n    let y = 200000000;\n    println!(\"A very long replacement string that tests padding\");\n    x * y\n}\n".to_string(),
            ext_hint: None, // Infer language automatically
        };

        let res = compute_diff(&t1, &t2, DiffDisplayMode::SideBySide, 140);
        println!("\n=== ALIGNED SPLIT DIFF ===\n{}", res.raw_ansi);

        assert_eq!(res.engine_name, "Split Diff (Rust)");

        // Strip ANSI escape sequences and check visible position of divider
        fn strip_ansi(s: &str) -> String {
            let mut out = String::new();
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\x1b' {
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        for nc in chars.by_ref() {
                            if nc == 'm' || nc.is_alphabetic() {
                                break;
                            }
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }

        let mut expected_divider_pos: Option<usize> = None;
        for line in res.raw_ansi.lines() {
            let plain = strip_ansi(line);
            if let Some(pos) = plain.find("│") {
                if let Some(expected) = expected_divider_pos {
                    assert_eq!(
                        pos, expected,
                        "Divider misaligned on line: {:?} (expected pos {}, got {})",
                        plain, expected, pos
                    );
                } else {
                    expected_divider_pos = Some(pos);
                }
            }
        }
        assert!(expected_divider_pos.is_some(), "Expected divider to be found");
    }

    #[test]
    fn test_resolve_diff_pair_single_slot_images() {
        use little_clip_core::ClipItem;
        let temp_dir = std::env::temp_dir().join("little_clip_test_diff_pair");
        let _ = std::fs::create_dir_all(&temp_dir);
        let img1_path = temp_dir.join("test1.png");
        let img2_path = temp_dir.join("test2.png");

        // 1x1 transparent PNG bytes
        let png_bytes = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&img1_path, &png_bytes).unwrap();
        std::fs::write(&img2_path, &png_bytes).unwrap();

        // 1. Test FileBatch with 2 image paths
        let batch_item = ClipItem::from_files(vec![img1_path.clone(), img2_path.clone()]);
        let pair = resolve_diff_pair(&batch_item);
        assert!(pair.is_some(), "FileBatch with 2 images should resolve to diff pair");
        let (t1, t2) = pair.unwrap();
        assert!(t1.is_image(), "Target 1 must be image");
        assert!(t2.is_image(), "Target 2 must be image");

        // 2. Test MultiImage with 2 paths
        let multi_item = ClipItem::from_multi_image(vec![img1_path.clone(), img2_path.clone()]);
        let pair_multi = resolve_diff_pair(&multi_item);
        assert!(pair_multi.is_some(), "MultiImage with 2 images should resolve to diff pair");

        // 3. Test Text with 2 file paths
        let text_content = format!("{}\n{}", img1_path.display(), img2_path.display());
        let text_item = ClipItem::from_text(text_content);
        let pair_text = resolve_diff_pair(&text_item);
        assert!(pair_text.is_some(), "Text with 2 image paths should resolve to diff pair");

        // Check get_applicable_ops for single item with 2 images
        let ops = crate::get_applicable_ops(&[&batch_item]);
        assert!(ops.iter().any(|op| op.name == "diff" && !op.is_binary), "Applicable ops should include diff op for single item containing 2 images");

        // Cleanup
        let _ = std::fs::remove_file(&img1_path);
        let _ = std::fs::remove_file(&img2_path);
        let _ = std::fs::remove_dir(&temp_dir);
    }
}



