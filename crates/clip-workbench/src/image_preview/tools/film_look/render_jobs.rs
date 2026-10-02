use super::*;

impl ImagePreviewState {
    pub(super) fn current_film_sim_render_stamp(&self) -> FilmSimRenderStamp {
        FilmSimRenderStamp {
            image: self.input_stamp(),
            parameter_revision: self.film_sim_state.parameter_revision,
        }
    }

    pub(crate) fn film_sim_preview_is_current(&self) -> bool {
        self.film_sim_state.preview_rgba.is_some()
            && self.film_sim_state.preview_stamp.as_ref()
                == Some(&self.current_film_sim_render_stamp())
    }

    pub fn run_film_sim_render(&mut self) {
        self.run_film_sim_render_on(little_clip_core::BackgroundJobs::global());
    }

    pub(super) fn run_film_sim_render_on(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        if self.film_sim_state.is_rendering || self.current_rgba.is_empty() {
            return;
        }
        let stocks = all_film_stocks();
        let idx = self
            .film_sim_state
            .selected_stock_idx
            .min(stocks.len().saturating_sub(1));
        let entry = &stocks[idx];
        let mut stock = (entry.build)().with_style(self.film_sim_state.style);
        stock.halation_strength = self.film_sim_state.halation;
        stock.vignette_strength = self.film_sim_state.vignette;

        let config = SimulationConfig {
            use_gpu: true,
            exposure_time: self.film_sim_state.brightness,
            warmth: self.film_sim_state.warmth,
            saturation: self.film_sim_state.intensity,
            auto_levels: false,
            white_balance_mode: self.film_sim_state.wb_mode,
            white_balance_strength: self.film_sim_state.wb_strength,
            output_mode: OutputMode::Positive,
            enable_grain: self.film_sim_state.enable_grain,
            light_leak: self.film_sim_state.light_leak.to_config(),
            ..SimulationConfig::default()
        };

        // Queued work must not keep an obsolete full-size source alive.
        let source = Arc::downgrade(&self.current_rgba);

        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;

        let stamp = self.current_film_sim_render_stamp();
        self.film_sim_state.render_stamp = Some(stamp.clone());
        self.film_sim_state.is_rendering = true;
        self.film_sim_state.render_error = None;
        self.film_sim_state.render_retry_at = None;
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.film_sim_state.render_cancel = Some(cancelled.clone());

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            let source = source.upgrade()?;
            let start = std::time::Instant::now();
            let process_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rgb_img = film_rgb_from_rgba(&source, || {
                    cancelled.load(std::sync::atomic::Ordering::Acquire)
                })?;
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return None;
                }
                process_film_image_inner(&rgb_img, &stock, &config, Some(&cancelled))
            }));
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            match process_result {
                Ok(Some(out_rgb)) => {
                    let Some(out_rgba) = apply_film_sim_post_adjustments_inner(
                        &out_rgb,
                        contrast,
                        tint,
                        Some(&cancelled),
                    ) else {
                        return None;
                    };
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        return None;
                    }
                    let duration_ms = start.elapsed().as_millis();
                    Some(FilmSimMsg::Done {
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
                        "Film simulation panicked".to_string()
                    };
                    Some(FilmSimMsg::Error {
                        stamp,
                        message: err_msg,
                    })
                }
            }
        });
        self.film_sim_state.render_job_ticket = Some(ticket);
        self.film_sim_state.render_rx = Some(job_rx);
    }

    /// Drain any messages from the film sim background thread and thumbnail worker.

    pub fn poll_film_sim(&mut self) {
        let current_stamp = self.current_film_sim_render_stamp();
        if self
            .film_sim_state
            .render_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &current_stamp)
            || (self.film_sim_state.is_rendering && self.active_mode != ImageToolMode::FilmSim)
        {
            self.film_sim_state.cancel_render();
        }
        if let Some(rx) = self.film_sim_state.render_rx.take() {
            match rx.try_recv() {
                Ok(Ok(Some(FilmSimMsg::Done {
                    stamp,
                    rgba,
                    duration_ms,
                }))) => {
                    if stamp == current_stamp
                        && self.film_sim_state.render_stamp.as_ref() == Some(&stamp)
                    {
                        self.film_sim_state.preview_rgba = Some(rgba);
                        self.film_sim_state.preview_stamp = Some(stamp);
                        self.film_sim_state.dirty_preview_texture = true;
                        self.film_sim_state.last_render_duration_ms = duration_ms;
                        self.film_sim_state.render_error = None;
                    }
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                    self.film_sim_state.render_job_ticket = None;
                }
                Ok(Ok(Some(FilmSimMsg::Error { stamp, message }))) => {
                    if stamp == current_stamp
                        && self.film_sim_state.render_stamp.as_ref() == Some(&stamp)
                    {
                        self.film_sim_state.render_error = Some(message);
                    }
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                    self.film_sim_state.render_job_ticket = None;
                }
                Ok(Ok(None)) | Ok(Err(little_clip_core::JobError::Cancelled)) => {
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                    self.film_sim_state.render_job_ticket = None;
                }
                Ok(Err(little_clip_core::JobError::QueueFull)) => {
                    self.film_sim_state.render_error = Some(
                        "Film Look is waiting for background workers; retrying shortly".to_owned(),
                    );
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                    self.film_sim_state.render_job_ticket = None;
                    self.film_sim_state.render_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                }
                Ok(Err(little_clip_core::JobError::TimedOut)) => {
                    self.film_sim_state.render_error = Some("Film simulation timed out".to_owned());
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                    self.film_sim_state.render_job_ticket = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.film_sim_state.render_rx = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.film_sim_state.is_rendering = false;
                    self.film_sim_state.render_stamp = None;
                    self.film_sim_state.render_cancel = None;
                    self.film_sim_state.render_job_ticket = None;
                }
            }
        }

        // A full shared queue rejects submission immediately. Clear that run so the
        // normal incomplete-thumbnail check can retry it on the next UI frame.
        self.film_sim_state.poll_thumbnail_job_queue();

        // Drain pending thumbnail messages
        let mut thumbs_done = false;
        if let Some(rx) = self.film_sim_state.thumbnail_rx.take() {
            let mut keep = true;
            loop {
                match rx.try_recv() {
                    Ok(msg) => {
                        if msg.epoch == self.film_sim_state.thumbnail_epoch {
                            self.film_sim_state
                                .pending_thumbnails
                                .push((msg.stock_idx, msg.rgba));
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        keep = false;
                        thumbs_done = true;
                        break;
                    }
                }
            }
            if keep {
                self.film_sim_state.thumbnail_rx = Some(rx);
            }
        }
        if thumbs_done {
            self.film_sim_state.is_generating_thumbnails = false;
            self.film_sim_state.thumbnail_cancel = None;
        }
    }

    /// Commit the film sim preview into current_rgba and push to undo stack.

    pub fn apply_film_sim(&mut self) {
        if !self.film_sim_preview_is_current() {
            return;
        }
        let preview = match self.film_sim_state.preview_rgba.take() {
            Some(p) => p,
            None => return,
        };
        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();
        self.current_rgba = Arc::new(preview);
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.resize_state.sync_from_image(&self.current_rgba);
        self.palette_state.is_extracted = false;
        // Reset sim state so re-entering the tab renders fresh.
        self.film_sim_state.preview_rgba = None;
        self.film_sim_state.preview_stamp = None;
        self.film_sim_state.preview_texture = None;
        self.film_sim_state.dirty_preview_texture = false;
        // Invalidate and regenerate thumbnails for updated image
        self.film_sim_state.stock_thumbnails.clear();
        self.film_sim_state.pending_thumbnails.clear();
        self.run_thumbnail_batch();
        self.show_toast(crate::ui_ext::StatusMessage::new(
            crate::icons::Icon::Film,
            "Film simulation applied",
        ));
    }
}
