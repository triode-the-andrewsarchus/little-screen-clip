use super::*;

impl ImagePreviewState {
    pub(super) fn render_pixel_sort_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        // Direction
        ui.label(
            egui::RichText::new("Scanline Direction")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            let dirs = [
                (PixelSortDirection::Vertical, "Vertical"),
                (PixelSortDirection::Horizontal, "Horizontal"),
                (PixelSortDirection::Diagonal, "Diagonal"),
            ];
            for (d, label) in dirs {
                let sel = self.creative_fx_state.pixel_sort_params.direction == d;
                if ui
                    .add(creative_fx_choice_button(label, sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.pixel_sort_params.direction = d;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Criteria
        ui.label(
            egui::RichText::new("Sort Metric")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            let crit = [
                (PixelSortCriteria::Brightness, "Brightness"),
                (PixelSortCriteria::Hue, "Hue"),
                (PixelSortCriteria::Saturation, "Saturation"),
            ];
            for (c, label) in crit {
                let sel = self.creative_fx_state.pixel_sort_params.criteria == c;
                if ui
                    .add(creative_fx_choice_button(label, sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.pixel_sort_params.criteria = c;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Order
        ui.label(
            egui::RichText::new("Sort Order")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            let orders = [
                (PixelSortOrder::Ascending, "Ascending"),
                (PixelSortOrder::Descending, "Descending"),
            ];
            for (o, label) in orders {
                let sel = self.creative_fx_state.pixel_sort_params.order == o;
                if ui
                    .add(creative_fx_choice_button(label, sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.pixel_sort_params.order = o;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);

        // Min Threshold
        if render_context_slider(
            ui,
            "Min Threshold",
            &mut self.creative_fx_state.pixel_sort_params.min_threshold,
            0.0..=1.0,
            0.25,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        // Max Threshold
        if render_context_slider(
            ui,
            "Max Threshold",
            &mut self.creative_fx_state.pixel_sort_params.max_threshold,
            0.0..=1.0,
            0.80,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        // Min Span
        let mut span_f = self.creative_fx_state.pixel_sort_params.min_span as f32;
        if render_context_slider(
            ui,
            "Min Span Length",
            &mut span_f,
            2.0..=50.0,
            6.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.pixel_sort_params.min_span = span_f.round() as usize;
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        // Invert mask
        let mut invert = self.creative_fx_state.pixel_sort_params.invert_mask;
        if ui
            .checkbox(
                &mut invert,
                egui::RichText::new("Invert Selection Mask")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            )
            .changed()
        {
            self.creative_fx_state.pixel_sort_params.invert_mask = invert;
            self.creative_fx_state.mark_custom();
        }
    }
}
