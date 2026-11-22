use super::*;

impl SpreadsheetViewerState {
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
        close: &mut bool,
    ) {
        let now = crate::ui_ext::feedback_time();

        // Let Escape clear the filter before closing the spreadsheet viewer.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
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

        egui::Frame::NONE
            .fill(bg)
            .inner_margin(egui::Margin::from(crate::ui_ext::SPACE_MD))
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

                if let Some((r, c)) = self.selected_cell
                    && let Some(sheet) = self.data.sheets.get(self.active_sheet_idx)
                    && let Some(val) = sheet.rows.get(r).and_then(|row| row.get(c))
                {
                    shell = shell.with_footer_readout(format!(
                        "Selected R{} C{} [{}]: {}",
                        r + sheet.origin.0 + 1,
                        c + sheet.origin.1 + 1,
                        col_label(c + sheet.origin.1),
                        val
                    ));
                }

                let mut copy_table_clicked = false;
                let clicked_hatch = shell.render_header(ui, |ui| {
                    // Copy Table button
                    let copy_btn = clip_ui::buttons::ButtonRole::Secondary
                        .button_with_icon(Some(Icon::Copy), "Copy Table", palette)
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
                    let search_width = ui.available_width().clamp(0.0, 160.0);
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

                if copy_table_clicked
                    && let Some(sheet) = self.data.sheets.get(self.active_sheet_idx)
                {
                    let tsv = sheet
                        .rows
                        .iter()
                        .map(|r| r.join("\t"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(tsv);
                    self.status_toast.set("Copied table as TSV", now);
                }

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 2. Sheet Tabs
                if sheet_count > 1 {
                    let tabs: Vec<_> = self
                        .data
                        .sheets
                        .iter()
                        .enumerate()
                        .map(|(index, sheet)| crate::ui_tabs::Tab::new(index, &sheet.name))
                        .collect();
                    if let Some(index) = crate::ui_tabs::TabBar::new(
                        "spreadsheet_sheets",
                        self.active_sheet_idx,
                        &tabs,
                        palette,
                    )
                    .show(ui)
                    {
                        self.active_sheet_idx = index;
                        self.selected_cell = None;
                        self.scroll_offset = Vec2::ZERO;
                    }
                    ui.add_space(crate::ui_ext::SPACE_XS);
                }

                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_XS);

                // 3. Scrollable 2D X/Y Spreadsheet Table with Sticky Headers
                let Some(sheet) = self.data.sheets.get(self.active_sheet_idx) else {
                    ui.label("No sheet data available.");
                    let toast_tuple = self.status_toast.active(now).cloned();
                    shell.render_footer(ui, &toast_tuple, now);
                    return;
                };

                if sheet.total_rows == 0 || sheet.total_cols == 0 {
                    ui.label("Sheet is empty.");
                    let toast_tuple = self.status_toast.active(now).cloned();
                    shell.render_footer(ui, &toast_tuple, now);
                    return;
                }

                let query = self.search_query.trim().to_lowercase();
                if self.filtered_sheet_idx != Some(self.active_sheet_idx)
                    || self.filtered_query != query
                {
                    self.filtered_rows = (0..sheet.total_rows)
                        .filter(|&row| {
                            query.is_empty()
                                || row == 0
                                || sheet.rows.get(row).is_some_and(|cells| {
                                    cells
                                        .iter()
                                        .any(|cell| cell.to_lowercase().contains(&query))
                                })
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
                    ui.fonts_mut(|fonts| {
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
                        .corner_radius(CornerRadius::ZERO);
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
                        .rect_filled(col_header_rect, CornerRadius::ZERO, b_med);

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

                    let mut header_child_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(col_header_ui_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
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

                        let source_col = c_idx + sheet.origin.1;
                        let col_title = format!("{} ({})", col_label(source_col), source_col + 1);
                        let col_btn = egui::Button::new(
                            egui::RichText::new(col_title)
                                .small()
                                .strong()
                                .color(text_color),
                        )
                        .fill(col_bg)
                        .min_size(Vec2::new(cell_w, header_height))
                        .stroke(Stroke::new(0.5_f32, b_low))
                        .corner_radius(CornerRadius::ZERO);

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
                            source_col + 1,
                            col_label(source_col),
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
                        .rect_filled(row_strip_rect, CornerRadius::ZERO, b_low);

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

                    let mut row_child_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(row_strip_ui_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    row_child_ui.set_clip_rect(row_strip_rect);
                    row_child_ui.spacing_mut().item_spacing = table_spacing;

                    row_child_ui.skip_ahead_auto_ids(gutter_rows.start);
                    for &r_idx in &visible_rows[gutter_rows] {
                        let source_row = r_idx + sheet.origin.0 + 1;
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
                            egui::RichText::new(source_row.to_string())
                                .small()
                                .strong()
                                .color(text_color),
                        )
                        .fill(row_bg)
                        .min_size(Vec2::new(corner_width, row_height))
                        .stroke(Stroke::new(0.5_f32, b_med))
                        .corner_radius(CornerRadius::ZERO);

                        let resp = row_child_ui.add(row_btn);
                        if resp.clicked() {
                            clicked_row =
                                Some((r_idx, sheet.rows.get(r_idx).cloned().unwrap_or_default()));
                        }
                        resp.on_hover_text(format!("Row {source_row}\nClick to copy row"));
                    }

                    // Main Data Grid (ScrollArea::both)
                    // Invert horizontal scroll delta so horizontal scrolling moves naturally
                    ui.input_mut(|i| {
                        i.smooth_scroll_delta.x = -i.smooth_scroll_delta.x;
                    });

                    let scroll_output = egui::ScrollArea::both()
                        .id_salt(("sheet_grid", self.active_sheet_idx, &query))
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
                            let origin = ui.max_rect().min;
                            for cell in sheet.grid_cells(visible_rows, rows, columns) {
                                let (row, column) = cell.anchor;
                                let rect = grid_cell_rect(
                                    origin,
                                    &col_offsets,
                                    row_stride,
                                    table_spacing,
                                    &cell,
                                );
                                if !ui.is_rect_visible(rect) {
                                    continue;
                                }
                                let value = sheet.cell(row, column);
                                let selected = self.selected_cell == Some(cell.anchor);
                                let matching =
                                    !query.is_empty() && value.to_lowercase().contains(&query);
                                let fill = if selected {
                                    accent
                                } else if matching {
                                    warn_accent
                                } else if row == 0 {
                                    b_med
                                } else if row % 2 == 1 {
                                    b_low
                                } else {
                                    bg
                                };
                                let text = crate::contrast_text_c32(fill, Some(f_high), palette);
                                paint_grid_cell(
                                    ui.painter(),
                                    rect,
                                    fill,
                                    b_med,
                                    value,
                                    font_id.clone(),
                                    text,
                                );
                                let response = ui.interact(
                                    rect,
                                    ui.id().with(("cell", row, column)),
                                    egui::Sense::click(),
                                );
                                if response.clicked() {
                                    clicked_cell = Some((row, column, value.to_owned()));
                                }
                                if !value.is_empty() {
                                    let address = format!(
                                        "{}{}",
                                        col_label(column + sheet.origin.1),
                                        row + sheet.origin.0 + 1
                                    );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::Role::Button,
                                            response.enabled(),
                                            format!(
                                                "{address} {}",
                                                inline_cell_display_text(value)
                                            ),
                                        )
                                    });
                                    response.on_hover_text(format!(
                                        "{address}\nClick to copy: {value}"
                                    ));
                                }
                            }
                        });

                    // Restore horizontal scroll delta
                    ui.input_mut(|i| {
                        i.smooth_scroll_delta.x = -i.smooth_scroll_delta.x;
                    });

                    new_scroll_offset = Some(scroll_output.state.clamped_offset());
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
                    ui.ctx().copy_text(tsv);
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
                    ui.ctx().copy_text(col_text);
                    self.status_toast.set(
                        format!(
                            "Copied Column {} ({})",
                            c_idx + sheet.origin.1 + 1,
                            col_label(c_idx + sheet.origin.1)
                        ),
                        now,
                    );
                }

                if let Some((r_idx, row_cells)) = clicked_row {
                    self.selected_cell = Some((r_idx, 0));
                    let tsv = row_cells.join("\t");
                    ui.ctx().copy_text(tsv);
                    self.status_toast
                        .set(format!("Copied Row {}", r_idx + sheet.origin.0 + 1), now);
                }

                if let Some((r_idx, c_idx, val)) = clicked_cell {
                    self.selected_cell = Some((r_idx, c_idx));
                    ui.ctx().copy_text(val.clone());
                    self.status_toast.set(format!("Copied: {}", val), now);
                }

                // 4. Footer
                let toast_tuple = self.status_toast.active(now).cloned();
                shell.render_footer(ui, &toast_tuple, now);
            });
    }
}
