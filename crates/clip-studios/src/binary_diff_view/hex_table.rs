use super::*;

impl BinaryDiffViewState {
    pub(super) fn render_hex_table(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        available_height: f32,
    ) {
        if self.is_computing {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.spinner();
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("Aligning binary sequences using Myers LCS diff...")
                        .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                        .color(crate::ui_ext::contrast_text_c32_on(
                            hex_to_c32(&palette.colors.canvas),
                            hex_to_c32(&palette.colors.canvas),
                            Some(hex_to_c32(&palette.harmonies.base)),
                            palette,
                        )),
                );
            });
            return;
        }

        let Some(res) = &self.result else {
            ui.label("No diff result available.");
            return;
        };
        let min_width = Self::natural_table_width(res.bytes_per_row) * Self::MIN_HEX_FONT_SIZE
            / crate::ui_ext::MONO_FONT_SIZE;
        let viewport_width = ui
            .available_rect_before_wrap()
            .intersect(ui.clip_rect())
            .width();
        let table_width = viewport_width.max(min_width);
        let visibility = if min_width > viewport_width {
            egui::scroll_area::ScrollBarVisibility::AlwaysVisible
        } else {
            egui::scroll_area::ScrollBarVisibility::AlwaysHidden
        };
        ui.scope(|ui| {
            ui.spacing_mut().scroll.floating = false;
            egui::ScrollArea::horizontal()
                .scroll_bar_visibility(visibility)
                .id_salt("binary_diff_hex_width")
                .max_width(viewport_width)
                .max_height(available_height)
                .auto_shrink([false, false])
                .min_scrolled_height(0.0)
                .show(ui, |ui| {
                    ui.set_min_width(table_width);
                    self.render_hex_columns(ui, palette, table_width);
                });
        });
    }

    fn render_hex_columns(&mut self, ui: &mut egui::Ui, palette: &AppPalette, table_width: f32) {
        let Some(res) = &self.result else {
            return;
        };
        let (cell_w, _cell_h, font_size, ascii_char_w, offset_w, row_height) =
            self.layout_metrics(res.bytes_per_row, table_width);
        let row_height = row_height.max(ui.spacing().interact_size.y);
        let total_rows = res.rows.len();
        let bpr = res.bytes_per_row;

        let center_w = 42.0;
        let scale = font_size / crate::ui_ext::MONO_FONT_SIZE;
        let hex_group_space = 5.0 * scale;
        let num_gaps = if bpr == 8 {
            1
        } else {
            (bpr / 8).saturating_sub(1)
        };
        let hex_w = bpr as f32 * cell_w
            + (bpr.saturating_sub(1) as f32 * 2.0 * scale)
            + (num_gaps as f32 * (hex_group_space + 2.0 * scale));
        let ascii_w = bpr as f32 * ascii_char_w + (bpr.saturating_sub(1) as f32 * 2.0 * scale);

        render_hex_table_header(
            ui,
            palette,
            res,
            HexHeaderColumns {
                offset: offset_w,
                hex: hex_w,
                ascii: ascii_w,
                diff: center_w,
            },
            font_size,
            scale,
        );

        ui.separator();

        let scroll_target = self.nav.scroll_target_row.take();
        let mut hovered = self.hovered_cell;

        let rem_h = ui.available_height();
        let mut scroll_area = egui::ScrollArea::vertical()
            .id_salt("binary_diff_hex_scroll")
            .auto_shrink([false, false])
            .max_height(rem_h)
            // The minimap remains the vertical navigator.
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);

        if let Some(target) = scroll_target {
            let target_scroll_row = target.saturating_sub(1);
            let target_px = target_scroll_row as f32 * row_height;
            scroll_area = scroll_area.vertical_scroll_offset(target_px);
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().scroll.bar_width = 12.0;

        let output = scroll_area.show_rows(ui, row_height, total_rows, |ui, row_range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row_idx in row_range {
                let row = res
                    .rows
                    .get(row_idx)
                    .expect("visible row is within row count");

                let is_search_match = self.nav.search_matches.contains(&row_idx);
                let is_selected_diff = self
                    .nav
                    .selected_diff_block
                    .and_then(|id| res.diff_blocks.summaries().get(id.saturating_sub(1)))
                    .is_some_and(|block| (block.start_row..=block.end_row).contains(&row_idx));

                // Row background
                let bg_color = if is_selected_diff {
                    hex_to_c32_alpha(&palette.harmonies.base, 45)
                } else if is_search_match {
                    hex_to_c32_alpha(&palette.harmonies.base, 25)
                } else if row.has_diff {
                    hex_to_c32_alpha(&palette.diff_mod, 18)
                } else if row_idx % 2 == 1 {
                    hex_to_c32_alpha(&palette.colors.text_muted, 4)
                } else {
                    Color32::TRANSPARENT
                };

                let row_rect = ui.available_rect_before_wrap();
                let actual_row_rect = egui::Rect::from_min_size(
                    row_rect.min,
                    Vec2::new(row_rect.width(), row_height),
                );
                ui.painter()
                    .rect_filled(actual_row_rect, CornerRadius::ZERO, bg_color);

                if is_selected_diff {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(actual_row_rect.min, Vec2::new(3.5, row_height)),
                        CornerRadius::ZERO,
                        hex_to_c32(&palette.harmonies.base),
                    );
                    ui.painter().line_segment(
                        [actual_row_rect.left_top(), actual_row_rect.right_top()],
                        Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.harmonies.base, 100)),
                    );
                    ui.painter().line_segment(
                        [
                            actual_row_rect.left_bottom(),
                            actual_row_rect.right_bottom(),
                        ],
                        Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.harmonies.base, 100)),
                    );
                }

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0 * scale, 0.0);
                    ui.add_space(crate::ui_ext::SPACE_XS * scale);

                    // 1. Left Offset
                    let l_off_str = if let Some(off) = row.left_offset {
                        format!("{:08X} ", off)
                    } else {
                        "         ".to_string()
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(offset_w, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(offset_w);
                            ui.label(
                                egui::RichText::new(l_off_str)
                                    .font(FontId::monospace(font_size))
                                    .color(hex_to_c32(&palette.colors.text_muted)),
                            );
                        },
                    );

                    let mut bytes = |ui: &mut egui::Ui, cells: &[AlignedCell], is_left| {
                        for (col_idx, cell) in cells.iter().enumerate() {
                            if col_idx > 0
                                && ((bpr == 8 && col_idx == 4) || (bpr != 8 && col_idx % 8 == 0))
                            {
                                ui.add_space(hex_group_space);
                            }

                            let (text, color, pill_bg) =
                                format_cell_hex(cell, is_left, self.color_mode, palette);
                            let is_cell_hovered = hovered == Some((row_idx, col_idx));

                            let (rect, r) = ui.allocate_exact_size(
                                Vec2::new(cell_w, row_height),
                                egui::Sense::hover(),
                            );

                            if let Some(pill) = pill_bg {
                                ui.painter().rect_filled(
                                    rect.expand(0.5),
                                    CornerRadius::same(2),
                                    pill,
                                );
                            }

                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                &text,
                                FontId::monospace(font_size),
                                color,
                            );

                            if is_cell_hovered {
                                ui.painter().rect_stroke(
                                    rect.expand(1.0),
                                    CornerRadius::same(2),
                                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                                    egui::StrokeKind::Middle,
                                );
                            }

                            if r.hovered() {
                                hovered = Some((row_idx, col_idx));
                            }
                        }

                        ui.add_space(crate::ui_ext::SPACE_SM * scale);

                        for (col_idx, cell) in cells.iter().enumerate() {
                            let (ch, color) =
                                format_cell_ascii(cell, is_left, self.color_mode, palette);
                            let is_cell_hovered = hovered == Some((row_idx, col_idx));

                            let (rect, r) = ui.allocate_exact_size(
                                Vec2::new(ascii_char_w, row_height),
                                egui::Sense::hover(),
                            );

                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                ch.to_string(),
                                FontId::monospace(font_size),
                                color,
                            );

                            if is_cell_hovered {
                                ui.painter().rect_stroke(
                                    rect.expand(1.0),
                                    CornerRadius::same(2),
                                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                                    egui::StrokeKind::Middle,
                                );
                            }

                            if r.hovered() {
                                hovered = Some((row_idx, col_idx));
                            }
                        }
                    };
                    bytes(ui, &row.left_cells, true);

                    // 4. Center Match Glyph
                    let glyph_color = match row.row_glyph {
                        "≠" => hex_to_c32(&palette.diff_mod),
                        "+" => hex_to_c32(&palette.diff_add),
                        "-" => hex_to_c32(&palette.diff_del),
                        "~" => hex_to_c32(&palette.harmonies.triadic_1),
                        _ => hex_to_c32(&palette.colors.text_muted),
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(center_w, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(center_w);
                            ui.label(
                                egui::RichText::new(format!(" {} ", row.row_glyph))
                                    .font(FontId::monospace(font_size))
                                    .color(glyph_color),
                            );
                        },
                    );

                    bytes(ui, &row.right_cells, false);

                    // 7. Right Offset
                    let r_off_str = if let Some(off) = row.right_offset {
                        format!("{:08X}", off)
                    } else {
                        "        ".to_string()
                    };
                    ui.allocate_ui_with_layout(
                        Vec2::new(offset_w, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(r_off_str)
                                    .font(FontId::monospace(font_size))
                                    .color(hex_to_c32(&palette.colors.text_muted)),
                            );
                        },
                    );
                });
            }
        });

        self.current_scroll_offset_y = output.state.clamped_offset().y;
        self.visible_row_count = (output.inner_rect.height() / row_height) as usize;
        self.hovered_cell = hovered;
    }
}
