use super::*;

impl ImagePreviewState {
    pub(super) fn render_dither_palette_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let base_c = hex_to_c32(&palette.harmonies.base);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
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
                let (b_bg, b_fg) = if sel {
                    (base_c, contrast_text_c32(base_c, None, palette))
                } else {
                    (b_med, f_med)
                };
                let btn = egui::Button::new(
                    egui::RichText::new(pal.label())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(b_fg),
                )
                .fill(b_bg)
                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui.add(btn).clicked() && !sel {
                    self.creative_fx_state.dither_params.palette = *pal;
                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                    self.creative_fx_state.mark_dirty();
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
                let (b_bg, b_fg) = if sel {
                    (base_c, contrast_text_c32(base_c, None, palette))
                } else {
                    (b_med, f_med)
                };
                let btn = egui::Button::new(
                    egui::RichText::new(method.label())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(b_fg),
                )
                .fill(b_bg)
                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui.add(btn).clicked() && !sel {
                    self.creative_fx_state.dither_params.method = *method;
                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                    self.creative_fx_state.mark_dirty();
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
                let (b_bg, b_fg) = if sel {
                    (base_c, contrast_text_c32(base_c, None, palette))
                } else {
                    (b_med, f_med)
                };
                let btn = egui::Button::new(
                    egui::RichText::new(format!("{}×", scale))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(b_fg),
                )
                .fill(b_bg)
                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .min_size(Vec2::new(32.0, 20.0));
                if ui.add(btn).clicked() && !sel {
                    self.creative_fx_state.dither_params.pixel_scale = scale;
                    self.creative_fx_state.selected_preset_name = "Custom".to_string();
                    self.creative_fx_state.mark_dirty();
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
            self.creative_fx_state.selected_preset_name = "Custom".to_string();
            self.creative_fx_state.mark_dirty();
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
            self.creative_fx_state.selected_preset_name = "Custom".to_string();
            self.creative_fx_state.mark_dirty();
        }
    }
}
