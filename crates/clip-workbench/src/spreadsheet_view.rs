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
        palette: &little_clip_core::palette::AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Let Escape clear the filter before closing the spreadsheet viewer.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.search_query.is_empty() {
                *close = true;
            } else {
                self.search_query.clear();
            }
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

        let bg = crate::hex_to_c32(&palette.colors.canvas);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let warn_accent = crate::hex_to_c32(&palette.harmonies.warm_attention);

        egui::Frame::none()
            .fill(bg)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
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
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    Icon::Search.ui(
                        ui,
                        crate::ui_ext::CONTROL_ICON_SIZE,
                        crate::hex_to_c32(&palette.colors.text_secondary),
                    );
                    let search_width = ui.available_width().min(160.0).max(0.0);
                    ui.add_sized(
                        [search_width, crate::ui_ext::CONTROL_HEIGHT],
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

                ui.add_space(crate::ui_ext::SPACE_XS);

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
                    ui.add_space(crate::ui_ext::SPACE_XS);
                }

                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_XS);

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
                                Some(crate::hex_to_c32(&palette.colors.text_inverse)),
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
                                Some(crate::hex_to_c32(&palette.colors.text_inverse)),
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
                                                    crate::hex_to_c32(&palette.colors.text_inverse)
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
    fn escape_clears_spreadsheet_search_before_closing_viewer() {
        let data = SpreadsheetData {
            title: "Escape filter fixture".into(),
            sheets: vec![SpreadsheetSheet::new(
                "Data".into(),
                vec![vec!["Name".into()], vec!["Needle".into()]],
            )],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        state.search_query = "needle".into();
        let palette = little_clip_core::palette::AppPalette::default();
        let ctx = egui::Context::default();
        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut close = false;

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 620.0),
                )),
                events: vec![escape()],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        assert!(state.search_query.is_empty());
        assert!(
            !close,
            "the first Escape clears the filter and keeps the viewer open"
        );

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(900.0, 620.0),
                )),
                events: vec![escape()],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            },
        );
        assert!(close, "the next Escape closes with an empty filter");
    }

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
        let palette = little_clip_core::palette::AppPalette::default();
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
    fn spreadsheet_grid_scrolls_horizontally_and_vertically() {
        let rows = (0..160)
            .map(|row| {
                (0..80)
                    .map(|column| format!("r{row}c{column}"))
                    .collect::<Vec<_>>()
            })
            .collect();
        let data = SpreadsheetData {
            title: "Two-axis scroll".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        let palette = little_clip_core::palette::AppPalette::default();
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(860.0, 620.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            (output, state.scroll_offset)
        };

        let (first, first_offset) = render(Vec::new());
        let grid_clip = first
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "r0c0" => Some(clipped.clip_rect),
                _ => None,
            })
            .expect("initial grid cell should render");
        let initial_header = first
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "A (1)" => Some(text.pos),
                _ => None,
            })
            .expect("initial column header should render");
        let initial_gutter_clip_left = first
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "1" => {
                    Some(clipped.clip_rect.left())
                }
                _ => None,
            })
            .expect("initial row number should render");
        assert_eq!(first_offset, Vec2::ZERO);

        let _ = render(vec![
            egui::Event::PointerMoved(grid_clip.center()),
            // Spreadsheet code normalizes horizontal wheel direction before the
            // grid ScrollArea consumes it.
            egui::Event::Scroll(Vec2::new(800.0, -800.0)),
        ]);
        let mut settled = None;
        for _ in 0..8 {
            settled = Some(render(Vec::new()));
        }

        let (output, offset) = settled.unwrap();
        assert!(offset.x > 20.0, "x offset: {offset:?}");
        assert!(offset.y > 20.0, "y offset: {offset:?}");
        let (scrolled_row, scrolled_column, scrolled_cell) = output
            .shapes
            .iter()
            .find_map(|clipped| {
                let egui::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                let (row, column) = text.galley.text().strip_prefix('r')?.split_once('c')?;
                let row = row.parse::<usize>().ok()?;
                let column = column.parse::<usize>().ok()?;
                (row > 0 && column > 0).then_some((row, column, text.pos))
            })
            .expect("scrolled visible cell should have nonzero row and column");
        let header_label = format!("{} ({})", col_label(scrolled_column), scrolled_column + 1);
        let scrolled_header = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == header_label => Some(text.pos),
                _ => None,
            })
            .expect("visible scrolled column header should stay rendered");
        assert!(
            (scrolled_header.y - initial_header.y).abs() <= 1.0,
            "column header moved vertically: {initial_header:?} -> {scrolled_header:?}"
        );
        let gutter_label = (scrolled_row + 1).to_string();
        let scrolled_gutter = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == gutter_label => {
                    Some((text.pos, clipped.clip_rect))
                }
                _ => None,
            })
            .expect("row number for scrolled visible row should render");
        assert_eq!(
            scrolled_gutter.1.left(),
            initial_gutter_clip_left,
            "row-number gutter should remain pinned to the left"
        );
        assert!(
            (scrolled_gutter.0.y - scrolled_cell.y).abs() <= 12.0,
            "row number and visible cell are not aligned: {:?} vs {:?}",
            scrolled_gutter.0,
            scrolled_cell
        );
        assert!(
            output.shapes.iter().any(|clipped| {
                let egui::Shape::Text(text) = &clipped.shape else {
                    return false;
                };
                let Some((row, column)) = text
                    .galley
                    .text()
                    .strip_prefix('r')
                    .and_then(|cell| cell.split_once('c'))
                else {
                    return false;
                };
                row.parse::<usize>().is_ok_and(|row| row > 0)
                    && column.parse::<usize>().is_ok_and(|column| column > 0)
                    && text.pos.x < grid_clip.right()
                    && text.pos.y < grid_clip.bottom()
            }),
            "scrolled grid should continue rendering visible cells"
        );
    }

    #[test]
    fn spreadsheet_scroll_targets_stay_aligned_across_host_sizes_and_zoom() {
        let rows = (0..320)
            .map(|row| {
                (0..72)
                    .map(|column| format!("r{row}c{column}"))
                    .collect::<Vec<_>>()
            })
            .collect();
        let data = SpreadsheetData {
            title: "Zoomed scroll alignment".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
        };
        let palette = little_clip_core::palette::AppPalette::default();

        for (size, zoom) in [
            (Vec2::new(640.0, 520.0), 1.0),
            (Vec2::new(640.0, 520.0), 1.5),
            (Vec2::new(1180.0, 720.0), 1.0),
            (Vec2::new(1180.0, 720.0), 1.5),
        ] {
            let mut state = SpreadsheetViewerState::new(data.clone(), None);
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            ctx.set_zoom_factor(zoom);
            let mut time = 0.0;
            let mut render = |events: Vec<egui::Event>| {
                time += 0.05;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_ui(ui, ctx, &palette, &mut false);
                        });
                    },
                );
                (output, state.scroll_offset)
            };

            let (initial, initial_offset) = render(Vec::new());
            assert_eq!(initial_offset, Vec2::ZERO, "host={size:?}, zoom={zoom}");
            let cell_clip = initial
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "r0c0" => {
                        Some((text.pos + text.galley.size() * 0.5, clipped.clip_rect))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("initial cell missing at host={size:?}, zoom={zoom}"));
            let initial_header_y = initial
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "A (1)" => Some(text.pos.y),
                    _ => None,
                })
                .expect("initial column header should render");
            let gutter_left = initial
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "1" => {
                        Some(clipped.clip_rect.left())
                    }
                    _ => None,
                })
                .expect("initial row number should render");

            let (cell_center, grid_clip) = cell_clip;
            assert!(
                grid_clip.contains(cell_center),
                "initial cell center {cell_center:?} outside grid clip {grid_clip:?} at host={size:?}, zoom={zoom}"
            );
            let _ = render(vec![
                egui::Event::PointerMoved(cell_center),
                egui::Event::Scroll(Vec2::new(800.0, -800.0)),
            ]);
            let mut settled = None;
            for _ in 0..10 {
                settled = Some(render(Vec::new()));
            }
            let (output, offset) = settled.expect("scroll settling frames ran");
            assert!(
                offset.x > 10.0 && offset.y > 10.0,
                "two-axis scroll should move at host={size:?}, zoom={zoom}: {offset:?}"
            );

            let (row, column, cell_center) = output
                .shapes
                .iter()
                .find_map(|clipped| {
                    let egui::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    let (row, column) = text.galley.text().strip_prefix('r')?.split_once('c')?;
                    let row = row.parse::<usize>().ok()?;
                    let column = column.parse::<usize>().ok()?;
                    (row > 0 && column > 0).then_some((
                        row,
                        column,
                        text.pos + text.galley.size() * 0.5,
                    ))
                })
                .unwrap_or_else(|| {
                    panic!("no later cell visible at host={size:?}, zoom={zoom}, offset={offset:?}")
                });
            let header_label = format!("{} ({})", col_label(column), column + 1);
            let header = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == header_label => {
                        Some((text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("scrolled header {header_label:?} missing at host={size:?}, zoom={zoom}")
                });
            let header_center = header.0 + header.1 * 0.5;
            assert!(
                (header_center.x - cell_center.x).abs() <= 4.0,
                "cell/header x mismatch at host={size:?}, zoom={zoom}: cell={cell_center:?}, header={header_center:?}"
            );
            assert!(
                (header.0.y - initial_header_y).abs() <= 1.0,
                "sticky column header moved vertically at host={size:?}, zoom={zoom}"
            );

            let row_label = (row + 1).to_string();
            let gutter = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text)
                        if text.galley.text() == row_label
                            && clipped.clip_rect.left() == gutter_left =>
                    {
                        Some((text.pos, text.galley.size(), clipped.clip_rect))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("scrolled row label {row_label:?} missing at host={size:?}, zoom={zoom}")
                });
            let gutter_center = gutter.0 + gutter.1 * 0.5;
            assert_eq!(
                gutter.2.left(),
                gutter_left,
                "row-number gutter shifted horizontally at host={size:?}, zoom={zoom}"
            );
            assert!(
                (gutter_center.y - cell_center.y).abs() <= 14.0,
                "row gutter/cell y mismatch at host={size:?}, zoom={zoom}: gutter={gutter_center:?}, cell={cell_center:?}"
            );
        }
    }

    #[test]
    fn spreadsheet_scrolled_cell_selection_copies_exact_value_across_host_sizes_and_zoom() {
        let rows = (0..320)
            .map(|row| {
                (0..72)
                    .map(|column| format!("r{row}c{column}"))
                    .collect::<Vec<_>>()
            })
            .collect();
        let data = SpreadsheetData {
            title: "Zoomed cell selection".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
        };
        let palette = little_clip_core::palette::AppPalette::default();

        for (size, zoom) in [
            (Vec2::new(640.0, 520.0), 1.0),
            (Vec2::new(640.0, 520.0), 1.5),
            (Vec2::new(1180.0, 720.0), 1.0),
            (Vec2::new(1180.0, 720.0), 1.5),
        ] {
            let mut state = SpreadsheetViewerState::new(data.clone(), None);
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            ctx.set_zoom_factor(zoom);
            let mut time = 0.0;
            let mut render = |events: Vec<egui::Event>| {
                time += 0.05;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_ui(ui, ctx, &palette, &mut false);
                        });
                    },
                );
                (output, state.scroll_offset, state.selected_cell)
            };

            let (initial, _, _) = render(Vec::new());
            let first_cell_center = initial
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "r0c0" => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("initial cell missing at host={size:?}, zoom={zoom}"));
            let _ = render(vec![
                egui::Event::PointerMoved(first_cell_center),
                egui::Event::Scroll(Vec2::new(800.0, -800.0)),
            ]);
            let mut settled = None;
            for _ in 0..10 {
                settled = Some(render(Vec::new()));
            }
            let (scrolled, offset, _) = settled.expect("scroll settling frames ran");
            assert!(
                offset.x > 10.0 && offset.y > 10.0,
                "two-axis scroll should move at host={size:?}, zoom={zoom}: {offset:?}"
            );

            let (row, column, target_center, target_clip) = scrolled
                .shapes
                .iter()
                .find_map(|clipped| {
                    let egui::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    let (row, column) = text.galley.text().strip_prefix('r')?.split_once('c')?;
                    let row = row.parse::<usize>().ok()?;
                    let column = column.parse::<usize>().ok()?;
                    let center = text.pos + text.galley.size() * 0.5;
                    (row > 0 && column > 0 && clipped.clip_rect.contains(center)).then_some((
                        row,
                        column,
                        center,
                        clipped.clip_rect,
                    ))
                })
                .unwrap_or_else(|| {
                    panic!("no later target cell at host={size:?}, zoom={zoom}, offset={offset:?}")
                });
            assert!(
                target_clip.contains(target_center),
                "target center {target_center:?} outside clip {target_clip:?} at host={size:?}, zoom={zoom}"
            );
            let expected_value = format!("r{row}c{column}");
            let (pressed, _, _) = render(vec![
                egui::Event::PointerMoved(target_center),
                egui::Event::PointerButton {
                    pos: target_center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            let (released, _, selected_cell) = render(vec![
                egui::Event::PointerMoved(target_center),
                egui::Event::PointerButton {
                    pos: target_center,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            assert_eq!(
                selected_cell,
                Some((row, column)),
                "selection mismatch for {expected_value} at host={size:?}, zoom={zoom}"
            );
            assert_eq!(
                released.platform_output.copied_text, expected_value,
                "copied value mismatch at host={size:?}, zoom={zoom}"
            );
            assert!(pressed.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == expected_value)
            }));
        }
    }

    #[test]
    fn spreadsheet_search_highlights_visible_result_across_host_sizes_and_zoom() {
        let match_row = 137;
        let match_column = 2;
        let rows = (0..320)
            .map(|row| {
                (0..72)
                    .map(|column| {
                        if row == match_row && column == match_column {
                            "needle".to_owned()
                        } else {
                            format!("r{row}c{column}")
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let data = SpreadsheetData {
            title: "Zoomed search result".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
        };
        let palette = little_clip_core::palette::AppPalette::default();
        let highlight = crate::hex_to_c32(&palette.harmonies.warm_attention);

        for (size, zoom) in [
            (Vec2::new(640.0, 520.0), 1.0),
            (Vec2::new(640.0, 520.0), 1.5),
            (Vec2::new(1180.0, 720.0), 1.0),
            (Vec2::new(1180.0, 720.0), 1.5),
        ] {
            let mut state = SpreadsheetViewerState::new(data.clone(), None);
            // Use the same state path the search field updates; rendering applies
            // row filtering and the per-cell match background.
            state.search_query = "needle".into();
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            ctx.set_zoom_factor(zoom);
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );

            assert_eq!(state.data.sheets[0].rows.len(), 320);
            assert_eq!(state.filtered_rows, vec![0, match_row]);
            let target_center = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "needle" => {
                        let center = text.pos + text.galley.size() * 0.5;
                        clipped.clip_rect.contains(center).then_some(center)
                    }
                    _ => None,
                })
                .find(|center| {
                    output.shapes.iter().any(|clipped| {
                        matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == highlight && rect.rect.contains(*center))
                    })
                })
                .unwrap_or_else(|| {
                    panic!("matching cell should be visible and highlighted at host={size:?}, zoom={zoom}")
                });

            let highlight_rect = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(rect)
                        if rect.fill == highlight && rect.rect.contains(target_center) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("matching cell should have the search highlight background");
            let row_label = (match_row + 1).to_string();
            let gutter = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == row_label => {
                        let center = text.pos + text.galley.size() * 0.5;
                        clipped.clip_rect.contains(center).then_some(center)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("matching row number {row_label} should be visible at host={size:?}, zoom={zoom}")
                });
            assert!(
                (gutter.y - target_center.y).abs() <= 14.0,
                "search result and row label lost alignment at host={size:?}, zoom={zoom}: cell={target_center:?}, gutter={gutter:?}"
            );
            assert!(
                highlight_rect.contains(target_center),
                "highlight should cover the visible result at host={size:?}, zoom={zoom}"
            );
            assert!(state.scroll_offset.y == 0.0);
        }
    }

    #[test]
    fn selecting_long_cell_shows_and_copies_full_value() {
        let long_value = format!(
            "Long clipboard cell value: {}",
            "complete-value ".repeat(40)
        );
        let data = SpreadsheetData {
            title: "Long cell".into(),
            sheets: vec![SpreadsheetSheet::new(
                "Data".into(),
                vec![vec!["Header".into()], vec![long_value.clone()]],
            )],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        let palette = little_clip_core::palette::AppPalette::default();
        let ctx = egui::Context::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(760.0, 620.0));
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            let column_width = state.column_width_cache[0]
                .as_ref()
                .map(|widths| widths[0].clamp(90.0, 420.0));
            (output, column_width, state.selected_cell)
        };

        let (initial, measured_cell_width, _) = render(Vec::new());
        let (cell_text, text_width) = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == long_value => {
                    Some((text.pos + Vec2::splat(1.0), text.galley.size().x))
                }
                _ => None,
            })
            .expect("long cell value should be rendered");
        let measured_cell_width = measured_cell_width.unwrap();
        assert!(
            measured_cell_width < text_width,
            "test value must exceed the grid column width ({measured_cell_width} < {text_width})"
        );

        let (pressed, _, _) = render(vec![
            egui::Event::PointerMoved(cell_text),
            egui::Event::PointerButton {
                pos: cell_text,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let (copied, _, selected_cell) = render(vec![
            egui::Event::PointerMoved(cell_text),
            egui::Event::PointerButton {
                pos: cell_text,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(selected_cell, Some((1, 0)));
        assert_eq!(copied.platform_output.copied_text, long_value);
        assert!(pressed.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == long_value)
        }));

        let (selected, _, _) = render(Vec::new());
        let readout = format!("Selected R2 C1 [A]: {long_value}");
        assert!(
            selected.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == readout)
            }),
            "selected-cell footer should expose the complete value"
        );
    }

    #[test]
    fn spreadsheet_header_controls_fit_compact_and_wide_hosts() {
        let data = SpreadsheetData {
            title: "Sheet overview".into(),
            sheets: vec![
                SpreadsheetSheet::new("Overview".into(), vec![vec!["Name".into()]]),
                SpreadsheetSheet::new("Archive".into(), vec![vec!["Old name".into()]]),
            ],
        };
        let palette = little_clip_core::palette::AppPalette::default();

        for width in [900.0, 1500.0] {
            let mut state = SpreadsheetViewerState::new(data.clone(), None);
            let ctx = egui::Context::default();
            let host = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 620.0));
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(host),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );

            let required_labels = [
                "Sheet overview",
                "Copy Table",
                "Filter cells...",
                "Calc",
                "Close (Esc)",
                "Sheets:",
                "Overview",
                "Archive",
            ];
            let rendered = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => Some((
                            clipped.clip_rect,
                            egui::Rect::from_min_size(text.pos, text.galley.size()),
                        )),
                        _ => None,
                    })
            };
            let mut header_text_rects = Vec::new();
            for label in required_labels {
                let (clip_rect, text_rect) = rendered(label)
                    .unwrap_or_else(|| panic!("{label:?} was not rendered at host width {width}"));
                assert!(
                    host.contains_rect(text_rect),
                    "{label:?} extends outside the {width}-point host: {text_rect:?}"
                );
                assert!(
                    clip_rect.contains_rect(text_rect),
                    "{label:?} is clipped at host width {width}: text={text_rect:?}, clip={clip_rect:?}"
                );
                if matches!(
                    label,
                    "Sheet overview" | "Copy Table" | "Filter cells..." | "Calc" | "Close (Esc)"
                ) {
                    header_text_rects.push((label, text_rect));
                }
            }

            for (idx, (left_label, left_rect)) in header_text_rects.iter().enumerate() {
                for (right_label, right_rect) in header_text_rects.iter().skip(idx + 1) {
                    assert!(
                        !left_rect.intersects(*right_rect),
                        "header labels {left_label:?} and {right_label:?} overlap at {width} points"
                    );
                }
            }
        }
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
        let palette = little_clip_core::palette::AppPalette::default();
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
    fn large_virtual_sheet_bounds_rendered_cells_and_reaches_far_corner() {
        let column_count = 512;
        let rows = std::iter::once(
            (0..column_count)
                .map(|column| format!("Column {column}"))
                .collect(),
        )
        .chain((0..3000).map(|row| {
            (0..column_count)
                .map(|column| format!("r{row}c{column}"))
                .collect::<Vec<_>>()
        }))
        .collect();
        let data = SpreadsheetData {
            title: "Large virtual sheet".into(),
            sheets: vec![SpreadsheetSheet::new("Data".into(), rows)],
        };
        let mut state = SpreadsheetViewerState::new(data, None);
        let palette = little_clip_core::palette::AppPalette::default();
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(900.0, 620.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut false);
                    });
                },
            );
            (output, state.scroll_offset)
        };
        let is_grid_cell = |text: &str| {
            text.strip_prefix('r')
                .and_then(|cell| cell.split_once('c'))
                .and_then(|(row, column)| {
                    Some((row.parse::<usize>().ok()?, column.parse::<usize>().ok()?))
                })
                .is_some()
        };
        let grid_cells = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => is_grid_cell(text.galley.text()),
                    _ => false,
                })
                .count()
        };

        let (initial, initial_offset) = render(Vec::new());
        assert_eq!(initial_offset, Vec2::ZERO);
        let initial_count = grid_cells(&initial);
        assert!(initial_count > 0);
        assert!(
            initial_count <= 400,
            "rendered {initial_count} grid cells initially for a 3,001 × {column_count} sheet"
        );
        let first_cell_clip = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "r0c0" => Some(clipped.clip_rect),
                _ => None,
            })
            .expect("top-left grid cell should be visible initially");

        let _ = render(vec![
            egui::Event::PointerMoved(first_cell_clip.center()),
            // Spreadsheet code normalizes horizontal wheel direction before the
            // grid ScrollArea consumes it.
            egui::Event::Scroll(Vec2::new(100_000.0, -100_000.0)),
        ]);
        let mut settled = None;
        for _ in 0..16 {
            settled = Some(render(Vec::new()));
        }
        let (scrolled, offset) = settled.expect("scroll settling frames ran");
        let scrolled_count = grid_cells(&scrolled);
        assert!(scrolled_count > 0);
        assert!(
            scrolled_count <= 400,
            "rendered {scrolled_count} grid cells after scrolling for a 3,001 × {column_count} sheet"
        );
        assert!(
            offset.x > 10_000.0,
            "horizontal offset did not reach later columns: {offset:?}"
        );
        assert!(
            offset.y > 50_000.0,
            "vertical offset did not reach later rows: {offset:?}"
        );
        assert!(
            scrolled.shapes.iter().any(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => text.galley.text() == "r2999c511",
                _ => false,
            }),
            "far-corner cell should render after scrolling; offset={offset:?}"
        );
        println!(
            "large sheet rendered {initial_count} initial and {scrolled_count} scrolled cells; offset={offset:?}"
        );
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
        let palette = little_clip_core::palette::AppPalette::default();
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
