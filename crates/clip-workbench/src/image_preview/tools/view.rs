use super::super::*;

impl ImagePreviewState {
    pub(crate) fn render_view_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let frame_bg = hex_to_c32(&palette.theme.b_low);
        let frame_stroke = hex_to_c32(&palette.theme.b_med);
        let btn_bg = hex_to_c32(&palette.theme.b_med);
        let _btn_border = hex_to_c32(&palette.theme.b_low);

        egui::Frame::none()
            .fill(frame_bg)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0_f32, frame_stroke))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Inspector:")
                                .strong()
                                .small()
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
                                    Some(hex_to_c32(&palette.theme.f_med)),
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
                                .small()
                                .color(hex_to_c32(&palette.theme.f_med)),
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
                                    .small()
                                    .color(hex_to_c32(&palette.theme.f_low)),
                            );
                        });
                    });

                    // Expanded Metadata Drawer
                    if self.show_metadata_drawer {
                        if let Some(meta) = &self.metadata {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(4.0);

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
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(egui::RichText::new(m).small());
                                                ui.end_row();
                                            }
                                            if let Some(m) = &meta.model {
                                                ui.label(
                                                    egui::RichText::new("Camera Model:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(egui::RichText::new(m).small());
                                                ui.end_row();
                                            }
                                            if let Some(l) = &meta.lens_model {
                                                ui.label(
                                                    egui::RichText::new("Lens:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(egui::RichText::new(l).small());
                                                ui.end_row();
                                            }
                                            if let Some(s) = &meta.software {
                                                ui.label(
                                                    egui::RichText::new("Software:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(egui::RichText::new(s).small());
                                                ui.end_row();
                                            }
                                            if let Some(dt) = &meta.date_time {
                                                ui.label(
                                                    egui::RichText::new("Timestamp:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(egui::RichText::new(dt).small());
                                                ui.end_row();
                                            }
                                            if let Some(iso) = meta.iso {
                                                ui.label(
                                                    egui::RichText::new("ISO Speed:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(iso.to_string()).small(),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(f) = meta.f_number {
                                                ui.label(
                                                    egui::RichText::new("Aperture:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("f/{:.1}", f))
                                                        .small(),
                                                );
                                                ui.end_row();
                                            }
                                            if let Some(exp) = &meta.exposure_time {
                                                ui.label(
                                                    egui::RichText::new("Shutter Speed:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(egui::RichText::new(exp).small());
                                                ui.end_row();
                                            }
                                            if let Some(fl) = meta.focal_length {
                                                ui.label(
                                                    egui::RichText::new("Focal Length:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("{:.1} mm", fl))
                                                        .small(),
                                                );
                                                ui.end_row();
                                            }
                                            if let (Some(lat), Some(lon)) =
                                                (meta.gps_latitude, meta.gps_longitude)
                                            {
                                                ui.label(
                                                    egui::RichText::new("GPS Coordinates:")
                                                        .strong()
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{:.6}, {:.6}",
                                                            lat, lon
                                                        ))
                                                        .small(),
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
                                                        .small()
                                                        .color(hex_to_c32(&palette.theme.f_med)),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("{:.1} m", alt))
                                                        .small(),
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
