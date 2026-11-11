use super::*;
use little_clip_core::CommandRequest;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

type InpaintResult = Result<(RgbaImage, Duration), String>;

pub(crate) struct InpaintJob {
    revision: u64,
    cancel: Arc<AtomicBool>,
    ticket: JobTicket,
    progress: Receiver<String>,
    completion: Receiver<Result<InpaintResult, JobError>>,
}

impl Drop for InpaintJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

impl ImagePreviewState {
    /// Triggers LaMa inpainting on a background worker.
    pub fn run_lama_inpaint(&mut self) {
        self.run_lama_inpaint_on(BackgroundJobs::global());
    }

    pub fn run_lama_inpaint_on(&mut self, jobs: &BackgroundJobs) {
        if self.inpaint_job.is_some() {
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
        let Some(mask) = self.redaction_state.inpaint_mask.clone() else {
            return;
        };
        let pixels = Arc::clone(&self.current_rgba);
        let revision = self.pixel_revision;
        let (cmd, args) = little_clip_core::process::resolve_python_script_command(
            self.inpaint_cmd.as_deref(),
            "inpaint_lama.py",
        );
        let request = CommandRequest::new(cmd).args(args);
        let model_url = self.lama_model_url.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, progress) = std::sync::mpsc::channel();
        let (ticket, completion) = jobs.submit(Duration::MAX, move || {
            let start = Instant::now();
            let model_path = crate::onnx_runtime::ensure_model_cancellable(
                crate::onnx_runtime::BIG_LAMA_MODEL_NAME,
                model_url.as_deref(),
                &worker_cancel,
                |_, _| {},
            )
            .ok();
            if worker_cancel.load(Ordering::Acquire) {
                return Err("Inpainting cancelled".into());
            }
            if let Some(path) = &model_path {
                let _ = tx.send("Running LaMa inpainting neural network…".to_owned());
                match run_native_lama_inpaint(&pixels, &mask, path) {
                    Ok(result) => return Ok((result, start.elapsed())),
                    Err(error) => {
                        let _ = tx.send(format!("Native ORT failed ({error}), trying fallback…"));
                    }
                }
            }
            run_inpaint_helper(
                request,
                &pixels,
                &mask,
                model_path.as_deref(),
                model_url.as_deref(),
                &worker_cancel,
            )
            .map(|result| (result, start.elapsed()))
        });

        self.inpaint_job = Some(InpaintJob {
            revision,
            cancel,
            ticket,
            progress,
            completion,
        });
        self.redaction_state.inpaint_in_progress = true;
        self.redaction_state.inpaint_status = Some("Starting LaMa inpainting…".into());
        self.redaction_state.inpaint_error = None;
    }

    pub(crate) fn cancel_lama_inpaint(&mut self) {
        self.inpaint_job = None;
        self.redaction_state.inpaint_in_progress = false;
        self.redaction_state.inpaint_status = None;
    }

    pub fn poll_lama_inpaint(&mut self) {
        let Some(job) = &self.inpaint_job else {
            return;
        };
        for status in job.progress.try_iter() {
            self.redaction_state.inpaint_status = Some(status);
        }
        let result = match job.completion.try_recv() {
            Ok(result) => result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Inpainting worker disconnected".into()),
        };
        let current = job.revision == self.pixel_revision;
        self.cancel_lama_inpaint();
        match result {
            Ok((pixels, elapsed)) if current => {
                self.undo_stack.push(Arc::clone(&self.current_rgba));
                self.redo_stack.clear();
                self.current_rgba = Arc::new(pixels);
                self.mark_pixels_changed();
                self.dirty_texture = true;
                self.clear_inpaint_mask();
                self.redaction_state.inpaint_error = None;
                self.show_toast(format!(
                    "LaMa inpainting complete ({:.2}s)",
                    elapsed.as_secs_f64()
                ));
            }
            Ok(_) => {
                self.show_toast("Image changed while inpainting. The old result was discarded.")
            }
            Err(error) => {
                self.redaction_state.inpaint_error = Some(error.clone());
                self.show_toast(crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    format!("Inpainting failed: {error}"),
                ));
            }
        }
    }
}

fn run_inpaint_helper(
    mut request: CommandRequest,
    pixels: &RgbaImage,
    mask: &image::GrayImage,
    model_path: Option<&std::path::Path>,
    model_url: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<RgbaImage, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Inpainting cancelled".into());
    }
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = dir.path().join("input.png");
    let mask_path = dir.path().join("mask.png");
    let output = dir.path().join("output.png");
    pixels.save(&input).map_err(|error| error.to_string())?;
    mask.save(&mask_path).map_err(|error| error.to_string())?;
    if let Some(path) = model_path {
        request = request
            .arg("--model")
            .arg_content_file(path)
            .map_err(|error| error.to_string())?;
    } else if let Some(url) = model_url {
        request = request.args(["--model-url", url]);
    }
    let result = request
        .arg_content_file(&input)
        .and_then(|request| request.arg_content_file(&mask_path))
        .and_then(|request| request.arg_content_output(&output))
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(30 * 60))
        .own_process_group()
        .die_with_parent()
        .run_cancellable(cancelled)
        .map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "LaMa process exited with {}: {}",
            result.status,
            result.stderr_lossy()
        ));
    }
    decode_inpaint_output(&output, pixels.dimensions())
}

fn decode_inpaint_output(
    path: &std::path::Path,
    expected: (u32, u32),
) -> Result<RgbaImage, String> {
    use image::ImageDecoder;

    const MAX_ENCODED_BYTES: usize = 128 * 1024 * 1024;
    let bytes = crate::file_source::read_file_bytes(path, MAX_ENCODED_BYTES, true, true)
        .map_err(|error| error.to_string())?;
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    reader.limits(little_clip_core::item::raster_preview_limits());
    let decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    if decoder.dimensions() != expected {
        return Err(format!(
            "Inpaint output dimensions {:?} do not match {expected:?}",
            decoder.dimensions()
        ));
    }
    image::DynamicImage::from_decoder(decoder)
        .map(|image| image.into_rgba8())
        .map_err(|error| error.to_string())
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

    if mask.dimensions() != image.dimensions() {
        return Err("Inpainting mask dimensions do not match the source image".into());
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

    if out_tensor.shape() != [1, 3, padded_h as usize, padded_w as usize] {
        return Err(format!(
            "Unexpected LaMa output tensor dimensions: {:?}",
            out_tensor.shape()
        ));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inpaint_queue_rejection_clears_busy_state_without_touching_pixels_or_mask() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started, ready) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let _blocker = jobs.submit(Duration::MAX, move || {
            started.send(()).unwrap();
            let _ = blocked.recv();
        });
        ready.recv_timeout(Duration::from_secs(3)).unwrap();
        let _queued = jobs.submit(Duration::MAX, || ());
        let mut state = crate::image_preview::tests::image_state_from_png((8, 8), "queue test");
        state.ensure_inpaint_mask().fill(255);
        let original = Arc::clone(&state.current_rgba);

        state.run_lama_inpaint_on(&jobs);
        assert!(state.redaction_state.inpaint_in_progress);
        state.poll_lama_inpaint();
        release.send(()).unwrap();

        assert!(!state.redaction_state.inpaint_in_progress);
        assert!(state.inpaint_job.is_none());
        assert!(
            state
                .redaction_state
                .inpaint_error
                .as_deref()
                .unwrap()
                .contains("queue is full")
        );
        assert!(Arc::ptr_eq(&state.current_rgba, &original));
        assert!(state.inpaint_has_mask());
    }

    #[test]
    fn stale_inpaint_completion_preserves_newer_pixels_and_mask() {
        let mut state = crate::image_preview::tests::image_state_from_png((8, 8), "revision test");
        let jobs = BackgroundJobs::new(1, 1);
        let (_, progress) = std::sync::mpsc::channel();
        let (ticket, completion) = jobs.submit(Duration::MAX, || {
            Ok((
                RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])),
                Duration::ZERO,
            ))
        });
        state.inpaint_job = Some(InpaintJob {
            revision: state.pixel_revision,
            cancel: Arc::new(AtomicBool::new(false)),
            ticket,
            progress,
            completion,
        });
        state.redaction_state.inpaint_in_progress = true;
        state.mark_pixels_changed();
        state.ensure_inpaint_mask().fill(255);
        let current = Arc::clone(&state.current_rgba);
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.inpaint_job.is_some() {
            state.poll_lama_inpaint();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }

        assert!(!state.redaction_state.inpaint_in_progress);
        assert!(Arc::ptr_eq(&state.current_rgba, &current));
        assert!(state.inpaint_has_mask());
        assert!(state.undo_stack.is_empty());
        assert!(
            state
                .status_toast
                .as_ref()
                .unwrap()
                .0
                .contains("old result was discarded")
        );
    }

    #[test]
    fn helper_drains_diagnostics_and_uses_pinned_operands() {
        let pixels = RgbaImage::from_pixel(4, 3, image::Rgba([20, 40, 60, 255]));
        let mask = image::GrayImage::new(4, 3);
        let request = CommandRequest::new("/bin/sh").args([
            "-c",
            r#"case "$1:$2:$3" in
                /proc/self/fd/*:/proc/self/fd/*:/proc/self/fd/*) ;;
                *) exit 2 ;;
            esac
            printf '%300000s' '' >&2
            cat "$1" > "$3""#,
            "inpaint-fixture",
        ]);
        let result =
            run_inpaint_helper(request, &pixels, &mask, None, None, &AtomicBool::new(false))
                .unwrap();
        assert_eq!(result, pixels);
    }

    #[test]
    fn inpaint_output_requires_a_bounded_png_at_the_source_dimensions() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output.png");
        let pixels = RgbaImage::from_pixel(4, 3, image::Rgba([20, 40, 60, 255]));
        pixels.save(&output).unwrap();
        assert_eq!(decode_inpaint_output(&output, (4, 3)).unwrap(), pixels);
        assert!(
            decode_inpaint_output(&output, (3, 4))
                .unwrap_err()
                .contains("dimensions")
        );

        std::fs::File::options()
            .write(true)
            .open(&output)
            .unwrap()
            .set_len(128 * 1024 * 1024 + 1)
            .unwrap();
        assert!(
            decode_inpaint_output(&output, (4, 3))
                .unwrap_err()
                .contains("limit")
        );
    }
}
