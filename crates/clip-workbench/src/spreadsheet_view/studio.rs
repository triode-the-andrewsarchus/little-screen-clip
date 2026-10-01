use super::*;

impl SpreadsheetViewerState {
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
