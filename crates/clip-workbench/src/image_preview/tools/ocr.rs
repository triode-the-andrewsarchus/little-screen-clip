use super::super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OcrEngine {
    #[default]
    GlmOcr,
    Tesseract,
}

impl OcrEngine {
    pub fn label(&self) -> &'static str {
        match self {
            OcrEngine::GlmOcr => "GLM-OCR (Default • 0.9B VLM)",
            OcrEngine::Tesseract => "Tesseract (Quick Local)",
        }
    }

    pub fn arg_name(&self) -> &'static str {
        match self {
            OcrEngine::GlmOcr => "glm-ocr",
            OcrEngine::Tesseract => "tesseract",
        }
    }
}

#[derive(Debug)]
pub enum OcrMsg {
    Progress(String),
    Success {
        text: String,
        engine: String,
        duration_ms: u128,
    },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct OcrState {
    pub engine: OcrEngine,
    pub custom_prompt: String,
    pub extracted_text: String,
    pub in_progress: bool,
    pub status_text: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<u128>,
}

impl Default for OcrState {
    fn default() -> Self {
        Self {
            engine: OcrEngine::GlmOcr,
            custom_prompt: String::new(),
            extracted_text: String::new(),
            in_progress: false,
            status_text: None,
            error: None,
            duration_ms: None,
        }
    }
}

impl ImagePreviewState {
    pub fn run_ocr(&mut self) {
        if self.ocr_state.in_progress {
            return;
        }
        let base_img = self.current_display_image();
        let mut png_bytes = Vec::new();
        if let Err(e) = base_img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        ) {
            self.ocr_state.error = Some(format!("Failed to encode image to PNG: {}", e));
            return;
        }

        let engine = self.ocr_state.engine;
        let engine_arg = engine.arg_name().to_string();
        let custom_prompt = if self.ocr_state.custom_prompt.trim().is_empty() {
            None
        } else {
            Some(self.ocr_state.custom_prompt.trim().to_string())
        };

        let (tx, rx) = std::sync::mpsc::channel::<OcrMsg>();
        self.ocr_rx = Some(rx);
        self.ocr_state.in_progress = true;
        self.ocr_state.error = None;
        self.ocr_state.status_text = Some(format!("Starting {}...", engine.label()));
        let child_pid_arc = self.ocr_child_pid.clone();

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let _ = tx.send(OcrMsg::Progress(format!(
                "Launching OCR engine ({})...",
                engine_arg
            )));

            let script_candidates = ["scripts/ocr.py", "./scripts/ocr.py"];
            let mut script_path = "scripts/ocr.py".to_string();
            for p in &script_candidates {
                if std::path::Path::new(p).exists() {
                    script_path = p.to_string();
                    break;
                }
            }

            let venv_python = std::env::var("HOME")
                .map(|h| format!("{}/.local/share/little-screen-clip/nobg-venv/bin/python", h))
                .unwrap_or_default();
            let python_bin = if std::path::Path::new(&venv_python).exists() {
                venv_python
            } else {
                "python3".to_string()
            };

            let mut cmd = Command::new(python_bin);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            cmd.arg(&script_path);
            cmd.arg("-").arg("-");
            cmd.arg("--engine").arg(&engine_arg);
            if let Some(ref p) = custom_prompt {
                cmd.arg("--prompt").arg(p);
            }

            let mut child = match cmd
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(OcrMsg::Error(format!(
                        "Failed to spawn python3 {}: {}",
                        script_path, e
                    )));
                    return;
                }
            };

            let pid = child.id();
            child_pid_arc.store(pid, std::sync::atomic::Ordering::SeqCst);

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    child_pid_arc.store(0, std::sync::atomic::Ordering::SeqCst);
                    let _ = tx.send(OcrMsg::Error(format!(
                        "Failed to pipe image to OCR worker: {}",
                        e
                    )));
                    return;
                }
            }

            let stdout_handle = child.stdout.take();
            let stderr_handle = child.stderr.take();

            let (stdout_tx, stdout_rx) = std::sync::mpsc::channel::<Vec<u8>>();
            if let Some(mut stdout) = stdout_handle {
                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut out = Vec::new();
                    let _ = stdout.read_to_end(&mut out);
                    let _ = stdout_tx.send(out);
                });
            }

            let (stderr_tx, stderr_rx) = std::sync::mpsc::channel::<String>();
            let tx_progress = tx.clone();
            if let Some(stderr) = stderr_handle {
                std::thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let reader = BufReader::new(stderr);
                    let mut all_stderr = Vec::new();
                    for line in reader.lines().flatten() {
                        if line.starts_with("[ocr:progress]") {
                            let msg = line.trim_start_matches("[ocr:progress]").trim().to_string();
                            let _ = tx_progress.send(OcrMsg::Progress(msg));
                        } else {
                            all_stderr.push(line);
                        }
                    }
                    let _ = stderr_tx.send(all_stderr.join("\n"));
                });
            }

            let status = match child.wait() {
                Ok(s) => {
                    child_pid_arc.store(0, std::sync::atomic::Ordering::SeqCst);
                    s
                }
                Err(e) => {
                    child_pid_arc.store(0, std::sync::atomic::Ordering::SeqCst);
                    let _ = tx.send(OcrMsg::Error(format!(
                        "Error waiting for OCR worker: {}",
                        e
                    )));
                    return;
                }
            };

            let stdout_bytes = stdout_rx.recv().unwrap_or_default();
            let stderr_text = stderr_rx.recv().unwrap_or_default();

            let duration_ms = start.elapsed().as_millis();
            if status.success() {
                let text = String::from_utf8_lossy(&stdout_bytes).to_string();
                if text.trim().is_empty() {
                    let err_msg = if !stderr_text.trim().is_empty() {
                        format!(
                            "OCR finished with no recognized text. Diagnostics:\n{}",
                            stderr_text
                        )
                    } else {
                        "OCR engine returned empty output (no text recognized in image)."
                            .to_string()
                    };
                    eprintln!("[OCR Warning] {}", err_msg);
                    let _ = tx.send(OcrMsg::Error(err_msg));
                } else {
                    let _ = tx.send(OcrMsg::Success {
                        text,
                        engine: engine_arg,
                        duration_ms,
                    });
                }
            } else {
                let err_detail = if !stderr_text.trim().is_empty() {
                    stderr_text
                } else {
                    format!("Process exited with status code: {:?}", status.code())
                };
                eprintln!("[OCR Error] {}", err_detail);
                let _ = tx.send(OcrMsg::Error(format!("OCR worker failed: {}", err_detail)));
            }
        });
    }

    pub fn poll_ocr(&mut self) {
        if let Some(ref rx) = self.ocr_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    OcrMsg::Progress(status) => {
                        self.ocr_state.status_text = Some(status);
                    }
                    OcrMsg::Success {
                        text,
                        engine,
                        duration_ms,
                    } => {
                        self.ocr_state.extracted_text = text;
                        self.ocr_state.duration_ms = Some(duration_ms);
                        self.ocr_state.in_progress = false;
                        self.ocr_state.status_text = None;
                        self.ocr_rx = None;
                        self.ocr_child_pid
                            .store(0, std::sync::atomic::Ordering::SeqCst);
                        self.show_toast(format!(
                            "✔ OCR complete with {} in {:.2}s",
                            engine,
                            duration_ms as f64 / 1000.0
                        ));
                        break;
                    }
                    OcrMsg::Error(err) => {
                        self.ocr_state.error = Some(err);
                        self.ocr_state.in_progress = false;
                        self.ocr_state.status_text = None;
                        self.ocr_rx = None;
                        self.ocr_child_pid
                            .store(0, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            }
        }
    }

    pub fn cancel_ocr(&mut self) {
        let pid = self
            .ocr_child_pid
            .swap(0, std::sync::atomic::Ordering::SeqCst);
        if pid != 0 {
            #[cfg(unix)]
            unsafe {
                // Send SIGTERM then SIGKILL to the entire process group
                libc::kill(-(pid as i32), libc::SIGTERM);
                libc::kill(-(pid as i32), libc::SIGKILL);
                // Also signal the process directly
                libc::kill(pid as i32, libc::SIGTERM);
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        if self.ocr_state.in_progress {
            self.ocr_state.in_progress = false;
            self.ocr_state.status_text = None;
            self.ocr_rx = None;
            self.show_toast("⏹ OCR process cancelled");
        }
    }

    pub fn push_text_to_hand_slot(&mut self, hand: HandSide, text: String) {
        let req = IpcRequest::PushText {
            text: text.clone(),
            hand,
        };
        let _ = crate::send_ipc_request(&req);
        let text_for_cmd = text;
        std::thread::spawn(move || {
            if let Err(error) = little_clip_core::copy_text_to_system_clipboard(&text_for_cmd) {
                tracing::debug!(%error, "failed to copy OCR text to system clipboard");
            }
        });
        self.show_toast(format!(
            "📋 Pushed to {} Hand slot",
            match hand {
                HandSide::Left => "Left",
                HandSide::Right => "Right",
            }
        ));
    }

    pub(crate) fn render_ocr_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.poll_ocr();
        if self.ocr_state.in_progress {
            ctx.request_repaint();
        }

        let accent = hex_to_c32(&palette.harmonies.triadic_1);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let border = hex_to_c32(&palette.colors.surface_recessed);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let danger = hex_to_c32(&palette.diff_del);

        ui.label(
            egui::RichText::new("Extract text")
                .size(crate::ui_ext::HEADING_FONT_SIZE)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new(
                "Extract editable text from the image or guide the model with a prompt.",
            )
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OCR ENGINE")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let engine_width = ((ui.available_width() - 6.0) * 0.5).max(76.0);
        ui.horizontal_wrapped(|ui| {
            for (engine, label, tooltip) in [
                (
                    OcrEngine::GlmOcr,
                    "GLM-OCR",
                    "Vision-language OCR for structured and difficult images",
                ),
                (
                    OcrEngine::Tesseract,
                    "Tesseract",
                    "Fast local OCR for clean printed text",
                ),
            ] {
                let active = self.ocr_state.engine == engine;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(4.0));
                if ui
                    .add_enabled_ui(!self.ocr_state.in_progress, |ui| {
                        ui.add_sized([engine_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    })
                    .inner
                    .on_hover_text(tooltip)
                    .clicked()
                {
                    self.ocr_state.engine = engine;
                }
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(match self.ocr_state.engine {
                OcrEngine::GlmOcr => "Best for layouts, handwriting, and contextual recognition.",
                OcrEngine::Tesseract => "Fastest for clean, high-contrast printed text.",
            })
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("Recognition prompt")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(3.0);
        ui.add_sized(
            [ui.available_width(), 120.0],
            egui::TextEdit::multiline(&mut self.ocr_state.custom_prompt)
                .font(egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                .hint_text("Optional instructions, for example:\nPreserve the table layout\nReturn only handwritten notes")
                .desired_width(f32::INFINITY)
                .desired_rows(5),
        )
        .on_hover_text("Tell the OCR engine what to recognize or how to structure its response");
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(
                "Leave blank for general text recognition. Ctrl+Enter runs while editing.",
            )
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        if self.ocr_state.in_progress {
            let cancel = crate::danger_button("Cancel OCR (Esc)", danger, palette);
            if ui
                .add_sized(
                    [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                    cancel,
                )
                .on_hover_text("Stop OCR and release the worker process")
                .clicked()
            {
                self.cancel_ocr();
            }
        } else {
            let run = crate::action_button("Run OCR", accent, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], run)
                .on_hover_text("Recognize text using the selected engine")
                .clicked()
            {
                self.run_ocr();
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("PROCESSING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        if self.ocr_state.in_progress {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(accent));
                ui.label(
                    egui::RichText::new("Recognizing text…")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(accent),
                );
            });
            ui.label(
                egui::RichText::new(
                    self.ocr_state
                        .status_text
                        .as_deref()
                        .unwrap_or("Preparing the OCR model…"),
                )
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
            );
        } else {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Selected")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(match self.ocr_state.engine {
                            OcrEngine::GlmOcr => "GLM-OCR 0.9B",
                            OcrEngine::Tesseract => "Tesseract",
                        })
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(text),
                    );
                });
            });
            ui.label(
                egui::RichText::new("The recognized text will appear in the output pane.")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
        }

        if let Some(error) = &self.ocr_state.error {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.label(
                egui::RichText::new("OCR FAILED")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(danger),
            );
            ui.add_space(3.0);
            ui.label(
                egui::RichText::new(error)
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(danger),
            );
        }
    }

    pub(crate) fn render_ocr_results(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let accent = hex_to_c32(&palette.harmonies.triadic_1);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let has_output = !self.ocr_state.extracted_text.is_empty();

        ui.label(
            egui::RichText::new("OCR output")
                .size(crate::ui_ext::HEADING_FONT_SIZE)
                .strong()
                .color(accent),
        );
        ui.label(
            egui::RichText::new("Review, edit, or copy the recognized text.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Characters")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(self.ocr_state.extracted_text.chars().count().to_string())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(text),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Lines")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(if has_output {
                        self.ocr_state.extracted_text.lines().count().to_string()
                    } else {
                        "0".to_string()
                    })
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .monospace()
                    .color(text),
                );
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Duration")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(
                        self.ocr_state
                            .duration_ms
                            .map(|ms| format!("{:.2}s", ms as f64 / 1000.0))
                            .unwrap_or_else(|| "—".to_string()),
                    )
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .monospace()
                    .color(text),
                );
            });
        });

        ui.add_space(10.0);
        let copy = crate::action_button("Copy all text", accent, palette);
        if ui
            .add_enabled_ui(has_output, |ui| {
                ui.add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], copy)
            })
            .inner
            .clicked()
        {
            ctx.output_mut(|output| output.copied_text = self.ocr_state.extracted_text.clone());
            self.show_toast("Extracted text copied to clipboard");
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let slot_width = ((ui.available_width() - 6.0) * 0.5).max(82.0);
        let output_for_slots = self.ocr_state.extracted_text.clone();
        ui.horizontal_wrapped(|ui| {
            let left = crate::secondary_button("Left hand", surface, palette);
            if ui
                .add_enabled_ui(has_output, |ui| {
                    ui.add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], left)
                })
                .inner
                .on_hover_text("Push recognized text to the Left Hand slot")
                .clicked()
            {
                self.push_text_to_hand_slot(HandSide::Left, output_for_slots.clone());
            }

            let right = crate::secondary_button("Right hand", surface, palette);
            if ui
                .add_enabled_ui(has_output, |ui| {
                    ui.add_sized([slot_width, crate::ui_ext::CONTROL_HEIGHT], right)
                })
                .inner
                .on_hover_text("Push recognized text to the Right Hand slot")
                .clicked()
            {
                self.push_text_to_hand_slot(HandSide::Right, output_for_slots.clone());
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("RECOGNIZED TEXT")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(text),
            );
            if has_output {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [64.0, crate::ui_ext::CONTROL_HEIGHT],
                            crate::compact_toolbar_button("Clear", surface, palette),
                        )
                        .clicked()
                    {
                        self.ocr_state.extracted_text.clear();
                        self.ocr_state.duration_ms = None;
                    }
                });
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);

        let editor_height = (ui.clip_rect().height() * 0.55).clamp(240.0, 520.0);
        egui::ScrollArea::vertical()
            .id_source("ocr_output_text_scroll")
            .max_height(editor_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_sized(
                    [ui.available_width(), editor_height],
                    egui::TextEdit::multiline(&mut self.ocr_state.extracted_text)
                        .font(egui::FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                        .hint_text(if self.ocr_state.in_progress {
                            "OCR is running…"
                        } else {
                            "Recognized text will appear here."
                        })
                        .desired_width(f32::INFINITY)
                        .desired_rows(18),
                );
            });
    }
}
