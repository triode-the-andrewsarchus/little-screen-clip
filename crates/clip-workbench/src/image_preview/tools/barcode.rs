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
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum BarcodeScanRequest {
    Program(std::path::PathBuf),
    #[cfg(test)]
    InProcess,
}

impl ImagePreviewState {
    pub fn run_barcode_scan(&mut self) {
        #[cfg(test)]
        {
            // Unit tests exercise the UI model without recursively launching libtest.
            self.run_barcode_scan_in_process_for_test();
            return;
        }
        #[cfg(not(test))]
        match std::env::current_exe() {
            Ok(program) => self.run_barcode_scan_with_program(program),
            Err(error) => self.show_toast(format!("Barcode scan failed: {error}")),
        }
    }

    #[cfg(test)]
    fn run_barcode_scan_in_process_for_test(&mut self) {
        self.run_barcode_scan_request_on(BarcodeScanRequest::InProcess, BackgroundJobs::global());
    }

    pub(crate) fn run_barcode_scan_with_program(&mut self, program: std::path::PathBuf) {
        self.run_barcode_scan_with_program_on(program, BackgroundJobs::global());
    }

    fn run_barcode_scan_with_program_on(
        &mut self,
        program: std::path::PathBuf,
        jobs: &BackgroundJobs,
    ) {
        self.run_barcode_scan_request_on(BarcodeScanRequest::Program(program), jobs);
    }

    fn run_barcode_scan_request_on(&mut self, request: BarcodeScanRequest, jobs: &BackgroundJobs) {
        if self.barcode_state.is_scanning {
            return;
        }
        let snapshot = self
            .barcode_retry_snapshot
            .take()
            .unwrap_or_else(|| self.display_image_snapshot_request());
        let retry_snapshot = snapshot.clone();
        let control = match &request {
            BarcodeScanRequest::Program(_) => {
                Some(std::sync::Arc::new(ImageToolProcessControl::new()))
            }
            #[cfg(test)]
            BarcodeScanRequest::InProcess => None,
        };
        self.barcode_cancel = control.clone();
        self.barcode_state.is_scanning = true;
        let input_stamp = snapshot.input_stamp.clone();
        self.barcode_input_stamp = Some(input_stamp);
        self.barcode_retry_at = None;
        self.barcode_retry_stamp = None;
        self.barcode_retry_request = None;
        self.barcode_queue_status = None;
        let worker_control = control.clone();
        let worker_request = request.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            let start = std::time::Instant::now();
            let image = snapshot.compose();
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match worker_request {
                    BarcodeScanRequest::Program(program) => worker_control
                        .as_ref()
                        .ok_or_else(|| "barcode process control is missing".to_string())
                        .and_then(|control| {
                            crate::barcode_worker::scan_in_child(image, &program, control)
                        }),
                    #[cfg(test)]
                    BarcodeScanRequest::InProcess => {
                        Ok(little_clip_ops::barcode::detect_barcodes(&image))
                    }
                }))
                .unwrap_or_else(|_| Err("Barcode scan worker panicked".to_string()));
            let (detections, error) = match result {
                Ok(detections) => (detections, None),
                Err(error) => (Vec::new(), Some(error)),
            };
            BarcodeScanMsg {
                detections,
                duration_ms: start.elapsed().as_millis(),
                error,
            }
        });
        self.barcode_ticket = Some(ticket);
        self.barcode_retry_request = Some(request);
        self.barcode_retry_snapshot = Some(retry_snapshot);
        self.barcode_rx = Some(rx);
    }

    pub(crate) fn cancel_barcode_scan(&mut self) {
        if let Some(ticket) = self.barcode_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.barcode_cancel.take() {
            control.cancel();
        }
        self.barcode_rx = None;
        self.barcode_input_stamp = None;
        self.barcode_state.is_scanning = false;
        self.barcode_retry_at = None;
        self.barcode_retry_stamp = None;
        self.barcode_retry_request = None;
        self.barcode_retry_snapshot = None;
        self.barcode_queue_status = None;
    }

    pub(crate) fn poll_barcode_scan(&mut self, ctx: &egui::Context) {
        self.poll_barcode_scan_on(ctx, BackgroundJobs::global());
    }

    fn poll_barcode_scan_on(&mut self, ctx: &egui::Context, jobs: &BackgroundJobs) {
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
        if (self.barcode_rx.is_some() && self.barcode_input_stamp.as_ref() != Some(&current_stamp))
            || (self.barcode_retry_at.is_some()
                && self.barcode_retry_stamp.as_ref() != Some(&current_stamp))
        {
            self.cancel_barcode_scan();
            return;
        }
        if let Some(retry_at) = self.barcode_retry_at {
            if std::time::Instant::now() < retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
                return;
            }
            if let Some(request) = self.barcode_retry_request.take() {
                self.run_barcode_scan_request_on(request, jobs);
            }
            return;
        }
        match self.barcode_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(Ok(message))) => {
                self.barcode_cancel = None;
                self.barcode_ticket = None;
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_retry_request = None;
                self.barcode_retry_snapshot = None;
                self.barcode_queue_status = None;
                self.barcode_state.is_scanning = false;
                if let Some(error) = message.error {
                    self.barcode_results_stamp = None;
                    self.barcode_state.detections.clear();
                    self.barcode_state.selected_idx = None;
                    self.barcode_state.has_scanned = false;
                    self.show_toast(format!("Barcode scan failed: {error}"));
                    return;
                }
                self.barcode_results_stamp = Some(current_stamp);
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
            Some(Ok(Err(little_clip_core::JobError::QueueFull))) => {
                self.barcode_cancel = None;
                self.barcode_ticket = None;
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_state.is_scanning = false;
                self.barcode_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.barcode_retry_stamp = Some(current_stamp);
                self.barcode_queue_status = Some("Workers busy; retrying scan…".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            Some(Ok(Err(little_clip_core::JobError::Cancelled))) => {
                self.barcode_cancel = None;
                self.barcode_ticket = None;
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_retry_request = None;
                self.barcode_retry_snapshot = None;
                self.barcode_state.is_scanning = false;
            }
            Some(Ok(Err(error))) => {
                self.barcode_cancel = None;
                self.barcode_ticket = None;
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_state.is_scanning = false;
                self.barcode_retry_request = None;
                self.barcode_retry_snapshot = None;
                self.show_toast(format!("Barcode scan failed: {error}"));
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) if self.barcode_state.is_scanning => {
                ctx.request_repaint();
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.barcode_cancel = None;
                self.barcode_ticket = None;
                self.barcode_rx = None;
                self.barcode_input_stamp = None;
                self.barcode_retry_request = None;
                self.barcode_retry_snapshot = None;
                self.barcode_state.is_scanning = false;
                self.barcode_results_stamp = None;
                self.barcode_state.detections.clear();
                self.barcode_state.selected_idx = None;
                self.barcode_state.has_scanned = false;
                self.show_toast("Barcode scan worker stopped unexpectedly");
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
                    } else if let Some(status) = self.barcode_queue_status.as_ref() {
                        (status.clone(), f_med, b_low)
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

#[cfg(all(test, unix))]
mod input_stamp_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
        tx.send(Ok(BarcodeScanMsg {
            detections: Vec::new(),
            duration_ms: 1,
            error: None,
        }))
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
        tx.send(Ok(BarcodeScanMsg {
            detections: Vec::new(),
            duration_ms: 2,
            error: None,
        }))
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
        let (tx, rx) =
            std::sync::mpsc::channel::<Result<BarcodeScanMsg, little_clip_core::JobError>>();
        state.barcode_rx = Some(rx);
        drop(tx);

        state.poll_barcode_scan(&ctx);

        assert!(!state.barcode_state.is_scanning);
        assert!(state.barcode_rx.is_none());
        assert!(state.barcode_input_stamp.is_none());
    }

    #[test]
    fn mode_exit_cancels_scan_and_reentry_accepts_current_result() {
        let directory = tempfile::tempdir().unwrap();
        let slow = directory.path().join("slow.sh");
        std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o700)).unwrap();
        let quick = directory.path().join("quick.sh");
        std::fs::write(&quick, "#!/bin/sh\nprintf '[]' > \"$3\"\n").unwrap();
        std::fs::set_permissions(&quick, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut state = state();
        state.set_tool_mode(ImageToolMode::Barcode);
        state.run_barcode_scan_with_program(slow);
        assert!(state.barcode_state.is_scanning);
        state.set_tool_mode(ImageToolMode::View);
        assert!(!state.barcode_state.is_scanning);
        assert!(state.barcode_rx.is_none());
        state.set_tool_mode(ImageToolMode::Barcode);
        state.run_barcode_scan_with_program(quick);
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state.barcode_state.is_scanning && std::time::Instant::now() < deadline {
            state.poll_barcode_scan(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(state.barcode_state.has_scanned);
        assert!(state.barcode_state.detections.is_empty());
    }

    #[test]
    fn queue_full_preserves_results_and_retries_fake_worker_after_delay() {
        let directory = tempfile::tempdir().unwrap();
        let quick = directory.path().join("quick.sh");
        std::fs::write(&quick, "#!/bin/sh\nprintf '[]' > \"$3\"\n").unwrap();
        std::fs::set_permissions(&quick, std::fs::Permissions::from_mode(0o700)).unwrap();

        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());

        let mut state = state();
        state.barcode_state.has_scanned = true;
        state.barcode_state.last_scan_duration_ms = 23;
        state.barcode_state.detections = vec![BarcodeDetection {
            format: "QR Code".to_owned(),
            text: "preserved payload".to_owned(),
            points: vec![],
            bbox: [0.0; 4],
            is_url: false,
        }];
        state.barcode_results_stamp = Some(state.input_stamp());
        state.run_barcode_scan_with_program_on(quick, &jobs);
        let retained_pixels =
            std::sync::Arc::clone(&state.barcode_retry_snapshot.as_ref().unwrap().pixels);
        let ctx = egui::Context::default();
        state.poll_barcode_scan_on(&ctx, &jobs);

        assert!(!state.barcode_state.is_scanning);
        assert!(state.barcode_state.has_scanned);
        assert_eq!(state.barcode_state.last_scan_duration_ms, 23);
        assert_eq!(state.barcode_state.detections[0].text, "preserved payload");
        assert!(state.barcode_retry_at.is_some());
        assert!(std::sync::Arc::ptr_eq(
            &retained_pixels,
            &state.barcode_retry_snapshot.as_ref().unwrap().pixels
        ));
        assert!(
            state
                .barcode_queue_status
                .as_deref()
                .is_some_and(|status| status.contains("busy"))
        );

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        state.barcode_retry_at = Some(std::time::Instant::now());
        state.poll_barcode_scan_on(&ctx, &jobs);
        assert!(state.barcode_state.is_scanning);
        assert!(state.barcode_retry_at.is_none());
        assert!(std::sync::Arc::ptr_eq(
            &retained_pixels,
            &state.barcode_retry_snapshot.as_ref().unwrap().pixels
        ));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state.barcode_state.is_scanning && std::time::Instant::now() < deadline {
            state.poll_barcode_scan_on(&ctx, &jobs);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        state.poll_barcode_scan_on(&ctx, &jobs);
        assert!(!state.barcode_state.is_scanning);
        assert!(state.barcode_state.has_scanned);
        assert!(state.barcode_queue_status.is_none());
    }

    #[test]
    fn queued_fake_worker_is_cancelled_before_launch_on_viewer_drop() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("ran");
        let program = directory.path().join("mark.sh");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch {:?}\nprintf '[]' > \"$3\"\n", marker),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();

        let jobs = BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, queued_rx) = jobs.submit(std::time::Duration::MAX, || ());
        let mut state = state();
        state.run_barcode_scan_with_program_on(program, &jobs);
        let control = state.barcode_cancel.as_ref().unwrap().clone();
        let worker_rx = state.barcode_rx.take().unwrap();
        drop(state);
        assert!(control.cancel.load(std::sync::atomic::Ordering::Acquire));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        queued_rx.recv().unwrap().unwrap();
        assert!(matches!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn image_change_and_viewer_drop_cancel_active_runs() {
        let directory = tempfile::tempdir().unwrap();
        let slow = directory.path().join("slow.sh");
        let pid_path = directory.path().join("pid");
        std::fs::write(
            &slow,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > {:?}\nexec sleep 30\n",
                pid_path
            ),
        )
        .unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut changed = state();
        changed.run_barcode_scan_with_program(slow.clone());
        let changed_control = changed.barcode_cancel.as_ref().unwrap().clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !pid_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            pid_path.exists(),
            "helper did not start before source change"
        );
        let pid: i32 = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        changed.mark_pixels_changed();
        changed.poll_barcode_scan(&egui::Context::default());
        assert!(
            changed_control
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(!changed.barcode_state.is_scanning);
        assert!(!changed.barcode_state.has_scanned);
        assert!(changed.barcode_rx.is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);

        let mut dropped = state();
        dropped.run_barcode_scan_with_program(slow);
        let dropped_control = dropped.barcode_cancel.as_ref().unwrap().clone();
        drop(dropped);
        assert!(
            dropped_control
                .cancel
                .load(std::sync::atomic::Ordering::Acquire)
        );
    }
}
