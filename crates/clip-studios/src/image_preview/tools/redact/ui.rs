use super::*;

impl ImagePreviewState {
    pub(crate) fn render_redact_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.vertical(|ui| {
            // Mode selector: Black Bar | Average Color | LaMa Inpaint
            ui.horizontal_wrapped(|ui| {
                let modes = [
                    (RedactToolType::BlackBar, "Black Bar"),
                    (RedactToolType::AverageColor, "Average Color"),
                    (RedactToolType::LamaInpaint, "LaMa Inpaint"),
                ];

                for (mode, label) in modes {
                    let is_active = self.redaction_state.tool_type == mode;
                    let btn = crate::ui_ext::mode_tab_button(label, is_active, palette);

                    if ui.add(btn).clicked() {
                        self.redaction_state.tool_type = mode;
                    }
                }
            });

            ui.add_space(crate::ui_ext::SPACE_SM);

            match self.redaction_state.tool_type {
                RedactToolType::BlackBar | RedactToolType::AverageColor => {
                    let title = match self.redaction_state.tool_type {
                        RedactToolType::BlackBar => "Privacy Redaction (Black Bar):",
                        RedactToolType::AverageColor => "Color Redaction (Average Color):",
                        _ => unreachable!(),
                    };
                    ui.label(
                        egui::RichText::new(title)
                            .strong()
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.diff_del)),
                    );

                    let box_count = self.redaction_state.boxes.len();
                    let count_str = if box_count == 0 {
                        "Drag on image to draw redaction box".to_string()
                    } else if box_count == 1 {
                        "1 bar placed".to_string()
                    } else {
                        format!("{} bars placed", box_count)
                    };
                    ui.label(
                        egui::RichText::new(count_str)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                    );

                    // Selected box style switcher
                    if let Some(sel_idx) = self.redaction_state.selected_idx
                        && sel_idx < self.redaction_state.boxes.len() {
                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.label(
                                egui::RichText::new("Selected Bar Style:")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                            ui.horizontal(|ui| {
                                let cur_style = self.redaction_state.boxes[sel_idx].style;
                                if ui
                                    .selectable_label(cur_style == RedactionStyle::Black, "Black")
                                    .clicked()
                                {
                                    self.redaction_state.boxes[sel_idx].style =
                                        RedactionStyle::Black;
                                }
                                if ui
                                    .selectable_label(
                                        cur_style == RedactionStyle::AverageColor,
                                        "Average Color",
                                    )
                                    .clicked()
                                {
                                    self.redaction_state.boxes[sel_idx].style =
                                        RedactionStyle::AverageColor;
                                }
                            });
                        }

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Burn / Apply Redactions button
                    let has_boxes = !self.redaction_state.boxes.is_empty();
                    let burn_btn = clip_ui::buttons::ButtonRole::Primary.button_with_icon(Some(Icon::Check), "Burn / Apply (Enter)", palette);

                    if ui
                        .add_enabled(has_boxes, burn_btn)
                        .on_hover_text("Permanently rasterize redactions into image pixels (Enter)")
                        .clicked()
                    {
                        self.apply_redactions();
                    }

                    // Delete Selected button
                    let has_selection = self.redaction_state.selected_idx.is_some();
                    let del_btn = clip_ui::buttons::ButtonRole::Destructive.button_with_icon(Some(Icon::Trash), "Delete Selected (Del)", palette);

                    if ui.add_enabled(has_selection, del_btn).clicked() {
                        self.delete_selected_redaction();
                    }

                    // Clear All button
                    let clear_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(Some(Icon::Close), "Clear All", palette);

                    if ui.add_enabled(has_boxes, clear_btn).clicked() {
                        self.clear_all_redactions();
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Strip Metadata / EXIF button
                    let has_meta = self.metadata.is_some();
                    let strip_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(Some(Icon::Shield), "Strip Metadata / EXIF", palette);

                    if ui
                        .add(strip_btn)
                        .on_hover_text(if has_meta {
                            "Permanently remove camera, GPS, timestamps, and all EXIF tags from this image"
                        } else {
                            "Image is already clean (No EXIF metadata detected)"
                        })
                        .clicked()
                    {
                        self.strip_metadata();
                    }
                }
                RedactToolType::LamaInpaint => {
                    ui.label(
                        egui::RichText::new("LaMa AI Inpainting:")
                            .strong()
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    // Selection Mode Toggle: Block Select | Paint | Point (SAM)
                    ui.horizontal(|ui| {
                        let modes = [
                            (InpaintSelectionMode::Rectangle, "Block Select"),
                            (InpaintSelectionMode::Brush, "Paint"),
                            (InpaintSelectionMode::PointSam, "Point (SAM)"),
                        ];
                        for (mode, label) in modes {
                            let is_active = self.redaction_state.inpaint_mode == mode;
                            if ui.selectable_label(is_active, label).clicked() {
                                self.set_inpaint_mode(mode);
                            }
                        }
                    });

                    // If Brush mode: radius slider and shortcut reminder
                    if self.redaction_state.inpaint_mode == InpaintSelectionMode::Brush {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.label("Brush Size:");
                            let mut rad = self.redaction_state.brush_radius;
                            if ui
                                .add(
                                    egui::Slider::new(&mut rad, 4.0..=128.0)
                                        .step_by(1.0)
                                        .text("px"),
                                )
                                .changed()
                            {
                                self.redaction_state.brush_radius = rad;
                            }
                        });
                        ui.label(
                            egui::RichText::new(
                                "Tip: (+/=) increase size, (-) decrease size\nRight-click drag to erase",
                            )
                            .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                        );
                    } else if self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        if self.sam_state.is_loading {
                            ui.label(
                                egui::RichText::new("Loading Segment Anything neural network...")
                                    .italics()
                                    .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                    .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        } else if let Some(ref err) = self.sam_state.error_text {
                            ui.colored_label(hex_to_c32(&palette.diff_del), format!("SAM: {err}"));
                        } else {
                            ui.label(
                                egui::RichText::new(
                                    "Tip: Hover to preview object (blue)\nClick to add to inpaint mask; Right-click to erase",
                                )
                                .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.horizontal(|ui| {
                        ui.label("Grow Selection:");
                        let mut grow = self.redaction_state.grow_selection_px;
                        if ui
                            .add(
                                egui::Slider::new(&mut grow, 0.0..=64.0)
                                    .step_by(1.0)
                                    .text("px"),
                            )
                            .on_hover_text(
                                "Expand selection mask outward by N pixels to give inpainting room to work with",
                            )
                            .changed()
                        {
                            self.redaction_state.grow_selection_px = grow;
                            self.refresh_sam_hover_preview(ui.ctx());
                        }
                    });

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    let has_mask = self.inpaint_has_mask();
                    let in_progress = self.redaction_state.inpaint_in_progress;

                    // Inpaint Run Button
                    let run_label = if in_progress {
                        "Inpainting... (Running LaMa)"
                    } else {
                        "Inpaint with LaMa (Enter)"
                    };
                    let inpaint_btn = clip_ui::buttons::ButtonRole::Primary.button_with_icon(Some(Icon::Wand), run_label, palette);

                    if ui
                        .add_enabled(has_mask && !in_progress, inpaint_btn)
                        .on_hover_text(
                            "Remove covered objects and fill background using LaMa AI (Enter)",
                        )
                        .clicked()
                    {
                        self.run_lama_inpaint();
                    }

                    // Action buttons: Clear Mask & Grow Mask
                    ui.horizontal(|ui| {
                        let clear_mask_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(Some(Icon::Close), "Clear Mask", palette);
                        if ui.add_enabled(has_mask, clear_mask_btn).clicked() {
                            self.clear_inpaint_mask();
                        }

                        let grow_mask_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(Some(Icon::Plus), "Grow Mask", palette);
                        if ui
                            .add_enabled(has_mask && !in_progress, grow_mask_btn)
                            .on_hover_text(
                                "Expand current inpaint mask by the grow-selection slider radius",
                            )
                            .clicked()
                        {
                            self.grow_current_inpaint_mask(ui.ctx());
                        }
                    });

                    if let Some(ref st) = self.redaction_state.inpaint_status {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new(st)
                                    .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                    .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        });
                    }

                    if let Some(ref err) = self.redaction_state.inpaint_error {
                        ui.add_space(crate::ui_ext::SPACE_XS);
                        ui.label(
                            egui::RichText::new(err)
                                .size(crate::ui_ext::BODY_FONT_SIZE - 2.0)
                                .color(hex_to_c32(&palette.diff_del)),
                        );
                    }
                }
            }

            // Undo button
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                if !self.undo_stack.is_empty() {
                    let undo_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(Some(crate::icons::Icon::Undo), "Undo (Ctrl+Z)", palette);
                    if ui.add(undo_btn).clicked() {
                        self.undo();
                    }
                }
            });
        });
    }

    pub(crate) fn render_redact_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let img_w = self.current_rgba.width();
        let img_h = self.current_rgba.height();
        let handle_radius = 5.0;
        let hit_dist = 12.0;

        let to_screen = |norm_p: Pos2| -> Pos2 {
            Pos2::new(
                img_rect.min.x + norm_p.x * img_rect.width(),
                img_rect.min.y + norm_p.y * img_rect.height(),
            )
        };
        let to_screen_rect = |norm_r: Rect| -> Rect {
            Rect::from_min_max(to_screen(norm_r.min), to_screen(norm_r.max))
        };
        let to_norm = |screen_p: Pos2| -> Pos2 {
            Pos2::new(
                ((screen_p.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0),
                ((screen_p.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0),
            )
        };

        let diff_del = hex_to_c32(&palette.diff_del);
        let b_low_badge = hex_to_c32_alpha(&palette.colors.surface_recessed, 230);
        let badge_text_col = contrast_text_c32(
            b_low_badge,
            Some(hex_to_c32(&palette.colors.text_primary)),
            palette,
        );

        // 1. Paint inpainting mask texture if present
        if let Some(ref tex) = self.inpaint_mask_texture {
            painter.image(
                tex.id(),
                img_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // 1b. Paint Segment Anything hovered object preview (transparent blue) if active
        if self.redaction_state.tool_type == RedactToolType::LamaInpaint
            && self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam
        {
            if let Some(ref tex) = self.sam_state.hovered_mask_texture {
                painter.image(
                    tex.id(),
                    img_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            if let Some(norm_p) = self.sam_state.hovered_pos {
                let screen_p = to_screen(norm_p);
                painter.circle_filled(screen_p, 3.5, Color32::from_rgb(30, 144, 255));
                painter.circle_stroke(screen_p, 5.5, Stroke::new(1.5_f32, Color32::WHITE));
            }
        }

        // 2. Draw existing redaction boxes
        for (i, b) in self.redaction_state.boxes.iter().enumerate() {
            let box_screen_rect = to_screen_rect(b.normalized_rect);

            let fill_color = match b.style {
                RedactionStyle::Black => Color32::BLACK,
                RedactionStyle::AverageColor => {
                    compute_box_average_color(&self.current_rgba, b.normalized_rect)
                }
            };

            painter.rect_filled(box_screen_rect, Rounding::ZERO, fill_color);

            let is_selected = self.redaction_state.selected_idx == Some(i);
            if is_selected {
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(2.0_f32, diff_del),
                );

                let handles = get_box_handles(box_screen_rect);
                for (_, pos, _) in &handles {
                    let handle_rect =
                        Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
                    painter.rect_filled(
                        handle_rect,
                        Rounding::same(1.0),
                        hex_to_c32(&palette.colors.text_primary),
                    );
                    painter.rect_stroke(
                        handle_rect,
                        Rounding::same(1.0),
                        Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_recessed)),
                    );
                }

                let px_w = (b.normalized_rect.width() * img_w as f32).round() as u32;
                let px_h = (b.normalized_rect.height() * img_h as f32).round() as u32;
                let style_desc = match b.style {
                    RedactionStyle::Black => "Black",
                    RedactionStyle::AverageColor => "Avg Color",
                };
                let badge_str = format!("{} × {} px ({})", px_w, px_h, style_desc);
                let badge_pos = Pos2::new(
                    box_screen_rect.center().x,
                    (box_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
                );
                let galley = ui.painter().layout_no_wrap(
                    badge_str,
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                    badge_text_col,
                );
                let badge_size = galley.size() + Vec2::new(12.0, 4.0);
                let ideal_badge_min = badge_pos - badge_size * 0.5;
                let badge_rect =
                    crate::ui_ext::clamp_overlay_rect(ideal_badge_min, badge_size, img_rect);
                painter.rect_filled(badge_rect, Rounding::same(4.0), b_low_badge);
                painter.rect_stroke(
                    badge_rect,
                    Rounding::same(4.0),
                    Stroke::new(1.0_f32, diff_del),
                );
                painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, badge_text_col);
            } else {
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(
                        1.0_f32,
                        hex_to_c32_alpha(&palette.colors.text_secondary, 60),
                    ),
                );
            }
        }

        // 3. Draw in-progress creating box / inpaint rectangle
        match self.redaction_state.active_drag {
            Some(RedactionDrag::Creating(start_norm, curr_norm)) => {
                let start_s = to_screen(start_norm);
                let curr_s = to_screen(curr_norm);
                let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
                let norm_r = Rect::from_two_pos(start_norm, curr_norm);

                let fill = match self.redaction_state.tool_type {
                    RedactToolType::BlackBar => Color32::from_rgba_unmultiplied(0, 0, 0, 210),
                    RedactToolType::AverageColor => {
                        let avg = compute_box_average_color(&self.current_rgba, norm_r);
                        Color32::from_rgba_unmultiplied(avg.r(), avg.g(), avg.b(), 210)
                    }
                    _ => Color32::from_rgba_unmultiplied(0, 0, 0, 210),
                };

                painter.rect_filled(in_progress_rect, Rounding::ZERO, fill);
                painter.rect_stroke(
                    in_progress_rect,
                    Rounding::ZERO,
                    Stroke::new(1.5_f32, diff_del),
                );
            }
            Some(RedactionDrag::InpaintRect(start_norm, curr_norm, is_erase)) => {
                let start_s = to_screen(start_norm);
                let curr_s = to_screen(curr_norm);
                let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
                let fill = if is_erase {
                    Color32::from_rgba_unmultiplied(60, 60, 60, 120)
                } else {
                    Color32::from_rgba_unmultiplied(235, 30, 120, 140)
                };
                let stroke_color = if is_erase {
                    Color32::WHITE
                } else {
                    hex_to_c32(&palette.harmonies.base)
                };
                painter.rect_filled(in_progress_rect, Rounding::ZERO, fill);
                painter.rect_stroke(
                    in_progress_rect,
                    Rounding::ZERO,
                    Stroke::new(1.5_f32, stroke_color),
                );
            }
            _ => {}
        }

        // 4. Cursor rendering & icon
        if let Some(ptr_pos) = response.hover_pos() {
            if self.redaction_state.tool_type == RedactToolType::LamaInpaint {
                match self.redaction_state.inpaint_mode {
                    InpaintSelectionMode::Rectangle => {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                    InpaintSelectionMode::Brush => {
                        // Render circular brush outline at pointer position
                        if img_rect.contains(ptr_pos) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                            let scale_x = img_rect.width() / img_w as f32;
                            let screen_radius = self.redaction_state.brush_radius * scale_x;
                            // High contrast double circle
                            painter.circle_stroke(
                                ptr_pos,
                                screen_radius + 0.5,
                                Stroke::new(1.5_f32, Color32::BLACK),
                            );
                            painter.circle_stroke(
                                ptr_pos,
                                screen_radius,
                                Stroke::new(1.0_f32, Color32::WHITE),
                            );
                        } else {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                        }
                    }
                    InpaintSelectionMode::PointSam => {
                        if img_rect.contains(ptr_pos) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                            let norm = to_norm(ptr_pos);
                            self.sam_on_hover(norm, ui.ctx());
                        } else {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                            self.sam_state.reset_hover();
                        }
                    }
                }
            } else if let Some(selected_idx) = self.redaction_state.selected_idx {
                if selected_idx < self.redaction_state.boxes.len() {
                    let box_screen_rect =
                        to_screen_rect(self.redaction_state.boxes[selected_idx].normalized_rect);
                    let handles = get_box_handles(box_screen_rect);
                    let hovered_handle = handles.iter().find_map(|(_, pos, cur)| {
                        if pos.distance(ptr_pos) <= hit_dist {
                            Some(*cur)
                        } else {
                            None
                        }
                    });

                    if let Some(cur) = hovered_handle {
                        ui.ctx().set_cursor_icon(cur);
                    } else if box_screen_rect.contains(ptr_pos) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                    } else {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    }
                } else {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }

        // 5. Handle Pointer & Drag Events
        let is_secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));

        if response.drag_started()
            && let Some(ptr) = response.interact_pointer_pos()
        {
            if self.redaction_state.tool_type == RedactToolType::LamaInpaint {
                match self.redaction_state.inpaint_mode {
                    InpaintSelectionMode::Rectangle => {
                        if img_rect.contains(ptr) {
                            let norm = to_norm(ptr);
                            self.redaction_state.active_drag =
                                Some(RedactionDrag::InpaintRect(norm, norm, is_secondary_down));
                        }
                    }
                    InpaintSelectionMode::Brush => {
                        if img_rect.contains(ptr) {
                            let norm = to_norm(ptr);
                            self.redaction_state.active_drag =
                                Some(RedactionDrag::InpaintBrush(is_secondary_down));
                            self.redaction_state.last_brush_pos = Some(norm);
                            let val = if is_secondary_down { 0 } else { 255 };
                            let radius = self.redaction_state.brush_radius;
                            {
                                let mask = self.ensure_inpaint_mask();
                                stamp_circle_on_mask(mask, norm, radius, val);
                            }
                            self.update_inpaint_mask_texture(ui.ctx());
                        }
                    }
                    InpaintSelectionMode::PointSam => {}
                }
            } else {
                let mut handled = false;
                if let Some(sel_idx) = self.redaction_state.selected_idx
                    && sel_idx < self.redaction_state.boxes.len()
                {
                    let init_norm = self.redaction_state.boxes[sel_idx].normalized_rect;
                    let box_screen_rect = to_screen_rect(init_norm);
                    let handles = get_box_handles(box_screen_rect);

                    if let Some((h, _, _)) = handles
                        .iter()
                        .find(|(_, pos, _)| pos.distance(ptr) <= hit_dist)
                    {
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::Resizing(sel_idx, *h, ptr, init_norm));
                        handled = true;
                    } else if box_screen_rect.contains(ptr) {
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::Moving(sel_idx, ptr, init_norm));
                        handled = true;
                    }
                }

                if !handled {
                    let found_idx = self
                        .redaction_state
                        .boxes
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(i, b)| {
                            let r = to_screen_rect(b.normalized_rect);
                            if r.contains(ptr) { Some(i) } else { None }
                        });

                    if let Some(idx) = found_idx {
                        self.redaction_state.selected_idx = Some(idx);
                        let init_norm = self.redaction_state.boxes[idx].normalized_rect;
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::Moving(idx, ptr, init_norm));
                    } else if img_rect.contains(ptr) {
                        let start_norm = to_norm(ptr);
                        self.redaction_state.selected_idx = None;
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::Creating(start_norm, start_norm));
                    }
                }
            }
        }

        if response.dragged()
            && let Some(ptr) = response.interact_pointer_pos()
        {
            match self.redaction_state.active_drag {
                Some(RedactionDrag::Creating(start_norm, _)) => {
                    let curr_norm = to_norm(ptr);
                    self.redaction_state.active_drag =
                        Some(RedactionDrag::Creating(start_norm, curr_norm));
                }
                Some(RedactionDrag::InpaintRect(start_norm, _, is_erase)) => {
                    let curr_norm = to_norm(ptr);
                    self.redaction_state.active_drag =
                        Some(RedactionDrag::InpaintRect(start_norm, curr_norm, is_erase));
                }
                Some(RedactionDrag::InpaintBrush(is_erase)) => {
                    let curr_norm = to_norm(ptr);
                    let radius = self.redaction_state.brush_radius;
                    let val = if is_erase { 0 } else { 255 };
                    if let Some(last_pos) = self.redaction_state.last_brush_pos {
                        let mask = self.ensure_inpaint_mask();
                        stamp_line_on_mask(mask, last_pos, curr_norm, radius, val);
                    }
                    self.redaction_state.last_brush_pos = Some(curr_norm);
                    self.update_inpaint_mask_texture(ui.ctx());
                }
                Some(RedactionDrag::Moving(idx, start_ptr, init_norm)) => {
                    let delta_s = ptr - start_ptr;
                    let delta_norm_x = delta_s.x / img_rect.width();
                    let delta_norm_y = delta_s.y / img_rect.height();
                    let box_w = init_norm.width();
                    let box_h = init_norm.height();
                    let new_min_x = (init_norm.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
                    let new_min_y = (init_norm.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
                    if idx < self.redaction_state.boxes.len() {
                        self.redaction_state.boxes[idx].normalized_rect = Rect::from_min_size(
                            Pos2::new(new_min_x, new_min_y),
                            Vec2::new(box_w, box_h),
                        );
                    }
                }
                Some(RedactionDrag::Resizing(idx, handle, start_ptr, init_norm)) => {
                    let delta_s = ptr - start_ptr;
                    let min_w = 4.0 / img_w as f32;
                    let min_h = 4.0 / img_h as f32;
                    let new_rect =
                        resize_norm_rect(init_norm, handle, delta_s, img_rect, min_w, min_h);
                    if idx < self.redaction_state.boxes.len() {
                        self.redaction_state.boxes[idx].normalized_rect = new_rect;
                    }
                }
                None => {}
            }
        }

        if response.drag_stopped() {
            match self.redaction_state.active_drag {
                Some(RedactionDrag::Creating(start_norm, end_norm)) => {
                    let min_x = start_norm.x.min(end_norm.x);
                    let max_x = start_norm.x.max(end_norm.x);
                    let min_y = start_norm.y.min(end_norm.y);
                    let max_y = start_norm.y.max(end_norm.y);
                    let norm_w = max_x - min_x;
                    let norm_h = max_y - min_y;

                    let min_norm_w = 4.0 / img_w as f32;
                    let min_norm_h = 4.0 / img_h as f32;
                    if norm_w >= min_norm_w && norm_h >= min_norm_h {
                        let style = match self.redaction_state.tool_type {
                            RedactToolType::AverageColor => RedactionStyle::AverageColor,
                            _ => RedactionStyle::Black,
                        };
                        self.redaction_state.boxes.push(RedactionBox {
                            normalized_rect: Rect::from_min_max(
                                Pos2::new(min_x, min_y),
                                Pos2::new(max_x, max_y),
                            ),
                            style,
                        });
                        self.redaction_state.selected_idx =
                            Some(self.redaction_state.boxes.len() - 1);
                    }
                }
                Some(RedactionDrag::InpaintRect(start_norm, end_norm, is_erase)) => {
                    let min_x = start_norm.x.min(end_norm.x);
                    let max_x = start_norm.x.max(end_norm.x);
                    let min_y = start_norm.y.min(end_norm.y);
                    let max_y = start_norm.y.max(end_norm.y);
                    let norm_r =
                        Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y));
                    let val = if is_erase { 0 } else { 255 };
                    {
                        let mask = self.ensure_inpaint_mask();
                        stamp_rect_on_mask(mask, norm_r, val);
                    }
                    self.update_inpaint_mask_texture(ui.ctx());
                }
                _ => {}
            }
            self.redaction_state.active_drag = None;
            self.redaction_state.last_brush_pos = None;
        }

        // Clicking without dragging on empty background deselects in box mode
        if response.clicked()
            && self.redaction_state.tool_type != RedactToolType::LamaInpaint
            && let Some(ptr) = response.interact_pointer_pos()
        {
            let hit_any = self
                .redaction_state
                .boxes
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, b)| {
                    if to_screen_rect(b.normalized_rect).contains(ptr) {
                        Some(i)
                    } else {
                        None
                    }
                });
            self.redaction_state.selected_idx = hit_any;
        }

        // Point-to-select SAM mask click actions
        if self.redaction_state.tool_type == RedactToolType::LamaInpaint
            && self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam
        {
            if response.clicked() {
                self.commit_sam_hover(255, ui.ctx());
            } else if response.secondary_clicked() {
                self.commit_sam_hover(0, ui.ctx());
            }
        }
    }
}
