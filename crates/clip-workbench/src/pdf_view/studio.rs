use super::navigation::LeftTab;
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
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        self.poll_info_worker(ctx, now);
        self.poll_preview_worker(ctx);
        self.poll_edit_worker(ctx, now);
        self.poll_document_tools(ctx, now);
        let navigation_escape = self.navigation_shortcuts(ctx);
        if !navigation_escape && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            if self.edit_in_progress {
                self.status_toast
                    .set("PDF edit is still running; wait for it to finish", now);
            } else {
                *close = true;
                return;
            }
        }
        if !ctx.wants_keyboard_input() && !self.document_tools.ocr_busy() {
            if ctx.input(|input| {
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
            }) {
                self.delete_current_page(now);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::R)) {
                self.rotate_current_page(90, now);
            }
        }
        self.texture_frame += 1;
        self.decode_requests.clear();
        self.poll_decode_worker(ctx);
        if let Some(page) = self.selected_page {
            self.ensure_page_texture(page, ctx);
        }
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none().fill(colors.canvas).inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                let actions_enabled = self.info_job.is_none() && !self.edit_in_progress
                    && !self.document_tools.ocr_busy() && self.selected_page.is_some();
                let mut rotate = false;
                let mut delete = false;
                let mut shell = StudioShell::new(&self.title, Icon::Pdf, palette, close)
                    .with_badge(if self.info_job.is_some() { "Loading PDF…".into() }
                        else { format!("{} pages", self.page_count) }, None)
                    .with_escape_hatch(EscapeHatch::primary("Okular", "okular", "Open PDF in Okular").with_icon(Icon::Pdf))
                    .with_footer_hints("Drag text: Select | Ctrl+C: Copy | Right-click image: Copy | Space-drag: Pan | Scroll: Zoom | T: ToC")
                    .with_footer_readout(self.selected_page.map(|page| format!("Page {page} of {}", self.page_count)).unwrap_or_default());
                let hatch = shell.render_header(ui, |ui| {
                    self.navigation_buttons(ui);
                    rotate = ui.add_enabled(actions_enabled, crate::ui_ext::secondary_button_icon(
                        Some(Icon::RotateCw), "Rotate [R]", colors.panel, palette)).clicked();
                    delete = ui.add_enabled(actions_enabled, crate::ui_ext::danger_button_icon(
                        Some(Icon::Trash), "Delete [Del]", colors.recessed, palette)).clicked();
                });
                if let Some(0) = hatch { EscapeHatchLauncher::launch("PDF Viewer", "okular", &self.path, &mut self.status_toast, now); }
                if rotate { self.rotate_current_page(90, now); }
                if delete { self.delete_current_page(now); }
                ui.separator();
                self.render_docked_body(ui, ctx, palette);
                let toast = self.status_toast.active(now).cloned();
                shell.render_footer(ui, &toast, now);
            });
        if let Some(page) = self.selected_page {
            self.ensure_page_texture(page, ctx);
        }
        self.schedule_page_decode(ctx);
    }

    fn render_docked_body(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let height = (ui.available_height() - 32.0).max(100.0);
        let gaps = ui.spacing().item_spacing.x * 2.0 + 1.0;
        let right_visible = self.navigation.right.is_some();
        let content =
            (ui.available_width() - gaps * if right_visible { 2.0 } else { 1.0 }).max(0.0);
        let left = (content * 0.22).clamp(160.0, 260.0).min(content * 0.3);
        let right = if right_visible {
            (content * 0.25).clamp(210.0, 310.0).min(content * 0.3)
        } else {
            0.0
        };
        let center = (content - left - right).max(0.0);
        ui.horizontal(|ui| {
            self.render_pane(ui, Vec2::new(left, height), palette, |state, ui| {
                state.navigation.left_tabs(ui, palette);
                ui.separator();
                match state.navigation.left {
                    LeftTab::Pages => state.render_page_thumbnails(ui, ctx, palette),
                    LeftTab::Contents => {
                        if let Some(page) = state.navigation.render_contents(ui, ctx) {
                            state.selected_page = Some(page);
                        }
                    }
                }
            });
            ui.separator();
            self.render_pane(ui, Vec2::new(center, height), palette, |state, ui| {
                state.render_page_canvas(ui, ctx, palette);
            });
            if right_visible {
                ui.separator();
                self.render_pane(ui, Vec2::new(right, height), palette, |state, ui| {
                    state.render_right_bar(ui, ctx, palette)
                });
            }
        });
    }

    fn render_pane(
        &mut self,
        ui: &mut egui::Ui,
        size: Vec2,
        palette: &little_clip_core::palette::AppPalette,
        render: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            egui::Frame::none()
                .fill(colors.recessed)
                .stroke(Stroke::new(1.0_f32, colors.panel))
                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| render(self, ui));
        });
    }

    fn render_page_thumbnails(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::ScrollArea::vertical()
            .id_source("pdf-page-thumbnails")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let pages = self
                    .pages
                    .iter()
                    .map(|page| {
                        (
                            page.page_num,
                            page.raster
                                .as_ref()
                                .map(|raster| raster.dimensions())
                                .unwrap_or((595, 842)),
                            page.error.is_some(),
                        )
                    })
                    .collect::<Vec<_>>();
                for (page, (width, height), failed) in pages {
                    let selected = self.selected_page == Some(page);
                    let accent = crate::hex_to_c32(&palette.harmonies.base);
                    egui::Frame::none()
                        .fill(colors.canvas)
                        .stroke(Stroke::new(
                            if selected { 2.0_f32 } else { 1.0_f32 },
                            if selected { accent } else { colors.panel },
                        ))
                        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                        .inner_margin(egui::Margin::same(crate::ui_ext::ITEM_SPACING_Y))
                        .show(ui, |ui| {
                            let aspect = width as f32 / height.max(1) as f32;
                            let image_width =
                                (130.0 * aspect).min((ui.available_width() - 10.0).max(20.0));
                            let size = Vec2::new(image_width, image_width / aspect);
                            let visible = ui.is_rect_visible(egui::Rect::from_min_size(
                                ui.next_widget_position(),
                                size,
                            ));
                            let texture = visible
                                .then(|| self.ensure_page_texture(page, ctx))
                                .flatten();
                            let clicked = if let Some(texture) = texture {
                                ui.add(egui::ImageButton::new(egui::load::SizedTexture::new(
                                    texture, size,
                                )))
                                .clicked()
                            } else {
                                ui.add(
                                    egui::Button::new(if failed { "Unavailable" } else { "…" })
                                        .min_size(size),
                                )
                                .clicked()
                            };
                            if clicked {
                                self.selected_page = Some(page);
                            }
                            ui.label(
                                egui::RichText::new(format!("Page {page}"))
                                    .strong()
                                    .color(colors.text_primary),
                            );
                        });
                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                }
            });
    }

    fn render_page_canvas(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let Some(page_num) = self.selected_page else {
            ui.centered_and_justified(|ui| {
                ui.label("Loading PDF pages…");
            });
            return;
        };
        self.ensure_page_texture(page_num, ctx);
        self.viewport.select_page(page_num);
        ui.horizontal(|ui| {
            ui.label(format!("Page {page_num} of {}", self.page_count));
            if ui.button("Fit").clicked() {
                self.viewport.zoom = 1.0;
                self.viewport.pan = Vec2::ZERO;
            }
            ui.label(format!("{:.0}%", self.viewport.zoom * 100.0));
        });
        ui.separator();
        if let Some(error) = &self.document_tools.layout_error {
            ui.collapsing("Text selection unavailable", |ui| {
                ui.label(error);
            });
        }
        let page = self.pages.iter().find(|page| page.page_num == page_num);
        let texture = page
            .and_then(|page| page.texture.as_ref())
            .map(|texture| texture.id());
        let dimensions = page
            .and_then(|page| page.raster.as_ref())
            .map(|raster| raster.dimensions())
            .unwrap_or_default();
        if let Some(texture) = texture {
            let layout = self.document_tools.layout_for(page_num);
            let viewport =
                egui::Rect::from_min_size(ui.next_widget_position(), ui.available_size());
            let page_rect = self.viewport.page_rect(viewport, dimensions);
            self.viewport.pan_with_primary = layout.as_ref().is_none_or(|layout| {
                self.document_tools
                    .interaction
                    .prepare(ui, page_rect, layout)
            });
            let response = self.viewport.show(ui, texture, dimensions);
            if let Some(layout) = layout {
                let page_rect = self.viewport.page_rect(response.rect, dimensions);
                if let Some(image) = self.document_tools.interaction.show(
                    ui,
                    &response,
                    page_rect,
                    &layout,
                    &self.navigation.query,
                ) {
                    self.copy_pdf_image(image);
                }
            }
        } else {
            let error = page.and_then(|page| page.error.as_ref());
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
            ui.centered_and_justified(|ui| {
                if let Some(error) = error {
                    ui.label(egui::RichText::new(error).color(colors.text_primary));
                } else {
                    ui.spinner();
                }
            });
        }
    }
}
