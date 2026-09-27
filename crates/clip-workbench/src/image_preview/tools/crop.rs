use super::super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CropRatio {
    #[default]
    Free,
    Ratio1x1,
    Ratio16x9,
    Ratio9x16,
}

impl CropRatio {
    pub fn label(&self) -> &'static str {
        match self {
            CropRatio::Free => "Free",
            CropRatio::Ratio1x1 => "1:1",
            CropRatio::Ratio16x9 => "16:9",
            CropRatio::Ratio9x16 => "9:16",
        }
    }

    pub fn value(&self) -> Option<f32> {
        match self {
            CropRatio::Free => None,
            CropRatio::Ratio1x1 => Some(1.0),
            CropRatio::Ratio16x9 => Some(16.0 / 9.0),
            CropRatio::Ratio9x16 => Some(9.0 / 16.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Bottom,
    Left,
    Right,
    MoveBox,
}

#[derive(Debug, Clone)]
pub struct CropState {
    /// Normalized coordinates in [0.0, 1.0] range relative to image
    pub normalized_rect: Rect,
    pub ratio: CropRatio,
    pub active_drag: Option<(CropHandle, Pos2, Rect)>, // handle, start_ptr_pos, initial_norm_rect
}

impl Default for CropState {
    fn default() -> Self {
        Self {
            normalized_rect: Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
            ratio: CropRatio::Free,
            active_drag: None,
        }
    }
}

impl CropState {
    pub fn set_ratio(&mut self, ratio: CropRatio, img_w: u32, img_h: u32) {
        self.ratio = ratio;
        if let Some(target_ratio) = ratio.value() {
            let img_w_f = img_w as f32;
            let img_h_f = img_h as f32;
            let img_ratio = img_w_f / img_h_f;

            let (box_w, box_h) = if img_ratio > target_ratio {
                // Image is wider than target ratio: constrained by height
                (target_ratio * img_h_f, img_h_f)
            } else {
                // Image is taller than target ratio: constrained by width
                (img_w_f, img_w_f / target_ratio)
            };

            let norm_w = (box_w / img_w_f).clamp(0.01, 1.0);
            let norm_h = (box_h / img_h_f).clamp(0.01, 1.0);
            let min_x = (1.0 - norm_w) / 2.0;
            let min_y = (1.0 - norm_h) / 2.0;
            self.normalized_rect =
                Rect::from_min_size(Pos2::new(min_x, min_y), Vec2::new(norm_w, norm_h));
        }
    }

    pub fn update_drag(&mut self, ptr_pos: Pos2, img_rect: Rect, img_w: u32, img_h: u32) {
        let (handle, start_ptr, init_rect) = match self.active_drag {
            Some(d) => d,
            None => return,
        };

        if img_rect.width() <= 0.0 || img_rect.height() <= 0.0 {
            return;
        }

        let delta_screen = ptr_pos - start_ptr;
        let delta_norm_x = delta_screen.x / img_rect.width();
        let delta_norm_y = delta_screen.y / img_rect.height();

        let img_w_f = img_w as f32;
        let img_h_f = img_h as f32;

        let min_norm_w = (16.0 / img_w_f).min(0.5);
        let min_norm_h = (16.0 / img_h_f).min(0.5);

        if handle == CropHandle::MoveBox {
            let box_w = init_rect.width();
            let box_h = init_rect.height();
            let new_min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
            let new_min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
            self.normalized_rect =
                Rect::from_min_size(Pos2::new(new_min_x, new_min_y), Vec2::new(box_w, box_h));
            return;
        }

        let mut min_x = init_rect.min.x;
        let mut max_x = init_rect.max.x;
        let mut min_y = init_rect.min.y;
        let mut max_y = init_rect.max.y;

        match handle {
            CropHandle::TopLeft => {
                min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
                min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
            }
            CropHandle::TopRight => {
                max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
                min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
            }
            CropHandle::BottomLeft => {
                min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
                max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
            }
            CropHandle::BottomRight => {
                max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
                max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
            }
            CropHandle::Top => {
                min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
            }
            CropHandle::Bottom => {
                max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
            }
            CropHandle::Left => {
                min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
            }
            CropHandle::Right => {
                max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
            }
            CropHandle::MoveBox => unreachable!(),
        }

        // Apply ratio constraint if not Free
        if let Some(target_ratio) = self.ratio.value() {
            let current_norm_w = max_x - min_x;
            let current_norm_h = max_y - min_y;

            match handle {
                CropHandle::TopLeft => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_min_y = max_y - desired_norm_h;
                    if candidate_min_y >= 0.0 {
                        min_y = candidate_min_y;
                    } else {
                        min_y = 0.0;
                        let allowed_norm_h = max_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        min_x = (max_x - allowed_norm_w).max(0.0);
                    }
                }
                CropHandle::TopRight => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_min_y = max_y - desired_norm_h;
                    if candidate_min_y >= 0.0 {
                        min_y = candidate_min_y;
                    } else {
                        min_y = 0.0;
                        let allowed_norm_h = max_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        max_x = (min_x + allowed_norm_w).min(1.0);
                    }
                }
                CropHandle::BottomLeft => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_max_y = min_y + desired_norm_h;
                    if candidate_max_y <= 1.0 {
                        max_y = candidate_max_y;
                    } else {
                        max_y = 1.0;
                        let allowed_norm_h = 1.0 - min_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        min_x = (max_x - allowed_norm_w).max(0.0);
                    }
                }
                CropHandle::BottomRight => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let candidate_max_y = min_y + desired_norm_h;
                    if candidate_max_y <= 1.0 {
                        max_y = candidate_max_y;
                    } else {
                        max_y = 1.0;
                        let allowed_norm_h = 1.0 - min_y;
                        let allowed_norm_w = (allowed_norm_h * target_ratio * img_h_f) / img_w_f;
                        max_x = (min_x + allowed_norm_w).min(1.0);
                    }
                }
                CropHandle::Top | CropHandle::Bottom => {
                    let desired_norm_w = (current_norm_h * target_ratio * img_h_f) / img_w_f;
                    let center_x = (min_x + max_x) / 2.0;
                    min_x = (center_x - desired_norm_w / 2.0).max(0.0);
                    max_x = (center_x + desired_norm_w / 2.0).min(1.0);
                }
                CropHandle::Left | CropHandle::Right => {
                    let desired_norm_h = (current_norm_w * img_w_f) / (target_ratio * img_h_f);
                    let center_y = (min_y + max_y) / 2.0;
                    min_y = (center_y - desired_norm_h / 2.0).max(0.0);
                    max_y = (center_y + desired_norm_h / 2.0).min(1.0);
                }
                CropHandle::MoveBox => unreachable!(),
            }
        }

        self.normalized_rect = Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y));
    }
}

pub fn get_box_handles(rect: Rect) -> [(CropHandle, Pos2, egui::CursorIcon); 8] {
    [
        (
            CropHandle::TopLeft,
            rect.left_top(),
            egui::CursorIcon::ResizeNorthWest,
        ),
        (
            CropHandle::TopRight,
            rect.right_top(),
            egui::CursorIcon::ResizeNorthEast,
        ),
        (
            CropHandle::BottomLeft,
            rect.left_bottom(),
            egui::CursorIcon::ResizeSouthWest,
        ),
        (
            CropHandle::BottomRight,
            rect.right_bottom(),
            egui::CursorIcon::ResizeSouthEast,
        ),
        (
            CropHandle::Top,
            Pos2::new(rect.center().x, rect.top()),
            egui::CursorIcon::ResizeNorth,
        ),
        (
            CropHandle::Bottom,
            Pos2::new(rect.center().x, rect.bottom()),
            egui::CursorIcon::ResizeSouth,
        ),
        (
            CropHandle::Left,
            Pos2::new(rect.left(), rect.center().y),
            egui::CursorIcon::ResizeWest,
        ),
        (
            CropHandle::Right,
            Pos2::new(rect.right(), rect.center().y),
            egui::CursorIcon::ResizeEast,
        ),
    ]
}

pub fn resize_norm_rect(
    init_rect: Rect,
    handle: CropHandle,
    delta_screen: Vec2,
    img_rect: Rect,
    min_norm_w: f32,
    min_norm_h: f32,
) -> Rect {
    let delta_norm_x = delta_screen.x / img_rect.width();
    let delta_norm_y = delta_screen.y / img_rect.height();

    if handle == CropHandle::MoveBox {
        let box_w = init_rect.width();
        let box_h = init_rect.height();
        let new_min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, 1.0 - box_w);
        let new_min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, 1.0 - box_h);
        return Rect::from_min_size(Pos2::new(new_min_x, new_min_y), Vec2::new(box_w, box_h));
    }

    let mut min_x = init_rect.min.x;
    let mut max_x = init_rect.max.x;
    let mut min_y = init_rect.min.y;
    let mut max_y = init_rect.max.y;

    match handle {
        CropHandle::TopLeft => {
            min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
            min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
        }
        CropHandle::TopRight => {
            max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
            min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
        }
        CropHandle::BottomLeft => {
            min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
            max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
        }
        CropHandle::BottomRight => {
            max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
            max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
        }
        CropHandle::Top => {
            min_y = (init_rect.min.y + delta_norm_y).clamp(0.0, max_y - min_norm_h);
        }
        CropHandle::Bottom => {
            max_y = (init_rect.max.y + delta_norm_y).clamp(min_y + min_norm_h, 1.0);
        }
        CropHandle::Left => {
            min_x = (init_rect.min.x + delta_norm_x).clamp(0.0, max_x - min_norm_w);
        }
        CropHandle::Right => {
            max_x = (init_rect.max.x + delta_norm_x).clamp(min_x + min_norm_w, 1.0);
        }
        CropHandle::MoveBox => unreachable!(),
    }

    Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y))
}

impl ImagePreviewState {
    pub fn apply_crop(&mut self) {
        let img_w = self.current_rgba.width() as f32;
        let img_h = self.current_rgba.height() as f32;
        let norm = self.crop_state.normalized_rect;

        let px_x = (norm.min.x * img_w).round().clamp(0.0, img_w - 1.0) as u32;
        let px_y = (norm.min.y * img_h).round().clamp(0.0, img_h - 1.0) as u32;
        let px_w = ((norm.max.x - norm.min.x) * img_w)
            .round()
            .clamp(1.0, img_w - px_x as f32) as u32;
        let px_h = ((norm.max.y - norm.min.y) * img_h)
            .round()
            .clamp(1.0, img_h - px_y as f32) as u32;

        if px_w > 0
            && px_h > 0
            && (px_w < self.current_rgba.width()
                || px_h < self.current_rgba.height()
                || px_x > 0
                || px_y > 0)
        {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();

            let cropped =
                image::imageops::crop_imm(&self.current_rgba, px_x, px_y, px_w, px_h).to_image();
            self.current_rgba = cropped;
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.show_toast(format!("✂ Cropped to {}×{}", px_w, px_h));
        }
    }

    pub(crate) fn render_crop_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET_X,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("✂ Crop Mode:")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Ratio:")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                    );

                    let ratios = [
                        CropRatio::Free,
                        CropRatio::Ratio1x1,
                        CropRatio::Ratio16x9,
                        CropRatio::Ratio9x16,
                    ];

                    for r in ratios {
                        let is_sel = self.crop_state.ratio == r;
                        let btn = crate::mode_tab_button(r.label(), is_sel, palette)
                            .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                        if ui.add(btn).clicked() {
                            self.crop_state.set_ratio(
                                r,
                                self.current_rgba.width(),
                                self.current_rgba.height(),
                            );
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Apply Crop Button
                        let apply_btn = crate::action_button(
                            "✓ Apply Crop (Enter)",
                            hex_to_c32(&palette.diff_add),
                            palette,
                        )
                        .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                        if ui.add(apply_btn).clicked() {
                            self.apply_crop();
                        }

                        // Reset Crop Button
                        let reset_crop_btn = crate::secondary_button(
                            "✕ Full Image",
                            hex_to_c32(&palette.colors.surface_panel),
                            palette,
                        )
                        .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                        if ui.add(reset_crop_btn).clicked() {
                            self.crop_state.normalized_rect =
                                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                            self.crop_state.ratio = CropRatio::Free;
                        }

                        if !self.undo_stack.is_empty() {
                            let undo_bg = hex_to_c32(&palette.colors.surface_panel);
                            let undo_fg = contrast_text_c32(
                                undo_bg,
                                Some(hex_to_c32(&palette.diff_mod)),
                                palette,
                            );
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(undo_fg),
                            )
                            .fill(undo_bg)
                            .stroke(Stroke::new(
                                1.0_f32,
                                hex_to_c32(&palette.colors.surface_recessed),
                            ))
                            .rounding(Rounding::same(4.0))
                            .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }

    pub(crate) fn render_crop_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let norm = self.crop_state.normalized_rect;
        let crop_screen_min = Pos2::new(
            img_rect.min.x + norm.min.x * img_rect.width(),
            img_rect.min.y + norm.min.y * img_rect.height(),
        );
        let crop_screen_max = Pos2::new(
            img_rect.min.x + norm.max.x * img_rect.width(),
            img_rect.min.y + norm.max.y * img_rect.height(),
        );
        let crop_screen_rect = Rect::from_min_max(crop_screen_min, crop_screen_max);

        // 1. Draw Exterior Darkened Mask
        let mask_col = Color32::from_black_alpha(170);
        if crop_screen_rect.min.y > img_rect.min.y {
            painter.rect_filled(
                Rect::from_min_max(
                    img_rect.min,
                    Pos2::new(img_rect.max.x, crop_screen_rect.min.y),
                ),
                Rounding::ZERO,
                mask_col,
            );
        }
        if crop_screen_rect.max.y < img_rect.max.y {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(img_rect.min.x, crop_screen_rect.max.y),
                    img_rect.max,
                ),
                Rounding::ZERO,
                mask_col,
            );
        }
        if crop_screen_rect.min.x > img_rect.min.x {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(img_rect.min.x, crop_screen_rect.min.y),
                    Pos2::new(crop_screen_rect.min.x, crop_screen_rect.max.y),
                ),
                Rounding::ZERO,
                mask_col,
            );
        }
        if crop_screen_rect.max.x < img_rect.max.x {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(crop_screen_rect.max.x, crop_screen_rect.min.y),
                    Pos2::new(img_rect.max.x, crop_screen_rect.max.y),
                ),
                Rounding::ZERO,
                mask_col,
            );
        }

        // 2. Rule of Thirds Grid
        let w = crop_screen_rect.width();
        let h = crop_screen_rect.height();
        let grid_stroke = Stroke::new(
            1.0_f32,
            hex_to_c32_alpha(&palette.colors.text_secondary, 60),
        );
        painter.line_segment(
            [
                Pos2::new(crop_screen_rect.min.x + w / 3.0, crop_screen_rect.min.y),
                Pos2::new(crop_screen_rect.min.x + w / 3.0, crop_screen_rect.max.y),
            ],
            grid_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(
                    crop_screen_rect.min.x + w * 2.0 / 3.0,
                    crop_screen_rect.min.y,
                ),
                Pos2::new(
                    crop_screen_rect.min.x + w * 2.0 / 3.0,
                    crop_screen_rect.max.y,
                ),
            ],
            grid_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(crop_screen_rect.min.x, crop_screen_rect.min.y + h / 3.0),
                Pos2::new(crop_screen_rect.max.x, crop_screen_rect.min.y + h / 3.0),
            ],
            grid_stroke,
        );
        painter.line_segment(
            [
                Pos2::new(
                    crop_screen_rect.min.x,
                    crop_screen_rect.min.y + h * 2.0 / 3.0,
                ),
                Pos2::new(
                    crop_screen_rect.max.x,
                    crop_screen_rect.min.y + h * 2.0 / 3.0,
                ),
            ],
            grid_stroke,
        );

        // 3. Crisp Crop Outline
        painter.rect_stroke(
            crop_screen_rect,
            Rounding::ZERO,
            Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
        );

        // 4. Handles definitions & drawing
        let handle_radius = 5.0;
        let handles: [(CropHandle, Pos2, egui::CursorIcon); 8] = [
            (
                CropHandle::TopLeft,
                crop_screen_rect.left_top(),
                egui::CursorIcon::ResizeNorthWest,
            ),
            (
                CropHandle::TopRight,
                crop_screen_rect.right_top(),
                egui::CursorIcon::ResizeNorthEast,
            ),
            (
                CropHandle::BottomLeft,
                crop_screen_rect.left_bottom(),
                egui::CursorIcon::ResizeSouthWest,
            ),
            (
                CropHandle::BottomRight,
                crop_screen_rect.right_bottom(),
                egui::CursorIcon::ResizeSouthEast,
            ),
            (
                CropHandle::Top,
                Pos2::new(crop_screen_rect.center().x, crop_screen_rect.top()),
                egui::CursorIcon::ResizeNorth,
            ),
            (
                CropHandle::Bottom,
                Pos2::new(crop_screen_rect.center().x, crop_screen_rect.bottom()),
                egui::CursorIcon::ResizeSouth,
            ),
            (
                CropHandle::Left,
                Pos2::new(crop_screen_rect.left(), crop_screen_rect.center().y),
                egui::CursorIcon::ResizeWest,
            ),
            (
                CropHandle::Right,
                Pos2::new(crop_screen_rect.right(), crop_screen_rect.center().y),
                egui::CursorIcon::ResizeEast,
            ),
        ];

        for (_, pos, _) in &handles {
            let handle_rect = Rect::from_center_size(*pos, Vec2::splat(handle_radius * 2.0));
            painter.rect_filled(
                handle_rect,
                Rounding::same(1.0),
                hex_to_c32(&palette.harmonies.base),
            );
            painter.rect_stroke(
                handle_rect,
                Rounding::same(1.0),
                Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
            );
        }

        // 5. Floating Dimensions Badge
        let px_w = ((norm.max.x - norm.min.x) * self.current_rgba.width() as f32).round() as u32;
        let px_h = ((norm.max.y - norm.min.y) * self.current_rgba.height() as f32).round() as u32;
        let badge_str = format!(
            "{} × {} px ({})",
            px_w,
            px_h,
            format_aspect_ratio(px_w, px_h)
        );
        let badge_pos = Pos2::new(
            crop_screen_rect.center().x,
            (crop_screen_rect.min.y - 14.0).max(img_rect.min.y + 10.0),
        );
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let badge_text_col = contrast_text_c32(
            b_low,
            Some(hex_to_c32(&palette.colors.text_primary)),
            palette,
        );
        let galley = ui.painter().layout_no_wrap(
            badge_str,
            egui::FontId::proportional(11.0),
            badge_text_col,
        );
        let badge_rect = Rect::from_center_size(badge_pos, galley.size() + Vec2::new(12.0, 4.0));
        painter.rect_filled(badge_rect, Rounding::same(4.0), b_low);
        painter.rect_stroke(
            badge_rect,
            Rounding::same(4.0),
            Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)),
        );
        painter.galley(badge_rect.min + Vec2::new(6.0, 2.0), galley, badge_text_col);

        // 6. Interaction logic
        if let Some(ptr_pos) = response.hover_pos() {
            let hit_dist = 12.0;
            let hovered_handle = handles.iter().find_map(|(h, pos, cur)| {
                if pos.distance(ptr_pos) <= hit_dist {
                    Some((*h, *cur))
                } else {
                    None
                }
            });

            if let Some((h, cur)) = hovered_handle {
                ui.output_mut(|o| o.cursor_icon = cur);
                if response.drag_started() {
                    self.crop_state.active_drag =
                        Some((h, ptr_pos, self.crop_state.normalized_rect));
                }
            } else if crop_screen_rect.contains(ptr_pos) {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
                if response.drag_started() {
                    self.crop_state.active_drag = Some((
                        CropHandle::MoveBox,
                        ptr_pos,
                        self.crop_state.normalized_rect,
                    ));
                }
            } else if img_rect.contains(ptr_pos) {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                if response.drag_started() {
                    let norm_x = ((ptr_pos.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0);
                    let norm_y = ((ptr_pos.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0);
                    let initial =
                        Rect::from_min_max(Pos2::new(norm_x, norm_y), Pos2::new(norm_x, norm_y));
                    self.crop_state.normalized_rect = initial;
                    self.crop_state.active_drag = Some((CropHandle::BottomRight, ptr_pos, initial));
                }
            }
        }

        if response.dragged() {
            if let Some(ptr_pos) = response.interact_pointer_pos() {
                self.crop_state.update_drag(
                    ptr_pos,
                    img_rect,
                    self.current_rgba.width(),
                    self.current_rgba.height(),
                );
            }
        }

        if response.drag_stopped() {
            self.crop_state.active_drag = None;
        }

        if response.double_clicked() {
            if let Some(ptr) = response.interact_pointer_pos() {
                if crop_screen_rect.contains(ptr) {
                    self.apply_crop();
                }
            }
        }
    }
}
