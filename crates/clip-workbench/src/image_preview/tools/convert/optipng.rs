use super::*;

impl ImagePreviewState {
    pub fn run_optipng_conversion(&mut self) {
        self.run_optipng_conversion_with(run_optipng_optimization);
    }

    pub(super) fn run_optipng_conversion_with<F>(&mut self, optimizer: F)
    where
        F: Fn(&[u8], &ImageToolProcessControl) -> Result<Vec<u8>, String> + Send + Sync + 'static,
    {
        self.run_optipng_conversion_with_on(
            std::sync::Arc::new(optimizer),
            BackgroundJobs::global(),
        );
    }

    pub(super) fn run_optipng_conversion_with_on(
        &mut self,
        optimizer: std::sync::Arc<OptipngOptimizer>,
        jobs: &BackgroundJobs,
    ) {
        if self.convert_state.is_running_optipng || self.convert_state.optipng_retry_at.is_some() {
            return;
        }
        if !self.convert_state.output_matches(&self.input_stamp()) {
            return;
        }
        if self.convert_state.target_format != ConvertTargetFormat::Png {
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                "OptiPNG is only applicable for PNG format",
            ));
            return;
        }

        let original_bytes = self.convert_state.encoded_bytes.clone();
        if original_bytes.is_empty() {
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Alert,
                "No PNG bytes available to optimize",
            ));
            return;
        }

        let request = OptipngRequestStamp {
            generation: self.convert_state.request_generation,
            input_stamp: self.input_stamp(),
            target_format: self.convert_state.target_format,
            quality: self.convert_state.quality,
            lossless: self.convert_state.lossless,
        };
        let work = OptipngRetryWork {
            request,
            original_bytes,
            optimizer,
        };
        self.start_optipng_job_on(work, jobs);
    }

    pub(super) fn start_optipng_job_on(&mut self, work: OptipngRetryWork, jobs: &BackgroundJobs) {
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        let worker_control = control.clone();
        let worker_optimizer = work.optimizer.clone();
        let original_bytes = work.original_bytes.clone();
        let original_len = original_bytes.len();
        let request = work.request.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_optimizer(&original_bytes, &worker_control)
            }))
            .unwrap_or_else(|_| Err("OptiPNG worker panicked".to_string()));
            OptipngWorkerMsg {
                generation: request.generation,
                input_stamp: request.input_stamp,
                original_len,
                result,
            }
        });
        self.convert_state.optipng_request = Some(work.request.clone());
        self.convert_state.optipng_process_control = Some(control.clone());
        self.convert_state.optipng_job_ticket = Some(ticket);
        self.convert_state.optipng_rx = Some(rx);
        self.convert_state.is_running_optipng = true;
        self.convert_state.optipng_retry_work = Some(work);
        self.convert_state.optipng_retry_at = None;
        self.convert_state.optipng_queue_status = None;
    }

    pub(crate) fn cancel_optipng_process(&mut self) {
        if let Some(ticket) = self.convert_state.optipng_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.convert_state.optipng_process_control.take() {
            control.cancel();
        }
        self.convert_state.optipng_rx = None;
        self.convert_state.optipng_request = None;
        self.convert_state.is_running_optipng = false;
        self.convert_state.optipng_retry_at = None;
        self.convert_state.optipng_retry_work = None;
        self.convert_state.optipng_queue_status = None;
    }

    pub(super) fn poll_optipng(&mut self, jobs: &BackgroundJobs) {
        let current_stamp = self.input_stamp();
        let request_is_stale = self
            .convert_state
            .optipng_request
            .as_ref()
            .is_some_and(|request| {
                request.generation != self.convert_state.request_generation
                    || request.input_stamp != current_stamp
                    || request.target_format != self.convert_state.target_format
                    || request.quality != self.convert_state.quality
                    || request.lossless != self.convert_state.lossless
                    || !self.convert_state.output_matches(&current_stamp)
            });
        if request_is_stale {
            self.cancel_optipng_process();
            return;
        }
        if let Some(retry_at) = self.convert_state.optipng_retry_at {
            if std::time::Instant::now() < retry_at {
                return;
            }
            if let Some(work) = self.convert_state.optipng_retry_work.clone() {
                self.start_optipng_job_on(work, jobs);
            }
            return;
        }
        let Some(rx) = self.convert_state.optipng_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(message)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                self.convert_state.optipng_retry_at = None;
                self.convert_state.optipng_queue_status = None;
                if message.generation != self.convert_state.request_generation
                    || message.input_stamp != current_stamp
                    || self.convert_state.target_format != ConvertTargetFormat::Png
                    || !self.convert_state.output_matches(&current_stamp)
                {
                    return;
                }
                match message.result {
                    Ok(bytes) => {
                        let optimized_len = bytes.len();
                        self.convert_state.encoded_bytes = bytes;
                        self.convert_state.optipng_savings =
                            Some((message.original_len, optimized_len));
                        let pct =
                            if message.original_len > 0 && optimized_len < message.original_len {
                                (message.original_len - optimized_len) as f32
                                    / message.original_len as f32
                                    * 100.0
                            } else {
                                0.0
                            };
                        self.show_toast(crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::Check,
                            format!("OptiPNG complete! Saved {:.1}%", pct),
                        ));
                    }
                    Err(error) => self.show_toast(crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        format!("OptiPNG error: {}", error),
                    )),
                }
            }
            Ok(Err(JobError::QueueFull)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_rx = None;
                self.convert_state.optipng_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.convert_state.optipng_queue_status =
                    Some("Background workers are busy; retrying OptiPNG shortly".to_string());
            }
            Ok(Err(JobError::Cancelled)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                self.convert_state.optipng_rx = None;
            }
            Ok(Err(error)) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                self.convert_state.optipng_rx = None;
                if self.convert_state.target_format == ConvertTargetFormat::Png
                    && self.convert_state.output_matches(&current_stamp)
                {
                    self.show_toast(crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        format!("OptiPNG error: {error}"),
                    ));
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.convert_state.optipng_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.convert_state.is_running_optipng = false;
                self.convert_state.optipng_request = None;
                self.convert_state.optipng_process_control = None;
                self.convert_state.optipng_job_ticket = None;
                self.convert_state.optipng_retry_work = None;
                if self.convert_state.target_format == ConvertTargetFormat::Png
                    && self.convert_state.output_matches(&current_stamp)
                {
                    self.show_toast(crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        "OptiPNG error: worker stopped",
                    ));
                }
            }
        }
    }
}
pub(super) fn run_optipng_optimization(
    png_bytes: &[u8],
    control: &ImageToolProcessControl,
) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut temp_file = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .map_err(|e| format!("Tempfile creation failed: {}", e))?;
    temp_file
        .write_all(png_bytes)
        .map_err(|e| format!("Tempfile write failed: {}", e))?;
    let temp_path = temp_file.path().to_path_buf();

    let mut command = Command::new("optipng");
    command.arg("-o2").arg("-quiet").arg(&temp_path);
    let status = super::super::background_removal::run_image_tool_status_command(command, control)
        .map_err(|error| {
            format!(
                "Failed to run optipng: {}. Ensure optipng is installed.",
                error
            )
        })?;

    if !status.success() {
        return Err(format!(
            "optipng failed with exit status {:?}",
            status.code()
        ));
    }

    std::fs::read(&temp_path).map_err(|e| format!("Failed to read optipng output: {}", e))
}
