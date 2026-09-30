use super::super::*;

impl ImagePreviewState {
    pub(crate) fn render_view_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let frame_bg = hex_to_c32(&palette.colors.surface_recessed);
        let frame_stroke = hex_to_c32(&palette.colors.surface_panel);
        let btn_bg = hex_to_c32(&palette.colors.surface_panel);
        let _btn_border = hex_to_c32(&palette.colors.surface_recessed);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, frame_stroke))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Inspector:")
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(hex_to_c32(&palette.harmonies.base)),
                        );

                        if let Some(meta) = &self.metadata {
                            if let Some(cam) = meta.camera_summary() {
                                crate::ui_ext::badge_pill(
                                    ui,
                                    &cam,
                                    Some(hex_to_c32(&palette.harmonies.split_comp_1)),
                                    palette,
                                );
                            }
                            if let Some(exp) = meta.exposure_summary() {
                                crate::ui_ext::badge_pill(
                                    ui,
                                    &exp,
                                    Some(hex_to_c32(&palette.diff_mod)),
                                    palette,
                                );
                            }
                            if let Some(dt) = &meta.date_time {
                                crate::ui_ext::badge_pill(
                                    ui,
                                    dt,
                                    Some(hex_to_c32(&palette.colors.text_secondary)),
                                    palette,
                                );
                            }
                            if let Some(gps) = meta.gps_display() {
                                crate::ui_ext::badge_pill(
                                    ui,
                                    &gps,
                                    Some(hex_to_c32(&palette.diff_add)),
                                    palette,
                                );
                                if let Some(osm) = meta.osm_url() {
                                    let map_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Search),
                                        "Map",
                                        btn_bg,
                                        palette,
                                    );
                                    if ui
                                        .add(map_btn)
                                        .on_hover_text("Open GPS location in OpenStreetMap")
                                        .clicked()
                                    {
                                        ctx.open_url(egui::output::OpenUrl::new_tab(osm));
                                    }
                                }
                            }

                            // Drawer toggle button
                            let (toggle_txt, toggle_icon) = if self.show_metadata_drawer {
                                ("Hide Details", Icon::ArrowUp)
                            } else {
                                ("EXIF Details", Icon::ArrowDown)
                            };
                            let toggle_btn = crate::ui_ext::secondary_button_icon(
                                Some(toggle_icon),
                                toggle_txt,
                                btn_bg,
                                palette,
                            );
                            if ui.add(toggle_btn).clicked() {
                                self.show_metadata_drawer = !self.show_metadata_drawer;
                            }
                        } else {
                            ui.label(
                                egui::RichText::new(
                                    "Clean Image (No EXIF or metadata tags detected)",
                                )
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(hex_to_c32(&palette.colors.text_secondary)),
                            );
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let dim_str = format!(
                                "{} × {} px",
                                self.current_rgba.width(),
                                self.current_rgba.height()
                            );
                            ui.label(
                                egui::RichText::new(dim_str)
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(hex_to_c32(&palette.colors.text_muted)),
                            );
                        });
                    });

                    // Expanded Metadata Drawer
                    if self.show_metadata_drawer {
                        if let Some(meta) = &self.metadata {
                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .show(ui, |ui| {
                                    egui::Grid::new("exif_metadata_grid")
                                        .striped(true)
                                        .spacing(Vec2::new(16.0, 4.0))
                                        .show(ui, |ui| {
                                            if let Some(m) = &meta.make {
                                                ui.label(
                                                    egui::RichText::new("Camera Make:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(m)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(m) = &meta.model {
                                                ui.label(
                                                    egui::RichText::new("Camera Model:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(m)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(l) = &meta.lens_model {
                                                ui.label(
                                                    egui::RichText::new("Lens:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(l)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(s) = &meta.software {
                                                ui.label(
                                                    egui::RichText::new("Software:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(s)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(dt) = &meta.date_time {
                                                ui.label(
                                                    egui::RichText::new("Timestamp:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(dt)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(iso) = meta.iso {
                                                ui.label(
                                                    egui::RichText::new("ISO Speed:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(iso.to_string())
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(f) = meta.f_number {
                                                ui.label(
                                                    egui::RichText::new("Aperture:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("f/{:.1}", f))
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(exp) = &meta.exposure_time {
                                                ui.label(
                                                    egui::RichText::new("Shutter Speed:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(exp)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(fl) = meta.focal_length {
                                                ui.label(
                                                    egui::RichText::new("Focal Length:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("{:.1} mm", fl))
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                            if let (Some(lat), Some(lon)) =
                                                (meta.gps_latitude, meta.gps_longitude)
                                            {
                                                ui.label(
                                                    egui::RichText::new("GPS Coordinates:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{:.6}, {:.6}",
                                                            lat, lon
                                                        ))
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                    );
                                                    let copy_btn =
                                                        crate::ui_ext::secondary_button_icon(
                                                            Some(Icon::Copy),
                                                            "Copy",
                                                            btn_bg,
                                                            palette,
                                                        );
                                                    if ui.add(copy_btn).clicked() {
                                                        ctx.output_mut(|o| {
                                                            o.copied_text =
                                                                format!("{:.6}, {:.6}", lat, lon)
                                                        });
                                                    }
                                                });
                                                ui.end_row();
                                            }
                                            if let Some(alt) = meta.gps_altitude {
                                                ui.label(
                                                    egui::RichText::new("GPS Altitude:")
                                                        .strong()
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_secondary,
                                                        )),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("{:.1} m", alt))
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                                ui.end_row();
                                            }
                                        });
                                });
                        }
                    }
                });
            });
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
        state.show_metadata_drawer = true;

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
