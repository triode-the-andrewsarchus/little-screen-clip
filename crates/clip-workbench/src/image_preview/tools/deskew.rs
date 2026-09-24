use super::super::*;

#[derive(Clone)]
pub struct DeskewState {
    /// Normalized corner coordinates in \[0.0, 1.0] range relative to image:
    /// \[0] Top-Left, \[1] Top-Right, \[2] Bottom-Right, \[3] Bottom-Left
    pub corners: [Pos2; 4],
    pub dragging_corner: Option<usize>,
    pub crop_mode: DeskewCropMode,
    /// When true, clicking on the image canvas sequentially places corners 1 to 4
    pub is_clicking_points: bool,
    /// Accumulates points clicked so far during point-placement mode
    pub placed_points: Vec<Pos2>,
    /// Tracks if the quad has been customized (either by clicking 4 points or dragging handles)
    pub corners_adjusted: bool,
}

impl Default for DeskewState {
    fn default() -> Self {
        Self {
            corners: [
                Pos2::new(0.08, 0.08),
                Pos2::new(0.92, 0.08),
                Pos2::new(0.92, 0.92),
                Pos2::new(0.08, 0.92),
            ],
            dragging_corner: None,
            crop_mode: DeskewCropMode::CropToQuad,
            is_clicking_points: false,
            placed_points: Vec::new(),
            corners_adjusted: false,
        }
    }
}

impl DeskewState {
    pub fn reset_quad(&mut self) {
        self.corners = [
            Pos2::new(0.08, 0.08),
            Pos2::new(0.92, 0.08),
            Pos2::new(0.92, 0.92),
            Pos2::new(0.08, 0.92),
        ];
        self.dragging_corner = None;
        self.is_clicking_points = false;
        self.placed_points.clear();
        self.corners_adjusted = false;
    }

    pub fn start_clicking_points(&mut self) {
        self.is_clicking_points = true;
        self.placed_points.clear();
        self.dragging_corner = None;
    }

    pub fn cancel_clicking_points(&mut self) {
        self.is_clicking_points = false;
        self.placed_points.clear();
    }
}

impl ImagePreviewState {
    pub fn apply_deskew(&mut self) {
        let w_f = self.current_rgba.width() as f32;
        let h_f = self.current_rgba.height() as f32;
        let c = self.deskew_state.corners;
        let quad = QuadCorners::new(
            [c[0].x * w_f, c[0].y * h_f],
            [c[1].x * w_f, c[1].y * h_f],
            [c[2].x * w_f, c[2].y * h_f],
            [c[3].x * w_f, c[3].y * h_f],
        );

        if self.deskew_state.is_clicking_points {
            self.show_toast("⚠ Place all 4 points first, or press Esc to cancel");
            return;
        }

        let (est_w, est_h) = quad.estimate_dimensions();

        match rectify_perspective_with_mode(
            &self.current_rgba,
            &quad,
            est_w,
            est_h,
            self.deskew_state.crop_mode,
        ) {
            Ok(rectified) => {
                let out_w = rectified.width();
                let out_h = rectified.height();
                self.undo_stack.push(self.current_rgba.clone());
                self.redo_stack.clear();
                self.current_rgba = rectified;
                self.dirty_texture = true;
                self.resize_state.sync_from_image(&self.current_rgba);
                self.deskew_state.reset_quad();
                let mode_str = match self.deskew_state.crop_mode {
                    DeskewCropMode::CropToQuad => "cropped to quad",
                    DeskewCropMode::MinimalCrop => "minimal crop",
                };
                self.show_toast(format!(
                    "📐 Deskewed perspective ({}): {}×{} px",
                    mode_str, out_w, out_h
                ));
            }
            Err(e) => {
                self.show_toast(format!("⚠ Deskew failed: {}", e));
            }
        }
    }

    /// Resizes current_rgba to the specified pixel dimensions using the chosen filter,
    /// recording an undo point.

    pub(crate) fn render_deskew_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let frame_bg = hex_to_c32(&palette.colors.surface_recessed);
        let frame_stroke = hex_to_c32(&palette.colors.surface_panel);
        let btn_bg = hex_to_c32(&palette.colors.surface_panel);
        let btn_border = hex_to_c32(&palette.colors.surface_recessed);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0_f32, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("4-Point Deskew:")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(4.0);

                    // 1. Click 4 Points toggle button
                    let is_placing = self.deskew_state.is_clicking_points;
                    let click_lbl = if is_placing {
                        format!("Placing ({}/4)...", self.deskew_state.placed_points.len())
                    } else {
                        "Click 4 Points".to_string()
                    };
                    let click_btn = crate::ui_ext::mode_tab_button_icon(
                        Some(Icon::Crosshair),
                        &click_lbl,
                        is_placing,
                        palette,
                    )
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                    if ui.add(click_btn)
                        .on_hover_text("Click 4 corners directly on the image: Top-Left → Top-Right → Bottom-Right → Bottom-Left")
                        .clicked()
                    {
                        if is_placing {
                            self.deskew_state.cancel_clicking_points();
                        } else {
                            self.deskew_state.start_clicking_points();
                            self.show_toast("Click 4 corners: TL → TR → BR → BL");
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // 2. Cropping mode buttons (Crop to Quad vs Minimal Crop)
                    ui.label(
                        egui::RichText::new("Cropping:")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                    );

                    let is_crop_to_quad = self.deskew_state.crop_mode == DeskewCropMode::CropToQuad;
                    let crop_quad_btn = crate::ui_ext::mode_tab_button_icon(
                        Some(Icon::Crop),
                        "Crop to Quad",
                        is_crop_to_quad,
                        palette,
                    )
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                    if ui.add(crop_quad_btn)
                        .on_hover_text("Crop tightly to the 4-point quadrilateral")
                        .clicked()
                    {
                        self.deskew_state.crop_mode = DeskewCropMode::CropToQuad;
                    }

                    let is_minimal_crop = self.deskew_state.crop_mode == DeskewCropMode::MinimalCrop;
                    let minimal_crop_btn = crate::ui_ext::mode_tab_button_icon(
                        Some(Icon::Image),
                        "Minimal Crop",
                        is_minimal_crop,
                        palette,
                    )
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                    if ui.add(minimal_crop_btn)
                        .on_hover_text("Preserve the entire rectified image, cropping only as strictly necessary")
                        .clicked()
                    {
                        self.deskew_state.crop_mode = DeskewCropMode::MinimalCrop;
                    }

                    ui.add_space(6.0);

                    // 3. Target dimensions calculation preview
                    let w_f = self.current_rgba.width() as f32;
                    let h_f = self.current_rgba.height() as f32;
                    let c = self.deskew_state.corners;
                    let quad = QuadCorners::new(
                        [c[0].x * w_f, c[0].y * h_f],
                        [c[1].x * w_f, c[1].y * h_f],
                        [c[2].x * w_f, c[2].y * h_f],
                        [c[3].x * w_f, c[3].y * h_f],
                    );
                    let (est_w, est_h) = quad.estimate_dimensions();
                    let (disp_w, disp_h) = match self.deskew_state.crop_mode {
                        DeskewCropMode::CropToQuad => (est_w, est_h),
                        DeskewCropMode::MinimalCrop => quad.estimate_minimal_crop_dimensions(
                            self.current_rgba.width(),
                            self.current_rgba.height(),
                            est_w,
                            est_h,
                        ),
                    };

                    crate::ui_ext::badge_pill(
                        ui,
                        &format!("Output: ~{}×{} px", disp_w, disp_h),
                        Some(hex_to_c32(&palette.harmonies.split_comp_1)),
                        palette,
                    );

                    ui.add_space(8.0);

                    // 4. Apply Deskew button
                    let apply_bg = hex_to_c32(&palette.diff_add);
                    let apply_btn = crate::ui_ext::action_button_icon(
                        Some(Icon::Check),
                        "Apply Deskew (Enter)",
                        apply_bg,
                        palette,
                    )
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                    if ui.add(apply_btn).on_hover_text("Rectify quad perspective into rectangular output (Enter)").clicked() {
                        self.apply_deskew();
                    }

                    // 5. Reset button
                    let reset_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.colors.text_secondary)), palette);
                    let reset_btn = egui::Button::new(
                        egui::RichText::new("⟲ Reset Quad").size(crate::ui_ext::BODY_FONT_SIZE).color(reset_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0_f32, btn_border))
                    .rounding(Rounding::same(4.0))
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                    if ui.add(reset_btn).on_hover_text("Reset corner handles to default positions").clicked() {
                        self.deskew_state.reset_quad();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("Click 4 points or drag handles")
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(hex_to_c32(&palette.colors.text_muted)),
                        );
                    });
                });
            });
    }

    pub(crate) fn render_deskew_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let labels = ["TL (1)", "TR (2)", "BR (3)", "BL (4)"];
        let corner_names = ["Top-Left", "Top-Right", "Bottom-Right", "Bottom-Left"];

        if self.deskew_state.is_clicking_points {
            // --- 4-Point Sequential Clicking Mode ---
            let n = self.deskew_state.placed_points.len();
            let placed_screen: Vec<Pos2> = self
                .deskew_state
                .placed_points
                .iter()
                .map(|p| {
                    Pos2::new(
                        img_rect.min.x + p.x * img_rect.width(),
                        img_rect.min.y + p.y * img_rect.height(),
                    )
                })
                .collect();

            // Draw existing placed points
            let pt_stroke = Stroke::new(2.0_f32, hex_to_c32(&palette.diff_add));
            for (i, &pt) in placed_screen.iter().enumerate() {
                painter.circle_stroke(
                    pt,
                    8.0,
                    Stroke::new(2.0_f32, hex_to_c32(&palette.colors.surface_recessed)),
                );
                painter.circle_filled(pt, 6.0, hex_to_c32(&palette.diff_add));
                painter.text(
                    pt + Vec2::new(8.0, -18.0),
                    egui::Align2::LEFT_TOP,
                    labels[i],
                    egui::FontId::proportional(11.0),
                    hex_to_c32(&palette.colors.text_primary),
                );
            }

            // Draw connecting lines between placed points
            if n >= 2 {
                painter.line_segment([placed_screen[0], placed_screen[1]], pt_stroke);
            }
            if n >= 3 {
                painter.line_segment([placed_screen[1], placed_screen[2]], pt_stroke);
            }

            // Cursor rubber-banding and guide badge
            if let Some(ptr_pos) = response.hover_pos() {
                if img_rect.contains(ptr_pos) {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);

                    // Rubber-band line from last placed point to cursor
                    let rubber_stroke =
                        Stroke::new(1.5_f32, hex_to_c32_alpha(&palette.diff_add, 180));
                    if n >= 1 {
                        painter
                            .line_segment([*placed_screen.last().unwrap(), ptr_pos], rubber_stroke);
                    }
                    // Closing segment preview if 3 points placed
                    if n == 3 {
                        painter.line_segment([ptr_pos, placed_screen[0]], rubber_stroke);
                    }

                    // Floating guide badge following pointer
                    let next_name = corner_names.get(n).copied().unwrap_or("Next");
                    let badge_text =
                        format!("🎯 Click {next_idx}/4: {next_name}", next_idx = n + 1);
                    let b_low = hex_to_c32(&palette.colors.surface_recessed);
                    let badge_fg = contrast_text_c32(
                        b_low,
                        Some(hex_to_c32(&palette.colors.text_primary)),
                        palette,
                    );
                    let galley = ui.painter().layout_no_wrap(
                        badge_text,
                        egui::FontId::proportional(11.0),
                        badge_fg,
                    );
                    let badge_pos = Pos2::new(
                        (ptr_pos.x + 16.0).clamp(
                            img_rect.min.x + 10.0,
                            img_rect.max.x - galley.size().x - 14.0,
                        ),
                        (ptr_pos.y + 16.0).clamp(img_rect.min.y + 10.0, img_rect.max.y - 26.0),
                    );
                    let badge_rect =
                        Rect::from_min_size(badge_pos, galley.size() + Vec2::new(12.0, 6.0));
                    painter.rect_filled(badge_rect, Rounding::same(4.0), b_low);
                    painter.rect_stroke(
                        badge_rect,
                        Rounding::same(4.0),
                        Stroke::new(1.0_f32, hex_to_c32(&palette.diff_add)),
                    );
                    painter.galley(badge_rect.min + Vec2::new(6.0, 3.0), galley, badge_fg);
                }
            }

            // Top banner instruction
            let top_banner_pos = Pos2::new(img_rect.center().x, img_rect.min.y + 18.0);
            let next_name = corner_names.get(n).copied().unwrap_or("");
            let banner_text = format!(
                "🎯 Click 4 points: (1) TL → (2) TR → (3) BR → (4) BL | Next: {} of 4 ({}) [Esc: Cancel]",
                n + 1,
                next_name
            );
            let b_low = hex_to_c32(&palette.colors.surface_recessed);
            let b_fg = contrast_text_c32(
                b_low,
                Some(hex_to_c32(&palette.colors.text_primary)),
                palette,
            );
            let banner_galley =
                ui.painter()
                    .layout_no_wrap(banner_text, egui::FontId::proportional(11.0), b_fg);
            let banner_rect =
                Rect::from_center_size(top_banner_pos, banner_galley.size() + Vec2::new(16.0, 6.0));
            painter.rect_filled(banner_rect, Rounding::same(4.0), b_low);
            painter.rect_stroke(
                banner_rect,
                Rounding::same(4.0),
                Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)),
            );
            painter.galley(banner_rect.min + Vec2::new(8.0, 3.0), banner_galley, b_fg);

            // Register point placement on click
            if response.clicked() {
                if let Some(ptr_pos) = response.interact_pointer_pos() {
                    if img_rect.contains(ptr_pos) {
                        let norm_x =
                            ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let norm_y =
                            ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.deskew_state
                            .placed_points
                            .push(Pos2::new(norm_x, norm_y));

                        if self.deskew_state.placed_points.len() == 4 {
                            self.deskew_state.corners = [
                                self.deskew_state.placed_points[0],
                                self.deskew_state.placed_points[1],
                                self.deskew_state.placed_points[2],
                                self.deskew_state.placed_points[3],
                            ];
                            self.deskew_state.is_clicking_points = false;
                            self.deskew_state.placed_points.clear();
                            self.deskew_state.corners_adjusted = true;
                            self.show_toast(
                                "✓ 4 points set! Drag handles to fine-tune, Enter to apply.",
                            );
                        }
                    }
                }
            }
        } else {
            // --- Standard Quad Handles & Inspection Mode ---
            let corners = self.deskew_state.corners;
            let screen_pts = [
                Pos2::new(
                    img_rect.min.x + corners[0].x * img_rect.width(),
                    img_rect.min.y + corners[0].y * img_rect.height(),
                ),
                Pos2::new(
                    img_rect.min.x + corners[1].x * img_rect.width(),
                    img_rect.min.y + corners[1].y * img_rect.height(),
                ),
                Pos2::new(
                    img_rect.min.x + corners[2].x * img_rect.width(),
                    img_rect.min.y + corners[2].y * img_rect.height(),
                ),
                Pos2::new(
                    img_rect.min.x + corners[3].x * img_rect.width(),
                    img_rect.min.y + corners[3].y * img_rect.height(),
                ),
            ];

            // 1. Perspective Grid (rule of thirds inside quad)
            let grid_stroke = Stroke::new(
                1.0_f32,
                hex_to_c32_alpha(&palette.colors.text_secondary, 80),
            );
            for &u in &[1.0 / 3.0, 2.0 / 3.0] {
                let top_pt = screen_pts[0].to_vec2() * (1.0 - u) + screen_pts[1].to_vec2() * u;
                let bot_pt = screen_pts[3].to_vec2() * (1.0 - u) + screen_pts[2].to_vec2() * u;
                painter.line_segment([top_pt.to_pos2(), bot_pt.to_pos2()], grid_stroke);

                let left_pt = screen_pts[0].to_vec2() * (1.0 - u) + screen_pts[3].to_vec2() * u;
                let right_pt = screen_pts[1].to_vec2() * (1.0 - u) + screen_pts[2].to_vec2() * u;
                painter.line_segment([left_pt.to_pos2(), right_pt.to_pos2()], grid_stroke);
            }

            // 2. Quad Outer Edges
            let edge_stroke = Stroke::new(2.0_f32, hex_to_c32(&palette.harmonies.base));
            painter.line_segment([screen_pts[0], screen_pts[1]], edge_stroke);
            painter.line_segment([screen_pts[1], screen_pts[2]], edge_stroke);
            painter.line_segment([screen_pts[2], screen_pts[3]], edge_stroke);
            painter.line_segment([screen_pts[3], screen_pts[0]], edge_stroke);

            // 3. Corner Handles & Labels
            let handle_radius = 6.0;
            for i in 0..4 {
                let pt = screen_pts[i];
                let is_dragging = self.deskew_state.dragging_corner == Some(i);
                let handle_col = if is_dragging {
                    hex_to_c32(&palette.diff_add)
                } else {
                    hex_to_c32(&palette.harmonies.base)
                };

                // Outer dark ring for contrast on any background
                painter.circle_stroke(
                    pt,
                    handle_radius + 2.0,
                    Stroke::new(2.0_f32, hex_to_c32(&palette.colors.surface_recessed)),
                );
                painter.circle_filled(pt, handle_radius, handle_col);

                let offset = match i {
                    0 => Vec2::new(-24.0, -18.0),
                    1 => Vec2::new(8.0, -18.0),
                    2 => Vec2::new(8.0, 8.0),
                    _ => Vec2::new(-24.0, 8.0),
                };
                painter.text(
                    pt + offset,
                    egui::Align2::LEFT_TOP,
                    labels[i],
                    egui::FontId::proportional(11.0),
                    hex_to_c32(&palette.colors.text_primary),
                );
            }

            // 4. Centroid Guide Badge
            let centroid = Pos2::new(
                (screen_pts[0].x + screen_pts[1].x + screen_pts[2].x + screen_pts[3].x) * 0.25,
                (screen_pts[0].y + screen_pts[1].y + screen_pts[2].y + screen_pts[3].y) * 0.25,
            );
            let badge_bg = hex_to_c32(&palette.colors.surface_recessed);
            let badge_fg = contrast_text_c32(
                badge_bg,
                Some(hex_to_c32(&palette.colors.text_primary)),
                palette,
            );
            let badge_str = if !self.deskew_state.corners_adjusted {
                "🎯 Click 4 Points (or drag handles) | Enter: Apply"
            } else {
                "Enter / Double-Click: Apply Deskew | Drag handles to adjust"
            };
            let galley = ui.painter().layout_no_wrap(
                badge_str.to_string(),
                egui::FontId::proportional(11.0),
                badge_fg,
            );
            let badge_rect = Rect::from_center_size(centroid, galley.size() + Vec2::new(14.0, 6.0));
            painter.rect_filled(badge_rect, Rounding::same(4.0), badge_bg);
            painter.rect_stroke(
                badge_rect,
                Rounding::same(4.0),
                Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)),
            );
            painter.galley(badge_rect.min + Vec2::new(7.0, 3.0), galley, badge_fg);

            // 5. Interaction Logic
            let hit_dist = 16.0;
            let mut hovered_corner: Option<usize> = None;

            if let Some(ptr_pos) = response.hover_pos() {
                hovered_corner = screen_pts.iter().enumerate().find_map(|(idx, pt)| {
                    if pt.distance(ptr_pos) <= hit_dist {
                        Some(idx)
                    } else {
                        None
                    }
                });

                if let Some(corner_idx) = hovered_corner {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grab);
                    if response.drag_started() {
                        self.deskew_state.dragging_corner = Some(corner_idx);
                        self.deskew_state.corners_adjusted = true;
                    }
                } else if self.deskew_state.dragging_corner.is_none() && img_rect.contains(ptr_pos)
                {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                    // If corners haven't been adjusted yet, hint to the user that clicking sets point 1
                    if !self.deskew_state.corners_adjusted {
                        let hint_text = "🎯 Click to place Point 1 (Top-Left)";
                        let b_low = hex_to_c32(&palette.colors.surface_recessed);
                        let h_fg = contrast_text_c32(
                            b_low,
                            Some(hex_to_c32(&palette.colors.text_primary)),
                            palette,
                        );
                        let h_galley = ui.painter().layout_no_wrap(
                            hint_text.to_string(),
                            egui::FontId::proportional(10.5),
                            h_fg,
                        );
                        let h_pos = Pos2::new(
                            (ptr_pos.x + 14.0).clamp(
                                img_rect.min.x + 10.0,
                                img_rect.max.x - h_galley.size().x - 14.0,
                            ),
                            (ptr_pos.y + 14.0).clamp(img_rect.min.y + 10.0, img_rect.max.y - 24.0),
                        );
                        let h_rect =
                            Rect::from_min_size(h_pos, h_galley.size() + Vec2::new(10.0, 4.0));
                        painter.rect_filled(h_rect, Rounding::same(3.0), b_low);
                        painter.rect_stroke(
                            h_rect,
                            Rounding::same(3.0),
                            Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
                        );
                        painter.galley(h_rect.min + Vec2::new(5.0, 2.0), h_galley, h_fg);
                    }
                }
            }

            if response.dragged() {
                if let Some(corner_idx) = self.deskew_state.dragging_corner {
                    if let Some(ptr_pos) = response.interact_pointer_pos() {
                        let norm_x =
                            ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let norm_y =
                            ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.deskew_state.corners[corner_idx] = Pos2::new(norm_x, norm_y);
                    }
                }
            }

            if response.drag_stopped() {
                self.deskew_state.dragging_corner = None;
            }

            // If user clicks on canvas when corners are at default state, immediately start point 1!
            if response.clicked() && hovered_corner.is_none() && !self.deskew_state.corners_adjusted
            {
                if let Some(ptr_pos) = response.interact_pointer_pos() {
                    if img_rect.contains(ptr_pos) {
                        let norm_x =
                            ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                        let norm_y =
                            ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                        self.deskew_state.is_clicking_points = true;
                        self.deskew_state.placed_points = vec![Pos2::new(norm_x, norm_y)];
                    }
                }
            }

            if response.double_clicked() {
                self.apply_deskew();
            }
        }
    }
}
