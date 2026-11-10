use super::*;

impl ImagePreviewState {
    /// Creative FX viewport with FX selection on the left, comparison in the center, and controls on the right.
    pub(crate) fn render_creative_fx_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.invalidate_stale_creative_fx_masks();
        self.invalidate_stale_creative_fx_thumbnails();
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_add = hex_to_c32(&palette.diff_add);

        // Upload any newly received thumbnails into GPU textures
        for (fx_type, rgba) in self.creative_fx_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("creative_fx_thumb_{:?}", fx_type),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.creative_fx_state.fx_thumbnails.insert(fx_type, tex);
        }

        // Fill thumbnails that were still outstanding when the mode was left.
        if self.creative_fx_state.fx_thumbnails.len()
            + self.creative_fx_state.pending_thumbnails.len()
            < CreativeFxType::all().len()
            && !self.creative_fx_state.is_generating_thumbnails
            && self.creative_fx_state.thumbnail_retry_at.is_none()
            && !self.current_rgba.is_empty()
        {
            self.run_creative_fx_thumbnail_batch();
        }

        let panel_layout = ImageToolMode::CreativeFx
            .panel_layout()
            .expect("Creative FX has a panel layout")
            .with_panel_sizes(&self.panel_state);
        let panels = panel_layout.allocate(ui);
        let browser = panels.left.expect("Creative FX has a browser rail");
        let controls = panels.right.expect("Creative FX has a controls rail");
        let browser_w = browser.width();
        let browser_h = browser.height();
        let controls_w = controls.width();
        let controls_h = controls.height();
        let center_w = panels.center.width();
        let main_h = panels.center.height();

        // Fit to view when first opening Creative FX
        if !self.creative_fx_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.creative_fx_state.auto_fitted = true;
        }

        // ── 1. LEFT PANEL: Creative FX List with 42x28 mini preview photos ──
        crate::shell::StudioPanelLayout::show_region(ui, browser, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(browser_w - 16.0);
                    ui.set_height((browser_h - 16.0).max(1.0));

                    ui.horizontal(|ui| {
                        ui.add(
                            crate::icons::Icon::Sparkles.label(
                                egui::RichText::new("Creative FX")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            ),
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", CreativeFxType::all().len()))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        if self.creative_fx_state.is_generating_thumbnails {
                            ui.add(
                                crate::icons::Icon::Hourglass.label(
                                    egui::RichText::new("")
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_low),
                                ),
                            )
                            .on_hover_text("Generating preview thumbnails…");
                        }
                    });
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    if let Some(status) = &self.creative_fx_state.thumbnail_queue_status {
                        ui.label(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                    }
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_XS);

                    egui::ScrollArea::vertical()
                        .id_source("creative_fx_list_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(browser_w - 24.0);
                            for fx in CreativeFxType::all() {
                                let selected = self.creative_fx_state.selected_fx == *fx;
                                let (row_bg, name_col, meta_col) = if selected {
                                    (
                                        base_c,
                                        contrast_text_c32(base_c, None, palette),
                                        contrast_text_c32(base_c, None, palette),
                                    )
                                } else {
                                    (b_low, f_high, f_med)
                                };

                                let row_frame = egui::Frame::none()
                                    .fill(row_bg)
                                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                    .inner_margin(egui::Margin::symmetric(6.0, 5.0));

                                let resp = row_frame
                                    .show(ui, |ui| {
                                        ui.set_width(browser_w - 32.0);
                                        ui.horizontal(|ui| {
                                            // 42x28 mini thumbnail preview
                                            if let Some(tex) =
                                                self.creative_fx_state.fx_thumbnails.get(fx)
                                            {
                                                ui.image((tex.id(), Vec2::new(42.0, 28.0)));
                                            } else {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(42.0, 28.0),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    Rounding::same(2.0),
                                                    b_med,
                                                );
                                            }
                                            ui.add_space(crate::ui_ext::SPACE_XS);
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(fx.name())
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(name_col),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(fx.category())
                                                            .monospace()
                                                            .color(meta_col),
                                                    );
                                                });
                                                ui.label(
                                                    egui::RichText::new(fx.subtitle())
                                                        .color(meta_col)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE),
                                                );
                                            });
                                        });
                                    })
                                    .response;

                                if resp.interact(egui::Sense::click()).clicked() && !selected {
                                    self.creative_fx_state.select_fx(*fx);
                                }
                                ui.add_space(crate::ui_ext::SPACE_XS);
                            }
                        });
                });
        });

        // ── 2. CENTER PANEL: Split-view Comparison Canvas ──
        crate::shell::StudioPanelLayout::show_region(ui, panels.center, |ui| {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.canvas))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.set_width(center_w);
                    ui.set_height(main_h);

                    // Reload preview texture if dirty
                    let preview_is_current = self.creative_fx_state.preview_stamp.as_ref()
                        == Some(&self.creative_fx_render_stamp());
                    if !preview_is_current {
                        self.creative_fx_state.preview_texture = None;
                        self.creative_fx_state.dirty_preview_texture = false;
                    } else if self.creative_fx_state.dirty_preview_texture {
                        if let Some(ref rgba) = self.creative_fx_state.preview_rgba {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            );
                            self.creative_fx_state.preview_texture = Some(ctx.load_texture(
                                "creative_fx_preview_tex",
                                ci,
                                image_texture_options(rgba),
                            ));
                        }
                        self.creative_fx_state.dirty_preview_texture = false;
                    }

                    self.viewport_size = Vec2::new(center_w, main_h);
                    self.zoom = self.zoom.min(self.maximum_zoom(self.viewport_size));
                    let (response, painter) = ui.allocate_painter(
                        Vec2::new(center_w, main_h),
                        egui::Sense::click_and_drag(),
                    );
                    let canvas_rect = response.rect;

                    painter.rect_filled(
                        canvas_rect,
                        Rounding::same(4.0),
                        hex_to_c32(&palette.colors.canvas),
                    );

                    // If mouse is near the comparison split divider when drag starts, initiate split dragging
                    let near_split = if let Some(pos) = response
                        .hover_pos()
                        .or_else(|| response.interact_pointer_pos())
                    {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));
                        let split_x = img_rect.min.x
                            + self.creative_fx_state.comparison_split.clamp(0.01, 0.99)
                                * img_rect.width();
                        (pos.x - split_x).abs() <= 18.0
                            && pos.y >= img_rect.min.y
                            && pos.y <= img_rect.max.y
                    } else {
                        false
                    };
                    if near_split && response.drag_started() {
                        self.creative_fx_state.is_dragging_split = true;
                    }

                    // Pan and Zoom
                    let middle_drag =
                        ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                    if (!self.creative_fx_state.is_dragging_split && response.dragged())
                        || middle_drag
                    {
                        self.pan += response.drag_delta();
                    }
                    if response.hovered() {
                        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
                        if scroll_delta.abs() > 0.0 {
                            let zoom_factor = (scroll_delta * 0.002).exp();
                            let old_zoom = self.zoom;
                            let new_zoom = (old_zoom * zoom_factor)
                                .clamp(0.05, self.maximum_zoom(self.viewport_size));
                            if let Some(ptr_pos) = response.hover_pos() {
                                let center = canvas_rect.center() + self.pan;
                                let to_ptr = ptr_pos - center;
                                self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                            }
                            self.zoom = new_zoom;
                        }
                    }
                    if !self.creative_fx_state.is_dragging_split && response.double_clicked() {
                        let avail_size = Vec2::new(center_w, main_h);
                        self.fit_to_view(avail_size);
                    }

                    if let Some(orig_tex) = self.texture.clone() {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                        draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                        self.render_creative_fx_comparison(
                            ui,
                            &painter,
                            &response,
                            img_rect,
                            canvas_rect,
                            ctx,
                            &orig_tex,
                            palette,
                        );

                        painter.rect_stroke(
                            img_rect,
                            Rounding::ZERO,
                            Stroke::new(
                                1.0_f32,
                                hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                            ),
                        );
                    }

                    // Rendering spinner overlay
                    if self.creative_fx_state.is_rendering {
                        let spinner_pos = canvas_rect.center();
                        let spin_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
                        let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                        painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                        painter.rect_stroke(
                            spin_rect,
                            Rounding::same(8.0),
                            Stroke::new(1.0_f32, b_med),
                        );
                        crate::icons::Icon::Hourglass.paint_text(
                            &painter,
                            spinner_pos,
                            egui::Align2::CENTER_CENTER,
                            "Processing Creative FX…",
                            egui::FontId::proportional(12.0),
                            f_med,
                        );
                    }

                    // Error overlay
                    if let Some(ref err) = self.creative_fx_state.render_error.clone() {
                        let err_pos = Pos2::new(canvas_rect.min.x + 10.0, canvas_rect.min.y + 10.0);
                        crate::icons::Icon::Alert.paint_text(
                            &painter,
                            err_pos,
                            egui::Align2::LEFT_TOP,
                            err.to_string(),
                            egui::FontId::proportional(11.0),
                            hex_to_c32(&palette.diff_del),
                        );
                    }
                });
        });

        // ── 3. RIGHT PANEL: Controls for Selected Effect ──
        crate::shell::StudioPanelLayout::show_region(ui, controls, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(controls_w - 16.0);
                    ui.set_height((controls_h - 16.0).max(1.0));

                    egui::ScrollArea::vertical()
                        .id_source("creative_fx_controls_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let full_w = (controls_w - 32.0).max(120.0);
                            let slider_w = (full_w - 68.0).max(80.0);
                            let cur_fx = self.creative_fx_state.selected_fx;

                            // Header
                            ui.horizontal(|ui| {
                                creative_fx_icon(cur_fx).ui(ui, 16.0, base_c);
                                ui.label(egui::RichText::new(cur_fx.name()).strong().color(base_c));
                            });
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(cur_fx.description())
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            );
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // ── Presets Selector ──
                            ui.add(
                                crate::icons::Icon::Tag.label(
                                    egui::RichText::new("Presets")
                                        .strong()
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(base_c),
                                ),
                            );
                            ui.add_space(3.0);
                            let presets = presets_for_fx(cur_fx);
                            for preset in &presets {
                                let sel =
                                    self.creative_fx_state.selected_preset_name == preset.name;
                                let (s_bg, s_fg) = if sel {
                                    (base_c, contrast_text_c32(base_c, None, palette))
                                } else {
                                    (b_med, f_med)
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(preset.name)
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(s_fg),
                                )
                                .fill(s_bg)
                                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                                if ui.add(btn).on_hover_text(preset.description).clicked() {
                                    self.creative_fx_state.selected_preset_name =
                                        preset.name.to_string();
                                    match &preset.params {
                                        CreativeFxParams::BasicControls(p) => {
                                            self.creative_fx_state.basic_params = p.clone();
                                        }
                                        CreativeFxParams::LensAberration(p) => {
                                            self.creative_fx_state.lens_params = *p;
                                        }
                                        CreativeFxParams::Trichromatic(p) => {
                                            self.creative_fx_state.trichromatic_params = *p;
                                        }
                                        CreativeFxParams::PixelSort(p) => {
                                            self.creative_fx_state.pixel_sort_params = *p;
                                        }
                                        CreativeFxParams::CmykHalftone(p) => {
                                            self.creative_fx_state.cmyk_params = *p;
                                        }
                                        CreativeFxParams::DitherPalette(p) => {
                                            self.creative_fx_state.dither_params = *p;
                                        }
                                        CreativeFxParams::Ntsc(p) => {
                                            self.creative_fx_state.ntsc_params = *p;
                                        }
                                        CreativeFxParams::Structure(p) => {
                                            self.creative_fx_state.structure_params = *p;
                                        }
                                    }
                                    self.creative_fx_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }

                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // ── Parameter Controls ──
                            ui.add(
                                crate::icons::Icon::Settings.label(
                                    egui::RichText::new("Controls")
                                        .strong()
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(base_c),
                                ),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            match cur_fx {
                                CreativeFxType::BasicControls => {
                                    self.render_basic_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::LensAberration => {
                                    self.render_lens_aberration_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::Trichromatic => {
                                    self.render_trichromatic_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::PixelSort => {
                                    self.render_pixel_sort_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::CmykHalftone => {
                                    self.render_cmyk_halftone_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::DitherPalette => {
                                    self.render_dither_palette_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::Ntsc => {
                                    self.render_ntsc_controls(ui, palette, slider_w)
                                }
                                CreativeFxType::Structure => {
                                    self.render_structure_controls(ui, palette, slider_w)
                                }
                            }

                            // ── Masking Controls ──
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            ui.separator();
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                            self.render_creative_fx_mask_sources(ui, ctx, palette, full_w);
                            self.render_creative_fx_mask_options(
                                ui, palette, cur_fx, full_w, slider_w,
                            );

                            ui.add_space(crate::ui_ext::SPACE_SM);
                            ui.separator();
                            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                            // ── Action Buttons ──
                            let apply_fg = contrast_text_c32(diff_add, Some(f_high), palette);
                            let apply_btn = egui::Button::image_and_text(
                                crate::icons::Icon::Check.image_tinted(
                                    crate::ui_ext::CONTROL_ICON_SIZE,
                                    ui.visuals().text_color(),
                                ),
                                egui::RichText::new("Apply to Image")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(apply_fg),
                            )
                            .fill(diff_add)
                            .stroke(Stroke::new(1.0_f32, diff_add))
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                            let apply_enabled = self.creative_fx_state.preview_rgba.is_some()
                                && self.creative_fx_state.preview_stamp.as_ref()
                                    == Some(&self.creative_fx_render_stamp());
                            if ui
                                .add_enabled(apply_enabled, apply_btn)
                                .on_hover_text(
                                    "Commit Creative FX result to image (pushes to undo stack)",
                                )
                                .clicked()
                            {
                                self.apply_creative_fx();
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let reset_fg = contrast_text_c32(b_med, Some(f_med), palette);
                            let reset_btn = egui::Button::new(
                                egui::RichText::new("⟲ Reset Defaults")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .strong()
                                    .color(reset_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0_f32, b_low))
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));

                            if ui
                                .add(reset_btn)
                                .on_hover_text("Reset parameters to default")
                                .clicked()
                            {
                                match cur_fx {
                                    CreativeFxType::BasicControls => {
                                        self.creative_fx_state.basic_params =
                                            BasicControlsParams::default()
                                    }
                                    CreativeFxType::LensAberration => {
                                        self.creative_fx_state.lens_params =
                                            LensAberrationParams::default();
                                    }
                                    CreativeFxType::Trichromatic => {
                                        self.creative_fx_state.trichromatic_params =
                                            TrichromaticParams::default();
                                    }
                                    CreativeFxType::PixelSort => {
                                        self.creative_fx_state.pixel_sort_params =
                                            PixelSortParams::default();
                                    }
                                    CreativeFxType::CmykHalftone => {
                                        self.creative_fx_state.cmyk_params =
                                            CmykHalftoneParams::default();
                                    }
                                    CreativeFxType::DitherPalette => {
                                        self.creative_fx_state.dither_params =
                                            DitherPaletteParams::default();
                                    }
                                    CreativeFxType::Ntsc => {
                                        self.creative_fx_state.ntsc_params = NtscParams::default();
                                    }
                                    CreativeFxType::Structure => {
                                        self.creative_fx_state.structure_params =
                                            StructureParams::default();
                                    }
                                }
                                self.creative_fx_state.selected_preset_name = "Default".to_string();
                                self.creative_fx_state.mark_dirty();
                                self.show_toast("⟲ Parameters reset to defaults");
                            }

                            if self.creative_fx_state.last_render_duration_ms > 0 {
                                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                ui.add(
                                    crate::icons::Icon::Clock.label(
                                        egui::RichText::new(format!(
                                            "{:.2}s",
                                            self.creative_fx_state.last_render_duration_ms as f64
                                                / 1000.0
                                        ))
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_low),
                                    ),
                                );
                            }
                        });
                });
        });
        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            "creative_fx",
        );
    }

    /// Split-view comparison painter for the Creative FX tab center canvas.
    #[allow(clippy::too_many_arguments)]

    pub(crate) fn render_creative_fx_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        _ctx: &egui::Context,
        orig_tex: &egui::TextureHandle,
        palette: &AppPalette,
    ) {
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);

        // Render split comparison using unified SplitCompareView
        let split_view = crate::shell::SplitCompareView::new(
            orig_tex,
            if self.creative_fx_state.preview_stamp.as_ref()
                == Some(&self.creative_fx_render_stamp())
            {
                self.creative_fx_state.preview_texture.as_ref()
            } else {
                None
            },
            &mut self.creative_fx_state.comparison_split,
            &mut self.creative_fx_state.is_dragging_split,
        );
        split_view.paint(ui, response, painter, img_rect, palette);

        // HUD badges
        let badge_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(100.0, 22.0),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0_f32, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            egui::FontId::proportional(11.0),
            hex_to_c32(&palette.colors.text_secondary),
        );

        let cur_fx = self.creative_fx_state.selected_fx;
        let fx_label = if self.creative_fx_state.is_rendering {
            "Processing…".to_string()
        } else {
            format!(
                "{} ({:.1}s)",
                cur_fx.name(),
                self.creative_fx_state.last_render_duration_ms as f64 / 1000.0
            )
        };
        let fx_badge_w = (fx_label.len() as f32 * 6.5 + 18.0).max(130.0);
        let fx_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - fx_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(fx_badge_w, 22.0),
        );
        painter.rect_filled(fx_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(fx_rect, Rounding::same(4.0), Stroke::new(1.0_f32, base_c));
        let badge_icon = if self.creative_fx_state.is_rendering {
            crate::icons::Icon::Hourglass
        } else {
            creative_fx_icon(cur_fx)
        };
        badge_icon.paint_text(
            painter,
            fx_rect.center(),
            egui::Align2::CENTER_CENTER,
            fx_label,
            egui::FontId::proportional(10.0),
            base_c,
        );
    }
}
