use super::*;

impl InlineSpreadsheetPreview {
    /// Creates a compact preview with a fixed finite host size.
    pub fn new(data: Arc<SpreadsheetData>, host_size: Vec2) -> Self {
        assert!(
            host_size.x.is_finite()
                && host_size.y.is_finite()
                && host_size.x > 0.0
                && host_size.y > 0.0,
            "inline spreadsheet preview requires a positive finite host size"
        );
        Self {
            data,
            host_size,
            id: egui::Id::new((
                "inline-spreadsheet-preview",
                NEXT_INLINE_SPREADSHEET_ID.fetch_add(1, Ordering::Relaxed),
            )),
            active_sheet_idx: 0,
            selected_cell: None,
            scroll_offset: Vec2::ZERO,
        }
    }

    /// Renders only cells intersecting the visible rows and columns.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(self.host_size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let data = Arc::clone(&self.data);
        let id = self.id;
        let mut active_sheet_idx = self
            .active_sheet_idx
            .min(data.sheets.len().saturating_sub(1));
        let mut selected_cell = self.selected_cell;
        let scroll_offset = self.scroll_offset;
        let output = ui.allocate_ui_at_rect(rect, |ui| {
            egui::Frame::none()
                .fill(crate::hex_to_c32(&palette.colors.surface_recessed))
                .stroke(Stroke::new(
                    1.0_f32,
                    crate::hex_to_c32(&palette.colors.surface_panel),
                ))
                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    if data.sheets.len() > 1 {
                        ui.push_id(id, |ui| {
                            let label_width = (ui.available_width()
                                - 2.0 * ui.spacing().interact_size.x
                                - 4.0 * ui.spacing().item_spacing.x)
                                .max(1.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(active_sheet_idx > 0, egui::Button::new("‹"))
                                    .clicked()
                                {
                                    active_sheet_idx -= 1;
                                    selected_cell = None;
                                }
                                let sheet = &data.sheets[active_sheet_idx];
                                ui.add_sized(
                                    Vec2::new(label_width, ui.spacing().interact_size.y),
                                    egui::Label::new(format!(
                                        "{}/{} · {}",
                                        active_sheet_idx + 1,
                                        data.sheets.len(),
                                        inline_sheet_label(&sheet.name)
                                    ))
                                    .truncate(true),
                                );
                                if ui
                                    .add_enabled(
                                        active_sheet_idx + 1 < data.sheets.len(),
                                        egui::Button::new("›"),
                                    )
                                    .clicked()
                                {
                                    active_sheet_idx += 1;
                                    selected_cell = None;
                                }
                            });
                        });
                    }

                    let Some(sheet) = data.sheets.get(active_sheet_idx) else {
                        ui.label("No sheet data available.");
                        return (Vec2::ZERO, active_sheet_idx, selected_cell);
                    };
                    if sheet.rows.is_empty() || sheet.total_cols == 0 {
                        ui.label("Sheet is empty.");
                        return (Vec2::ZERO, active_sheet_idx, selected_cell);
                    }

                    let preview_column_count = sheet.total_cols.min(INLINE_SPREADSHEET_MAX_COLUMNS);
                    let column_widths = vec![INLINE_SPREADSHEET_COLUMN_WIDTH; preview_column_count];
                    let col_offsets = column_offsets(&column_widths, 1.0);
                    let total_width = *col_offsets.last().unwrap_or(&0.0);
                    let row_stride = INLINE_SPREADSHEET_ROW_HEIGHT + 1.0;
                    let total_height = (sheet.rows.len() as f32 * row_stride - 1.0).max(0.0);
                    let mut clicked_cell = None;
                    let output = egui::ScrollArea::both()
                        .id_source(id)
                        .auto_shrink([false, false])
                        .scroll_offset(scroll_offset)
                        .show_viewport(ui, |ui, viewport| {
                            ui.set_min_size(Vec2::new(total_width, total_height));
                            let rows = visible_row_range(
                                viewport.min.y,
                                viewport.height(),
                                sheet.rows.len(),
                                row_stride,
                            );
                            let columns = visible_column_range(
                                viewport.min.x,
                                viewport.width(),
                                &col_offsets,
                            );
                            let origin = ui.max_rect().min;
                            for row_idx in rows {
                                let Some(row) = sheet.rows.get(row_idx) else {
                                    continue;
                                };
                                let row_fill = if row_idx == 0 {
                                    crate::hex_to_c32(&palette.colors.surface_panel)
                                } else {
                                    crate::hex_to_c32(&palette.colors.canvas)
                                };
                                for col_idx in columns.clone() {
                                    let cell_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            origin.x + col_offsets[col_idx],
                                            origin.y + row_idx as f32 * row_stride,
                                        ),
                                        Vec2::new(
                                            INLINE_SPREADSHEET_COLUMN_WIDTH,
                                            INLINE_SPREADSHEET_ROW_HEIGHT,
                                        ),
                                    );
                                    let visible_cell_rect = cell_rect.intersect(rect);
                                    let cell_id = id.with(("cell", row_idx, col_idx));
                                    let cell_response = ui.interact(
                                        visible_cell_rect,
                                        cell_id,
                                        egui::Sense::click(),
                                    );
                                    if cell_response.clicked() {
                                        clicked_cell = Some((row_idx, col_idx));
                                    }
                                    let is_selected = selected_cell == Some((row_idx, col_idx));
                                    let fill = if is_selected {
                                        crate::hex_to_c32(&palette.harmonies.base)
                                    } else {
                                        row_fill
                                    };
                                    let cell_painter =
                                        ui.painter().with_clip_rect(visible_cell_rect);
                                    cell_painter.rect_filled(cell_rect, Rounding::ZERO, fill);
                                    cell_painter.rect_stroke(
                                        cell_rect,
                                        Rounding::ZERO,
                                        Stroke::new(
                                            0.5_f32,
                                            crate::hex_to_c32(&palette.colors.surface_panel),
                                        ),
                                    );
                                    let text_color = crate::contrast_text_c32(
                                        fill,
                                        Some(crate::hex_to_c32(&palette.colors.text_primary)),
                                        palette,
                                    );
                                    cell_painter.text(
                                        cell_rect.left_center() + egui::vec2(6.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        inline_cell_display_text(
                                            row.get(col_idx).map(String::as_str).unwrap_or(""),
                                        ),
                                        FontId::proportional(BODY_FONT_SIZE),
                                        text_color,
                                    );
                                }
                            }
                            clicked_cell
                        });
                    if output.inner.is_some() {
                        selected_cell = output.inner;
                    }
                    (output.state.offset, active_sheet_idx, selected_cell)
                })
                .inner
        });
        self.scroll_offset = output.inner.0;
        self.active_sheet_idx = output.inner.1;
        self.selected_cell = output.inner.2;
        response
    }
}
