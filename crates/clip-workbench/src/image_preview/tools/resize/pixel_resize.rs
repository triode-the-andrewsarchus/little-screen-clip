use super::*;

impl ImagePreviewState {
    pub fn apply_pixel_resize(&mut self, target_w: u32, target_h: u32, filter: ResizeFilter) {
        self.apply_pixel_resize_on(target_w, target_h, filter, BackgroundJobs::global());
    }

    pub(super) fn apply_pixel_resize_on(
        &mut self,
        target_w: u32,
        target_h: u32,
        filter: ResizeFilter,
        jobs: &BackgroundJobs,
    ) {
        let (w, h) = (target_w.clamp(1, 16384), target_h.clamp(1, 16384));
        if (w == self.current_rgba.width() && h == self.current_rgba.height())
            || self.pixel_resize_job.pending
        {
            return;
        }

        self.cancel_pixel_resize();
        self.resize_state.target_width = w;
        self.resize_state.target_height = h;
        self.resize_state.width_input = w.to_string();
        self.resize_state.height_input = h.to_string();
        self.resize_state.filter = filter;
        let snapshot = self.display_image_snapshot_request();
        let request = PixelResizeRequest {
            source: snapshot.pixels,
            redactions: self.redaction_state.boxes.clone(),
            stamp: PixelResizeStamp {
                image: snapshot.input_stamp,
                width: w,
                height: h,
                filter,
            },
        };
        self.submit_pixel_resize(request, jobs);
    }

    pub(super) fn current_pixel_resize_stamp(&self) -> PixelResizeStamp {
        PixelResizeStamp {
            image: self.input_stamp(),
            width: self.resize_state.target_width.clamp(1, 16384),
            height: self.resize_state.target_height.clamp(1, 16384),
            filter: self.resize_state.filter,
        }
    }

    pub(super) fn submit_pixel_resize(
        &mut self,
        request: PixelResizeRequest,
        jobs: &BackgroundJobs,
    ) {
        let worker_request = request.clone();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let prepared = super::super::redact::compose_redactions_only(
                    &worker_request.source,
                    &worker_request.redactions,
                );
                image::imageops::resize(
                    &prepared,
                    worker_request.stamp.width,
                    worker_request.stamp.height,
                    worker_request.stamp.filter.to_image_filter(),
                )
            }))
            .map_err(|_| "Resize worker panicked".to_string());
            if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            Some(PixelResizeMessage {
                stamp: worker_request.stamp,
                result,
            })
        });
        self.pixel_resize_job.ticket = Some(ticket);
        self.pixel_resize_job.rx = Some(rx);
        self.pixel_resize_job.cancel = Some(cancel);
        self.pixel_resize_job.stamp = Some(request.stamp.clone());
        self.pixel_resize_job.retry_request = Some(request);
        self.pixel_resize_job.retry_at = None;
        self.pixel_resize_job.pending = true;
        self.pixel_resize_job.status = Some("Resizing image…".into());
        self.pixel_resize_job.error = None;
    }

    pub(crate) fn poll_pixel_resize(&mut self, ctx: &egui::Context, jobs: &BackgroundJobs) {
        if self.pixel_resize_job.pending
            && self.pixel_resize_job.stamp.as_ref() != Some(&self.current_pixel_resize_stamp())
        {
            self.cancel_pixel_resize();
            return;
        }
        if self.pixel_resize_job.retry_at.is_some()
            && self
                .pixel_resize_job
                .retry_request
                .as_ref()
                .map(|r| &r.stamp)
                != Some(&self.current_pixel_resize_stamp())
        {
            self.cancel_pixel_resize();
            return;
        }
        if let Some(retry_at) = self.pixel_resize_job.retry_at {
            if std::time::Instant::now() < retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
                return;
            }
            if let Some(request) = self.pixel_resize_job.retry_request.take() {
                self.submit_pixel_resize(request, jobs);
            }
            return;
        }
        let Some(rx) = self.pixel_resize_job.rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(message))) => {
                if message.stamp != self.current_pixel_resize_stamp() {
                    self.clear_pixel_resize_job();
                    return;
                }
                match message.result {
                    Ok(resized) => {
                        let redactions = self
                            .pixel_resize_job
                            .retry_request
                            .as_ref()
                            .map(|request| !request.redactions.is_empty())
                            .unwrap_or(false);
                        self.clear_pixel_resize_job();
                        if redactions {
                            self.apply_redactions();
                        }
                        self.undo_stack.push(self.current_rgba.clone());
                        self.redo_stack.clear();
                        self.current_rgba = Arc::new(resized);
                        self.mark_pixels_changed();
                        self.dirty_texture = true;
                        self.resize_state.sync_from_image(&self.current_rgba);
                        self.palette_state.is_extracted = false;
                        self.show_toast(format!(
                            "↔ Resized to {}×{} px ({})",
                            message.stamp.width,
                            message.stamp.height,
                            message.stamp.filter.label()
                        ));
                    }
                    Err(error) => {
                        self.clear_pixel_resize_job();
                        self.pixel_resize_job.error = Some(error);
                        self.pixel_resize_job.status = None;
                    }
                }
            }
            Ok(Err(little_clip_core::JobError::QueueFull)) => {
                self.pixel_resize_job.ticket = None;
                self.pixel_resize_job.rx = None;
                self.pixel_resize_job.cancel = None;
                self.pixel_resize_job.stamp = None;
                self.pixel_resize_job.pending = false;
                self.pixel_resize_job.status =
                    Some("Background workers busy; retrying resize…".into());
                self.pixel_resize_job.retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            Ok(Err(little_clip_core::JobError::Cancelled)) => self.clear_pixel_resize_job(),
            Ok(Err(error)) => {
                self.clear_pixel_resize_job();
                self.pixel_resize_job.error = Some(format!("Resize worker failed: {error}"));
            }
            Ok(Ok(None)) => self.clear_pixel_resize_job(),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.pixel_resize_job.rx = Some(rx);
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.clear_pixel_resize_job();
                self.pixel_resize_job.error = Some("Resize worker stopped unexpectedly".into());
            }
        }
    }

    pub(super) fn clear_pixel_resize_job(&mut self) {
        self.pixel_resize_job.ticket = None;
        self.pixel_resize_job.rx = None;
        self.pixel_resize_job.cancel = None;
        self.pixel_resize_job.stamp = None;
        self.pixel_resize_job.retry_at = None;
        self.pixel_resize_job.retry_request = None;
        self.pixel_resize_job.pending = false;
        self.pixel_resize_job.status = None;
    }

    pub(crate) fn cancel_pixel_resize(&mut self) {
        if let Some(ticket) = self.pixel_resize_job.ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.pixel_resize_job.cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.pixel_resize_job.rx = None;
        self.pixel_resize_job.stamp = None;
        self.pixel_resize_job.retry_at = None;
        self.pixel_resize_job.retry_request = None;
        self.pixel_resize_job.pending = false;
        self.pixel_resize_job.status = None;
    }
}
