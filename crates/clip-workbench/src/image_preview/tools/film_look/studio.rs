use super::*;

impl ImagePreviewState {
    /// Full 3-panel film simulation viewport (replaces the normal render_viewport when FilmSim is active).

    pub(crate) fn render_film_sim_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);

        // Upload any newly received thumbnails into GPU textures
        for (idx, rgba) in self.film_sim_state.pending_thumbnails.drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [rgba.width() as usize, rgba.height() as usize],
                rgba.as_raw(),
            );
            let tex = ctx.load_texture(
                format!("film_sim_thumb_{}", idx),
                ci,
                egui::TextureOptions::LINEAR,
            );
            self.film_sim_state.stock_thumbnails.insert(idx, tex);
        }

        // Fill thumbnails that were still outstanding when the mode was left.
        let panel_layout = ImageToolMode::FilmSim
            .panel_layout()
            .expect("Film Look has a panel layout")
            .with_panel_sizes(&self.panel_state);
        let panels = panel_layout.allocate(ui);
        let left = panels.left.expect("Film Look has a stock browser");
        let right = panels.right.expect("Film Look has an inspector");
        let bottom = panels.bottom.expect("Film Look has a control strip");
        let left_w = left.width();
        let left_h = left.height();
        let right_w = right.width();
        let right_h = right.height();
        let center_w = panels.center.width();
        let main_h = panels.center.height();

        let stocks = all_film_stocks();

        // Ensure fit-to-view is performed on the center canvas when first entering FilmSim
        if !self.film_sim_state.auto_fitted && center_w > 50.0 && main_h > 50.0 {
            self.fit_to_view(Vec2::new(center_w, main_h));
            self.film_sim_state.auto_fitted = true;
        }

        // ── 1. LEFT PANEL: Film Stock Picker with Async 42x28 Mini-Photos ──
        crate::shell::StudioPanelLayout::show_region(ui, left, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(left_w - 16.0);
                    ui.set_height((left_h - 16.0).max(1.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Film Stock")
                                .strong()
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(base_c),
                        );
                        ui.label(
                            egui::RichText::new(format!("({})", stocks.len()))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        if self.film_sim_state.is_generating_thumbnails {
                            ui.label(
                                egui::RichText::new("...")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                            )
                            .on_hover_text("Generating thumbnails in background…");
                        }
                    });
                    ui.add_space(crate::ui_ext::SPACE_XS);

                    // Search box
                    ui.add_sized(
                        [left_w - 24.0, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut self.film_sim_state.search_query)
                            .hint_text("Filter stocks...")
                            .desired_width(left_w - 24.0),
                    );
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.separator();
                    ui.add_space(2.0);

                    let search = self.film_sim_state.search_query.trim().to_lowercase();
                    egui::ScrollArea::vertical()
                        .id_source("film_sim_stock_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(left_w - 24.0);
                            let mut current_brand = "";
                            for (i, entry) in stocks.iter().enumerate() {
                                if !search.is_empty() {
                                    let matches = entry.name.to_lowercase().contains(&search)
                                        || entry.brand.to_lowercase().contains(&search)
                                        || entry.film_type_label.to_lowercase().contains(&search);
                                    if !matches {
                                        continue;
                                    }
                                }

                                // Brand separator heading
                                if entry.brand != current_brand {
                                    if !current_brand.is_empty() {
                                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                    }
                                    ui.label(
                                        egui::RichText::new(entry.brand.to_uppercase())
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    current_brand = entry.brand;
                                }

                                let selected = self.film_sim_state.selected_stock_idx == i;
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
                                    .inner_margin(egui::Margin::symmetric(4.0, 3.0));

                                let resp = row_frame
                                    .show(ui, |ui| {
                                        ui.set_width(left_w - 32.0);
                                        ui.horizontal(|ui| {
                                            // 42x28 mini-photo thumbnail preview
                                            if let Some(tex) =
                                                self.film_sim_state.stock_thumbnails.get(&i)
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
                                            ui.add_space(2.0);
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(entry.name)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(name_col),
                                                );
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(entry.film_type_label)
                                                            .monospace()
                                                            .color(meta_col),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "ISO {}",
                                                            entry.iso
                                                        ))
                                                        .monospace()
                                                        .color(meta_col),
                                                    );
                                                });
                                            });
                                        });
                                    })
                                    .response;

                                if resp.interact(egui::Sense::click()).clicked() && !selected {
                                    self.film_sim_state.selected_stock_idx = i;
                                    self.film_sim_state.mark_dirty();
                                }
                                ui.add_space(2.0);
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
                    if self.film_sim_state.dirty_preview_texture {
                        if let Some(ref rgba) = self.film_sim_state.preview_rgba {
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [rgba.width() as usize, rgba.height() as usize],
                                rgba.as_raw(),
                            );
                            self.film_sim_state.preview_texture = Some(ctx.load_texture(
                                "film_sim_preview_tex",
                                ci,
                                image_texture_options(rgba),
                            ));
                        }
                        self.film_sim_state.dirty_preview_texture = false;
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
                            + self.film_sim_state.comparison_split.clamp(0.01, 0.99)
                                * img_rect.width();
                        (pos.x - split_x).abs() <= 18.0
                            && pos.y >= img_rect.min.y
                            && pos.y <= img_rect.max.y
                    } else {
                        false
                    };
                    if near_split && response.drag_started() {
                        self.film_sim_state.is_dragging_split = true;
                    }

                    // Pan/zoom via this response
                    let middle_drag =
                        ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
                    if (!self.film_sim_state.is_dragging_split && response.dragged()) || middle_drag
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
                    if !self.film_sim_state.is_dragging_split && response.double_clicked() {
                        let avail_size = Vec2::new(center_w, main_h);
                        self.fit_to_view(avail_size);
                    }

                    if let Some(orig_tex) = self.texture.clone() {
                        let img_w = self.current_rgba.width() as f32 * self.zoom;
                        let img_h = self.current_rgba.height() as f32 * self.zoom;
                        let center_pt = canvas_rect.center() + self.pan;
                        let img_rect = Rect::from_center_size(center_pt, Vec2::new(img_w, img_h));

                        draw_checkerboard(&painter, img_rect, self.zoom, b_low, b_med);

                        self.render_film_sim_comparison(
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
                    if self.film_sim_state.is_rendering {
                        let spinner_pos = canvas_rect.center();
                        let spin_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
                        let spin_rect = Rect::from_center_size(spinner_pos, Vec2::new(220.0, 36.0));
                        painter.rect_filled(spin_rect, Rounding::same(8.0), spin_bg);
                        painter.rect_stroke(
                            spin_rect,
                            Rounding::same(8.0),
                            Stroke::new(1.0_f32, b_med),
                        );
                        painter.text(
                            spinner_pos,
                            egui::Align2::CENTER_CENTER,
                            "⏳ Rendering film simulation…",
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            f_med,
                        );
                    }

                    // Error overlay
                    if let Some(ref err) = self.film_sim_state.render_error.clone() {
                        let err_pos = Pos2::new(canvas_rect.min.x + 10.0, canvas_rect.min.y + 10.0);
                        painter.text(
                            err_pos,
                            egui::Align2::LEFT_TOP,
                            format!("⚠ {}", err),
                            egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE),
                            hex_to_c32(&palette.diff_del),
                        );
                    }
                });
        });

        // ── 3. RIGHT PANEL: Style + Optical Effects + Stock Info ──
        crate::shell::StudioPanelLayout::show_region(ui, right, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width(right_w - 16.0);
                    ui.set_height((right_h - 16.0).max(1.0));

                    egui::ScrollArea::vertical()
                        .id_source("film_sim_right_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(right_w - 24.0);

                            // 1. Style
                            ui.label(
                                egui::RichText::new("Style")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let styles = [
                                (
                                    FilmStyle::Accurate,
                                    "Accurate",
                                    "Physics-faithful rendering",
                                ),
                                (
                                    FilmStyle::Artistic,
                                    "Artistic",
                                    "Boosted contrast & saturation",
                                ),
                                (FilmStyle::Vintage, "Vintage", "Warm, faded, nostalgic"),
                                (FilmStyle::HighContrast, "Hi-Con", "High contrast, punchy"),
                                (FilmStyle::Pastel, "Pastel", "Low-contrast, soft tones"),
                            ];

                            for (style, label, tooltip) in &styles {
                                let sel = self.film_sim_state.style == *style;
                                let (s_bg, s_fg) = if sel {
                                    (base_c, contrast_text_c32(base_c, None, palette))
                                } else {
                                    (b_med, f_med)
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(*label)
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .strong()
                                        .color(s_fg),
                                )
                                .fill(s_bg)
                                .stroke(Stroke::new(1.0_f32, if sel { base_c } else { b_med }))
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                .min_size(Vec2::new(right_w - 28.0, crate::ui_ext::CONTROL_HEIGHT));

                                if ui.add(btn).on_hover_text(*tooltip).clicked() && !sel {
                                    self.film_sim_state.style = *style;
                                    self.film_sim_state.mark_dirty();
                                }
                                ui.add_space(2.0);
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // 2. Optical Effects: Halation, Vignette, Light Leak, Grain
                            ui.label(
                                egui::RichText::new("Optics & Effects")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let full_w = (right_w - 32.0).max(100.0);
                            let opt_slider_w = (full_w - 68.0).max(60.0);

                            if render_context_slider(
                                ui,
                                "Halation",
                                &mut self.film_sim_state.halation,
                                0.0..=1.0,
                                0.25,
                                "%",
                                opt_slider_w,
                                palette,
                            ) {
                                self.film_sim_state.mark_dirty();
                            }
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            if render_context_slider(
                                ui,
                                "Vignette",
                                &mut self.film_sim_state.vignette,
                                0.0..=1.0,
                                0.35,
                                "%",
                                opt_slider_w,
                                palette,
                            ) {
                                self.film_sim_state.mark_dirty();
                            }
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            ui.label(
                                egui::RichText::new("Light Leak")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_med),
                            );
                            egui::ComboBox::from_id_source("film_sim_light_leak_combo")
                                .selected_text(self.film_sim_state.light_leak.label())
                                .width(full_w)
                                .show_ui(ui, |ui| {
                                    let presets = [
                                        LightLeakPreset::None,
                                        LightLeakPreset::WarmCorner,
                                        LightLeakPreset::SunStreak,
                                        LightLeakPreset::SoftEdge,
                                        LightLeakPreset::OrganicBlob,
                                    ];
                                    for p in presets {
                                        let sel = self.film_sim_state.light_leak == p;
                                        if ui.selectable_label(sel, p.label()).clicked() && !sel {
                                            self.film_sim_state.light_leak = p;
                                            self.film_sim_state.mark_dirty();
                                        }
                                    }
                                });
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let mut grain_on = self.film_sim_state.enable_grain;
                            if ui
                                .checkbox(
                                    &mut grain_on,
                                    egui::RichText::new("Grain emulation")
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_med),
                                )
                                .changed()
                            {
                                self.film_sim_state.enable_grain = grain_on;
                                self.film_sim_state.mark_dirty();
                            }

                            ui.add_space(crate::ui_ext::SPACE_XS);
                            ui.separator();
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            // 3. Stock metadata
                            let idx = self
                                .film_sim_state
                                .selected_stock_idx
                                .min(stocks.len().saturating_sub(1));
                            let entry = &stocks[idx];
                            ui.label(
                                egui::RichText::new("Stock Info")
                                    .strong()
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(base_c),
                            );
                            ui.add_space(crate::ui_ext::SPACE_XS);

                            let stock_built = (entry.build)();
                            egui::Grid::new("film_sim_stock_meta_grid")
                                .spacing(Vec2::new(6.0, 3.0))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("Brand:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.brand)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Film:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.name)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("ISO:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!("{}", entry.iso))
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Type:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(entry.film_type_label)
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_high),
                                    );
                                    ui.end_row();

                                    ui.label(
                                        egui::RichText::new("Res:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .color(f_med),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:.0} lp/mm",
                                            stock_built.resolution_lp_mm
                                        ))
                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                        .color(f_low),
                                    );
                                    ui.end_row();
                                });

                            if self.film_sim_state.last_render_duration_ms > 0 {
                                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "⏱ {:.2}s",
                                        self.film_sim_state.last_render_duration_ms as f64 / 1000.0
                                    ))
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(f_low),
                                );
                            }
                        });
                });
        });

        // ── BOTTOM STRIP: Context Sliders + WB Controls + Action Buttons ───────────
        crate::shell::StudioPanelLayout::show_region(ui, bottom, |ui| {
            egui::Frame::none()
                .fill(b_low)
                .stroke(Stroke::new(1.0_f32, b_med))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::symmetric(
                    crate::ui_ext::SPACE_MD,
                    crate::ui_ext::CONTROL_GROUP_INSET_Y,
                ))
                .show(ui, |ui| {
                    ui.set_width((bottom.width() - 24.0).max(1.0));
                    egui::ScrollArea::both()
                        .id_source("film_sim_bottom_controls")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                // Row 1: Exposure & Color Sliders with contextual start/center/end/default tracks
                                ui.horizontal(|ui| {
                                    let slider_w =
                                        ((bottom.width() - 48.0) / 6.0 - 12.0).clamp(70.0, 115.0);

                                    // Brightness
                                    if render_context_slider(
                                        ui,
                                        "Brightness",
                                        &mut self.film_sim_state.brightness,
                                        0.1..=4.0,
                                        1.0,
                                        "×",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Contrast
                                    if render_context_slider(
                                        ui,
                                        "Contrast",
                                        &mut self.film_sim_state.contrast,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Warmth
                                    if render_context_slider(
                                        ui,
                                        "Warmth",
                                        &mut self.film_sim_state.warmth,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Tint (Green ↔ Magenta)
                                    if render_context_slider(
                                        ui,
                                        "Tint",
                                        &mut self.film_sim_state.tint,
                                        -1.0..=1.0,
                                        0.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // Saturation
                                    if render_context_slider(
                                        ui,
                                        "Saturation",
                                        &mut self.film_sim_state.intensity,
                                        0.0..=2.0,
                                        1.0,
                                        "",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                    ui.add_space(3.0);

                                    // WB Strength
                                    if render_context_slider(
                                        ui,
                                        "WB Str",
                                        &mut self.film_sim_state.wb_strength,
                                        0.0..=1.0,
                                        DEFAULT_WB_STRENGTH,
                                        "%",
                                        slider_w,
                                        palette,
                                    ) {
                                        self.film_sim_state.mark_dirty();
                                    }
                                });

                                ui.add_space(3.0);

                                // Row 2: WB Mode Selector + Auto Presets + Actions
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("WB:")
                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                            .strong()
                                            .color(base_c),
                                    );
                                    let wb_modes = [
                                        (WhiteBalanceMode::Auto, "Auto"),
                                        (WhiteBalanceMode::Gray, "Gray"),
                                        (WhiteBalanceMode::White, "White"),
                                        (WhiteBalanceMode::Off, "Off"),
                                    ];
                                    for (mode, label) in wb_modes {
                                        let sel = self.film_sim_state.wb_mode == mode;
                                        let (btn_bg, btn_fg) = if sel {
                                            (base_c, contrast_text_c32(base_c, None, palette))
                                        } else {
                                            (b_med, f_med)
                                        };
                                        let b = egui::Button::new(
                                            egui::RichText::new(label)
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .strong()
                                                .color(btn_fg),
                                        )
                                        .fill(btn_bg)
                                        .stroke(Stroke::new(
                                            1.0_f32,
                                            if sel { base_c } else { b_low },
                                        ))
                                        .rounding(Rounding::same(
                                            crate::ui_ext::BADGE_CORNER_RADIUS,
                                        ))
                                        .min_size(Vec2::new(36.0, crate::ui_ext::CONTROL_HEIGHT));
                                        let description = match mode {
                                            WhiteBalanceMode::Auto => {
                                                "Balance from plausible neutral highlights; skips images without reliable neutral samples"
                                            }
                                            WhiteBalanceMode::Gray => {
                                                "Balance the scene-wide average toward neutral gray"
                                            }
                                            WhiteBalanceMode::White => {
                                                "Balance each channel from its brightest 1% white-patch reference"
                                            }
                                            WhiteBalanceMode::Off => "Do not apply white-balance correction",
                                        };
                                        if ui.add(b).on_hover_text(description).clicked() && !sel {
                                            self.film_sim_state.wb_mode = mode;
                                            self.film_sim_state.mark_dirty();
                                        }
                                    }

                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                    ui.separator();
                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                                    // Auto Levels
                                    let al_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Wand),
                                        "Auto Levels",
                                        diff_mod,
                                        palette,
                                    );
                                    if ui
                            .add(al_btn)
                            .on_hover_text(
                                "Reset brightness to neutral, warmth to 0, and tint to 0",
                            )
                            .clicked()
                        {
                            self.film_sim_state.brightness = 1.0;
                            self.film_sim_state.warmth = 0.0;
                            self.film_sim_state.tint = 0.0;
                            self.film_sim_state.mark_dirty();
                            self.show_toast("Auto levels applied");
                        }

                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    // Auto Enhance
                                    let ae_btn = crate::ui_ext::secondary_button_icon(
                                        Some(Icon::Sparkles),
                                        "Auto Enhance",
                                        base_c,
                                        palette,
                                    );
                                    if ui
                            .add(ae_btn)
                            .on_hover_text(
                                "Boost intensity, warmth, and contrast for a cinematic look",
                            )
                            .clicked()
                        {
                            self.film_sim_state.brightness =
                                (self.film_sim_state.brightness * 1.05).min(4.0);
                            self.film_sim_state.intensity =
                                (self.film_sim_state.intensity + 0.15).min(2.0);
                            self.film_sim_state.warmth =
                                (self.film_sim_state.warmth + 0.05).min(1.0);
                            self.film_sim_state.contrast =
                                (self.film_sim_state.contrast + 0.08).min(1.0);
                            self.film_sim_state.mark_dirty();
                            self.show_toast("Auto enhance applied");
                        }

                                    // Right aligned: Apply, Reset, Undo
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let apply_btn = crate::ui_ext::action_button_icon(
                                                Some(Icon::Check),
                                                "Apply to Image",
                                                diff_add,
                                                palette,
                                            );
                                            let apply_enabled = self.film_sim_preview_is_current();
                                            if ui
                                .add_enabled(apply_enabled, apply_btn)
                                .on_hover_text(
                                    "Commit film simulation result to image (adds to undo stack)",
                                )
                                .clicked()
                            {
                                self.apply_film_sim();
                            }

                                            ui.add_space(crate::ui_ext::SPACE_XS);

                                            let reset_btn = crate::ui_ext::secondary_button_icon(
                                                Some(Icon::RotateCcw),
                                                "Reset",
                                                b_med,
                                                palette,
                                            );
                                            if ui
                                                .add(reset_btn)
                                                .on_hover_text(
                                                    "Reset all film sim parameters to defaults",
                                                )
                                                .clicked()
                                            {
                                                self.film_sim_state.brightness = 1.0;
                                                self.film_sim_state.contrast = 0.0;
                                                self.film_sim_state.warmth = 0.0;
                                                self.film_sim_state.tint = 0.0;
                                                self.film_sim_state.intensity = 1.0;
                                                self.film_sim_state.wb_mode =
                                                    WhiteBalanceMode::Auto;
                                                self.film_sim_state.wb_strength =
                                                    DEFAULT_WB_STRENGTH;
                                                self.film_sim_state.halation = 0.25;
                                                self.film_sim_state.vignette = 0.35;
                                                self.film_sim_state.light_leak =
                                                    LightLeakPreset::None;
                                                self.film_sim_state.enable_grain = true;
                                                self.film_sim_state.style = FilmStyle::Accurate;
                                                self.film_sim_state.mark_dirty();
                                                self.show_toast(
                                                    "⟲ All parameters reset to defaults",
                                                );
                                            }

                                            ui.add_space(crate::ui_ext::SPACE_XS);

                                            if !self.undo_stack.is_empty() {
                                                let undo_fg = contrast_text_c32(
                                                    b_med,
                                                    Some(diff_mod),
                                                    palette,
                                                );
                                                let undo_btn = egui::Button::new(
                                                    egui::RichText::new("⟲ Undo (Ctrl+Z)")
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .strong()
                                                        .color(undo_fg),
                                                )
                                                .fill(b_med)
                                                .rounding(Rounding::same(
                                                    crate::ui_ext::CONTROL_CORNER_RADIUS,
                                                ));
                                                if ui.add(undo_btn).clicked() {
                                                    self.undo();
                                                    self.film_sim_state.preview_rgba = None;
                                                    self.film_sim_state.preview_texture = None;
                                                    self.film_sim_state.mark_dirty();
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                        });
                });
        });
        crate::shell::StudioPanelLayout::show_resize_handles(
            ui,
            &panels,
            &mut self.panel_state,
            palette,
            "film_look",
        );
    }

    /// Split-view comparison painter for the Film Sim tab center canvas.
    #[allow(clippy::too_many_arguments)]

    pub(crate) fn render_film_sim_comparison(
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
            self.film_sim_state.preview_texture.as_ref(),
            &mut self.film_sim_state.comparison_split,
            &mut self.film_sim_state.is_dragging_split,
        );
        split_view.paint(ui, response, painter, img_rect, palette);

        // HUD badges
        let badge_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 210);
        let badge_font = egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE);
        let orig_badge_w = painter
            .layout_no_wrap("ORIGINAL".to_owned(), badge_font.clone(), b_med)
            .size()
            .x
            + 20.0;

        let orig_rect = Rect::from_min_size(
            Pos2::new(img_rect.min.x + 10.0, img_rect.min.y + 10.0),
            Vec2::new(orig_badge_w, crate::ui_ext::CONTROL_HEIGHT),
        );
        painter.rect_filled(orig_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(orig_rect, Rounding::same(4.0), Stroke::new(1.0_f32, b_med));
        painter.text(
            orig_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ORIGINAL",
            badge_font.clone(),
            hex_to_c32(&palette.colors.text_secondary),
        );

        let stocks = all_film_stocks();
        let idx = self
            .film_sim_state
            .selected_stock_idx
            .min(stocks.len().saturating_sub(1));
        let sim_label = if self.film_sim_state.is_rendering {
            "⏳ Rendering…".to_string()
        } else {
            format!(
                "🎞 {} ({:.1}s)",
                stocks[idx].name,
                self.film_sim_state.last_render_duration_ms as f64 / 1000.0
            )
        };
        let sim_badge_w = painter
            .layout_no_wrap(sim_label.clone(), badge_font.clone(), base_c)
            .size()
            .x
            + 20.0;
        let sim_rect = Rect::from_min_size(
            Pos2::new(img_rect.max.x - sim_badge_w - 10.0, img_rect.min.y + 10.0),
            Vec2::new(sim_badge_w, crate::ui_ext::CONTROL_HEIGHT),
        );
        painter.rect_filled(sim_rect, Rounding::same(4.0), badge_bg);
        painter.rect_stroke(sim_rect, Rounding::same(4.0), Stroke::new(1.0_f32, base_c));
        painter.text(
            sim_rect.center(),
            egui::Align2::CENTER_CENTER,
            sim_label,
            badge_font,
            base_c,
        );
    }
}
