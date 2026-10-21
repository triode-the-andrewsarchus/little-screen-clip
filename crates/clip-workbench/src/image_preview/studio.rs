use super::*;

impl ImagePreviewState {
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        self.poll_save_job();
        if self.save_snapshot_pending() && self.render_save_progress(ui, ctx, close) {
            return;
        }
        self.poll_copy_job();
        if self.copy_snapshot_pending() {
            self.render_copy_progress(ui, ctx, close);
            return;
        }
        if self.copy_delivery_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        self.rendering_frame = true;

        // Upload GPU texture if dirty or not loaded
        if self.texture.is_none() || self.dirty_texture {
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [
                    self.current_rgba.width() as usize,
                    self.current_rgba.height() as usize,
                ],
                self.current_rgba.as_raw(),
            );
            self.texture = Some(ctx.load_texture(
                "image_preview_tex",
                color_image,
                image_texture_options(&self.current_rgba),
            ));
            self.dirty_texture = false;
        }

        // Keep ownership for this whole frame, even if Save or Cancel closes the dialog.
        let save_modal_active = self.show_fallback_save_modal;
        if save_modal_active {
            egui::Area::new(egui::Id::new("image_save_backdrop"))
                .order(egui::Order::Middle)
                .fixed_pos(ctx.screen_rect().min)
                .show(ctx, |ui| {
                    let rect = ctx.screen_rect();
                    ui.allocate_rect(rect, egui::Sense::click());
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        hex_to_c32_alpha(&palette.colors.canvas, 180),
                    );
                });
            egui::Window::new("Save Image As")
                .collapsible(false)
                .resizable(false)
                .movable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    ui.label(egui::RichText::new("Enter destination file path:").strong());
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    let path_field = ui.add_sized(
                        [
                            ui.available_width().clamp(260.0, 440.0),
                            crate::ui_ext::CONTROL_HEIGHT,
                        ],
                        egui::TextEdit::singleline(&mut self.fallback_save_path),
                    );
                    if self.fallback_save_needs_focus {
                        path_field.request_focus();
                        self.fallback_save_needs_focus = false;
                    }
                    if self.fallback_save_confirm_overwrite {
                        ui.label(
                            egui::RichText::new("A file at this path already exists. Replace it?")
                                .color(hex_to_c32(&palette.harmonies.warm_attention)),
                        );
                    }
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.horizontal(|ui| {
                        let save_label = if self.fallback_save_confirm_overwrite {
                            "Replace"
                        } else {
                            "Save"
                        };
                        let save_color = if self.fallback_save_confirm_overwrite {
                            hex_to_c32(&palette.harmonies.warm_attention)
                        } else {
                            hex_to_c32(&palette.diff_add)
                        };
                        if ui
                            .add(crate::ui_ext::action_button_icon(
                                Some(Icon::Save),
                                save_label,
                                save_color,
                                palette,
                            ))
                            .clicked()
                        {
                            let p = PathBuf::from(&self.fallback_save_path);
                            if self.fallback_save_confirm_overwrite {
                                self.save_image_to_path(&p);
                                self.show_fallback_save_modal = false;
                                self.fallback_save_needs_focus = false;
                                self.fallback_save_confirm_overwrite = false;
                            } else if p.exists() {
                                self.fallback_save_confirm_overwrite = true;
                            } else {
                                self.save_image_to_path(&p);
                                self.show_fallback_save_modal = false;
                                self.fallback_save_needs_focus = false;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            if self.fallback_save_confirm_overwrite {
                                self.fallback_save_confirm_overwrite = false;
                            } else {
                                self.show_fallback_save_modal = false;
                                self.fallback_save_needs_focus = false;
                            }
                        }
                    });
                });
        }

        if self.render_save_progress(ui, ctx, close) {
            return;
        }

        if save_modal_active && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_fallback_save_modal = false;
            self.fallback_save_needs_focus = false;
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        }
        let image_shortcuts_enabled = !save_modal_active && !ctx.wants_keyboard_input();

        // Global hotkey handling
        if image_shortcuts_enabled && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.ocr_state.in_progress {
                self.cancel_ocr();
            } else if self.active_mode == ImageToolMode::Crop
                && self.crop_state.normalized_rect
                    != Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0))
            {
                self.crop_state.normalized_rect =
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
                self.crop_state.ratio = CropRatio::Free;
            } else if self.active_mode == ImageToolMode::Deskew
                && self.deskew_state.is_clicking_points
            {
                self.deskew_state.cancel_clicking_points();
                self.show_toast("Cancelled 4-point placement");
            } else if self.active_mode == ImageToolMode::Redact
                && self.redaction_state.selected_idx.is_some()
            {
                self.redaction_state.selected_idx = None;
            } else if self.active_mode == ImageToolMode::Redact
                && !self.redaction_state.boxes.is_empty()
            {
                self.clear_all_redactions();
            } else if self.active_mode == ImageToolMode::Markup
                && self.markup_state.editing_text_idx.is_some()
            {
                self.markup_state.editing_text_idx = None;
            } else if self.active_mode == ImageToolMode::Markup
                && self.markup_state.selected_idx.is_some()
            {
                self.markup_state.selected_idx = None;
            } else {
                if self.ocr_state.in_progress {
                    self.cancel_ocr();
                }
                *close = true;
                return;
            }
        }
        if image_shortcuts_enabled
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z))
        {
            if ui.input(|i| i.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }
        if image_shortcuts_enabled
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Y))
        {
            self.redo();
        }
        let enter_pressed = !save_modal_active && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let enter_from_field = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        if enter_pressed && (image_shortcuts_enabled || enter_from_field) {
            match self.active_mode {
                ImageToolMode::Crop if image_shortcuts_enabled => self.apply_crop(),
                ImageToolMode::Deskew if image_shortcuts_enabled => self.apply_deskew(),
                ImageToolMode::Redact if image_shortcuts_enabled => {
                    if self.redaction_state.tool_type == RedactToolType::LamaInpaint {
                        self.run_lama_inpaint();
                    } else {
                        self.apply_redactions();
                    }
                }
                ImageToolMode::Markup
                    if image_shortcuts_enabled && self.markup_state.editing_text_idx.is_none() =>
                {
                    self.apply_flatten_markup();
                }
                ImageToolMode::Resize
                    if image_shortcuts_enabled
                        && self.resize_state.sub_tab == ResizeSubTab::Pixels =>
                {
                    self.apply_pixel_resize(
                        self.resize_state.target_width,
                        self.resize_state.target_height,
                        self.resize_state.filter,
                    );
                }
                ImageToolMode::RemoveBg => self.run_background_removal(),
                ImageToolMode::DepthMap if image_shortcuts_enabled => self.run_depth_estimation(),
                ImageToolMode::Convert
                    if image_shortcuts_enabled
                        && self.convert_state.output_matches(&self.input_stamp()) =>
                {
                    self.apply_converted_to_canvas();
                }
                ImageToolMode::Ocr => self.run_ocr(),
                ImageToolMode::Barcode if image_shortcuts_enabled => self.run_barcode_scan(),
                ImageToolMode::DecorrelationStretch if image_shortcuts_enabled => {
                    self.apply_decorrelation_stretch();
                }
                _ => {}
            }
        }
        if image_shortcuts_enabled {
            if self.active_mode == ImageToolMode::Redact {
                let del_pressed = ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                }) || ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                });
                if del_pressed {
                    if self.redaction_state.tool_type == RedactToolType::LamaInpaint {
                        self.clear_inpaint_mask();
                    } else {
                        self.delete_selected_redaction();
                    }
                }

                let plus_pressed = ui.input(|i| {
                    i.key_pressed(egui::Key::Plus)
                        || i.key_pressed(egui::Key::Equals)
                        || i.events.iter().any(|e| match e {
                            egui::Event::Key {
                                key: egui::Key::Plus,
                                pressed: true,
                                ..
                            }
                            | egui::Event::Key {
                                key: egui::Key::Equals,
                                pressed: true,
                                ..
                            } => true,
                            egui::Event::Text(t) => t == "+" || t == "=",
                            _ => false,
                        })
                });
                let minus_pressed = ui.input(|i| {
                    i.key_pressed(egui::Key::Minus)
                        || i.events.iter().any(|e| match e {
                            egui::Event::Key {
                                key: egui::Key::Minus,
                                pressed: true,
                                ..
                            } => true,
                            egui::Event::Text(t) => t == "-",
                            _ => false,
                        })
                });

                if plus_pressed {
                    self.adjust_brush_radius(4.0);
                } else if minus_pressed {
                    self.adjust_brush_radius(-4.0);
                }
            }
            if self.active_mode == ImageToolMode::Markup
                && self.markup_state.editing_text_idx.is_none()
            {
                let del_pressed = ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                }) || ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                });
                if del_pressed {
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            self.markup_state.items.remove(sel);
                            self.markup_state.selected_idx = None;
                            self.markup_state.sync_next_badge_number();
                            self.show_toast(crate::ui_ext::StatusMessage::new(
                                crate::icons::Icon::Trash,
                                "Markup item deleted",
                            ));
                        }
                    }
                }
            }
            if self.active_mode == ImageToolMode::Markup
                && self.markup_state.editing_text_idx.is_none()
            {
                let no_modifiers = ui.input(|i| {
                    !i.modifiers.command
                        && !i.modifiers.ctrl
                        && !i.modifiers.alt
                        && !i.modifiers.mac_cmd
                });
                if no_modifiers {
                    if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
                        self.markup_state.active_tool = MarkupToolType::Select;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num2)) {
                        self.markup_state.active_tool = MarkupToolType::Arrow;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num3)) {
                        self.markup_state.active_tool = MarkupToolType::Circle;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num4)) {
                        self.markup_state.active_tool = MarkupToolType::Rectangle;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num5)) {
                        self.markup_state.active_tool = MarkupToolType::Line;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num6)) {
                        self.markup_state.active_tool = MarkupToolType::Highlighter;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num7)) {
                        self.markup_state.active_tool = MarkupToolType::StepBadge;
                        self.markup_state.editing_text_idx = None;
                    } else if ui.input(|i| i.key_pressed(egui::Key::Num8)) {
                        self.markup_state.active_tool = MarkupToolType::Text;
                    }
                }
            } else {
                if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
                    self.zoom = 1.0;
                    self.pan = Vec2::ZERO;
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Num0)) {
                self.fit_to_view(self.viewport_size);
            }
            if ui.input(|i| i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus)) {
                self.zoom = (self.zoom * 1.25).clamp(0.05, self.maximum_zoom(self.viewport_size));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.zoom = (self.zoom / 1.25).clamp(0.05, self.maximum_zoom(self.viewport_size));
            }

            // Unmodified single-key tool selection shortcuts
            let no_modifiers = ui.input(|i| {
                !i.modifiers.command
                    && !i.modifiers.ctrl
                    && !i.modifiers.alt
                    && !i.modifiers.mac_cmd
            });
            if no_modifiers {
                if ui.input(|i| i.key_pressed(egui::Key::V)) {
                    self.set_tool_mode(ImageToolMode::View);
                } else if ui.input(|i| i.key_pressed(egui::Key::C)) {
                    self.set_tool_mode(ImageToolMode::Crop);
                } else if ui.input(|i| i.key_pressed(egui::Key::K)) {
                    self.set_tool_mode(ImageToolMode::Deskew);
                } else if ui.input(|i| i.key_pressed(egui::Key::F)) {
                    self.set_tool_mode(ImageToolMode::Transform);
                } else if ui.input(|i| i.key_pressed(egui::Key::X)) {
                    self.set_tool_mode(ImageToolMode::Redact);
                } else if ui.input(|i| i.key_pressed(egui::Key::M)) {
                    self.set_tool_mode(ImageToolMode::Markup);
                } else if ui.input(|i| i.key_pressed(egui::Key::P)) {
                    self.set_tool_mode(ImageToolMode::PickColor);
                } else if ui.input(|i| i.key_pressed(egui::Key::S)) {
                    self.set_tool_mode(ImageToolMode::Resize);
                } else if ui.input(|i| i.key_pressed(egui::Key::T)) {
                    self.set_tool_mode(ImageToolMode::Convert);
                } else if ui.input(|i| i.key_pressed(egui::Key::O)) {
                    self.set_tool_mode(ImageToolMode::Ocr);
                } else if ui.input(|i| i.key_pressed(egui::Key::B)) {
                    self.set_tool_mode(ImageToolMode::Barcode);
                } else if ui.input(|i| i.key_pressed(egui::Key::R)) {
                    self.set_tool_mode(ImageToolMode::RemoveBg);
                } else if ui.input(|i| i.key_pressed(egui::Key::D)) {
                    self.set_tool_mode(ImageToolMode::DepthMap);
                } else if ui.input(|i| i.key_pressed(egui::Key::L)) {
                    self.set_tool_mode(ImageToolMode::FilmSim);
                }
            }
        }

        // Poll background file size optimizer if running
        self.poll_palette_extraction(ctx);

        self.poll_decorrelation_stretch(ctx, BackgroundJobs::global());
        if self.active_mode == ImageToolMode::DecorrelationStretch {
            if let Some(retry_at) = self.dcs_state.retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else if self.dcs_state.is_computing {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }

        // Poll background file size optimizer if running
        self.poll_optimizer();
        self.poll_pixel_resize(ctx, BackgroundJobs::global());
        if self.pixel_resize_job.pending {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        if let Some(retry_at) = self.pixel_resize_job.retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if self.resize_state.is_optimizing {
            ctx.request_repaint();
        }
        if let Some(retry_at) = self.optimizer_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }

        // Poll background AI background removal if running
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            if let Some(retry_at) = self.bg_removal_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        }

        // Poll background LaMa AI inpainting if running
        self.poll_lama_inpaint();
        if self.redaction_state.inpaint_in_progress {
            ctx.request_repaint();
        }

        // Poll Segment Anything point-to-select worker if active
        if self.active_mode == ImageToolMode::Redact
            && self.redaction_state.tool_type == RedactToolType::LamaInpaint
            && self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam
        {
            self.poll_sam(ctx);
        }

        // Poll format conversion and debounce costly full-resolution encodes.
        self.poll_conversion();
        if self.active_mode == ImageToolMode::Convert {
            if let Some(dirty_at) = self.convert_state.params_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(180)
                    && !self.convert_state.is_encoding
                {
                    self.run_conversion_worker();
                }
            }
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

        // Poll film sim background render + debounce
        self.poll_film_sim();
        if self.active_mode == ImageToolMode::FilmSim {
            if self
                .film_sim_state
                .render_retry_at
                .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
                && !self.film_sim_state.is_rendering
            {
                self.film_sim_state.render_retry_at = None;
                self.run_film_sim_render();
            }
            if let Some(retry_at) = self.film_sim_state.render_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            }
            // Debounce: fire render 400ms after last parameter change
            if let Some(dirty_at) = self.film_sim_state.params_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(400)
                    && !self.film_sim_state.is_rendering
                {
                    self.film_sim_state.params_dirty_at = None;
                    self.run_film_sim_render();
                }
            }
            // Debounce thumbnail batch: fire 500ms after last parameter change
            if let Some(dirty_at) = self.film_sim_state.thumbnail_dirty_at {
                if dirty_at.elapsed() >= std::time::Duration::from_millis(500)
                    && !self.film_sim_state.is_generating_thumbnails
                {
                    self.film_sim_state.thumbnail_dirty_at = None;
                    self.run_thumbnail_batch();
                }
            }
            self.retry_thumbnail_batch_if_due(BackgroundJobs::global());
            if let Some(retry_at) = self.film_sim_state.thumbnail_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            }
            if self.film_sim_state.is_rendering || self.film_sim_state.is_generating_thumbnails {
                ctx.request_repaint();
            }
        }

        // Poll creative fx background render + debounce
        self.poll_creative_fx();
        if let Some(retry_at) = self.creative_fx_state.thumbnail_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if let Some(retry_at) = self.creative_fx_state.depth_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if let Some(retry_at) = self.creative_fx_state.alpha_retry_at {
            ctx.request_repaint_after(
                retry_at.saturating_duration_since(std::time::Instant::now()),
            );
        }
        if self.active_mode == ImageToolMode::CreativeFx {
            if let Some(retry_at) = self.creative_fx_state.render_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            }
            if let Some(dirty_at) = self.creative_fx_state.params_dirty_at {
                let debounce = std::time::Duration::from_millis(150);
                if dirty_at.elapsed() >= debounce && !self.creative_fx_state.is_rendering {
                    self.creative_fx_state.params_dirty_at = None;
                    self.run_creative_fx_render();
                } else {
                    ctx.request_repaint_after(debounce.saturating_sub(dirty_at.elapsed()));
                }
            }
            if self.creative_fx_state.is_rendering
                || self.creative_fx_state.is_generating_thumbnails
            {
                ctx.request_repaint();
            }
        }

        // Copy to clipboard slot (Ctrl+C / Cmd+C) at any point
        if image_shortcuts_enabled {
            let copy_requested = ui.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                    || (i.key_pressed(egui::Key::C)
                        && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
            }) || ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
            });

            if copy_requested {
                self.copy_to_clipboard_slot();
            }

            // Save As shortcut (Ctrl+S / Cmd+S)
            let save_requested = ui.input(|i| {
                i.key_pressed(egui::Key::S)
                    && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd)
            }) || ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                    || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
            });

            if save_requested {
                self.open_save_dialog();
            }
        }

        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.canvas))
            .show(ui, |ui| {
                let toast = self.status_toast.clone();
                let mut shell = self.create_studio_shell(palette, close);
                egui::TopBottomPanel::bottom(ui.make_persistent_id("image_studio_footer"))
                    .frame(egui::Frame::none())
                    .resizable(false)
                    .show_inside(ui, |ui| {
                        shell.render_footer_with_center(ui, &toast, now, 184.0, |ui| {
                            self.render_zoom_controls(ui)
                        });
                    });

                // 1. TOP HEADER & ESCAPE HATCHES
                self.render_header(ui, ctx, palette, &mut shell);

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 2. TOOLBAR (Mode selection)
                self.render_toolbar(ui, palette);

                ui.add_space(crate::ui_ext::SPACE_XS);

                // 3. MAIN INTERACTIVE VIEWPORT
                self.render_active_tool_workspace(ui, ctx, palette);
            });
        self.rendering_frame = false;
        if let Some(hand) = self.copy_requested.take() {
            self.start_copy_to_hand_slot(hand, little_clip_core::BackgroundJobs::global());
        }
    }

    pub(super) fn create_studio_shell<'a>(
        &self,
        palette: &'a AppPalette,
        close: &'a mut bool,
    ) -> crate::shell::StudioShell<'a> {
        let aspect_str = format_aspect_ratio(self.current_rgba.width(), self.current_rgba.height());
        let dim_text = format!(
            "{}×{} ({})",
            self.current_rgba.width(),
            self.current_rgba.height(),
            aspect_str
        );
        let fmt_str = if self.is_svg && self.undo_stack.is_empty() {
            "SVG (VECTOR)".to_string()
        } else {
            self.mime.replace("image/", "").to_uppercase()
        };
        let size_str = format_bytes_size(self.file_size_bytes);

        let is_primary_svg = self.is_svg && self.undo_stack.is_empty();
        let is_primary_raster = !is_primary_svg;

        crate::shell::StudioShell::new(
            format!("IMAGE STUDIO — {}", self.title),
            Icon::Image,
            palette,
            close,
        )
        .with_compact_title(self.title.clone())
        .with_badge(dim_text, Some(hex_to_c32(&palette.harmonies.split_comp_1)))
        .with_badge(fmt_str, Some(hex_to_c32(&palette.diff_add)))
        .with_badge(size_str, Some(hex_to_c32(&palette.diff_mod)))
        .with_escape_hatch(
            image_editor_hatch("Krita", "krita", &self.external_editor)
                .with_primary(is_primary_raster)
                .with_icon(Icon::Palette),
        )
        .with_escape_hatch(
            image_editor_hatch("Inkscape", "inkscape", &self.svg_editor)
                .with_primary(is_primary_svg)
                .with_icon(Icon::Pencil),
        )
        .with_escape_hatch(
            image_editor_hatch("Nomacs", "nomacs", &self.viewer)
                .with_primary(false)
                .with_icon(Icon::Eye),
        )
        .with_footer_hint_icon(crate::icons::Icon::Mouse)
        .with_footer_hints(image_studio_footer_hint(self.active_mode))
    }

    pub(super) fn render_header(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        shell: &mut crate::shell::StudioShell<'_>,
    ) {
        let mut clicked_save_as = false;
        let mut clicked_copy = false;

        let clicked_hatch = shell.render_header(ui, |ui| {
            // The shared header lays actions out from the right edge inward.
            // Draw Copy first so Save As appears immediately after Close.
            let copy_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Copy),
                "Copy (Ctrl+C)",
                hex_to_c32(&palette.harmonies.base),
                palette,
            )
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)));
            if ui
                .add(copy_btn)
                .on_hover_text(
                    "Copy current image to Left Hand clipboard slot & OS clipboard (Ctrl+C)",
                )
                .clicked()
            {
                clicked_copy = true;
            }

            ui.add_space(crate::ui_ext::SPACE_XS);

            // Save As button
            let save_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Save),
                "Save As...",
                hex_to_c32(&palette.diff_add),
                palette,
            )
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.diff_add)));
            if ui
                .add(save_btn)
                .on_hover_text("Save current image to file (Ctrl+S)")
                .clicked()
            {
                clicked_save_as = true;
            }
        });

        if clicked_save_as {
            self.open_save_dialog();
        }
        if clicked_copy {
            self.copy_to_clipboard_slot();
        }

        if let Some(hatch) = clicked_hatch.and_then(|idx| shell.escape_hatches.get(idx)) {
            self.launch_external_app(&hatch.command, ctx, shell.close);
        }
    }

    pub fn set_tool_mode(&mut self, mode: ImageToolMode) {
        let target_mode = if mode == ImageToolMode::Palette {
            ImageToolMode::PickColor
        } else {
            mode
        };
        if self.active_mode == target_mode {
            return;
        }
        let prev_mode = self.active_mode;
        self.active_mode = target_mode;
        if prev_mode == ImageToolMode::FilmSim && target_mode != ImageToolMode::FilmSim {
            self.film_sim_state.cancel_render();
            self.film_sim_state.cancel_thumbnail_batch();
        }
        if prev_mode == ImageToolMode::CreativeFx && target_mode != ImageToolMode::CreativeFx {
            self.creative_fx_state.cancel_render();
            self.cancel_creative_fx_thumbnail_batch();
        }
        if prev_mode == ImageToolMode::DecorrelationStretch
            && target_mode != ImageToolMode::DecorrelationStretch
        {
            self.cancel_decorrelation_stretch();
        }
        if prev_mode == ImageToolMode::Resize && target_mode != ImageToolMode::Resize {
            self.cancel_pixel_resize();
        }
        if prev_mode == ImageToolMode::Redact && target_mode != ImageToolMode::Redact {
            self.sam_state.reset_hover();
        }
        if target_mode == ImageToolMode::Redact
            && self.redaction_state.tool_type == RedactToolType::LamaInpaint
            && self.redaction_state.inpaint_mode == InpaintSelectionMode::PointSam
        {
            self.ensure_sam_worker();
            self.sync_sam_image();
        }
        if prev_mode == ImageToolMode::Ocr
            && target_mode != ImageToolMode::Ocr
            && self.ocr_state.in_progress
        {
            self.cancel_ocr();
        }
        if prev_mode == ImageToolMode::Barcode && target_mode != ImageToolMode::Barcode {
            self.cancel_barcode_scan();
        }
        let leaving_active_image_tool = matches!(
            (prev_mode, self.bg_removal_operation),
            (
                ImageToolMode::RemoveBg,
                BgRemovalOperation::BackgroundRemoval
            ) | (ImageToolMode::DepthMap, BgRemovalOperation::DepthMap)
        );
        if leaving_active_image_tool && target_mode != prev_mode && self.bg_removal_in_progress {
            self.cancel_image_tool_process();
        }
        match target_mode {
            ImageToolMode::View
            | ImageToolMode::Crop
            | ImageToolMode::Deskew
            | ImageToolMode::Transform
            | ImageToolMode::Redact
            | ImageToolMode::Markup
            | ImageToolMode::Ocr
            | ImageToolMode::RemoveBg
            | ImageToolMode::DepthMap => {}
            ImageToolMode::PickColor | ImageToolMode::Palette => {
                self.invalidate_stale_palette_result();
                if !self.palette_state.is_extracted
                    || self.palette_state.extracted_colors.is_empty()
                {
                    self.run_palette_extraction();
                }
            }
            ImageToolMode::Resize => {
                self.resize_state.sync_from_image(&self.current_rgba);
            }
            ImageToolMode::Convert => {
                if !self.convert_state.output_matches(&self.input_stamp())
                    && !self.convert_state.is_encoding
                    && self.convert_state.params_dirty_at.is_none()
                {
                    self.schedule_conversion();
                }
            }
            ImageToolMode::DecorrelationStretch => {
                if !self.dcs_preview_is_current() && !self.dcs_state.is_computing {
                    self.update_decorrelation_stretch();
                }
            }
            ImageToolMode::Barcode => {
                // Decoding is explicitly user initiated. Entering this mode must
                // remain instant even for a multi-megapixel image.
            }
            ImageToolMode::FilmSim => {
                self.film_sim_state.auto_fitted = false;
                // Rerender when either the image or Film Look parameters changed.
                if !self.film_sim_preview_is_current() && !self.film_sim_state.is_rendering {
                    self.run_film_sim_render();
                }
                // Trigger thumbnail batch if not generated yet.
                if self.film_sim_state.stock_thumbnails.len()
                    + self.film_sim_state.pending_thumbnails.len()
                    < tools::all_film_stocks().len()
                    && !self.film_sim_state.is_generating_thumbnails
                {
                    self.run_thumbnail_batch();
                }
            }
            ImageToolMode::CreativeFx => {
                self.creative_fx_state
                    .select_fx(CreativeFxType::BasicControls);
                self.creative_fx_state.auto_fitted = false;
                // Re-entering after an image edit must not retain a hidden stale preview.
                if !self.creative_fx_preview_is_current() && !self.creative_fx_state.is_rendering {
                    self.run_creative_fx_render();
                }
                // Trigger thumbnail batch if not generated yet.
                if self.creative_fx_state.fx_thumbnails.len()
                    + self.creative_fx_state.pending_thumbnails.len()
                    < CreativeFxType::all().len()
                    && !self.creative_fx_state.is_generating_thumbnails
                {
                    self.run_creative_fx_thumbnail_batch();
                }
            }
        }
    }

    pub(super) fn render_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .show(ui, |ui| {
                {
                    // Tool Modes
                    let modes = [
                        ImageToolMode::View,
                        ImageToolMode::Crop,
                        ImageToolMode::Deskew,
                        ImageToolMode::Transform,
                        ImageToolMode::Redact,
                        ImageToolMode::Markup,
                        ImageToolMode::PickColor,
                        ImageToolMode::Resize,
                        ImageToolMode::Convert,
                        ImageToolMode::Ocr,
                        ImageToolMode::Barcode,
                        ImageToolMode::RemoveBg,
                        ImageToolMode::DepthMap,
                        ImageToolMode::DecorrelationStretch,
                        ImageToolMode::FilmSim,
                        ImageToolMode::CreativeFx,
                    ];

                    let tabs: Vec<_> = modes
                        .iter()
                        .map(|&mode| crate::ui_tabs::Tab::new(mode, mode.label()).icon(mode.icon()))
                        .collect();
                    let selected = if self.active_mode == ImageToolMode::Palette {
                        ImageToolMode::PickColor
                    } else {
                        self.active_mode
                    };
                    if let Some(mode) =
                        crate::ui_tabs::TabBar::new("image_tools", selected, &tabs, palette)
                            .show(ui)
                    {
                        self.set_tool_mode(mode);
                    }
                }
            });
    }
}
