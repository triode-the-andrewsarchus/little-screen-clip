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

pub(super) fn background_removal_command(configured_cmd: Option<&str>) -> (String, Vec<String>) {
    if let Some(cmd) = configured_cmd {
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
    }
}

pub(super) fn run_image_tool_subprocess(
    mut cmd: Command,
    png_bytes: &[u8],
    prompt: Option<&str>,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    child_pid: Option<&std::sync::atomic::AtomicU32>,
    operation: BgRemovalOperation,
    command_name: &str,
) -> Result<std::process::Output, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    if let Some(prompt) = prompt {
        cmd.arg("--prompt").arg(prompt);
    }
    cmd.arg("-").arg("-");

    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| match operation {
            BgRemovalOperation::BackgroundRemoval => format!(
                "Failed to launch background removal process '{}': {}. Ensure Python with nobg is installed.",
                command_name, error
            ),
            BgRemovalOperation::DepthMap => format!(
                "Failed to launch depth estimation '{}': {}. Ensure Python environment is setup.",
                command_name, error
            ),
        })?;

    let pid = child.id();
    if let Some(child_pid) = child_pid {
        child_pid.store(pid, std::sync::atomic::Ordering::SeqCst);
    }
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
        terminate_image_tool_process(pid);
        let _ = child.wait();
        if let Some(child_pid) = child_pid {
            clear_image_tool_pid_if_owned(child_pid, pid);
        }
        return Err(image_tool_cancelled_message(operation).to_string());
    }

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if let Err(error) = stdin.write_all(png_bytes) {
            terminate_image_tool_process(pid);
            let _ = child.wait();
            if let Some(child_pid) = child_pid {
                clear_image_tool_pid_if_owned(child_pid, pid);
            }
            return Err(format!("Failed to write image data to worker: {}", error));
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            if let Some(child_pid) = child_pid {
                let owns_pid = clear_image_tool_pid_if_owned(child_pid, pid);
                if owns_pid {
                    terminate_image_tool_process(pid);
                }
            } else {
                terminate_image_tool_process(pid);
            }
            let message = match operation {
                BgRemovalOperation::BackgroundRemoval => format!("Worker process failed: {error}"),
                BgRemovalOperation::DepthMap => format!("Depth estimation process failed: {error}"),
            };
            return Err(message);
        }
    };
    if let Some(child_pid) = child_pid {
        clear_image_tool_pid_if_owned(child_pid, pid);
    }
    if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
        return Err(image_tool_cancelled_message(operation).to_string());
    }
    if !output.status.success() {
        let err_str = String::from_utf8_lossy(&output.stderr);
        return Err(match operation {
            BgRemovalOperation::BackgroundRemoval => format!(
                "Background removal failed (exit code {:?}):\n{}",
                output.status.code(),
                err_str.trim()
            ),
            BgRemovalOperation::DepthMap => format!("Depth estimation failed:\n{}", err_str.trim()),
        });
    }
    Ok(output)
}

fn image_tool_cancelled_message(operation: BgRemovalOperation) -> &'static str {
    match operation {
        BgRemovalOperation::BackgroundRemoval => "Background removal cancelled",
        BgRemovalOperation::DepthMap => "Depth estimation cancelled",
    }
}

pub(super) fn decode_image_tool_output(output: std::process::Output) -> Result<RgbaImage, String> {
    image::load_from_memory_with_format(&output.stdout, image::ImageFormat::Png)
        .map(|image| image.to_rgba8())
        .map_err(|e| format!("Failed to decode output PNG: {}", e))
}

pub(crate) fn terminate_image_tool_process(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGTERM);
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[derive(Debug)]
pub struct ImageToolProcessControl {
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub child_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

pub(super) fn run_image_tool_status_command(
    cmd: Command,
    control: &ImageToolProcessControl,
) -> Result<std::process::ExitStatus, String> {
    let mut child = spawn_image_tool_child(cmd, control)?;
    let pid = child.id();
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            if clear_image_tool_pid_if_owned(&control.child_pid, pid) {
                terminate_image_tool_process(pid);
            }
            return Err(error.to_string());
        }
    };
    clear_image_tool_pid_if_owned(&control.child_pid, pid);
    if control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("Image tool process cancelled".to_string());
    }
    Ok(status)
}

pub(crate) fn spawn_image_tool_child(
    mut cmd: Command,
    control: &ImageToolProcessControl,
) -> Result<std::process::Child, String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|error| error.to_string())?;
    let pid = child.id();
    control
        .child_pid
        .store(pid, std::sync::atomic::Ordering::SeqCst);
    if control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
        terminate_image_tool_process(pid);
        let _ = child.wait();
        clear_image_tool_pid_if_owned(&control.child_pid, pid);
        return Err("Image tool process cancelled".to_string());
    }
    Ok(child)
}

impl ImageToolProcessControl {
    pub fn new() -> Self {
        Self {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            child_pid: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        let pid = self.child_pid.swap(0, std::sync::atomic::Ordering::SeqCst);
        terminate_image_tool_process(pid);
    }
}

pub(crate) fn clear_image_tool_pid_if_owned(
    child_pid: &std::sync::atomic::AtomicU32,
    pid: u32,
) -> bool {
    child_pid
        .compare_exchange(
            pid,
            0,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
}

#[cfg(all(test, unix))]
mod cancellation_tests {
    use super::*;

    #[test]
    fn old_worker_cannot_clear_a_new_process_pid() {
        let published_pid = std::sync::atomic::AtomicU32::new(202);

        assert!(!clear_image_tool_pid_if_owned(&published_pid, 101));
        assert_eq!(published_pid.load(std::sync::atomic::Ordering::SeqCst), 202);
    }

    #[test]
    fn cancellation_requested_before_pid_publication_terminates_spawned_child() {
        let script =
            std::env::temp_dir().join(format!("clip-bg-start-race-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
        )
        .unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let pid = std::sync::atomic::AtomicU32::new(0);
        let mut cmd = Command::new("python3");
        cmd.arg(&script);
        let result = run_image_tool_subprocess(
            cmd,
            b"input",
            None,
            Some(&cancelled),
            Some(&pid),
            BgRemovalOperation::BackgroundRemoval,
            "python3",
        );
        assert_eq!(result.unwrap_err(), "Background removal cancelled");
        let child_pid = pid.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(child_pid, 0);
        let _ = std::fs::remove_file(script);
    }
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
        self.bg_removal_input_stamp = Some(self.input_stamp());
        self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
        self.bg_removal_in_progress = true;
        self.bg_removal_status = Some(status_desc);
        self.bg_removal_error = None;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.image_tool_cancel = Some(cancel.clone());
        self.image_tool_child_pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let child_pid = self.image_tool_child_pid.clone();

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

            let (cmd_bin, cmd_args) = background_removal_command(configured_cmd.as_deref());
            let mut cmd = Command::new(&cmd_bin);
            cmd.args(&cmd_args);
            let result = run_image_tool_subprocess(
                cmd,
                &png_bytes,
                thread_prompt.as_deref(),
                Some(&cancel),
                Some(&child_pid),
                BgRemovalOperation::BackgroundRemoval,
                &cmd_bin,
            )
            .and_then(decode_image_tool_output);
            match result {
                Ok(result_rgba) => {
                    let _ = tx.send(BgRemovalMsg::Success {
                        result_rgba,
                        duration_ms: start.elapsed().as_millis(),
                    });
                }
                Err(error) => {
                    if !cancel.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = tx.send(BgRemovalMsg::Error(error));
                    }
                }
            }
        });
    }

    /// Runs AI depth estimation asynchronously using Depth-Anything / MiDaS.

    pub fn poll_bg_removal(&mut self) {
        if self.bg_removal_rx.is_some()
            && self.bg_removal_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.cancel_image_tool_process();
            self.bg_removal_rx = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            return;
        }

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
            let operation = self.bg_removal_operation;
            let included_crop = self
                .bg_removal_input_stamp
                .as_ref()
                .is_some_and(|stamp| stamp.crop.is_some());
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = Arc::new(result_rgba);
            self.mark_pixels_changed();
            self.redaction_state.boxes.clear();
            self.markup_state.items.clear();
            if included_crop {
                self.crop_state.normalized_rect =
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            }
            self.creative_fx_state.cached_background_removal =
                Some(self.current_rgba.as_ref().clone());
            self.creative_fx_state.background_removal_input_stamp = Some(self.input_stamp());
            self.creative_fx_state.mask_cache_revision =
                self.creative_fx_state.mask_cache_revision.wrapping_add(1);
            self.dirty_texture = true;
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
            self.bg_removal_error = None;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;

            let elapsed = duration_ms as f64 / 1000.0;
            self.show_toast(match operation {
                BgRemovalOperation::BackgroundRemoval => {
                    format!("✨ Background removed in {elapsed:.2}s!")
                }
                BgRemovalOperation::DepthMap => {
                    format!("🌊 Depth map generated in {elapsed:.2}s!")
                }
            });
        } else if let Some(err) = error_msg {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
            self.bg_removal_error = Some(err);
        } else if disconnected {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_rx = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
        }
    }

    pub(crate) fn cancel_image_tool_process(&mut self) {
        if let Some(cancel) = self.image_tool_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let pid = self
            .image_tool_child_pid
            .swap(0, std::sync::atomic::Ordering::SeqCst);
        terminate_image_tool_process(pid);
        self.bg_removal_rx = None;
        self.bg_removal_input_stamp = None;
        self.bg_removal_in_progress = false;
        self.bg_removal_status = None;
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
                .size(crate::ui_ext::HEADING_FONT_SIZE)
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

        ui.add_space(crate::ui_ext::SPACE_MD);
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
        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

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
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(if has_prompt {
                "MultiMatte will isolate the described subject. Ctrl+Enter runs while editing."
            } else {
                "Leave blank to detect the foreground automatically with FeyNoBg."
            })
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
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
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("PROCESSING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

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
            ui.add_space(crate::ui_ext::SPACE_XS);
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
            ui.add_space(crate::ui_ext::SPACE_SM);
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
                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
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

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("OUTPUT")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

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
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
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
