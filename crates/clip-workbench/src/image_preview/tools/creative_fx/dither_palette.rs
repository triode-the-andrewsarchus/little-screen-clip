use super::*;

impl ImagePreviewState {
    pub(super) fn render_dither_palette_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        // Palette selection
        ui.label(
            egui::RichText::new("Hardware / Aesthetic Palette")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for pal in RetroPalette::all() {
                let sel = self.creative_fx_state.dither_params.palette == *pal;
                if ui
                    .add(creative_fx_choice_button(pal.label(), sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.dither_params.palette = *pal;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

        // Dither Method selection
        ui.label(
            egui::RichText::new("Dithering Method")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for method in DitherMethod::all() {
                let sel = self.creative_fx_state.dither_params.method == *method;
                if ui
                    .add(creative_fx_choice_button(method.label(), sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.dither_params.method = *method;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

        // Pixel Scale selection (1x..4x)
        ui.label(
            egui::RichText::new("Pixel Grid Scale")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for scale in [1u32, 2, 3, 4] {
                let sel = self.creative_fx_state.dither_params.pixel_scale == scale;
                if ui
                    .add(creative_fx_choice_button(
                        format!("{}×", scale),
                        sel,
                        palette,
                    ))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.dither_params.pixel_scale = scale;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Dither Strength
        if render_context_slider(
            ui,
            "⚡ Dither Strength",
            &mut self.creative_fx_state.dither_params.dither_strength,
            0.0..=1.0,
            0.80,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(3.0);

        // Pre-dither Contrast
        if render_context_slider(
            ui,
            "🌓 Contrast Boost",
            &mut self.creative_fx_state.dither_params.contrast,
            -50.0..=50.0,
            0.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
    }
}
