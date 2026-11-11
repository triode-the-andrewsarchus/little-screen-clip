use super::*;

impl ImagePreviewState {
    /// Triggers LaMa inpainting on background worker.
    pub fn run_lama_inpaint(&mut self) {
        self.run_lama_inpaint_on(BackgroundJobs::global());
    }

    pub fn run_lama_inpaint_on(&mut self, jobs: &BackgroundJobs) {
        if self.redaction_state.inpaint_in_progress {
            return;
        }

        if !self.inpaint_has_mask() {
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                "Draw an inpainting mask first (brush or rectangle)",
            ));
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let current_image = self.current_rgba.as_ref().clone();
        let mask_image = match self.redaction_state.inpaint_mask {
            Some(ref m) => m.clone(),
            None => return,
        };

        let (cmd, args) = little_clip_core::process::resolve_python_script_command(
            self.inpaint_cmd.as_deref(),
            "inpaint_lama.py",
        );

        let (tx, rx) = std::sync::mpsc::channel::<LamaInpaintMsg>();
        self.inpaint_rx = Some(rx);
        self.redaction_state.inpaint_in_progress = true;
        self.redaction_state.inpaint_status = Some("Starting LaMa inpainting...".to_string());
        self.redaction_state.inpaint_error = None;

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.inpaint_cancel = Some(cancel.clone());
        self.inpaint_child_pid = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let child_pid = self.inpaint_child_pid.clone();
        let lama_url = self.lama_model_url.clone();

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let start = std::time::Instant::now();
            let _ = tx.send(LamaInpaintMsg::Progress(
                "Running LaMa inpainting neural network...".to_string(),
            ));

            // Priority 1: Native Rust ORT inference if model is available
            let model_path = crate::onnx_runtime::ensure_model_cancellable(
                crate::onnx_runtime::BIG_LAMA_MODEL_NAME,
                lama_url.as_deref(),
                &cancel,
                |_, _| {},
            )
            .ok();

            if let Some(ref model_path) = model_path {
                match run_native_lama_inpaint(&current_image, &mask_image, model_path) {
                    Ok(result_rgba) => {
                        let _ = tx.send(LamaInpaintMsg::Success {
                            result_rgba,
                            duration_ms: start.elapsed().as_millis(),
                        });
                        return;
                    }
                    Err(native_err) => {
                        let _ = tx.send(LamaInpaintMsg::Progress(format!(
                            "Native ORT failed ({native_err}), trying fallback..."
                        )));
                    }
                }
            }

            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            // Priority 2: Fallback to scripts/inpaint_lama.py
            let temp_in =
                std::env::temp_dir().join(format!("clip_inpaint_in_{}.png", uuid::Uuid::new_v4()));
            let temp_mask = std::env::temp_dir()
                .join(format!("clip_inpaint_mask_{}.png", uuid::Uuid::new_v4()));
            let temp_out =
                std::env::temp_dir().join(format!("clip_inpaint_out_{}.png", uuid::Uuid::new_v4()));

            if let Err(err) = current_image.save_with_format(&temp_in, image::ImageFormat::Png) {
                let _ = tx.send(LamaInpaintMsg::Error(format!(
                    "Failed to save temp image: {err}"
                )));
                return;
            }

            if let Err(err) = mask_image.save_with_format(&temp_mask, image::ImageFormat::Png) {
                let _ = std::fs::remove_file(&temp_in);
                let _ = tx.send(LamaInpaintMsg::Error(format!(
                    "Failed to save temp mask: {err}"
                )));
                return;
            }

            let mut process_cmd = Command::new(&cmd);
            process_cmd.args(&args);
            if let Some(ref path) = model_path {
                process_cmd.arg("--model").arg(path);
            } else if let Some(ref url) = lama_url {
                process_cmd.arg("--model-url").arg(url);
                process_cmd.env("LAMA_MODEL_URL", url);
            }
            process_cmd.arg(&temp_in);
            process_cmd.arg(&temp_mask);
            process_cmd.arg(&temp_out);

            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                process_cmd.process_group(0);
            }

            let mut child = match process_cmd
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => child,
                Err(err) => {
                    let _ = std::fs::remove_file(&temp_in);
                    let _ = std::fs::remove_file(&temp_mask);
                    let _ = std::fs::remove_file(&temp_out);
                    let _ = tx.send(LamaInpaintMsg::Error(format!(
                        "Failed to launch inpainting process '{cmd}': {err}"
                    )));
                    return;
                }
            };

            let pid = child.id();
            child_pid.store(pid, std::sync::atomic::Ordering::SeqCst);

            let status = child.wait();
            let _ = std::fs::remove_file(&temp_in);
            let _ = std::fs::remove_file(&temp_mask);

            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = std::fs::remove_file(&temp_out);
                let _ = tx.send(LamaInpaintMsg::Error("Inpainting cancelled".to_string()));
                return;
            }

            match status {
                Ok(st) if st.success() => match image::open(&temp_out) {
                    Ok(dyn_img) => {
                        let _ = std::fs::remove_file(&temp_out);
                        let _ = tx.send(LamaInpaintMsg::Success {
                            result_rgba: dyn_img.to_rgba8(),
                            duration_ms: start.elapsed().as_millis(),
                        });
                    }
                    Err(err) => {
                        let _ = std::fs::remove_file(&temp_out);
                        let _ = tx.send(LamaInpaintMsg::Error(format!(
                            "Failed to read inpaint output: {err}"
                        )));
                    }
                },
                Ok(st) => {
                    let mut stderr_msg = String::new();
                    if let Some(mut err_pipe) = child.stderr.take() {
                        use std::io::Read;
                        let _ = err_pipe.read_to_string(&mut stderr_msg);
                    }
                    let _ = std::fs::remove_file(&temp_out);
                    let _ = tx.send(LamaInpaintMsg::Error(format!(
                        "LaMa process exited with code {:?}: {}",
                        st.code(),
                        stderr_msg.trim()
                    )));
                }
                Err(err) => {
                    let _ = std::fs::remove_file(&temp_out);
                    let _ = tx.send(LamaInpaintMsg::Error(format!("Worker failed: {err}")));
                }
            }
        });

        self.inpaint_ticket = Some(ticket);
        self.inpaint_job_rx = Some(job_rx);
    }

    pub fn poll_lama_inpaint(&mut self) {
        self.poll_lama_inpaint_on(BackgroundJobs::global());
    }

    pub fn poll_lama_inpaint_on(&mut self, _jobs: &BackgroundJobs) {
        let mut success = None;
        let mut error = None;

        if let Some(rx) = &self.inpaint_rx {
            loop {
                match rx.try_recv() {
                    Ok(LamaInpaintMsg::Progress(st)) => {
                        self.redaction_state.inpaint_status = Some(st);
                    }
                    Ok(LamaInpaintMsg::Success {
                        result_rgba,
                        duration_ms,
                    }) => {
                        success = Some((result_rgba, duration_ms));
                        break;
                    }
                    Ok(LamaInpaintMsg::Error(err)) => {
                        error = Some(err);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }

        if let Some((result_rgba, duration_ms)) = success {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = Arc::new(result_rgba);
            self.mark_pixels_changed();
            self.dirty_texture = true;
            if let Some(mask) = self.redaction_state.inpaint_mask.as_mut() {
                mask.fill(0);
            }
            self.inpaint_mask_texture = None;
            self.redaction_state.inpaint_in_progress = false;
            self.redaction_state.inpaint_status = None;
            self.redaction_state.inpaint_error = None;
            self.inpaint_rx = None;
            self.inpaint_job_rx = None;
            self.inpaint_ticket = None;
            self.inpaint_cancel = None;
            self.show_toast(format!(
                "✦ LaMa inpainting complete ({:.2}s)",
                duration_ms as f64 / 1000.0
            ));
        } else if let Some(err) = error {
            self.redaction_state.inpaint_in_progress = false;
            self.redaction_state.inpaint_status = None;
            self.redaction_state.inpaint_error = Some(err.clone());
            self.inpaint_rx = None;
            self.inpaint_job_rx = None;
            self.inpaint_ticket = None;
            self.inpaint_cancel = None;
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                format!("Inpainting failed: {err}"),
            ));
        }
    }
}

pub fn run_native_lama_inpaint(
    image: &RgbaImage,
    mask: &image::GrayImage,
    model_path: &std::path::Path,
) -> Result<RgbaImage, String> {
    let (orig_w, orig_h) = image.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return Err("Image has zero dimension".to_string());
    }

    let pad_h = (8 - orig_h % 8) % 8;
    let pad_w = (8 - orig_w % 8) % 8;
    let padded_w = orig_w + pad_w;
    let padded_h = orig_h + pad_h;

    let mut img_arr = ndarray::Array4::<f32>::zeros((1, 3, padded_h as usize, padded_w as usize));
    let mut mask_arr = ndarray::Array4::<f32>::zeros((1, 1, padded_h as usize, padded_w as usize));

    for y in 0..padded_h {
        let src_y = if y < orig_h {
            y
        } else {
            orig_h.saturating_sub(1)
        };
        for x in 0..padded_w {
            let src_x = if x < orig_w {
                x
            } else {
                orig_w.saturating_sub(1)
            };
            let px = image.get_pixel(src_x, src_y);
            img_arr[[0, 0, y as usize, x as usize]] = px[0] as f32 / 255.0;
            img_arr[[0, 1, y as usize, x as usize]] = px[1] as f32 / 255.0;
            img_arr[[0, 2, y as usize, x as usize]] = px[2] as f32 / 255.0;

            let m = mask.get_pixel(src_x, src_y)[0];
            mask_arr[[0, 0, y as usize, x as usize]] = if m > 0 { 1.0 } else { 0.0 };
        }
    }

    let mut session = crate::onnx_runtime::load_session(model_path)
        .map_err(|e| format!("ORT model load failed: {e}"))?;

    let img_tensor = ort::value::Tensor::from_array(img_arr)
        .map_err(|e| format!("ORT image tensor conversion failed: {e}"))?;
    let mask_tensor = ort::value::Tensor::from_array(mask_arr)
        .map_err(|e| format!("ORT mask tensor conversion failed: {e}"))?;

    let outputs = session
        .run(ort::inputs![img_tensor, mask_tensor])
        .map_err(|e| format!("ORT inference failed: {e}"))?;

    let out_tensor = outputs[0]
        .try_extract_array::<f32>()
        .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

    let mut result = image.clone();
    for y in 0..orig_h {
        for x in 0..orig_w {
            let m = mask.get_pixel(x, y)[0];
            if m > 0 {
                let orig_alpha = image.get_pixel(x, y)[3];
                let r = (out_tensor[[0, 0, y as usize, x as usize]].clamp(0.0, 1.0) * 255.0).round()
                    as u8;
                let g = (out_tensor[[0, 1, y as usize, x as usize]].clamp(0.0, 1.0) * 255.0).round()
                    as u8;
                let b = (out_tensor[[0, 2, y as usize, x as usize]].clamp(0.0, 1.0) * 255.0).round()
                    as u8;
                result.put_pixel(x, y, image::Rgba([r, g, b, orig_alpha]));
            }
        }
    }

    Ok(result)
}
