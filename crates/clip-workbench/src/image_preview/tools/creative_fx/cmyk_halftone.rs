use super::*;

impl ImagePreviewState {
    pub(super) fn render_cmyk_halftone_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        // Dot Size: 2..=16 px
        if render_context_slider(
            ui,
            (crate::icons::Icon::ShapeCircle, "Dot Screen Size"),
            &mut self.creative_fx_state.cmyk_params.dot_size,
            2.0..=16.0,
            5.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(3.0);

        // Dot Gain: 0.5..=2.0
        if render_context_slider(
            ui,
            (crate::icons::Icon::Chart, "Dot Gain (Ink Bleed)"),
            &mut self.creative_fx_state.cmyk_params.dot_gain,
            0.50..=2.00,
            1.10,
            "×",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Paper Tint
        ui.label(
            egui::RichText::new("Paper Substrate Tint")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for tint in PaperTint::all() {
                let sel = self.creative_fx_state.cmyk_params.paper_tint == *tint;
                if ui
                    .add(creative_fx_choice_button(tint.label(), sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.cmyk_params.paper_tint = *tint;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Plate Misregistration Offsets
        ui.add(
            crate::icons::Icon::Layers.label(
                egui::RichText::new("Plate Misregistration Offsets")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(base_c),
            ),
        );
        ui.add_space(3.0);

        // Cyan plate
        ui.label(
            egui::RichText::new("Cyan Plate Offset")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipHorizontal, "Cyan X"),
            &mut self.creative_fx_state.cmyk_params.cyan_offset[0],
            -10.0..=10.0,
            -1.5,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipVertical, "Cyan Y"),
            &mut self.creative_fx_state.cmyk_params.cyan_offset[1],
            -10.0..=10.0,
            0.5,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(3.0);

        // Magenta plate
        ui.label(
            egui::RichText::new("Magenta Plate Offset")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipHorizontal, "Magenta X"),
            &mut self.creative_fx_state.cmyk_params.magenta_offset[0],
            -10.0..=10.0,
            1.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipVertical, "Magenta Y"),
            &mut self.creative_fx_state.cmyk_params.magenta_offset[1],
            -10.0..=10.0,
            -1.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(3.0);

        // Yellow plate
        ui.label(
            egui::RichText::new("Yellow Plate Offset")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipHorizontal, "Yellow X"),
            &mut self.creative_fx_state.cmyk_params.yellow_offset[0],
            -10.0..=10.0,
            0.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipVertical, "Yellow Y"),
            &mut self.creative_fx_state.cmyk_params.yellow_offset[1],
            -10.0..=10.0,
            1.5,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(3.0);

        // Black (Key) plate
        ui.label(
            egui::RichText::new("Black (Key) Plate Offset")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipHorizontal, "Black X"),
            &mut self.creative_fx_state.cmyk_params.black_offset[0],
            -10.0..=10.0,
            0.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        if render_context_slider(
            ui,
            (crate::icons::Icon::FlipVertical, "Black Y"),
            &mut self.creative_fx_state.cmyk_params.black_offset[1],
            -10.0..=10.0,
            0.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
    }
}
