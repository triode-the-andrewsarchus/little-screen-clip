//! 2D grid spreadsheet inspection workbench view.
//!
//! Renders multi-sheet tabs, column auto-sizing, cell search filtering, and TSV clipboard exporting.

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::StatusToast;
use eframe::egui::{self, Rounding, Stroke, Vec2};
use little_clip_ops::spreadsheet::{SpreadsheetData, SpreadsheetSheet};
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
        Self {
            title,
            path,
            data,
            active_sheet_idx: 0,
            search_query: String::new(),
            selected_cell: None,
            status_toast: StatusToast::new(),
            scroll_offset: Vec2::ZERO,
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
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Filter cells...")
                            .desired_width(160.0),
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
                        ui.label(egui::RichText::new("Sheets:").small().color(f_low));
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
                let visible_rows: Vec<(usize, &Vec<String>)> = sheet
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(r_idx, row)| {
                        if query.is_empty() {
                            true
                        } else {
                            *r_idx == 0 || row.iter().any(|c| c.to_lowercase().contains(&query))
                        }
                    })
                    .collect();

                let col_count = sheet.total_cols;
                let mut col_widths = vec![90.0f32; col_count];
                for row in sheet.rows.iter().take(40) {
                    for (c_idx, cell) in row.iter().enumerate() {
                        if c_idx < col_widths.len() {
                            let w = (cell.len() as f32 * 8.5 + 24.0).clamp(90.0, 300.0);
                            if w > col_widths[c_idx] {
                                col_widths[c_idx] = w;
                            }
                        }
                    }
                }

                let corner_width = 48.0;
                let header_height = 26.0;
                let row_height = 26.0;
                let table_spacing = Vec2::new(1.0, 1.0);

                let total_content_width: f32 = col_widths.iter().sum::<f32>()
                    + (col_widths.len().saturating_sub(1) as f32 * table_spacing.x);
                let total_content_height =
                    visible_rows.len() as f32 * (row_height + table_spacing.y);

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

                    for (c_idx, &cell_w) in col_widths.iter().enumerate() {
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

                        let resp = header_child_ui.add(col_btn);
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

                    let row_strip_ui_rect = egui::Rect::from_min_max(
                        egui::pos2(
                            row_strip_rect.min.x,
                            row_strip_rect.min.y - self.scroll_offset.y,
                        ),
                        egui::pos2(
                            row_strip_rect.max.x,
                            row_strip_rect.min.y - self.scroll_offset.y
                                + total_content_height
                                + 500.0,
                        ),
                    );

                    let mut row_child_ui =
                        ui.child_ui(row_strip_ui_rect, egui::Layout::top_down(egui::Align::Min));
                    row_child_ui.set_clip_rect(row_strip_rect);
                    row_child_ui.spacing_mut().item_spacing = table_spacing;

                    for (r_idx, row) in &visible_rows {
                        let is_selected_row = self
                            .selected_cell
                            .map(|(r, _)| r == *r_idx)
                            .unwrap_or(false);
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
                            clicked_row = Some((*r_idx, (*row).clone()));
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
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = table_spacing;
                            ui.vertical(|ui| {
                                for (r_idx, row) in &visible_rows {
                                    let is_header = *r_idx == 0;
                                    let row_bg = if is_header {
                                        b_med
                                    } else if *r_idx % 2 == 1 {
                                        b_low
                                    } else {
                                        bg
                                    };

                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing = table_spacing;
                                        for c_idx in 0..col_count {
                                            let cell_val =
                                                row.get(c_idx).map(|s| s.as_str()).unwrap_or("");
                                            let cell_w =
                                                col_widths.get(c_idx).copied().unwrap_or(90.0);

                                            let is_selected =
                                                self.selected_cell == Some((*r_idx, c_idx));
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
                                                    Some((*r_idx, c_idx, cell_val.to_string()));
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
