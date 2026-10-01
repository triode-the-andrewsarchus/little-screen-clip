use super::super::*;

impl ImagePreviewState {
    pub(crate) fn has_view_metadata(&self) -> bool {
        self.metadata.as_ref().is_some_and(|meta| {
            [
                &meta.make,
                &meta.model,
                &meta.lens_model,
                &meta.date_time,
                &meta.exposure_time,
                &meta.software,
            ]
            .iter()
            .any(|value| value.as_ref().is_some_and(|value| !value.trim().is_empty()))
                || meta.iso.is_some()
                || meta.f_number.is_some()
                || meta.focal_length.is_some()
                || meta.has_gps()
                || meta.gps_altitude.is_some()
        })
    }

    pub(crate) fn render_view_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        if !self.has_view_metadata() {
            return;
        }
        let Some(meta) = &self.metadata else {
            return;
        };
        let text = hex_to_c32(&palette.colors.text_secondary);
        let accent = hex_to_c32(&palette.harmonies.base);
        let btn_bg = hex_to_c32(&palette.colors.surface_panel);
        ui.label(egui::RichText::new("Metadata").strong().color(accent));
        for summary in [
            meta.camera_summary(),
            meta.exposure_summary(),
            meta.date_time.clone(),
            meta.gps_display(),
        ]
        .into_iter()
        .flatten()
        {
            ui.label(egui::RichText::new(summary).color(text));
        }
        if let Some(osm) = meta.osm_url() {
            let map =
                crate::ui_ext::secondary_button_icon(Some(Icon::Search), "Map", btn_bg, palette);
            if ui
                .add(map)
                .on_hover_text("Open GPS location in OpenStreetMap")
                .clicked()
            {
                ctx.open_url(egui::output::OpenUrl::new_tab(osm));
            }
        }
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.separator();
        for (label, value) in [
            ("Camera Make", meta.make.clone()),
            ("Camera Model", meta.model.clone()),
            ("Lens", meta.lens_model.clone()),
            ("Date / Time", meta.date_time.clone()),
            ("ISO", meta.iso.map(|value| value.to_string())),
            (
                "Aperture",
                meta.f_number.map(|value| format!("f/{value:.1}")),
            ),
            ("Exposure", meta.exposure_time.clone()),
            (
                "Focal Length",
                meta.focal_length.map(|value| format!("{value:.1} mm")),
            ),
            ("Software", meta.software.clone()),
            (
                "GPS Altitude",
                meta.gps_altitude.map(|value| format!("{value:.1} m")),
            ),
        ] {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                ui.label(egui::RichText::new(label).strong().color(text));
                ui.label(value);
                ui.add_space(crate::ui_ext::SPACE_XS);
            }
        }
        if let (Some(lat), Some(lon)) = (meta.gps_latitude, meta.gps_longitude) {
            ui.label(egui::RichText::new("GPS Coordinates").strong().color(text));
            ui.label(format!("{lat:.6}, {lon:.6}"));
            let copy =
                crate::ui_ext::secondary_button_icon(Some(Icon::Copy), "Copy", btn_bg, palette);
            if ui.add(copy).clicked() {
                ctx.output_mut(|output| output.copied_text = format!("{lat:.6}, {lon:.6}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_metadata_gps_copy_button_publishes_selected_coordinates() {
        let mut png_bytes = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            png_bytes,
            "image/png".into(),
            None,
            "Synthetic metadata fixture".into(),
            &AppConfig::default(),
        )
        .unwrap();
        state.metadata = Some(little_clip_ops::metadata::ImageMetadataSummary {
            make: Some("Synthetic Camera Co.".to_owned()),
            model: Some("Fixture 1".to_owned()),
            gps_latitude: Some(47.25),
            gps_longitude: Some(-122.5),
            ..Default::default()
        });

        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 800.0));
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_view_toolbar(ui, ctx, &palette);
                });
            },
        );
        let initial = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_view_toolbar(ui, ctx, &palette);
                });
            },
        );
        let copy_position = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Copy" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                let labels = initial
                    .shapes
                    .iter()
                    .filter_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                panic!("rendered GPS coordinate Copy action missing; labels={labels:?}")
            });
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_view_toolbar(ui, ctx, &palette);
                    });
                },
            )
        };
        render(vec![egui::Event::PointerMoved(copy_position)]);
        render(vec![
            egui::Event::PointerMoved(copy_position),
            egui::Event::PointerButton {
                pos: copy_position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let copied = render(vec![
            egui::Event::PointerMoved(copy_position),
            egui::Event::PointerButton {
                pos: copy_position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);

        assert_eq!(copied.platform_output.copied_text, "47.250000, -122.500000");
    }
}
