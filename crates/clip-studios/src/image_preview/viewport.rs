use super::*;

impl ImagePreviewState {
    pub(super) fn render_viewport(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let avail_size = ui.available_size();
        self.viewport_size = avail_size;
        self.zoom = self.zoom.min(self.maximum_zoom(avail_size));
        if !self.auto_fitted && avail_size.x > 50.0 && avail_size.y > 50.0 {
            self.fit_to_view(avail_size);
            self.auto_fitted = true;
        }

        let (response, painter) = ui.allocate_painter(avail_size, egui::Sense::click_and_drag());
        let canvas_rect = response.rect;

        // Background canvas fill
        painter.rect_filled(
            canvas_rect,
            Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS),
            hex_to_c32(&palette.colors.canvas),
        );

        // Panning interaction (drag) when in View, Resize, Palette, RemoveBg, DepthMap, Ocr, Barcode, or Convert mode (when not dragging split handle)
        let middle_drag = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let can_drag_pan = self.active_mode == ImageToolMode::View
            || self.active_mode == ImageToolMode::Resize
            || self.active_mode == ImageToolMode::Palette
            || self.active_mode == ImageToolMode::RemoveBg
            || self.active_mode == ImageToolMode::DepthMap
            || self.active_mode == ImageToolMode::Ocr
            || self.active_mode == ImageToolMode::Barcode
            || (self.active_mode == ImageToolMode::Convert
                && !self.convert_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::DecorrelationStretch
                && !self.dcs_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::FilmSim
                && !self.film_sim_state.is_dragging_split)
            || (self.active_mode == ImageToolMode::CreativeFx
                && !self.creative_fx_state.is_dragging_split);
        if (can_drag_pan && response.dragged()) || middle_drag {
            self.pan += response.drag_delta();
        }

        // Zooming interaction (mouse wheel)
        if response.hovered() {
            let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
            if scroll_delta.abs() > 0.0 {
                let zoom_factor = (scroll_delta * 0.002).exp();
                let old_zoom = self.zoom;
                let new_zoom =
                    (old_zoom * zoom_factor).clamp(0.05, self.maximum_zoom(self.viewport_size));

                if let Some(ptr_pos) = response.hover_pos() {
                    let center = canvas_rect.center() + self.pan;
                    let to_ptr = ptr_pos - center;
                    self.pan += to_ptr * (1.0 - new_zoom / old_zoom);
                }
                self.zoom = new_zoom;
            }
        }

        // Double click resets view to fit
        if can_drag_pan && response.double_clicked() {
            self.fit_to_view(avail_size);
        }

        // Draw image texture
        if let Some(texture) = &self.texture {
            let img_w = self.current_rgba.width() as f32 * self.zoom;
            let img_h = self.current_rgba.height() as f32 * self.zoom;
            let center = canvas_rect.center() + self.pan;
            let img_rect = Rect::from_center_size(center, Vec2::new(img_w, img_h));

            // Draw checkerboard under image for alpha transparency
            draw_checkerboard(
                &painter,
                img_rect,
                self.zoom,
                hex_to_c32(&palette.colors.surface_recessed),
                hex_to_c32(&palette.colors.surface_panel),
            );

            // Draw image mesh (or Squoosh-style split view in Convert mode or DCS curtain view in DecorrelationStretch mode)
            if self.active_mode == ImageToolMode::Convert {
                self.render_convert_comparison(
                    ui,
                    &painter,
                    &response,
                    img_rect,
                    canvas_rect,
                    ctx,
                    palette,
                );
            } else if self.active_mode == ImageToolMode::DecorrelationStretch {
                self.render_decorrelation_stretch_comparison(
                    ui,
                    &painter,
                    &response,
                    img_rect,
                    canvas_rect,
                    ctx,
                    palette,
                );
            } else {
                let mut mesh = egui::Mesh::with_texture(texture.id());
                mesh.add_rect_with_uv(
                    img_rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.add(egui::Shape::mesh(mesh));
            }

            // Draw crisp subtle border around the image bounds
            painter.rect_stroke(
                img_rect,
                Rounding::ZERO,
                Stroke::new(
                    1.0_f32,
                    hex_to_c32_alpha(&palette.colors.text_secondary, 40),
                ),
            );

            // Draw any placed redaction bars so they remain visible across all modes
            for b in &self.redaction_state.boxes {
                let screen_min = Pos2::new(
                    img_rect.min.x + b.normalized_rect.min.x * img_rect.width(),
                    img_rect.min.y + b.normalized_rect.min.y * img_rect.height(),
                );
                let screen_max = Pos2::new(
                    img_rect.min.x + b.normalized_rect.max.x * img_rect.width(),
                    img_rect.min.y + b.normalized_rect.max.y * img_rect.height(),
                );
                painter.rect_filled(
                    Rect::from_min_max(screen_min, screen_max),
                    Rounding::ZERO,
                    Color32::BLACK,
                );
            }

            // Draw any placed markup layers so they remain visible across all modes
            if self.active_mode != ImageToolMode::Markup && !self.markup_state.items.is_empty() {
                self.render_markup_passive(&painter, img_rect);
            }

            // Mode-specific on-canvas overlays
            if self.active_mode == ImageToolMode::Crop {
                self.render_crop_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Deskew {
                self.render_deskew_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Redact {
                self.render_redact_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::Markup {
                self.render_markup_overlay(ui, &painter, &response, img_rect, palette);
            } else if self.active_mode == ImageToolMode::PickColor {
                self.render_pick_color_overlay(
                    ui,
                    &painter,
                    &response,
                    img_rect,
                    canvas_rect,
                    palette,
                );
            } else if self.active_mode == ImageToolMode::Barcode {
                self.render_barcode_overlay(ui, &painter, &response, img_rect, palette);
            }
        }
    }

    // Permit five times the fitted scale, keeping maximum magnification within 500–5000%.
    pub(super) fn maximum_zoom(&self, canvas_size: Vec2) -> f32 {
        let image_size = Vec2::new(
            self.current_rgba.width() as f32,
            self.current_rgba.height() as f32,
        );
        let available = (canvas_size - Vec2::splat(40.0)).max(Vec2::splat(10.0));
        let fit = (available.x / image_size.x.max(1.0)).min(available.y / image_size.y.max(1.0));
        (fit * 5.0).clamp(5.0, 50.0)
    }

    pub(super) fn render_zoom_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if ui
            .add_sized(Vec2::new(56.0, 24.0), egui::Button::new("Reset").small())
            .on_hover_text("Fit and center the image (0)")
            .clicked()
        {
            self.fit_to_view(self.viewport_size);
        }
        if ui
            .add_sized(Vec2::new(40.0, 24.0), egui::Button::new("1:1").small())
            .on_hover_text("100% actual pixel size (1)")
            .clicked()
        {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        ui.add_sized(
            Vec2::new(64.0, 24.0),
            egui::Label::new(format!("{:.0}%", self.zoom * 100.0)),
        );
    }

    pub(super) fn fit_to_view(&mut self, canvas_size: Vec2) {
        let img_w = self.current_rgba.width() as f32;
        let img_h = self.current_rgba.height() as f32;
        if img_w <= 0.0 || img_h <= 0.0 {
            return;
        }

        let margin = 40.0;
        let avail_w = (canvas_size.x - margin).max(10.0);
        let avail_h = (canvas_size.y - margin).max(10.0);

        let scale_x = avail_w / img_w;
        let scale_y = avail_h / img_h;
        self.zoom = scale_x
            .min(scale_y)
            .clamp(0.01, self.maximum_zoom(canvas_size));
        self.pan = Vec2::ZERO;
    }
}
