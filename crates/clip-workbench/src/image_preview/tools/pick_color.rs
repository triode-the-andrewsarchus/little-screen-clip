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
        self.copy_color_to_clipboard_on(col, ctx, BackgroundJobs::global(), |text| {
            if let Err(error) = little_clip_core::copy_text_to_system_clipboard(text) {
                tracing::debug!(%error, "failed to copy picked color to system clipboard");
            }
        });
    }

    fn copy_color_to_clipboard_on(
        &mut self,
        col: Color32,
        ctx: &egui::Context,
        jobs: &BackgroundJobs,
        copy: impl FnOnce(&str) + Send + 'static,
    ) {
        let hex = format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b());
        ctx.output_mut(|o| o.copied_text = hex.clone());

        schedule_system_clipboard_copy_on(jobs, hex.clone(), copy);

        self.pick_color_state.last_picked = Some(col);
        if !self.pick_color_state.recent_colors.contains(&col) {
            self.pick_color_state.recent_colors.insert(0, col);
            if self.pick_color_state.recent_colors.len() > 12 {
                self.pick_color_state.recent_colors.pop();
            }
        }

        self.show_toast(crate::ui_ext::StatusMessage::new(
            crate::icons::Icon::Eyedropper,
            format!("Copied {} to clipboard!", hex),
        ));
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
        canvas_rect: Rect,
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

                self.render_pick_color_loupe(painter, ptr, canvas_rect, px, py, palette);
            } else {
                self.pick_color_state.hovered_color = None;
                self.pick_color_state.hovered_pixel = None;
            }
        } else {
            self.pick_color_state.hovered_color = None;
            self.pick_color_state.hovered_pixel = None;
        }
    }

    fn render_pick_color_loupe(
        &self,
        painter: &egui::Painter,
        ptr: Pos2,
        canvas_rect: Rect,
        px: i32,
        py: i32,
        palette: &AppPalette,
    ) {
        let img_w = self.current_rgba.width() as i32;
        let img_h = self.current_rgba.height() as i32;
        let pixel_data = *self.current_rgba.get_pixel(px as u32, py as u32);
        let col = Color32::from_rgba_unmultiplied(
            pixel_data[0],
            pixel_data[1],
            pixel_data[2],
            pixel_data[3],
        );
        // Render Magnifying Loupe Card
        let loupe_r: i32 = 4; // 9x9 grid (-4..=4)
        // Keep enough width for the shared 15pt monospace hex value
        // while scaling the loupe cells down for compact panels.
        let card_w = canvas_rect.width().clamp(92.0, 146.0);
        let cell_size = ((card_w - 20.0) / 9.0).min(14.0);
        let grid_w = (loupe_r * 2 + 1) as f32 * cell_size; // 126.0
        let card_padding = (card_w - grid_w) / 2.0;

        let card_bg = hex_to_c32_alpha(&palette.colors.surface_recessed, 245);
        let hex_text_col = contrast_text_c32(
            card_bg,
            Some(hex_to_c32(&palette.colors.text_primary)),
            palette,
        );
        let info_text_col = contrast_text_c32(
            card_bg,
            Some(hex_to_c32(&palette.colors.text_secondary)),
            palette,
        );
        let hint_text_col = contrast_text_c32(
            card_bg,
            Some(hex_to_c32(&palette.harmonies.triadic_1)),
            palette,
        );
        let hex_str = format!(
            "#{:02X}{:02X}{:02X}",
            pixel_data[0], pixel_data[1], pixel_data[2]
        );
        let readout_lines = [
            (hex_str, true, hex_text_col),
            (
                format!("R{} G{} B{}", pixel_data[0], pixel_data[1], pixel_data[2]),
                true,
                info_text_col,
            ),
            (format!("X: {px}  Y: {py}"), true, info_text_col),
            ("Click to keep".to_owned(), false, hint_text_col),
        ];
        let readout: Vec<_> = readout_lines
            .into_iter()
            .map(|(line, monospace, color)| {
                let font = if monospace {
                    egui::FontId::monospace(crate::ui_ext::MONO_FONT_SIZE)
                } else {
                    egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE)
                };
                (painter.layout_no_wrap(line, font, color), color)
            })
            .collect();
        let readout_height = readout
            .iter()
            .map(|(galley, _)| galley.size().y)
            .sum::<f32>()
            + (readout.len().saturating_sub(1) as f32 * 2.0);
        let card_h = grid_w + 56.0 + readout_height;
        let card_h = card_h.min(canvas_rect.height().max(1.0));

        let mut card_min = ptr + Vec2::new(20.0, 20.0);
        if card_min.x + card_w > canvas_rect.max.x {
            card_min.x = ptr.x - card_w - 20.0;
        }
        if card_min.y + card_h > canvas_rect.max.y {
            card_min.y = ptr.y - card_h - 20.0;
        }
        let card_rect =
            crate::ui_ext::clamp_overlay_rect(card_min, Vec2::new(card_w, card_h), canvas_rect);
        card_min = card_rect.min;

        // Card background & sleek harmonic border
        painter.rect_filled(card_rect, Rounding::same(8.0), card_bg);
        painter.rect_stroke(
            card_rect,
            Rounding::same(8.0),
            Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)),
        );

        // Loupe Grid
        let grid_origin = card_min + Vec2::new(card_padding, 10.0);
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
                        Rect::from_min_size(cell_min + Vec2::splat(half), Vec2::splat(half)),
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
                        Stroke::new(2.0_f32, hex_to_c32(&palette.colors.text_primary)),
                    );
                    painter.rect_stroke(
                        cell_rect.expand(1.0),
                        Rounding::ZERO,
                        Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_recessed)),
                    );
                }
            }
        }

        // Color Swatch below grid
        let swatch_y = grid_origin.y + grid_w + 8.0;
        let swatch_rect = Rect::from_min_size(
            Pos2::new(card_min.x + card_padding, swatch_y),
            Vec2::new(grid_w, 16.0),
        );
        painter.rect_filled(swatch_rect, Rounding::same(3.0), col);
        painter.rect_stroke(
            swatch_rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
        );

        // Stacked rows remain legible without a long line overflowing
        // the compact card.
        let mut readout_y = swatch_y + 20.0;
        for (galley, color) in readout {
            let row_height = galley.size().y;
            painter.galley(
                Pos2::new(card_min.x + card_padding, readout_y),
                galley,
                color,
            );
            readout_y += row_height + 2.0;
        }
    }
}

pub fn schedule_system_clipboard_copy_on(
    jobs: &BackgroundJobs,
    text: String,
    copy: impl FnOnce(&str) + Send + 'static,
) {
    let _ = jobs.submit(std::time::Duration::MAX, move || copy(&text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_preview::ImagePreviewState;
    use little_clip_core::AppConfig;
    use std::sync::mpsc;

    fn state() -> ImagePreviewState {
        let mut bytes = Vec::new();
        image::RgbaImage::new(2, 2)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".into(),
            None,
            "pick color clipboard test".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn color_copy_keeps_immediate_egui_output_and_recent_color_with_bounded_write() {
        let jobs = BackgroundJobs::new(1, 1);
        let (text_tx, text_rx) = mpsc::channel();
        let context = egui::Context::default();
        let mut state = state();
        let color = Color32::from_rgb(0x12, 0x34, 0x56);
        let output = context.run(Default::default(), |ctx| {
            state.copy_color_to_clipboard_on(color, ctx, &jobs, move |text| {
                text_tx.send(text.to_owned()).unwrap();
            });
        });

        assert_eq!(output.platform_output.copied_text, "#123456");
        assert_eq!(state.pick_color_state.last_picked, Some(color));
        assert_eq!(state.pick_color_state.recent_colors, vec![color]);
        assert!(
            state
                .status_toast
                .as_ref()
                .is_some_and(|(toast, _)| toast.contains("#123456"))
        );
        assert_eq!(
            text_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            "#123456"
        );
    }

    #[test]
    fn rendered_copy_current_hex_uses_sampled_color_and_injected_clipboard_writer() {
        let jobs = BackgroundJobs::new(1, 1);
        let (text_tx, text_rx) = mpsc::channel();
        let context = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&context);
        let palette = AppPalette::default();
        let mut state = state();
        let color = Color32::from_rgb(0x12, 0x34, 0x56);
        state.pick_color_state.hovered_color = Some(color);
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
        let mut render = |events, copy: Box<dyn FnOnce(&str) + Send>| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_pick_color_results_with_copy(ui, ctx, &palette, &jobs, copy);
                    });
                },
            )
        };

        let initial = render(
            Vec::new(),
            Box::new(|_| panic!("initial frame cannot copy")),
        );
        let copy_position = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Copy current hex" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("rendered Copy current hex button label");
        render(
            vec![egui::Event::PointerMoved(copy_position)],
            Box::new(|_| panic!("hover frame cannot copy")),
        );
        render(
            vec![
                egui::Event::PointerMoved(copy_position),
                egui::Event::PointerButton {
                    pos: copy_position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            Box::new(|_| panic!("press frame cannot copy")),
        );
        let copied = render(
            vec![
                egui::Event::PointerMoved(copy_position),
                egui::Event::PointerButton {
                    pos: copy_position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            Box::new(move |text| text_tx.send(text.to_owned()).unwrap()),
        );

        assert_eq!(copied.platform_output.copied_text, "#123456");
        assert_eq!(
            text_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            "#123456"
        );
        assert_eq!(state.pick_color_state.last_picked, Some(color));
        assert_eq!(state.pick_color_state.recent_colors, vec![color]);
    }

    #[test]
    fn color_copy_queue_full_keeps_egui_clipboard_fallback_without_running_write() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let (copied_tx, copied_rx) = mpsc::channel();
        let context = egui::Context::default();
        let mut state = state();
        let color = Color32::from_rgb(0xAB, 0xCD, 0xEF);
        let output = context.run(Default::default(), |ctx| {
            state.copy_color_to_clipboard_on(color, ctx, &jobs, move |text| {
                copied_tx.send(text.to_owned()).unwrap();
            });
        });

        assert_eq!(output.platform_output.copied_text, "#ABCDEF");
        assert_eq!(state.pick_color_state.recent_colors, vec![color]);
        assert!(
            state
                .status_toast
                .as_ref()
                .is_some_and(|(toast, _)| toast.contains("#ABCDEF"))
        );
        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        assert!(
            copied_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err()
        );
    }
}

impl ImagePreviewState {
    pub(super) fn render_pick_color_results(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.render_pick_color_results_with_copy(
            ui,
            ctx,
            palette,
            BackgroundJobs::global(),
            |text| {
                if let Err(error) = little_clip_core::copy_text_to_system_clipboard(text) {
                    tracing::debug!(%error, "failed to copy picked color to system clipboard");
                }
            },
        );
    }

    fn render_pick_color_results_with_copy(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        jobs: &BackgroundJobs,
        copy: impl FnOnce(&str) + Send + 'static,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);

        ui.label(
            egui::RichText::new("Color picker")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Hover the image to inspect; click to keep a color.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
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
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
                );
            });
        });
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        let copy_hex = crate::action_button("Copy current hex", accent, palette);
        if ui
            .add_sized(
                [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                copy_hex,
            )
            .clicked()
        {
            self.copy_color_to_clipboard_on(current, ctx, jobs, copy);
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("PICKED COLORS")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(self.pick_color_state.picked_colors.len().to_string())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
            });
        });

        if self.pick_color_state.picked_colors.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.label(
                egui::RichText::new("No colors picked yet.")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .italics()
                    .color(muted),
            );
            return;
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let copy_all = crate::secondary_button("Copy all colors", surface, palette);
        if ui
            .add_sized(
                [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                copy_all,
            )
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
                    .size(crate::ui_ext::BODY_FONT_SIZE)
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
