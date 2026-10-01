use super::*;

impl ImagePreviewState {
    pub(super) fn render_structure_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        // Structure Amount: -100..+100
        if render_context_slider(
            ui,
            "⚡ Structure Amount (-100: Soften, +100: Enhance)",
            &mut self.creative_fx_state.structure_params.amount,
            -100.0..=100.0,
            35.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Detail Scale: -50..+50
        if render_context_slider(
            ui,
            "🔍 Detail Scale (-50: Fine, +50: Coarse)",
            &mut self.creative_fx_state.structure_params.detail_scale,
            -50.0..=50.0,
            0.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Edge Protection: 0..100
        if render_context_slider(
            ui,
            "🛡 Edge Protection (0: Off, 100: Strict)",
            &mut self.creative_fx_state.structure_params.edge_protection,
            0.0..=100.0,
            50.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
    }
}
