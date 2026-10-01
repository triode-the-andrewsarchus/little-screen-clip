use super::*;

impl DiffViewerState {
    pub(super) fn render_text_viewport(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        if self.is_computing {
            self.render_computing_ui(ui, palette);
            return;
        }

        if let Some(error) = &self.decode_error {
            ui.label(
                egui::RichText::new(format!("Diff unavailable: {error}"))
                    .color(crate::hex_to_c32(&palette.colors.text_primary))
                    .size(BODY_FONT_SIZE),
            );
        }

        let b_low = crate::hex_to_c32(&palette.colors.console_surface);
        let b_med = crate::hex_to_c32(&palette.chrome_border);

        egui::Frame::none()
            .fill(b_low)
            .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
            .stroke(Stroke::new(1.0_f32, b_med))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(self.layout_job.clone())
                                .wrap(false)
                                .sense(egui::Sense::click()),
                        );
                    });
            });
    }

    pub(super) fn render_text_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::SPACE_MD,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
            .show(ui, |ui| {
                ui.set_min_height(CONTROL_HEIGHT);
                ui.horizontal(|ui| {
                    let is_side = self.mode == DiffDisplayMode::SideBySide;
                    let (mode_label, mode_icon) = if is_side {
                        ("Side-by-Side", Icon::Layers)
                    } else {
                        ("Unified", Icon::Text)
                    };
                    let btn = crate::ui_ext::mode_tab_button_icon(
                        Some(mode_icon),
                        mode_label,
                        true,
                        palette,
                    );
                    if ui
                        .add(btn)
                        .on_hover_text("Toggle text diff display mode (Tab / C)")
                        .clicked()
                    {
                        self.toggle_mode();
                    }
                });
            });
    }
}
