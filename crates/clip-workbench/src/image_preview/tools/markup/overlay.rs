use super::*;

impl ImagePreviewState {
    /// Burns all vector markup layers into `current_rgba` permanently, pushing to undo stack.
    pub fn apply_flatten_markup(&mut self) {
        if self.markup_state.items.is_empty() {
            return;
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let count = self.markup_state.items.len();
        self.current_rgba = Arc::new(rasterize_markup(
            &self.current_rgba,
            &self.markup_state.items,
        ));
        self.mark_pixels_changed();
        self.markup_state.items.clear();
        self.markup_state.selected_idx = None;
        self.markup_state.editing_text_idx = None;
        self.markup_state.active_drag = None;
        self.markup_state.sync_next_badge_number();
        self.dirty_texture = true;

        self.show_toast(format!(
            "🔥 Flattened {} markup layers into image pixels",
            count
        ));
    }

    /// Renders placed markup layers passively on any preview viewport without interaction.
    pub(crate) fn render_markup_passive(&self, painter: &egui::Painter, img_rect: Rect) {
        let to_screen = |norm_p: Pos2| -> Pos2 {
            Pos2::new(
                img_rect.min.x + norm_p.x * img_rect.width(),
                img_rect.min.y + norm_p.y * img_rect.height(),
            )
        };
        let to_screen_rect = |norm_r: Rect| -> Rect {
            Rect::from_min_max(to_screen(norm_r.min), to_screen(norm_r.max))
        };
        let zoom_scale =
            (img_rect.width() / self.current_rgba.width().max(1) as f32).clamp(0.1_f32, 10.0_f32);

        for shape in &self.markup_state.items {
            match shape {
                MarkupShape::Arrow {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    draw_arrow_painter(painter, s_screen, e_screen, *color, sw);
                }
                MarkupShape::Line {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    painter.line_segment([s_screen, e_screen], Stroke::new(sw, *color));
                }
                MarkupShape::Rectangle {
                    rect,
                    color,
                    width,
                    filled,
                    rounding,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    if *filled {
                        painter.rect_filled(s_rect, Rounding::same(*rounding), *color);
                    } else {
                        painter.rect_stroke(
                            s_rect,
                            Rounding::same(*rounding),
                            Stroke::new(sw, *color),
                        );
                    }
                }
                MarkupShape::Circle {
                    rect,
                    color,
                    width,
                    filled,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    let center = s_rect.center();
                    let radius = s_rect.width().min(s_rect.height()) * 0.5_f32;

                    if *filled {
                        painter.circle_filled(center, radius, *color);
                    } else {
                        painter.circle_stroke(center, radius, Stroke::new(sw, *color));
                    }
                }
                MarkupShape::Highlighter { rect, color } => {
                    let s_rect = to_screen_rect(*rect);
                    let highlight_col =
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90);
                    painter.rect_filled(s_rect, Rounding::same(2.0_f32), highlight_col);
                }
                MarkupShape::StepBadge {
                    center,
                    radius,
                    number,
                    color,
                } => {
                    let c_screen = to_screen(*center);
                    let img_h = self.current_rgba.height().max(1) as f32;
                    let r_px = if *radius <= 1.0_f32 {
                        *radius * img_h
                    } else {
                        *radius
                    };
                    let badge_radius = (r_px * zoom_scale).max(8.0_f32);
                    painter.circle_filled(c_screen, badge_radius, *color);
                    let stroke_w = (badge_radius * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    painter.circle_stroke(
                        c_screen,
                        badge_radius,
                        Stroke::new(
                            stroke_w,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        ),
                    );
                    let auto_font_size = step_badge_font_size(badge_radius, *number);
                    painter.text(
                        c_screen,
                        Align2::CENTER_CENTER,
                        number.to_string(),
                        FontId::proportional(auto_font_size),
                        Color32::WHITE,
                    );
                }
                MarkupShape::Text {
                    pos,
                    text,
                    font_size,
                } => {
                    if text.is_empty() {
                        continue;
                    }
                    let p_screen = to_screen(*pos);
                    let scaled_font_size = (*font_size * zoom_scale).max(11.0_f32);
                    let galley = painter.layout_no_wrap(
                        text.to_string(),
                        FontId::proportional(scaled_font_size),
                        Color32::WHITE,
                    );
                    let pad_x = (scaled_font_size * 0.35).clamp(6.0_f32, 24.0_f32);
                    let pad_y = (scaled_font_size * 0.15).clamp(3.0_f32, 12.0_f32);
                    let box_rect = Rect::from_min_size(
                        p_screen,
                        galley.size() + Vec2::new(pad_x * 2.0_f32, pad_y * 2.0_f32),
                    );

                    painter.rect_filled(
                        box_rect,
                        Rounding::same(3.0_f32),
                        Color32::from_rgba_unmultiplied(12, 12, 12, 245),
                    );
                    painter.rect_stroke(
                        box_rect,
                        Rounding::same(3.0_f32),
                        Stroke::new(1.0_f32, Color32::from_gray(70)),
                    );
                    painter.galley(p_screen + Vec2::new(pad_x, pad_y), galley, Color32::WHITE);
                }
            }
        }
    }

    /// Interactive canvas overlay rendering all placed shapes, handles, and inline text editing.
    pub(crate) fn render_markup_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
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

        let img_w = self.current_rgba.width().max(1) as f32;
        let img_h = self.current_rgba.height().max(1) as f32;
        let zoom_scale = (img_rect.width() / img_w).clamp(0.1, 10.0);
        let base_accent = hex_to_c32(&palette.harmonies.base);
        let handle_radius = 5.0;
        let hit_dist = 12.0;

        // ── 1. Draw Placed Shapes ──
        for (i, shape) in self.markup_state.items.iter_mut().enumerate() {
            let is_selected = self.markup_state.selected_idx == Some(i);

            match shape {
                MarkupShape::Arrow {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5);
                    draw_arrow_painter(painter, s_screen, e_screen, *color, sw);

                    if is_selected {
                        draw_handle(painter, s_screen, handle_radius, base_accent);
                        draw_handle(painter, e_screen, handle_radius, base_accent);
                    }
                }
                MarkupShape::Line {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5);
                    painter.line_segment([s_screen, e_screen], Stroke::new(sw, *color));

                    if is_selected {
                        draw_handle(painter, s_screen, handle_radius, base_accent);
                        draw_handle(painter, e_screen, handle_radius, base_accent);
                    }
                }
                MarkupShape::Rectangle {
                    rect,
                    color,
                    width,
                    filled,
                    rounding,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5);
                    if *filled {
                        painter.rect_filled(s_rect, Rounding::same(*rounding), *color);
                    } else {
                        painter.rect_stroke(
                            s_rect,
                            Rounding::same(*rounding),
                            Stroke::new(sw, *color),
                        );
                    }

                    if is_selected {
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::Circle {
                    rect,
                    color,
                    width,
                    filled,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5);
                    let center = s_rect.center();
                    let radius = s_rect.width().min(s_rect.height()) * 0.5;

                    if *filled {
                        painter.circle_filled(center, radius, *color);
                    } else {
                        painter.circle_stroke(center, radius, Stroke::new(sw, *color));
                    }

                    if is_selected {
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::Highlighter { rect, color } => {
                    let s_rect = to_screen_rect(*rect);
                    let highlight_col =
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90);
                    painter.rect_filled(s_rect, Rounding::same(2.0), highlight_col);

                    if is_selected {
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::StepBadge {
                    center,
                    radius,
                    number,
                    color,
                } => {
                    let c_screen = to_screen(*center);
                    let r_px = if *radius <= 1.0_f32 {
                        *radius * img_h
                    } else {
                        *radius
                    };
                    let badge_radius = (r_px * zoom_scale).max(8.0_f32);
                    painter.circle_filled(c_screen, badge_radius, *color);
                    let stroke_w = (badge_radius * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    painter.circle_stroke(
                        c_screen,
                        badge_radius,
                        Stroke::new(
                            stroke_w,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        ),
                    );
                    let auto_font_size = step_badge_font_size(badge_radius, *number);
                    painter.text(
                        c_screen,
                        Align2::CENTER_CENTER,
                        number.to_string(),
                        FontId::proportional(auto_font_size),
                        Color32::WHITE,
                    );

                    if is_selected {
                        let s_rect =
                            Rect::from_center_size(c_screen, Vec2::splat(badge_radius * 2.0_f32));
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::Text {
                    pos,
                    text,
                    font_size,
                } => {
                    let p_screen = to_screen(*pos);
                    let scaled_font_size = (*font_size * zoom_scale).max(11.0);
                    let is_editing = self.markup_state.editing_text_idx == Some(i);

                    let display_str = if text.is_empty() {
                        "Type text here..."
                    } else {
                        text.as_str()
                    };

                    let galley = painter.layout_no_wrap(
                        display_str.to_string(),
                        FontId::proportional(scaled_font_size),
                        if text.is_empty() {
                            Color32::from_gray(160)
                        } else {
                            Color32::WHITE
                        },
                    );

                    let pad_x = (scaled_font_size * 0.35).clamp(6.0, 24.0);
                    let pad_y = (scaled_font_size * 0.15).clamp(3.0, 12.0);
                    let edit_w = (galley.size().x + scaled_font_size * 0.5).max(60.0);
                    let box_w = if is_editing {
                        (galley.size().x + pad_x * 2.0).max(edit_w + pad_x * 2.0)
                    } else {
                        galley.size().x + pad_x * 2.0
                    };
                    let box_h = galley.size().y + pad_y * 2.0;
                    let box_rect = Rect::from_min_size(p_screen, Vec2::new(box_w, box_h));

                    // Solid black expanding box
                    painter.rect_filled(
                        box_rect,
                        Rounding::same(3.0),
                        Color32::from_rgba_unmultiplied(12, 12, 12, 245),
                    );
                    painter.rect_stroke(
                        box_rect,
                        Rounding::same(3.0),
                        Stroke::new(
                            1.0_f32,
                            if is_editing || is_selected {
                                base_accent
                            } else {
                                Color32::from_gray(70)
                            },
                        ),
                    );

                    if is_editing {
                        let mut temp_text = text.clone();
                        let text_edit = egui::TextEdit::singleline(&mut temp_text)
                            .font(FontId::proportional(scaled_font_size))
                            .text_color(Color32::WHITE)
                            .frame(false)
                            .desired_width(edit_w);

                        let edit_rect = Rect::from_min_size(
                            p_screen + Vec2::new(pad_x, pad_y),
                            Vec2::new(edit_w, galley.size().y.max(scaled_font_size)),
                        );

                        let res = ui.put(edit_rect, text_edit);
                        res.request_focus();
                        if res.changed() {
                            *text = temp_text;
                        }

                        if ui.input(|inp| {
                            inp.key_pressed(egui::Key::Enter) || inp.key_pressed(egui::Key::Escape)
                        }) {
                            self.markup_state.editing_text_idx = None;
                        }
                    } else {
                        painter.galley(
                            p_screen + Vec2::new(pad_x, pad_y),
                            galley,
                            if text.is_empty() {
                                Color32::from_gray(160)
                            } else {
                                Color32::WHITE
                            },
                        );

                        if is_selected {
                            draw_selection_bounding_box(
                                painter,
                                box_rect,
                                base_accent,
                                handle_radius,
                            );
                        }
                    }
                }
            }
        }

        // ── 2. Draw In-Progress Drag / Creation ──
        if let Some(MarkupDrag::Creating {
            tool,
            start_norm,
            current_norm,
        }) = self.markup_state.active_drag
        {
            let s_screen = to_screen(start_norm);
            let c_screen = to_screen(current_norm);
            let active_col = self.markup_state.active_color;
            let sw = (self.markup_state.active_stroke_width * zoom_scale).max(1.5);

            match tool {
                MarkupToolType::Arrow => {
                    draw_arrow_painter(painter, s_screen, c_screen, active_col, sw);
                }
                MarkupToolType::Line => {
                    painter.line_segment([s_screen, c_screen], Stroke::new(sw, active_col));
                }
                MarkupToolType::Rectangle => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    if self.markup_state.active_filled {
                        painter.rect_filled(r, Rounding::same(3.0), active_col);
                    } else {
                        painter.rect_stroke(r, Rounding::same(3.0), Stroke::new(sw, active_col));
                    }
                }
                MarkupToolType::Circle => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    let radius = r.width().min(r.height()) * 0.5;
                    if self.markup_state.active_filled {
                        painter.circle_filled(r.center(), radius, active_col);
                    } else {
                        painter.circle_stroke(r.center(), radius, Stroke::new(sw, active_col));
                    }
                }
                MarkupToolType::Highlighter => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    let highlight_col = Color32::from_rgba_unmultiplied(
                        active_col.r(),
                        active_col.g(),
                        active_col.b(),
                        90,
                    );
                    painter.rect_filled(r, Rounding::same(2.0), highlight_col);
                }
                MarkupToolType::Text => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    let box_h_img = (current_norm.y - start_norm.y).abs() * img_h;
                    let font_size = if box_h_img >= 10.0 {
                        infer_font_size_from_box_height(box_h_img)
                    } else {
                        self.markup_state.active_font_size
                    };
                    let scaled_font_size = (font_size * zoom_scale).max(9.0);

                    // Solid dark background for the box preview
                    painter.rect_filled(
                        r,
                        Rounding::same(3.0),
                        Color32::from_rgba_unmultiplied(12, 12, 12, 240),
                    );
                    painter.rect_stroke(r, Rounding::same(3.0), Stroke::new(1.5_f32, base_accent));

                    // Live preview of text fitting inside the box height
                    if r.height() >= 12.0 {
                        let galley = painter.layout_no_wrap(
                            "Aa Text".to_string(),
                            FontId::proportional(scaled_font_size),
                            Color32::from_gray(180),
                        );
                        let pad_x = (scaled_font_size * 0.35).clamp(6.0, 24.0);
                        let text_pos =
                            Pos2::new(r.min.x + pad_x, r.center().y - galley.size().y * 0.5);
                        painter.galley(text_pos, galley, Color32::from_gray(180));
                    }
                }
                MarkupToolType::StepBadge => {
                    let drag_dist = s_screen.distance(c_screen);
                    let radius_screen = if drag_dist >= 8.0_f32 {
                        drag_dist
                    } else {
                        (self.markup_state.active_badge_radius * zoom_scale).max(10.0_f32)
                    };

                    painter.circle_filled(s_screen, radius_screen, active_col);
                    let stroke_w = (radius_screen * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    painter.circle_stroke(
                        s_screen,
                        radius_screen,
                        Stroke::new(
                            stroke_w,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        ),
                    );
                    let next_num = self.markup_state.next_available_badge_number();
                    let auto_font_size = step_badge_font_size(radius_screen, next_num);
                    painter.text(
                        s_screen,
                        Align2::CENTER_CENTER,
                        next_num.to_string(),
                        FontId::proportional(auto_font_size),
                        Color32::WHITE,
                    );
                }
                _ => {}
            }
        }

        // ── 3. Cursor management ──
        if let Some(ptr_pos) = response.hover_pos() {
            if self.markup_state.active_tool == MarkupToolType::Select {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Default);
            } else if self.markup_state.active_tool == MarkupToolType::Text {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Text);
            } else {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
            }

            // Check hovered handles on selected shape
            if let Some(sel_idx) = self.markup_state.selected_idx {
                if sel_idx < self.markup_state.items.len() {
                    let shape = &self.markup_state.items[sel_idx];
                    match shape {
                        MarkupShape::Arrow { start, end, .. }
                        | MarkupShape::Line { start, end, .. } => {
                            let s = to_screen(*start);
                            let e = to_screen(*end);
                            if s.distance(ptr_pos) <= hit_dist || e.distance(ptr_pos) <= hit_dist {
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            }
                        }
                        _ => {
                            let s_rect = to_screen_rect(shape.bounding_rect_norm(img_w, img_h));
                            let handles = get_box_handles(s_rect);
                            if let Some((_, _, cur)) = handles
                                .iter()
                                .find(|(_, pos, _)| pos.distance(ptr_pos) <= hit_dist)
                            {
                                ui.output_mut(|o| o.cursor_icon = *cur);
                            } else if s_rect.contains(ptr_pos) {
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
                            }
                        }
                    }
                }
            }
        }

        // ── 4. Mouse Input Handling ──
        if response.drag_started() {
            if let Some(ptr) = response.interact_pointer_pos() {
                let mut handled = false;

                // 4a. Check if clicking on active selection handles
                if let Some(sel_idx) = self.markup_state.selected_idx {
                    if sel_idx < self.markup_state.items.len() {
                        let shape = &self.markup_state.items[sel_idx];
                        match shape {
                            MarkupShape::Arrow { start, end, .. }
                            | MarkupShape::Line { start, end, .. } => {
                                let s = to_screen(*start);
                                let e = to_screen(*end);
                                if s.distance(ptr) <= hit_dist {
                                    self.markup_state.active_drag =
                                        Some(MarkupDrag::ArrowEndpoint {
                                            idx: sel_idx,
                                            is_start: true,
                                            start_ptr: ptr,
                                            init_pos: *start,
                                        });
                                    handled = true;
                                } else if e.distance(ptr) <= hit_dist {
                                    self.markup_state.active_drag =
                                        Some(MarkupDrag::ArrowEndpoint {
                                            idx: sel_idx,
                                            is_start: false,
                                            start_ptr: ptr,
                                            init_pos: *end,
                                        });
                                    handled = true;
                                }
                            }
                            _ => {
                                let s_rect = to_screen_rect(shape.bounding_rect_norm(img_w, img_h));
                                let handles = get_box_handles(s_rect);
                                if let Some((h, pos, _)) =
                                    handles.iter().find(|(_, p, _)| p.distance(ptr) <= hit_dist)
                                {
                                    self.markup_state.active_drag = Some(MarkupDrag::Resizing {
                                        idx: sel_idx,
                                        handle: *h,
                                        start_ptr: *pos,
                                        init_rect: shape.bounding_rect_norm(img_w, img_h),
                                    });
                                    handled = true;
                                }
                            }
                        }
                    }
                }

                // 4b. Check if clicking an existing shape body
                if !handled {
                    let clicked_shape =
                        self.markup_state
                            .items
                            .iter()
                            .enumerate()
                            .rev()
                            .find_map(|(i, s)| {
                                let s_rect = to_screen_rect(s.bounding_rect_norm(img_w, img_h));
                                if s_rect.contains(ptr) { Some(i) } else { None }
                            });

                    if let Some(found_idx) = clicked_shape {
                        self.markup_state.selected_idx = Some(found_idx);
                        let init_shape = self.markup_state.items[found_idx].clone();
                        self.markup_state.active_drag = Some(MarkupDrag::Moving {
                            idx: found_idx,
                            start_ptr: ptr,
                            init_shape,
                        });
                        handled = true;

                        if let MarkupShape::Text { .. } = &self.markup_state.items[found_idx] {
                            if self.markup_state.active_tool == MarkupToolType::Text
                                || response.double_clicked()
                            {
                                self.markup_state.editing_text_idx = Some(found_idx);
                            }
                        }
                    }
                }

                // 4c. Start creating a new shape if in drawing tool
                if !handled && img_rect.contains(ptr) {
                    let norm = to_norm(ptr);
                    match self.markup_state.active_tool {
                        MarkupToolType::Select => {
                            self.markup_state.selected_idx = None;
                            self.markup_state.editing_text_idx = None;
                        }
                        drawing_tool => {
                            self.markup_state.selected_idx = None;
                            self.markup_state.editing_text_idx = None;
                            self.markup_state.active_drag = Some(MarkupDrag::Creating {
                                tool: drawing_tool,
                                start_norm: norm,
                                current_norm: norm,
                            });
                        }
                    }
                }
            }
        }

        if response.dragged() {
            if let Some(ptr) = response.interact_pointer_pos() {
                match self.markup_state.active_drag {
                    Some(MarkupDrag::Creating {
                        tool, start_norm, ..
                    }) => {
                        let current_norm = to_norm(ptr);
                        self.markup_state.active_drag = Some(MarkupDrag::Creating {
                            tool,
                            start_norm,
                            current_norm,
                        });
                    }
                    Some(MarkupDrag::Moving {
                        idx,
                        start_ptr,
                        ref init_shape,
                    }) => {
                        let delta_screen = ptr - start_ptr;
                        let delta_norm = Vec2::new(
                            delta_screen.x / img_rect.width(),
                            delta_screen.y / img_rect.height(),
                        );
                        if idx < self.markup_state.items.len() {
                            let mut updated = init_shape.clone();
                            updated.translate(delta_norm);
                            self.markup_state.items[idx] = updated;
                        }
                    }
                    Some(MarkupDrag::ArrowEndpoint {
                        idx,
                        is_start,
                        start_ptr,
                        init_pos,
                    }) => {
                        let delta_screen = ptr - start_ptr;
                        let delta_norm = Vec2::new(
                            delta_screen.x / img_rect.width(),
                            delta_screen.y / img_rect.height(),
                        );
                        let new_pos =
                            (init_pos + delta_norm).clamp(Pos2::ZERO, Pos2::new(1.0, 1.0));
                        if idx < self.markup_state.items.len() {
                            match &mut self.markup_state.items[idx] {
                                MarkupShape::Arrow { start, end, .. }
                                | MarkupShape::Line { start, end, .. } => {
                                    if is_start {
                                        *start = new_pos;
                                    } else {
                                        *end = new_pos;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(MarkupDrag::Resizing {
                        idx,
                        handle,
                        start_ptr,
                        init_rect,
                    }) => {
                        let delta_screen = ptr - start_ptr;
                        let delta_norm_x = delta_screen.x / img_rect.width();
                        let delta_norm_y = delta_screen.y / img_rect.height();

                        let mut min_x = init_rect.min.x;
                        let mut max_x = init_rect.max.x;
                        let mut min_y = init_rect.min.y;
                        let mut max_y = init_rect.max.y;

                        match handle {
                            CropHandle::TopLeft => {
                                min_x = (min_x + delta_norm_x).clamp(0.0, max_x - 0.01);
                                min_y = (min_y + delta_norm_y).clamp(0.0, max_y - 0.01);
                            }
                            CropHandle::TopRight => {
                                max_x = (max_x + delta_norm_x).clamp(min_x + 0.01, 1.0);
                                min_y = (min_y + delta_norm_y).clamp(0.0, max_y - 0.01);
                            }
                            CropHandle::BottomLeft => {
                                min_x = (min_x + delta_norm_x).clamp(0.0, max_x - 0.01);
                                max_y = (max_y + delta_norm_y).clamp(min_y + 0.01, 1.0);
                            }
                            CropHandle::BottomRight => {
                                max_x = (max_x + delta_norm_x).clamp(min_x + 0.01, 1.0);
                                max_y = (max_y + delta_norm_y).clamp(min_y + 0.01, 1.0);
                            }
                            CropHandle::Left => {
                                min_x = (min_x + delta_norm_x).clamp(0.0, max_x - 0.01);
                            }
                            CropHandle::Right => {
                                max_x = (max_x + delta_norm_x).clamp(min_x + 0.01, 1.0);
                            }
                            CropHandle::Top => {
                                min_y = (min_y + delta_norm_y).clamp(0.0, max_y - 0.01);
                            }
                            CropHandle::Bottom => {
                                max_y = (max_y + delta_norm_y).clamp(min_y + 0.01, 1.0);
                            }
                            _ => {}
                        }

                        let new_rect =
                            Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y));
                        if idx < self.markup_state.items.len() {
                            match &mut self.markup_state.items[idx] {
                                MarkupShape::Circle { rect, .. }
                                | MarkupShape::Rectangle { rect, .. }
                                | MarkupShape::Highlighter { rect, .. } => {
                                    *rect = new_rect;
                                }
                                MarkupShape::StepBadge { center, radius, .. } => {
                                    *center = new_rect.center();
                                    let new_r_norm = new_rect.width().min(new_rect.height()) * 0.5;
                                    let new_r_px = new_r_norm * img_h;
                                    if new_r_px >= 8.0 {
                                        *radius = new_r_px;
                                        self.markup_state.active_badge_radius = new_r_px;
                                    }
                                }
                                MarkupShape::Text { pos, font_size, .. } => {
                                    *pos = new_rect.min;
                                    let new_h_px = new_rect.height() * img_h;
                                    if new_h_px >= 10.0 {
                                        *font_size = infer_font_size_from_box_height(new_h_px);
                                        self.markup_state.active_font_size = *font_size;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    None => {}
                }
            }
        }

        if response.drag_stopped() {
            if let Some(MarkupDrag::Creating {
                tool,
                start_norm,
                current_norm,
            }) = self.markup_state.active_drag
            {
                let active_col = self.markup_state.active_color;
                let stroke_w = self.markup_state.active_stroke_width;
                let filled = self.markup_state.active_filled;

                let min_p = Pos2::new(
                    start_norm.x.min(current_norm.x),
                    start_norm.y.min(current_norm.y),
                );
                let max_p = Pos2::new(
                    start_norm.x.max(current_norm.x),
                    start_norm.y.max(current_norm.y),
                );
                let rect = Rect::from_min_max(min_p, max_p);

                let shape = match tool {
                    MarkupToolType::Arrow => Some(MarkupShape::Arrow {
                        start: start_norm,
                        end: current_norm,
                        color: active_col,
                        width: stroke_w,
                    }),
                    MarkupToolType::Line => Some(MarkupShape::Line {
                        start: start_norm,
                        end: current_norm,
                        color: active_col,
                        width: stroke_w,
                    }),
                    MarkupToolType::Rectangle => Some(MarkupShape::Rectangle {
                        rect,
                        color: active_col,
                        width: stroke_w,
                        filled,
                        rounding: 3.0,
                    }),
                    MarkupToolType::Circle => Some(MarkupShape::Circle {
                        rect,
                        color: active_col,
                        width: stroke_w,
                        filled,
                    }),
                    MarkupToolType::Highlighter => Some(MarkupShape::Highlighter {
                        rect,
                        color: active_col,
                    }),
                    MarkupToolType::Text => {
                        let box_h_img = rect.height() * img_h;
                        let font_size = if box_h_img >= 10.0 {
                            infer_font_size_from_box_height(box_h_img)
                        } else {
                            self.markup_state.active_font_size
                        };
                        self.markup_state.active_font_size = font_size;
                        Some(MarkupShape::Text {
                            pos: rect.min,
                            text: String::new(),
                            font_size,
                        })
                    }
                    MarkupToolType::StepBadge => {
                        let s_screen = to_screen(start_norm);
                        let c_screen = to_screen(current_norm);
                        let drag_dist = s_screen.distance(c_screen);
                        let radius_px = if drag_dist >= 8.0_f32 {
                            let r_img = drag_dist / zoom_scale;
                            self.markup_state.active_badge_radius = r_img;
                            r_img
                        } else {
                            self.markup_state.active_badge_radius
                        };

                        let num = self.markup_state.next_available_badge_number();
                        Some(MarkupShape::StepBadge {
                            center: start_norm,
                            radius: radius_px,
                            number: num,
                            color: active_col,
                        })
                    }
                    _ => None,
                };

                if let Some(new_shape) = shape {
                    let is_text = matches!(new_shape, MarkupShape::Text { .. });
                    self.markup_state.items.push(new_shape);
                    self.markup_state.sync_next_badge_number();
                    let new_idx = self.markup_state.items.len() - 1;
                    self.markup_state.selected_idx = Some(new_idx);
                    if is_text {
                        self.markup_state.editing_text_idx = Some(new_idx);
                    }
                }
            }
            self.markup_state.active_drag = None;
        }

        // Cleanup empty text shapes when not actively editing
        if self.markup_state.editing_text_idx.is_none() {
            let before_len = self.markup_state.items.len();
            self.markup_state.items.retain(|item| match item {
                MarkupShape::Text { text, .. } => !text.trim().is_empty(),
                _ => true,
            });
            if self.markup_state.items.len() < before_len {
                if let Some(sel) = self.markup_state.selected_idx {
                    if sel >= self.markup_state.items.len() {
                        self.markup_state.selected_idx = None;
                    }
                }
            }
        }
    }
}
pub(super) fn draw_handle(painter: &egui::Painter, center: Pos2, radius: f32, fill_col: Color32) {
    let r = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    painter.rect_filled(r, Rounding::same(1.5_f32), fill_col);
    painter.rect_stroke(
        r,
        Rounding::same(1.5_f32),
        Stroke::new(1.0_f32, Color32::BLACK),
    );
}

pub(super) fn draw_selection_bounding_box(
    painter: &egui::Painter,
    rect: Rect,
    accent: Color32,
    handle_radius: f32,
) {
    painter.rect_stroke(rect, Rounding::ZERO, Stroke::new(1.5_f32, accent));
    let handles = get_box_handles(rect);
    for (_, pos, _) in &handles {
        draw_handle(painter, *pos, handle_radius, Color32::WHITE);
    }
}

pub(super) fn draw_arrow_painter(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    color: Color32,
    width: f32,
) {
    let dir = end - start;
    let len = dir.length();
    if len < 2.0 {
        return;
    }

    let norm_dir = dir / len;
    let angle = dir.y.atan2(dir.x);
    let head_len = (width * 3.5).clamp(12.0, 36.0);
    let head_angle = 30.0_f32.to_radians();

    // Shorten line slightly so it ends flush behind arrow tip
    let line_end = end - norm_dir * (head_len * 0.4);
    painter.line_segment([start, line_end], Stroke::new(width, color));

    let p1 = end - Vec2::angled(angle - head_angle) * head_len;
    let p2 = end - Vec2::angled(angle + head_angle) * head_len;

    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(end, color);
    mesh.colored_vertex(p1, color);
    mesh.colored_vertex(p2, color);
    mesh.add_triangle(0, 1, 2);
    painter.add(egui::Shape::mesh(mesh));
}

// ── Tiny-Skia & Fontdue Image Rasterization ──
