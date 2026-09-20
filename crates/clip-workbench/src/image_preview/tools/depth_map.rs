use super::super::*;

impl ImagePreviewState {
    pub fn run_depth_estimation(&mut self) {
        if self.bg_removal_in_progress {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            self.bg_removal_error = Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        let depth_script = "scripts/depth_estimation.py";
        let (cmd_bin, cmd_args) = if std::path::Path::new(depth_script).exists() {
            (depth_script.to_string(), Vec::new())
        } else {
            ("python3".to_string(), vec![depth_script.to_string()])
        };

        let (tx, rx) = std::sync::mpsc::channel::<BgRemovalMsg>();
        self.bg_removal_rx = Some(rx);
        self.bg_removal_in_progress = true;
        self.bg_removal_status = Some("Estimating depth map with Depth-Anything...".to_string());
        self.bg_removal_error = None;

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let tx_prog = tx.clone();
            let _ = tx_prog.send(BgRemovalMsg::Progress(
                "Invoking depth estimation neural network...".to_string(),
            ));

            let mut cmd = Command::new(&cmd_bin);
            cmd.args(&cmd_args).arg("-").arg("-");

            let mut child = match cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to launch depth estimation '{}': {}. Ensure Python environment is setup.",
                        cmd_bin, e
                    )));
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to write image data to worker: {}",
                        e
                    )));
                    return;
                }
            }

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Depth estimation process failed: {}",
                        e
                    )));
                    return;
                }
            };

            if !output.status.success() {
                let err_str = String::from_utf8_lossy(&output.stderr);
                let _ = tx.send(BgRemovalMsg::Error(format!(
                    "Depth estimation failed:\n{}",
                    err_str.trim()
                )));
                return;
            }

            let res_img = match image::load_from_memory_with_format(
                &output.stdout,
                image::ImageFormat::Png,
            ) {
                Ok(dyn_img) => dyn_img.to_rgba8(),
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to decode output PNG: {}",
                        e
                    )));
                    return;
                }
            };

            let duration_ms = start.elapsed().as_millis();
            let _ = tx.send(BgRemovalMsg::Success {
                result_rgba: res_img,
                duration_ms,
            });
        });
    }

    /// Polls the background removal worker channel and applies result on success.

    pub(crate) fn render_depth_map_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
        }

        let b_low = hex_to_c32(&palette.theme.b_low);
        let b_med = hex_to_c32(&palette.theme.b_med);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);
        let f_low = hex_to_c32(&palette.theme.f_low);

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0_f32, b_med))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.bg_removal_in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(base_c));
                        ui.add_space(6.0);
                        let st = self.bg_removal_status.as_deref().unwrap_or("Estimating depth map with Depth-Anything...");
                        ui.label(
                            egui::RichText::new(st)
                                .small()
                                .monospace()
                                .strong()
                                .color(base_c),
                        );
                    } else {
                        let depth_fg = contrast_text_c32(base_c, None, palette);
                        let depth_btn = egui::Button::new(
                            egui::RichText::new("🌊 Generate Depth Map (Enter)")
                                .small()
                                .strong()
                                .color(depth_fg),
                        )
                        .fill(base_c)
                        .stroke(Stroke::new(1.0_f32, b_med))
                        .rounding(Rounding::same(4.0));

                        if ui.add(depth_btn).on_hover_text("Generate AI metric depth map with Depth-Anything V2 neural network (Enter)").clicked() {
                            self.run_depth_estimation();
                        }

                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Depth-Anything V2 • Metric Monocular Depth Estimation • CUDA")
                                .small()
                                .color(f_low),
                        );
                    }

                    if let Some(err) = &self.bg_removal_error {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .small()
                                .strong()
                                .color(diff_del),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(b_med, Some(diff_mod), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").small().strong().color(undo_fg),
                            )
                            .fill(b_med)
                            .rounding(Rounding::same(4.0));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }

                        // Direct Slot Push buttons
                        let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Right Hand").small().color(rh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0_f32, diff_add))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_rh).on_hover_text("Push current depth map directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let lh_fg = contrast_text_c32(b_low, Some(base_c), palette);
                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Left Hand").small().color(lh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0_f32, base_c))
                        .rounding(Rounding::same(3.0));
                        if ui.add(push_lh).on_hover_text("Push current depth map directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });
                });
            });
    }
}
