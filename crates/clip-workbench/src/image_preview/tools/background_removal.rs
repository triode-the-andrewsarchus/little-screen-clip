use super::super::*;

#[derive(Debug)]
pub enum BgRemovalMsg {
    Progress(String),
    ProgressFraction {
        fraction: f32,
        status: String,
    },
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

pub(crate) fn run_image_tool_status_command(
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

pub fn run_native_background_removal_with_progress(
    image: &RgbaImage,
    model_path: &std::path::Path,
    on_progress: Option<&dyn Fn(f32, &str)>,
) -> Result<RgbaImage, String> {
    let (orig_w, orig_h) = image.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return Err("Image has zero dimension".to_string());
    }

    if let Some(ref cb) = on_progress {
        cb(0.15, "Resizing and normalizing image (1024x1024)...");
    }

    let resized = image::imageops::resize(image, 1024, 1024, image::imageops::FilterType::Triangle);
    let mut img_arr = ndarray::Array4::<f32>::zeros((1, 3, 1024, 1024));

    for y in 0..1024 {
        for x in 0..1024 {
            let px = resized.get_pixel(x, y);
            img_arr[[0, 0, y as usize, x as usize]] = (px[0] as f32 / 255.0 - 0.485) / 0.229;
            img_arr[[0, 1, y as usize, x as usize]] = (px[1] as f32 / 255.0 - 0.456) / 0.224;
            img_arr[[0, 2, y as usize, x as usize]] = (px[2] as f32 / 255.0 - 0.406) / 0.225;
        }
    }

    if let Some(ref cb) = on_progress {
        cb(
            0.25,
            "Loading FeyNoBg neural network weights into ONNX session...",
        );
    }

    let env = crate::onnx_runtime::get_ort_environment()?;
    let mut session = ort::session::Session::builder(&env)
        .map_err(|e| format!("ORT builder failed: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| format!("ORT model load failed: {e}"))?;

    let img_tensor = ort::value::Tensor::from_array(img_arr)
        .map_err(|e| format!("ORT image tensor conversion failed: {e}"))?;

    if let Some(ref cb) = on_progress {
        cb(0.35, "Running FeyNoBg neural network inference...");
    }

    let outputs = session
        .run(ort::inputs![img_tensor])
        .map_err(|e| format!("ORT inference failed: {e}"))?;

    if let Some(ref cb) = on_progress {
        cb(0.95, "Extracting alpha matte and compositing...");
    }

    let out_tensor = outputs[0]
        .try_extract_array::<f32>()
        .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

    let get_alpha = |y: usize, x: usize| -> f32 {
        if out_tensor.ndim() == 4 {
            out_tensor[[0, 0, y, x]]
        } else if out_tensor.ndim() == 3 {
            out_tensor[[0, y, x]]
        } else if out_tensor.ndim() == 2 {
            out_tensor[[y, x]]
        } else {
            0.0
        }
    };

    let mut mask_1024 = image::GrayImage::new(1024, 1024);
    for y in 0..1024 {
        for x in 0..1024 {
            let val = get_alpha(y, x).clamp(0.0, 1.0);
            mask_1024.put_pixel(
                x as u32,
                y as u32,
                image::Luma([(val * 255.0).round() as u8]),
            );
        }
    }

    let mask_orig = image::imageops::resize(
        &mask_1024,
        orig_w,
        orig_h,
        image::imageops::FilterType::Triangle,
    );

    let mut result = image.clone();
    for y in 0..orig_h {
        for x in 0..orig_w {
            let mask_val = mask_orig.get_pixel(x, y)[0];
            let orig_alpha = image.get_pixel(x, y)[3];
            let new_alpha = ((orig_alpha as f32) * (mask_val as f32 / 255.0)).round() as u8;
            let mut px = *image.get_pixel(x, y);
            px[3] = new_alpha;
            result.put_pixel(x, y, px);
        }
    }

    Ok(result)
}

pub fn run_native_background_removal(
    image: &RgbaImage,
    model_path: &std::path::Path,
) -> Result<RgbaImage, String> {
    run_native_background_removal_with_progress(image, model_path, None)
}

pub fn run_native_depth_map_with_progress(
    image: &RgbaImage,
    model_path: &std::path::Path,
    on_progress: Option<&dyn Fn(f32, &str)>,
) -> Result<RgbaImage, String> {
    let (orig_w, orig_h) = image.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return Err("Image has zero dimension".to_string());
    }

    if let Some(ref cb) = on_progress {
        cb(0.15, "Resizing and normalizing image (518x518)...");
    }

    let resized = image::imageops::resize(image, 518, 518, image::imageops::FilterType::Triangle);
    let mut img_arr = ndarray::Array4::<f32>::zeros((1, 3, 518, 518));

    for y in 0..518 {
        for x in 0..518 {
            let px = resized.get_pixel(x, y);
            img_arr[[0, 0, y as usize, x as usize]] = (px[0] as f32 / 255.0 - 0.485) / 0.229;
            img_arr[[0, 1, y as usize, x as usize]] = (px[1] as f32 / 255.0 - 0.456) / 0.224;
            img_arr[[0, 2, y as usize, x as usize]] = (px[2] as f32 / 255.0 - 0.406) / 0.225;
        }
    }

    if let Some(ref cb) = on_progress {
        cb(
            0.25,
            "Loading Depth-Anything V2 weights into ONNX session...",
        );
    }

    let env = crate::onnx_runtime::get_ort_environment()?;
    let mut session = ort::session::Session::builder(&env)
        .map_err(|e| format!("ORT builder failed: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| format!("ORT model load failed: {e}"))?;

    let img_tensor = ort::value::Tensor::from_array(img_arr)
        .map_err(|e| format!("ORT image tensor conversion failed: {e}"))?;

    if let Some(ref cb) = on_progress {
        cb(
            0.35,
            "Running Depth-Anything V2 neural network inference...",
        );
    }

    let outputs = session
        .run(ort::inputs![img_tensor])
        .map_err(|e| format!("ORT inference failed: {e}"))?;

    if let Some(ref cb) = on_progress {
        cb(0.95, "Normalizing depth map values...");
    }

    let out_tensor = outputs[0]
        .try_extract_array::<f32>()
        .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

    let get_depth = |y: usize, x: usize| -> f32 {
        if out_tensor.ndim() == 4 {
            out_tensor[[0, 0, y, x]]
        } else if out_tensor.ndim() == 3 {
            out_tensor[[0, y, x]]
        } else if out_tensor.ndim() == 2 {
            out_tensor[[y, x]]
        } else {
            0.0
        }
    };

    let mut min_val = f32::INFINITY;
    let mut max_val = f32::NEG_INFINITY;
    for y in 0..518 {
        for x in 0..518 {
            let val = get_depth(y, x);
            if val < min_val {
                min_val = val;
            }
            if val > max_val {
                max_val = val;
            }
        }
    }

    let range = if (max_val - min_val) > 1e-6 {
        max_val - min_val
    } else {
        1.0
    };

    let mut depth_518 = image::GrayImage::new(518, 518);
    for y in 0..518 {
        for x in 0..518 {
            let val = get_depth(y, x);
            let norm = ((val - min_val) / range).clamp(0.0, 1.0);
            depth_518.put_pixel(
                x as u32,
                y as u32,
                image::Luma([(norm * 255.0).round() as u8]),
            );
        }
    }

    let depth_orig = image::imageops::resize(
        &depth_518,
        orig_w,
        orig_h,
        image::imageops::FilterType::Triangle,
    );

    let mut result = RgbaImage::new(orig_w, orig_h);
    for y in 0..orig_h {
        for x in 0..orig_w {
            let d = depth_orig.get_pixel(x, y)[0];
            result.put_pixel(x, y, image::Rgba([d, d, d, 255]));
        }
    }

    Ok(result)
}

pub fn run_native_depth_map(
    image: &RgbaImage,
    model_path: &std::path::Path,
) -> Result<RgbaImage, String> {
    run_native_depth_map_with_progress(image, model_path, None)
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
        self.run_background_removal_on(BackgroundJobs::global());
    }

    fn run_background_removal_on(&mut self, jobs: &BackgroundJobs) {
        if self.bg_removal_in_progress {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let snapshot = self.display_image_snapshot_request();

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
        let (command, args) = background_removal_command(self.bg_removal_cmd.as_deref());

        self.bg_removal_input_stamp = Some(snapshot.input_stamp.clone());
        self.bg_removal_request = Some(BgRemovalRequest {
            snapshot,
            prompt: prompt_opt,
            command,
            args,
            operation: BgRemovalOperation::BackgroundRemoval,
            initial_progress: "Invoking FeyNoBg neural network...".to_string(),
            status: status_desc.clone(),
            model_url: self.rmbg_model_url.clone(),
        });
        self.bg_removal_retry_at = None;
        self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
        self.bg_removal_in_progress = true;
        self.bg_removal_started_at = Some(std::time::Instant::now());
        self.bg_removal_progress = Some(0.0);
        self.bg_removal_status = Some(status_desc);
        self.bg_removal_error = None;
        self.submit_image_tool_attempt(jobs);
    }

    pub(super) fn submit_image_tool_attempt(&mut self, jobs: &BackgroundJobs) {
        let Some(request) = self.bg_removal_request.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel::<BgRemovalMsg>();
        self.bg_removal_rx = Some(rx);
        self.bg_removal_retry_at = None;
        self.bg_removal_status = Some(request.status.clone());
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.image_tool_cancel = Some(cancel.clone());
        self.image_tool_child_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let child_pid = self.image_tool_child_pid.clone();
        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let start = std::time::Instant::now();
            let init_msg = if let Some(ref p) = request.prompt {
                format!("Invoking MultiMatte for '{}'...", p)
            } else {
                request.initial_progress.clone()
            };
            let _ = tx.send(BgRemovalMsg::Progress(init_msg));

            // Native ORT fast path if using default/bundled scripts without custom overrides
            let is_default_bg_cmd = request.command == "scripts/remove_bg.py"
                || request.command == "./scripts/remove_bg.py"
                || (request.command == "python3"
                    && request
                        .args
                        .first()
                        .is_some_and(|a| a.contains("remove_bg.py")));

            let is_default_depth_cmd = request.command.contains("depth_estimation.py")
                || request
                    .args
                    .iter()
                    .any(|a| a.contains("depth_estimation.py"));

            if request.operation == BgRemovalOperation::BackgroundRemoval
                && request.prompt.is_none()
                && is_default_bg_cmd
            {
                let tx_dl = tx.clone();
                let on_dl = move |frac: f32, status: &str| {
                    let _ = tx_dl.send(BgRemovalMsg::ProgressFraction {
                        fraction: frac * 0.40,
                        status: status.to_string(),
                    });
                };
                if let Ok(model_path) = crate::onnx_runtime::ensure_model_with_progress(
                    crate::onnx_runtime::FEYNOBG_FP16_MODEL_NAME,
                    request.model_url.as_deref(),
                    on_dl,
                ) {
                    let tx_run = tx.clone();
                    let on_run = move |frac: f32, status: &str| {
                        let _ = tx_run.send(BgRemovalMsg::ProgressFraction {
                            fraction: frac,
                            status: status.to_string(),
                        });
                    };
                    let source_rgba = request.snapshot.compose();
                    match run_native_background_removal_with_progress(
                        &source_rgba,
                        &model_path,
                        Some(&on_run),
                    ) {
                        Ok(result_rgba) => {
                            let _ = tx.send(BgRemovalMsg::Success {
                                result_rgba,
                                duration_ms: start.elapsed().as_millis(),
                            });
                            return;
                        }
                        Err(err) => {
                            let _ = tx.send(BgRemovalMsg::Progress(format!(
                                "Native FeyNoBg failed ({err}), trying fallback..."
                            )));
                        }
                    }
                }
            } else if request.operation == BgRemovalOperation::DepthMap && is_default_depth_cmd {
                let tx_dl = tx.clone();
                let on_dl = move |frac: f32, status: &str| {
                    let _ = tx_dl.send(BgRemovalMsg::ProgressFraction {
                        fraction: frac * 0.40,
                        status: status.to_string(),
                    });
                };
                if let Ok(depth_path) = crate::onnx_runtime::ensure_model_with_progress(
                    crate::onnx_runtime::DEPTH_ANYTHING_V2_MODEL_NAME,
                    request.model_url.as_deref(),
                    on_dl,
                ) {
                    let tx_run = tx.clone();
                    let on_run = move |frac: f32, status: &str| {
                        let _ = tx_run.send(BgRemovalMsg::ProgressFraction {
                            fraction: frac,
                            status: status.to_string(),
                        });
                    };
                    let source_rgba = request.snapshot.compose();
                    match run_native_depth_map_with_progress(
                        &source_rgba,
                        &depth_path,
                        Some(&on_run),
                    ) {
                        Ok(result_rgba) => {
                            let _ = tx.send(BgRemovalMsg::Success {
                                result_rgba,
                                duration_ms: start.elapsed().as_millis(),
                            });
                            return;
                        }
                        Err(err) => {
                            let _ = tx.send(BgRemovalMsg::Progress(format!(
                                "Native Depth-Anything failed ({err}), trying fallback..."
                            )));
                        }
                    }
                }
            }

            let png_bytes = match request.snapshot.compose_png() {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = tx.send(BgRemovalMsg::Error(format!(
                        "Failed to encode image to PNG: {error}"
                    )));
                    return;
                }
            };

            let mut cmd = Command::new(&request.command);
            cmd.args(&request.args);
            let result = run_image_tool_subprocess(
                cmd,
                &png_bytes,
                request.prompt.as_deref(),
                Some(&cancel),
                Some(&child_pid),
                request.operation,
                &request.command,
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
        self.bg_removal_ticket = Some(ticket);
        self.bg_removal_job_rx = Some(job_rx);
    }

    /// Polls background removal or depth estimation worker completion.
    pub fn poll_bg_removal(&mut self) {
        self.poll_bg_removal_on(BackgroundJobs::global());
    }

    pub(super) fn poll_bg_removal_on(&mut self, jobs: &BackgroundJobs) {
        if self.bg_removal_in_progress
            && self.bg_removal_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.cancel_image_tool_process();
            self.bg_removal_rx = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_progress = None;
            self.bg_removal_started_at = None;
            return;
        }

        if self
            .bg_removal_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.submit_image_tool_attempt(jobs);
        } else if self.bg_removal_retry_at.is_some() {
            return;
        }

        if let Some(job_rx) = self.bg_removal_job_rx.as_ref() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.bg_removal_job_rx = None;
                    self.bg_removal_ticket = None;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.bg_removal_job_rx = None;
                    self.bg_removal_ticket = None;
                    self.bg_removal_rx = None;
                    self.bg_removal_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                    let label = match self.bg_removal_operation {
                        BgRemovalOperation::BackgroundRemoval => "Background removal",
                        BgRemovalOperation::DepthMap => "Depth Map",
                    };
                    self.bg_removal_status =
                        Some(format!("{label} waiting for workers; retrying shortly"));
                    return;
                }
                Ok(Err(JobError::Cancelled)) => {
                    self.bg_removal_job_rx = None;
                    self.bg_removal_ticket = None;
                    self.bg_removal_rx = None;
                    self.bg_removal_input_stamp = None;
                    self.bg_removal_request = None;
                    self.bg_removal_retry_at = None;
                    self.bg_removal_in_progress = false;
                    self.bg_removal_status = None;
                    self.bg_removal_progress = None;
                    self.bg_removal_started_at = None;
                    return;
                }
                Ok(Err(error)) => {
                    self.bg_removal_job_rx = None;
                    self.bg_removal_ticket = None;
                    self.bg_removal_rx = None;
                    self.bg_removal_input_stamp = None;
                    self.bg_removal_request = None;
                    self.bg_removal_retry_at = None;
                    self.bg_removal_in_progress = false;
                    self.bg_removal_status = None;
                    self.bg_removal_progress = None;
                    self.bg_removal_started_at = None;
                    let label = match self.bg_removal_operation {
                        BgRemovalOperation::BackgroundRemoval => "Background removal",
                        BgRemovalOperation::DepthMap => "Depth Map",
                    };
                    self.bg_removal_error = Some(format!("{label} worker failed: {error}"));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.bg_removal_job_rx = None;
                    self.bg_removal_ticket = None;
                    self.bg_removal_rx = None;
                    self.bg_removal_input_stamp = None;
                    self.bg_removal_request = None;
                    self.bg_removal_retry_at = None;
                    self.bg_removal_in_progress = false;
                    self.bg_removal_status = None;
                    self.bg_removal_progress = None;
                    self.bg_removal_started_at = None;
                    let label = match self.bg_removal_operation {
                        BgRemovalOperation::BackgroundRemoval => "Background removal",
                        BgRemovalOperation::DepthMap => "Depth Map",
                    };
                    self.bg_removal_error = Some(format!("{label} worker stopped"));
                    return;
                }
            }
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
                    Ok(BgRemovalMsg::ProgressFraction { fraction, status }) => {
                        self.bg_removal_progress = Some(fraction);
                        self.bg_removal_status = Some(status);
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
            self.bg_removal_progress = None;
            self.bg_removal_started_at = None;
            self.bg_removal_rx = None;
            self.bg_removal_job_rx = None;
            self.bg_removal_ticket = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_request = None;
            self.bg_removal_retry_at = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
            self.bg_removal_error = None;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;

            match operation {
                BgRemovalOperation::BackgroundRemoval => {
                    self.last_bg_removal_duration_ms = Some(duration_ms);
                }
                BgRemovalOperation::DepthMap => {
                    self.last_depth_map_duration_ms = Some(duration_ms);
                }
            }

            let elapsed = duration_ms as f64 / 1000.0;
            self.show_toast(crate::ui_ext::StatusMessage::new(
                match operation {
                    BgRemovalOperation::BackgroundRemoval => crate::icons::Icon::Sparkles,
                    BgRemovalOperation::DepthMap => crate::icons::Icon::Waves,
                },
                match operation {
                    BgRemovalOperation::BackgroundRemoval => {
                        format!("Background removed in {elapsed:.2}s!")
                    }
                    BgRemovalOperation::DepthMap => {
                        format!("Depth map generated in {elapsed:.2}s!")
                    }
                },
            ));
        } else if let Some(err) = error_msg {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_progress = None;
            self.bg_removal_started_at = None;
            self.bg_removal_rx = None;
            self.bg_removal_job_rx = None;
            self.bg_removal_ticket = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_request = None;
            self.bg_removal_retry_at = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
            self.bg_removal_error = Some(err);
        } else if disconnected {
            self.bg_removal_in_progress = false;
            self.bg_removal_status = None;
            self.bg_removal_progress = None;
            self.bg_removal_started_at = None;
            self.bg_removal_rx = None;
            self.bg_removal_job_rx = None;
            self.bg_removal_ticket = None;
            self.bg_removal_input_stamp = None;
            self.bg_removal_request = None;
            self.bg_removal_retry_at = None;
            self.bg_removal_operation = BgRemovalOperation::BackgroundRemoval;
        }
    }

    pub(crate) fn cancel_image_tool_process(&mut self) {
        if let Some(ticket) = self.bg_removal_ticket.take() {
            ticket.cancel();
        }
        self.bg_removal_job_rx = None;
        self.bg_removal_request = None;
        self.bg_removal_retry_at = None;
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
        self.bg_removal_progress = None;
        self.bg_removal_started_at = None;
    }

    pub fn effective_bg_removal_progress(&self) -> f32 {
        if !self.bg_removal_in_progress {
            return 0.0;
        }

        let base_progress = self.bg_removal_progress.unwrap_or(0.0);

        if let Some(ref status) = self.bg_removal_status {
            if status.contains("Downloading") || status.contains("Connecting to download") {
                return (base_progress * 0.45).clamp(0.02, 0.45);
            }
        }

        let started = match self.bg_removal_started_at {
            Some(t) => t,
            None => return base_progress.clamp(0.05, 0.95),
        };
        let elapsed = started.elapsed().as_secs_f32();

        let expected_duration = match self.bg_removal_operation {
            BgRemovalOperation::BackgroundRemoval => self
                .last_bg_removal_duration_ms
                .map(|ms| (ms as f32 / 1000.0).max(1.0))
                .unwrap_or(11.0),
            BgRemovalOperation::DepthMap => self
                .last_depth_map_duration_ms
                .or(self.last_bg_removal_duration_ms)
                .map(|ms| (ms as f32 / 1000.0).max(0.5))
                .unwrap_or(2.0),
        };

        let start_p = base_progress.max(0.15);
        let target_p = 0.95;
        let tau = (expected_duration * 0.75).max(0.5);
        let curve = 1.0 - (-elapsed / tau).exp();
        (start_p + (target_p - start_p) * curve).clamp(0.05, 0.96)
    }

    /// Renders the background removal tool options, progress bar, and actions.
    pub(crate) fn render_remove_bg_toolbar(
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
            let progress = self.effective_bg_removal_progress();
            let pct = (progress * 100.0).round() as u32;
            let elapsed_sec = self
                .bg_removal_started_at
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);

            ui.add_space(crate::ui_ext::SPACE_XS);
            let bar_text = format!("{pct}% • {elapsed_sec:.1}s");
            let progress_bar = egui::ProgressBar::new(progress)
                .fill(action_color)
                .animate(true)
                .desired_width(ui.available_width())
                .text(bar_text);
            ui.add(progress_bar);
            ui.add_space(crate::ui_ext::SPACE_XS);

            let status_text = self
                .bg_removal_status
                .as_deref()
                .unwrap_or("Removing background with FeyNoBg…");
            ui.label(
                egui::RichText::new(status_text)
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

#[cfg(all(test, unix))]
mod bounded_job_tests {
    use super::*;
    use crate::image_preview::ImagePreviewState;
    use little_clip_core::AppConfig;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn state_with_command(command: String) -> ImagePreviewState {
        let image = RgbaImage::from_pixel(5, 4, image::Rgba([20, 80, 140, 255]));
        let mut encoded = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut encoded),
                image::ImageFormat::Png,
            )
            .unwrap();
        let mut state = ImagePreviewState::from_bytes_with_meta(
            encoded,
            "image/png".into(),
            None,
            "background removal job test".into(),
            &AppConfig::default(),
        )
        .unwrap();
        state.bg_removal_cmd = Some(command);
        state
    }

    fn write_script(directory: &Path, name: &str, body: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        path
    }

    fn wait_for_removal(state: &mut ImagePreviewState, jobs: &BackgroundJobs) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.bg_removal_in_progress && Instant::now() < deadline {
            state.poll_bg_removal_on(jobs);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !state.bg_removal_in_progress,
            "synthetic background-removal worker did not finish"
        );
    }

    #[test]
    fn queue_full_keeps_request_and_retries_after_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let output_image = RgbaImage::from_pixel(3, 2, image::Rgba([230, 30, 40, 255]));
        let mut output_png = Vec::new();
        output_image
            .write_to(
                &mut std::io::Cursor::new(&mut output_png),
                image::ImageFormat::Png,
            )
            .unwrap();
        let output_path = directory.path().join("result.png");
        std::fs::write(&output_path, output_png).unwrap();
        let script = write_script(
            directory.path(),
            "fake-remove-bg.sh",
            &format!("cat >/dev/null\ncat '{}'", output_path.display()),
        );
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_blocker_ticket, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_filler_ticket, filler_rx) = jobs.submit(Duration::MAX, || ());
        let mut state = state_with_command(format!("sh {}", script.display()));

        state.run_background_removal_on(&jobs);
        let retained_pixels =
            Arc::clone(&state.bg_removal_request.as_ref().unwrap().snapshot.pixels);
        state.poll_bg_removal_on(&jobs);
        let retry_at = state
            .bg_removal_retry_at
            .expect("QueueFull must schedule retry");
        assert!(state.bg_removal_in_progress);
        assert!(
            state
                .bg_removal_status
                .as_deref()
                .unwrap()
                .contains("retrying")
        );
        assert!(Arc::ptr_eq(
            &retained_pixels,
            &state.bg_removal_request.as_ref().unwrap().snapshot.pixels
        ));
        state.poll_bg_removal_on(&jobs);
        assert_eq!(state.bg_removal_retry_at, Some(retry_at));

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
        wait_for_removal(&mut state, &jobs);

        assert!(state.bg_removal_error.is_none());
        assert_eq!(state.current_rgba.as_ref(), &output_image);
        assert_eq!(state.undo_stack.len(), 1);
    }

    #[test]
    fn queued_request_cancellation_prevents_helper_start() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("started");
        let script = write_script(
            directory.path(),
            "queued-remove-bg.sh",
            &format!("echo started > '{}'\ncat >/dev/null", marker.display()),
        );
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (_blocker_ticket, blocker_rx) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut state = state_with_command(format!("sh {}", script.display()));
        state.run_background_removal_on(&jobs);
        assert!(state.bg_removal_in_progress);
        state.cancel_image_tool_process();
        assert!(!state.bg_removal_in_progress);
        assert!(state.bg_removal_retry_at.is_none());

        release_tx.send(()).unwrap();
        blocker_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (_, result_rx) = jobs.submit(Duration::MAX, || ());
            match result_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(())) => break,
                Ok(Err(JobError::QueueFull)) | Err(mpsc::RecvTimeoutError::Timeout)
                    if Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("scheduler did not drain canceled job: {other:?}"),
            }
        }
        assert!(!marker.exists());
    }

    #[test]
    fn running_cancellation_kills_and_reaps_fake_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("worker.pid");
        let script = write_script(
            directory.path(),
            "running-remove-bg.sh",
            &format!(
                "echo $$ > '{}'\ncat >/dev/null\nsleep 30",
                pid_file.display()
            ),
        );
        let jobs = BackgroundJobs::new(1, 1);
        let mut state = state_with_command(format!("sh {}", script.display()));
        state.run_background_removal_on(&jobs);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            state.poll_bg_removal_on(&jobs);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pid_file.exists(), "synthetic child did not start");
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);

        state.cancel_image_tool_process();
        assert!(!state.bg_removal_in_progress);
        let reap_deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < reap_deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(state.image_tool_child_pid.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn native_background_removal_zero_dimension_fails() {
        let empty = RgbaImage::new(0, 0);
        let dummy = std::path::Path::new("dummy.onnx");
        assert!(run_native_background_removal(&empty, dummy).is_err());
    }

    #[test]
    fn native_depth_map_zero_dimension_fails() {
        let empty = RgbaImage::new(0, 0);
        let dummy = std::path::Path::new("dummy.onnx");
        assert!(run_native_depth_map(&empty, dummy).is_err());
    }

    #[test]
    fn native_background_removal_runs_if_model_present() {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let model_path = std::path::PathBuf::from(home)
            .join(".cache/little-screen-clip/models/feynobg-fp16.onnx");
        if !model_path.exists() {
            return;
        }

        let img = RgbaImage::from_pixel(32, 24, image::Rgba([120, 150, 200, 255]));
        let result =
            run_native_background_removal(&img, &model_path).expect("native feynobg inference");
        assert_eq!(result.dimensions(), (32, 24));
    }

    #[test]
    fn native_depth_map_runs_if_model_present() {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let model_path = std::path::PathBuf::from(home)
            .join(".cache/little-screen-clip/models/depth-anything-v2-small.onnx");
        if !model_path.exists() {
            return;
        }

        let img = RgbaImage::from_pixel(32, 24, image::Rgba([120, 150, 200, 255]));
        let result = run_native_depth_map(&img, &model_path).expect("native depth inference");
        assert_eq!(result.dimensions(), (32, 24));
        let px = result.get_pixel(10, 10);
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
        assert_eq!(px[3], 255);
    }

    #[test]
    fn effective_bg_removal_progress_eases_smoothly_within_bounds() {
        let mut state = state_with_command("dummy".into());
        assert_eq!(state.effective_bg_removal_progress(), 0.0);

        state.bg_removal_in_progress = true;
        state.bg_removal_started_at = Some(Instant::now());
        state.last_bg_removal_duration_ms = Some(2000);

        let p0 = state.effective_bg_removal_progress();
        assert!(p0 >= 0.05 && p0 <= 0.35);

        // Advance artificial start time into the future of the calculation
        state.bg_removal_started_at = Some(Instant::now() - Duration::from_secs(1));
        let p1 = state.effective_bg_removal_progress();
        assert!(p1 > p0);
        assert!(p1 < 0.96);

        state.bg_removal_started_at = Some(Instant::now() - Duration::from_secs(10));
        let p_late = state.effective_bg_removal_progress();
        assert!(p_late >= p1);
        assert!(p_late <= 0.96);

        // Downloading status scaling
        state.bg_removal_status = Some("Downloading feynobg-fp16.onnx: 50% (280/563 MB)".into());
        state.bg_removal_progress = Some(0.50);
        let p_dl = state.effective_bg_removal_progress();
        assert!((p_dl - 0.225).abs() < 0.05);
    }

    #[test]
    fn bg_removal_progress_message_updates_state_and_success_clears_it() {
        let mut state = state_with_command("dummy".into());
        let (tx, rx) = std::sync::mpsc::channel();
        state.bg_removal_in_progress = true;
        state.bg_removal_started_at = Some(Instant::now());
        state.bg_removal_input_stamp = Some(state.input_stamp());
        state.bg_removal_rx = Some(rx);

        tx.send(BgRemovalMsg::ProgressFraction {
            fraction: 0.35,
            status: "Running FeyNoBg neural network inference...".into(),
        })
        .unwrap();

        state.poll_bg_removal();
        assert_eq!(state.bg_removal_progress, Some(0.35));
        assert_eq!(
            state.bg_removal_status.as_deref(),
            Some("Running FeyNoBg neural network inference...")
        );

        let res_img = RgbaImage::new(16, 16);
        tx.send(BgRemovalMsg::Success {
            result_rgba: res_img,
            duration_ms: 1250,
        })
        .unwrap();

        state.poll_bg_removal();
        assert!(!state.bg_removal_in_progress);
        assert_eq!(state.bg_removal_progress, None);
        assert_eq!(state.bg_removal_started_at, None);
        assert_eq!(state.last_bg_removal_duration_ms, Some(1250));
    }
}
