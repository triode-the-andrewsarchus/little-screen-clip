use super::*;

impl ImagePreviewState {
    /// Renders the Left-Hand studio pane: Tool picker & color swatches.
    pub(crate) fn render_markup_left_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let base_accent = hex_to_c32(&palette.harmonies.base);

        ui.vertical(|ui| {
            ui.heading(
                egui::RichText::new("✏ Markup Tools")
                    .strong()
                    .size(15.0)
                    .color(f_high),
            );
            ui.label(
                egui::RichText::new("Vector screenshot markup tools")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.add_space(crate::ui_ext::SPACE_SM);

            ui.label(
                egui::RichText::new("SHAPES & TOOLS")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.add_space(crate::ui_ext::SPACE_XS);

            let tools = [
                MarkupToolType::Select,
                MarkupToolType::Arrow,
                MarkupToolType::Circle,
                MarkupToolType::Rectangle,
                MarkupToolType::Line,
                MarkupToolType::Highlighter,
                MarkupToolType::StepBadge,
                MarkupToolType::Text,
            ];

            for tool in tools {
                let is_active = self.markup_state.active_tool == tool;
                let btn = crate::ui_ext::mode_tab_button_icon(Some(tool.icon()), tool.label(), is_active, palette)
                    .min_size(Vec2::new(ui.available_width(), crate::ui_ext::CONTROL_HEIGHT));

                if ui.add(btn).clicked() {
                    self.markup_state.active_tool = tool;
                    if tool != MarkupToolType::Text {
                        self.markup_state.editing_text_idx = None;
                    }
                }
                ui.add_space(2.0);
            }

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

            ui.label(
                egui::RichText::new("COLOR PALETTE")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

            // Swatches grid: 3 columns
            egui::Grid::new("markup_color_swatches")
                .spacing(Vec2::new(6.0, 6.0))
                .show(ui, |ui| {
                    for (i, (name, col)) in MarkupState::PALETTE_SWATCHES.iter().enumerate() {
                        let is_active = self.markup_state.active_color == *col;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());

                        // Swatch background
                        ui.painter().rect_filled(rect, Rounding::same(4.0), *col);
                        // Swatch border
                        let border_stroke = if is_active {
                            Stroke::new(2.5_f32, base_accent)
                        } else {
                            Stroke::new(1.0_f32, b_med)
                        };
                        ui.painter().rect_stroke(rect, Rounding::same(4.0), border_stroke);

                        if resp.on_hover_text(*name).clicked() {
                            self.markup_state.active_color = *col;
                            if let Some(sel) = self.markup_state.selected_idx {
                                if sel < self.markup_state.items.len() {
                                    self.markup_state.items[sel].set_color(*col);
                                }
                            }
                        }

                        if (i + 1) % 3 == 0 {
                            ui.end_row();
                        }
                    }
                });

            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Custom:").size(crate::ui_ext::BODY_FONT_SIZE).color(f_med));
                let mut custom_c = self.markup_state.active_color;
                if egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut custom_c,
                    egui::color_picker::Alpha::Opaque,
                )
                .changed()
                {
                    self.markup_state.active_color = custom_c;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            self.markup_state.items[sel].set_color(custom_c);
                        }
                    }
                }
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

            ui.label(
                egui::RichText::new("SHORTCUTS")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.label(
                egui::RichText::new("• Del / Backspace: Delete item\n• Esc: Deselect / Exit edit\n• Enter: Flatten / Burn")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_muted)),
            );
        });
    }

    /// Renders the Right-Hand studio pane: Active shape properties, layer list, and burn action.
    pub(crate) fn render_markup_right_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);

        ui.vertical(|ui| {
            ui.heading(
                egui::RichText::new("Properties & Layers")
                    .strong()
                    .size(15.0)
                    .color(f_high),
            );
            ui.add_space(crate::ui_ext::SPACE_SM);

            // ── Section 1: Properties ──
            ui.label(
                egui::RichText::new("STYLE & SIZING")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.add_space(crate::ui_ext::SPACE_XS);

            // Stroke Width Slider
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Stroke:")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_med),
                );
                let mut stroke_w = self.markup_state.active_stroke_width;
                if ui
                    .add(egui::Slider::new(&mut stroke_w, 1.0..=32.0).suffix(" px"))
                    .changed()
                {
                    self.markup_state.active_stroke_width = stroke_w;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            self.markup_state.items[sel].set_width(stroke_w);
                        }
                    }
                }
            });

            // Font Size Slider
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Font:")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_med),
                );
                let mut font_sz = self.markup_state.active_font_size;
                if ui
                    .add(egui::Slider::new(&mut font_sz, 10.0..=200.0).suffix(" px"))
                    .changed()
                {
                    self.markup_state.active_font_size = font_sz;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            if let MarkupShape::Text { font_size, .. } =
                                &mut self.markup_state.items[sel]
                            {
                                *font_size = font_sz;
                            }
                        }
                    }
                }
            });

            // Fill Toggle (for circle / rect)
            ui.horizontal(|ui| {
                let mut filled = self.markup_state.active_filled;
                if ui.checkbox(&mut filled, "Fill shapes").changed() {
                    self.markup_state.active_filled = filled;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            match &mut self.markup_state.items[sel] {
                                MarkupShape::Circle { filled: f, .. }
                                | MarkupShape::Rectangle { filled: f, .. } => {
                                    *f = filled;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            });

            // Step Badge counter
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Next Badge:")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_med),
                );
                let next_num = self.markup_state.sync_next_badge_number();
                ui.label(
                    egui::RichText::new(format!("#{next_num}"))
                        .strong()
                        .color(f_high),
                );
                if ui
                    .button("Reset")
                    .on_hover_text("Reset next badge number to lowest available positive integer")
                    .clicked()
                {
                    self.markup_state.sync_next_badge_number();
                }
            });

            // Badge Size Slider
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Badge:")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_med),
                );
                let mut badge_r = self.markup_state.active_badge_radius;
                if ui
                    .add(egui::Slider::new(&mut badge_r, 12.0..=300.0).suffix(" px"))
                    .changed()
                {
                    self.markup_state.active_badge_radius = badge_r;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            if let MarkupShape::StepBadge { radius, .. } =
                                &mut self.markup_state.items[sel]
                            {
                                *radius = badge_r;
                            }
                        }
                    }
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

            // ── Section 2: Layers / Placed Items List ──
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("LAYERS")
                        .extra_letter_spacing(1.0)
                        .strong()
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_med),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{} items", self.markup_state.items.len()))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(f_med),
                    );
                });
            });
            ui.add_space(crate::ui_ext::SPACE_XS);

            let list_h = (ui.available_height() - 90.0).max(60.0);
            egui::ScrollArea::vertical()
                .max_height(list_h)
                .show(ui, |ui| {
                    if self.markup_state.items.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(20.0);
                            ui.label(
                                egui::RichText::new(
                                    "No markup shapes placed.\nClick & drag on image to draw.",
                                )
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(hex_to_c32(&palette.colors.text_muted)),
                            );
                        });
                    } else {
                        let mut to_remove = None;
                        let mut move_up = None;
                        let mut move_down = None;

                        for (i, item) in self.markup_state.items.iter().enumerate().rev() {
                            let is_selected = self.markup_state.selected_idx == Some(i);
                            let row_bg = if is_selected {
                                hex_to_c32(&palette.colors.surface_panel)
                            } else {
                                b_low
                            };

                            egui::Frame::none()
                                .fill(row_bg)
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if is_selected {
                                        hex_to_c32(&palette.harmonies.base)
                                    } else {
                                        b_med
                                    },
                                ))
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .inner_margin(egui::Margin::symmetric(
                                    crate::ui_ext::ITEM_SPACING_Y,
                                    crate::ui_ext::SPACE_XS,
                                ))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        // Shape icon & name
                                        let label_text = match item {
                                            MarkupShape::Text { text, .. } => {
                                                if text.is_empty() {
                                                    "Text Box".to_string()
                                                } else {
                                                    format!("\"{}\"", text)
                                                }
                                            }
                                            MarkupShape::StepBadge { number, .. } => {
                                                format!("Badge #{}", number)
                                            }
                                            _ => item.type_name().to_string(),
                                        };

                                        let icon_tint = if is_selected {
                                            hex_to_c32(&palette.harmonies.base)
                                        } else {
                                            f_high
                                        };
                                        ui.add(item.type_icon().image_tinted(14.0, icon_tint));

                                        if ui.selectable_label(is_selected, label_text).clicked() {
                                            self.markup_state.selected_idx = Some(i);
                                        }

                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add(crate::ui_ext::icon_button(
                                                        Icon::Trash,
                                                        12.0,
                                                        b_med,
                                                        None,
                                                        palette,
                                                    ))
                                                    .on_hover_text("Delete")
                                                    .clicked()
                                                {
                                                    to_remove = Some(i);
                                                }
                                                if i + 1 < self.markup_state.items.len() {
                                                    if ui
                                                        .add(crate::ui_ext::icon_button(
                                                            Icon::ArrowUp,
                                                            12.0,
                                                            b_med,
                                                            None,
                                                            palette,
                                                        ))
                                                        .on_hover_text("Bring forward")
                                                        .clicked()
                                                    {
                                                        move_up = Some(i);
                                                    }
                                                }
                                                if i > 0 {
                                                    if ui
                                                        .add(crate::ui_ext::icon_button(
                                                            Icon::ArrowDown,
                                                            12.0,
                                                            b_med,
                                                            None,
                                                            palette,
                                                        ))
                                                        .on_hover_text("Send backward")
                                                        .clicked()
                                                    {
                                                        move_down = Some(i);
                                                    }
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(2.0);
                        }

                        if let Some(idx) = to_remove {
                            self.markup_state.items.remove(idx);
                            if self.markup_state.selected_idx == Some(idx) {
                                self.markup_state.selected_idx = None;
                            } else if let Some(sel) = self.markup_state.selected_idx {
                                if sel > idx {
                                    self.markup_state.selected_idx = Some(sel - 1);
                                }
                            }
                            self.markup_state.sync_next_badge_number();
                        }
                        if let Some(idx) = move_up {
                            self.markup_state.items.swap(idx, idx + 1);
                            self.markup_state.selected_idx = Some(idx + 1);
                        }
                        if let Some(idx) = move_down {
                            self.markup_state.items.swap(idx, idx - 1);
                            self.markup_state.selected_idx = Some(idx - 1);
                        }
                    }
                });

            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

            // ── Section 3: Global Actions (Clear All & Flatten) ──
            ui.vertical_centered_justified(|ui| {
                let has_items = !self.markup_state.items.is_empty();

                // Burn / Flatten button
                let burn_bg = if has_items {
                    hex_to_c32(&palette.diff_del)
                } else {
                    b_med
                };
                let burn_btn = crate::ui_ext::action_button_icon(
                    Some(Icon::Check),
                    "Flatten / Burn Layers (Enter)",
                    burn_bg,
                    palette,
                );

                if ui
                    .add_enabled(has_items, burn_btn)
                    .on_hover_text(
                        "Permanently rasterizes vector markup layers into image pixels (Enter)",
                    )
                    .clicked()
                {
                    self.apply_flatten_markup();
                }

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Clear All button
                let clear_btn = crate::ui_ext::danger_button_icon(
                    Some(Icon::Close),
                    "Clear All",
                    b_low,
                    palette,
                );

                if ui.add_enabled(has_items, clear_btn).clicked() {
                    self.markup_state.items.clear();
                    self.markup_state.selected_idx = None;
                    self.markup_state.editing_text_idx = None;
                    self.markup_state.sync_next_badge_number();
                    self.show_toast("Cleared all markup items");
                }
            });
        });
    }
}
