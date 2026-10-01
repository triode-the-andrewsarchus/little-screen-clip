use super::*;

impl ImagePreviewState {
    pub(super) fn creative_fx_render_stamp(&self) -> CreativeFxRenderStamp {
        self.creative_fx_state.render_stamp(self.pixel_revision)
    }

    pub(crate) fn creative_fx_preview_is_current(&self) -> bool {
        self.creative_fx_state.preview_stamp.as_ref() == Some(&self.creative_fx_render_stamp())
    }

    pub fn run_creative_fx_render(&mut self) {
        self.run_creative_fx_render_with_on(
            BackgroundJobs::global(),
            |image, depth, alpha, params, cancel| {
                apply_creative_fx_with_masks_cancellable(image, depth, alpha, params, cancel)
            },
        );
    }

    pub(super) fn run_creative_fx_render_on(&mut self, jobs: &BackgroundJobs) {
        self.run_creative_fx_render_with_on(jobs, |image, depth, alpha, params, cancel| {
            apply_creative_fx_with_masks_cancellable(image, depth, alpha, params, cancel)
        });
    }

    pub(super) fn run_creative_fx_render_with_on<F>(&mut self, jobs: &BackgroundJobs, processor: F)
    where
        F: FnOnce(
                &RgbaImage,
                Option<&RgbaImage>,
                Option<&RgbaImage>,
                &CreativeFxParams,
                &std::sync::atomic::AtomicBool,
            ) -> Option<RgbaImage>
            + Send
            + 'static,
    {
        if self.creative_fx_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }

        self.invalidate_stale_creative_fx_masks();

        let input_rgba = self.current_rgba.clone();
        let stamp = self.creative_fx_render_stamp();
        let params = self.creative_fx_state.current_params();
        let cached_depth = self.creative_fx_state.cached_depth_map.clone();
        let cached_background_removal = self.creative_fx_state.cached_background_removal.clone();

        self.creative_fx_state.render_stamp = Some(stamp.clone());
        self.creative_fx_state.is_rendering = true;
        self.creative_fx_state.render_error = None;
        self.creative_fx_state.render_retry_at = None;
        self.creative_fx_state.params_dirty_at = None;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.creative_fx_state.render_cancel = Some(cancelled.clone());
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                processor(
                    &input_rgba,
                    cached_depth.as_ref(),
                    cached_background_removal.as_ref(),
                    &params,
                    &cancelled,
                )
            }));
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            match process_result {
                Ok(Some(out_rgba)) => {
                    let duration_ms = start.elapsed().as_millis();
                    Some(CreativeFxMsg::Done {
                        stamp,
                        rgba: out_rgba,
                        duration_ms,
                    })
                }
                Ok(None) => None,
                Err(e) => {
                    let err_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Creative FX processing panicked".to_string()
                    };
                    Some(CreativeFxMsg::Error {
                        stamp,
                        message: err_msg,
                    })
                }
            }
        });
        self.creative_fx_state.render_job_ticket = Some(ticket);
        self.creative_fx_state.render_rx = Some(rx);
    }

    /// Poll each Creative FX worker independently so result ownership and cleanup stay local.
    pub fn poll_creative_fx(&mut self) {
        self.poll_creative_fx_on(BackgroundJobs::global());
    }

    pub(super) fn poll_creative_fx_on(&mut self, jobs: &BackgroundJobs) {
        self.invalidate_stale_creative_fx_masks();
        self.invalidate_stale_creative_fx_thumbnails();
        self.poll_creative_fx_render_on(jobs);
        self.poll_creative_fx_depth_mask(jobs);
        self.poll_creative_fx_alpha_mask(jobs);
        self.poll_creative_fx_thumbnail_job();
        self.poll_creative_fx_thumbnails();
        self.retry_creative_fx_thumbnail_batch_if_due(jobs);
    }

    pub(super) fn poll_creative_fx_render_on(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.render_retry_at.is_some()
            && self.active_mode != ImageToolMode::CreativeFx
        {
            self.creative_fx_state.cancel_render();
        }
        if self
            .creative_fx_state
            .render_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &self.creative_fx_render_stamp())
            || (self.creative_fx_state.is_rendering
                && self.active_mode != ImageToolMode::CreativeFx)
        {
            self.creative_fx_state.cancel_render();
        }
        if self.creative_fx_state.render_retry_at.is_some()
            && self.creative_fx_state.render_stamp.as_ref()
                != Some(&self.creative_fx_render_stamp())
        {
            self.creative_fx_state.render_retry_at = None;
            self.creative_fx_state.render_stamp = None;
            self.creative_fx_state.render_error = None;
            if self.active_mode == ImageToolMode::CreativeFx {
                self.creative_fx_state.params_dirty_at = Some(std::time::Instant::now());
            }
        }
        if self
            .creative_fx_state
            .render_retry_at
            .is_some_and(|retry_at| {
                std::time::Instant::now() >= retry_at
                    && self.active_mode == ImageToolMode::CreativeFx
            })
        {
            self.creative_fx_state.render_retry_at = None;
            self.run_creative_fx_render_on(jobs);
        }
        let Some(rx) = self.creative_fx_state.render_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(CreativeFxMsg::Done {
                stamp,
                rgba,
                duration_ms,
            }))) => {
                let is_current = self.creative_fx_state.render_stamp.as_ref() == Some(&stamp)
                    && stamp == self.creative_fx_render_stamp();
                if is_current {
                    self.creative_fx_state.preview_rgba = Some(rgba);
                    self.creative_fx_state.preview_stamp = Some(stamp);
                    self.creative_fx_state.dirty_preview_texture = true;
                    self.creative_fx_state.last_render_duration_ms = duration_ms;
                    self.creative_fx_state.render_error = None;
                } else if self.active_mode == ImageToolMode::CreativeFx
                    && self.creative_fx_state.params_dirty_at.is_none()
                {
                    self.creative_fx_state.mark_dirty();
                }
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_retry_at = None;
            }
            Ok(Ok(Some(CreativeFxMsg::Error { stamp, message }))) => {
                let is_current = self.creative_fx_state.render_stamp.as_ref() == Some(&stamp)
                    && stamp == self.creative_fx_render_stamp();
                if is_current {
                    self.creative_fx_state.render_error = Some(message);
                } else if self.active_mode == ImageToolMode::CreativeFx
                    && self.creative_fx_state.params_dirty_at.is_none()
                {
                    self.creative_fx_state.mark_dirty();
                }
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_retry_at = None;
            }
            Ok(Ok(None)) | Ok(Err(JobError::Cancelled)) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
            }
            Ok(Err(JobError::QueueFull)) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_rx = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.creative_fx_state.render_error =
                    Some("Waiting for background workers; retrying shortly".to_string());
            }
            Ok(Err(error)) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_error =
                    Some(format!("Creative FX worker failed: {error}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.render_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.creative_fx_state.is_rendering = false;
                self.creative_fx_state.render_stamp = None;
                self.creative_fx_state.render_cancel = None;
                self.creative_fx_state.render_job_ticket = None;
                self.creative_fx_state.render_error =
                    Some("Creative FX worker disconnected".into());
            }
        }
    }

    /// Commit creative FX result into current_rgba and push to undo stack.

    pub fn apply_creative_fx(&mut self) {
        self.invalidate_stale_creative_fx_masks();
        if self.creative_fx_state.preview_stamp.as_ref() != Some(&self.creative_fx_render_stamp()) {
            return;
        }
        let preview = match self.creative_fx_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        let fx_name = self.creative_fx_state.selected_fx.name();
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(preview);
        self.mark_pixels_changed();
        self.invalidate_stale_creative_fx_masks();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;

        self.creative_fx_state.preview_rgba = None;
        self.creative_fx_state.preview_stamp = None;
        self.creative_fx_state.preview_texture = None;
        self.creative_fx_state.dirty_preview_texture = false;

        self.creative_fx_state.fx_thumbnails.clear();
        self.creative_fx_state.pending_thumbnails.clear();
        self.creative_fx_state.thumbnail_pixel_revision = None;
        self.run_creative_fx_thumbnail_batch();
        self.show_toast(format!("✨ {} applied", fx_name));
    }
}
