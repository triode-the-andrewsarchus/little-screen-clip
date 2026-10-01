use super::*;

impl DiffViewerState {
    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context, close_requested: &mut bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close_requested = true;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| {
            i.key_pressed(egui::Key::Tab)
                || (i.key_pressed(egui::Key::C)
                    && !i.modifiers.command
                    && !i.modifiers.ctrl
                    && !i.modifiers.mac_cmd)
        }) {
            self.toggle_mode();
        }
        if ctx.input(|i| {
            i.key_pressed(egui::Key::S)
                && !i.modifiers.command
                && !i.modifiers.ctrl
                && !i.modifiers.mac_cmd
                && !i.modifiers.shift
                && !i.modifiers.alt
        }) {
            self.swap_operands();
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);
        self.poll_handoff(now, ctx);

        self.handle_shortcuts(ctx, close_requested);

        let is_img = self.is_image();

        if is_img {
            if self.is_computing {
                ctx.request_repaint();
                if let Some(ref rx) = self.image_receiver {
                    match rx.try_recv() {
                        Ok(Ok(Ok(res))) => {
                            self.image_result = Some(res);
                            self.decode_error = None;
                            self.is_computing = false;
                            self.textures_loaded = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                        Ok(Ok(Err(err))) => {
                            self.decode_error = Some(err);
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                        Ok(Err(err)) => {
                            self.decode_error = Some(format!("Diff computation failed: {err}"));
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            self.decode_error = Some("Diff worker thread disconnected".to_string());
                            self.image_result = None;
                            self.is_computing = false;
                            self.image_receiver = None;
                            self.compute_ticket = None;
                        }
                    }
                }
            }

            self.ensure_image_textures(ctx);
        } else if self.is_computing {
            ctx.request_repaint();
            if let Some(ref rx) = self.text_receiver {
                match rx.try_recv() {
                    Ok(Ok((res, job))) => {
                        self.result = res;
                        self.layout_job = job;
                        self.is_computing = false;
                        self.text_receiver = None;
                        self.decode_error = None;
                        self.compute_ticket = None;
                    }
                    Ok(Err(err)) => {
                        self.is_computing = false;
                        self.text_receiver = None;
                        self.decode_error = Some(format!("Diff computation failed: {err}"));
                        self.compute_ticket = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.is_computing = false;
                        self.text_receiver = None;
                        self.decode_error = Some("Diff worker disconnected".to_string());
                        self.compute_ticket = None;
                    }
                }
            }
        }

        // Copy shortcut
        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || i.raw.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.key_pressed(egui::Key::C)
                    && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
        });

        if copy_requested {
            if is_img {
                self.copy_image_diff_to_clipboard(now);
            } else {
                ctx.output_mut(|o| o.copied_text = strip_ansi_codes(&self.result.raw_ansi));
                self.status_toast = Some(("Diff text copied to clipboard!".to_string(), now));
            }
        }

        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let success_color = crate::hex_to_c32(&palette.diff_add);
        let del_color = crate::hex_to_c32(&palette.diff_del);

        let hatch = EscapeHatch::new("Meld", "meld", "Open comparison in external Meld tool")
            .with_icon(Icon::Diff);

        let mut shell = if is_img {
            let mut s = StudioShell::new("Picture Diff Comparison", Icon::Diff, palette, close_requested)
                .with_escape_hatch(hatch)
                .with_footer_hints("Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Mask");

            if self.is_computing {
                let elapsed = self.compute_start_time.elapsed().as_secs_f32();
                s = s.with_badge(format!("Comparing... ({:.1}s)", elapsed), Some(accent));
            } else if let Some(res) = &self.image_result {
                if res.is_identical {
                    s = s.with_badge("0 diff pixels • Identical", Some(success_color));
                } else {
                    s = s.with_badge(
                        format!(
                            "{} diff pixels ({:.2}%)",
                            res.diff_count, res.diff_percentage
                        ),
                        Some(del_color),
                    );
                }
                s = s.with_badge(format!("{} × {}", res.width, res.height), None);
                s = s.with_footer_readout(format!(
                    "Tolerance: {:.2} • {} diff pixels",
                    self.image_options.threshold, res.diff_count
                ));
            }
            s
        } else {
            let lines_count = self.result.raw_ansi.lines().count();
            let mut s = StudioShell::new("Diff Comparison", Icon::Diff, palette, close_requested)
                .with_badge(format!("{}", self.result.engine_name), Some(accent))
                .with_escape_hatch(hatch)
                .with_footer_hints("Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Diff");

            if self.result.is_identical {
                s = s.with_badge("Identical", Some(success_color));
            } else {
                s = s.with_badge(format!("{} lines in diff", lines_count), None);
            }
            s = s.with_footer_readout(format!(
                "LHS: {} • RHS: {}",
                self.lhs.label(),
                self.rhs.label()
            ));
            s
        };

        egui::Frame::none()
            .fill(crate::hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Swap),
                        "Swap [S]",
                        b_med,
                        palette,
                    );
                    if ui
                        .add(swap_btn)
                        .on_hover_text("Swap operands (S)")
                        .clicked()
                    {
                        self.swap_operands();
                    }

                    let copy_label = if is_img {
                        "Copy Diff Mask"
                    } else {
                        "Copy Diff"
                    };
                    let copy_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::Copy),
                        copy_label,
                        b_med,
                        palette,
                    );
                    if ui.add(copy_btn).clicked() {
                        if is_img {
                            self.copy_image_diff_to_clipboard(now);
                        } else {
                            ui.output_mut(|o| {
                                o.copied_text = strip_ansi_codes(&self.result.raw_ansi)
                            });
                            self.status_toast =
                                Some(("Diff text copied to clipboard!".to_string(), now));
                        }
                    }
                });

                if clicked_hatch.is_some() {
                    self.trigger_escape_hatch(now);
                }

                if shell.is_closed() {
                    return;
                }

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 2: Mode Toolbar
                if is_img {
                    self.render_image_toolbar(ui, palette);
                } else {
                    self.render_text_toolbar(ui, palette);
                }

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                // Zone 3: Main comparison viewport
                let footer_h = 28.0;
                let avail = ui.available_size();
                let viewport_h = (avail.y - footer_h - 6.0).max(100.0);

                ui.allocate_ui_with_layout(
                    Vec2::new(avail.x, viewport_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        if is_img {
                            self.render_image_viewport(ui, palette);
                        } else {
                            self.render_text_viewport(ui, palette);
                        }
                    },
                );

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 5: Footer
                shell.render_footer(ui, &self.status_toast, now);
            });
    }

    pub(super) fn render_computing_ui(
        &self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let elapsed = self.compute_start_time.elapsed().as_secs_f32();
        let progress = (elapsed * 0.75).fract();
        let b_med = crate::hex_to_c32(&palette.colors.surface_panel);
        let b_low = crate::hex_to_c32(&palette.colors.surface_recessed);
        let f_high = crate::hex_to_c32(&palette.colors.text_primary);
        let f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let accent = crate::hex_to_c32(&palette.harmonies.base);

        let heading = if self.is_image() {
            "Comparing Images..."
        } else {
            "Computing Diff..."
        };
        let bar_text = if self.is_image() {
            "Parallel Perceptual Diff"
        } else {
            "Syntax-Highlighted Diff"
        };
        let readout = if self.is_image() {
            format!("{:.2}s elapsed • multithreaded SIMD comparison", elapsed)
        } else {
            format!("{:.2}s elapsed • background syntax diff engine", elapsed)
        };

        ui.centered_and_justified(|ui| {
            egui::Frame::none()
                .fill(b_med)
                .stroke(Stroke::new(1.0_f32, b_low))
                .rounding(Rounding::same(12.0))
                .inner_margin(egui::Margin::symmetric(36.0, 28.0))
                .show(ui, |ui| {
                    ui.set_max_width(400.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Spinner::new().size(40.0).color(accent));
                        ui.add_space(14.0);

                        ui.label(
                            egui::RichText::new(heading)
                                .heading()
                                .strong()
                                .color(f_high),
                        );
                        ui.add_space(crate::ui_ext::SPACE_SM);

                        // Animated progress indicator bar
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .animate(true)
                                .desired_width(300.0)
                                .text(bar_text),
                        );

                        ui.add_space(crate::ui_ext::SPACE_MD);

                        ui.label(
                            egui::RichText::new(format!("Base: {}", self.lhs.label()))
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );
                        ui.label(
                            egui::RichText::new(format!("Comp: {}", self.rhs.label()))
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );

                        ui.add_space(crate::ui_ext::SPACE_SM);

                        ui.label(egui::RichText::new(readout).size(BODY_FONT_SIZE).color(
                            crate::ui_ext::contrast_text_c32_on(
                                b_med,
                                b_med,
                                Some(accent),
                                palette,
                            ),
                        ));

                        ui.add_space(10.0);

                        ui.label(
                            egui::RichText::new("[Esc] Cancel / Close")
                                .size(BODY_FONT_SIZE)
                                .color(f_low),
                        );
                    });
                });
        });
    }
}
