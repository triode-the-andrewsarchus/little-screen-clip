use super::super::*;

impl ImagePreviewState {
    pub fn rotate_cw(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(image::imageops::rotate90(self.current_rgba.as_ref()));
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("⟳ Rotated 90° Clockwise");
    }

    pub fn rotate_ccw(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(image::imageops::rotate270(self.current_rgba.as_ref()));
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("⟲ Rotated 90° Counter-Clockwise");
    }

    pub fn rotate_180(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(image::imageops::rotate180(self.current_rgba.as_ref()));
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("🔄 Rotated 180°");
    }

    pub fn flip_horizontal(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(image::imageops::flip_horizontal(self.current_rgba.as_ref()));
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.show_toast("⇆ Flipped Horizontally");
    }

    pub fn flip_vertical(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(image::imageops::flip_vertical(self.current_rgba.as_ref()));
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.show_toast("⇅ Flipped Vertically");
    }

    pub(crate) fn render_transform_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.colors.surface_recessed);
        let frame_stroke = hex_to_c32(&palette.colors.surface_panel);
        let btn_bg = hex_to_c32(&palette.colors.surface_panel);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Transforms:")
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Rotate CCW
                    let ccw_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCcw),
                        "90° CCW",
                        btn_bg,
                        palette,
                    );
                    if ui
                        .add(ccw_btn)
                        .on_hover_text("Rotate 90 degrees counter-clockwise")
                        .clicked()
                    {
                        self.rotate_ccw();
                    }

                    // Rotate CW
                    let cw_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCw),
                        "90° CW",
                        btn_bg,
                        palette,
                    );
                    if ui
                        .add(cw_btn)
                        .on_hover_text("Rotate 90 degrees clockwise")
                        .clicked()
                    {
                        self.rotate_cw();
                    }

                    // Rotate 180
                    let r180_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Repeat),
                        "180°",
                        btn_bg,
                        palette,
                    );
                    if ui
                        .add(r180_btn)
                        .on_hover_text("Rotate 180 degrees")
                        .clicked()
                    {
                        self.rotate_180();
                    }

                    ui.add_space(crate::ui_ext::SPACE_MD);
                    ui.label(
                        egui::RichText::new("Mirror:")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                    );

                    // Flip H
                    let fh_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::FlipHorizontal),
                        "Flip Horiz",
                        btn_bg,
                        palette,
                    );
                    if ui
                        .add(fh_btn)
                        .on_hover_text("Flip horizontally (mirror left-right)")
                        .clicked()
                    {
                        self.flip_horizontal();
                    }

                    // Flip V
                    let fv_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::FlipVertical),
                        "Flip Vert",
                        btn_bg,
                        palette,
                    );
                    if ui
                        .add(fv_btn)
                        .on_hover_text("Flip vertically (mirror up-down)")
                        .clicked()
                    {
                        self.flip_vertical();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Revert all
                        let revert_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::RotateCcw),
                            "Revert to Original",
                            btn_bg,
                            palette,
                        );
                        if ui.add(revert_btn).clicked() {
                            self.revert_to_original();
                        }

                        // Undo
                        if !self.undo_stack.is_empty() {
                            let undo_btn =
                                crate::ui_ext::secondary_button("⟲ Undo (Ctrl+Z)", btn_bg, palette);
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }
}
