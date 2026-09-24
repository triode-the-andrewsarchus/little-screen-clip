//! 2D grid spreadsheet inspection workbench view.
//!
//! Renders multi-sheet tabs, column auto-sizing, cell search filtering, and TSV clipboard exporting.

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::{BODY_FONT_SIZE, StatusToast};
use eframe::egui::{self, FontId, Rounding, Stroke, TextStyle, Vec2};
use little_clip_ops::spreadsheet::{SpreadsheetData, SpreadsheetSheet};
use std::ops::Range;
use std::path::PathBuf;

/// Interactive UI state for multi-sheet spreadsheet data tables and search filters.
pub struct SpreadsheetViewerState {
    pub title: String,
    pub path: Option<PathBuf>,
    pub data: SpreadsheetData,
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

    #[allow(dead_code)]
    pub fn current_sheet(&self) -> Option<&SpreadsheetSheet> {
        self.data.sheets.get(self.active_sheet_idx)
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::theme::AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Escape hotkey to close
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close = true;
            return;
        }

        let sheet_count = self.data.sheets.len();
        let total_rows = self
            .data
            .sheets
            .get(self.active_sheet_idx)
            .map(|s| s.total_rows)
            .unwrap_or(0);
        let total_cols = self
            .data
            .sheets
            .get(self.active_sheet_idx)
            .map(|s| s.total_cols)
            .unwrap_or(0);

        let bg = crate::hex_to_c32(&palette.theme.background);
        let f_high = crate::hex_to_c32(&palette.theme.f_high);
        let f_low = crate::hex_to_c32(&palette.theme.f_low);
        let b_med = crate::hex_to_c32(&palette.theme.b_med);
        let b_low = crate::hex_to_c32(&palette.theme.b_low);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let warn_accent = crate::hex_to_c32(&palette.harmonies.warm_attention);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                let info_badge = format!("{} rows × {} cols", total_rows, total_cols);
                let mut shell = StudioShell::new(&self.title, Icon::Spreadsheet, palette, close)
                    .with_badge(info_badge, None)
                    .with_escape_hatch(
                        EscapeHatch::primary(
                            "Calc",
                            "localc",
                            "Open spreadsheet in LibreOffice Calc",
                        )
                        .with_icon(Icon::Spreadsheet),
                    )
                    .with_footer_hints("Click cell: Copy | Filter cells above | Esc: Close");

                if let Some((r, c)) = self.selected_cell {
                    if let Some(sheet) = self.data.sheets.get(self.active_sheet_idx) {
                        if let Some(row) = sheet.rows.get(r) {
                            if let Some(val) = row.get(c) {
                                shell = shell.with_footer_readout(format!(
                                    "Selected R{} C{} [{}]: {}",
                                    r + 1,
                                    c + 1,
                                    col_label(c),
                                    val
                                ));
                            }
                        }
                    }
                }

                let mut copy_table_clicked = false;
                let clicked_hatch = shell.render_header(ui, |ui| {
                    // Copy Table button
                    let copy_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Copy),
                        "Copy Table",
                        b_med,
                        palette,
                    )
                    .stroke(Stroke::new(1.0_f32, b_low));
                    if ui.add(copy_btn).clicked() {
                        copy_table_clicked = true;
                    }

                    // Search box
                    ui.add_space(8.0);
                    Icon::Search.ui(ui, 13.0, crate::hex_to_c32(&palette.theme.f_med));
                    ui.add_sized(
                        [160.0, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Filter cells..."),
                    );
                });

                if let Some(0) = clicked_hatch {
                    if let Some(ref p) = self.path {
                        EscapeHatchLauncher::launch(
                            "LibreOffice Calc",
                            "localc",
                            p,
                            &mut self.status_toast,
                            now,
                        );
                    } else {
                        self.status_toast
                            .set("No on-disk file path for this spreadsheet", now);
                    }
                }

                if copy_table_clicked {
                    if let Some(sheet) = self.data.sheets.get(self.active_sheet_idx) {
                        let tsv = sheet
                            .rows
                            .iter()
                            .map(|r| r.join("\t"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        ui.output_mut(|o| o.copied_text = tsv);
                        self.status_toast.set("Copied table as TSV", now);
                    }
                }

                ui.add_space(4.0);

                // 2. Sheet Tabs
                if sheet_count > 1 {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Sheets:")
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        for (idx, sheet) in self.data.sheets.iter().enumerate() {
                            let is_active = idx == self.active_sheet_idx;
                            let tab_btn = crate::mode_tab_button(&sheet.name, is_active, palette);

                            if ui.add(tab_btn).clicked() {
                                self.active_sheet_idx = idx;
                                self.selected_cell = None;
                                self.scroll_offset = Vec2::ZERO;
                            }
                        }
                    });
                    ui.add_space(4.0);
                }

                ui.separator();
                ui.add_space(4.0);

                // 3. Scrollable 2D X/Y Spreadsheet Table with Sticky Headers
                let Some(sheet) = self.data.sheets.get(self.active_sheet_idx) else {
                    ui.label("No sheet data available.");
                    let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                    shell.render_footer(ui, &toast_tuple, now);
                    return;
                };

                if sheet.rows.is_empty() {
                    ui.label("Sheet is empty.");
                    let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                    shell.render_footer(ui, &toast_tuple, now);
                    return;
                }

                let query = self.search_query.trim().to_lowercase();
                if self.filtered_sheet_idx != Some(self.active_sheet_idx)
                    || self.filtered_query != query
                {
                    self.filtered_rows = sheet
                        .rows
                        .iter()
                        .enumerate()
                        .filter_map(|(row_idx, row)| {
                            (query.is_empty()
                                || row_idx == 0
                                || row.iter().any(|cell| cell.to_lowercase().contains(&query)))
                            .then_some(row_idx)
                        })
                        .collect();
                    self.filtered_sheet_idx = Some(self.active_sheet_idx);
                    self.filtered_query = query.clone();
                    self.scroll_offset.y = 0.0;
                }
                let visible_rows = &self.filtered_rows;

                let col_count = sheet.total_cols;
                let font_id = TextStyle::Small.resolve(ui.style());
                if self.measured_font.as_ref() != Some(&font_id) {
                    self.column_width_cache.fill(None);
                    self.measured_font = Some(font_id.clone());
                }
                if self.column_width_cache[self.active_sheet_idx].is_none() {
                    let mut measured = vec![0.0f32; col_count];
                    ui.fonts(|fonts| {
                        for row in sheet.rows.iter().take(40) {
                            for (column, cell) in row.iter().enumerate().take(col_count) {
                                let width = fonts
                                    .layout_no_wrap(
                                        cell.clone(),
                                        font_id.clone(),
                                        egui::Color32::WHITE,
                                    )
                                    .size()
                                    .x;
                                measured[column] = measured[column].max(width);
                            }
                        }
                    });
                    self.column_width_cache[self.active_sheet_idx] = Some(measured);
                }
                let min_col_width = ui.spacing().interact_size.x.max(90.0);
                let max_col_width = (ui.available_width() * 0.35).clamp(180.0, 420.0);
                let cell_padding = ui.spacing().button_padding.x * 2.0 + 16.0;
                let col_widths: Vec<f32> = self.column_width_cache[self.active_sheet_idx]
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|width| (width + cell_padding).clamp(min_col_width, max_col_width))
                    .collect();

                let corner_width = 48.0;
                let row_height = ui.spacing().interact_size.y.max(26.0);
                let header_height = row_height;
                let table_spacing = Vec2::new(1.0, 1.0);

                let col_offsets = column_offsets(&col_widths, table_spacing.x);
                let total_content_width = *col_offsets.last().unwrap_or(&0.0);
                let row_stride = row_height + table_spacing.y;
                let total_content_height =
                    (visible_rows.len() as f32 * row_stride - table_spacing.y).max(0.0);

                let avail_w = ui.available_width();
                let avail_h = (ui.available_height() - 32.0).max(100.0);
                let grid_w = (avail_w - corner_width).max(10.0);

                let mut clicked_corner = false;
                let mut clicked_col: Option<usize> = None;
                let mut clicked_row: Option<(usize, Vec<String>)> = None;
                let mut clicked_cell: Option<(usize, usize, String)> = None;
                let mut new_scroll_offset: Option<Vec2> = None;

                // --- Top row: Fixed Corner "#" + Sticky Column Numbers ---
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;

                    let corner_label = egui::RichText::new("#").small().strong().color(f_low);
                    let corner_btn = egui::Button::new(corner_label)
                        .fill(b_low)
                        .min_size(Vec2::new(corner_width, header_height))
                        .stroke(Stroke::new(0.5_f32, b_med))
                        .rounding(Rounding::ZERO);
                    if ui
                        .add(corner_btn)
                        .on_hover_text("Click to copy entire sheet as TSV")
                        .clicked()
                    {
                        clicked_corner = true;
                    }

                    let (col_header_rect, _) = ui.allocate_exact_size(
                        Vec2::new(grid_w, header_height),
                        egui::Sense::hover(),
                    );
                    ui.painter()
                        .rect_filled(col_header_rect, Rounding::ZERO, b_med);

                    let col_header_ui_rect = egui::Rect::from_min_max(
                        egui::pos2(
                            col_header_rect.min.x - self.scroll_offset.x,
                            col_header_rect.min.y,
                        ),
                        egui::pos2(
                            col_header_rect.min.x - self.scroll_offset.x
                                + total_content_width
                                + 500.0,
                            col_header_rect.max.y,
                        ),
                    );

                    let mut header_child_ui = ui.child_ui(
                        col_header_ui_rect,
                        egui::Layout::left_to_right(egui::Align::Center),
                    );
                    header_child_ui.set_clip_rect(col_header_rect);
                    header_child_ui.spacing_mut().item_spacing = table_spacing;

                    let header_columns =
                        visible_column_range(self.scroll_offset.x, grid_w, &col_offsets);
                    for c_idx in header_columns {
                        let cell_w = col_widths[c_idx];
                        let is_selected_col =
                            self.selected_cell.map(|(_, c)| c == c_idx).unwrap_or(false);
                        let col_bg = if is_selected_col { accent } else { b_med };
                        let text_color = if is_selected_col {
                            crate::contrast_text_c32(
                                col_bg,
                                Some(crate::hex_to_c32(&palette.theme.f_inv)),
                                palette,
                            )
                        } else {
                            crate::contrast_text_c32(col_bg, Some(f_high), palette)
                        };

                        let col_title = format!("{} ({})", col_label(c_idx), c_idx + 1);
                        let col_btn = egui::Button::new(
                            egui::RichText::new(col_title)
                                .small()
                                .strong()
                                .color(text_color),
                        )
                        .fill(col_bg)
                        .min_size(Vec2::new(cell_w, header_height))
                        .stroke(Stroke::new(0.5_f32, b_low))
                        .rounding(Rounding::ZERO);

                        let cell_rect = egui::Rect::from_min_size(
                            egui::pos2(
                                col_header_ui_rect.min.x + col_offsets[c_idx],
                                col_header_rect.min.y,
                            ),
                            Vec2::new(cell_w, header_height),
                        );
                        let resp = header_child_ui.put(cell_rect, col_btn);
                        if resp.clicked() {
                            clicked_col = Some(c_idx);
                        }
                        resp.on_hover_text(format!(
                            "Column {} ({})\nWidth: {:.0}px\nClick to copy column",
                            c_idx + 1,
                            col_label(c_idx),
                            cell_w
                        ));
                    }
                });

                // --- Body: Sticky Left Row Numbers + Scrollable 2D Grid ---
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;

                    // Sticky Row Numbers Gutter on the Left
                    let (row_strip_rect, _) = ui.allocate_exact_size(
                        Vec2::new(corner_width, avail_h),
                        egui::Sense::hover(),
                    );
                    ui.painter()
                        .rect_filled(row_strip_rect, Rounding::ZERO, b_low);

                    let gutter_rows = visible_row_range(
                        self.scroll_offset.y,
                        avail_h,
                        visible_rows.len(),
                        row_stride,
                    );
                    let row_strip_ui_rect = egui::Rect::from_min_max(
                        egui::pos2(
                            row_strip_rect.min.x,
                            row_strip_rect.min.y - self.scroll_offset.y
                                + gutter_rows.start as f32 * row_stride,
                        ),
                        egui::pos2(
                            row_strip_rect.max.x,
                            row_strip_rect.min.y - self.scroll_offset.y
                                + gutter_rows.end as f32 * row_stride,
                        ),
                    );

                    let mut row_child_ui =
                        ui.child_ui(row_strip_ui_rect, egui::Layout::top_down(egui::Align::Min));
                    row_child_ui.set_clip_rect(row_strip_rect);
                    row_child_ui.spacing_mut().item_spacing = table_spacing;

                    row_child_ui.skip_ahead_auto_ids(gutter_rows.start);
                    for &r_idx in &visible_rows[gutter_rows] {
                        let row = &sheet.rows[r_idx];
                        let is_selected_row =
                            self.selected_cell.map(|(r, _)| r == r_idx).unwrap_or(false);
                        let row_bg = if is_selected_row { accent } else { b_low };
                        let text_color = if is_selected_row {
                            crate::contrast_text_c32(
                                row_bg,
                                Some(crate::hex_to_c32(&palette.theme.f_inv)),
                                palette,
                            )
                        } else {
                            crate::contrast_text_c32(row_bg, Some(f_low), palette)
                        };

                        let row_btn = egui::Button::new(
                            egui::RichText::new(format!("{}", r_idx + 1))
                                .small()
                                .strong()
                                .color(text_color),
                        )
                        .fill(row_bg)
                        .min_size(Vec2::new(corner_width, row_height))
                        .stroke(Stroke::new(0.5_f32, b_med))
                        .rounding(Rounding::ZERO);

                        let resp = row_child_ui.add(row_btn);
                        if resp.clicked() {
                            clicked_row = Some((r_idx, row.clone()));
                        }
                        resp.on_hover_text(format!("Row {}\nClick to copy row", r_idx + 1));
                    }

                    // Main Data Grid (ScrollArea::both)
                    // Invert horizontal scroll delta so horizontal scrolling moves naturally
                    ui.input_mut(|i| {
                        i.raw_scroll_delta.x = -i.raw_scroll_delta.x;
                        i.smooth_scroll_delta.x = -i.smooth_scroll_delta.x;
                    });

                    let scroll_output = egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .max_width(grid_w)
                        .max_height(avail_h)
                        .show_viewport(ui, |ui, viewport| {
                            ui.spacing_mut().item_spacing = table_spacing;
                            ui.set_min_size(Vec2::new(total_content_width, total_content_height));
                            let rows = visible_row_range(
                                viewport.min.y,
                                viewport.height(),
                                visible_rows.len(),
                                row_stride,
                            );
                            let columns = visible_column_range(
                                viewport.min.x,
                                viewport.width(),
                                &col_offsets,
                            );
                            let row_rect = egui::Rect::from_min_size(
                                egui::pos2(
                                    ui.max_rect().left(),
                                    ui.max_rect().top() + rows.start as f32 * row_stride,
                                ),
                                Vec2::new(total_content_width, rows.len() as f32 * row_stride),
                            );
                            ui.allocate_ui_at_rect(row_rect, |ui| {
                                ui.spacing_mut().item_spacing = table_spacing;
                                ui.skip_ahead_auto_ids(rows.start);
                                ui.vertical(|ui| {
                                    for &r_idx in &visible_rows[rows] {
                                        let row = &sheet.rows[r_idx];
                                        let is_header = r_idx == 0;
                                        let row_bg = if is_header {
                                            b_med
                                        } else if r_idx % 2 == 1 {
                                            b_low
                                        } else {
                                            bg
                                        };

                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing = table_spacing;
                                            if columns.start > 0 {
                                                ui.add_space(
                                                    col_offsets[columns.start] - table_spacing.x,
                                                );
                                            }
                                            for c_idx in columns.clone() {
                                                let cell_val = row
                                                    .get(c_idx)
                                                    .map(|s| s.as_str())
                                                    .unwrap_or("");
                                                let cell_w =
                                                    col_widths.get(c_idx).copied().unwrap_or(90.0);

                                                let is_selected =
                                                    self.selected_cell == Some((r_idx, c_idx));
                                                let is_match = !query.is_empty()
                                                    && cell_val.to_lowercase().contains(&query);

                                                let cell_bg = if is_selected {
                                                    accent
                                                } else if is_match {
                                                    warn_accent
                                                } else {
                                                    row_bg
                                                };

                                                let pref_text_color = if is_header {
                                                    accent
                                                } else if is_selected {
                                                    crate::hex_to_c32(&palette.theme.f_inv)
                                                } else {
                                                    f_high
                                                };
                                                let text_color = crate::contrast_text_c32(
                                                    cell_bg,
                                                    Some(pref_text_color),
                                                    palette,
                                                );

                                                let btn = egui::Button::new(
                                                    egui::RichText::new(cell_val)
                                                        .small()
                                                        .strong()
                                                        .color(text_color),
                                                )
                                                .fill(cell_bg)
                                                .min_size(Vec2::new(cell_w, row_height))
                                                .stroke(Stroke::new(0.5_f32, b_med))
                                                .rounding(Rounding::ZERO);

                                                let resp = ui.add(btn);
                                                if resp.clicked() {
                                                    clicked_cell =
                                                        Some((r_idx, c_idx, cell_val.to_string()));
                                                }
                                                if !cell_val.is_empty() {
                                                    resp.on_hover_text(format!(
                                                        "Row {}, Col {} ({})\nClick to copy: {}",
                                                        r_idx + 1,
                                                        c_idx + 1,
                                                        col_label(c_idx),
                                                        cell_val
                                                    ));
                                                }
                                            }
                                        });
                                    }
                                });
                            });
                        });

                    // Restore horizontal scroll delta
                    ui.input_mut(|i| {
                        i.raw_scroll_delta.x = -i.raw_scroll_delta.x;
                        i.smooth_scroll_delta.x = -i.smooth_scroll_delta.x;
                    });

                    new_scroll_offset = Some(scroll_output.state.offset);
                });

                if let Some(offset) = new_scroll_offset {
                    self.scroll_offset = offset;
                }

                if clicked_corner {
                    let tsv = sheet
                        .rows
                        .iter()
                        .map(|r| r.join("\t"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.output_mut(|o| o.copied_text = tsv);
                    self.status_toast.set("Copied entire table as TSV", now);
                }

                if let Some(c_idx) = clicked_col {
                    self.selected_cell = Some((0, c_idx));
                    let col_text = sheet
                        .rows
                        .iter()
                        .filter_map(|r| r.get(c_idx).cloned())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.output_mut(|o| o.copied_text = col_text);
                    self.status_toast.set(
                        format!("Copied Column {} ({})", c_idx + 1, col_label(c_idx)),
                        now,
                    );
                }

                if let Some((r_idx, row_cells)) = clicked_row {
                    self.selected_cell = Some((r_idx, 0));
                    let tsv = row_cells.join("\t");
                    ui.output_mut(|o| o.copied_text = tsv);
                    self.status_toast
                        .set(format!("Copied Row {}", r_idx + 1), now);
                }

                if let Some((r_idx, c_idx, val)) = clicked_cell {
                    self.selected_cell = Some((r_idx, c_idx));
                    ui.output_mut(|o| o.copied_text = val.clone());
                    self.status_toast.set(format!("Copied: {}", val), now);
                }

                // 4. Footer
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_tracks_scroll_with_bounded_overscan() {
        assert_eq!(visible_row_range(0.0, 270.0, 2000, 27.0), 0..11);
        assert_eq!(visible_row_range(27000.0, 270.0, 2000, 27.0), 999..1011);
        assert_eq!(visible_row_range(80000.0, 270.0, 2000, 27.0), 1998..2000);
        assert_eq!(visible_row_range(0.0, 270.0, 0, 27.0), 0..0);
    }

    #[test]
    fn visible_columns_follow_measured_widths() {
        let offsets = column_offsets(&[100.0, 120.0, 80.0, 90.0], 1.0);
        assert_eq!(offsets, vec![0.0, 101.0, 222.0, 303.0, 393.0]);
        assert_eq!(visible_column_range(0.0, 100.0, &offsets), 0..2);
        assert_eq!(visible_column_range(230.0, 80.0, &offsets), 1..4);
        assert_eq!(visible_column_range(1000.0, 80.0, &offsets), 2..4);
        assert_eq!(visible_column_range(0.0, 80.0, &[0.0]), 0..0);
    }

    #[test]
    fn wide_sheet_renders_only_visible_columns_with_aligned_header() {
        let first_row = (0..1000)
            .map(|column| {
                if column == 0 {
                    "A (1)".to_string()
                } else {
                    format!("Cell {column}")
                }
            })
            .collect();
        let data = SpreadsheetData {
            title: "Wide sheet".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), vec![first_row])],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        let palette = little_clip_core::theme::AppPalette::default();
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 620.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );

        assert!(
            output.shapes.len() < 1000,
            "rendered {} shapes for a 1,000-column sheet",
            output.shapes.len()
        );
        let matching_x: Vec<f32> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "A (1)" => Some(text.pos.x),
                _ => None,
            })
            .collect();
        assert_eq!(matching_x.len(), 2);
        assert!((matching_x[0] - matching_x[1]).abs() <= 2.0);
    }

    #[test]
    fn large_sheet_renders_only_visible_cells() {
        let rows = std::iter::once(vec!["Name".into(), "Value".into()])
            .chain((0..5000).map(|row| vec![format!("Row {row}"), row.to_string()]))
            .collect();
        let data = SpreadsheetData {
            title: "Large sheet".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        let palette = little_clip_core::theme::AppPalette::default();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 620.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut close = false;
                state.render_ui(ui, ctx, &palette, &mut close);
                assert!(!close);
            });
        });

        assert_eq!(state.filtered_rows.len(), 5001);
        assert!(
            output.shapes.len() < 1000,
            "rendered {} shapes for a 5,001-row sheet",
            output.shapes.len()
        );
        let text_y = |label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos.y),
                _ => None,
            })
        };
        let first_gutter = text_y("1").expect("first row number is visible");
        let first_cell = text_y("Name").expect("first row is visible");
        assert!(
            (first_gutter - first_cell).abs() <= 2.0,
            "gutter y={first_gutter}, cell y={first_cell}"
        );

        state.search_query = "Row 4975".into();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(ui, ctx, &palette, &mut false);
            });
        });
        assert_eq!(state.filtered_rows, vec![0, 4976]);
    }

    #[test]
    fn column_widths_follow_rendered_text_width() {
        let data = SpreadsheetData {
            title: "Widths".into(),
            sheets: vec![SpreadsheetSheet::new(
                "Data".into(),
                vec![vec!["W".repeat(20), "i".repeat(20)]],
            )],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        let palette = little_clip_core::theme::AppPalette::default();
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 620.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut false);
                });
            },
        );
        let measured = state.column_width_cache[0].as_ref().unwrap();
        assert!(measured[0] > measured[1] * 2.0);
    }

    #[test]
    fn test_col_label() {
        assert_eq!(col_label(0), "A");
        assert_eq!(col_label(1), "B");
        assert_eq!(col_label(25), "Z");
        assert_eq!(col_label(26), "AA");
        assert_eq!(col_label(27), "AB");
        assert_eq!(col_label(51), "AZ");
        assert_eq!(col_label(52), "BA");
        assert_eq!(col_label(701), "ZZ");
        assert_eq!(col_label(702), "AAA");
    }

    #[test]
    fn test_spreadsheet_state_creation() {
        let sheet = SpreadsheetSheet::new(
            "Sheet1".to_string(),
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
        );
        let data = SpreadsheetData {
            title: "Test".to_string(),
            sheets: vec![sheet],
        };
        let state = SpreadsheetViewerState::new(data, None);
        assert_eq!(state.scroll_offset, Vec2::ZERO);
        assert_eq!(state.active_sheet_idx, 0);
        assert_eq!(state.current_sheet().unwrap().total_rows, 2);
    }
}
