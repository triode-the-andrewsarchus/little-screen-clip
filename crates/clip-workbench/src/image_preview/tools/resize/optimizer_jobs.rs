use super::*;

impl ImagePreviewState {
    pub(crate) fn cancel_optimizer(&mut self) {
        if let Some(ticket) = self.optimizer_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancelled) = self.optimizer_cancel.take() {
            cancelled.store(true, std::sync::atomic::Ordering::Release);
        }
        self.optimizer_rx = None;
        self.optimizer_job_rx = None;
        self.optimizer_retry_at = None;
        self.optimizer_retry_stamp = None;
        self.optimizer_retry_snapshot = None;
        self.resize_state.optimizer_request_stamp = None;
        self.resize_state.is_optimizing = false;
        self.optimizer_status = None;
    }

    pub(super) fn current_optimizer_request_stamp(&self) -> OptimizerRequestStamp {
        OptimizerRequestStamp {
            image: self.input_stamp(),
            target_bytes: self.resize_state.target_bytes(),
            format: self.resize_state.target_format,
        }
    }

    pub(super) fn optimized_result_is_current(&self) -> bool {
        self.resize_state.last_result_request_stamp.as_ref()
            == Some(&self.current_optimizer_request_stamp())
    }

    /// Runs the target file size optimizer solving the quality vs. resolution trade-off asynchronously.

    pub fn run_target_file_size_optimizer(&mut self) {
        self.run_target_file_size_optimizer_on(little_clip_core::BackgroundJobs::global());
    }

    pub(super) fn run_target_file_size_optimizer_on(
        &mut self,
        jobs: &little_clip_core::BackgroundJobs,
    ) {
        if self.resize_state.is_optimizing {
            return;
        }

        let request_stamp = self.current_optimizer_request_stamp();
        let target_bytes = request_stamp.target_bytes;
        let format = request_stamp.format;
        let snapshot = self
            .optimizer_retry_snapshot
            .take()
            .unwrap_or_else(|| self.display_image_snapshot_request());
        self.optimizer_retry_snapshot = Some(snapshot.clone());
        self.resize_state.optimizer_request_stamp = Some(request_stamp);

        let (progress_tx, rx) = std::sync::mpsc::channel::<OptimizerMsg>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        self.optimizer_cancel = Some(cancelled);
        self.optimizer_rx = Some(rx);
        self.optimizer_retry_at = None;
        self.optimizer_retry_stamp = None;
        self.resize_state.is_optimizing = true;
        self.optimizer_status = Some("Starting optimizer...".to_string());

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let tx_prog = progress_tx.clone();
            let progress_cb = move |msg: String| {
                let _ = tx_prog.send(OptimizerMsg::Progress(msg));
            };

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                optimize_to_target_size_cancellable(
                    &snapshot.compose(),
                    target_bytes,
                    format,
                    Some(&progress_cb),
                    &|| worker_cancelled.load(std::sync::atomic::Ordering::Acquire),
                )
            }));

            if !worker_cancelled.load(std::sync::atomic::Ordering::Acquire) {
                match result {
                    Ok(Some(result)) => {
                        let _ = progress_tx.send(OptimizerMsg::Done(result));
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let _ = progress_tx
                            .send(OptimizerMsg::Failed("Optimizer worker panicked".into()));
                    }
                }
            }
        });
        self.optimizer_ticket = Some(ticket);
        self.optimizer_job_rx = Some(job_rx);
    }

    /// Polls the background file size optimizer channel, updating status messages
    /// and storing the final result when complete.

    pub fn poll_optimizer(&mut self) {
        self.poll_optimizer_on(little_clip_core::BackgroundJobs::global());
    }

    pub(super) fn poll_optimizer_on(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        if self.optimizer_rx.is_some()
            && self.resize_state.optimizer_request_stamp.as_ref()
                != Some(&self.current_optimizer_request_stamp())
        {
            self.cancel_optimizer();
            return;
        }
        if self.optimizer_retry_at.is_some()
            && self.optimizer_retry_stamp.as_ref() != Some(&self.current_optimizer_request_stamp())
        {
            self.optimizer_retry_at = None;
            self.optimizer_retry_stamp = None;
            self.optimizer_retry_snapshot = None;
            self.optimizer_status = None;
        }
        if let Some(retry_at) = self.optimizer_retry_at {
            if std::time::Instant::now() < retry_at {
                return;
            }
            self.optimizer_retry_at = None;
            self.run_target_file_size_optimizer_on(jobs);
            return;
        }
        let mut done_res = None;
        let mut worker_error = None;
        let mut disconnected = false;
        if let Some(rx) = &self.optimizer_rx {
            loop {
                match rx.try_recv() {
                    Ok(OptimizerMsg::Progress(status)) => {
                        self.optimizer_status = Some(status);
                    }
                    Ok(OptimizerMsg::Done(res)) => {
                        done_res = Some(res);
                        break;
                    }
                    Ok(OptimizerMsg::Failed(error)) => {
                        worker_error = Some(error);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let mut queue_full = false;
        let mut job_failed = None;
        if let Some(job_rx) = self.optimizer_job_rx.take() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
                Ok(Err(little_clip_core::JobError::QueueFull)) => {
                    queue_full = true;
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
                Ok(Err(little_clip_core::JobError::Cancelled)) => {
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                    self.resize_state.is_optimizing = false;
                    self.resize_state.optimizer_request_stamp = None;
                    self.optimizer_rx = None;
                    self.optimizer_status = None;
                    self.optimizer_retry_snapshot = None;
                    return;
                }
                Ok(Err(error)) => {
                    job_failed = Some(error.to_string());
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.optimizer_job_rx = Some(job_rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    self.optimizer_ticket = None;
                    self.optimizer_cancel = None;
                    self.optimizer_job_rx = None;
                }
            }
        }
        if queue_full {
            self.resize_state.is_optimizing = false;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_rx = None;
            self.optimizer_status = Some("Background workers are busy; retrying shortly".into());
            self.optimizer_retry_stamp = Some(self.current_optimizer_request_stamp());
            self.optimizer_retry_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
            return;
        }
        if let Some(error) = job_failed {
            self.resize_state.is_optimizing = false;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_rx = None;
            self.optimizer_status = Some(format!("Optimizer worker failed: {error}"));
            self.optimizer_retry_snapshot = None;
            return;
        }
        if let Some(error) = worker_error {
            self.resize_state.is_optimizing = false;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_rx = None;
            self.optimizer_cancel = None;
            self.optimizer_ticket = None;
            self.optimizer_job_rx = None;
            self.optimizer_status = Some(error);
            self.optimizer_retry_snapshot = None;
            return;
        }
        if let Some(res) = done_res {
            self.resize_state.last_result = Some(res);
            self.resize_state.last_result_request_stamp =
                self.resize_state.optimizer_request_stamp.take();
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
            self.optimizer_cancel = None;
            self.optimizer_ticket = None;
            self.optimizer_job_rx = None;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_retry_snapshot = None;
        } else if disconnected {
            self.resize_state.is_optimizing = false;
            self.optimizer_status = None;
            self.optimizer_rx = None;
            self.optimizer_cancel = None;
            self.optimizer_ticket = None;
            self.optimizer_job_rx = None;
            self.resize_state.optimizer_request_stamp = None;
            self.optimizer_retry_snapshot = None;
        }
    }

    /// Applies the optimized image result directly to current_rgba.

    pub fn apply_optimized_result(&mut self) {
        if !self.optimized_result_is_current() {
            return;
        }
        let res = match self.resize_state.last_result.clone() {
            Some(r) => r,
            None => return,
        };

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        self.current_rgba = Arc::new(res.result_rgba);
        self.mark_pixels_changed();
        self.redaction_state.boxes.clear();
        self.markup_state.items.clear();
        self.resize_state.last_result_request_stamp = None;
        self.file_size_bytes = res.achieved_bytes;
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.show_toast(format!(
            "✔ Applied optimized {}×{} image ({})",
            res.width,
            res.height,
            format_bytes_size(res.achieved_bytes)
        ));
    }

    /// Copies the optimized result directly to the Left Hand clipboard slot via daemon IPC
    /// and synchronizes with OS clipboard.

    pub fn copy_optimized_to_slot(&mut self) {
        self.copy_optimized_to_slot_on(BackgroundJobs::global());
    }

    pub(super) fn copy_optimized_to_slot_on(&mut self, jobs: &BackgroundJobs) {
        if !self.optimized_result_is_current() {
            return;
        }
        let (bytes, mime, achieved_bytes, format_label) = match &self.resize_state.last_result {
            Some(r) => (
                Arc::clone(&r.encoded_data),
                Self::format_label_or_mime(r.format),
                r.achieved_bytes,
                r.format.label(),
            ),
            None => return,
        };
        self.start_optimized_copy_to_slot(bytes, mime, achieved_bytes, format_label, jobs);
    }
}
