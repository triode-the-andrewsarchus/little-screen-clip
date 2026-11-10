use super::*;

impl ImagePreviewState {
    pub(super) fn render_trichromatic_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        // Mode: RGB vs OKLab
        ui.label(
            egui::RichText::new("Color Separation Model")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            let modes = [
                (TrichromaticMode::Rgb, "RGB"),
                (TrichromaticMode::OkLab, "OKLab"),
            ];
            for (mode, label) in modes {
                let sel = self.creative_fx_state.trichromatic_params.mode == mode;
                if ui
                    .add(creative_fx_choice_button(label, sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.trichromatic_params.mode = mode;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

        let names = self
            .creative_fx_state
            .trichromatic_params
            .mode
            .channel_names();
        for i in 0..3 {
            ui.label(
                egui::RichText::new(format!("Channel {}: {}", i + 1, names[i]))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(base_c),
            );
            ui.add_space(2.0);

            // Offset X
            if render_context_slider(
                ui,
                (
                    crate::icons::Icon::FlipHorizontal,
                    format!("X Offset (Ch {})", i + 1),
                ),
                &mut self.creative_fx_state.trichromatic_params.channels[i].offset_x,
                -50.0..=50.0,
                0.0,
                "px",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(2.0);

            // Offset Y
            if render_context_slider(
                ui,
                (
                    crate::icons::Icon::FlipVertical,
                    format!("Y Offset (Ch {})", i + 1),
                ),
                &mut self.creative_fx_state.trichromatic_params.channels[i].offset_y,
                -50.0..=50.0,
                0.0,
                "px",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(2.0);

            // Zoom
            if render_context_slider(
                ui,
                (crate::icons::Icon::Search, format!("Zoom (Ch {})", i + 1)),
                &mut self.creative_fx_state.trichromatic_params.channels[i].zoom,
                0.80..=1.20,
                1.00,
                "×",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(crate::ui_ext::SPACE_XS);
        }

        // Overlap crop window calculation
        let (_, _, cw, ch) = trichromatic_intersection_box(
            self.current_rgba.width(),
            self.current_rgba.height(),
            &self.creative_fx_state.trichromatic_params.channels,
        );
        ui.add_space(2.0);
        ui.add(
            crate::icons::Icon::Scissors.label(
                egui::RichText::new(format!("Crop Window: {} × {} px", cw, ch))
                    .monospace()
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            ),
        );
        ui.label(
            egui::RichText::new("(auto-cropped to 3-channel overlap)")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_low),
        );
    }
}
