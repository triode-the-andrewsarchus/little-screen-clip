use eframe::egui::{self, Color32, Rounding, Stroke, Vec2};
use little_clip_ops::spreadsheet::{SpreadsheetData, SpreadsheetSheet};
use std::path::PathBuf;

pub struct SpreadsheetViewerState {
    pub title: String,
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
    pub data: SpreadsheetData,
    pub active_sheet_idx: usize,
    pub search_query: String,
    pub selected_cell: Option<(usize, usize)>, // (row, col)
    pub status_toast: Option<(String, f64)>,
}

impl SpreadsheetViewerState {
    pub fn new(data: SpreadsheetData, path: Option<PathBuf>) -> Self {
        let title = data.title.clone();
        Self {
            title,
            path,
            data,
            active_sheet_idx: 0,
            search_query: String::new(),
            selected_cell: None,
            status_toast: None,
        }
    }

    pub fn current_sheet(&self) -> Option<&SpreadsheetSheet> {
        self.data.sheets.get(self.active_sheet_idx)
    }

    pub fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        // Escape hotkey to close
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close = true;
        }

        let sheet_count = self.data.sheets.len();
        let total_rows = self.current_sheet().map(|s| s.total_rows).unwrap_or(0);
        let total_cols = self.current_sheet().map(|s| s.total_cols).unwrap_or(0);

        egui::Frame::none()
            .fill(Color32::from_rgb(15, 23, 42)) // Slate 900
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                // 1. Header Toolbar
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("📊")
                            .size(24.0),
                    );
                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(18.0)
                            .color(Color32::WHITE),
                    );

                    ui.add_space(8.0);
                    let info_badge = format!("{} rows × {} cols", total_rows, total_cols);
                    ui.label(
                        egui::RichText::new(info_badge)
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );

                    // Search box
                    ui.add_space(16.0);
                    ui.label(egui::RichText::new("🔍").small());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Filter cells...")
                            .desired_width(180.0),
                    );

                    // Right-aligned actions & close button
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui::RichText::new("✕ Close (Esc)")
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(220, 38, 38))
                        .rounding(Rounding::same(6.0));
                        if ui.add(close_btn).clicked() {
                            *close = true;
                        }

                        // Copy Table / Selected Cell button
                        let copy_btn = egui::Button::new(
                            egui::RichText::new("📋 Copy Table")
                                .small()
                                .color(Color32::from_rgb(226, 232, 240)),
                        )
                        .fill(Color32::from_rgb(30, 41, 59))
                        .rounding(Rounding::same(4.0));
                        if ui.add(copy_btn).clicked() {
                            if let Some(sheet) = self.current_sheet() {
                                let csv_str = sheet
                                    .rows
                                    .iter()
                                    .map(|r| r.join("\t"))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                ui.output_mut(|o| o.copied_text = csv_str);
                                self.status_toast = Some(("Copied table as TSV".to_string(), now));
                            }
                        }

                        if let Some((msg, ts)) = &self.status_toast {
                            if now - ts < 4.0 {
                                ui.label(
                                    egui::RichText::new(msg)
                                        .small()
                                        .color(Color32::from_rgb(52, 211, 153)),
                                );
                            }
                        }
                    });
                });

                ui.add_space(8.0);

                // 2. Sheet Tabs (when > 1 sheet, or single sheet selector)
                if sheet_count > 1 {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("Sheets:").small().color(Color32::from_rgb(148, 163, 184)));
                        for (idx, sheet) in self.data.sheets.iter().enumerate() {
                            let is_active = idx == self.active_sheet_idx;
                            let tab_btn = egui::Button::new(
                                egui::RichText::new(&sheet.name)
                                    .small()
                                    .strong()
                                    .color(if is_active { Color32::WHITE } else { Color32::from_rgb(148, 163, 184) }),
                            )
                            .fill(if is_active { Color32::from_rgb(37, 99, 235) } else { Color32::from_rgb(30, 41, 59) })
                            .rounding(Rounding::same(4.0));

                            if ui.add(tab_btn).clicked() {
                                self.active_sheet_idx = idx;
                                self.selected_cell = None;
                            }
                        }
                    });
                    ui.add_space(8.0);
                }

                ui.separator();
                ui.add_space(8.0);

                // 3. Scrollable 2D X/Y Spreadsheet Table
                let query = self.search_query.trim().to_lowercase();

                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let Some(sheet) = self.current_sheet() else {
                            ui.label("No sheet data available.");
                            return;
                        };

                        if sheet.rows.is_empty() {
                            ui.label("Sheet is empty.");
                            return;
                        }

                        // Calculate column widths based on cell content (min 80.0, max 280.0)
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

                        let row_height = 26.0;

                        let mut clicked_cell: Option<(usize, usize, String)> = None;

                        ui.vertical(|ui| {
                            for (r_idx, row) in sheet.rows.iter().enumerate() {
                                // Search filtering
                                if !query.is_empty() {
                                    let matches_row = row.iter().any(|c| c.to_lowercase().contains(&query));
                                    if !matches_row && r_idx != 0 {
                                        continue;
                                    }
                                }

                                let is_header = r_idx == 0;
                                let row_bg = if is_header {
                                    Color32::from_rgb(30, 41, 59) // Slate 800
                                } else if r_idx % 2 == 1 {
                                    Color32::from_rgb(18, 26, 46)
                                } else {
                                    Color32::from_rgb(15, 23, 42)
                                };

                                ui.horizontal(|ui| {
                                    // Row index number column
                                    let row_label = if is_header { "#".to_string() } else { format!("{}", r_idx) };
                                    let idx_badge = egui::Button::new(
                                        egui::RichText::new(row_label)
                                            .small()
                                            .color(Color32::from_rgb(100, 116, 139)),
                                    )
                                    .fill(Color32::from_rgb(20, 29, 47))
                                    .min_size(Vec2::new(36.0, row_height))
                                    .stroke(Stroke::new(0.5, Color32::from_rgb(51, 65, 85)));
                                    ui.add(idx_badge);

                                    // Cells across columns
                                    for c_idx in 0..col_count {
                                        let cell_val = row.get(c_idx).map(|s| s.as_str()).unwrap_or("");
                                        let cell_w = col_widths.get(c_idx).copied().unwrap_or(90.0);

                                        let is_selected = self.selected_cell == Some((r_idx, c_idx));
                                        let is_match = !query.is_empty() && cell_val.to_lowercase().contains(&query);

                                        let cell_bg = if is_selected {
                                            Color32::from_rgb(59, 130, 246) // Bright blue
                                        } else if is_match {
                                            Color32::from_rgb(120, 53, 15) // Amber highlight
                                        } else {
                                            row_bg
                                        };

                                        let text_color = if is_header {
                                            Color32::from_rgb(56, 189, 248) // Sky blue
                                        } else if is_selected {
                                            Color32::WHITE
                                        } else {
                                            Color32::from_rgb(226, 232, 240)
                                        };

                                        let btn = egui::Button::new(
                                            egui::RichText::new(cell_val)
                                                .small()
                                                .strong()
                                                .color(text_color),
                                        )
                                        .fill(cell_bg)
                                        .min_size(Vec2::new(cell_w, row_height))
                                        .stroke(Stroke::new(0.5, Color32::from_rgb(51, 65, 85)))
                                        .rounding(Rounding::ZERO);

                                        let resp = ui.add(btn);
                                        if resp.clicked() {
                                            clicked_cell = Some((r_idx, c_idx, cell_val.to_string()));
                                        }
                                        if !cell_val.is_empty() {
                                            resp.on_hover_text(format!("Row {}, Col {}\nClick to copy: {}", r_idx, c_idx + 1, cell_val));
                                        }
                                    }
                                });
                            }
                        });

                        if let Some((r_idx, c_idx, val)) = clicked_cell {
                            self.selected_cell = Some((r_idx, c_idx));
                            ui.output_mut(|o| o.copied_text = val.clone());
                            self.status_toast = Some((format!("Copied: {}", val), now));
                        }
                    });
            });
    }
}
