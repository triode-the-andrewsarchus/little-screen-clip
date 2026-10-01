use super::*;

impl ImagePreviewState {
    #[cfg(test)]
    pub fn update_conversion(&mut self) {
        let base_img = self.current_display_image();
        let input_stamp = self.input_stamp();
        let fmt = self.convert_state.target_format;
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = None;
        self.convert_state.render_rx = None;
        self.convert_state.is_encoding = false;
        self.convert_state.render_input_stamp = None;

        match encode_conversion(base_img, fmt, quality, lossless, None, None) {
            Ok((bytes, decoded_rgba)) => {
                self.convert_state.encoded_bytes = bytes;
                self.convert_state.converted_rgba = Some(decoded_rgba);
                self.convert_state.dirty_texture = true;
                self.convert_state.optipng_savings = None;
                self.convert_state.render_error = None;
                self.convert_state.render_error_input_stamp = None;
                self.convert_state.completed_generation = self.convert_state.request_generation;
                self.convert_state.completed_input_stamp = Some(input_stamp);
            }
            Err(e) => {
                self.convert_state.render_error = Some(e.clone());
                self.convert_state.render_error_input_stamp = Some(input_stamp);
                self.show_toast(format!("Conversion failed: {}", e));
            }
        }
    }

    pub fn schedule_conversion(&mut self) {
        self.cancel_optipng_process();
        self.cancel_conversion_worker();
        self.convert_state.request_generation =
            self.convert_state.request_generation.wrapping_add(1);
        self.convert_state.params_dirty_at = Some(std::time::Instant::now());
        self.convert_state.optipng_savings = None;
        self.convert_state.render_error = None;
        self.convert_state.render_error_input_stamp = None;
        self.convert_state.render_queue_status = None;
        self.convert_state.render_retry_input_stamp = None;
    }

    pub fn run_conversion_worker(&mut self) {
        self.run_conversion_worker_on(BackgroundJobs::global());
    }

    pub(super) fn run_conversion_worker_on(&mut self, jobs: &BackgroundJobs) {
        if self.convert_state.is_encoding || self.current_rgba.is_empty() {
            return;
        }

        let format = self.convert_state.target_format;
        let decode_program = if cfg!(test) || format == ConvertTargetFormat::Png {
            None
        } else {
            match std::env::current_exe() {
                Ok(program) => Some(program),
                Err(error) => {
                    self.convert_state.render_error =
                        Some(format!("Conversion decoder unavailable: {error}"));
                    self.convert_state.render_error_input_stamp = Some(self.input_stamp());
                    self.convert_state.params_dirty_at = None;
                    return;
                }
            }
        };
        let decode_control = decode_program
            .as_ref()
            .map(|_| std::sync::Arc::new(ImageToolProcessControl::new()));

        let snapshot = self
            .convert_state
            .render_snapshot
            .take()
            .unwrap_or_else(|| self.display_image_snapshot_request());
        let input_stamp = self.input_stamp();
        let quality = self.convert_state.quality;
        let lossless = self.convert_state.lossless;
        let generation = self.convert_state.request_generation;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.convert_state.render_cancel = Some(cancel.clone());
        self.convert_state.render_decode_control = decode_control.clone();
        self.convert_state.render_input_stamp = Some(input_stamp.clone());
        self.convert_state.render_snapshot = Some(snapshot.clone());
        self.convert_state.is_encoding = true;
        self.convert_state.params_dirty_at = None;
        self.convert_state.render_retry_at = None;
        self.convert_state.render_queue_status = None;
        self.convert_state.render_retry_input_stamp = None;
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                encode_conversion(
                    snapshot.compose(),
                    format,
                    quality,
                    lossless,
                    Some(&cancel),
                    decode_program.as_deref().zip(decode_control.as_deref()),
                )
            }))
            .unwrap_or_else(|_| Err("Conversion worker panicked".to_string()));
            ConvertWorkerMsg {
                generation,
                input_stamp,
                result,
            }
        });
        self.convert_state.render_job_ticket = Some(ticket);
        self.convert_state.render_rx = Some(rx);
    }

    pub fn poll_conversion(&mut self) {
        self.poll_conversion_on(BackgroundJobs::global());
    }

    pub(super) fn poll_conversion_on(&mut self, jobs: &BackgroundJobs) {
        self.poll_optipng(jobs);
        if self.convert_state.is_encoding
            && self.convert_state.render_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.cancel_conversion_worker();
            if self.active_mode == ImageToolMode::Convert {
                self.convert_state.params_dirty_at = Some(std::time::Instant::now());
            }
            return;
        }
        if self.convert_state.render_retry_at.is_some()
            && self.convert_state.render_retry_input_stamp.as_ref() != Some(&self.input_stamp())
        {
            self.convert_state.render_retry_at = None;
            self.convert_state.render_retry_input_stamp = None;
            self.convert_state.render_queue_status = None;
            self.convert_state.render_snapshot = None;
            if self.active_mode == ImageToolMode::Convert {
                self.convert_state.params_dirty_at = Some(std::time::Instant::now());
            }
        }
        if self
            .convert_state
            .render_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() < retry_at)
        {
            return;
        }
        if self
            .convert_state
            .render_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.convert_state.render_retry_at = None;
            self.run_conversion_worker_on(jobs);
        }
        let Some(rx) = self.convert_state.render_rx.take() else {
            return;
        };

        let received = rx.try_recv();
        if !matches!(&received, Err(std::sync::mpsc::TryRecvError::Empty)) {
            self.convert_state.render_decode_control = None;
        }
        match received {
            Ok(Ok(message)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_snapshot = None;
                self.convert_state.render_queue_status = None;
                self.convert_state.render_retry_input_stamp = None;
                if message.generation != self.convert_state.request_generation
                    || message.input_stamp != self.input_stamp()
                {
                    if self.active_mode == ImageToolMode::Convert {
                        self.convert_state.params_dirty_at = Some(std::time::Instant::now());
                    }
                    return;
                }
                match message.result {
                    Ok((bytes, rgba)) => {
                        self.convert_state.encoded_bytes = bytes;
                        self.convert_state.converted_rgba = Some(rgba);
                        self.convert_state.dirty_texture = true;
                        self.convert_state.completed_generation = message.generation;
                        self.convert_state.completed_input_stamp = Some(message.input_stamp);
                        self.convert_state.render_error = None;
                        self.convert_state.render_error_input_stamp = None;
                    }
                    Err(error) => {
                        self.convert_state.render_error = Some(error.clone());
                        self.convert_state.render_error_input_stamp =
                            Some(message.input_stamp.clone());
                        self.show_toast(format!("Conversion failed: {}", error));
                    }
                }
            }
            Ok(Err(JobError::QueueFull)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.convert_state.render_retry_input_stamp = Some(self.input_stamp());
                self.convert_state.render_queue_status =
                    Some("Waiting for background workers; retrying shortly".to_string());
            }
            Ok(Err(JobError::Cancelled)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_snapshot = None;
            }
            Ok(Err(error)) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_cancel = None;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_input_stamp = None;
                self.convert_state.render_snapshot = None;
                self.convert_state.render_error =
                    Some(format!("Conversion worker failed: {error}"));
                self.convert_state.render_error_input_stamp = Some(self.input_stamp());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.convert_state.render_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.convert_state.is_encoding = false;
                self.convert_state.render_job_ticket = None;
                self.convert_state.render_retry_at = None;
                self.convert_state.render_retry_input_stamp = None;
                let was_cancelled = self
                    .convert_state
                    .render_cancel
                    .take()
                    .is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::Acquire));
                self.convert_state.render_snapshot = None;
                if was_cancelled {
                    self.convert_state.render_input_stamp = None;
                } else {
                    self.convert_state.render_error_input_stamp =
                        self.convert_state.render_input_stamp.take();
                    self.convert_state.render_error = Some("Conversion worker stopped".to_string());
                }
            }
        }
    }

    pub(super) fn cancel_conversion_worker(&mut self) {
        if let Some(ticket) = self.convert_state.render_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.convert_state.render_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(control) = self.convert_state.render_decode_control.take() {
            control.cancel();
        }
        self.convert_state.render_rx = None;
        self.convert_state.render_input_stamp = None;
        self.convert_state.render_snapshot = None;
        self.convert_state.is_encoding = false;
        self.convert_state.render_retry_at = None;
        self.convert_state.render_queue_status = None;
        self.convert_state.render_retry_input_stamp = None;
    }

    pub fn apply_converted_to_canvas(&mut self) {
        if !self.convert_state.output_matches(&self.input_stamp()) {
            return;
        }
        if let Some(converted) = self.convert_state.converted_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = Arc::new(converted);
            self.mark_pixels_changed();
            self.redaction_state.boxes.clear();
            self.markup_state.items.clear();
            self.convert_state.completed_input_stamp = None;
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.schedule_conversion();
            self.show_toast("✔ Applied converted image to canvas");
        }
    }
}
