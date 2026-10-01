use super::*;

impl BinaryDiffViewState {
    pub(super) fn render_minimap(&mut self, ui: &mut egui::Ui, palette: &AppPalette, avail_h: f32) {
        let Some(ref res) = self.result else {
            return;
        };

        let total_rows = res.rows.len();
        if total_rows == 0 {
            return;
        }

        let width = 16.0;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(width, avail_h - 10.0),
            egui::Sense::click_and_drag(),
        );

        // Background groove
        ui.painter().rect_filled(
            rect,
            Rounding::same(3.0),
            hex_to_c32(&palette.colors.surface_recessed),
        );
        ui.painter().rect_stroke(
            rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
        );

        // Difference markers
        for block in &res.diff_blocks {
            let start_frac = block.start_row as f32 / total_rows as f32;
            let end_frac = (block.end_row + 1) as f32 / total_rows as f32;
            let y_start = rect.top() + start_frac * rect.height();
            let y_end = rect.top() + end_frac * rect.height();
            let h = (y_end - y_start).max(2.5);

            let marker_rect = egui::Rect::from_min_size(
                egui::Pos2::new(rect.left() + 2.0, y_start),
                Vec2::new(width - 4.0, h),
            );

            let marker_color = match block.kind {
                DiffKind::LeftOnly => hex_to_c32(&palette.diff_del),
                DiffKind::RightOnly => hex_to_c32(&palette.diff_add),
                _ => hex_to_c32(&palette.harmonies.warm_attention),
            };

            ui.painter()
                .rect_filled(marker_rect, Rounding::same(1.0), marker_color);
        }

        // Viewport position indicator
        let (_cell_w, _cell_h, _font_size, _ascii_w, _offset_w, row_height) =
            self.layout_metrics(res.bytes_per_row, ui.available_width());
        let total_height = (total_rows as f32 * row_height).max(1.0);
        let scroll_frac = (self.current_scroll_offset_y / total_height).clamp(0.0, 1.0);
        let visible_frac = (avail_h / total_height).clamp(0.03, 1.0);

        let view_y = rect.top() + scroll_frac * rect.height();
        let view_h = (visible_frac * rect.height()).max(14.0);
        let view_rect = egui::Rect::from_min_size(
            egui::Pos2::new(rect.left() + 1.0, view_y.min(rect.bottom() - view_h)),
            Vec2::new(width - 2.0, view_h),
        );

        ui.painter().rect_filled(
            view_rect,
            Rounding::same(2.0),
            hex_to_c32_alpha(&palette.colors.text_primary, 25),
        );
        ui.painter().rect_stroke(
            view_rect,
            Rounding::same(2.0),
            Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
        );

        // Click / Drag to jump
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_y = (pos.y - rect.top()).clamp(0.0, rect.height());
                let frac = rel_y / rect.height();
                let target_row =
                    ((frac * total_rows as f32) as usize).min(total_rows.saturating_sub(1));
                self.scroll_target_row = Some(target_row);
                ui.ctx().request_repaint();
            }
        }

        // Tooltip on hover
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            if let Some(pos) = response.hover_pos() {
                let rel_y = (pos.y - rect.top()).clamp(0.0, rect.height());
                let frac = rel_y / rect.height();
                let hover_row =
                    ((frac * total_rows as f32) as usize).min(total_rows.saturating_sub(1));
                let approx_off = hover_row * res.bytes_per_row;
                response.on_hover_ui_at_pointer(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Jump to ~0x{:08X} (Row {})",
                            approx_off,
                            hover_row + 1
                        ))
                        .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                        .color(hex_to_c32(&palette.colors.text_primary)),
                    );
                });
            }
        }
    }

    pub(super) fn render_differences_panel(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        avail_h: f32,
        panel_width: f32,
    ) {
        let Some(ref res) = self.result else {
            return;
        };

        let mut do_next = false;
        let mut do_prev = false;

        ui.allocate_ui_with_layout(
            Vec2::new(panel_width, avail_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                // Header bar
                egui::Frame::none()
                    .fill(hex_to_c32(&palette.colors.surface_recessed))
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("Diffs ({})", res.diff_blocks.len()))
                                    .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                                    .strong()
                                    .color(crate::ui_ext::contrast_text_c32_on(
                                        hex_to_c32(&palette.colors.surface_recessed),
                                        hex_to_c32(&palette.colors.surface_recessed),
                                        Some(hex_to_c32(&palette.diff_mod)),
                                        palette,
                                    )),
                            );

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let next_btn = crate::ui_ext::icon_button(
                                        Icon::ArrowDown,
                                        14.0,
                                        hex_to_c32(&palette.colors.surface_panel),
                                        None,
                                        palette,
                                    );
                                    if ui
                                        .add(next_btn)
                                        .on_hover_text("Next Difference [J]")
                                        .clicked()
                                    {
                                        do_next = true;
                                    }
                                    let prev_btn = crate::ui_ext::icon_button(
                                        Icon::ArrowUp,
                                        14.0,
                                        hex_to_c32(&palette.colors.surface_panel),
                                        None,
                                        palette,
                                    );
                                    if ui
                                        .add(prev_btn)
                                        .on_hover_text("Previous Difference [K]")
                                        .clicked()
                                    {
                                        do_prev = true;
                                    }
                                },
                            );
                        });
                    });

                ui.separator();

                if res.diff_blocks.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Files identical")
                                .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                                .color(crate::ui_ext::contrast_text_c32_on(
                                    hex_to_c32(&palette.colors.canvas),
                                    hex_to_c32(&palette.colors.canvas),
                                    Some(hex_to_c32(&palette.diff_add)),
                                    palette,
                                )),
                        );
                    });
                    return;
                }

                // Scrollable list of diff cards
                let rem_h = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_source("binary_diff_cards_scroll")
                    .auto_shrink([false, false])
                    .max_height(rem_h)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.spacing_mut().scroll.bar_width = 6.0;
                        let card_margin_x = 6.0;
                        let card_outer_w = (ui.available_width() - 10.0).max(60.0);
                        let card_inner_w = (card_outer_w - 2.0 * card_margin_x).max(40.0);

                        let mut clicked_row = None;
                        let mut clicked_id = None;

                        for block in &res.diff_blocks {
                            let is_selected = self.selected_diff_block == Some(block.id);

                            let badge_color = match block.kind {
                                DiffKind::Mismatch => hex_to_c32(&palette.diff_mod),
                                DiffKind::LeftOnly => hex_to_c32(&palette.diff_del),
                                DiffKind::RightOnly => hex_to_c32(&palette.diff_add),
                                DiffKind::Match => hex_to_c32(&palette.colors.text_secondary),
                            };
                            let card_fill = if is_selected {
                                hex_to_c32_alpha(&palette.harmonies.base, 35)
                            } else {
                                hex_to_c32(&palette.colors.surface_recessed)
                            };
                            let badge_text = crate::ui_ext::contrast_text_c32_on(
                                card_fill,
                                hex_to_c32(&palette.colors.canvas),
                                Some(badge_color),
                                palette,
                            );

                            let frame = egui::Frame::none()
                                .fill(card_fill)
                                .stroke(if is_selected {
                                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base))
                                } else {
                                    Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel))
                                })
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .inner_margin(egui::Margin {
                                    left: card_margin_x,
                                    right: card_margin_x,
                                    top: 4.0,
                                    bottom: 4.0,
                                });

                            ui.push_id(block.id, |ui| {
                                let resp = frame.show(ui, |ui| {
                                    ui.set_width(card_inner_w);
                                    ui.set_max_width(card_inner_w);
                                    ui.vertical(|ui| {
                                        // Row 1: ID badge + Kind + byte count
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("#{}", block.id))
                                                    .font(FontId::monospace(
                                                        crate::ui_ext::MONO_FONT_SIZE,
                                                    ))
                                                    .strong()
                                                    .color(hex_to_c32(
                                                        &palette.colors.text_secondary,
                                                    )),
                                            );

                                            ui.label(
                                                egui::RichText::new(&block.kind_label)
                                                    .font(FontId::proportional(
                                                        crate::ui_ext::BODY_FONT_SIZE,
                                                    ))
                                                    .strong()
                                                    .color(badge_text),
                                            );

                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{} B",
                                                            block.byte_count
                                                        ))
                                                        .font(FontId::monospace(
                                                            crate::ui_ext::MONO_FONT_SIZE,
                                                        ))
                                                        .strong()
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                    );
                                                },
                                            );
                                        });

                                        // Row 2: Offset
                                        let off_str = match (block.left_offset, block.right_offset)
                                        {
                                            (Some(l), Some(r)) if l == r => format!("0x{:06X}", l),
                                            (Some(l), Some(r)) => {
                                                format!("L:0x{:04X} R:0x{:04X}", l, r)
                                            }
                                            (Some(l), None) => format!("L: 0x{:06X}", l),
                                            (None, Some(r)) => format!("R: 0x{:06X}", r),
                                            (None, None) => "Unknown".to_string(),
                                        };
                                        ui.label(
                                            egui::RichText::new(off_str)
                                                .font(FontId::monospace(
                                                    crate::ui_ext::MONO_FONT_SIZE,
                                                ))
                                                .color(hex_to_c32(&palette.colors.text_muted)),
                                        );

                                        // Row 3: Preview
                                        ui.label(
                                            egui::RichText::new(&block.preview)
                                                .font(FontId::monospace(
                                                    crate::ui_ext::MONO_FONT_SIZE,
                                                ))
                                                .color(hex_to_c32(&palette.colors.text_primary)),
                                        );
                                    });
                                });

                                if is_selected && self.last_scrolled_diff_id != Some(block.id) {
                                    resp.response.scroll_to_me(Some(egui::Align::Center));
                                }

                                let interact = ui.interact(
                                    resp.response.rect,
                                    resp.response.id,
                                    egui::Sense::click(),
                                );
                                if interact.clicked() {
                                    clicked_row = Some(block.start_row);
                                    clicked_id = Some(block.id);
                                    if let Some(idx) =
                                        res.diff_blocks.iter().position(|b| b.id == block.id)
                                    {
                                        self.current_diff_idx = idx;
                                    }
                                }
                                if interact.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                ui.add_space(2.0);
                            });
                        }

                        if let Some(target) = clicked_row {
                            self.scroll_target_row = Some(target);
                            self.selected_diff_block = clicked_id;
                            ui.ctx().request_repaint();
                        }
                        self.last_scrolled_diff_id = self.selected_diff_block;
                    });
            },
        );

        if do_next {
            self.next_diff();
        }
        if do_prev {
            self.prev_diff();
        }
    }

    pub(super) fn render_footer_inspection(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        if let (Some(res), Some((row_idx, col_idx))) = (&self.result, self.hovered_cell) {
            if let Some(row) = res.rows.get(row_idx) {
                let l_cell = row.left_cells.get(col_idx);
                let r_cell = row.right_cells.get(col_idx);

                egui::Frame::none()
                    .fill(hex_to_c32(&palette.colors.surface_recessed))
                    .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)))
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .show(ui, |ui| {
                        let inspection_surface = hex_to_c32(&palette.colors.surface_recessed);
                        let inspection_text = |preferred| {
                            crate::ui_ext::contrast_text_c32_on(
                                inspection_surface,
                                inspection_surface,
                                Some(preferred),
                                palette,
                            )
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new("INSPECT: ")
                                    .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                                    .strong()
                                    .color(inspection_text(hex_to_c32(&palette.harmonies.base))),
                            );

                            // Left cell details
                            if let Some(c) = l_cell {
                                if let Some(b) = c.byte_val {
                                    let off_str = c.offset.map(|o| format!("0x{:08X}", o)).unwrap_or_else(|| "none".to_string());
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "LEFT: 0x{:02X} (Dec: {}, Bin: {:08b}, ASCII: '{}', Off: {})",
                                            b, b, b, byte_to_ascii_char(b), off_str
                                        ))
                                        .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                        .color(hex_to_c32(&palette.colors.text_primary)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("LEFT: <GAP (Deletion)>")
                                            .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                            .color(inspection_text(hex_to_c32(&palette.diff_del))),
                                    );
                                }
                            }

                            ui.separator();

                            // Diff status
                            let (kind_txt, kind_col) = match (l_cell.map(|c| c.kind), r_cell.map(|c| c.kind)) {
                                (Some(DiffKind::Match), _) => ("MATCH (=)", hex_to_c32(&palette.diff_add)),
                                (Some(DiffKind::Mismatch), _) => ("MISMATCH (≠)", hex_to_c32(&palette.diff_mod)),
                                (Some(DiffKind::LeftOnly), _) => ("LEFT ONLY (DELETION)", hex_to_c32(&palette.diff_del)),
                                (Some(DiffKind::RightOnly), _) => ("RIGHT ONLY (INSERTION)", hex_to_c32(&palette.diff_add)),
                                _ => ("UNKNOWN", hex_to_c32(&palette.colors.text_secondary)),
                            };
                            ui.label(
                                egui::RichText::new(format!("DIFF: {}", kind_txt))
                                    .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                    .strong()
                                    .color(inspection_text(kind_col)),
                            );

                            ui.separator();

                            // Right cell details
                            if let Some(c) = r_cell {
                                if let Some(b) = c.byte_val {
                                    let off_str = c.offset.map(|o| format!("0x{:08X}", o)).unwrap_or_else(|| "none".to_string());
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "RIGHT: 0x{:02X} (Dec: {}, Bin: {:08b}, ASCII: '{}', Off: {})",
                                            b, b, b, byte_to_ascii_char(b), off_str
                                        ))
                                        .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                        .color(hex_to_c32(&palette.colors.text_primary)),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("RIGHT: <GAP (Insertion)>")
                                            .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                            .color(inspection_text(hex_to_c32(&palette.diff_add))),
                                    );
                                }
                            }
                        });
                    });
            }
        }
    }
}
