use super::super::*;

impl ImagePreviewState {
    pub fn rotate_cw(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::rotate90(&self.current_rgba);
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("⟳ Rotated 90° Clockwise");
    }

    pub fn rotate_ccw(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::rotate270(&self.current_rgba);
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("⟲ Rotated 90° Counter-Clockwise");
    }

    pub fn rotate_180(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::rotate180(&self.current_rgba);
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.show_toast("🔄 Rotated 180°");
    }

    pub fn flip_horizontal(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::flip_horizontal(&self.current_rgba);
        self.dirty_texture = true;
        self.show_toast("⇆ Flipped Horizontally");
    }

    pub fn flip_vertical(&mut self) {
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = image::imageops::flip_vertical(&self.current_rgba);
        self.dirty_texture = true;
        self.show_toast("⇅ Flipped Vertically");
    }

    pub(crate) fn render_transform_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let btn_border = hex_to_c32(&palette.theme.b_low);
        let btn_fg = contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0, frame_stroke))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("🔄 Transforms:")
                            .strong()
                            .color(hex_to_c32(&palette.harmonies.base)),
                    );

                    ui.add_space(8.0);

                    // Rotate CCW
                    let ccw_btn = egui::Button::new(
                        egui::RichText::new("⟲ 90° CCW")
                            .small()
                            .strong()
                            .color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui
                        .add(ccw_btn)
                        .on_hover_text("Rotate 90 degrees counter-clockwise")
                        .clicked()
                    {
                        self.rotate_ccw();
                    }

                    // Rotate CW
                    let cw_btn = egui::Button::new(
                        egui::RichText::new("⟳ 90° CW")
                            .small()
                            .strong()
                            .color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui
                        .add(cw_btn)
                        .on_hover_text("Rotate 90 degrees clockwise")
                        .clicked()
                    {
                        self.rotate_cw();
                    }

                    // Rotate 180
                    let r180_btn = egui::Button::new(
                        egui::RichText::new("180°").small().strong().color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui
                        .add(r180_btn)
                        .on_hover_text("Rotate 180 degrees")
                        .clicked()
                    {
                        self.rotate_180();
                    }

                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Mirror:")
                            .small()
                            .color(hex_to_c32(&palette.theme.f_med)),
                    );

                    // Flip H
                    let fh_btn = egui::Button::new(
                        egui::RichText::new("⇆ Flip Horiz")
                            .small()
                            .strong()
                            .color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui
                        .add(fh_btn)
                        .on_hover_text("Flip horizontally (mirror left-right)")
                        .clicked()
                    {
                        self.flip_horizontal();
                    }

                    // Flip V
                    let fv_btn = egui::Button::new(
                        egui::RichText::new("⇅ Flip Vert")
                            .small()
                            .strong()
                            .color(btn_fg),
                    )
                    .fill(btn_bg)
                    .stroke(Stroke::new(1.0, btn_border))
                    .rounding(Rounding::same(4.0));
                    if ui
                        .add(fv_btn)
                        .on_hover_text("Flip vertically (mirror up-down)")
                        .clicked()
                    {
                        self.flip_vertical();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Revert all
                        let rev_fg =
                            contrast_text_c32(btn_bg, Some(hex_to_c32(&palette.diff_del)), palette);
                        let revert_btn = egui::Button::new(
                            egui::RichText::new("↺ Revert to Original")
                                .small()
                                .color(rev_fg),
                        )
                        .fill(btn_bg)
                        .stroke(Stroke::new(1.0, btn_border))
                        .rounding(Rounding::same(4.0));
                        if ui.add(revert_btn).clicked() {
                            self.revert_to_original();
                        }

                        // Undo
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(
                                btn_bg,
                                Some(hex_to_c32(&palette.diff_mod)),
                                palette,
                            );
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                    .small()
                                    .strong()
                                    .color(undo_fg),
                            )
                            .fill(btn_bg)
                            .stroke(Stroke::new(1.0, btn_border))
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }
                    });
                });
            });
    }
}
