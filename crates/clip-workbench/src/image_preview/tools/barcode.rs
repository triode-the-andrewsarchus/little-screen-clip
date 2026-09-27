use super::super::*;

#[derive(Debug, Clone, Default)]
pub struct BarcodeScanState {
    pub detections: Vec<BarcodeDetection>,
    pub selected_idx: Option<usize>,
    pub is_scanning: bool,
    pub has_scanned: bool,
    pub last_scan_duration_ms: u128,
}

/// Result sent by the barcode worker. Keeping decoding off the egui thread is
/// important for large photographs, where multi-format detection is expensive.
#[derive(Debug)]
pub struct BarcodeScanMsg {
    detections: Vec<BarcodeDetection>,
    duration_ms: u128,
}

impl ImagePreviewState {
    pub fn run_barcode_scan(&mut self) {
        if self.barcode_state.is_scanning {
            return;
        }
        self.barcode_state.is_scanning = true;
        let base_img = self.current_display_image();
        self.barcode_input_stamp = Some(self.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let detections = little_clip_ops::barcode::detect_barcodes(&base_img);
            let _ = tx.send(BarcodeScanMsg {
                detections,
                duration_ms: start.elapsed().as_millis(),
            });
        });
        self.barcode_rx = Some(rx);
    }

    pub(crate) fn poll_barcode_scan(&mut self, ctx: &egui::Context) {
        let current_stamp = self.input_stamp();
        if self
            .barcode_results_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &current_stamp)
        {
            self.barcode_results_stamp = None;
            self.barcode_state.detections.clear();
            self.barcode_state.selected_idx = None;
            self.barcode_state.has_scanned = false;
            self.barcode_state.last_scan_duration_ms = 0;
        }
        if self.barcode_rx.is_some() && self.barcode_input_stamp.as_ref() != Some(&current_stamp) {
            self.barcode_rx = None;
            self.barcode_input_stamp = None;
            self.barcode_state.is_scanning = false;
            return;
        }
        match self.barcode_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(message)) => {
                self.barcode_results_stamp = self.barcode_input_stamp.clone();
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_state.is_scanning = false;
                self.barcode_state.has_scanned = true;
                self.barcode_state.last_scan_duration_ms = message.duration_ms;
                let count = message.detections.len();
                self.barcode_state.detections = message.detections;
                self.barcode_state.selected_idx = (count > 0).then_some(0);
                if count > 0 {
                    self.show_toast(format!(
                        "Found {} code{} in {}ms",
                        count,
                        if count == 1 { "" } else { "s" },
                        self.barcode_state.last_scan_duration_ms
                    ));
                } else {
                    self.show_toast("No barcodes or QR codes found");
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) if self.barcode_state.is_scanning => {
                ctx.request_repaint();
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_state.is_scanning = false;
            }
            _ => {}
        }
    }

    pub(crate) fn render_barcode_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.poll_barcode_scan(ctx);
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let rescan_fg = contrast_text_c32(base_c, None, palette);
                    let rescan_btn = egui::Button::new(
                        egui::RichText::new(if self.barcode_state.is_scanning {
                            "Scanning…"
                        } else {
                            "Rescan (Enter)"
                        })
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(rescan_fg),
                    )
                    .fill(base_c)
                    .stroke(Stroke::new(1.0_f32, b_med))
                    .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                    .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                    if ui
                        .add_enabled(!self.barcode_state.is_scanning, rescan_btn)
                        .on_hover_text(
                            "Scan current image for QR codes, Data Matrix, and barcodes (Enter)",
                        )
                        .clicked()
                    {
                        self.run_barcode_scan();
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    let count = self.barcode_state.detections.len();
                    let (badge_txt, fg, bg) = if self.barcode_state.is_scanning {
                        ("Scanning image…".to_string(), base_c, b_low)
                    } else if count > 0 {
                        (
                            format!(
                                "{} Codes Detected ({}ms)",
                                count, self.barcode_state.last_scan_duration_ms
                            ),
                            base_c,
                            b_low,
                        )
                    } else if self.barcode_state.has_scanned {
                        ("No Barcodes Detected".to_string(), f_med, b_low)
                    } else {
                        ("Ready to Scan".to_string(), f_med, b_low)
                    };
                    chip_badge(ui, &badge_txt, fg, bg);

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new("QR • DataMatrix • Aztec • PDF417 • Code 128 • EAN-13")
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(f_low),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.barcode_state.detections.is_empty() {
                            let copy_fg = contrast_text_c32(b_med, Some(f_med), palette);
                            let copy_all_btn = egui::Button::new(
                                egui::RichText::new("📋 Copy All")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(copy_fg),
                            )
                            .fill(b_med)
                            .stroke(Stroke::new(1.0_f32, b_low))
                            .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                            .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                            if ui.add(copy_all_btn).clicked() {
                                let combined: Vec<String> = self
                                    .barcode_state
                                    .detections
                                    .iter()
                                    .map(|d| d.text.clone())
                                    .collect();
                                ctx.output_mut(|o| o.copied_text = combined.join("\n"));
                                self.show_toast("📋 Copied all barcode payloads to clipboard!");
                            }
                        }
                    });
                });

                if !self.barcode_state.detections.is_empty() {
                    ui.add_space(crate::ui_ext::SPACE_XS);
                    egui::ScrollArea::horizontal()
                        .max_height(75.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (idx, det) in
                                    self.barcode_state.detections.clone().iter().enumerate()
                                {
                                    let is_selected = self.barcode_state.selected_idx == Some(idx);
                                    let card_border = if is_selected { diff_mod } else { b_med };

                                    egui::Frame::none()
                                        .fill(b_low)
                                        .inner_margin(egui::Margin::symmetric(
                                            crate::ui_ext::SPACE_SM,
                                            crate::ui_ext::SPACE_XS,
                                        ))
                                        .stroke(Stroke::new(1.0_f32, card_border))
                                        .rounding(Rounding::same(
                                            crate::ui_ext::CONTROL_CORNER_RADIUS,
                                        ))
                                        .show(ui, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                                chip_badge(ui, &det.format, base_c, b_low);

                                                let display_text = if det.text.chars().count() > 36
                                                {
                                                    format!(
                                                        "{}…",
                                                        det.text
                                                            .chars()
                                                            .take(36)
                                                            .collect::<String>()
                                                    )
                                                } else {
                                                    det.text.clone()
                                                };
                                                let text_fg =
                                                    contrast_text_c32(b_low, Some(f_high), palette);
                                                ui.label(
                                                    egui::RichText::new(&display_text)
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .monospace()
                                                        .color(text_fg),
                                                )
                                                .on_hover_text(&det.text);

                                                if det.is_url {
                                                    let url_fg = contrast_text_c32(
                                                        b_med,
                                                        Some(base_c),
                                                        palette,
                                                    );
                                                    let url_btn = egui::Button::new(
                                                        egui::RichText::new("🌐 Open")
                                                            .size(crate::ui_ext::BODY_FONT_SIZE)
                                                            .color(url_fg),
                                                    )
                                                    .fill(b_med)
                                                    .rounding(Rounding::same(
                                                        crate::ui_ext::BADGE_CORNER_RADIUS,
                                                    ))
                                                    .min_size(Vec2::new(
                                                        0.0,
                                                        crate::ui_ext::CONTROL_HEIGHT,
                                                    ));
                                                    if ui.add(url_btn).clicked() {
                                                        ctx.open_url(
                                                            egui::output::OpenUrl::new_tab(
                                                                det.text.clone(),
                                                            ),
                                                        );
                                                    }
                                                }

                                                let copy_btn =
                                                    crate::secondary_button("📋", b_med, palette)
                                                        .rounding(Rounding::same(
                                                            crate::ui_ext::BADGE_CORNER_RADIUS,
                                                        ));
                                                if ui
                                                    .add(copy_btn)
                                                    .on_hover_text("Copy this code")
                                                    .clicked()
                                                {
                                                    ctx.output_mut(|o| {
                                                        o.copied_text = det.text.clone()
                                                    });
                                                    self.show_toast(
                                                        "📋 Copied code payload to clipboard!",
                                                    );
                                                }

                                                let rh_fg = contrast_text_c32(
                                                    b_low,
                                                    Some(diff_add),
                                                    palette,
                                                );
                                                let push_btn = egui::Button::new(
                                                    egui::RichText::new("RH")
                                                        .size(crate::ui_ext::BODY_FONT_SIZE)
                                                        .color(rh_fg),
                                                )
                                                .fill(b_low)
                                                .stroke(Stroke::new(1.0_f32, diff_add))
                                                .rounding(Rounding::same(
                                                    crate::ui_ext::BADGE_CORNER_RADIUS,
                                                ))
                                                .min_size(Vec2::new(
                                                    0.0,
                                                    crate::ui_ext::CONTROL_HEIGHT,
                                                ));
                                                if ui
                                                    .add(push_btn)
                                                    .on_hover_text("Push to Right Hand slot")
                                                    .clicked()
                                                {
                                                    self.push_text_to_hand_slot(
                                                        HandSide::Right,
                                                        det.text.clone(),
                                                    );
                                                }
                                            });
                                        });
                                }
                            });
                        });
                }
            });
    }

    pub(crate) fn render_barcode_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        if self.barcode_results_stamp.as_ref() != Some(&self.input_stamp()) {
            return;
        }
        let ptr_pos = response.hover_pos();
        let clicked = response.clicked();
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let f_high = hex_to_c32(&palette.colors.text_primary);

        for (idx, det) in self.barcode_state.detections.iter().enumerate() {
            let is_selected = self.barcode_state.selected_idx == Some(idx);
            let b = det.bbox;
            let scr_min = Pos2::new(
                img_rect.min.x + b[0] * img_rect.width(),
                img_rect.min.y + b[1] * img_rect.height(),
            );
            let scr_max = Pos2::new(
                img_rect.min.x + b[2] * img_rect.width(),
                img_rect.min.y + b[3] * img_rect.height(),
            );
            let code_rect = Rect::from_min_max(scr_min, scr_max);

            let hovered = ptr_pos.map(|p| code_rect.contains(p)).unwrap_or(false);

            let (fill, border, stroke_w) = if is_selected || hovered {
                (hex_to_c32_alpha(&palette.diff_mod, 50), diff_mod, 2.5_f32)
            } else {
                (
                    hex_to_c32_alpha(&palette.harmonies.base, 30),
                    base_c,
                    1.5_f32,
                )
            };

            if det.points.len() >= 3 {
                let pts: Vec<Pos2> = det
                    .points
                    .iter()
                    .map(|p| {
                        Pos2::new(
                            img_rect.min.x + p[0] * img_rect.width(),
                            img_rect.min.y + p[1] * img_rect.height(),
                        )
                    })
                    .collect();
                for i in 0..pts.len() {
                    let next = pts[(i + 1) % pts.len()];
                    painter.line_segment([pts[i], next], Stroke::new(stroke_w, border));
                }
                painter.rect_filled(code_rect, Rounding::same(3.0), fill);
            } else {
                painter.rect_filled(code_rect, Rounding::same(4.0), fill);
                painter.rect_stroke(
                    code_rect,
                    Rounding::same(4.0),
                    Stroke::new(stroke_w, border),
                );
            }

            let badge_pos = Pos2::new(
                code_rect.min.x,
                (code_rect.min.y - 18.0).max(img_rect.min.y),
            );
            let badge_rect = Rect::from_min_size(badge_pos, Vec2::new(75.0, 16.0));
            let badge_text_col = contrast_text_c32(b_low, Some(f_high), palette);
            painter.rect_filled(badge_rect, Rounding::same(3.0), b_low);
            painter.rect_stroke(
                badge_rect,
                Rounding::same(3.0),
                Stroke::new(1.0_f32, border),
            );
            painter.text(
                badge_rect.center(),
                egui::Align2::CENTER_CENTER,
                &det.format,
                egui::FontId::proportional(9.0),
                badge_text_col,
            );

            if hovered {
                egui::show_tooltip_at_pointer(
                    ui.ctx(),
                    egui::Id::new(format!("barcode_tip_{}", idx)),
                    |ui| {
                        ui.label(
                            egui::RichText::new(format!("Format: {}", det.format))
                                .strong()
                                .color(base_c),
                        );
                        ui.label(egui::RichText::new(&det.text).monospace());
                        if det.is_url {
                            let triadic_1 = hex_to_c32(&palette.harmonies.triadic_1);
                            ui.label(
                                egui::RichText::new("🌐 Click in toolbar to open URL")
                                    .size(crate::ui_ext::BODY_FONT_SIZE)
                                    .color(triadic_1),
                            );
                        }
                    },
                );

                if clicked {
                    self.barcode_state.selected_idx = Some(idx);
                }
            }
        }
    }
}

#[cfg(test)]
mod input_stamp_tests {
    use super::*;

    fn state() -> ImagePreviewState {
        let mut bytes = Vec::new();
        RgbaImage::new(4, 4)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".into(),
            None,
            "Barcode stamp test".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn barcode_scan_discards_stale_and_accepts_current_results() {
        let ctx = egui::Context::default();
        let mut stale = state();
        stale.barcode_state.is_scanning = true;
        stale.barcode_input_stamp = Some(stale.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel();
        stale.barcode_rx = Some(rx);
        tx.send(BarcodeScanMsg {
            detections: Vec::new(),
            duration_ms: 1,
        })
        .unwrap();
        stale.mark_pixels_changed();

        stale.poll_barcode_scan(&ctx);

        assert!(!stale.barcode_state.is_scanning);
        assert!(!stale.barcode_state.has_scanned);
        assert!(stale.barcode_rx.is_none());

        let mut current = state();
        current.barcode_state.is_scanning = true;
        current.barcode_input_stamp = Some(current.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel();
        current.barcode_rx = Some(rx);
        tx.send(BarcodeScanMsg {
            detections: Vec::new(),
            duration_ms: 2,
        })
        .unwrap();

        current.poll_barcode_scan(&ctx);

        assert!(!current.barcode_state.is_scanning);
        assert!(current.barcode_state.has_scanned);
        assert_eq!(current.barcode_state.last_scan_duration_ms, 2);
        assert!(current.barcode_rx.is_none());
    }

    #[test]
    fn disconnected_barcode_worker_clears_loading_state() {
        let ctx = egui::Context::default();
        let mut state = state();
        state.barcode_state.is_scanning = true;
        state.barcode_input_stamp = Some(state.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel::<BarcodeScanMsg>();
        state.barcode_rx = Some(rx);
        drop(tx);

        state.poll_barcode_scan(&ctx);

        assert!(!state.barcode_state.is_scanning);
        assert!(state.barcode_rx.is_none());
        assert!(state.barcode_input_stamp.is_none());
    }
}
