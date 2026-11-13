//! Tables, as `\addplot table` reads them (`pgfplotstable`'s parser): rows separated by newlines
//! (or `\\`), cells by spaces (or `col sep`), a first row of column names when it is not numeric.
//!
//! Files are read from the folders of `TIKRA_INPUT_DIR` (`:`-separated; the current directory if
//! unset), with `table/search path`; only relative names without `..` are accepted, and at most
//! [`MAX_FILE`] bytes.

use std::path::PathBuf;

/// Largest table file read.
pub const MAX_FILE: u64 = 4 << 20;

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub names: Vec<String>,
    /// Cells as text, row by row.
    pub rows: Vec<Vec<String>>,
    /// Row counts at which an empty line ended a scanline.
    pub scanlines: Vec<usize>,
}

/// How cells are separated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColSep {
    Space,
    Char(char),
}

pub fn col_sep(name: &str) -> ColSep {
    match name.trim() {
        "comma" => ColSep::Char(','),
        "semicolon" => ColSep::Char(';'),
        "colon" => ColSep::Char(':'),
        "tab" => ColSep::Char('\t'),
        "&" | "ampersand" => ColSep::Char('&'),
        "braces" => ColSep::Space,
        "|" => ColSep::Char('|'),
        _ => ColSep::Space,
    }
}

impl Table {
    /// Parses table text. `header`: `true` (names if the first row is not all numbers), `false`,
    /// or `has colnames`.
    pub fn parse(text: &str, sep: ColSep, header: &str, skip: usize, comment_chars: &str) -> Table {
        let mut lines: Vec<&str> = Vec::new();
        let mut breaks = Vec::new();
        for l in text.lines() {
            // '#' and '%' start comments, as pgfplotstable's default
            let cut = l.find(|c: char| c == '#' || c == '%' || comment_chars.contains(c)).unwrap_or(l.len());
            let l = &l[..cut];
            if l.trim().is_empty() {
                if !lines.is_empty() && breaks.last() != Some(&lines.len()) {
                    breaks.push(lines.len());
                }
                continue;
            }
            lines.push(l);
        }
        let lines: Vec<&str> = lines.into_iter().skip(skip).collect();
        let split = |l: &str| -> Vec<String> {
            match sep {
                ColSep::Space => split_space(l).into_iter().map(clean_cell).collect(),
                ColSep::Char(c) => l.split(c).map(clean_cell).collect(),
            }
        };
        let mut rows: Vec<Vec<String>> = lines.iter().map(|l| split(l)).collect();
        let first_is_names = match header.trim() {
            "false" => false,
            "has colnames" => true,
            _ => rows.first().is_some_and(|r| r.iter().any(|c| parse_number(c).is_none())),
        };
        let names = if first_is_names && !rows.is_empty() { rows.remove(0) } else { Vec::new() };
        let shift = skip + usize::from(first_is_names);
        let scanlines = breaks.into_iter().filter_map(|b| b.checked_sub(shift)).filter(|b| *b > 0).collect();
        Table { names, rows, scanlines }
    }

    pub fn columns(&self) -> usize {
        self.names.len().max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
    }

    /// The index of a column by name (or a number given as text).
    pub fn column(&self, name: &str) -> Option<usize> {
        let n = name.trim();
        self.names.iter().position(|c| c == n).or_else(|| {
            // pgfplotstable's aliases: [index]3
            n.strip_prefix("[index]").and_then(|i| i.trim().parse().ok())
        })
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&str> {
        self.rows.get(row).and_then(|r| r.get(col)).map(|s| s.as_str())
    }
}

/// Cells separated by white space; a `{…}` group is one cell, spaces and all (`{Palo Alto}`).
fn split_space(l: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (i, c) in l.char_indices() {
        match c {
            '{' => {
                depth += 1;
                start.get_or_insert(i);
            }
            '}' => depth = depth.saturating_sub(1),
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&l[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&l[s..]);
    }
    out
}

fn clean_cell(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')).unwrap_or(t);
    t.to_string()
}

/// A number as pgfplots' float parser reads it: `1.5`, `-2e3`, `nan`, `inf`.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    match t {
        "nan" | "NaN" | "NAN" => return Some(f64::NAN),
        "inf" | "+inf" | "INF" => return Some(f64::INFINITY),
        "-inf" | "-INF" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let t = t.strip_prefix('+').unwrap_or(t);
    if t.is_empty() || !t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') {
        return None;
    }
    t.parse::<f64>().ok()
}

/// Reads a table file, if it may be read: the folders searched and the reason it was refused.
pub fn read_file(name: &str, search: &[String]) -> Result<String, String> {
    let bytes = read_bytes(name, search, MAX_FILE)?;
    String::from_utf8(bytes).map_err(|e| format!("file '{}': {e}", name.trim()))
}

/// Reads a file of at most `max` bytes from the input folders (with the folders of `search` in
/// each), as [`read_file`] finds it.
pub fn read_bytes(name: &str, search: &[String], max: u64) -> Result<Vec<u8>, String> {
    let n = name.trim();
    let full = find_file(n, search)?;
    let meta = std::fs::metadata(&full).map_err(|e| format!("file '{n}': {e}"))?;
    if meta.len() > max {
        return Err(format!("file '{n}' is larger than {max} bytes"));
    }
    std::fs::read(&full).map_err(|e| format!("file '{n}': {e}"))
}

/// Where a file is: the first input folder (and folder of `search` in it) that has it.
pub fn find_file(name: &str, search: &[String]) -> Result<PathBuf, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("no file name".into());
    }
    let p = std::path::Path::new(n);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(format!("file '{n}' refused: only relative names inside the input folders are read"));
    }
    let roots: Vec<PathBuf> = match std::env::var("TIKRA_INPUT_DIR") {
        Ok(v) if !v.is_empty() => v.split(':').map(PathBuf::from).collect(),
        _ => vec![PathBuf::from(".")],
    };
    for root in &roots {
        for dir in search.iter().map(|s| s.trim()).chain(std::iter::once(".")) {
            if std::path::Path::new(dir).components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                continue;
            }
            let full = root.join(dir).join(n);
            if std::fs::metadata(&full).is_ok_and(|m| m.is_file()) {
                return Ok(full);
            }
        }
    }
    Err(format!("file '{n}' not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_rows() {
        let t = Table::parse("x y\n1 2\n3 4 % comment\n\n5 6", ColSep::Space, "true", 0, "");
        assert_eq!(t.names, vec!["x", "y"]);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.column("y"), Some(1));
        let t = Table::parse("1,2\n3,4", ColSep::Char(','), "true", 0, "");
        assert!(t.names.is_empty());
        assert_eq!(t.cell(1, 1), Some("4"));
    }

    #[test]
    fn refuses_escapes() {
        assert!(read_file("../secret", &[]).is_err());
        assert!(read_file("/etc/passwd", &[]).is_err());
    }
}
