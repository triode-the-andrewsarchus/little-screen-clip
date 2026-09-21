use super::super::*;

impl ImageToolMode {
    /// Canonical panel contract for every tool that owns a studio workspace.
    pub(crate) fn panel_layout(self) -> Option<crate::shell::StudioPanelLayout> {
        let base = crate::shell::StudioPanelLayout::default();
        match self {
            Self::PickColor
            | Self::Ocr
            | Self::DecorrelationStretch
            | Self::CreativeFx
            | Self::Markup => Some(base.with_left().with_right()),
            Self::FilmSim => Some(base.with_left().with_right().with_bottom()),
            Self::Resize | Self::Convert | Self::RemoveBg => Some(base.with_left()),
            _ => None,
        }
    }
}

impl ImagePreviewState {
    /// Render controls for tools that intentionally use the compact secondary
    /// strip. Panel-based tools render their controls inside the workspace.
    pub(crate) fn render_active_tool_controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        match self.active_mode {
            ImageToolMode::View => self.render_view_toolbar(ui, ctx, palette),
            ImageToolMode::Crop => self.render_crop_toolbar(ui, palette),
            ImageToolMode::Deskew => self.render_deskew_toolbar(ui, ctx, palette),
            ImageToolMode::Transform => self.render_transform_toolbar(ui, palette),
            ImageToolMode::Redact => self.render_redact_toolbar(ui, palette),
            ImageToolMode::Barcode => self.render_barcode_toolbar(ui, ctx, palette),
            ImageToolMode::Palette => self.render_palette_panel(ui, ctx, palette),
            ImageToolMode::DepthMap => self.render_depth_map_toolbar(ui, ctx, palette),
            ImageToolMode::PickColor
            | ImageToolMode::Ocr
            | ImageToolMode::Resize
            | ImageToolMode::Convert
            | ImageToolMode::RemoveBg
            | ImageToolMode::DecorrelationStretch
            | ImageToolMode::FilmSim
            | ImageToolMode::CreativeFx
            | ImageToolMode::Markup => return,
        }
        ui.add_space(2.0);
    }

    pub(crate) fn render_active_tool_workspace(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        match self.active_mode {
            ImageToolMode::FilmSim => self.render_film_sim_viewport(ui, ctx, palette),
            ImageToolMode::CreativeFx => self.render_creative_fx_viewport(ui, ctx, palette),
            ImageToolMode::PickColor
            | ImageToolMode::Ocr
            | ImageToolMode::Resize
            | ImageToolMode::Convert
            | ImageToolMode::RemoveBg
            | ImageToolMode::DecorrelationStretch
            | ImageToolMode::Markup => self.render_tool_workspace(ui, ctx, palette),
            _ => self.render_viewport(ui, ctx, palette),
        }
    }

    pub(crate) fn render_tool_workspace(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let layout = self
            .active_mode
            .panel_layout()
            .expect("panel workspace requested for a non-panel image tool");
        let panels = layout.allocate(ui);

        if let Some(rect) = panels.left {
            crate::shell::StudioPanelLayout::show_side(
                ui,
                rect,
                ("image_tool_left", self.active_mode as u8),
                palette,
                |ui| match self.active_mode {
                    ImageToolMode::PickColor => self.render_palette_panel(ui, ctx, palette),
                    ImageToolMode::Ocr => self.render_ocr_toolbar(ui, ctx, palette),
                    ImageToolMode::Resize => self.render_resize_toolbar(ui, ctx, palette),
                    ImageToolMode::Convert => self.render_convert_toolbar(ui, ctx, palette),
                    ImageToolMode::RemoveBg => self.render_remove_bg_toolbar(ui, ctx, palette),
                    ImageToolMode::DecorrelationStretch => {
                        self.render_decorrelation_stretch_toolbar(ui, ctx, palette)
                    }
                    ImageToolMode::Markup => self.render_markup_left_panel(ui, palette),
                    _ => {}
                },
            );
        }

        crate::shell::StudioPanelLayout::show_center(ui, panels.center, |ui| {
            self.render_viewport(ui, ctx, palette);
        });

        if let Some(rect) = panels.right {
            crate::shell::StudioPanelLayout::show_side(
                ui,
                rect,
                ("image_tool_right", self.active_mode as u8),
                palette,
                |ui| match self.active_mode {
                    ImageToolMode::PickColor => self.render_pick_color_results(ui, ctx, palette),
                    ImageToolMode::Ocr => self.render_ocr_results(ui, ctx, palette),
                    ImageToolMode::DecorrelationStretch => {
                        self.render_decorrelation_stretch_stats_drawer(ui, palette)
                    }
                    ImageToolMode::Markup => self.render_markup_right_panel(ui, palette),
                    _ => {}
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tool_panel_profiles_match_reference_studios() {
        let film = ImageToolMode::FilmSim.panel_layout().unwrap();
        let creative = ImageToolMode::CreativeFx.panel_layout().unwrap();
        assert!(film.left && film.right && film.bottom);
        assert!(creative.left && creative.right && !creative.bottom);

        let markup = ImageToolMode::Markup.panel_layout().unwrap();
        assert!(markup.left && markup.right && !markup.bottom);

        let pick = ImageToolMode::PickColor.panel_layout().unwrap();
        let resize = ImageToolMode::Resize.panel_layout().unwrap();
        let convert = ImageToolMode::Convert.panel_layout().unwrap();
        let ocr = ImageToolMode::Ocr.panel_layout().unwrap();
        assert!(pick.left && pick.right);
        assert!(ocr.left && ocr.right);
        assert!(resize.left && !resize.right);
        assert!(convert.left && !convert.right);
        assert_eq!(pick.left_width, creative.left_width);
        assert_eq!(pick.right_width, creative.right_width);
        assert_eq!(ocr.left_width, creative.left_width);
        assert_eq!(ocr.right_width, creative.right_width);
        assert_eq!(markup.left_width, creative.left_width);
        assert_eq!(markup.right_width, creative.right_width);
    }
}
