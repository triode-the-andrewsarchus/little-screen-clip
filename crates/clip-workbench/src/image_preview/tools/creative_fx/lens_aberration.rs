use super::*;

impl ImagePreviewState {
    pub(super) fn render_lens_aberration_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        // Strength: 0..100
        if render_context_slider(
            ui,
            "⚡ Strength",
            &mut self.creative_fx_state.lens_params.strength,
            0.0..=100.0,
            25.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.selected_preset_name = "Custom".to_string();
            self.creative_fx_state.mark_dirty();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Falloff: 0..100 (0: entire image, 100: edges only)
        if render_context_slider(
            ui,
            "🔘 Falloff (0: All, 100: Edges)",
            &mut self.creative_fx_state.lens_params.falloff,
            0.0..=100.0,
            50.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.selected_preset_name = "Custom".to_string();
            self.creative_fx_state.mark_dirty();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Color Balance: -50..+50 (-50: red/blue, +50: blue/yellow)
        if render_context_slider(
            ui,
            "🎨 Color Balance (-50: Red/Blue, +50: Blue/Yellow)",
            &mut self.creative_fx_state.lens_params.color_balance,
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
