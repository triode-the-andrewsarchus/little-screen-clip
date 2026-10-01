use super::*;

impl ImagePreviewState {
    pub(crate) fn render_convert_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        if self.convert_state.is_running_optipng {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let border = hex_to_c32(&palette.colors.surface_recessed);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let success = hex_to_c32(&palette.diff_add);
        let warning = hex_to_c32(&palette.diff_mod);
        let mut changed = false;

        ui.label(
            egui::RichText::new("Convert")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Choose an output format and compare it on the canvas.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), crate::ui_ext::CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                if self.convert_state.render_error_input_stamp.as_ref() == Some(&self.input_stamp())
                    && let Some(error) = &self.convert_state.render_error
                {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
                        .truncate(true),
                    );
                } else if let Some(status) = &self.convert_state.render_queue_status {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
                        .truncate(true),
                    );
                } else if let Some(status) = &self.convert_state.optipng_queue_status {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(status)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(warning),
                        )
                        .truncate(true),
                    );
                } else if self.convert_state.is_running_optipng {
                    ui.add(egui::Spinner::new().size(13.0).color(accent));
                    ui.label(
                        egui::RichText::new("Optimizing PNG…")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(accent),
                    );
                } else if self.convert_state.is_pending() {
                    ui.add(egui::Spinner::new().size(13.0).color(accent));
                    ui.label(
                        egui::RichText::new(if self.convert_state.is_encoding {
                            "Encoding preview…"
                        } else {
                            "Waiting for changes…"
                        })
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(accent),
                    );
                }
            },
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OUTPUT FORMAT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        let formats = [
            (ConvertTargetFormat::Png, "PNG"),
            (ConvertTargetFormat::Jpeg, "JPEG"),
            (ConvertTargetFormat::Webp, "WebP"),
        ];
        let format_width =
            crate::ui_ext::wrapped_equal_button_width(ui, &["PNG", "JPEG", "WebP"], false, 3);
        ui.horizontal_wrapped(|ui| {
            for (format, label) in formats {
                let active = self.convert_state.target_format == format;
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
                    .add_sized([format_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .clicked()
                    && !active
                {
                    self.convert_state.target_format = format;
                    match format {
                        ConvertTargetFormat::Png => self.convert_state.lossless = true,
                        ConvertTargetFormat::Jpeg => self.convert_state.lossless = false,
                        ConvertTargetFormat::Webp => {}
                    }
                    changed = true;
                }
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("ENCODING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        match self.convert_state.target_format {
            ConvertTargetFormat::Png => {
                ui.label(
                    egui::RichText::new("Lossless PNG")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Preserves every pixel and transparency.")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Jpeg => {
                ui.label(
                    egui::RichText::new("Lossy JPEG")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.label(
                    egui::RichText::new("Small files; transparency is flattened.")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            }
            ConvertTargetFormat::Webp => {
                let mut lossless = self.convert_state.lossless;
                if ui
                    .checkbox(
                        &mut lossless,
                        egui::RichText::new("Lossless WebP")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .color(text),
                    )
                    .on_hover_text("Preserve every pixel using WebP lossless encoding")
                    .changed()
                {
                    self.convert_state.lossless = lossless;
                    changed = true;
                }
                ui.label(
                    egui::RichText::new(if lossless {
                        "Preserves every pixel and transparency."
                    } else {
                        "Compresses for a smaller output file."
                    })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
                );
            }
        }

        if !self.convert_state.lossless {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Quality")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}%", self.convert_state.quality))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let mut quality = self.convert_state.quality;
            ui.spacing_mut().slider_width = ui.available_width();
            if ui
                .add(egui::Slider::new(&mut quality, 1..=100).show_value(false))
                .on_hover_text("Higher quality produces a larger file")
                .changed()
            {
                self.convert_state.quality = quality;
                changed = true;
            }
        }

        if changed {
            self.schedule_conversion();
        }

        if self.convert_state.target_format == ConvertTargetFormat::Png {
            ui.add_space(10.0);
            let optimize_label = if self.convert_state.is_running_optipng {
                "Optimizing…"
            } else {
                "Run OptiPNG"
            };
            let optimize = crate::secondary_button(optimize_label, warning, palette);
            if ui
                .add_enabled_ui(
                    !self.convert_state.is_running_optipng
                        && self.convert_state.optipng_retry_at.is_none()
                        && self.convert_state.output_matches(&self.input_stamp()),
                    |ui| {
                        ui.add_sized(
                            [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                            optimize,
                        )
                    },
                )
                .inner
                .on_hover_text("Losslessly reduce the encoded PNG file size")
                .clicked()
            {
                self.run_optipng_conversion();
            }
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("FILE SIZE")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        let original_size = self.file_size_bytes;
        let converted_size = if self.convert_state.output_matches(&self.input_stamp()) {
            self.convert_state.encoded_bytes.len()
        } else {
            0
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Original")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format_bytes_size(original_size))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(text),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Converted")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(if converted_size == 0 {
                        "—".to_string()
                    } else {
                        format_bytes_size(converted_size)
                    })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .monospace()
                    .color(text),
                );
            });
        });

        if converted_size > 0 && original_size > 0 {
            let delta =
                (converted_size as f64 - original_size as f64) / original_size as f64 * 100.0;
            let delta_color = if delta <= 0.0 { success } else { warning };
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Difference")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:+.1}%", delta))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .strong()
                            .monospace()
                            .color(delta_color),
                    );
                });
            });
        }

        if self.convert_state.output_matches(&self.input_stamp())
            && let Some((before, after)) = self.convert_state.optipng_savings
        {
            let saved = before.saturating_sub(after);
            ui.label(
                egui::RichText::new(format!("OptiPNG saved {}", format_bytes_size(saved)))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(success),
            );
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("Drag the canvas divider to compare before and after.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .italics()
                .color(muted),
        );

        ui.add_space(14.0);
        let output_ready = self.convert_state.output_matches(&self.input_stamp());
        let extension = self.convert_state.target_format.extension();
        let save = crate::action_button(format!("Save as .{}", extension), accent, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], save)
            })
            .inner
            .on_hover_text(format!("Export the converted image as .{}", extension))
            .clicked()
        {
            self.open_save_dialog_with_ext(extension);
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let apply = crate::secondary_button("Apply to canvas (Enter)", success, palette);
        if ui
            .add_enabled_ui(output_ready, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
            })
            .inner
            .on_hover_text("Use the converted image as the active editor canvas")
            .clicked()
        {
            self.apply_converted_to_canvas();
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("COPY TO SLOT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let slot_width =
            crate::ui_ext::wrapped_equal_button_width(ui, &["Left hand", "Right hand"], false, 2);
        ui.horizontal_wrapped(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], left)
                .on_hover_text("Copy the converted image to the Left Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Left);
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], right)
                .on_hover_text("Copy the converted image to the Right Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Right);
            }
        });

        // A control can become dirty after the top-level polling pass for this
        // frame. Ensure the debounce timer and worker continue without needing
        // another pointer event to wake the UI.
        let retry_at = self
            .convert_state
            .render_retry_at
            .into_iter()
            .chain(self.convert_state.optipng_retry_at)
            .min();
        if let Some(retry_at) = retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        } else if self.convert_state.is_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub(crate) fn render_convert_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        // Ensure converted texture is loaded
        if self.convert_state.output_matches(&self.input_stamp())
            && (self.convert_state.dirty_texture || self.convert_state.converted_texture.is_none())
        {
            if let Some(ref conv_rgba) = self.convert_state.converted_rgba {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [conv_rgba.width() as usize, conv_rgba.height() as usize],
                    conv_rgba.as_raw(),
                );
                self.convert_state.converted_texture = Some(ctx.load_texture(
                    "converted_preview_tex",
                    color_img,
                    image_texture_options(conv_rgba),
                ));
                self.convert_state.dirty_texture = false;
            }
        }

        let split = self.convert_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // 1. Draw Left Side (Original / Before)
        if let Some(ref orig_tex) = self.texture {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut left_mesh = egui::Mesh::with_texture(orig_tex.id());
            left_mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(left_mesh));
        }

        // 2. Draw Right Side (Converted / After)
        let conv_tex_id = if self.convert_state.output_matches(&self.input_stamp()) {
            self.convert_state
                .converted_texture
                .as_ref()
                .map(|t| t.id())
                .or_else(|| self.texture.as_ref().map(|t| t.id()))
        } else {
            self.texture.as_ref().map(|t| t.id())
        };
        if let Some(tex_id) = conv_tex_id {
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
        let base_c = hex_to_c32(&palette.harmonies.base);
        let knob_radius = crate::ui_ext::paint_split_handle(painter, knob_center, palette);

        // 5. Draggable Handle Interaction
        let ptr_pos = response.hover_pos();
        let near_divider =
            crate::ui_ext::split_handle_hit_test(ptr_pos, img_rect, knob_center, knob_radius, 16.0);

        if near_divider || self.convert_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            self.convert_state.is_dragging_split = true;
        }

        if self.convert_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let new_split = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    self.convert_state.comparison_split = new_split;
                }
            }
            if response.drag_stopped() {
                self.convert_state.is_dragging_split = false;
            }
        }

        // 6. Squoosh-Style Floating HUD Badges
        let orig_size_str = format_bytes_size(self.file_size_bytes);
        let conv_size_str = if self.convert_state.output_matches(&self.input_stamp()) {
            format_bytes_size(self.convert_state.encoded_bytes.len())
        } else {
            "—".to_string()
        };
        let orig_fmt = self.mime.replace("image/", "").to_uppercase();
        let conv_fmt = self.convert_state.target_format.extension().to_uppercase();

        let bounds = img_rect.intersect(painter.clip_rect());
        let gap = 8.0;
        let max_width = ((bounds.width() - gap * 3.0) * 0.5).max(1.0);
        let before_value = format!("{} · {}", orig_fmt, orig_size_str);
        let after_value = format!("{} · {}", conv_fmt, conv_size_str);
        crate::ui_ext::CanvasBadge {
            heading: "BEFORE",
            value: Some(&before_value),
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
        crate::ui_ext::CanvasBadge {
            heading: "AFTER",
            value: Some(&after_value),
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
