//! Spreadsheet parsing and clipboard table detection.
//!
//! Provides parsing for delimiter-separated text (CSV, TSV, semicolon-delimited), heuristic
//! tabular clipboard detection that guards against prose punctuation false-positives, and workbook
//! loading for Excel / OpenDocument formats (`.xlsx`, `.xls`, `.xlsb`, `.ods`) using `calamine`.

use calamine::{
    Cell, CellType, Data, DataRef, DataType, Dimensions, Range as WorksheetRange, Reader,
    SheetVisible, Sheets, open_workbook_auto_from_rs,
};
use std::io::Cursor;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

mod ods;

/// A merged range in sheet-relative, zero-based coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedCell {
    pub rows: Range<usize>,
    pub columns: Range<usize>,
}

/// A cell positioned within a grid, including its source anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell {
    pub anchor: (usize, usize),
    pub rows: Range<usize>,
    pub columns: Range<usize>,
}

/// A single tabular sheet containing rows and columns of cell string values.
#[derive(Debug, Clone, PartialEq)]
pub struct SpreadsheetSheet {
    /// Sheet name or identifier.
    pub name: String,
    /// Position in the source workbook, including hidden sheets.
    pub workbook_index: usize,
    /// Total row count.
    pub total_rows: usize,
    /// Total column count.
    pub total_cols: usize,
    /// 2D grid of cell values.
    pub rows: Vec<Vec<String>>,
    /// Zero-based workbook coordinate of the grid's top-left cell.
    pub origin: (usize, usize),
    /// Rectangular merged cells. The top-left cell owns the value.
    pub merges: Vec<MergedCell>,
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
            workbook_index: 0,
            total_rows,
            total_cols,
            rows,
            origin: (0, 0),
            merges: Vec::new(),
        }
    }

    /// Returns a sliced 2D grid of the upper-left cells (e.g. top 8 rows, first 5 cols).
    pub fn upper_left_slice(&self, max_rows: usize, max_cols: usize) -> Vec<Vec<String>> {
        self.rows
            .iter()
            .take(max_rows)
            .map(|row| row[..row.len().min(max_cols)].to_vec())
            .collect()
    }

    /// Returns the stored value at a sheet-relative coordinate.
    pub fn cell(&self, row: usize, column: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|row| row.get(column))
            .map_or("", String::as_str)
    }

    /// Projects sheet cells into a visible grid, emitting each merged cell once.
    ///
    /// `rows` maps display rows to source rows in ascending order. Cell geometry
    /// can extend past the viewport so callers can clip it without shifting the anchor.
    pub fn grid_cells(
        &self,
        rows: &[usize],
        visible_rows: Range<usize>,
        columns: Range<usize>,
    ) -> Vec<GridCell> {
        let Some(visible_source_rows) = rows.get(visible_rows.clone()).filter(|r| !r.is_empty())
        else {
            return Vec::new();
        };
        if columns.is_empty() {
            return Vec::new();
        }

        let mut merges: Vec<_> = self
            .merges
            .iter()
            .filter(|span| {
                span.rows.start <= visible_source_rows[visible_source_rows.len() - 1]
                    && span.rows.end > visible_source_rows[0]
                    && span.columns.start < columns.end
                    && span.columns.end > columns.start
            })
            .filter_map(|span| {
                let start = rows.partition_point(|&row| row < span.rows.start);
                let end = rows.partition_point(|&row| row < span.rows.end);
                (start < end).then(|| GridCell {
                    anchor: (span.rows.start, span.columns.start),
                    rows: start..end,
                    columns: span.columns.clone(),
                })
            })
            .collect();
        merges.sort_unstable_by_key(|cell| cell.columns.start);

        let mut cells = Vec::new();
        for display_row in visible_rows.clone() {
            let mut column = columns.start;
            for merge in merges
                .iter()
                .filter(|cell| cell.rows.contains(&display_row))
            {
                cells.extend((column..merge.columns.start).map(|col| GridCell {
                    anchor: (rows[display_row], col),
                    rows: display_row..display_row + 1,
                    columns: col..col + 1,
                }));
                if display_row == merge.rows.start.max(visible_rows.start) {
                    cells.push(merge.clone());
                }
                column = column.max(merge.columns.end);
            }
            cells.extend((column..columns.end).map(|col| GridCell {
                anchor: (rows[display_row], col),
                rows: display_row..display_row + 1,
                columns: col..col + 1,
            }));
        }
        cells
    }
}

/// Parses a delimiter-separated text string (auto-detecting comma, tab, or semicolon delimiters) into rows.
pub fn parse_csv_string(csv_text: &str) -> Vec<Vec<String>> {
    delimited_rows(csv_text, detect_delimiter(csv_text)).collect()
}

fn detect_delimiter(text: &str) -> char {
    let mut counts = [0; 3];
    let mut quoted = false;
    let mut lines = 0;
    for character in text.chars().take(64 * 1024) {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => counts[0] += 1,
            '\t' if !quoted => counts[1] += 1,
            ';' if !quoted => counts[2] += 1,
            '\n' if !quoted => {
                lines += 1;
                if lines == 5 {
                    break;
                }
            }
            _ => (),
        }
    }
    if counts[1] > counts[0] && counts[1] > counts[2] {
        '\t'
    } else if counts[2] > counts[0] {
        ';'
    } else {
        ','
    }
}

fn delimited_rows(text: &str, delimiter: char) -> impl Iterator<Item = Vec<String>> {
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    std::iter::from_fn(move || {
        loop {
            let mut row = Vec::new();
            let mut field = String::new();
            let mut quoted = false;
            let mut explicit_field = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' if quoted && chars.peek() == Some(&'"') => {
                        chars.next();
                        field.push('"');
                    }
                    '"' if quoted => quoted = false,
                    '"' if field.trim().is_empty() => {
                        quoted = true;
                        explicit_field = true;
                    }
                    c if c == delimiter && !quoted => {
                        row.push(field.trim().to_owned());
                        field.clear();
                    }
                    '\n' if !quoted => break,
                    _ => field.push(c),
                }
            }
            if !row.is_empty() || !field.trim().is_empty() || explicit_field {
                row.push(field.trim().to_owned());
                return Some(row);
            }
            chars.peek()?;
        }
    })
}

/// Fast first-pass check for delimited clipboard text. Full parsing and shape
/// validation still happen in [`detect_csv_text`] when the table is opened.
pub fn looks_like_csv_text(text: &str) -> bool {
    candidate_delimiter(text).is_some()
}

fn looks_like_comma_prose(sample: &[&str], comma_count: usize) -> bool {
    let sentence_lines = sample
        .iter()
        .filter(|line| line.ends_with('.') || line.ends_with('!') || line.ends_with('?'))
        .count();
    if sentence_lines >= 2 {
        return true;
    }
    let spaced_commas: usize = sample.iter().map(|line| line.matches(", ").count()).sum();
    comma_count > 0
        && spaced_commas == comma_count
        && sample
            .iter()
            .map(|line| line.split_whitespace().count())
            .sum::<usize>()
            / sample.len()
            > 6
}

fn candidate_delimiter(text: &str) -> Option<char> {
    let lines: Vec<&str> = text
        .trim()
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(5)
        .collect();
    if lines.len() < 2 {
        return None;
    }

    let comma_count: usize = lines.iter().map(|l| l.matches(',').count()).sum();
    let tab_count: usize = lines.iter().map(|l| l.matches('\t').count()).sum();
    let semi_count: usize = lines.iter().map(|l| l.matches(';').count()).sum();

    // Must have at least 1 delimiter per line on average
    if comma_count < lines.len() && tab_count < lines.len() && semi_count < lines.len() {
        return None;
    }

    let delimiter = if tab_count >= comma_count && tab_count >= semi_count {
        '\t'
    } else if semi_count >= comma_count && semi_count >= tab_count {
        ';'
    } else {
        ','
    };

    if delimiter == ',' && looks_like_comma_prose(&lines, comma_count) {
        return None;
    }

    Some(delimiter)
}

/// Detects if raw clipboard text is genuinely a formatted CSV / TSV table.
pub fn detect_csv_text(text: &str) -> Option<SpreadsheetData> {
    let trimmed = text.trim();
    let delimiter = candidate_delimiter(trimmed)?;
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

/// Loads a spreadsheet from a guarded file source.
pub fn load_spreadsheet(path: &Path) -> Result<SpreadsheetData, String> {
    load(path, None, &AtomicBool::new(false))
}

/// Loads the upper-left cells of the first visible, populated worksheet.
///
/// The returned sheet keeps its full used dimensions even though only the
/// requested cells and intersecting merges are retained.
pub fn load_spreadsheet_preview(
    path: &Path,
    rows: usize,
    columns: usize,
    cancelled: &AtomicBool,
) -> Result<SpreadsheetData, String> {
    load(path, Some((rows, columns)), cancelled)
}

fn load(
    path: &Path,
    preview: Option<(usize, usize)>,
    cancelled: &AtomicBool,
) -> Result<SpreadsheetData, String> {
    check_cancelled(cancelled)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let title = path.file_name().map_or_else(
        || "Spreadsheet".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    let sheets = match extension.as_str() {
        "csv" | "tsv" => vec![load_delimited(
            path,
            extension == "tsv",
            preview,
            cancelled,
        )?],
        "xlsx" | "xls" | "xlsb" | "ods" => load_workbook(path, preview, cancelled)?,
        _ => return Err(format!("Unsupported spreadsheet format: .{extension}")),
    };
    Ok(SpreadsheetData { title, sheets })
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Spreadsheet preview cancelled".to_owned());
    }
    Ok(())
}

fn limit_preview_text(value: &mut String) {
    if let Some((end, _)) = value.char_indices().nth(256) {
        value.truncate(end);
        value.push('…');
    }
}

fn load_delimited(
    path: &Path,
    tsv: bool,
    preview: Option<(usize, usize)>,
    cancelled: &AtomicBool,
) -> Result<SpreadsheetSheet, String> {
    let content = crate::kernel_interface::read_content_to_string_bounded(path, 32 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let delimiter = if tsv {
        '\t'
    } else {
        detect_delimiter(&content)
    };
    let (row_limit, column_limit) = preview.unwrap_or((usize::MAX, usize::MAX));
    let mut sheet = SpreadsheetSheet::new("Sheet1".to_owned(), Vec::new());
    for (index, mut row) in delimited_rows(&content, delimiter).enumerate() {
        check_cancelled(cancelled)?;
        sheet.total_rows = index + 1;
        sheet.total_cols = sheet.total_cols.max(row.len());
        if index < row_limit {
            row.truncate(column_limit);
            if preview.is_some() {
                row.iter_mut().for_each(limit_preview_text);
            }
            sheet.rows.push(row);
        }
    }
    Ok(sheet)
}

fn load_workbook(
    path: &Path,
    preview: Option<(usize, usize)>,
    cancelled: &AtomicBool,
) -> Result<Vec<SpreadsheetSheet>, String> {
    let bytes = crate::kernel_interface::read_content_bounded(path, 64 * 1024 * 1024)
        .map_err(|error| format!("Failed to read workbook: {error}"))?;
    check_cancelled(cancelled)?;
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes.as_slice()))
        .map_err(|error| format!("Failed to open workbook: {error}"))?;
    let mut ods_merges = if matches!(workbook, Sheets::Ods(_)) {
        ods::read_merges(&bytes, cancelled).unwrap_or_default()
    } else {
        Default::default()
    };
    let metadata = workbook.sheets_metadata().to_vec();
    let mut sheets = Vec::new();
    for (index, info) in metadata.into_iter().enumerate() {
        check_cancelled(cancelled)?;
        if preview.is_some() && info.visible != SheetVisible::Visible {
            continue;
        }
        let name = info.name;
        let merges = ods_merges.remove(&name).unwrap_or_default();
        let sheet = read_workbook_sheet(&mut workbook, name, merges, preview, cancelled);
        check_cancelled(cancelled)?;
        if let Ok(mut sheet) = sheet {
            sheet.workbook_index = index;
            if preview.is_some() && sheet.total_rows > 0 && sheet.total_cols > 0 {
                return Ok(vec![sheet]);
            }
            sheets.push(sheet);
        }
    }
    if sheets.is_empty() {
        return Err("No sheets found in workbook".to_owned());
    }
    if preview.is_some() {
        sheets.truncate(1);
    }
    Ok(sheets)
}

fn read_workbook_sheet(
    workbook: &mut Sheets<Cursor<&[u8]>>,
    name: String,
    ods_merges: Vec<Dimensions>,
    preview: Option<(usize, usize)>,
    cancelled: &AtomicBool,
) -> Result<SpreadsheetSheet, String> {
    let mut merges = match workbook {
        Sheets::Xlsx(book) => book.merge_cells_by_sheet_name(&name).unwrap_or_default(),
        Sheets::Xls(book) => book.merge_cells_by_sheet_name(&name).unwrap_or_default(),
        Sheets::Ods(_) => ods_merges,
        Sheets::Xlsb(_) => Vec::new(),
    };
    merges.retain(|span| span.start.0 <= span.end.0 && span.start.1 <= span.end.1);
    if let Some(limits) = preview {
        match workbook {
            Sheets::Xlsx(book) => {
                let mut reader = book
                    .worksheet_cells_reader(&name)
                    .map_err(|err| err.to_string())?;
                return stream_preview(name, || reader.next_cell(), merges, limits, cancelled);
            }
            Sheets::Xlsb(book) => {
                let mut reader = book
                    .worksheet_cells_reader(&name)
                    .map_err(|err| err.to_string())?;
                return stream_preview(name, || reader.next_cell(), merges, limits, cancelled);
            }
            _ => (),
        }
    }
    let range = workbook
        .worksheet_range(&name)
        .map_err(|err| err.to_string())?;
    let bounds = range
        .start()
        .zip(range.end())
        .map(|(start, end)| Dimensions { start, end });
    Ok(workbook_sheet(name, &range, bounds, merges, preview))
}

fn include_dimensions(bounds: &mut Option<Dimensions>, span: Dimensions) {
    if let Some(bounds) = bounds {
        bounds.start = (
            bounds.start.0.min(span.start.0),
            bounds.start.1.min(span.start.1),
        );
        bounds.end = (bounds.end.0.max(span.end.0), bounds.end.1.max(span.end.1));
    } else {
        *bounds = Some(span);
    }
}

fn stream_preview<'a, E: std::fmt::Display>(
    name: String,
    mut next_cell: impl FnMut() -> Result<Option<Cell<DataRef<'a>>>, E>,
    merges: Vec<Dimensions>,
    limits: (usize, usize),
    cancelled: &AtomicBool,
) -> Result<SpreadsheetSheet, String> {
    let mut bounds = None;
    for &span in &merges {
        include_dimensions(&mut bounds, span);
    }
    let mut cells: Vec<Cell<String>> = Vec::new();
    while let Some(cell) = next_cell().map_err(|err| err.to_string())? {
        check_cancelled(cancelled)?;
        if cell.get_value().is_empty() {
            continue;
        }
        let position = cell.get_position();
        let old_start = bounds.map(|bounds| bounds.start);
        include_dimensions(
            &mut bounds,
            Dimensions {
                start: position,
                end: position,
            },
        );
        let start = bounds.map_or(position, |bounds| bounds.start);
        let in_window = |(row, column): (u32, u32)| {
            ((row - start.0) as usize) < limits.0 && ((column - start.1) as usize) < limits.1
        };
        if old_start != Some(start) {
            cells.retain(|cell| in_window(cell.get_position()));
        }
        if in_window(position) {
            let display = Data::from(cell.get_value().clone()).to_string();
            let mut value = display.trim().to_owned();
            limit_preview_text(&mut value);
            cells.push(Cell::new(position, value));
        }
    }
    let range = WorksheetRange::from_sparse(cells);
    Ok(workbook_sheet(name, &range, bounds, merges, Some(limits)))
}

fn workbook_sheet(
    name: String,
    range: &WorksheetRange<impl CellType + std::fmt::Display>,
    mut bounds: Option<Dimensions>,
    merges: Vec<Dimensions>,
    preview: Option<(usize, usize)>,
) -> SpreadsheetSheet {
    for &span in &merges {
        include_dimensions(&mut bounds, span);
    }
    let Some(Dimensions {
        start: first,
        end: last,
    }) = bounds
    else {
        return SpreadsheetSheet::new(name, Vec::new());
    };
    let origin = (first.0 as usize, first.1 as usize);
    let total_rows = last.0 as usize - origin.0 + 1;
    let total_cols = last.1 as usize - origin.1 + 1;
    let (max_rows, max_cols) = preview.unwrap_or((usize::MAX, usize::MAX));
    let row_limit = total_rows.min(max_rows);
    let column_limit = total_cols.min(max_cols);
    let merges = merges
        .into_iter()
        .filter_map(|span| {
            let row = span.start.0 as usize - origin.0;
            let column = span.start.1 as usize - origin.1;
            (row < row_limit && column < column_limit).then(|| MergedCell {
                rows: row..span.end.0 as usize - origin.0 + 1,
                columns: column..span.end.1 as usize - origin.1 + 1,
            })
        })
        .collect();

    let mut rows = Vec::new();
    if let Some((row_start, column_start)) = range.start() {
        let row_offset = row_start as usize - origin.0;
        let column_offset = column_start as usize - origin.1;
        if row_offset < row_limit && column_offset < column_limit {
            rows.resize_with(row_offset, Vec::new);
            for row in range.rows().take(row_limit - row_offset) {
                let mut values = vec![String::new(); column_offset];
                values.extend(row.iter().take(column_limit - column_offset).map(|cell| {
                    let mut value = cell.to_string().trim().to_owned();
                    if preview.is_some() {
                        limit_preview_text(&mut value);
                    }
                    value
                }));
                rows.push(values);
            }
        }
    }

    SpreadsheetSheet {
        name,
        workbook_index: 0,
        total_rows,
        total_cols,
        rows,
        origin,
        merges,
    }
}

/// Returns `true` if the given path has a supported spreadsheet file extension.
pub fn is_spreadsheet_file(path: &Path) -> bool {
    crate::file_format::Format::for_path(path) == Some(crate::file_format::Format::Spreadsheet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn workbook_corner_keeps_offset_merge_and_full_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged.xlsx");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, xml) in [
            (
                "_rels/.rels",
                r#"<Relationships><Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Hidden" r:id="r0" state="hidden"/><sheet name="Forecast" r:id="r1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="r0" Type="worksheet" Target="worksheets/hidden.xml"/><Relationship Id="r1" Type="worksheet" Target="worksheets/forecast.xml"/></Relationships>"#,
            ),
            (
                "xl/worksheets/hidden.xml",
                r#"<worksheet><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Hidden value</t></is></c></row></sheetData></worksheet>"#,
            ),
            (
                "xl/worksheets/forecast.xml",
                r#"<worksheet><sheetData><row r="3"><c r="B3" t="inlineStr"><is><t>Quarterly forecast</t></is></c></row><row r="8"><c r="F8"><v>7</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="B3:E4"/></mergeCells></worksheet>"#,
            ),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(xml.as_bytes()).unwrap();
        }
        zip.finish().unwrap();

        let full = load_spreadsheet(&path).unwrap();
        let preview = load_spreadsheet_preview(&path, 2, 3, &AtomicBool::new(false)).unwrap();
        let sheet = &preview.sheets[0];
        assert_eq!(sheet.name, "Forecast");
        assert_eq!(sheet.workbook_index, 1);
        assert_eq!(sheet.origin, (2, 1));
        assert_eq!((sheet.total_rows, sheet.total_cols), (6, 5));
        assert_eq!(sheet.cell(0, 0), "Quarterly forecast");
        assert_eq!(sheet.cell(5, 4), "");
        assert_eq!(full.sheets[1].cell(5, 4), "7");
        assert_eq!(sheet.merges, full.sheets[1].merges);
        assert_eq!(
            sheet.grid_cells(&[0, 1], 0..2, 0..3),
            [GridCell {
                anchor: (0, 0),
                rows: 0..2,
                columns: 0..4
            }]
        );
    }

    #[test]
    fn opendocument_merges_follow_repeated_rows_and_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged.ods");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let worksheet = r#"<office:document-content
            xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
            xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
            xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
            <office:body><office:spreadsheet><table:table table:name="Forecast">
            <table:table-row table:number-rows-repeated="2"><table:table-cell/></table:table-row>
            <table:table-row><table:table-cell table:number-columns-repeated="2"/>
              <table:table-cell table:number-columns-spanned="3" table:number-rows-spanned="2" office:value-type="string"><text:p>Quarterly forecast</text:p></table:table-cell>
              <table:covered-table-cell table:number-columns-repeated="2"/>
            </table:table-row>
            <table:table-row><table:table-cell table:number-columns-repeated="2"/><table:covered-table-cell table:number-columns-repeated="3"/></table:table-row>
            </table:table></office:spreadsheet></office:body></office:document-content>"#;
        for (name, content) in [
            ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
            ("content.xml", worksheet),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();

        let data = load_spreadsheet(&path).unwrap();
        let sheet = &data.sheets[0];
        assert_eq!(sheet.origin, (2, 2));
        assert_eq!(sheet.cell(0, 0), "Quarterly forecast");
        assert_eq!(
            sheet.merges,
            [MergedCell {
                rows: 0..2,
                columns: 0..3
            }]
        );
    }

    #[test]
    fn merge_crossing_viewport_uses_its_anchor_in_filtered_grid() {
        let mut sheet = SpreadsheetSheet::new("Data".into(), Vec::new());
        sheet.merges.push(MergedCell {
            rows: 1..9,
            columns: 0..4,
        });
        let cells = sheet.grid_cells(&[1, 4, 8, 11], 1..3, 2..5);
        assert_eq!(
            cells[0],
            GridCell {
                anchor: (1, 0),
                rows: 0..3,
                columns: 0..4
            }
        );
        assert_eq!(cells[1].anchor, (4, 4));
        assert_eq!(cells[2].anchor, (8, 4));
        assert_eq!(cells.len(), 3);
    }

    #[test]
    fn delimited_corner_keeps_multiline_fields_and_trailing_tabs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("table.TSV");
        std::fs::write(
            &path,
            "Name\tComment\t\r\nAda\t\"first\nsecond, third\"\t\r\nGrace\tlast\t\r\n",
        )
        .unwrap();
        let preview = load_spreadsheet_preview(&path, 2, 2, &AtomicBool::new(false)).unwrap();
        let sheet = &preview.sheets[0];
        assert_eq!((sheet.total_rows, sheet.total_cols), (3, 3));
        assert_eq!(
            sheet.rows,
            [["Name", "Comment"], ["Ada", "first\nsecond, third"]]
        );
    }

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
        assert!(!looks_like_csv_text(prose));
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
        assert!(looks_like_csv_text(tsv));
        let res = detect_csv_text(tsv);
        assert!(res.is_some());
        let sheet = &res.unwrap().sheets[0];
        assert_eq!(sheet.total_rows, 3);
        assert_eq!(sheet.total_cols, 3);
    }

    #[test]
    fn candidate_check_defers_full_table_validation() {
        let inconsistent = "Name,Age\nAlice,30\nBob,27\nCarol,34,Extra";
        assert!(looks_like_csv_text(inconsistent));
        assert!(detect_csv_text(inconsistent).is_none());
    }
}
