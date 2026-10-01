use super::*;

impl ImagePreviewState {
    /// Spawn low-priority background thread to generate 42x28 thumbnail previews for all film stocks.

    pub fn run_thumbnail_batch(&mut self) {
        self.run_thumbnail_batch_on(little_clip_core::BackgroundJobs::global());
    }

    pub(super) fn run_thumbnail_batch_on(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        if self.current_rgba.is_empty() {
            return;
        }
        self.film_sim_state.cancel_thumbnail_batch();
        self.film_sim_state.thumbnail_epoch = self.film_sim_state.thumbnail_epoch.wrapping_add(1);
        let epoch = self.film_sim_state.thumbnail_epoch;

        // Downscale base image to 42x28 (sub-3ms per stock)
        let thumb_base = image::imageops::resize(
            self.current_rgba.as_ref(),
            42,
            28,
            image::imageops::FilterType::Nearest,
        );
        let rgb_pixels: Vec<u8> = thumb_base
            .pixels()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let thumb_rgb = match image::RgbImage::from_raw(42, 28, rgb_pixels) {
            Some(img) => img,
            None => return,
        };

        let brightness = self.film_sim_state.brightness;
        let warmth = self.film_sim_state.warmth;
        let intensity = self.film_sim_state.intensity;
        let contrast = self.film_sim_state.contrast;
        let tint = self.film_sim_state.tint;
        let wb_mode = self.film_sim_state.wb_mode;
        let wb_strength = self.film_sim_state.wb_strength;
        let halation = self.film_sim_state.halation;
        let vignette = self.film_sim_state.vignette;
        let light_leak = self.film_sim_state.light_leak;
        let style = self.film_sim_state.style;
        let enable_grain = self.film_sim_state.enable_grain;

        let (tx, rx) = std::sync::mpsc::channel::<FilmSimThumbnailMsg>();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut completed: std::collections::HashSet<usize> = self
            .film_sim_state
            .stock_thumbnails
            .keys()
            .copied()
            .collect();
        completed.extend(
            self.film_sim_state
                .pending_thumbnails
                .iter()
                .map(|(idx, _)| *idx),
        );
        let todo: Vec<_> = all_film_stocks()
            .into_iter()
            .enumerate()
            .filter(|(idx, _)| !completed.contains(idx))
            .collect();
        if todo.is_empty() {
            return;
        }
        self.film_sim_state.thumbnail_cancel = Some(cancelled.clone());
        self.film_sim_state.thumbnail_rx = Some(rx);
        self.film_sim_state.is_generating_thumbnails = true;

        let (ticket, job_rx) = jobs.submit(std::time::Duration::from_secs(60), move || {
            super::super::run_thumbnail_items(
                todo,
                || cancelled.load(std::sync::atomic::Ordering::Acquire),
                |(idx, entry)| {
                    let mut stock = (entry.build)().with_style(style);
                    stock.halation_strength = halation;
                    stock.vignette_strength = vignette;

                    let config = SimulationConfig {
                        exposure_time: brightness,
                        warmth,
                        saturation: intensity,
                        auto_levels: false,
                        white_balance_mode: wb_mode,
                        white_balance_strength: wb_strength,
                        output_mode: OutputMode::Positive,
                        enable_grain,
                        light_leak: light_leak.to_config(),
                        ..SimulationConfig::default()
                    };

                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        process_film_image(&thumb_rgb, &stock, &config)
                    }));

                    if let Ok(out_rgb) = res {
                        let out_rgba = apply_film_sim_post_adjustments(&out_rgb, contrast, tint);
                        tx.send(FilmSimThumbnailMsg {
                            epoch,
                            stock_idx: idx,
                            rgba: out_rgba,
                        })
                        .is_ok()
                    } else {
                        true
                    }
                },
            );
        });
        self.film_sim_state.thumbnail_job_ticket = Some(ticket);
        self.film_sim_state.thumbnail_job_rx = Some(job_rx);
    }

    pub(in crate::image_preview) fn retry_thumbnail_batch_if_due(
        &mut self,
        jobs: &little_clip_core::BackgroundJobs,
    ) -> bool {
        if self.film_sim_state.stock_thumbnails.len() + self.film_sim_state.pending_thumbnails.len()
            >= all_film_stocks().len()
            || self.film_sim_state.is_generating_thumbnails
            || self.current_rgba.is_empty()
            || self.film_sim_state.thumbnail_retry_at.is_none()
            || !self
                .film_sim_state
                .thumbnail_retry_ready(std::time::Instant::now())
        {
            return false;
        }
        self.run_thumbnail_batch_on(jobs);
        true
    }
}
