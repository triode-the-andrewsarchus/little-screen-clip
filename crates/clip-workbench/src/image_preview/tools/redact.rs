use super::super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct RedactionBox {
    /// Normalized coordinates in [0.0, 1.0] range relative to image
    pub normalized_rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RedactionDrag {
    Creating(Pos2, Pos2),
    Moving(usize, Pos2, Rect),
    Resizing(usize, CropHandle, Pos2, Rect),
}

#[derive(Debug, Clone, Default)]
pub struct RedactionState {
    pub boxes: Vec<RedactionBox>,
    pub selected_idx: Option<usize>,
    pub active_drag: Option<RedactionDrag>,
}

impl ImagePreviewState {
    pub fn apply_redactions(&mut self) {
        if self.redaction_state.boxes.is_empty() {
            return;
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let img_w = self.current_rgba.width() as f32;
        let img_h = self.current_rgba.height() as f32;
        let count = self.redaction_state.boxes.len();

        for b in &self.redaction_state.boxes {
            let px_min_x = (b.normalized_rect.min.x * img_w).round().clamp(0.0, img_w) as u32;
            let px_max_x = (b.normalized_rect.max.x * img_w).round().clamp(0.0, img_w) as u32;
            let px_min_y = (b.normalized_rect.min.y * img_h).round().clamp(0.0, img_h) as u32;
            let px_max_y = (b.normalized_rect.max.y * img_h).round().clamp(0.0, img_h) as u32;

            for y in px_min_y..px_max_y {
                for x in px_min_x..px_max_x {
                    self.current_rgba
                        .put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }

        self.redaction_state.boxes.clear();
        self.redaction_state.selected_idx = None;
        self.redaction_state.active_drag = None;
        self.dirty_texture = true;

        // Automatically ensure metadata is stripped when redactions are permanently burned
        if let Ok(clean) = strip_metadata(&self.current_rgba, &self.mime) {
            self.raw_bytes = clean;
            self.file_size_bytes = self.raw_bytes.len();
            self.metadata = None;
        }

        self.show_toast(format!("█ Burned {} redaction bars into image", count));
    }

    /// Deletes the currently selected redaction bar.

    pub fn delete_selected_redaction(&mut self) {
        if let Some(idx) = self.redaction_state.selected_idx {
            if idx < self.redaction_state.boxes.len() {
                self.redaction_state.boxes.remove(idx);
                self.redaction_state.selected_idx = None;
                self.redaction_state.active_drag = None;
                self.show_toast("🗑 Redaction bar removed");
            }
        }
    }

    /// Clears all pending redaction bars without burning.

    pub fn clear_all_redactions(&mut self) {
        let count = self.redaction_state.boxes.len();
        if count > 0 {
            self.redaction_state.boxes.clear();
            self.redaction_state.selected_idx = None;
            self.redaction_state.active_drag = None;
            self.show_toast(format!("✕ Cleared {} pending redaction bars", count));
        }
    }

    /// Strips all EXIF, GPS, camera metadata, and timestamps by cleanly re-encoding pristine pixels.

    pub(crate) fn render_redact_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0_f32, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Privacy Redaction:")
                            .strong()
                            .small()
                            .color(hex_to_c32(&palette.diff_del)),
                    );

                    let box_count = self.redaction_state.boxes.len();
                    let count_str = if box_count == 0 {
                        "Drag on image to draw black bar".to_string()
                    } else if box_count == 1 {
                        "1 bar placed".to_string()
                    } else {
                        format!("{} bars placed", box_count)
                    };
                    ui.label(
                        egui::RichText::new(count_str)
                            .small()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    ui.add_space(8.0);

                    // Burn / Apply Redactions button
                    let has_boxes = !self.redaction_state.boxes.is_empty();
                    let burn_bg = if has_boxes {
                        hex_to_c32(&palette.diff_del)
                    } else {
                        btn_bg
                    };
                    let burn_btn = crate::ui_ext::action_button_icon(
                        Some(Icon::Check),
                        "Burn / Apply (Enter)",
                        burn_bg,
                        palette,
                    );

                    if ui.add_enabled(has_boxes, burn_btn).on_hover_text("Permanently rasterize black bars into image pixels (Enter)").clicked() {
                        self.apply_redactions();
                    }

                    // Delete Selected button
                    let has_selection = self.redaction_state.selected_idx.is_some();
                    let del_btn = crate::ui_ext::danger_button_icon(
                        Some(Icon::Trash),
                        "Delete Selected (Del)",
                        btn_bg,
                        palette,
                    );

                    if ui.add_enabled(has_selection, del_btn).clicked() {
                        self.delete_selected_redaction();
                    }

                    // Clear All button
                    let clear_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Close),
                        "Clear All",
                        btn_bg,
                        palette,
                    );

                    if ui.add_enabled(has_boxes, clear_btn).clicked() {
                        self.clear_all_redactions();
                    }

                    ui.add_space(8.0);

                    // Strip Metadata / EXIF button
                    let has_meta = self.metadata.is_some();
                    let strip_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Shield),
                        "Strip Metadata / EXIF",
                        btn_bg,
                        palette,
                    );

                    if ui.add(strip_btn)
                        .on_hover_text(if has_meta {
                            "Permanently remove camera, GPS, timestamps, and all EXIF tags from this image"
                        } else {
                            "Image is already clean (No EXIF metadata detected)"
                        })
                        .clicked()
                    {
                        self.strip_metadata();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_mod)), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(btn_bg)
                            .stroke(Stroke::new(1.0_f32, btn_border))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
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
        let b_low_badge = hex_to_c32_alpha(&palette.theme.b_low, 230);
        let badge_text_col = contrast_text_c32(
            b_low_badge,
            Some(hex_to_c32(&palette.theme.f_high)),
            palette,
        );

        // 1. Draw existing redaction boxes
        for (i, b) in self.redaction_state.boxes.iter().enumerate() {
            let box_screen_rect = to_screen_rect(b.normalized_rect);

            // Draw solid black redaction bar
            painter.rect_filled(box_screen_rect, Rounding::ZERO, Color32::BLACK);

            let is_selected = self.redaction_state.selected_idx == Some(i);
            if is_selected {
                // Highlight border
                painter.rect_stroke(
                    box_screen_rect,
                    Rounding::ZERO,
                    Stroke::new(2.0_f32, diff_del),
                );

                // 8 Grab Handles
                let handles = get_box_handles(box_screen_rect);
                for (_, pos, _) in &handles {
                    let handle_rect =
                        Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
                    painter.rect_filled(
                        handle_rect,
                        Rounding::same(1.0),
                        hex_to_c32(&palette.theme.f_high),
                    );
                    painter.rect_stroke(
                        handle_rect,
                        Rounding::same(1.0),
                        Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_low)),
                    );
                }

                // Floating size badge
                let px_w = (b.normalized_rect.width() * img_w as f32).round() as u32;
                let px_h = (b.normalized_rect.height() * img_h as f32).round() as u32;
                let badge_str = format!("{} × {} px", px_w, px_h);
                let badge_pos = Pos2::new(
                    box_screen_rect.center().x,
                    (box_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
                );
                let galley = ui.painter().layout_no_wrap(
                    badge_str,
                    egui::FontId::proportional(11.0),
                    badge_text_col,
                );
                let badge_rect =
                    Rect::from_center_size(badge_pos, galley.size() + Vec2::new(12.0, 4.0));
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
                    Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.theme.f_med, 60)),
                );
            }
        }

        // 2. Draw in-progress creating box
        if let Some(RedactionDrag::Creating(start_norm, curr_norm)) =
            self.redaction_state.active_drag
        {
            let start_s = to_screen(start_norm);
            let curr_s = to_screen(curr_norm);
            let in_progress_rect = Rect::from_two_pos(start_s, curr_s);
            painter.rect_filled(
                in_progress_rect,
                Rounding::ZERO,
                Color32::from_rgba_unmultiplied(0, 0, 0, 210),
            );
            painter.rect_stroke(
                in_progress_rect,
                Rounding::ZERO,
                Stroke::new(1.5_f32, diff_del),
            );
        }

        // 3. Set Cursor icon
        if let Some(ptr_pos) = response.hover_pos() {
            if let Some(selected_idx) = self.redaction_state.selected_idx {
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

        // 4. Handle Dragging
        if response.drag_started() {
            if let Some(ptr) = response.interact_pointer_pos() {
                let mut handled = false;
                if let Some(sel_idx) = self.redaction_state.selected_idx {
                    if sel_idx < self.redaction_state.boxes.len() {
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

        if response.dragged() {
            if let Some(ptr) = response.interact_pointer_pos() {
                match self.redaction_state.active_drag {
                    Some(RedactionDrag::Creating(start_norm, _)) => {
                        let curr_norm = to_norm(ptr);
                        self.redaction_state.active_drag =
                            Some(RedactionDrag::Creating(start_norm, curr_norm));
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
        }

        if response.drag_stopped() {
            if let Some(RedactionDrag::Creating(start_norm, end_norm)) =
                self.redaction_state.active_drag
            {
                let min_x = start_norm.x.min(end_norm.x);
                let max_x = start_norm.x.max(end_norm.x);
                let min_y = start_norm.y.min(end_norm.y);
                let max_y = start_norm.y.max(end_norm.y);
                let norm_w = max_x - min_x;
                let norm_h = max_y - min_y;

                let min_norm_w = 4.0 / img_w as f32;
                let min_norm_h = 4.0 / img_h as f32;
                if norm_w >= min_norm_w && norm_h >= min_norm_h {
                    self.redaction_state.boxes.push(RedactionBox {
                        normalized_rect: Rect::from_min_max(
                            Pos2::new(min_x, min_y),
                            Pos2::new(max_x, max_y),
                        ),
                    });
                    self.redaction_state.selected_idx = Some(self.redaction_state.boxes.len() - 1);
                }
            }
            self.redaction_state.active_drag = None;
        }

        // Clicking without dragging on empty background deselects
        if response.clicked() {
            if let Some(ptr) = response.interact_pointer_pos() {
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
        }
    }
}
