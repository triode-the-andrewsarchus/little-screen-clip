use super::*;

impl DiffViewerState {
    pub fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.poll_handoff(now, ctx);
        self.poll_copy_job(now);
        self.poll_computation();
        if self.is_computing() || self.copy_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context, close_requested: &mut bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *close_requested = true;
        }
        if ctx.egui_wants_keyboard_input() {
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

        self.handle_shortcuts(ctx, close_requested);

        if self.is_image() && self.image_mode == ImageDiffViewMode::Blink {
            // Reserve Space for the held preview while this view owns it.
            ctx.input_mut(|input| input.consume_key(input.modifiers, egui::Key::Space));
        }

        let is_svg = self.is_svg();
        let is_img = self.is_image() && !is_svg;

        self.handle_copy_shortcut(ctx, now);

        let mut shell = self.comparison_shell(palette, close_requested);

        egui::Frame::NONE
            .fill(crate::hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::from(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                        Some(Icon::Swap),
                        "Swap [S]",
                        palette,
                    );
                    if ui
                        .add(swap_btn)
                        .on_hover_text("Swap operands (S)")
                        .clicked()
                    {
                        self.swap_operands();
                    }

                    let copy_label = if is_svg {
                        "Copy Summary"
                    } else if is_img {
                        "Copy Diff Mask"
                    } else {
                        "Copy Diff"
                    };
                    let copy_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
                        Some(Icon::Copy),
                        copy_label,
                        palette,
                    );
                    if ui
                        .add_enabled(!is_svg || self.svg.is_some(), copy_btn)
                        .clicked()
                    {
                        self.copy_current_diff(ctx, now);
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
                if self.is_image() {
                    ui.horizontal_wrapped(|ui| {
                        graphical::toolbar(ui, &mut self.image_mode, palette);
                        if is_img {
                            ui.separator();
                            self.render_image_tolerance(ui, palette);
                        }
                    });
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
                        if self.is_image() {
                            self.render_graphical_viewport(ui, palette);
                        } else {
                            self.render_text_viewport(ui, palette);
                        }
                    },
                );

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 5: Footer
                shell.render_footer(ui, &self.status_toast, now);
            });
        if self.copy_pending() {
            *close_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn comparison_shell<'a>(
        &self,
        palette: &'a little_clip_core::palette::AppPalette,
        close_requested: &'a mut bool,
    ) -> StudioShell<'a> {
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let added = crate::hex_to_c32(&palette.diff_add);
        let removed = crate::hex_to_c32(&palette.diff_del);
        let (title, hints) = if self.is_svg() {
            (
                "SVG Comparison",
                "Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Summary",
            )
        } else if self.is_image() {
            (
                "Picture Diff Comparison",
                "Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Mask",
            )
        } else {
            (
                "Diff Comparison",
                "Navigation: [Esc] Close • [Tab/C] Toggle View • [S] Swap Operands • [Ctrl+C] Copy Diff",
            )
        };
        let hatch = EscapeHatch::new("Meld", "meld", "Open comparison in external Meld tool")
            .with_icon(Icon::Diff);
        let shell = StudioShell::new(title, Icon::Diff, palette, close_requested)
            .with_escape_hatch(hatch)
            .with_footer_hints(hints);
        if self.is_svg() {
            let (status, color) = match &self.svg {
                _ if self.is_computing() => ("Comparing SVG…".into(), accent),
                Some(result) if result.matches() => (result.summary(), added),
                Some(result) => (result.summary(), removed),
                None => ("Comparison unavailable".into(), removed),
            };
            return shell
                .with_badge(status, Some(color))
                .with_footer_readout(format!(
                    "Geometry tolerance: {} SVG user units",
                    little_clip_render::diagram::retained::compare::USER_TOLERANCE
                ));
        }
        if self.is_image() {
            return self.bitmap_shell(shell, [accent, added, removed]);
        }
        let status = if self.result.is_identical {
            ("Identical".into(), Some(added))
        } else {
            (
                format!("{} lines in diff", self.result.raw_ansi.lines().count()),
                None,
            )
        };
        shell
            .with_badge(self.result.engine_name.clone(), Some(accent))
            .with_badge(status.0, status.1)
            .with_footer_readout(format!(
                "LHS: {} • RHS: {}",
                self.lhs.label(),
                self.rhs.label()
            ))
    }

    fn bitmap_shell<'a>(
        &self,
        shell: StudioShell<'a>,
        [accent, added, removed]: [Color32; 3],
    ) -> StudioShell<'a> {
        if self.is_computing() {
            let elapsed = self.compute_start_time.elapsed().as_secs_f32();
            return shell.with_badge(format!("Comparing... ({elapsed:.1}s)"), Some(accent));
        }
        let Some(result) = &self.image_result else {
            return shell;
        };
        let (status, color) = if result.is_identical {
            ("0 diff pixels • Identical".into(), added)
        } else {
            (
                format!(
                    "{} diff pixels ({:.2}%)",
                    result.diff_count, result.diff_percentage
                ),
                removed,
            )
        };
        let aligned = self
            .raster_preview
            .as_ref()
            .is_some_and(|preview| preview.dimensions[0] != preview.dimensions[1]);
        let size = if aligned {
            format!("Aligned to {} × {}", result.width, result.height)
        } else {
            format!("{} × {}", result.width, result.height)
        };
        shell
            .with_badge(status, Some(color))
            .with_badge(size, None)
            .with_footer_readout(format!(
                "Tolerance: {:.2} • {} diff pixels",
                self.image_options.threshold, result.diff_count
            ))
    }

    fn render_graphical_viewport(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        if self.is_computing() {
            self.render_computing_ui(ui, palette);
            return;
        }
        if let Some(error) = &self.decode_error {
            ui.centered_and_justified(|ui| ui.label(error));
            return;
        }
        let sizes = if let Some(svg) = &self.svg {
            self.graphical.prepare_svg(ui.ctx(), svg);
            [svg.base.viewport, svg.revised.viewport].map(|[w, h]| egui::vec2(w, h))
        } else if let Some(raster) = &self.image_result {
            if let Some((images, overlay)) = self
                .raster_preview
                .as_mut()
                .and_then(|preview| preview.images.take())
            {
                self.graphical.prepare(ui.ctx(), images, Some(overlay));
            }
            [egui::vec2(raster.width as f32, raster.height as f32); 2]
        } else {
            return;
        };
        self.graphical.render(
            ui,
            palette,
            self.image_mode,
            graphical::Pair {
                labels: [self.lhs.label(), self.rhs.label()],
                sizes,
                svg: self.svg.as_ref(),
            },
        );
    }

    fn handle_copy_shortcut(&mut self, ctx: &egui::Context, now: f64) {
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
            self.copy_current_diff(ctx, now);
        }
    }

    fn copy_current_diff(&mut self, ctx: &egui::Context, now: f64) {
        if self.is_svg() {
            if let Some(result) = &self.svg {
                ctx.copy_text(format!(
                    "{} → {}\n{}",
                    self.lhs.label(),
                    self.rhs.label(),
                    result.summary()
                ));
                self.status_toast = Some(("SVG comparison summary copied".into(), now));
            }
        } else if self.is_image() {
            self.copy_image_diff_to_clipboard();
        } else {
            ctx.copy_text(strip_ansi_codes(&self.result.raw_ansi));
            self.status_toast = Some(("Diff text copied to clipboard!".into(), now));
        }
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

        let (heading, engine, detail) = if self.is_svg() {
            (
                "Comparing SVG…",
                "Retained vector geometry",
                "source geometry and paint",
            )
        } else if self.is_image() {
            (
                "Comparing Images...",
                "Parallel Perceptual Diff",
                "multithreaded SIMD comparison",
            )
        } else {
            (
                "Computing Diff...",
                "Syntax-Highlighted Diff",
                "background syntax diff engine",
            )
        };
        let readout = format!("{elapsed:.2}s elapsed • {detail}");

        ui.centered_and_justified(|ui| {
            egui::Frame::NONE
                .fill(b_med)
                .stroke(Stroke::new(1.0_f32, b_low))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(36, 28))
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
                                .text(engine),
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
