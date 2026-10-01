use super::*;

impl ImagePreviewState {
    pub(super) fn invalidate_stale_creative_fx_thumbnails(&mut self) {
        if self
            .creative_fx_state
            .thumbnail_pixel_revision
            .is_some_and(|revision| revision != self.pixel_revision)
        {
            self.cancel_creative_fx_thumbnail_batch();
            self.creative_fx_state.thumbnail_epoch =
                self.creative_fx_state.thumbnail_epoch.wrapping_add(1);
            self.creative_fx_state.thumbnail_pixel_revision = None;
            self.creative_fx_state.thumbnail_rx = None;
            self.creative_fx_state.pending_thumbnails.clear();
            self.creative_fx_state.fx_thumbnails.clear();
            self.creative_fx_state.is_generating_thumbnails = false;
        }
    }

    pub(crate) fn cancel_creative_fx_thumbnail_batch(&mut self) {
        if let Some(ticket) = self.creative_fx_state.thumbnail_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.creative_fx_state.thumbnail_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.creative_fx_state.thumbnail_epoch =
            self.creative_fx_state.thumbnail_epoch.wrapping_add(1);
        self.creative_fx_state.thumbnail_rx = None;
        self.creative_fx_state.thumbnail_job_rx = None;
        self.creative_fx_state.is_generating_thumbnails = false;
        self.creative_fx_state.thumbnail_retry_at = None;
        self.creative_fx_state.thumbnail_queue_status = None;
    }

    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all FX.
    pub fn run_creative_fx_thumbnail_batch(&mut self) {
        self.run_creative_fx_thumbnail_batch_on(BackgroundJobs::global());
    }

    pub(super) fn run_creative_fx_thumbnail_batch_on(&mut self, jobs: &BackgroundJobs) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.cancel_creative_fx_thumbnail_batch();
        self.creative_fx_state.thumbnail_epoch =
            self.creative_fx_state.thumbnail_epoch.wrapping_add(1);
        let epoch = self.creative_fx_state.thumbnail_epoch;
        let pixel_revision = self.pixel_revision;
        self.creative_fx_state.thumbnail_pixel_revision = Some(pixel_revision);

        // Downscale base image to 42x28 (sub-millisecond)
        let thumb_base = image::imageops::resize(
            self.current_rgba.as_ref(),
            42,
            28,
            image::imageops::FilterType::Nearest,
        );

        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxThumbnailMsg>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed: std::collections::HashSet<_> = self
            .creative_fx_state
            .fx_thumbnails
            .keys()
            .copied()
            .chain(
                self.creative_fx_state
                    .pending_thumbnails
                    .iter()
                    .map(|(fx, _)| *fx),
            )
            .collect();
        let todo: Vec<_> = CreativeFxType::all()
            .into_iter()
            .copied()
            .filter(|fx| !completed.contains(fx))
            .collect();
        if todo.is_empty() {
            return;
        }
        self.creative_fx_state.thumbnail_cancel = Some(cancelled.clone());
        self.creative_fx_state.thumbnail_rx = Some(rx);
        self.creative_fx_state.is_generating_thumbnails = true;
        self.creative_fx_state.thumbnail_queue_status = None;
        self.creative_fx_state.thumbnail_retry_at = None;

        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            super::run_thumbnail_items(
                todo,
                || cancelled.load(std::sync::atomic::Ordering::Acquire),
                |fx| {
                    let preset = default_preset_for_fx(fx);
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        apply_creative_fx(&thumb_base, &preset.params)
                    }));
                    if let Ok(rgba) = res {
                        tx.send(CreativeFxThumbnailMsg {
                            epoch,
                            pixel_revision,
                            fx_type: fx,
                            rgba,
                        })
                        .is_ok()
                    } else {
                        true
                    }
                },
            );
        });
        self.creative_fx_state.thumbnail_job_ticket = Some(ticket);
        self.creative_fx_state.thumbnail_job_rx = Some(job_rx);
    }

    pub(super) fn poll_creative_fx_thumbnail_job(&mut self) {
        let Some(rx) = self.creative_fx_state.thumbnail_job_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Err(JobError::QueueFull)) => {
                self.cancel_creative_fx_thumbnail_batch();
                self.creative_fx_state.thumbnail_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.creative_fx_state.thumbnail_queue_status =
                    Some("Waiting for background workers; retrying thumbnail batch shortly".into());
            }
            Ok(Err(error)) => {
                self.creative_fx_state.thumbnail_job_ticket = None;
                self.creative_fx_state.is_generating_thumbnails = false;
                self.creative_fx_state.thumbnail_queue_status =
                    Some(format!("Thumbnail worker failed: {error}"));
            }
            Ok(Ok(())) => self.creative_fx_state.thumbnail_job_ticket = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.thumbnail_job_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.creative_fx_state.thumbnail_job_ticket = None;
                self.creative_fx_state.is_generating_thumbnails = false;
                self.creative_fx_state.thumbnail_queue_status =
                    Some("Thumbnail worker disconnected".into());
            }
        }
    }

    pub(super) fn retry_creative_fx_thumbnail_batch_if_due(
        &mut self,
        jobs: &BackgroundJobs,
    ) -> bool {
        if self.current_rgba.is_empty()
            || self.creative_fx_state.is_generating_thumbnails
            || self
                .creative_fx_state
                .thumbnail_retry_at
                .is_none_or(|retry_at| std::time::Instant::now() < retry_at)
            || self.creative_fx_state.fx_thumbnails.len()
                + self.creative_fx_state.pending_thumbnails.len()
                >= CreativeFxType::all().len()
            || self.creative_fx_state.thumbnail_pixel_revision != Some(self.pixel_revision)
        {
            return false;
        }
        self.creative_fx_state.thumbnail_retry_at = None;
        self.run_creative_fx_thumbnail_batch_on(jobs);
        true
    }

    pub(super) fn poll_creative_fx_thumbnails(&mut self) {
        let Some(rx) = self.creative_fx_state.thumbnail_rx.take() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(msg)
                    if msg.epoch == self.creative_fx_state.thumbnail_epoch
                        && Some(msg.pixel_revision)
                            == self.creative_fx_state.thumbnail_pixel_revision
                        && msg.pixel_revision == self.pixel_revision =>
                {
                    self.creative_fx_state
                        .pending_thumbnails
                        .push((msg.fx_type, msg.rgba));
                }
                Ok(_) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.creative_fx_state.thumbnail_rx = Some(rx);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.creative_fx_state.is_generating_thumbnails = false;
                    self.creative_fx_state.thumbnail_cancel = None;
                    break;
                }
            }
        }
    }
}
