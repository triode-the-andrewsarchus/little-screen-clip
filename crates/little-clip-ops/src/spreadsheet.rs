//! Spreadsheet parsing and clipboard table detection.
//!
//! Provides parsing for delimiter-separated text (CSV, TSV, semicolon-delimited), heuristic
//! tabular clipboard detection that guards against prose punctuation false-positives, and workbook
//! loading for Excel / OpenDocument formats (`.xlsx`, `.xls`, `.xlsb`, `.ods`) using `calamine`.

use calamine::{open_workbook_auto, Reader};
use std::path::Path;

/// A single tabular sheet containing rows and columns of cell string values.
#[derive(Debug, Clone, PartialEq)]
pub struct SpreadsheetSheet {
    /// Sheet name or identifier.
    pub name: String,
    /// Total row count.
    pub total_rows: usize,
    /// Total column count.
    pub total_cols: usize,
    /// 2D grid of cell values.
    pub rows: Vec<Vec<String>>,
}

/// A parsed workbook or tabular dataset containing one or more sheets.
#[derive(Debug, Clone, PartialEq)]
pub struct SpreadsheetData {
    /// Workbook or file title.
    pub title: String,
    /// Sheets contained within the workbook.
    pub sheets: Vec<SpreadsheetSheet>,
}

impl SpreadsheetSheet {
    /// Constructs a new `SpreadsheetSheet` and calculates its dimensions.
    pub fn new(name: String, rows: Vec<Vec<String>>) -> Self {
        let total_rows = rows.len();
        let total_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        Self {
            name,
            total_rows,
            total_cols,
            rows,
        }
    }

    /// Returns a sliced 2D grid of the upper-left cells (e.g. top 8 rows, first 5 cols).
    pub fn upper_left_slice(&self, max_rows: usize, max_cols: usize) -> Vec<Vec<String>> {
        let r_limit = self.rows.len().min(max_rows);
        let mut slice = Vec::with_capacity(r_limit);

        for r in 0..r_limit {
            let row = &self.rows[r];
            let c_limit = row.len().min(max_cols);
            let mut row_slice = Vec::with_capacity(c_limit);
            for c in 0..c_limit {
                row_slice.push(row[c].clone());
            }
            slice.push(row_slice);
        }
        slice
    }
}

/// Parses a delimiter-separated text string (auto-detecting comma, tab, or semicolon delimiters) into rows.
pub fn parse_csv_string(csv_text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    // Detect delimiter: comma, tab, or semicolon
    let first_few_lines: Vec<&str> = csv_text.lines().take(5).collect();
    if first_few_lines.is_empty() {
        return rows;
    }

    let comma_count: usize = first_few_lines.iter().map(|l| l.matches(',').count()).sum();
    let tab_count: usize = first_few_lines.iter().map(|l| l.matches('\t').count()).sum();
    let semi_count: usize = first_few_lines.iter().map(|l| l.matches(';').count()).sum();

    let delimiter = if tab_count > comma_count && tab_count > semi_count {
        '\t'
    } else if semi_count > comma_count && semi_count > tab_count {
        ';'
    } else {
        ','
    };

    for line in csv_text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }

        let mut row = Vec::new();
        let mut in_quotes = false;
        let mut cur_field = String::new();

        let chars: Vec<char> = trimmed.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '"' {
                if in_quotes && i + 1 < chars.len() && chars[i + 1] == '"' {
                    cur_field.push('"');
                    i += 1;
                } else {
                    in_quotes = !in_quotes;
                }
            } else if c == delimiter && !in_quotes {
                row.push(cur_field.trim().to_string());
                cur_field.clear();
            } else {
                cur_field.push(c);
            }
            i += 1;
        }
        row.push(cur_field.trim().to_string());
        rows.push(row);
    }

    rows
}

/// Detects if raw clipboard text is genuinely a formatted CSV / TSV table
pub fn detect_csv_text(text: &str) -> Option<SpreadsheetData> {
    let trimmed = text.trim();
    let lines: Vec<&str> = trimmed.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.len() < 2 {
        return None;
    }

    let sample = &lines[..lines.len().min(5)];
    let comma_count: usize = sample.iter().map(|l| l.matches(',').count()).sum();
    let tab_count: usize = sample.iter().map(|l| l.matches('\t').count()).sum();
    let semi_count: usize = sample.iter().map(|l| l.matches(';').count()).sum();

    // Must have at least 1 delimiter per line on average
    if comma_count < sample.len() && tab_count < sample.len() && semi_count < sample.len() {
        return None;
    }

    let delimiter = if tab_count >= comma_count && tab_count >= semi_count {
        '\t'
    } else if semi_count >= comma_count && semi_count >= tab_count {
        ';'
    } else {
        ','
    };

    // If delimiter is comma, guard against English prose punctuation
    if delimiter == ',' {
        // If lines end with sentence punctuation ('.', '!', '?'), it's prose sentences
        let sentences_ending_with_period = sample
            .iter()
            .filter(|l| l.ends_with('.') || l.ends_with('!') || l.ends_with('?'))
            .count();
        if sentences_ending_with_period >= 2 {
            return None;
        }

        // If all commas in sample have spaces after them, check if lines look like long prose
        let space_after_commas: usize = sample.iter().map(|l| l.matches(", ").count()).sum();
        if comma_count > 0 && space_after_commas == comma_count {
            let total_words: usize = sample.iter().map(|l| l.split_whitespace().count()).sum();
            if total_words / sample.len() > 6 {
                return None;
            }
        }
    }

    let rows = parse_csv_string(trimmed);
    if rows.len() < 2 {
        return None;
    }

    let expected_cols = rows[0].len();
    if expected_cols < 2 {
        return None;
    }

    // For 2-column comma data, require at least 3 rows to avoid accidental 2-line prose matches
    if delimiter == ',' && expected_cols == 2 && rows.len() < 3 {
        return None;
    }

    // Consistency check: all inspected rows must have the exact same number of columns
    let check_rows = &rows[..rows.len().min(5)];
    if !check_rows.iter().all(|r| r.len() == expected_cols) {
        return None;
    }

    // Cells in header shouldn't be overly long prose paragraphs
    if rows[0].iter().any(|c| c.len() > 50) {
        return None;
    }

    let sheet = SpreadsheetSheet::new("Data".to_string(), rows);
    Some(SpreadsheetData {
        title: "Clipboard CSV".to_string(),
        sheets: vec![sheet],
    })
}

/// Loads a spreadsheet from a file path (.csv, .tsv, .xlsx, .xls, .xlsb, .ods)
pub fn load_spreadsheet(path: &Path) -> Result<SpreadsheetData, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Spreadsheet".to_string());

    if ext == "csv" || ext == "tsv" {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let rows = parse_csv_string(&content);
        let sheet = SpreadsheetSheet::new("Sheet1".to_string(), rows);
        return Ok(SpreadsheetData {
            title,
            sheets: vec![sheet],
        });
    }

    if ext == "xlsx" || ext == "xls" || ext == "xlsb" || ext == "ods" {
        let mut workbook = open_workbook_auto(path).map_err(|e| format!("Failed to open workbook: {}", e))?;
        let sheet_names = workbook.sheet_names();
        let mut sheets = Vec::new();

        for name in sheet_names {
            if let Ok(range) = workbook.worksheet_range(&name) {
                let mut rows = Vec::new();
                for r in range.rows() {
                    let row_str: Vec<String> = r.iter().map(|cell| cell.to_string().trim().to_string()).collect();
                    rows.push(row_str);
                }
                sheets.push(SpreadsheetSheet::new(name, rows));
            }
        }

        if sheets.is_empty() {
            return Err("No sheets found in workbook".to_string());
        }

        return Ok(SpreadsheetData { title, sheets });
    }

    Err(format!("Unsupported spreadsheet format: .{}", ext))
}

/// Returns `true` if the given path has a supported spreadsheet file extension.
pub fn is_spreadsheet_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(ext.as_str(), "csv" | "tsv" | "xlsx" | "xls" | "xlsb" | "ods")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csv_string() {
        let data = "name,age,city\nAlice,30,New York\nBob,25,\"San Francisco, CA\"\n";
        let rows = parse_csv_string(data);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec!["name", "age", "city"]);
        assert_eq!(rows[1], vec!["Alice", "30", "New York"]);
        assert_eq!(rows[2], vec!["Bob", "25", "San Francisco, CA"]);
    }

    #[test]
    fn test_detect_csv_text() {
        let data = "col1,col2,col3\n1,2,3\n4,5,6\n";
        let res = detect_csv_text(data);
        assert!(res.is_some());
        let sheet = &res.unwrap().sheets[0];
        assert_eq!(sheet.total_rows, 3);
        assert_eq!(sheet.total_cols, 3);
        let slice = sheet.upper_left_slice(2, 2);
        assert_eq!(slice.len(), 2);
        assert_eq!(slice[0].len(), 2);
    }

    #[test]
    fn test_detect_csv_text_rejects_prose() {
        // Two sentences with commas should NOT be recognized as CSV
        let prose = "Hello, my friend.\nHow are you, today?";
        assert!(detect_csv_text(prose).is_none());

        // Multi-line prose with punctuation
        let prose2 = "First, install dependencies.\nSecond, run the build.\nFinally, test.";
        assert!(detect_csv_text(prose2).is_none());

        // Plain text without delimiters
        let plain = "Just a line of text\nAnd another line without commas";
        assert!(detect_csv_text(plain).is_none());
    }

    #[test]
    fn test_detect_tsv() {
        let tsv = "Name\tAge\tCity\nAlice\t30\tNYC\nBob\t25\tSF";
        let res = detect_csv_text(tsv);
        assert!(res.is_some());
        let sheet = &res.unwrap().sheets[0];
        assert_eq!(sheet.total_rows, 3);
        assert_eq!(sheet.total_cols, 3);
    }
}
