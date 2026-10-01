use super::*;

impl DiffViewerState {
    pub(super) fn ensure_image_textures(&mut self, ctx: &egui::Context) {
        if self.textures_loaded || self.is_computing {
            return;
        }

        if let Some(res) = &self.image_result {
            let max_tex_dim = 2048;
            let (w, h, base_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.base_rgba,
                max_tex_dim,
            );
            let (_, _, comp_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.comp_rgba,
                max_tex_dim,
            );
            let (_, _, mask_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.mask_rgba,
                max_tex_dim,
            );
            let (_, _, overlay_rgba) = little_clip_ops::image_diff::downsample_rgba(
                res.width,
                res.height,
                &res.overlay_rgba,
                max_tex_dim,
            );

            let size = [w as usize, h as usize];
            if size[0] > 0 && size[1] > 0 {
                let base_img = egui::ColorImage::from_rgba_unmultiplied(size, &base_rgba);
                self.base_texture =
                    Some(ctx.load_texture("diff-base", base_img, egui::TextureOptions::LINEAR));

                let comp_img = egui::ColorImage::from_rgba_unmultiplied(size, &comp_rgba);
                self.comp_texture =
                    Some(ctx.load_texture("diff-comp", comp_img, egui::TextureOptions::LINEAR));

                let mask_img = egui::ColorImage::from_rgba_unmultiplied(size, &mask_rgba);
                self.mask_texture =
                    Some(ctx.load_texture("diff-mask", mask_img, egui::TextureOptions::NEAREST));

                let overlay_img = egui::ColorImage::from_rgba_unmultiplied(size, &overlay_rgba);
                self.overlay_texture = Some(ctx.load_texture(
                    "diff-overlay",
                    overlay_img,
                    egui::TextureOptions::LINEAR,
                ));
            }
            self.textures_loaded = true;
        }
    }

    pub(super) fn render_image_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::SPACE_MD,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
            .show(ui, |ui| {
                ui.set_min_height(CONTROL_HEIGHT);
                ui.horizontal(|ui| {
                    for mode in &[
                        ImageDiffViewMode::ThreeUp,
                        ImageDiffViewMode::Overlay,
                        ImageDiffViewMode::MaskOnly,
                        ImageDiffViewMode::Split,
                    ] {
                        let is_active = self.image_mode == *mode;
                        let btn = crate::ui_ext::mode_tab_button_icon(Some(mode.icon()), mode.label(), is_active, palette);
                        if ui.add(btn).clicked() {
                            self.image_mode = *mode;
                        }
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_SM);

                    ui.label(
                        egui::RichText::new("Tolerance:")
                            .size(BODY_FONT_SIZE)
                            .color(f_low),
                    );
                    let slider = egui::Slider::new(&mut self.image_options.threshold, 0.00..=0.50)
                        .step_by(0.01)
                        .show_value(true);
                    if ui.add(slider).on_hover_text("Allowable perceptual difference threshold (0.00 = strict, 0.50 = loose)").changed() {
                        self.recompute();
                    }

                    for (val, label) in &[(0.05f32, "0.05"), (0.10f32, "0.10"), (0.20f32, "0.20")] {
                        let is_cur = (self.image_options.threshold - val).abs() < 0.005;
                        let t_btn = crate::mode_tab_button(*label, is_cur, palette);
                        if ui.add(t_btn).clicked() && !is_cur {
                            self.image_options.threshold = *val;
                            self.recompute();
                        }
                    }
                });
            });
    }

    pub(super) fn render_image_viewport(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);
        let f_med = crate::hex_to_c32(&palette.colors.text_secondary);
        let del_color = crate::hex_to_c32(&palette.diff_del);
        let del_label_color = crate::ui_ext::contrast_text_c32_on(
            ui.visuals().panel_fill,
            ui.visuals().panel_fill,
            Some(del_color),
            palette,
        );

        if self.is_computing {
            self.render_computing_ui(ui, palette);
        } else if let Some(res) = &self.image_result {
            let aspect = (res.width as f32) / (res.height as f32).max(1.0);

            match self.image_mode {
                ImageDiffViewMode::ThreeUp => {
                    let avail_w = ui.available_width() - 32.0;
                    let col_w = ((avail_w - 24.0) / 3.0).max(180.0);
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = col_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.horizontal(|ui| {
                        ui.add_space(crate::ui_ext::SPACE_MD);

                        // Column 1: Base
                        self.render_image_column(
                            ui,
                            palette,
                            "base",
                            self.lhs.label(),
                            f_high,
                            self.base_texture.as_ref(),
                            img_w,
                            img_h,
                        );

                        ui.add_space(crate::ui_ext::SPACE_MD);

                        // Column 2: Comparison
                        self.render_image_column(
                            ui,
                            palette,
                            "comparison",
                            self.rhs.label(),
                            f_high,
                            self.comp_texture.as_ref(),
                            img_w,
                            img_h,
                        );

                        ui.add_space(crate::ui_ext::SPACE_MD);

                        // Column 3: Diff
                        let diff_sub = format!(
                            "{} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        );
                        self.render_image_column(
                            ui,
                            palette,
                            "diff",
                            &diff_sub,
                            del_color,
                            self.mask_texture.as_ref(),
                            img_w,
                            img_h,
                        );
                    });
                }
                ImageDiffViewMode::Overlay => {
                    let avail_w = ui.available_width() - 32.0;
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = avail_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Diff Overlay (Dimmed Base + Glowing Difference)")
                                .strong()
                                .color(f_high),
                        );
                        ui.add_space(crate::ui_ext::SPACE_XS);

                        if let Some(tex) = &self.overlay_texture {
                            egui::Frame::none()
                                .fill(b_low)
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .stroke(Stroke::new(1.0_f32, b_med))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_XS))
                                .show(ui, |ui| {
                                    ui.image(egui::load::SizedTexture::new(
                                        tex.id(),
                                        Vec2::new(img_w, img_h),
                                    ));
                                });
                        }
                    });
                }
                ImageDiffViewMode::MaskOnly => {
                    let avail_w = ui.available_width() - 32.0;
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = avail_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Diff Mask (Dark Canvas + Glowing Difference)")
                                .strong()
                                .color(del_label_color),
                        );
                        ui.add_space(crate::ui_ext::SPACE_XS);

                        if let Some(tex) = &self.mask_texture {
                            egui::Frame::none()
                                .fill(b_low)
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .stroke(Stroke::new(1.0_f32, b_med))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_XS))
                                .show(ui, |ui| {
                                    ui.image(egui::load::SizedTexture::new(
                                        tex.id(),
                                        Vec2::new(img_w, img_h),
                                    ));
                                });
                        }
                    });
                }
                ImageDiffViewMode::Split => {
                    let avail_w = ui.available_width() - 32.0;
                    let col_w = ((avail_w - 16.0) / 2.0).max(240.0);
                    let avail_h = (ui.available_height() - 40.0).max(200.0);

                    let mut img_w = col_w;
                    let mut img_h = img_w / aspect;
                    if img_h > avail_h {
                        img_h = avail_h;
                        img_w = img_h * aspect;
                    }

                    ui.horizontal(|ui| {
                        ui.add_space(crate::ui_ext::SPACE_MD);

                        self.render_image_column(
                            ui,
                            palette,
                            "base",
                            self.lhs.label(),
                            f_high,
                            self.base_texture.as_ref(),
                            img_w,
                            img_h,
                        );

                        ui.add_space(16.0);

                        let diff_sub = format!(
                            "{} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        );
                        self.render_image_column(
                            ui,
                            palette,
                            "diff",
                            &diff_sub,
                            del_color,
                            self.mask_texture.as_ref(),
                            img_w,
                            img_h,
                        );
                    });
                }
            }
        } else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Unable to Compare Images")
                            .color(del_label_color)
                            .heading(),
                    );
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    let err_text = self
                        .decode_error
                        .as_deref()
                        .unwrap_or("Unable to decode one or both images for comparison");
                    ui.label(
                        egui::RichText::new(err_text)
                            .color(f_med)
                            .size(BODY_FONT_SIZE),
                    );
                });
            });
        }
    }

    pub(super) fn render_image_column(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
        title: &str,
        subtitle: &str,
        title_color: Color32,
        texture: Option<&egui::TextureHandle>,
        w: f32,
        h: f32,
    ) {
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let surface = ui.visuals().panel_fill;

        ui.vertical(|ui| {
            ui.set_width(w);

            // Column Header
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).strong().color(
                    crate::ui_ext::contrast_text_c32_on(
                        surface,
                        surface,
                        Some(title_color),
                        palette,
                    ),
                ));
                ui.label(
                    egui::RichText::new(format!("• {}", subtitle))
                        .size(BODY_FONT_SIZE)
                        .color(f_low),
                );
            });

            ui.add_space(crate::ui_ext::SPACE_XS);

            // Image Frame Box
            egui::Frame::none()
                .fill(b_low)
                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                .stroke(Stroke::new(1.0_f32, b_med))
                .inner_margin(egui::Margin::same(2.0))
                .show(ui, |ui| {
                    if let Some(tex) = texture {
                        ui.image(egui::load::SizedTexture::new(tex.id(), Vec2::new(w, h)));
                    } else {
                        ui.allocate_space(Vec2::new(w, h));
                    }
                });
        });
    }

    pub(super) fn copy_image_diff_to_clipboard(&mut self, now: f64) {
        if let Some(res) = &self.image_result {
            if let Ok(req) = image_mask_push_request(res) {
                let _ = send_ipc_request(&req);
                self.status_toast = Some((
                    "Diff Mask copied to Left Hand & clipboard!".to_string(),
                    now,
                ));
            }
        }
    }
}
