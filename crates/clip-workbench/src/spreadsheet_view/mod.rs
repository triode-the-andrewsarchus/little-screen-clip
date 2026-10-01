//! 2D grid spreadsheet inspection workbench view.
//!
//! Renders multi-sheet tabs, column auto-sizing, cell search filtering, and TSV clipboard exporting.

mod inline_preview;
mod studio;

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::{BODY_FONT_SIZE, StatusToast};
use eframe::egui::{self, FontId, Rounding, Stroke, TextStyle, Vec2};
use little_clip_ops::spreadsheet::SpreadsheetData;
#[cfg(test)]
use little_clip_ops::spreadsheet::SpreadsheetSheet;
use std::borrow::Cow;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const INLINE_SPREADSHEET_COLUMN_WIDTH: f32 = 132.0;
const INLINE_SPREADSHEET_ROW_HEIGHT: f32 = 28.0;
const INLINE_SPREADSHEET_MAX_COLUMNS: usize = 32;
static NEXT_INLINE_SPREADSHEET_ID: AtomicU64 = AtomicU64::new(1);

const INLINE_SPREADSHEET_MAX_CELL_CHARS: usize = 160;
const INLINE_SPREADSHEET_MAX_SHEET_LABEL_CHARS: usize = 40;

fn inline_cell_display_text(value: &str) -> Cow<'_, str> {
    let mut chars = value.chars();
    let prefix: String = chars
        .by_ref()
        .take(INLINE_SPREADSHEET_MAX_CELL_CHARS)
        .collect();
    if chars.next().is_some() {
        Cow::Owned(format!("{prefix}…"))
    } else {
        Cow::Borrowed(value)
    }
}

fn inline_sheet_label(name: &str) -> String {
    let mut chars = name.chars();
    let prefix: String = chars
        .by_ref()
        .take(INLINE_SPREADSHEET_MAX_SHEET_LABEL_CHARS)
        .collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

/// Interactive UI state for multi-sheet spreadsheet data tables and search filters.
pub struct SpreadsheetViewerState {
    pub title: String,
    pub path: Option<PathBuf>,
    pub data: Arc<SpreadsheetData>,
    pub active_sheet_idx: usize,
    pub search_query: String,
    pub selected_cell: Option<(usize, usize)>, // (row, col)
    pub status_toast: StatusToast,
    pub scroll_offset: Vec2,
    filtered_sheet_idx: Option<usize>,
    filtered_query: String,
    filtered_rows: Vec<usize>,
    column_width_cache: Vec<Option<Vec<f32>>>,
    measured_font: Option<FontId>,
}

/// Compact virtualized spreadsheet preview for embedding in another view.
///
/// The workbook data is shared, while sheet selection, cell selection, and
/// scroll offsets belong to each preview instance.
pub struct InlineSpreadsheetPreview {
    data: Arc<SpreadsheetData>,
    host_size: Vec2,
    id: egui::Id,
    active_sheet_idx: usize,
    selected_cell: Option<(usize, usize)>,
    scroll_offset: Vec2,
}

fn visible_row_range(offset: f32, viewport_height: f32, total: usize, stride: f32) -> Range<usize> {
    if total == 0 {
        return 0..0;
    }
    let stride = stride.max(1.0);
    let first = ((offset.max(0.0) / stride).floor() as usize)
        .min(total - 1)
        .saturating_sub(1);
    let last = (((offset + viewport_height).max(0.0) / stride).ceil() as usize + 1)
        .min(total)
        .max(first + 1);
    first..last
}

fn column_offsets(widths: &[f32], spacing: f32) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(widths.len() + 1);
    offsets.push(0.0);
    for (index, width) in widths.iter().enumerate() {
        let gap = if index + 1 < widths.len() {
            spacing
        } else {
            0.0
        };
        offsets.push(offsets[index] + width + gap);
    }
    offsets
}

fn visible_column_range(offset: f32, viewport_width: f32, offsets: &[f32]) -> Range<usize> {
    let total = offsets.len().saturating_sub(1);
    if total == 0 {
        return 0..0;
    }
    let left = offset.max(0.0);
    let first = offsets[1..]
        .partition_point(|end| *end <= left)
        .min(total - 1)
        .saturating_sub(1);
    let last = offsets[..total]
        .partition_point(|start| *start < left + viewport_width.max(0.0))
        .saturating_add(1)
        .min(total)
        .max(first + 1);
    first..last
}

/// Returns the standard spreadsheet column label (e.g. 0 -> "A", 25 -> "Z", 26 -> "AA").
pub fn col_label(mut idx: usize) -> String {
    let mut s = String::new();
    loop {
        let rem = (idx % 26) as u8;
        s.push((b'A' + rem) as char);
        if idx < 26 {
            break;
        }
        idx = idx / 26 - 1;
    }
    s.chars().rev().collect()
}

impl SpreadsheetViewerState {
    /// Creates a new spreadsheet viewer state from parsed spreadsheet data.
    pub fn new(data: SpreadsheetData, path: Option<PathBuf>) -> Self {
        Self::from_shared(Arc::new(data), path)
    }

    /// Creates an independent viewer state from shared parsed workbook data.
    pub fn from_shared(data: Arc<SpreadsheetData>, path: Option<PathBuf>) -> Self {
        let title = data.title.clone();
        let column_width_cache = vec![None; data.sheets.len()];
        Self {
            title,
            path,
            data,
            active_sheet_idx: 0,
            search_query: String::new(),
            selected_cell: None,
            status_toast: StatusToast::new(),
            scroll_offset: Vec2::ZERO,
            filtered_sheet_idx: None,
            filtered_query: String::new(),
            filtered_rows: Vec::new(),
            column_width_cache,
            measured_font: None,
        }
    }

    #[cfg(test)]
    pub fn current_sheet(&self) -> Option<&SpreadsheetSheet> {
        self.data.sheets.get(self.active_sheet_idx)
    }
}

#[cfg(test)]
mod tests;
