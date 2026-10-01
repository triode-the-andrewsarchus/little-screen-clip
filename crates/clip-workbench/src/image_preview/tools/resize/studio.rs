use super::*;

impl ImagePreviewState {
    pub(super) fn format_label_or_mime(format: TargetFormat) -> &'static str {
        match format {
            TargetFormat::Jpeg => "image/jpeg",
            TargetFormat::Png => "image/png",
        }
    }

    /// Extracts dominant colors from current_display_image into palette_state.

    pub(crate) fn render_resize_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.poll_optimizer();
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }

        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let success = hex_to_c32(&palette.diff_add);

        ui.label(egui::RichText::new("Resize").strong().color(accent));
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

        // Stable, equal-width mode selector at the top of the rail.
        let mode_button_width = ((ui.available_width() - 6.0) * 0.5).max(70.0);
        ui.horizontal(|ui| {
            let dimensions = crate::mode_tab_button(
                "Dimensions",
                self.resize_state.sub_tab == ResizeSubTab::Pixels,
                palette,
            );
            if ui
                .add_sized(
                    [mode_button_width, crate::ui_ext::CONTROL_HEIGHT],
                    dimensions,
                )
                .clicked()
            {
                self.resize_state.sub_tab = ResizeSubTab::Pixels;
                self.resize_state.sync_from_image(&self.current_rgba);
            }

            let file_size = crate::mode_tab_button(
                "File size",
                self.resize_state.sub_tab == ResizeSubTab::FileSize,
                palette,
            );
            if ui
                .add_sized(
                    [mode_button_width, crate::ui_ext::CONTROL_HEIGHT],
                    file_size,
                )
                .clicked()
            {
                self.resize_state.sub_tab = ResizeSubTab::FileSize;
            }
        });

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);

        match self.resize_state.sub_tab {
            ResizeSubTab::Pixels => {
                ui.label(
                    egui::RichText::new("OUTPUT DIMENSIONS")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);

                // Width and height stay together as one WxH control.
                ui.horizontal(|ui| {
                    let field_width = ((ui.available_width() - 24.0) * 0.5).max(56.0);

                    let mut width = self.resize_state.width_input.clone();
                    let width_response = ui.add_sized(
                        [field_width, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut width)
                            .desired_width(field_width)
                            .hint_text("Width")
                            .font(egui::TextStyle::Monospace),
                    );
                    if width_response.changed() {
                        self.resize_state.width_input = width.clone();
                        if let Ok(value) = width.trim().parse::<u32>() {
                            self.resize_state.set_width(value);
                        }
                    }

                    ui.label(egui::RichText::new("×").strong().color(muted));

                    let mut height = self.resize_state.height_input.clone();
                    let height_response = ui.add_sized(
                        [field_width, crate::ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut height)
                            .desired_width(field_width)
                            .hint_text("Height")
                            .font(egui::TextStyle::Monospace),
                    );
                    if height_response.changed() {
                        self.resize_state.height_input = height.clone();
                        if let Ok(value) = height.trim().parse::<u32>() {
                            self.resize_state.set_height(value);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("W")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("H")
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(muted),
                        );
                    });
                });

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                let lock_icon = if self.resize_state.lock_aspect {
                    Icon::Lock
                } else {
                    Icon::Unlock
                };
                let lock_label = if self.resize_state.lock_aspect {
                    "Aspect ratio locked"
                } else {
                    "Aspect ratio unlocked"
                };
                let lock_button = crate::ui_ext::mode_tab_button_icon(
                    Some(lock_icon),
                    lock_label,
                    self.resize_state.lock_aspect,
                    palette,
                );
                if ui
                    .add_sized(
                        [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                        lock_button,
                    )
                    .on_hover_text("Keep width and height proportional")
                    .clicked()
                {
                    self.resize_state.lock_aspect = !self.resize_state.lock_aspect;
                    if self.resize_state.lock_aspect && self.resize_state.target_height > 0 {
                        self.resize_state.aspect_ratio_base = self.resize_state.target_width as f32
                            / self.resize_state.target_height as f32;
                    }
                }

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.label(
                    egui::RichText::new("SCALE PRESET")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);
                let current = (self.current_rgba.width(), self.current_rgba.height());
                let presets = [
                    ("25%", 0.25_f32),
                    ("50%", 0.5),
                    ("75%", 0.75),
                    ("100%", 1.0),
                    ("150%", 1.5),
                    ("200%", 2.0),
                ];
                let labels = presets.map(|(label, _)| label);
                let button_width =
                    crate::ui_ext::wrapped_equal_button_width(ui, &labels, false, presets.len());
                ui.horizontal_wrapped(|ui| {
                    for (label, factor) in presets {
                        let button = crate::compact_toolbar_button(label, surface, palette);
                        if ui
                            .add_sized([button_width, crate::ui_ext::CONTROL_HEIGHT], button)
                            .clicked()
                        {
                            self.resize_state
                                .set_scale_preset(factor, current.0, current.1);
                        }
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.label(
                    egui::RichText::new("RESAMPLING")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                egui::ComboBox::from_id_source("resize_filter_combo")
                    .selected_text(self.resize_state.filter.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.resize_state.filter,
                            ResizeFilter::Lanczos3,
                            ResizeFilter::Lanczos3.label(),
                        );
                        ui.selectable_value(
                            &mut self.resize_state.filter,
                            ResizeFilter::Triangle,
                            ResizeFilter::Triangle.label(),
                        );
                        ui.selectable_value(
                            &mut self.resize_state.filter,
                            ResizeFilter::Nearest,
                            ResizeFilter::Nearest.label(),
                        );
                    });

                ui.add_space(14.0);
                if let Some(status) = &self.pixel_resize_job.status {
                    ui.label(
                        egui::RichText::new(status)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(muted),
                    );
                }
                if let Some(error) = &self.pixel_resize_job.error {
                    ui.label(
                        egui::RichText::new(error)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.diff_del)),
                    );
                }
                let apply = crate::action_button("Apply resize (Enter)", accent, palette);
                if ui
                    .add_enabled_ui(!self.pixel_resize_job.pending, |ui| {
                        ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
                    })
                    .inner
                    .on_hover_text("Resize the image to the dimensions above")
                    .clicked()
                {
                    self.apply_pixel_resize(
                        self.resize_state.target_width,
                        self.resize_state.target_height,
                        self.resize_state.filter,
                    );
                }
            }
            ResizeSubTab::FileSize => {
                ui.label(
                    egui::RichText::new("TARGET SIZE")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);

                let presets = [
                    ("250 KB", 250.0_f32, false),
                    ("500 KB", 500.0, false),
                    ("1 MB", 1.0, true),
                    ("2 MB", 2.0, true),
                ];
                let labels = presets.map(|(label, _, _)| label);
                let button_width =
                    crate::ui_ext::wrapped_equal_button_width(ui, &labels, false, presets.len());
                ui.horizontal_wrapped(|ui| {
                    for (label, value, megabytes) in presets {
                        let selected = (self.resize_state.target_size_val - value).abs() < 0.01
                            && self.resize_state.target_size_unit_mb == megabytes;
                        let fill = if selected { success } else { surface };
                        let button = crate::compact_toolbar_button(label, fill, palette);
                        if ui
                            .add_sized([button_width, crate::ui_ext::CONTROL_HEIGHT], button)
                            .clicked()
                        {
                            self.resize_state.target_size_val = value;
                            self.resize_state.target_size_unit_mb = megabytes;
                        }
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_SM);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Custom")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(text),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.resize_state.target_size_val)
                            .speed(0.1)
                            .clamp_range(0.01..=1000.0),
                    );
                    let unit = if self.resize_state.target_size_unit_mb {
                        "MB"
                    } else {
                        "KB"
                    };
                    if ui
                        .add(crate::compact_toolbar_button(unit, surface, palette))
                        .on_hover_text("Switch between KB and MB")
                        .clicked()
                    {
                        self.resize_state.target_size_unit_mb =
                            !self.resize_state.target_size_unit_mb;
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_MD);
                ui.label(
                    egui::RichText::new("OUTPUT FORMAT")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                egui::ComboBox::from_id_source("opt_format_combo")
                    .selected_text(self.resize_state.target_format.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.resize_state.target_format,
                            TargetFormat::Jpeg,
                            "JPEG — quality + resolution",
                        );
                        ui.selectable_value(
                            &mut self.resize_state.target_format,
                            TargetFormat::Png,
                            "PNG — lossless resolution",
                        );
                    });

                // Settings can change during this frame, after the top-level poll.
                self.poll_optimizer();

                ui.add_space(14.0);
                if self.resize_state.is_optimizing {
                    ui.add_enabled(
                        false,
                        crate::action_button("Optimizing…", surface, palette).min_size(Vec2::new(
                            ui.available_width(),
                            crate::ui_ext::CONTROL_HEIGHT,
                        )),
                    );
                } else {
                    let run = crate::action_button("Run optimizer", success, palette);
                    if ui
                        .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], run)
                        .on_hover_text("Fit quality and resolution to the target file size")
                        .clicked()
                    {
                        self.run_target_file_size_optimizer();
                    }
                }

                if self.resize_state.is_optimizing {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(accent));
                        ui.label(
                            egui::RichText::new(
                                self.optimizer_status
                                    .as_deref()
                                    .unwrap_or("Optimizing image file size…"),
                            )
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(accent),
                        );
                    });
                } else if let Some(status) = self.optimizer_status.as_deref() {
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new(status)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(accent),
                    );
                } else if self.optimized_result_is_current()
                    && let Some(result) = self.resize_state.last_result.clone()
                {
                    ui.add_space(crate::ui_ext::SPACE_MD);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new("OPTIMIZED RESULT")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(success),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} of {}",
                            format_bytes_size(result.achieved_bytes),
                            format_bytes_size(result.target_bytes)
                        ))
                        .strong()
                        .color(success),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{} × {} · {:.0}% scale · Q{}",
                            result.width,
                            result.height,
                            result.scale * 100.0,
                            result.quality
                        ))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(text),
                    );
                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                    let apply = crate::action_button("Apply to canvas", success, palette);
                    if ui
                        .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
                        .clicked()
                    {
                        self.apply_optimized_result();
                    }
                    let copy = crate::secondary_button("Copy to slot", surface, palette);
                    if ui
                        .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], copy)
                        .clicked()
                    {
                        self.copy_optimized_to_slot();
                    }
                }
            }
        }

        if !self.undo_stack.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            let undo = crate::secondary_button("Undo resize (Ctrl+Z)", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], undo)
                .clicked()
            {
                self.undo();
            }
        }
    }
}
