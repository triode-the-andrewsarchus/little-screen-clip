use super::super::*;

#[derive(Debug)]
pub enum BgRemovalMsg {
    Progress(String),
    Success {
        result_rgba: RgbaImage,
        duration_ms: u128,
    },
    Error(String),
}

impl ImagePreviewState {
    pub fn run_background_removal(&mut self) {
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

        // Determine executable command
        let (cmd_bin, cmd_args) = if let Some(ref cmd) = self.bg_removal_cmd {
            let parts: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
            if !parts.is_empty() {
                (parts[0].clone(), parts[1..].to_vec())
            } else {
                ("scripts/remove_bg.py".to_string(), Vec::new())
            }
        } else {
            let candidates = ["scripts/remove_bg.py", "./scripts/remove_bg.py"];
            let mut found = None;
            for path in &candidates {
                if std::path::Path::new(path).exists() {
                    found = Some((path.to_string(), Vec::new()));
                    break;
                }
            }
            found.unwrap_or_else(|| {
                (
                    "python3".to_string(),
                    vec!["scripts/remove_bg.py".to_string()],
                )
            })
        };

        let prompt_opt = if !self.multimatte_prompt.trim().is_empty() {
            Some(self.multimatte_prompt.trim().to_string())
        } else {
            None
        };
        let status_desc = if let Some(ref p) = prompt_opt {
            format!("Segmenting '{}' with MultiMatte...", p)
        } else {
            "Starting FeyNoBg AI model...".to_string()
        };

        let (tx, rx) = std::sync::mpsc::channel::<BgRemovalMsg>();
        self.bg_removal_rx = Some(rx);
        self.bg_removal_in_progress = true;
        self.bg_removal_status = Some(status_desc);
        self.bg_removal_error = None;

        let thread_prompt = prompt_opt.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let tx_prog = tx.clone();
            let init_msg = if let Some(ref p) = thread_prompt {
                format!("Invoking MultiMatte for '{}'...", p)
            } else {
                "Invoking FeyNoBg neural network...".to_string()
            };
            let _ = tx_prog.send(BgRemovalMsg::Progress(init_msg));

            let mut cmd = Command::new(&cmd_bin);
            cmd.args(&cmd_args);
            if let Some(ref p) = thread_prompt {
                cmd.arg("--prompt").arg(p);
            }
            cmd.arg("-").arg("-");

            let mut child = match cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to launch background removal process '{}': {}. Ensure Python with nobg is installed.",
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
                    let _ = tx.send(BgRemovalMsg::Error(format!("Worker process failed: {}", e)));
                    return;
                }
            };

            if !output.status.success() {
                let err_str = String::from_utf8_lossy(&output.stderr);
                let _ = tx.send(BgRemovalMsg::Error(format!(
                    "Background removal failed (exit code {:?}):\n{}",
                    output.status.code(),
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

    /// Runs AI depth estimation asynchronously using Depth-Anything / MiDaS.

    pub fn poll_bg_removal(&mut self) {
        let mut success_data = None;
        let mut error_msg = None;
        let mut disconnected = false;

        if let Some(rx) = &self.bg_removal_rx {
            loop {
                match rx.try_recv() {
                    Ok(BgRemovalMsg::Progress(st)) => {
                        self.bg_removal_status = Some(st);
                    }
                    Ok(BgRemovalMsg::Success {
                        result_rgba,
                        duration_ms,
                    }) => {
                        success_data = Some((result_rgba, duration_ms));
                        break;
                    }
                    Ok(BgRemovalMsg::Error(err)) => {
                        error_msg = Some(err);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        if let Some((result_rgba, duration_ms)) = success_data {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = result_rgba;
            self.dirty_texture = true;
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_error = None;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;

            self.show_toast(format!(
                "✨ Background removed in {:.2}s!",
                duration_ms as f64 / 1000.0
            ));
        } else if let Some(err) = error_msg {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_error = Some(err);
        } else if disconnected {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
        }
    }

    /// Opens a native save-as dialog (zenity) or falls back to an in-app path input modal.

    pub(crate) fn render_remove_bg_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.poll_bg_removal();
        if self.bg_removal_in_progress {
            ctx.request_repaint();
        }

        let accent = hex_to_c32(&palette.harmonies.base);
        let guided_accent = hex_to_c32(&palette.harmonies.triadic_2);
        let surface = hex_to_c32(&palette.theme.b_med);
        let text = hex_to_c32(&palette.theme.f_med);
        let muted = hex_to_c32(&palette.theme.f_low);
        let danger = hex_to_c32(&palette.diff_del);
        let has_prompt = !self.multimatte_prompt.trim().is_empty();
        let action_color = if has_prompt { guided_accent } else { accent };

        ui.label(
            egui::RichText::new("Remove background")
                .size(16.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new(
                "Create a transparent cutout automatically or describe what to keep.",
            )
            .size(11.5)
            .color(muted),
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("SEGMENTATION")
                    .size(12.0)
                    .strong()
                    .color(text),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(if has_prompt {
                        "Prompt guided"
                    } else {
                        "Automatic"
                    })
                    .size(11.5)
                    .strong()
                    .color(action_color),
                );
            });
        });
        ui.add_space(6.0);

        ui.label(
            egui::RichText::new("What should remain?")
                .size(13.0)
                .strong()
                .color(text),
        );
        ui.add_space(3.0);
        let prompt_response = ui.add_sized(
            [ui.available_width(), 92.0],
            egui::TextEdit::multiline(&mut self.multimatte_prompt)
                .hint_text("Optional prompt, for example:\nperson in the foreground\nred bicycle and its basket")
                .desired_width(f32::INFINITY)
                .desired_rows(5),
        );
        prompt_response.on_hover_text(
            "Leave this blank for automatic background removal. Use a prompt to keep a specific subject or concept.",
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(if has_prompt {
                "MultiMatte will isolate the described subject. Ctrl+Enter runs while editing."
            } else {
                "Leave blank to detect the foreground automatically with FeyNoBg."
            })
            .size(11.0)
            .color(muted),
        );

        ui.add_space(12.0);
        let run_label = if self.bg_removal_in_progress {
            "Removing background…"
        } else if has_prompt {
            "Segment described subject"
        } else {
            "Remove background"
        };
        let run_button = crate::action_button(run_label, action_color, palette);
        if ui
            .add_enabled_ui(!self.bg_removal_in_progress, |ui| {
                ui.add_sized([ui.available_width(), 32.0], run_button)
            })
            .inner
            .on_hover_text(if has_prompt {
                "Run prompt-guided MultiMatte segmentation"
            } else {
                "Run automatic FeyNoBg foreground detection"
            })
            .clicked()
        {
            self.run_background_removal();
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("PROCESSING")
                .size(12.0)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        if self.bg_removal_in_progress {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(action_color));
                ui.label(
                    egui::RichText::new("Working…")
                        .size(12.0)
                        .strong()
                        .color(action_color),
                );
            });
            ui.label(
                egui::RichText::new(
                    self.bg_removal_status
                        .as_deref()
                        .unwrap_or("Preparing the segmentation model…"),
                )
                .size(11.5)
                .color(muted),
            );
        } else {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Engine").size(12.0).color(muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(if has_prompt { "MultiMatte" } else { "FeyNoBg" })
                            .size(12.0)
                            .strong()
                            .color(text),
                    );
                });
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Acceleration").size(12.0).color(muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("CUDA when available")
                            .size(11.5)
                            .color(text),
                    );
                });
            });
        }

        if let Some(error) = self.bg_removal_error.clone() {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("BACKGROUND REMOVAL FAILED")
                    .size(12.0)
                    .strong()
                    .color(danger),
            );
            ui.add_space(3.0);
            ui.label(egui::RichText::new(&error).size(11.5).color(danger));

            if error.contains("nobg") || error.contains("pip") {
                ui.add_space(6.0);
                let copy_install = crate::secondary_button("Copy setup command", surface, palette);
                if ui
                    .add_sized([ui.available_width(), 30.0], copy_install)
                    .clicked()
                {
                    let install_cmd = "uv venv --python /usr/bin/python3 --system-site-packages ~/.local/share/little-screen-clip/nobg-venv && uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python nobg";
                    ctx.output_mut(|output| output.copied_text = install_cmd.to_string());
                    self.show_toast("Copied setup command to clipboard");
                }
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("OUTPUT")
                .size(12.0)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        let slot_width = ((ui.available_width() - 6.0) * 0.5).max(70.0);
        ui.horizontal(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_sized([slot_width, 28.0], left)
                .on_hover_text("Copy the current cutout to the Left Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Left);
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_sized([slot_width, 28.0], right)
                .on_hover_text("Copy the current cutout to the Right Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Right);
            }
        });

        if !self.undo_stack.is_empty() {
            ui.add_space(6.0);
            let undo = crate::secondary_button("Undo last edit (Ctrl+Z)", surface, palette);
            if ui.add_sized([ui.available_width(), 30.0], undo).clicked() {
                self.undo();
            }
        }
    }
}
