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

pub(super) fn remove_background_rgba(
    png_bytes: &[u8],
    configured_cmd: Option<&str>,
    prompt: Option<&str>,
) -> Result<RgbaImage, String> {
    let (cmd_bin, cmd_args) = if let Some(cmd) = configured_cmd {
        let parts: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
        if parts.is_empty() {
            ("scripts/remove_bg.py".to_string(), Vec::new())
        } else {
            (parts[0].clone(), parts[1..].to_vec())
        }
    } else if std::path::Path::new("scripts/remove_bg.py").exists() {
        ("scripts/remove_bg.py".to_string(), Vec::new())
    } else {
        (
            "python3".to_string(),
            vec!["scripts/remove_bg.py".to_string()],
        )
    };

    let mut cmd = Command::new(&cmd_bin);
    cmd.args(&cmd_args);
    if let Some(prompt) = prompt {
        cmd.arg("--prompt").arg(prompt);
    }
    cmd.arg("-").arg("-");

    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to launch background removal process '{}': {}. Ensure Python with nobg is installed.",
                cmd_bin, e
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(png_bytes)
            .map_err(|e| format!("Failed to write image data to worker: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Worker process failed: {}", e))?;
    if !output.status.success() {
        let err_str = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Background removal failed (exit code {:?}):\n{}",
            output.status.code(),
            err_str.trim()
        ));
    }

    image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png)
        .map(|image| image.to_rgba8())
        .map_err(|e| format!("Failed to decode output PNG: {}", e))
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

        let configured_cmd = self.bg_removal_cmd.clone();
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

            match remove_background_rgba(
                &png_bytes,
                configured_cmd.as_deref(),
                thread_prompt.as_deref(),
            ) {
                Ok(result_rgba) => {
                    let _ = tx.send(BgRemovalMsg::Success {
                        result_rgba,
                        duration_ms: start.elapsed().as_millis(),
                    });
                }
                Err(error) => {
                    let _ = tx.send(BgRemovalMsg::Error(error));
                }
            }
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
            self.creative_fx_state.cached_background_removal = Some(result_rgba.clone());
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
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let danger = hex_to_c32(&palette.diff_del);
        let has_prompt = !self.multimatte_prompt.trim().is_empty();
        let action_color = if has_prompt { guided_accent } else { accent };

        ui.label(
            egui::RichText::new("Remove background")
                .size(20.0)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new(
                "Create a transparent cutout automatically or describe what to keep.",
            )
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .color(muted),
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("SEGMENTATION")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
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
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(action_color),
                );
            });
        });
        ui.add_space(6.0);

        ui.label(
            egui::RichText::new("What should remain?")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(3.0);
        let prompt_response = ui.add_sized(
            [ui.available_width(), 132.0],
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
            .size(crate::ui_ext::BODY_FONT_SIZE)
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
                ui.add_sized(
                    [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                    run_button,
                )
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
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        if self.bg_removal_in_progress {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(action_color));
                ui.label(
                    egui::RichText::new("Working…")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
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
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
            );
        } else {
            ui.label(
                egui::RichText::new("Engine")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.label(
                egui::RichText::new(if has_prompt { "MultiMatte" } else { "FeyNoBg" })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(text),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Acceleration")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.label(
                egui::RichText::new("CUDA when available")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(text),
            );
        }

        if let Some(error) = self.bg_removal_error.clone() {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("BACKGROUND REMOVAL FAILED")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(danger),
            );
            ui.add_space(3.0);
            ui.label(
                egui::RichText::new(&error)
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(danger),
            );

            if error.contains("nobg") || error.contains("pip") {
                ui.add_space(6.0);
                let copy_install = crate::secondary_button("Copy setup command", surface, palette);
                if ui
                    .add_sized(
                        [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                        copy_install,
                    )
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
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(4.0);

        ui.vertical(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], left)
                .on_hover_text("Copy the current cutout to the Left Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Left);
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], right)
                .on_hover_text("Copy the current cutout to the Right Hand slot")
                .clicked()
            {
                self.copy_to_hand_slot(HandSide::Right);
            }
        });

        if !self.undo_stack.is_empty() {
            ui.add_space(6.0);
            let undo = crate::secondary_button("Undo last edit (Ctrl+Z)", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], undo)
                .clicked()
            {
                self.undo();
            }
        }
    }
}
