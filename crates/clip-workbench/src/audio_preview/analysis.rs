use super::*;

impl AudioPreviewState {
    pub(super) fn restart_session_analysis(&mut self) {
        self.restart_session_analysis_on(BackgroundJobs::global());
    }

    pub(super) fn cancel_session_analysis(&mut self) {
        if let Some(ticket) = self.analysis_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancelled) = self.analysis_cancel.take() {
            cancelled.store(true, Ordering::Release);
        }
        self.analysis_rx = None;
        self.analysis_job_rx = None;
        self.analysis_progress_disconnected = false;
    }

    pub(super) fn restart_session_analysis_on(&mut self, jobs: &BackgroundJobs) {
        self.cancel_session_analysis();
        self.cancel_edit_analysis();
        self.analysis = None;
        self.completed_source_analysis = None;
        self.edited_waveform = None;
        self.session_transients = None;
        self.spectrogram_texture = None;
        self.note_strip_texture = None;
        self.hf_analysis.viewport_cache.clear();
        self.inspect_viewport_cache.clear();
        self.is_loading_analysis = true;
        self.edit_analysis_rx = None;
        self.edit_analysis_retry_at = None;
        self.edit_analysis_observed = SessionAudioEdits::default();
        self.edit_analysis_inflight = SessionAudioEdits::default();
        self.edit_analysis_displayed = SessionAudioEdits::default();
        if let Ok(mut edits) = self.playback_edits.edits.write() {
            *edits = SessionAudioEdits::default();
            self.playback_edits
                .generation
                .fetch_add(1, Ordering::Release);
        }
        let (cancelled, ticket, progress_rx, job_rx) =
            submit_session_analysis(self.path.clone(), jobs);
        self.analysis_cancel = Some(cancelled);
        self.analysis_ticket = Some(ticket);
        self.analysis_rx = Some(progress_rx);
        self.analysis_job_rx = Some(job_rx);
        self.analysis_retry_at = None;
    }

    pub(super) fn cancel_edit_analysis(&mut self) {
        if let Some(ticket) = self.edit_analysis_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancelled) = self.edit_analysis_cancel.take() {
            cancelled.store(true, Ordering::Release);
        }
        self.edit_analysis_rx = None;
        self.edit_analysis_job_rx = None;
        self.edit_analysis_progress_disconnected = false;
    }

    pub(super) fn poll_audio_analysis(&mut self, ctx: &egui::Context, now: f64) {
        self.poll_audio_analysis_on(ctx, now, BackgroundJobs::global());
    }

    pub(super) fn poll_audio_analysis_on(
        &mut self,
        ctx: &egui::Context,
        now: f64,
        jobs: &BackgroundJobs,
    ) {
        if let Some(retry_at) = self.analysis_retry_at {
            if Instant::now() >= retry_at && self.analysis_rx.is_none() {
                self.start_session_analysis_job(jobs);
            } else {
                ctx.request_repaint_after(retry_at.saturating_duration_since(Instant::now()));
            }
        }
        if let Some(rx) = self.analysis_rx.take() {
            ctx.request_repaint_after(Duration::from_millis(50));
            let mut finished = false;
            loop {
                let progress = match rx.try_recv() {
                    Ok(progress) => progress,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        self.analysis_progress_disconnected = true;
                        break;
                    }
                };
                match progress {
                    AudioAnalysisProgress::WaveformReady(box_analysis) => {
                        let analysis = *box_analysis;
                        self.duration_sec = analysis.duration_ms as f32 / 1000.0;
                        if self.view_duration_sec <= 0.05 {
                            self.view_duration_sec = self.duration_sec;
                        }
                        self.analysis = Some(Arc::new(analysis));
                        self.is_loading_analysis = false;
                    }
                    AudioAnalysisProgress::TransientsReady(transients) => {
                        if let Some(analysis) = self.analysis.as_mut() {
                            let analysis = Arc::make_mut(analysis);
                            analysis.transients = transients.clone();
                            analysis.is_transients_ready = true;
                        }
                        self.session_transients = Some(transients.into());
                    }
                    progress @ AudioAnalysisProgress::SpectrogramAndNotesReady { .. } => {
                        self.install_spectral_analysis(ctx, progress, false);
                    }
                    AudioAnalysisProgress::Complete => {
                        self.completed_source_analysis = self.analysis.clone();
                        finished = true;
                        break;
                    }
                    AudioAnalysisProgress::Failed(error) => {
                        self.status_toast = Some((error, now));
                        self.is_loading_analysis = false;
                        finished = true;
                        break;
                    }
                }
            }
            if !finished {
                self.analysis_rx = Some(rx);
            }
        }
        if let Some(rx) = self.analysis_job_rx.take() {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.analysis_ticket = None;
                    self.analysis_cancel = None;
                    if self.analysis_progress_disconnected {
                        self.is_loading_analysis = false;
                        self.status_toast = Some((
                            "Audio analysis worker stopped before completing".to_owned(),
                            now,
                        ));
                    }
                    self.analysis_progress_disconnected = false;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.analysis_rx = None;
                    self.analysis_ticket = None;
                    self.analysis_cancel = None;
                    self.is_loading_analysis = true;
                    self.analysis_progress_disconnected = false;
                    self.analysis_retry_at = Some(Instant::now() + Duration::from_millis(500));
                    self.status_toast = Some((
                        "Audio analysis is waiting for background workers; retrying shortly"
                            .to_owned(),
                        now,
                    ));
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
                Ok(Err(error)) => {
                    self.analysis_ticket = None;
                    self.analysis_cancel = None;
                    self.analysis_rx = None;
                    self.is_loading_analysis = false;
                    self.analysis_progress_disconnected = false;
                    self.status_toast = Some((format!("Audio analysis failed: {error}"), now));
                }
                Err(TryRecvError::Empty) => self.analysis_job_rx = Some(rx),
                Err(TryRecvError::Disconnected) => {
                    self.analysis_ticket = None;
                    self.analysis_cancel = None;
                    self.analysis_rx = None;
                    self.is_loading_analysis = false;
                    self.analysis_progress_disconnected = false;
                    self.status_toast = Some((
                        "Audio analysis worker disconnected unexpectedly".to_owned(),
                        now,
                    ));
                }
            }
        }
        self.poll_edit_analysis(ctx, now, jobs);
    }

    pub(super) fn start_session_analysis_job(&mut self, jobs: &BackgroundJobs) {
        let (cancelled, ticket, progress_rx, job_rx) =
            submit_session_analysis(self.path.clone(), jobs);
        self.analysis_cancel = Some(cancelled);
        self.analysis_ticket = Some(ticket);
        self.analysis_rx = Some(progress_rx);
        self.analysis_job_rx = Some(job_rx);
        self.analysis_retry_at = None;
        self.analysis_progress_disconnected = false;
    }

    pub(super) fn install_spectral_analysis(
        &mut self,
        ctx: &egui::Context,
        progress: AudioAnalysisProgress,
        edited: bool,
    ) {
        let AudioAnalysisProgress::SpectrogramAndNotesReady {
            spectrogram_width,
            spectrogram_height,
            spectrogram_rgba,
            note_strip_width,
            note_strip_height,
            note_strip_rgba,
            note_strip_info,
            spectral_colors,
            ultrasonic_roi,
            ultrasonic_peak_times,
        } = progress
        else {
            return;
        };
        let spec_img = egui::ColorImage::from_rgba_unmultiplied(
            [spectrogram_width, spectrogram_height],
            &spectrogram_rgba,
        );
        self.spectrogram_texture = Some(ctx.load_texture(
            format!("spectrogram:{}", self.path.display()),
            spec_img,
            egui::TextureOptions::LINEAR,
        ));
        let note_img = egui::ColorImage::from_rgba_unmultiplied(
            [note_strip_width, note_strip_height],
            &note_strip_rgba,
        );
        self.note_strip_texture = Some(ctx.load_texture(
            format!("note_strip:{}", self.path.display()),
            note_img,
            egui::TextureOptions::NEAREST,
        ));
        let target = if edited {
            self.edited_waveform.as_mut()
        } else {
            self.analysis.as_mut()
        };
        if let Some(analysis) = target {
            let analysis = Arc::make_mut(analysis);
            analysis.spectrogram_width = spectrogram_width;
            analysis.spectrogram_height = spectrogram_height;
            analysis.spectrogram_rgba = spectrogram_rgba;
            analysis.note_strip_width = note_strip_width;
            analysis.note_strip_height = note_strip_height;
            analysis.note_strip_rgba = note_strip_rgba;
            analysis.note_strip_info = note_strip_info;
            analysis.spectral_colors = spectral_colors;
            analysis.ultrasonic_roi = ultrasonic_roi;
            analysis.ultrasonic_peak_times = ultrasonic_peak_times;
            analysis.is_spectrogram_ready = true;
        }
        self.inspect_viewport_cache.clear();
        self.hf_analysis.viewport_cache.clear();
    }

    pub(super) fn filter_region_changed(a: &AudioRegionEdit, b: &AudioRegionEdit) -> bool {
        a.low_pass_hz != b.low_pass_hz
            || a.high_pass_hz != b.high_pass_hz
            || ((a.start_sec != b.start_sec
                || a.end_sec != b.end_sec
                || a.crossfade_sec != b.crossfade_sec)
                && (a.low_pass_hz.is_some()
                    || a.high_pass_hz.is_some()
                    || b.low_pass_hz.is_some()
                    || b.high_pass_hz.is_some()))
    }

    pub(super) fn poll_edit_analysis(
        &mut self,
        ctx: &egui::Context,
        now: f64,
        jobs: &BackgroundJobs,
    ) {
        let edits = self.session_audio_edits();
        if edits != self.edit_analysis_observed {
            self.cancel_edit_analysis();
            self.edited_waveform = None;
            let old = &self.edit_analysis_observed;
            let filter_changed = old.regions.len() != edits.regions.len()
                && (old
                    .regions
                    .iter()
                    .any(|r| r.low_pass_hz.is_some() || r.high_pass_hz.is_some())
                    || edits
                        .regions
                        .iter()
                        .any(|r| r.low_pass_hz.is_some() || r.high_pass_hz.is_some()))
                || old
                    .regions
                    .iter()
                    .zip(&edits.regions)
                    .any(|(a, b)| Self::filter_region_changed(a, b));
            self.edit_analysis_debounce =
                Duration::from_millis(if filter_changed { 70 } else { 180 });
            if let Ok(mut playback_edits) = self.playback_edits.edits.write() {
                playback_edits.clone_from(&edits);
                self.playback_edits
                    .generation
                    .fetch_add(1, Ordering::Release);
            }
            self.edit_analysis_observed = edits;
            self.edit_change_at = Instant::now();
            self.edit_analysis_retry_at = None;
        }
        if let Some(rx) = self.edit_analysis_rx.take() {
            let mut finished = false;
            ctx.request_repaint_after(Duration::from_millis(50));
            loop {
                let progress = match rx.try_recv() {
                    Ok(progress) => progress,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        self.edit_analysis_progress_disconnected = true;
                        break;
                    }
                };
                match progress {
                    AudioAnalysisProgress::WaveformReady(analysis) => {
                        if self.edit_analysis_inflight == self.edit_analysis_observed {
                            self.edited_waveform = Some(Arc::new(*analysis));
                        }
                    }
                    progress @ AudioAnalysisProgress::SpectrogramAndNotesReady { .. } => {
                        if self.edit_analysis_inflight == self.edit_analysis_observed {
                            self.install_spectral_analysis(ctx, progress, true);
                            self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        }
                    }
                    AudioAnalysisProgress::Complete => {
                        finished = true;
                        break;
                    }
                    AudioAnalysisProgress::Failed(error) => {
                        self.status_toast = Some((error, now));
                        self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        finished = true;
                        break;
                    }
                    _ => {}
                }
            }
            if !finished {
                self.edit_analysis_rx = Some(rx);
            }
        }
        if let Some(rx) = self.edit_analysis_job_rx.take() {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.edit_analysis_ticket = None;
                    self.edit_analysis_cancel = None;
                    if self.edit_analysis_progress_disconnected
                        && self.edit_analysis_inflight == self.edit_analysis_observed
                    {
                        self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        self.status_toast = Some((
                            "Edited audio analysis worker stopped before completing".to_owned(),
                            now,
                        ));
                    }
                    self.edit_analysis_progress_disconnected = false;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.edit_analysis_rx = None;
                    self.edit_analysis_ticket = None;
                    self.edit_analysis_cancel = None;
                    self.edit_analysis_progress_disconnected = false;
                    self.edit_analysis_retry_at = Some(Instant::now() + Duration::from_millis(500));
                    self.status_toast = Some((
                        "Edited audio analysis is waiting for background workers; retrying shortly"
                            .to_owned(),
                        now,
                    ));
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
                Ok(Err(error)) => {
                    self.edit_analysis_ticket = None;
                    self.edit_analysis_cancel = None;
                    self.edit_analysis_rx = None;
                    self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                    self.status_toast = Some((format!("Edited analysis failed: {error}"), now));
                    self.edit_analysis_progress_disconnected = false;
                }
                Err(TryRecvError::Empty) => self.edit_analysis_job_rx = Some(rx),
                Err(TryRecvError::Disconnected) => {
                    self.edit_analysis_ticket = None;
                    self.edit_analysis_cancel = None;
                    self.edit_analysis_rx = None;
                    if self.edit_analysis_inflight == self.edit_analysis_observed {
                        self.edit_analysis_displayed = self.edit_analysis_inflight.clone();
                        self.status_toast = Some((
                            "Edited audio analysis worker disconnected unexpectedly".to_owned(),
                            now,
                        ));
                    }
                    self.edit_analysis_progress_disconnected = false;
                }
            }
        }
        if self.edit_analysis_rx.is_none()
            && self.edit_analysis_job_rx.is_none()
            && self.analysis_rx.is_none()
            && self.analysis.is_some()
            && self.edit_analysis_observed != self.edit_analysis_displayed
        {
            let debounce = self.edit_analysis_debounce;
            if let Some(retry_at) = self.edit_analysis_retry_at {
                if Instant::now() < retry_at {
                    ctx.request_repaint_after(retry_at.saturating_duration_since(Instant::now()));
                    return;
                }
                self.edit_analysis_retry_at = None;
            }
            if self.edit_change_at.elapsed() < debounce {
                ctx.request_repaint_after(debounce - self.edit_change_at.elapsed());
            } else {
                self.start_edit_analysis_job(jobs);
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    pub(super) fn start_edit_analysis_job(&mut self, jobs: &BackgroundJobs) {
        let path = self.path.clone();
        let edits = self.edit_analysis_observed.clone();
        let job_edits = edits.clone();
        let has_filter = edits
            .regions
            .iter()
            .any(|region| region.low_pass_hz.is_some() || region.high_pass_hz.is_some());
        let cancelled = Arc::new(AtomicBool::new(false));
        let job_cancelled = Arc::clone(&cancelled);
        let (tx, rx) = channel();
        let (ticket, job_rx) = jobs.submit(Duration::MAX, move || {
            let send_progress = |progress| {
                if matches!(
                    progress,
                    AudioAnalysisProgress::WaveformReady(_)
                        | AudioAnalysisProgress::SpectrogramAndNotesReady { .. }
                        | AudioAnalysisProgress::Complete
                        | AudioAnalysisProgress::Failed(_)
                ) {
                    let _ = tx.send(progress);
                }
            };
            if has_filter {
                compute_audio_spectral_preview_with_gain_and_edits_cancellable(
                    &path,
                    1200,
                    job_edits.track_gain_db,
                    &job_edits.regions,
                    &job_cancelled,
                    send_progress,
                );
            } else {
                compute_audio_analysis_progressive_with_gain_and_edits_cancellable(
                    &path,
                    1200,
                    job_edits.track_gain_db,
                    &job_edits.regions,
                    &job_cancelled,
                    send_progress,
                );
            }
        });
        self.edit_analysis_inflight = edits;
        self.edit_analysis_cancel = Some(cancelled);
        self.edit_analysis_ticket = Some(ticket);
        self.edit_analysis_rx = Some(rx);
        self.edit_analysis_job_rx = Some(job_rx);
        self.edit_analysis_retry_at = None;
        self.edit_analysis_progress_disconnected = false;
    }
}
