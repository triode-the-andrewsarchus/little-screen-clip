use super::{
    BINS, Bin, COLORS, COLUMNS, Document, EXCERPT_COLUMNS, EXCERPT_LINES, ExcerptLine,
    stream::{self, Lines},
};
use crate::{
    file_source::FileSource,
    text_preview::{SYNTAX_SET, THEME_SET},
};
use std::{
    io::{BufReader, Seek, SeekFrom},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use syntect::{easy::HighlightLines, highlighting::Color};
use unicode_width::UnicodeWidthChar;

/// Resolve extensions shared by TypeScript and transport-stream video on workers.
pub(in crate::file_manager) fn is_typescript_source(path: &Path) -> bool {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ts" | "mts" | "cts"
            )
        })
    {
        return false;
    }
    let Ok(bytes) = crate::file_source::read_file_bytes(path, 8192, false, true) else {
        return false;
    };
    if bytes
        .iter()
        .any(|byte| *byte < 32 && !matches!(*byte, b'\n' | b'\r' | b'\t' | 12))
    {
        return false;
    }
    match std::str::from_utf8(&bytes) {
        Ok(_) => true,
        Err(error) if error.error_len().is_none() => true,
        Err(_) => {
            String::from_utf8_lossy(&bytes)
                .chars()
                .filter(|character| *character == '\u{fffd}')
                .count()
                <= bytes.len().div_ceil(100)
        }
    }
}

pub(in crate::file_manager) fn known_text(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    little_clip_core::syntax::is_code_file(path)
        || matches!(
            name.as_str(),
            "makefile"
                | "dockerfile"
                | "license"
                | "readme"
                | ".gitignore"
                | ".gitattributes"
                | ".editorconfig"
                | ".env"
        )
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "log" | "csv" | "tsv" | "nix" | "cmake" | "srt" | "vtt"
                )
            })
}

pub(in crate::file_manager) fn load(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Arc<Document>, String> {
    let source = FileSource::capture(path)?;
    let mut file = crate::file_source::open_regular_file(path, true).map_err(|e| e.to_string())?;
    let bytes = file.metadata().map_err(|e| e.to_string())?.len();
    if bytes > 256 * 1024 * 1024 {
        return Err("Structural preview limited to 256 MiB; open to inspect".into());
    }
    let shape = stream::scan(&mut file, cancelled).map_err(|e| e.to_string())?;
    if shape.lossy && !known_text(path) {
        return Err("Unrecognized non-UTF-8 file".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let neutral =
        bytes > 16 * 1024 * 1024 || shape.lines > 250_000 || shape.longest > stream::LINE_LIMIT;
    let (mut document, syntax) = prepare(path, &shape, neutral);
    summarize(&mut document, file, syntax, neutral, cancelled)?;
    source.check()?;
    Ok(Arc::new(document))
}

fn prepare(
    path: &Path,
    shape: &stream::Shape,
    neutral: bool,
) -> (Document, &'static syntect::parsing::SyntaxReference) {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or(filename.trim_start_matches('.'))
        .to_ascii_lowercase();
    let detected = little_clip_core::syntax::detect_language(&shape.prefix, Some(&extension));
    let syntax = SYNTAX_SET
        .find_syntax_by_extension(filename)
        .or_else(|| SYNTAX_SET.find_syntax_by_extension(&extension))
        .or_else(|| {
            SYNTAX_SET.find_syntax_by_first_line(
                shape
                    .prefix
                    .trim_start_matches('\u{feff}')
                    .lines()
                    .next()
                    .unwrap_or(""),
            )
        })
        .unwrap_or_else(|| crate::text_preview::find_syntax(detected));
    let bins = shape.lines.clamp(1, BINS as u64) as usize;
    let document = Document {
        lines: shape.lines,
        language: if syntax.name == "Configuration" {
            let language = little_clip_core::syntax::language_display_name(detected);
            if language == "Text" {
                syntax.name.clone()
            } else {
                language.into()
            }
        } else if syntax.name == "Plain Text" {
            "Text".into()
        } else {
            syntax.name.clone()
        },
        badge: if extension.len() <= 5 && SYNTAX_SET.find_syntax_by_extension(&extension).is_some()
        {
            extension.to_uppercase()
        } else {
            detected.to_uppercase()
        },
        note: match (neutral, shape.lossy) {
            (true, true) => "Monochrome overview · lossy UTF-8".into(),
            (true, false) => "Monochrome overview (large file or long lines)".into(),
            (false, true) => "Lossy UTF-8 preview".into(),
            _ => String::new(),
        },
        columns: COLUMNS,
        bins: (0..bins)
            .map(|_| Bin {
                start: 0,
                end: 0,
                cells: vec![[0; COLORS.len()]; COLUMNS],
                excerpt: Vec::new(),
            })
            .collect(),
    };
    (document, syntax)
}

fn summarize(
    document: &mut Document,
    file: std::fs::File,
    syntax: &'static syntect::parsing::SyntaxReference,
    neutral: bool,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let bins = document.bins.len();
    let mut highlighter = HighlightLines::new(syntax, &THEME_SET.themes["base16-eighties.dark"]);
    let mut reader = Lines::new(BufReader::with_capacity(64 * 1024, file));
    let mut number = 0;
    while let Some(line) = reader.next(cancelled).map_err(|e| e.to_string())? {
        if number >= document.lines {
            return Err("Source changed while reading".into());
        }
        let bin = &mut document.bins[(number * bins as u64 / document.lines.max(1)) as usize];
        if bin.end == 0 {
            bin.start = number;
        }
        bin.end = number + 1;
        let expanded = expand_tabs(&line);
        let tokens = if neutral {
            vec![(COLORS[0], expanded.as_str())]
        } else {
            highlighter
                .highlight_line(&expanded, &SYNTAX_SET)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|(style, text)| (style.foreground, text))
                .collect()
        };
        let keep_excerpt = bin.excerpt.len() < EXCERPT_LINES;
        let excerpt = add_line(bin, &tokens, number + 1, keep_excerpt);
        if keep_excerpt {
            bin.excerpt.push(excerpt);
        }
        number += 1;
    }
    if number != document.lines {
        return Err("Source changed while reading".into());
    }
    document.columns = document
        .bins
        .iter()
        .filter_map(|bin| {
            bin.cells
                .iter()
                .rposition(|cell| cell.iter().any(|count| *count > 0))
                .map(|column| column + 1)
        })
        .max()
        .unwrap_or(0)
        .clamp(16, COLUMNS);
    Ok(())
}

fn expand_tabs(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut column = 0;
    for character in line.trim_start_matches('\u{feff}').chars() {
        if character == '\t' {
            let count = 4 - column % 4;
            result.extend(std::iter::repeat_n(' ', count));
            column += count;
        } else {
            result.push(character);
            column += character.width().unwrap_or(0);
        }
    }
    result
}

fn category(color: Color) -> usize {
    COLORS
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| {
            let dr = color.r as i32 - candidate.r as i32;
            let dg = color.g as i32 - candidate.g as i32;
            let db = color.b as i32 - candidate.b as i32;
            dr * dr + dg * dg + db * db
        })
        .map_or(0, |(index, _)| index)
}

fn add_line(bin: &mut Bin, tokens: &[(Color, &str)], number: u64, keep: bool) -> ExcerptLine {
    let mut column = 0;
    let mut excerpt_characters = 0;
    let mut truncated = false;
    let mut runs = Vec::new();
    for &(color, token) in tokens {
        let category = category(color);
        let mut text = String::new();
        for character in token
            .chars()
            .filter(|character| !matches!(character, '\r' | '\n'))
        {
            let width = character.width().unwrap_or(0);
            if !character.is_whitespace() {
                for cell in bin
                    .cells
                    .iter_mut()
                    .take((column + width).min(COLUMNS))
                    .skip(column)
                {
                    cell[category] += 1;
                }
            }
            if keep {
                if column < EXCERPT_COLUMNS && excerpt_characters < EXCERPT_COLUMNS {
                    text.push(character);
                    excerpt_characters += 1;
                } else {
                    truncated = true;
                }
            }
            column += width;
        }
        if !text.is_empty() {
            runs.push((text, category));
        }
    }
    if keep && truncated {
        runs.push(("…".into(), 1));
    }
    ExcerptLine { number, runs }
}
