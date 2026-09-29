use super::super::*;

#[cfg(all(test, unix))]
mod cancellation_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn state() -> ImagePreviewState {
        let image = RgbaImage::from_pixel(4, 4, image::Rgba([20, 30, 40, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            bytes,
            "image/png".into(),
            None,
            "depth cancellation".into(),
            &AppConfig::default(),
        )
        .unwrap()
    }

    fn start_sleeping_job(
        state: &mut ImagePreviewState,
        label: &str,
    ) -> (
        u32,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        std::path::PathBuf,
    ) {
        let script =
            std::env::temp_dir().join(format!("clip-depth-{label}-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
        )
        .unwrap();
        state.set_tool_mode(ImageToolMode::DepthMap);
        state.run_depth_estimation_with_command(
            "python3".into(),
            vec![script.display().to_string()],
        );
        let cancel = state.image_tool_cancel.as_ref().unwrap().clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while state
            .image_tool_child_pid
            .load(std::sync::atomic::Ordering::SeqCst)
            == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let pid = state
            .image_tool_child_pid
            .load(std::sync::atomic::Ordering::SeqCst);
        assert_ne!(pid, 0, "depth child PID was not published");
        (pid, cancel, script)
    }

    fn assert_exited(pid: u32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let result = unsafe { libc::kill(pid as i32, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("depth child {pid} remained alive after cancellation");
    }

    fn write_script(directory: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        path
    }

    fn wait_depth(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.bg_removal_in_progress && Instant::now() < deadline {
            state.poll_bg_removal_on(jobs);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !state.bg_removal_in_progress,
            "synthetic depth worker did not finish"
        );
    }

    #[test]
    fn depth_queue_full_retains_request_and_retries_with_output() {
        let directory = tempfile::tempdir().unwrap();
        let expected = RgbaImage::from_pixel(3, 2, image::Rgba([180, 40, 20, 255]));
        let mut expected_png = Vec::new();
        expected
            .write_to(
                &mut std::io::Cursor::new(&mut expected_png),
                image::ImageFormat::Png,
            )
            .unwrap();
        let output = directory.path().join("depth.png");
        std::fs::write(&output, expected_png).unwrap();
        let script = write_script(
            directory.path(),
            "fake-depth.sh",
            &format!("cat >/dev/null\ncat '{}'", output.display()),
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
        state.set_tool_mode(ImageToolMode::DepthMap);
        state.run_depth_estimation_with_command_on(
            "sh".into(),
            vec![script.display().to_string()],
            &jobs,
        );
        let retained = Arc::clone(&state.bg_removal_request.as_ref().unwrap().png_bytes);
        state.poll_bg_removal_on(&jobs);
        let deadline = state.bg_removal_retry_at.unwrap();
        assert!(
            state
                .bg_removal_status
                .as_deref()
                .unwrap()
                .contains("Depth Map")
        );
        assert!(Arc::ptr_eq(
            &retained,
            &state.bg_removal_request.as_ref().unwrap().png_bytes
        ));
        state.poll_bg_removal_on(&jobs);
        assert_eq!(state.bg_removal_retry_at, Some(deadline));

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        filler_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        state.bg_removal_retry_at = Some(Instant::now());
        state.poll_bg_removal_on(&jobs);
        wait_depth(&mut state, &jobs);
        assert_eq!(state.current_rgba.as_ref(), &expected);
        assert!(
            state
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .starts_with("🌊 Depth map generated")
        );
    }

    #[test]
    fn queued_depth_cancellation_prevents_helper_start() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("started");
        let script = write_script(
            directory.path(),
            "queued-depth.sh",
            &format!("echo started > '{}'\ncat >/dev/null", marker.display()),
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
        state.set_tool_mode(ImageToolMode::DepthMap);
        state.run_depth_estimation_with_command_on(
            "sh".into(),
            vec![script.display().to_string()],
            &jobs,
        );
        state.cancel_image_tool_process();
        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let (_, receiver) = jobs.submit(Duration::MAX, || ());
            match receiver.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(())) => break,
                Ok(Err(JobError::QueueFull)) | Err(mpsc::RecvTimeoutError::Timeout) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("scheduler did not drain canceled depth job: {other:?}"),
            }
        }
        assert!(!marker.exists());
        assert!(!state.bg_removal_in_progress);
    }

    #[test]
    fn running_depth_cancellation_kills_and_reaps_fake_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("worker.pid");
        let script = write_script(
            directory.path(),
            "running-depth.sh",
            &format!(
                "echo $$ > '{}'\ncat >/dev/null\nsleep 30",
                pid_file.display()
            ),
        );
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = state();
        state.set_tool_mode(ImageToolMode::DepthMap);
        state.run_depth_estimation_with_command_on(
            "sh".into(),
            vec![script.display().to_string()],
            &jobs,
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            state.poll_bg_removal_on(&jobs);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pid_file.exists(), "synthetic depth child did not start");
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        state.cancel_image_tool_process();
        assert_exited(pid as u32);
        assert_eq!(
            state
                .image_tool_child_pid
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn depth_cancel_before_pid_publication_stops_spawned_child() {
        let script =
            std::env::temp_dir().join(format!("clip-depth-start-race-{}.py", std::process::id()));
        std::fs::write(
            &script,
            "import sys,time\nsys.stdin.buffer.read()\ntime.sleep(30)\n",
        )
        .unwrap();
        let mut cmd = Command::new("python3");
        cmd.arg(&script);
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let pid = std::sync::atomic::AtomicU32::new(0);
        let result = super::super::background_removal::run_image_tool_subprocess(
            cmd,
            b"input",
            None,
            Some(&cancelled),
            Some(&pid),
            BgRemovalOperation::DepthMap,
            "python3",
        );
        assert_eq!(result.unwrap_err(), "Depth estimation cancelled");
        assert_eq!(pid.load(std::sync::atomic::Ordering::SeqCst), 0);
        let _ = std::fs::remove_file(script);
    }

    #[test]
    fn depth_subprocess_stops_on_stale_input_mode_exit_and_viewer_drop() {
        let mut stale = state();
        let (pid, cancel, script) = start_sleeping_job(&mut stale, "stale");
        stale.markup_state.items.push(MarkupShape::Text {
            pos: Pos2::ZERO,
            text: "changed".into(),
            font_size: 10.0,
        });
        stale.poll_bg_removal();
        assert!(cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_exited(pid);
        let _ = std::fs::remove_file(script);

        let mut mode = state();
        let (pid, cancel, script) = start_sleeping_job(&mut mode, "mode");
        mode.set_tool_mode(ImageToolMode::View);
        assert!(cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_exited(pid);
        let _ = std::fs::remove_file(script);

        let mut dropped = state();
        let (pid, cancel, script) = start_sleeping_job(&mut dropped, "drop");
        drop(dropped);
        assert!(cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert_exited(pid);
        let _ = std::fs::remove_file(script);
    }

    #[test]
    fn depth_success_and_process_failure_keep_operation_specific_results() {
        let mut success = state();
        success.set_tool_mode(ImageToolMode::DepthMap);
        success.run_depth_estimation_with_command(
            "python3".into(),
            vec![
                "-c".into(),
                "import sys; d=sys.stdin.buffer.read(); sys.stdout.buffer.write(d)".into(),
            ],
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while success.bg_removal_in_progress && std::time::Instant::now() < deadline {
            success.poll_bg_removal();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        success.poll_bg_removal();
        assert!(success.bg_removal_error.is_none());
        assert!(
            success
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .starts_with("🌊 Depth map generated")
        );

        let mut failure = state();
        failure.set_tool_mode(ImageToolMode::DepthMap);
        failure.run_depth_estimation_with_command(
            "python3".into(),
            vec!["-c".into(), "import sys; sys.stdin.buffer.read(); print('expected depth failure', file=sys.stderr); sys.exit(7)".into()],
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while failure.bg_removal_in_progress && std::time::Instant::now() < deadline {
            failure.poll_bg_removal();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        failure.poll_bg_removal();
        assert!(
            failure
                .bg_removal_error
                .as_deref()
                .unwrap()
                .contains("Depth estimation failed:\nexpected depth failure")
        );
    }
}

impl ImagePreviewState {
    pub fn run_depth_estimation(&mut self) {
        let depth_script = "scripts/depth_estimation.py";
        let (cmd_bin, cmd_args) = if std::path::Path::new(depth_script).exists() {
            (depth_script.to_string(), Vec::new())
        } else {
            ("python3".to_string(), vec![depth_script.to_string()])
        };
        self.run_depth_estimation_with_command(cmd_bin, cmd_args);
    }

    pub(crate) fn run_depth_estimation_with_command(
        &mut self,
        cmd_bin: String,
        cmd_args: Vec<String>,
    ) {
        self.run_depth_estimation_with_command_on(cmd_bin, cmd_args, BackgroundJobs::global());
    }

    fn run_depth_estimation_with_command_on(
        &mut self,
        cmd_bin: String,
        cmd_args: Vec<String>,
        jobs: &BackgroundJobs,
    ) {
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

        self.bg_removal_input_stamp = Some(self.input_stamp());
        self.bg_removal_operation = BgRemovalOperation::DepthMap;
        self.bg_removal_in_progress = true;
        let status = "Estimating depth map with Depth-Anything...".to_string();
        self.bg_removal_status = Some(status.clone());
        self.bg_removal_error = None;
        self.bg_removal_request = Some(BgRemovalRequest {
            png_bytes: Arc::new(png_bytes),
            prompt: None,
            command: cmd_bin,
            args: cmd_args,
            operation: BgRemovalOperation::DepthMap,
            initial_progress: "Invoking depth estimation neural network...".to_string(),
            status,
        });
        self.bg_removal_retry_at = None;
        self.submit_image_tool_attempt(jobs);
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
            if let Some(retry_at) = self.bg_removal_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        }

        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let diff_add = hex_to_c32(&palette.diff_add);
        let diff_mod = hex_to_c32(&palette.diff_mod);
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
                    if self.bg_removal_in_progress {
                        ui.add(egui::Spinner::new().size(16.0).color(base_c));
                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                        let st = self.bg_removal_status.as_deref().unwrap_or("Estimating depth map with Depth-Anything...");
                        ui.label(
                            egui::RichText::new(st)
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .monospace()
                                .strong()
                                .color(base_c),
                        );
                    } else {
                        let depth_fg = contrast_text_c32(base_c, None, palette);
                        let depth_btn = egui::Button::new(
                            egui::RichText::new("🌊 Generate Depth Map (Enter)")
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .strong()
                                .color(depth_fg),
                        )
                        .fill(base_c)
                        .stroke(Stroke::new(1.0_f32, b_med))
                        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                        .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                        if ui.add(depth_btn).on_hover_text("Generate AI metric depth map with Depth-Anything V2 neural network (Enter)").clicked() {
                            self.run_depth_estimation();
                        }

                        ui.add_space(crate::ui_ext::SPACE_SM);
                        ui.label(
                            egui::RichText::new("Depth-Anything V2 • Metric Monocular Depth Estimation • CUDA")
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .color(f_low),
                        );
                    }

                    if let Some(err) = &self.bg_removal_error {
                        ui.add_space(crate::ui_ext::SPACE_SM);
                        ui.label(
                            egui::RichText::new(format!("⚠ {}", err))
                                .size(crate::ui_ext::BODY_FONT_SIZE)
                                .strong()
                                .color(diff_del),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !self.undo_stack.is_empty() {
                            let undo_fg = contrast_text_c32(b_med, Some(diff_mod), palette);
                            let undo_btn = egui::Button::new(
                                egui::RichText::new("⟲ Undo (Ctrl+Z)").size(crate::ui_ext::BODY_FONT_SIZE).strong().color(undo_fg),
                            )
                            .fill(b_med)
                            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
                            .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                            if ui.add(undo_btn).clicked() {
                                self.undo();
                            }
                        }

                        // Direct Slot Push buttons
                        let rh_fg = contrast_text_c32(b_low, Some(diff_add), palette);
                        let push_rh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Right Hand").size(crate::ui_ext::BODY_FONT_SIZE).color(rh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0_f32, diff_add))
                        .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                        .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                        if ui.add(push_rh).on_hover_text("Push current depth map directly to Right Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Right);
                        }

                        let lh_fg = contrast_text_c32(b_low, Some(base_c), palette);
                        let push_lh = egui::Button::new(
                            egui::RichText::new("📋 Slot: Left Hand").size(crate::ui_ext::BODY_FONT_SIZE).color(lh_fg),
                        )
                        .fill(b_low)
                        .stroke(Stroke::new(1.0_f32, base_c))
                        .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                        .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));
                        if ui.add(push_lh).on_hover_text("Push current depth map directly to Left Hand slot").clicked() {
                            self.copy_to_hand_slot(HandSide::Left);
                        }
                    });
                });
            });
    }
}
