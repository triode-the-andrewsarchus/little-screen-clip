use super::super::*;

struct OcrChildGuard {
    child: std::process::Child,
    control: std::sync::Arc<ImageToolProcessControl>,
    reaped: bool,
}

impl OcrChildGuard {
    fn new(child: std::process::Child, control: std::sync::Arc<ImageToolProcessControl>) -> Self {
        Self {
            child,
            control,
            reaped: false,
        }
    }

    fn reap(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
}

impl std::ops::Deref for OcrChildGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for OcrChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for OcrChildGuard {
    fn drop(&mut self) {
        if !self.reaped {
            self.control.cancel();
            let _ = self.child.wait();
        }
    }
}

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

#[derive(Clone)]
pub struct OcrRequest {
    snapshot: DisplayImageSnapshotRequest,
    python_bin: String,
    script_path: String,
    engine_arg: String,
    custom_prompt: Option<String>,
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
        let script_path = ["scripts/ocr.py", "./scripts/ocr.py"]
            .iter()
            .find(|path| std::path::Path::new(path).exists())
            .copied()
            .unwrap_or("scripts/ocr.py")
            .to_string();
        let venv_python = std::env::var("HOME")
            .map(|home| {
                format!(
                    "{}/.local/share/little-screen-clip/nobg-venv/bin/python",
                    home
                )
            })
            .unwrap_or_default();
        let python_bin = if std::path::Path::new(&venv_python).exists() {
            venv_python
        } else {
            "python3".to_string()
        };
        self.run_ocr_with_command(python_bin, script_path);
    }

    fn run_ocr_with_command(&mut self, python_bin: String, script_path: String) {
        self.run_ocr_with_command_on(python_bin, script_path, BackgroundJobs::global());
    }

    fn run_ocr_with_command_on(
        &mut self,
        python_bin: String,
        script_path: String,
        jobs: &BackgroundJobs,
    ) {
        if self.ocr_state.in_progress {
            return;
        }
        let snapshot = self.display_image_snapshot_request();

        let engine = self.ocr_state.engine;
        let engine_arg = engine.arg_name().to_string();
        let custom_prompt = if self.ocr_state.custom_prompt.trim().is_empty() {
            None
        } else {
            Some(self.ocr_state.custom_prompt.trim().to_string())
        };

        self.ocr_rx = None;
        self.ocr_job_rx = None;
        self.ocr_retry_at = None;
        self.ocr_request = Some(OcrRequest {
            snapshot: snapshot.clone(),
            python_bin,
            script_path,
            engine_arg,
            custom_prompt,
        });
        self.ocr_input_stamp = Some(snapshot.input_stamp);
        self.ocr_state.in_progress = true;
        self.ocr_state.error = None;
        self.ocr_state.status_text = Some(format!("Starting {}...", engine.label()));
        self.submit_ocr_attempt(jobs);
    }

    fn submit_ocr_attempt(&mut self, jobs: &BackgroundJobs) {
        let Some(request) = self.ocr_request.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel::<OcrMsg>();
        self.ocr_rx = Some(rx);
        self.ocr_retry_at = None;
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        self.ocr_process_control = Some(control.clone());

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let start = std::time::Instant::now();
            let png_bytes = match request.snapshot.compose_png() {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = tx.send(OcrMsg::Error(format!(
                        "Failed to encode image to PNG: {error}"
                    )));
                    return;
                }
            };
            let _ = tx.send(OcrMsg::Progress(format!(
                "Launching OCR engine ({})...",
                request.engine_arg
            )));

            let mut cmd = Command::new(&request.python_bin);
            cmd.arg(&request.script_path);
            cmd.arg("-").arg("-");
            cmd.arg("--engine").arg(&request.engine_arg);
            if let Some(ref p) = request.custom_prompt {
                cmd.arg("--prompt").arg(p);
            }

            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = match super::background_removal::spawn_image_tool_child(cmd, &control) {
                Ok(c) => OcrChildGuard::new(c, control.clone()),
                Err(e) => {
                    if !control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = tx.send(OcrMsg::Error(format!(
                            "Failed to spawn python3 {}: {}",
                            request.script_path, e
                        )));
                    }
                    return;
                }
            };

            let pid = child.id();

            // Start both drainers before writing image input: either output pipe can fill while
            // the parent is still feeding a large PNG to stdin.
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

            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                if let Err(e) = stdin.write_all(&png_bytes) {
                    drop(stdin);
                    super::background_removal::terminate_image_tool_process(pid);
                    let _ = child.reap();
                    let _ = stdout_rx.recv();
                    let _ = stderr_rx.recv();
                    super::background_removal::clear_image_tool_pid_if_owned(
                        &control.child_pid,
                        pid,
                    );
                    if !control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = tx.send(OcrMsg::Error(format!(
                            "Failed to pipe image to OCR worker: {}",
                            e
                        )));
                    }
                    return;
                }
            }

            let status = match child.reap() {
                Ok(status) => status,
                Err(e) => {
                    if super::background_removal::clear_image_tool_pid_if_owned(
                        &control.child_pid,
                        pid,
                    ) {
                        super::background_removal::terminate_image_tool_process(pid);
                    }
                    let _ = child.reap();
                    let _ = stdout_rx.recv();
                    let _ = stderr_rx.recv();
                    if !control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = tx.send(OcrMsg::Error(format!(
                            "Error waiting for OCR worker: {}",
                            e
                        )));
                    }
                    return;
                }
            };
            super::background_removal::clear_image_tool_pid_if_owned(&control.child_pid, pid);
            let stdout_bytes = stdout_rx.recv().unwrap_or_default();
            let stderr_text = stderr_rx.recv().unwrap_or_default();
            if control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }

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
                        engine: request.engine_arg,
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
        self.ocr_ticket = Some(ticket);
        self.ocr_job_rx = Some(job_rx);
    }

    pub fn poll_ocr(&mut self) {
        self.poll_ocr_on(BackgroundJobs::global());
    }

    fn poll_ocr_on(&mut self, jobs: &BackgroundJobs) {
        if self.ocr_state.in_progress && self.ocr_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.cancel_ocr_with_feedback(false);
            return;
        }
        if self
            .ocr_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.submit_ocr_attempt(jobs);
        } else if self.ocr_retry_at.is_some() {
            return;
        }
        if let Some(job_rx) = self.ocr_job_rx.as_ref() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.ocr_job_rx = None;
                    self.ocr_ticket = None;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.ocr_job_rx = None;
                    self.ocr_ticket = None;
                    self.ocr_rx = None;
                    self.ocr_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                    self.ocr_state.status_text =
                        Some("OCR waiting for workers; retrying shortly".into());
                    return;
                }
                Ok(Err(JobError::Cancelled)) => {
                    self.finish_ocr_worker();
                    return;
                }
                Ok(Err(error)) => {
                    self.finish_ocr_worker();
                    self.ocr_state.error = Some(format!("OCR worker failed: {error}"));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.finish_ocr_worker();
                    self.ocr_state.error = Some("OCR worker stopped".into());
                    return;
                }
            }
        }
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
                        self.ocr_results_stamp = self.ocr_input_stamp.take();
                        self.ocr_state.in_progress = false;
                        self.ocr_state.status_text = None;
                        self.ocr_rx = None;
                        self.ocr_process_control = None;
                        self.ocr_request = None;
                        self.ocr_retry_at = None;
                        self.ocr_ticket = None;
                        self.ocr_job_rx = None;
                        self.show_toast(crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::Check,
                            format!(
                                "OCR complete with {} in {:.2}s",
                                engine,
                                duration_ms as f64 / 1000.0
                            ),
                        ));
                        break;
                    }
                    OcrMsg::Error(err) => {
                        self.ocr_state.error = Some(err);
                        self.ocr_state.in_progress = false;
                        self.ocr_state.status_text = None;
                        self.ocr_rx = None;
                        self.ocr_input_stamp = None;
                        self.ocr_process_control = None;
                        self.ocr_request = None;
                        self.ocr_retry_at = None;
                        self.ocr_ticket = None;
                        self.ocr_job_rx = None;
                        break;
                    }
                }
            }
        }
    }

    pub fn cancel_ocr(&mut self) {
        self.cancel_ocr_with_feedback(true);
    }

    fn cancel_ocr_with_feedback(&mut self, show_toast: bool) {
        if let Some(ticket) = self.ocr_ticket.take() {
            ticket.cancel();
        }
        self.ocr_job_rx = None;
        self.ocr_request = None;
        self.ocr_retry_at = None;
        if let Some(control) = self.ocr_process_control.take() {
            control.cancel();
        }
        if self.ocr_state.in_progress {
            self.ocr_state.in_progress = false;
            self.ocr_state.status_text = None;
            self.ocr_rx = None;
            self.ocr_input_stamp = None;
            if show_toast {
                self.show_toast(crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Stop,
                    "OCR process cancelled",
                ));
            }
        }
    }

    fn finish_ocr_worker(&mut self) {
        if let Some(control) = self.ocr_process_control.take() {
            control.cancel();
        }
        self.ocr_job_rx = None;
        self.ocr_ticket = None;
        self.ocr_rx = None;
        self.ocr_request = None;
        self.ocr_retry_at = None;
        self.ocr_input_stamp = None;
        self.ocr_process_control = None;
        self.ocr_state.in_progress = false;
        self.ocr_state.status_text = None;
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
        self.show_toast(crate::ui_ext::StatusMessage::new(
            crate::icons::Icon::Copy,
            format!(
                "Pushed to {} Hand slot",
                match hand {
                    HandSide::Left => "Left",
                    HandSide::Right => "Right",
                }
            ),
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
            if let Some(retry_at) = self.ocr_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
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
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
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
        let output_is_current = self.ocr_output_is_current();
        let output_visible = output_is_current || self.ocr_results_stamp.is_none();
        let has_output = output_visible && !self.ocr_state.extracted_text.is_empty();

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
                    egui::RichText::new(if output_visible {
                        self.ocr_state.extracted_text.chars().count().to_string()
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
                        output_is_current
                            .then_some(self.ocr_state.duration_ms)
                            .flatten()
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
                        self.ocr_results_stamp = None;
                    }
                });
            }
        });
        ui.add_space(crate::ui_ext::SPACE_XS);

        let editor_height = (ui.clip_rect().height() * 0.55).clamp(240.0, 520.0);
        let source_stamp = self.input_stamp();
        let in_progress = self.ocr_state.in_progress;
        let has_stale_text = !self.ocr_state.extracted_text.is_empty();
        let text_changed = {
            let mut stale_output = String::new();
            let text_output = if output_visible {
                &mut self.ocr_state.extracted_text
            } else {
                &mut stale_output
            };
            egui::ScrollArea::vertical()
                .id_source("ocr_output_text_scroll")
                .max_height(editor_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_enabled_ui(output_visible, |ui| {
                        ui.add_sized(
                            [ui.available_width(), editor_height],
                            egui::TextEdit::multiline(text_output)
                                .font(egui::FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                .hint_text(if in_progress {
                                    "OCR is running…"
                                } else if has_stale_text {
                                    "Image changed; run OCR again to view current text."
                                } else {
                                    "Recognized text will appear here."
                                })
                                .desired_width(f32::INFINITY)
                                .desired_rows(18),
                        )
                    })
                    .inner
                    .changed()
                })
                .inner
        };
        if text_changed
            && self.ocr_results_stamp.is_none()
            && !self.ocr_state.extracted_text.trim().is_empty()
        {
            self.ocr_results_stamp = Some(source_stamp);
        }
    }
}

#[cfg(test)]
mod input_stamp_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

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
            "OCR stamp test".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    fn write_script(directory: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/usr/bin/env python3\n{body}\n")).unwrap();
        path
    }

    fn wait_ocr(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.ocr_state.in_progress && Instant::now() < deadline {
            state.poll_ocr_on(jobs);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !state.ocr_state.in_progress,
            "synthetic OCR worker did not finish"
        );
    }

    #[test]
    fn rendered_ocr_copy_all_publishes_exact_recognized_text() {
        let mut state = state();
        state.ocr_state.extracted_text = "First recognized line\n日本語 OCR ✓".to_owned();
        let expected = state.ocr_state.extracted_text.clone();
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));

        let initial = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ocr_results(ui, ctx, &palette);
                });
            },
        );
        let copy_position = initial
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "Copy all text" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("rendered OCR Copy all text button label");
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen_rect),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ocr_results(ui, ctx, &palette);
                    });
                },
            )
        };
        render(vec![egui::Event::PointerMoved(copy_position)]);
        render(vec![
            egui::Event::PointerMoved(copy_position),
            egui::Event::PointerButton {
                pos: copy_position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let copied = render(vec![
            egui::Event::PointerMoved(copy_position),
            egui::Event::PointerButton {
                pos: copy_position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);

        assert_eq!(copied.platform_output.copied_text, expected);
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_rx.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn ocr_queue_full_retains_request_and_retries_progress_and_output() {
        let directory = tempfile::tempdir().unwrap();
        let script = write_script(
            directory.path(),
            "fake-ocr.py",
            "import sys\nprint('[ocr:progress] fake stage', file=sys.stderr, flush=True)\nsys.stderr.write('x' * 200000 + '\\n')\nsys.stderr.flush()\nsys.stdin.buffer.read()\nprint('recognized from synthetic helper')",
        );
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_blocker, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_filler, filler_rx) = jobs.submit(Duration::MAX, || ());
        let mut state = state();
        state.current_rgba = std::sync::Arc::new(RgbaImage::from_fn(512, 512, |x, y| {
            let n = x
                .wrapping_mul(1_103_515_245)
                .wrapping_add(y.wrapping_mul(12_345));
            image::Rgba([
                (n & 0xff) as u8,
                ((n >> 8) & 0xff) as u8,
                ((n >> 16) & 0xff) as u8,
                255,
            ])
        }));
        state.run_ocr_with_command_on("python3".into(), script.display().to_string(), &jobs);
        let retained = std::sync::Arc::clone(&state.ocr_request.as_ref().unwrap().snapshot.pixels);
        assert!(
            retained.len() > 64 * 1024,
            "fixture PNG must exceed a pipe buffer"
        );
        state.poll_ocr_on(&jobs);
        let retry_at = state.ocr_retry_at.expect("QueueFull must schedule a retry");
        assert!(
            state
                .ocr_state
                .status_text
                .as_deref()
                .unwrap()
                .contains("waiting")
        );
        assert!(std::sync::Arc::ptr_eq(
            &retained,
            &state.ocr_request.as_ref().unwrap().snapshot.pixels
        ));
        state.poll_ocr_on(&jobs);
        assert_eq!(state.ocr_retry_at, Some(retry_at));

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        filler_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        state.ocr_retry_at = Some(Instant::now());
        state.poll_ocr_on(&jobs);
        wait_ocr(&mut state, &jobs);
        assert_eq!(
            state.ocr_state.extracted_text.trim(),
            "recognized from synthetic helper"
        );
        assert!(state.ocr_output_is_current());
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[test]
    fn queued_ocr_cancellation_prevents_helper_start() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("started");
        let script = write_script(
            directory.path(),
            "queued-ocr.py",
            &format!(
                "import pathlib,sys\nsys.stdin.buffer.read()\npathlib.Path(r'{}').write_text('started')\nprint('unexpected')",
                marker.display()
            ),
        );
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_blocker, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut state = state();
        state.run_ocr_with_command_on("python3".into(), script.display().to_string(), &jobs);
        assert!(state.ocr_ticket.is_some());
        state.cancel_ocr_with_feedback(false);
        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (_, receiver) = jobs.submit(Duration::MAX, || ());
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(())) => break,
                Ok(Err(JobError::QueueFull)) | Err(mpsc::RecvTimeoutError::Timeout)
                    if Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("scheduler did not drain canceled OCR job: {other:?}"),
            }
        }
        assert!(!marker.exists());
        assert!(!state.ocr_state.in_progress);
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[test]
    fn running_ocr_cancellation_kills_and_reaps_fake_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("worker.pid");
        let script = write_script(
            directory.path(),
            "running-ocr.py",
            &format!(
                "import os,sys,time\nopen(r'{}','w').write(str(os.getpid()))\nsys.stdin.buffer.read()\ntime.sleep(30)",
                pid_file.display()
            ),
        );
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = state();
        state.run_ocr_with_command_on("python3".into(), script.display().to_string(), &jobs);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            state.poll_ocr_on(&jobs);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pid_file.exists(), "synthetic OCR child did not start");
        let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        let control = state.ocr_process_control.as_ref().unwrap().clone();
        state.cancel_ocr_with_feedback(false);
        assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_ocr_child_exited(pid);
        assert!(!state.ocr_state.in_progress);
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    fn wait_for_ocr_pid(state: &ImagePreviewState) -> u32 {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let pid = state
                .ocr_process_control
                .as_ref()
                .unwrap()
                .child_pid
                .load(std::sync::atomic::Ordering::SeqCst);
            if pid != 0 {
                return pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "OCR child PID was not published"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    fn assert_ocr_child_exited(pid: u32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("OCR child {pid} remained alive after cancellation");
    }

    #[cfg(unix)]
    #[test]
    fn stale_ocr_input_terminates_child_without_manual_cancel_toast() {
        let script = std::env::temp_dir().join(format!("clip-ocr-stale-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
        )
        .unwrap();
        let mut state = state();
        state.run_ocr_with_command("python3".into(), script.display().to_string());
        let pid = wait_for_ocr_pid(&state);
        let control = state.ocr_process_control.as_ref().unwrap().clone();
        state.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::ZERO,
            text: "changed OCR source".into(),
            font_size: 10.0,
        });

        state.poll_ocr();

        assert!(control.cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_ocr_child_exited(pid);
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.status_text.is_none());
        assert!(state.ocr_state.error.is_none());
        assert!(state.status_toast.is_none());
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[test]
    fn ocr_cancel_before_pid_publication_terminates_spawned_child() {
        let script = std::env::temp_dir().join(format!("clip-ocr-race-{}.py", std::process::id()));
        let pid_file =
            std::env::temp_dir().join(format!("clip-ocr-race-{}.pid", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
        )
        .unwrap();
        let mut command = Command::new("python3");
        command
            .arg(&script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let pid_file_c =
            std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(pid_file.as_os_str()))
                .unwrap();
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(move || {
                let fd = libc::open(
                    pid_file_c.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
                    0o600,
                );
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let pid = libc::getpid() as u32;
                let wrote =
                    libc::write(fd, (&pid as *const u32).cast(), std::mem::size_of::<u32>());
                libc::close(fd);
                if wrote != std::mem::size_of::<u32>() as isize {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let control = ImageToolProcessControl::new();
        control
            .cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let start = std::time::Instant::now();

        let result = super::super::background_removal::spawn_image_tool_child(command, &control);

        assert_eq!(result.unwrap_err(), "Image tool process cancelled");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(
            control.child_pid.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let pid_bytes: [u8; 4] = std::fs::read(&pid_file).unwrap().try_into().unwrap();
        assert_ocr_child_exited(u32::from_ne_bytes(pid_bytes));
        let _ = std::fs::remove_file(script);
        let _ = std::fs::remove_file(pid_file);
    }

    #[cfg(unix)]
    #[test]
    fn current_ocr_process_success_keeps_text_and_progress_handling() {
        let script =
            std::env::temp_dir().join(format!("clip-ocr-success-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\nprint('[ocr:progress] fake stage', file=sys.stderr, flush=True)\ntime.sleep(0.05)\nsys.stdout.write('recognized text')\n",
        )
        .unwrap();
        let mut state = state();
        state.run_ocr_with_command("python3".into(), script.display().to_string());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state.ocr_state.status_text.as_deref() != Some("fake stage")
            && state.ocr_state.in_progress
            && std::time::Instant::now() < deadline
        {
            state.poll_ocr();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state.ocr_state.status_text.as_deref(), Some("fake stage"));
        while state.ocr_state.in_progress && std::time::Instant::now() < deadline {
            state.poll_ocr();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        state.poll_ocr();
        assert_eq!(state.ocr_state.extracted_text, "recognized text");
        assert!(state.ocr_output_is_current());
        assert!(state.ocr_state.error.is_none());
        let _ = std::fs::remove_file(script);
    }

    #[cfg(unix)]
    #[test]
    fn current_ocr_process_failure_preserves_diagnostics() {
        let script =
            std::env::temp_dir().join(format!("clip-ocr-failure-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys\nsys.stdin.buffer.read()\nprint('fake OCR diagnostic', file=sys.stderr)\nsys.exit(5)\n",
        )
        .unwrap();
        let mut state = state();
        state.run_ocr_with_command("python3".into(), script.display().to_string());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state.ocr_state.in_progress && std::time::Instant::now() < deadline {
            state.poll_ocr();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        state.poll_ocr();
        assert!(
            state
                .ocr_state
                .error
                .as_deref()
                .unwrap()
                .contains("OCR worker failed: fake OCR diagnostic")
        );
        let _ = std::fs::remove_file(script);
    }

    #[test]
    fn stale_ocr_success_keeps_existing_text_and_clears_loading() {
        let mut state = state();
        state.ocr_state.extracted_text = "previous text".into();
        state.ocr_state.in_progress = true;
        state.ocr_state.status_text = Some("working".into());
        state.ocr_process_control = Some(std::sync::Arc::new(ImageToolProcessControl::new()));
        state.ocr_input_stamp = Some(state.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel();
        state.ocr_rx = Some(rx);
        tx.send(OcrMsg::Success {
            text: "stale text".into(),
            engine: "test".into(),
            duration_ms: 1,
        })
        .unwrap();
        state.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::ZERO,
            text: "changed source".into(),
            font_size: 10.0,
        });

        state.poll_ocr();

        assert_eq!(state.ocr_state.extracted_text, "previous text");
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.status_text.is_none());
        assert!(state.ocr_rx.is_none());
        assert!(state.ocr_state.error.is_none());
        assert!(state.ocr_process_control.is_none());
    }

    #[test]
    fn current_ocr_success_publishes_text() {
        let mut state = state();
        state.ocr_state.in_progress = true;
        state.ocr_input_stamp = Some(state.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel();
        state.ocr_rx = Some(rx);
        tx.send(OcrMsg::Success {
            text: "current text".into(),
            engine: "test".into(),
            duration_ms: 1,
        })
        .unwrap();

        state.poll_ocr();

        assert_eq!(state.ocr_state.extracted_text, "current text");
        assert!(state.ocr_output_is_current());
        state.mark_pixels_changed();
        assert!(!state.ocr_output_is_current());
        assert_eq!(state.ocr_state.extracted_text, "current text");
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_rx.is_none());
        assert!(state.ocr_state.error.is_none());
    }

    #[test]
    fn stale_ocr_error_is_discarded_and_loading_clears() {
        let mut state = state();
        state.ocr_state.in_progress = true;
        state.ocr_input_stamp = Some(state.input_stamp());
        let (tx, rx) = std::sync::mpsc::channel();
        state.ocr_rx = Some(rx);
        tx.send(OcrMsg::Error("stale error".into())).unwrap();
        state.redaction_state.boxes.push(RedactionBox {
            normalized_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 0.5)),
        });

        state.poll_ocr();

        assert!(state.ocr_state.error.is_none());
        assert!(!state.ocr_state.in_progress);
        assert!(state.ocr_state.status_text.is_none());
        assert!(state.ocr_rx.is_none());
    }
}
