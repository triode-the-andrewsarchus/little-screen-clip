use super::super::*;

#[derive(Debug, Clone, Default)]
pub struct PickColorState {
    pub hovered_color: Option<Color32>,
    pub hovered_pixel: Option<(u32, u32)>,
    pub last_picked: Option<Color32>,
    pub recent_colors: Vec<Color32>,
    /// Explicit palette selections. Clicking the image/swatch adds here; it
    /// never mutates the system clipboard implicitly.
    pub picked_colors: Vec<Color32>,
}

impl ImagePreviewState {
    /// Copies the given color as a hex code string to the clipboard and tracks recent colors.

    pub fn copy_color_to_clipboard(&mut self, col: Color32, ctx: &egui::Context) {
        let hex = format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b());
        ctx.output_mut(|o| o.copied_text = hex.clone());

        let hex_for_cmd = hex.clone();
        std::thread::spawn(move || {
            if let Err(error) = little_clip_core::copy_text_to_system_clipboard(&hex_for_cmd) {
                tracing::debug!(%error, "failed to copy picked color to system clipboard");
            }
        });

        self.pick_color_state.last_picked = Some(col);
        if !self.pick_color_state.recent_colors.contains(&col) {
            self.pick_color_state.recent_colors.insert(0, col);
            if self.pick_color_state.recent_colors.len() > 12 {
                self.pick_color_state.recent_colors.pop();
            }
        }

        self.show_toast(format!("💉 Copied {} to clipboard!", hex));
    }

    /// Records a picked swatch without changing the system clipboard. Clipboard
    /// writes are intentionally explicit through the Copy Hex action.

    pub(crate) fn record_picked_color(&mut self, col: Color32) {
        self.pick_color_state.last_picked = Some(col);
        if !self.pick_color_state.picked_colors.contains(&col) {
            self.pick_color_state.picked_colors.push(col);
        }
        if !self.pick_color_state.recent_colors.contains(&col) {
            self.pick_color_state.recent_colors.insert(0, col);
            self.pick_color_state.recent_colors.truncate(12);
        }
    }

    /// Copies the currently displayed image to a specific clipboard hand slot (Left or Right) via daemon IPC,
    /// and simultaneously syncs to the OS clipboard through the shared adapter.

    pub(crate) fn render_pick_color_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);

        let img_w = self.current_rgba.width() as i32;
        let img_h = self.current_rgba.height() as i32;

        let ptr_opt = response.hover_pos();
        if let Some(ptr) = ptr_opt {
            if img_rect.contains(ptr) && img_rect.width() > 0.0 && img_rect.height() > 0.0 {
                let norm_x = ((ptr.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 0.9999);
                let norm_y = ((ptr.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 0.9999);

                let px = (norm_x * img_w as f32).floor() as i32;
                let py = (norm_y * img_h as f32).floor() as i32;

                let pixel_data = *self.current_rgba.get_pixel(px as u32, py as u32);
                let col = Color32::from_rgba_unmultiplied(
                    pixel_data[0],
                    pixel_data[1],
                    pixel_data[2],
                    pixel_data[3],
                );
                self.pick_color_state.hovered_color = Some(col);
                self.pick_color_state.hovered_pixel = Some((px as u32, py as u32));

                if response.clicked() {
                    self.record_picked_color(col);
                    self.show_toast("Color added to picked colors");
                }

                // Render Magnifying Loupe Card
                let canvas_rect = ui.available_rect_before_wrap();
                let loupe_r: i32 = 4; // 9x9 grid (-4..=4)
                let cell_size = 14.0;
                let grid_w = (loupe_r * 2 + 1) as f32 * cell_size; // 126.0
                let card_w = grid_w + 20.0; // 146.0
                let card_h = grid_w + 80.0; // 206.0

                let mut card_min = ptr + Vec2::new(20.0, 20.0);
                if card_min.x + card_w > canvas_rect.max.x {
                    card_min.x = ptr.x - card_w - 20.0;
                }
                if card_min.y + card_h > canvas_rect.max.y {
                    card_min.y = ptr.y - card_h - 20.0;
                }
                let card_rect = Rect::from_min_size(card_min, Vec2::new(card_w, card_h));

                // Card background & sleek harmonic border
                let card_bg = hex_to_c32_alpha(&palette.theme.b_low, 245);
                painter.rect_filled(card_rect, Rounding::same(8.0), card_bg);
                painter.rect_stroke(
                    card_rect,
                    Rounding::same(8.0),
                    Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
                );

                // Loupe Grid
                let grid_origin = card_min + Vec2::new(10.0, 10.0);
                for dy in -loupe_r..=loupe_r {
                    for dx in -loupe_r..=loupe_r {
                        let samp_x = (px + dx).clamp(0, img_w - 1) as u32;
                        let samp_y = (py + dy).clamp(0, img_h - 1) as u32;
                        let samp_pix = self.current_rgba.get_pixel(samp_x, samp_y);
                        let cell_col = Color32::from_rgba_unmultiplied(
                            samp_pix[0],
                            samp_pix[1],
                            samp_pix[2],
                            samp_pix[3],
                        );

                        let cell_min = grid_origin
                            + Vec2::new(
                                (dx + loupe_r) as f32 * cell_size,
                                (dy + loupe_r) as f32 * cell_size,
                            );
                        let cell_rect = Rect::from_min_size(cell_min, Vec2::splat(cell_size));

                        // Draw alpha checkerboard under transparent pixels
                        if samp_pix[3] < 255 {
                            let half = cell_size / 2.0;
                            painter.rect_filled(
                                Rect::from_min_size(cell_min, Vec2::splat(half)),
                                Rounding::ZERO,
                                Color32::from_gray(180),
                            );
                            painter.rect_filled(
                                Rect::from_min_size(
                                    cell_min + Vec2::splat(half),
                                    Vec2::splat(half),
                                ),
                                Rounding::ZERO,
                                Color32::from_gray(180),
                            );
                        }

                        painter.rect_filled(cell_rect, Rounding::ZERO, cell_col);

                        // Highlight center cell
                        if dx == 0 && dy == 0 {
                            painter.rect_stroke(
                                cell_rect,
                                Rounding::ZERO,
                                Stroke::new(2.0_f32, hex_to_c32(&palette.theme.f_high)),
                            );
                            painter.rect_stroke(
                                cell_rect.expand(1.0),
                                Rounding::ZERO,
                                Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_low)),
                            );
                        }
                    }
                }

                // Color Swatch below grid
                let swatch_y = grid_origin.y + grid_w + 8.0;
                let swatch_rect = Rect::from_min_size(
                    Pos2::new(card_min.x + 10.0, swatch_y),
                    Vec2::new(card_w - 20.0, 16.0),
                );
                painter.rect_filled(swatch_rect, Rounding::same(3.0), col);
                painter.rect_stroke(
                    swatch_rect,
                    Rounding::same(3.0),
                    Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)),
                );

                // Hex Code Text
                let hex_str = format!(
                    "#{:02X}{:02X}{:02X}",
                    pixel_data[0], pixel_data[1], pixel_data[2]
                );
                let hex_text_col =
                    contrast_text_c32(card_bg, Some(hex_to_c32(&palette.theme.f_high)), palette);
                let hex_galley =
                    painter.layout_no_wrap(hex_str, egui::FontId::monospace(12.0), hex_text_col);
                painter.galley(
                    Pos2::new(card_min.x + 10.0, swatch_y + 20.0),
                    hex_galley,
                    hex_text_col,
                );

                // RGB & Coordinate Info
                let info_str = format!(
                    "RGB({}, {}, {}) | {}, {}",
                    pixel_data[0], pixel_data[1], pixel_data[2], px, py
                );
                let info_text_col =
                    contrast_text_c32(card_bg, Some(hex_to_c32(&palette.theme.f_med)), palette);
                let info_galley = painter.layout_no_wrap(
                    info_str,
                    egui::FontId::proportional(9.5),
                    info_text_col,
                );
                painter.galley(
                    Pos2::new(card_min.x + 10.0, swatch_y + 36.0),
                    info_galley,
                    info_text_col,
                );

                // Click to copy hint
                let hint_text_col = contrast_text_c32(
                    card_bg,
                    Some(hex_to_c32(&palette.harmonies.triadic_1)),
                    palette,
                );
                let hint_galley = painter.layout_no_wrap(
                    "🖱 Click to copy".to_string(),
                    egui::FontId::proportional(9.0),
                    hint_text_col,
                );
                painter.galley(
                    Pos2::new(card_min.x + 10.0, swatch_y + 49.0),
                    hint_galley,
                    hint_text_col,
                );
            } else {
                self.pick_color_state.hovered_color = None;
                self.pick_color_state.hovered_pixel = None;
            }
        } else {
            self.pick_color_state.hovered_color = None;
            self.pick_color_state.hovered_pixel = None;
        }
    }
}

impl ImagePreviewState {
    pub(super) fn render_pick_color_results(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.theme.b_med);
        let text = hex_to_c32(&palette.theme.f_med);
        let muted = hex_to_c32(&palette.theme.f_low);

        ui.label(
            egui::RichText::new("Color picker")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Hover the image to inspect; click to keep a color.")
                .size(11.5)
                .color(muted),
        );
        ui.add_space(10.0);

        let current = self
            .pick_color_state
            .hovered_color
            .or(self.pick_color_state.last_picked)
            .unwrap_or(Color32::WHITE);
        ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(Vec2::splat(54.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(swatch, Rounding::same(5.0), current);
            ui.painter()
                .rect_stroke(swatch, Rounding::same(5.0), Stroke::new(1.0_f32, surface));

            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "#{:02X}{:02X}{:02X}",
                        current.r(),
                        current.g(),
                        current.b()
                    ))
                    .size(16.0)
                    .strong()
                    .monospace()
                    .color(text),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "RGB {}, {}, {}",
                        current.r(),
                        current.g(),
                        current.b()
                    ))
                    .size(11.5)
                    .color(muted),
                );
            });
        });
        ui.add_space(6.0);
        let copy_hex = crate::action_button("Copy current hex", accent, palette);
        if ui
            .add_sized([ui.available_width(), 30.0], copy_hex)
            .clicked()
        {
            self.copy_color_to_clipboard(current, ctx);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("PICKED COLORS")
                    .size(12.0)
                    .strong()
                    .color(text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(self.pick_color_state.picked_colors.len().to_string())
                        .size(12.0)
                        .color(muted),
                );
            });
        });

        if self.pick_color_state.picked_colors.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("No colors picked yet.")
                    .size(12.0)
                    .italics()
                    .color(muted),
            );
            return;
        }

        ui.add_space(4.0);
        let copy_all = crate::secondary_button("Copy all colors", surface, palette);
        if ui
            .add_sized([ui.available_width(), 28.0], copy_all)
            .clicked()
        {
            let colors = self
                .pick_color_state
                .picked_colors
                .iter()
                .map(|color| format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b()))
                .collect::<Vec<_>>()
                .join(", ");
            ctx.output_mut(|output| output.copied_text = colors);
        }

        for color in self.pick_color_state.picked_colors.clone() {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                let (swatch, _) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                ui.painter().rect_filled(swatch, Rounding::same(4.0), color);
                ui.painter().rect_stroke(
                    swatch,
                    Rounding::same(4.0),
                    Stroke::new(1.0_f32, surface),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "#{:02X}{:02X}{:02X}",
                        color.r(),
                        color.g(),
                        color.b()
                    ))
                    .size(13.0)
                    .monospace()
                    .color(text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(crate::compact_toolbar_button("Copy", surface, palette))
                        .clicked()
                    {
                        self.copy_color_to_clipboard(color, ctx);
                    }
                });
            });
        }
    }
}
