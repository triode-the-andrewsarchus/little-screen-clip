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
            Self::FilmSim => Some(crate::shell::StudioPanelLayout {
                bottom_height: 160.0,
                ..base.with_left().with_right().with_bottom()
            }),
            Self::View
            | Self::Crop
            | Self::Deskew
            | Self::Transform
            | Self::Redact
            | Self::Barcode
            | Self::Palette
            | Self::DepthMap
            | Self::Resize
            | Self::Convert
            | Self::RemoveBg => Some(base.with_left()),
        }
    }
}

impl ImagePreviewState {
    pub(crate) fn render_active_tool_workspace(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        match self.active_mode {
            ImageToolMode::FilmSim => self.render_film_sim_viewport(ui, ctx, palette),
            ImageToolMode::CreativeFx => self.render_creative_fx_viewport(ui, ctx, palette),
            _ => self.render_tool_workspace(ui, ctx, palette),
        }
    }

    pub(crate) fn render_tool_workspace(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let mut layout = self
            .active_mode
            .panel_layout()
            .expect("panel workspace requested for a non-panel image tool")
            .with_panel_sizes(&self.panel_state);
        if self.active_mode == ImageToolMode::View && !self.has_view_metadata() {
            layout.left = false;
        }
        let panels = layout.allocate(ui);

        if let Some(rect) = panels.left {
            crate::shell::StudioPanelLayout::show_side(
                ui,
                rect,
                ("image_tool_left", self.active_mode as u8),
                palette,
                |ui| match self.active_mode {
                    ImageToolMode::View => self.render_view_toolbar(ui, ctx, palette),
                    ImageToolMode::Crop => self.render_crop_toolbar(ui, palette),
                    ImageToolMode::Deskew => self.render_deskew_toolbar(ui, ctx, palette),
                    ImageToolMode::Transform => self.render_transform_toolbar(ui, palette),
                    ImageToolMode::Redact => self.render_redact_toolbar(ui, palette),
                    ImageToolMode::Barcode => self.render_barcode_toolbar(ui, ctx, palette),
                    ImageToolMode::DepthMap => self.render_depth_map_toolbar(ui, ctx, palette),
                    ImageToolMode::Palette => self.render_palette_panel(ui, ctx, palette),
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

        crate::shell::StudioPanelLayout::show_region(ui, panels.center, |ui| {
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

        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            ("image_tool", self.active_mode as u8),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tool_panel_profiles_match_reference_studios() {
        for mode in [
            ImageToolMode::View,
            ImageToolMode::Crop,
            ImageToolMode::Deskew,
            ImageToolMode::Transform,
            ImageToolMode::Redact,
            ImageToolMode::Barcode,
            ImageToolMode::DepthMap,
            ImageToolMode::Palette,
        ] {
            let layout = mode.panel_layout().unwrap();
            assert!(layout.left && !layout.right && !layout.bottom, "{mode:?}");
        }
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

    fn sidebar_fixture() -> ImagePreviewState {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(16, 16, image::Rgba([24, 80, 180, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".into(),
            None,
            "Sidebar fixture".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn left_tool_content_keeps_padding_at_standard_and_minimum_panel_widths() {
        let palette = AppPalette::default();
        for width in [
            crate::shell::PANEL_MIN_WIDTH,
            crate::shell::PANEL_LEFT_DEFAULT_WIDTH,
        ] {
            for mode in [
                ImageToolMode::View,
                ImageToolMode::Crop,
                ImageToolMode::Deskew,
                ImageToolMode::Transform,
                ImageToolMode::Redact,
                ImageToolMode::Barcode,
                ImageToolMode::DepthMap,
                ImageToolMode::Convert,
            ] {
                let mut state = sidebar_fixture();
                state.active_mode = mode;
                state.panel_state.left_width = width;
                if mode == ImageToolMode::View {
                    state.metadata = Some(little_clip_ops::metadata::ImageMetadataSummary {
                        make: Some("A camera manufacturer with a long name".into()),
                        model: Some("Camera model".into()),
                        gps_latitude: Some(47.25),
                        gps_longitude: Some(-122.5),
                        ..Default::default()
                    });
                }
                if mode == ImageToolMode::Barcode {
                    state.barcode_state.has_scanned = true;
                    state.barcode_state.detections = (0..3)
                        .map(|_| BarcodeDetection {
                            format: "QR Code".into(),
                            text: "https://example.com/a/long/payload/to/inspect".into(),
                            points: vec![],
                            bbox: [0.0; 4],
                            is_url: true,
                        })
                        .collect();
                }

                let ctx = egui::Context::default();
                crate::ui_ext::configure_workbench_style(&ctx);
                let input = || egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 900.0))),
                    ..Default::default()
                };
                let render = |state: &mut ImagePreviewState| {
                    ctx.run(input(), |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_tool_workspace(ui, ctx, &palette);
                        });
                    })
                };
                render(&mut state);
                let output = render(&mut state);
                let mut labels = 0;
                for clipped in &output.shapes {
                    if let egui::Shape::Text(text) = &clipped.shape {
                        // Sidebar content has its own clip; the center canvas may also paint text.
                        if clipped.clip_rect.right() < 400.0 {
                            let bounds = text.galley.rect.translate(text.pos.to_vec2());
                            assert!(
                                bounds.right()
                                    <= clipped.clip_rect.right() - crate::ui_ext::SPACE_SM + 0.5,
                                "{mode:?}, width {width}: {} overflows {:?} with {:?}",
                                text.galley.text(),
                                clipped.clip_rect,
                                bounds
                            );
                            labels += 1;
                        }
                    }
                }
                assert!(
                    labels > 0,
                    "{mode:?} must render controls in the left panel"
                );
            }
        }
    }

    #[test]
    fn view_without_metadata_keeps_the_entire_canvas_width() {
        let mut state = sidebar_fixture();
        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        for metadata in [
            None,
            Some(little_clip_ops::metadata::ImageMetadataSummary::default()),
            Some(little_clip_ops::metadata::ImageMetadataSummary {
                has_exif: true,
                ..Default::default()
            }),
        ] {
            state.metadata = metadata;
            assert!(!state.has_view_metadata());
            state.auto_fitted = false;
            let mut expected_width = 0.0;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        expected_width = ui.available_width();
                        state.render_tool_workspace(ui, ctx, &palette);
                    });
                },
            );
            assert!(
                output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                egui::Shape::Rect(rect) if rect.rect.width() >= expected_width - 0.5)),
                "metadata-free View must not reserve an empty left panel"
            );
            assert!(!output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                egui::Shape::Text(text) if text.galley.text().contains("Metadata") || text.galley.text().contains("Inspector"))));
        }
    }
}
