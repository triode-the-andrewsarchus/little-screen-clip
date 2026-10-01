use super::*;

impl PdfEditorState {
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.poll_info_worker(ctx, now);
        self.poll_preview_worker(ctx);
        self.poll_edit_worker(ctx, now);

        // Hotkeys
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.edit_in_progress {
                self.status_toast
                    .set("PDF edit is still running; wait for it to finish", now);
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                *close = true;
                return;
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            self.delete_current_page(now);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) {
            self.rotate_current_page(90, now);
        }

        self.texture_frame += 1;
        self.decode_requests.clear();
        self.poll_decode_worker(ctx);
        if let Some(page_num) = self.selected_page {
            self.ensure_page_texture(page_num, ctx);
        }

        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        let warn_accent = crate::hex_to_c32(&palette.undo_border);
        let warn_bg = crate::hex_to_c32(&palette.colors.surface_recessed);

        egui::Frame::none()
            .fill(colors.canvas)
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let page_count = self.page_count;
                let metadata_loading = self.info_job.is_some();
                let page_actions_enabled =
                    !metadata_loading && !self.edit_in_progress && self.selected_page.is_some();
                let mut rotate_clicked = false;
                let mut delete_clicked = false;

                let mut shell = StudioShell::new(&self.title, Icon::Pdf, palette, close)
                    .with_badge(
                        if metadata_loading {
                            "Loading PDF…".to_owned()
                        } else {
                            format!("{} pages", page_count)
                        },
                        None,
                    )
                    .with_escape_hatch(EscapeHatch::primary(
                        "Okular",
                        "okular",
                        "Open PDF in external viewer (Okular / Evince)",
                    ).with_icon(Icon::Pdf))
                    .with_footer_hints("Scroll: Zoom | Drag: Pan | Double-click: Fit | [R] Rotate | [Del] Delete | Esc: Close");

                if let Some(cur) = self.selected_page.filter(|_| !metadata_loading) {
                    shell = shell.with_footer_readout(format!("Selected: Page {} of {}", cur, page_count));
                }

                let clicked_hatch = shell.render_header(ui, |ui| {
                    // Delete Page Button
                    let del_btn = crate::ui_ext::danger_button_icon(
                        Some(Icon::Trash),
                        "Delete Page [Del]",
                        warn_bg,
                        palette,
                    )
                    .stroke(egui::Stroke::new(1.0_f32, warn_accent));
                    if ui
                        .add_enabled(page_actions_enabled, del_btn)
                        .on_hover_text("Delete selected page from PDF (Hotkey: Del)")
                        .clicked()
                    {
                        delete_clicked = true;
                    }

                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                    // Rotate 90 CW Button
                    let rot_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCw),
                        "Rotate 90° [R]",
                        colors.panel,
                        palette,
                    );
                    if ui
                        .add_enabled(page_actions_enabled, rot_btn)
                        .on_hover_text("Rotate selected page 90° clockwise (Hotkey: R)")
                        .clicked()
                    {
                        rotate_clicked = true;
                    }
                });

                if let Some(0) = clicked_hatch {
                    EscapeHatchLauncher::launch("PDF Viewer", "okular", &self.path, &mut self.status_toast, now);
                }

                if rotate_clicked {
                    self.rotate_current_page(90, now);
                }
                if delete_clicked {
                    self.delete_current_page(now);
                }

                ui.add_space(crate::ui_ext::SPACE_XS);
                ui.separator();
                ui.add_space(crate::ui_ext::SPACE_XS);

                // Two-Pane Split Layout
                // Left pane: page thumbnail list / grid (takes left side, wrapping if width permits)
                // Right pane: large preview of the selected page
                let avail_w = ui.available_width();
                let avail_h = (ui.available_height() - 32.0).max(100.0);
                // The horizontal row contains two item gaps and the separator in
                // addition to the panes. Budget those first so pane minimums never
                // make the combined layout wider than a compact popup.
                let pane_gap = ui.spacing().item_spacing.x * 2.0 + 1.0;
                let pane_width = (avail_w - pane_gap).max(0.0);
                let left_pane_w = (pane_width * 0.30)
                    .clamp(180.0, 360.0)
                    .min((pane_width - 200.0).max(0.0));
                let right_pane_w = (pane_width - left_pane_w).max(0.0);
                ui.horizontal(|ui| {
                    // --- Left Pane: Page Selector Grid ---
                    ui.allocate_ui_with_layout(
                        Vec2::new(left_pane_w, avail_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left_pane_w);
                            ui.set_height(avail_h);

                            egui::Frame::none()
                                .fill(colors.recessed)
                                .stroke(Stroke::new(1.0_f32, colors.panel))
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Pages")
                                                .strong()
                                                .color(colors.text_primary),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("{} total", page_count))
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(colors.text_muted),
                                                );
                                            },
                                        );
                                    });
                                    ui.add_space(crate::ui_ext::SPACE_XS);
                                    ui.separator();
                                    ui.add_space(crate::ui_ext::SPACE_XS);

                                    egui::ScrollArea::vertical()
                                        .max_width(
                                            (left_pane_w
                                                - 2.0 * crate::ui_ext::SPACE_SM
                                                - 2.0)
                                                .max(0.0),
                                        )
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            let thumbnail_area_width = (left_pane_w
                                                - 2.0 * crate::ui_ext::SPACE_SM
                                                - 2.0)
                                                .max(0.0);
                                            ui.set_width(thumbnail_area_width);
                                            ui.vertical(|ui| {
                                                let pages_clone: Vec<(
                                                    usize,
                                                    u32,
                                                    u32,
                                                    bool,
                                                )> = self
                                                    .pages
                                                    .iter()
                                                    .map(|p| {
                                                        let (width, height) = p
                                                            .raster
                                                            .as_ref()
                                                            .map(|raster| raster.dimensions())
                                                            .unwrap_or_default();
                                                        (
                                                            p.page_num,
                                                            width,
                                                            height,
                                                            p.error.is_some(),
                                                        )
                                                    })
                                                    .collect();

                                                for (page_num, w, h, failed) in pages_clone {
                                                    let is_selected =
                                                        self.selected_page == Some(page_num);

                                                    ui.vertical(|ui| {
                                                        let border_col = if is_selected {
                                                            accent
                                                        } else {
                                                            colors.panel
                                                        };
                                                        let stroke_w = if is_selected { 2.0_f32 } else { 1.0_f32 };

                                                        egui::Frame::none()
                                                            .fill(if is_selected {
                                                                colors.panel
                                                            } else {
                                                                colors.canvas
                                                            })
                                                            .stroke(Stroke::new(stroke_w, border_col))
                                                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                                                            .inner_margin(egui::Margin::same(crate::ui_ext::ITEM_SPACING_Y))
                                                            .show(ui, |ui| {
                                                                let aspect = if h > 0 {
                                                                    w as f32 / h as f32
                                                                } else {
                                                                    0.707
                                                                };
                                                                let panel_insets =
                                                                    2.0 * crate::ui_ext::SPACE_SM
                                                                        + 2.0;
                                                                let card_insets = 2.0
                                                                    * (2.0
                                                                        * crate::ui_ext::ITEM_SPACING_Y
                                                                        + 2.0);
                                                                let thumb_w_limit = (left_pane_w
                                                                    - panel_insets
                                                                    - card_insets)
                                                                    .max(80.0);
                                                                let thumb_w = (130.0 * aspect)
                                                                    .min(thumb_w_limit);
                                                                let thumb_h = thumb_w / aspect;

                                                                let thumbnail_rect = egui::Rect::from_min_size(
                                                                    ui.next_widget_position(),
                                                                    Vec2::new(thumb_w, thumb_h),
                                                                );
                                                                let tex_id_opt = ui.is_rect_visible(thumbnail_rect)
                                                                    .then(|| self.ensure_page_texture(page_num, ctx))
                                                                    .flatten();
                                                                if let Some(tex_id) = tex_id_opt {
                                                                    let img_btn = egui::ImageButton::new(
                                                                        egui::load::SizedTexture::new(
                                                                            tex_id,
                                                                            Vec2::new(thumb_w, thumb_h),
                                                                        ),
                                                                    );
                                                                    if ui.add(img_btn).clicked() {
                                                                        self.selected_page = Some(page_num);
                                                                    }
                                                                } else {
                                                                    let placeholder = egui::Button::new(if failed { "Unavailable" } else { "…" })
                                                                        .min_size(Vec2::new(thumb_w, thumb_h));
                                                                    if ui.add(placeholder).clicked() {
                                                                        self.selected_page = Some(page_num);
                                                                    }
                                                                }

                                                                ui.add_space(2.0);
                                                                ui.horizontal(|ui| {
                                                                    ui.label(
                                                                        egui::RichText::new(format!(
                                                                            "Page {}",
                                                                            page_num
                                                                        ))
                                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                                        .strong()
                                                                        .color(if is_selected {
                                                                            crate::ui_ext::contrast_text_c32_on(
                                                                                colors.panel,
                                                                                colors.panel,
                                                                                Some(accent),
                                                                                palette,
                                                                            )
                                                                        } else {
                                                                            colors.text_muted
                                                                        }),
                                                                    );
                                                                });
                                                            });
                                                    });

                                                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                                        }
                                            });
                                        });
                                });
                        },
                    );

                    ui.separator();

                    // --- Right Pane: Large Page Preview ---
                    ui.allocate_ui_with_layout(
                        Vec2::new(right_pane_w, avail_h),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.set_width(right_pane_w);
                            ui.set_height(avail_h);

                            egui::Frame::none()
                                .fill(colors.recessed)
                                .stroke(Stroke::new(1.0_f32, colors.panel))
                                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
                                .show(ui, |ui| {
                                    if let Some(page_num) = self.selected_page {
                                        self.ensure_page_texture(page_num, ctx);
                                    }
                                    let selected_item = self
                                        .selected_page
                                        .and_then(|sel| self.pages.iter().find(|p| p.page_num == sel));

                                    if let Some(page) = selected_item {
                                        let (page_width, page_height) = page
                                            .raster
                                            .as_ref()
                                            .map(|raster| raster.dimensions())
                                            .unwrap_or_default();
                                        self.viewport.select_page(page.page_num);
                                        // Header bar above preview
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Page {} of {}",
                                                    page.page_num, page_count
                                                ))
                                                .strong()
                                                .color(colors.text_primary),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "• {} × {} px",
                                                    page_width, page_height
                                                ))
                                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                                .color(colors.text_muted),
                                            );
                                            if ui.button("Fit").on_hover_text("Fit page and reset pan").clicked() {
                                                self.viewport.zoom = 1.0;
                                                self.viewport.pan = Vec2::ZERO;
                                            }
                                            ui.label(format!("{:.0}%", self.viewport.zoom * 100.0));
                                        });

                                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                                        ui.separator();
                                        ui.add_space(crate::ui_ext::SPACE_SM);

                                        if let Some(ref tex) = page.texture {
                                            self.viewport.show(ui, tex.id(), (page_width, page_height));
                                        } else {
                                            ui.centered_and_justified(|ui| {
                                                if let Some(error) = &page.error {
                                                    ui.label(
                                                        egui::RichText::new(error)
                                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                                            .color(crate::ui_ext::contrast_text_c32_on(
                                                                colors.recessed,
                                                                colors.recessed,
                                                                Some(colors.danger),
                                                                palette,
                                                            )),
                                                    );
                                                } else {
                                                    ui.spinner();
                                                }
                                            });
                                        }
                                    } else {
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                egui::RichText::new(if metadata_loading {
                                                    "Loading PDF pages…"
                                                } else {
                                                    "No page selected."
                                                })
                                                    .color(colors.text_muted),
                                            );
                                        });
                                    }
                                });
                        },
                    );
                });

                // Footer
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
        if let Some(page_num) = self.selected_page {
            self.ensure_page_texture(page_num, ctx);
        }
        self.schedule_page_decode(ctx);
    }
}
