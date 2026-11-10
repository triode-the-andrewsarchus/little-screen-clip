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
        self.show_toast(crate::ui_ext::StatusMessage::new(
            crate::icons::Icon::RotateCw,
            "Rotated 180°",
        ));
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
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("Transforms:")
                    .strong()
                    .color(hex_to_c32(&palette.harmonies.base)),
            );

            ui.add_space(crate::ui_ext::SPACE_SM);

            // Rotate CCW
            let ccw_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                Some(Icon::RotateCcw),
                "90° CCW",
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
            let cw_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                Some(Icon::RotateCw),
                "90° CW",
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
            let r180_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                Some(Icon::Repeat),
                "180°",
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
            let fh_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                Some(Icon::FlipHorizontal),
                "Flip Horiz",
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
            let fv_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                Some(Icon::FlipVertical),
                "Flip Vert",
                palette,
            );
            if ui
                .add(fv_btn)
                .on_hover_text("Flip vertically (mirror up-down)")
                .clicked()
            {
                self.flip_vertical();
            }

            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                // Revert all
                let revert_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                    Some(Icon::RotateCcw),
                    "Revert to Original",
                    palette,
                );
                if ui.add(revert_btn).clicked() {
                    self.revert_to_original();
                }

                // Undo
                if !self.undo_stack.is_empty() {
                    let undo_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                        Some(crate::icons::Icon::Undo),
                        "Undo (Ctrl+Z)",
                        palette,
                    );
                    if ui.add(undo_btn).clicked() {
                        self.undo();
                    }
                }
            });
        });
    }
}
