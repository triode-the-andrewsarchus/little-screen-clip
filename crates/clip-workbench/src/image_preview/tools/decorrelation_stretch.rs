use super::super::*;

/// Keep numeric rows aligned and readable even when the analysis drawer is narrow.
fn draw_numeric_rows(
    ui: &mut egui::Ui,
    id: &str,
    rows: impl IntoIterator<Item = String>,
    color: Color32,
) {
    egui::ScrollArea::horizontal()
        .id_source(id)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.vertical(|ui| {
                for row in rows {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(row)
                                .font(egui::FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                                .color(color),
                        )
                        .wrap(false),
                    );
                }
            });
        });
}

#[derive(Clone)]
pub struct DecorrelationStretchState {
    pub params: DecorrelationStretchParams,
    pub custom_mean: f64,
    pub custom_sigma: f64,
    pub stats: Option<DecorrelationStats>,
    pub transform: Option<DecorrelationTransform>,
    pub stretched_rgba: Option<RgbaImage>,
    pub stretched_texture: Option<egui::TextureHandle>,
    pub dirty_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub is_holding_original: bool,
    pub last_duration_ms: u128,
    pub params_dirty: bool,
}

impl Default for DecorrelationStretchState {
    fn default() -> Self {
        Self {
            params: DecorrelationStretchParams::nasa(),
            custom_mean: 127.5,
            custom_sigma: 50.0,
            stats: None,
            transform: None,
            stretched_rgba: None,
            stretched_texture: None,
            dirty_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            is_holding_original: false,
            last_duration_ms: 0,
            params_dirty: false,
        }
    }
}

impl ImagePreviewState {
    pub fn update_decorrelation_stretch(&mut self) {
        let base_img = self.current_display_image();
        let t0 = std::time::Instant::now();
        let (stats, transform, stretched) =
            run_decorrelation_stretch(&base_img, &self.dcs_state.params);
        let duration_ms = t0.elapsed().as_millis();

        self.dcs_state.stats = Some(stats);
        self.dcs_state.transform = Some(transform);
        self.dcs_state.stretched_rgba = Some(stretched);
        self.dcs_state.dirty_texture = true;
        self.dcs_state.last_duration_ms = duration_ms;
        self.dcs_state.params_dirty = false;
    }

    pub fn apply_decorrelation_stretch(&mut self) {
        if let Some(stretched) = self.dcs_state.stretched_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = stretched;
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.update_decorrelation_stretch();
            self.show_toast("✔ Applied decorrelation stretch to canvas");
        }
    }

    pub(crate) fn render_decorrelation_stretch_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let border = hex_to_c32(&palette.colors.surface_recessed);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let success = hex_to_c32(&palette.diff_add);
        let mut recompute = false;

        ui.label(
            egui::RichText::new("Decorrelation stretch")
                .size(crate::ui_ext::HEADING_FONT_SIZE)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Separate correlated color channels to reveal subtle variation.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("PRESET")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let preset_width =
            crate::ui_ext::wrapped_equal_button_width(ui, &["NASA / ASTER", "Preserve"], false, 2);
        ui.horizontal_wrapped(|ui| {
            for (preset, label, tooltip) in [
                (
                    DecorrelationStretchParams::nasa(),
                    "NASA / ASTER",
                    "Balanced color enhancement with mean 127.5 and spread 50",
                ),
                (
                    DecorrelationStretchParams::matlab_preserve(),
                    "Preserve",
                    "MATLAB-style stretch that preserves band means and spread",
                ),
            ] {
                let active = self.dcs_state.params == preset;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui
                    .add_sized([preset_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .on_hover_text(tooltip)
                    .clicked()
                    && !active
                {
                    self.dcs_state.params = preset;
                    if preset == DecorrelationStretchParams::nasa() {
                        self.dcs_state.custom_mean = 127.5;
                        self.dcs_state.custom_sigma = 50.0;
                    }
                    recompute = true;
                }
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("COLOR MODEL")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let mode_width =
            crate::ui_ext::wrapped_equal_button_width(ui, &["Correlation", "Covariance"], false, 2);
        ui.horizontal_wrapped(|ui| {
            for (mode, label, tooltip) in [
                (
                    DecorrelationMatrixMode::Correlation,
                    "Correlation",
                    "Give each RGB band equal weight",
                ),
                (
                    DecorrelationMatrixMode::Covariance,
                    "Covariance",
                    "Weight RGB bands by their original variance",
                ),
            ] {
                let active = self.dcs_state.params.mode == mode;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui
                    .add_sized([mode_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .on_hover_text(tooltip)
                    .clicked()
                    && !active
                {
                    self.dcs_state.params.mode = mode;
                    recompute = true;
                }
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OUTPUT DISTRIBUTION")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );

        let mut preserve_mean = matches!(self.dcs_state.params.target_mean, TargetValue::Preserve);
        if ui
            .checkbox(
                &mut preserve_mean,
                egui::RichText::new("Preserve original band means")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(text),
            )
            .changed()
        {
            self.dcs_state.params.target_mean = if preserve_mean {
                TargetValue::Preserve
            } else {
                TargetValue::Custom(self.dcs_state.custom_mean)
            };
            recompute = true;
        }
        if !preserve_mean {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Target mean")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.1}", self.dcs_state.custom_mean))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let response = ui.add(
                egui::Slider::new(&mut self.dcs_state.custom_mean, 0.0..=255.0).show_value(false),
            );
            if response.changed() {
                self.dcs_state.params.target_mean = TargetValue::Custom(self.dcs_state.custom_mean);
                self.dcs_state.params_dirty = true;
                recompute |= !response.dragged();
            }
            if response.drag_stopped() && self.dcs_state.params_dirty {
                recompute = true;
            }
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        let mut preserve_sigma =
            matches!(self.dcs_state.params.target_sigma, TargetValue::Preserve);
        if ui
            .checkbox(
                &mut preserve_sigma,
                egui::RichText::new("Preserve original band spread")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(text),
            )
            .changed()
        {
            self.dcs_state.params.target_sigma = if preserve_sigma {
                TargetValue::Preserve
            } else {
                TargetValue::Custom(self.dcs_state.custom_sigma)
            };
            recompute = true;
        }
        if !preserve_sigma {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Target spread")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.1}", self.dcs_state.custom_sigma))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let response = ui.add(
                egui::Slider::new(&mut self.dcs_state.custom_sigma, 1.0..=200.0).show_value(false),
            );
            if response.changed() {
                self.dcs_state.params.target_sigma =
                    TargetValue::Custom(self.dcs_state.custom_sigma);
                self.dcs_state.params_dirty = true;
                recompute |= !response.dragged();
            }
            if response.drag_stopped() && self.dcs_state.params_dirty {
                recompute = true;
            }
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("SAMPLING & CLIPPING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        ui.label(
            egui::RichText::new("Highlight clipping")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        let current_tol = self.dcs_state.params.tol;
        egui::ComboBox::from_id_source("dcs_tol_combo")
            .selected_text(match current_tol {
                value if value <= 0.0 => "Off".to_string(),
                value => format!("{:.1}% per tail", value * 100.0),
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (value, label) in [
                    (0.0, "Off"),
                    (0.005, "0.5% per tail"),
                    (0.01, "1% per tail"),
                    (0.02, "2% per tail"),
                ] {
                    if ui
                        .selectable_label((current_tol - value).abs() < 1e-4, label)
                        .clicked()
                    {
                        self.dcs_state.params.tol = value;
                        recompute = true;
                    }
                }
            });

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("Statistics sampling")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        let current_stride = self.dcs_state.params.stride;
        egui::ComboBox::from_id_source("dcs_stride_combo")
            .selected_text(match current_stride {
                0 => "Automatic".to_string(),
                1 => "Every pixel".to_string(),
                value => format!("Every {} pixels", value),
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (value, label) in [
                    (0, "Automatic"),
                    (1, "Every pixel"),
                    (2, "Every 2 pixels"),
                    (3, "Every 3 pixels"),
                    (5, "Every 5 pixels"),
                ] {
                    if ui
                        .selectable_label(current_stride == value, label)
                        .clicked()
                    {
                        self.dcs_state.params.stride = value;
                        recompute = true;
                    }
                }
            });

        ui.add_space(crate::ui_ext::SPACE_MD);
        let hold = crate::secondary_button("Hold to view original", surface, palette);
        let hold_response = ui
            .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], hold)
            .on_hover_text("Press and hold to temporarily hide the stretch");
        self.dcs_state.is_holding_original = hold_response.is_pointer_button_down_on();

        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        let apply = crate::action_button("Apply stretch (Enter)", success, palette);
        if ui
            .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
            .on_hover_text("Commit the current stretch to the image")
            .clicked()
        {
            self.apply_decorrelation_stretch();
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let refresh = crate::secondary_button("Recompute preview", surface, palette);
        if ui
            .add_sized(
                [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                refresh,
            )
            .on_hover_text("Recalculate statistics and the color transform")
            .clicked()
        {
            self.update_decorrelation_stretch();
        }

        if !self.undo_stack.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_XS);
            let undo = crate::secondary_button("Undo last edit (Ctrl+Z)", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], undo)
                .clicked()
            {
                self.undo();
                self.update_decorrelation_stretch();
            }
        }

        if recompute {
            self.update_decorrelation_stretch();
        }
    }

    pub(crate) fn render_decorrelation_stretch_stats_drawer(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
    ) {
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_del = hex_to_c32(&palette.diff_del);
        let base_c = hex_to_c32(&palette.harmonies.base);

        ui.label(
            egui::RichText::new("Stretch analysis")
                .size(crate::ui_ext::HEADING_FONT_SIZE)
                .strong()
                .color(base_c),
        );
        ui.label(
            egui::RichText::new("Input statistics and the generated RGB transform.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_low),
        );

        let (Some(stats), Some(transform)) = (&self.dcs_state.stats, &self.dcs_state.transform)
        else {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.label(
                egui::RichText::new("No analysis is available yet.")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .italics()
                    .color(f_med),
            );
            return;
        };

        let stride_used = if self.dcs_state.params.stride == 0 {
            auto_stride(self.current_rgba.width(), self.current_rgba.height())
        } else {
            self.dcs_state.params.stride
        };

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OVERVIEW")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        for (label, value) in [
            ("Samples", format!("{} px", stats.sample_count)),
            ("Compute", format!("{} ms", self.dcs_state.last_duration_ms)),
            ("Stride", stride_used.to_string()),
            (
                "Matrix",
                match self.dcs_state.params.mode {
                    DecorrelationMatrixMode::Correlation => "Correlation".to_string(),
                    DecorrelationMatrixMode::Covariance => "Covariance".to_string(),
                },
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_low),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(value)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(f_high),
                    );
                });
            });
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("CHANNEL STATISTICS")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        for (index, band) in ["R", "G", "B"].into_iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(band)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(base_c),
                );
                ui.label(
                    egui::RichText::new(format!("Mean {:.1}", stats.mean[index]))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(f_high),
                );
                ui.label(
                    egui::RichText::new(format!("Spread {:.1}", stats.sd[index]))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(f_high),
                );
            });
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("CORRELATION MATRIX")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_corr",
            stats
                .corr
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2])),
            f_high,
        );

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("COVARIANCE MATRIX")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_cov",
            stats
                .cov
                .map(|row| format!("[{:>7.1}  {:>7.1}  {:>7.1}]", row[0], row[1], row[2])),
            f_high,
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("PRINCIPAL COMPONENTS")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        for (index, value) in transform.eigenvalues.into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("λ{}", index + 1))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_low),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.5}", value))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(f_high),
                    );
                });
            });
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("EIGENVECTORS")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_eigenvectors",
            transform
                .eigenvectors
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2])),
            f_high,
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("RGB TRANSFORM")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_transform",
            transform
                .transform
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2])),
            f_high,
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(format!(
                "Offset  [{:.1}, {:.1}, {:.1}]",
                transform.offset[0], transform.offset[1], transform.offset[2]
            ))
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .monospace()
            .color(f_low),
        );

        if let Some(limits) = transform.tol_limits {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.label(
                egui::RichText::new("CLIPPING LIMITS")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(f_med),
            );
            for (index, band) in ["R", "G", "B"].into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(band)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(f_low),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.1} – {:.1}",
                                limits[index][0], limits[index][1]
                            ))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(f_high),
                        );
                    });
                });
            }
        }

        if !transform.warnings.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.label(
                egui::RichText::new("DIAGNOSTICS")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(diff_del),
            );
            for warning in &transform.warnings {
                ui.label(
                    egui::RichText::new(format!("⚠ {}", warning))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(diff_del),
                );
            }
        }
    }

    pub(crate) fn render_decorrelation_stretch_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        // Ensure stretched texture is loaded
        if self.dcs_state.dirty_texture || self.dcs_state.stretched_texture.is_none() {
            if let Some(ref dcs_rgba) = self.dcs_state.stretched_rgba {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [dcs_rgba.width() as usize, dcs_rgba.height() as usize],
                    dcs_rgba.as_raw(),
                );
                self.dcs_state.stretched_texture = Some(ctx.load_texture(
                    "decorrelation_stretch_preview_tex",
                    color_img,
                    egui::TextureOptions::LINEAR,
                ));
                self.dcs_state.dirty_texture = false;
            }
        }

        let base_c = hex_to_c32(&palette.harmonies.base);

        // If user is holding original (via Hold to Compare button), show full original
        if self.dcs_state.is_holding_original {
            if let Some(ref orig_tex) = self.texture {
                let mut full_mesh = egui::Mesh::with_texture(orig_tex.id());
                full_mesh.add_rect_with_uv(
                    img_rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.add(egui::Shape::mesh(full_mesh));

                let bounds = img_rect.intersect(painter.clip_rect());
                crate::ui_ext::CanvasBadge {
                    heading: "ORIGINAL (HOLDING)",
                    value: None,
                    accent: None,
                }
                .paint(
                    painter,
                    bounds.min + Vec2::splat(8.0),
                    egui::Align2::LEFT_TOP,
                    bounds,
                    (bounds.width() - 16.0).max(1.0),
                    palette,
                );
            }
            return;
        }

        let split = self.dcs_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // 1. Draw Left Side (Original / Before)
        if let Some(ref orig_tex) = self.texture {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut left_mesh = egui::Mesh::with_texture(orig_tex.id());
            left_mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(left_mesh));
        }

        // 2. Draw Right Side (Decorrelation Stretched / After)
        let dcs_tex_id = self
            .dcs_state
            .stretched_texture
            .as_ref()
            .map(|t| t.id())
            .or_else(|| self.texture.as_ref().map(|t| t.id()));
        if let Some(tex_id) = dcs_tex_id {
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut right_mesh = egui::Mesh::with_texture(tex_id);
            right_mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(right_mesh));
        }

        // 3. Draw Crisp Divider Line
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(3.0_f32, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(1.5_f32, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_radius = crate::ui_ext::paint_split_handle(painter, knob_center, palette);

        // 5. Draggable Handle Interaction
        let ptr_pos = response.hover_pos();
        let near_divider =
            crate::ui_ext::split_handle_hit_test(ptr_pos, img_rect, knob_center, knob_radius, 16.0);

        if near_divider || self.dcs_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            self.dcs_state.is_dragging_split = true;
        }

        if self.dcs_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let new_split = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    self.dcs_state.comparison_split = new_split;
                }
            }
            if response.drag_stopped() {
                self.dcs_state.is_dragging_split = false;
            }
        }

        // 6. Floating HUD Badges
        let bounds = img_rect.intersect(painter.clip_rect());
        let gap = 8.0;
        let max_width = ((bounds.width() - gap * 3.0) * 0.5).max(1.0);
        crate::ui_ext::CanvasBadge {
            heading: "ORIGINAL",
            value: None,
            accent: None,
        }
        .paint(
            painter,
            bounds.min + Vec2::splat(gap),
            egui::Align2::LEFT_TOP,
            bounds,
            max_width,
            palette,
        );
        let after_text = format!("DCS ({:.0} ms)", self.dcs_state.last_duration_ms);
        crate::ui_ext::CanvasBadge {
            heading: &after_text,
            value: None,
            accent: Some(base_c),
        }
        .paint(
            painter,
            Pos2::new(bounds.right() - gap, bounds.top() + gap),
            egui::Align2::RIGHT_TOP,
            bounds,
            max_width,
            palette,
        );
    }
}
