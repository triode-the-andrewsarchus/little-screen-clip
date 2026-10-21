use super::*;

impl ImagePreviewState {
    pub(super) fn invalidate_stale_creative_fx_masks(&mut self) {
        let current = self.input_stamp();
        let stale_depth = self.creative_fx_state.cached_depth_map.is_some()
            && self.creative_fx_state.depth_map_input_stamp.as_ref() != Some(&current);
        let stale_alpha = self.creative_fx_state.cached_background_removal.is_some()
            && self
                .creative_fx_state
                .background_removal_input_stamp
                .as_ref()
                != Some(&current);
        if stale_depth {
            self.clear_creative_fx_mask_cache(CreativeFxMaskSource::DepthMap);
        }
        if stale_alpha {
            self.clear_creative_fx_mask_cache(CreativeFxMaskSource::BackgroundRemovalAlpha);
        }
        self.finish_creative_fx_mask_invalidation(stale_depth, stale_alpha);
    }

    pub(super) fn clear_creative_fx_mask_cache(&mut self, source: CreativeFxMaskSource) {
        match source {
            CreativeFxMaskSource::DepthMap => {
                self.creative_fx_state.cached_depth_map = None;
                self.creative_fx_state.depth_map_input_stamp = None;
            }
            CreativeFxMaskSource::BackgroundRemovalAlpha => {
                self.creative_fx_state.cached_background_removal = None;
                self.creative_fx_state.background_removal_input_stamp = None;
            }
        }
    }

    pub(super) fn finish_creative_fx_mask_invalidation(
        &mut self,
        stale_depth: bool,
        stale_alpha: bool,
    ) {
        if !stale_depth && !stale_alpha {
            return;
        }
        let active_config = *self.creative_fx_state.depth_mask_for_active_fx();
        let active_source_invalidated = (stale_depth
            && active_config.enabled
            && active_config.source == CreativeFxMaskSource::DepthMap)
            || (stale_alpha
                && active_config.enabled
                && active_config.source == CreativeFxMaskSource::BackgroundRemovalAlpha);
        let disabled_depth_configs = stale_depth
            && self
                .creative_fx_state
                .disable_mask_source(CreativeFxMaskSource::DepthMap);
        let disabled_alpha_configs = stale_alpha
            && self
                .creative_fx_state
                .disable_mask_source(CreativeFxMaskSource::BackgroundRemovalAlpha);
        self.schedule_mask_preview_after_invalidation(
            active_source_invalidated,
            disabled_depth_configs || disabled_alpha_configs,
        );
    }

    pub(super) fn schedule_mask_preview_after_invalidation(
        &mut self,
        active_source_invalidated: bool,
        any_config_disabled: bool,
    ) {
        if !active_source_invalidated || !any_config_disabled {
            return;
        }
        self.creative_fx_state.mask_cache_revision =
            self.creative_fx_state.mask_cache_revision.wrapping_add(1);
        self.creative_fx_state.mark_dirty();
        let preview_needs_refresh = self.active_mode == ImageToolMode::CreativeFx
            && (self.creative_fx_state.preview_rgba.is_some()
                || self.creative_fx_state.is_rendering);
        if !preview_needs_refresh {
            self.creative_fx_state.params_dirty_at = None;
        }
    }

    #[cfg(test)]
    pub(super) fn current_creative_fx_mask_stamp(&self) -> CreativeFxMaskStamp {
        self.creative_fx_state.mask_stamp(self.input_stamp())
    }

    pub(super) fn creative_fx_mask_source_matches(
        &self,
        expected: Option<&CreativeFxMaskStamp>,
        actual: &CreativeFxMaskStamp,
    ) -> bool {
        expected == Some(actual) && actual.input == self.input_stamp()
    }

    pub(super) fn creative_fx_mask_intent_matches(&self, stamp: &CreativeFxMaskStamp) -> bool {
        stamp.params_revision == self.creative_fx_state.params_revision
            && stamp.fx_type == self.creative_fx_state.selected_fx
            && stamp.selection_revision == self.creative_fx_state.selection_revision
    }

    pub(super) fn depth_mask_cache_is_current(&self) -> bool {
        self.creative_fx_state.cached_depth_map.is_some()
            && self.creative_fx_state.depth_map_input_stamp.as_ref() == Some(&self.input_stamp())
    }

    pub(super) fn alpha_mask_cache_is_current(&self) -> bool {
        self.creative_fx_state.cached_background_removal.is_some()
            && self
                .creative_fx_state
                .background_removal_input_stamp
                .as_ref()
                == Some(&self.input_stamp())
    }

    /// Spawn background thread to estimate AI depth map using Depth-Anything V2.
    pub fn run_depth_estimation_for_fx(&mut self) {
        let depth_script = "scripts/depth_estimation.py";
        let (cmd_bin, cmd_args) = if std::path::Path::new(depth_script).exists() {
            (depth_script.to_string(), Vec::new())
        } else {
            ("python3".to_string(), vec![depth_script.to_string()])
        };
        self.run_depth_estimation_for_fx_with_command(cmd_bin, cmd_args);
    }

    pub(super) fn run_depth_estimation_for_fx_with_command(
        &mut self,
        cmd_bin: String,
        cmd_args: Vec<String>,
    ) {
        self.run_depth_estimation_for_fx_with_command_on(
            cmd_bin,
            cmd_args,
            BackgroundJobs::global(),
        );
    }

    pub(super) fn run_depth_estimation_for_fx_with_command_on(
        &mut self,
        cmd_bin: String,
        cmd_args: Vec<String>,
        jobs: &BackgroundJobs,
    ) {
        if self.creative_fx_state.depth_estimation_in_progress || self.current_rgba.is_empty() {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let snapshot = self.display_image_snapshot_request();
        let stamp = self
            .creative_fx_state
            .mask_stamp(snapshot.input_stamp.clone());
        self.creative_fx_state.depth_estimation_stamp = Some(stamp.clone());
        self.creative_fx_state.depth_estimation_in_progress = true;
        self.creative_fx_state.depth_estimation_error = None;
        self.creative_fx_state.depth_queue_status = None;
        self.creative_fx_state.depth_retry_at = None;
        self.creative_fx_state.depth_request = Some(CreativeFxMaskRequest {
            snapshot,
            cmd_bin,
            cmd_args,
            stamp,
            model_url: self.depth_model_url.clone(),
        });
        self.submit_creative_fx_depth_attempt(jobs);
    }

    pub(super) fn submit_creative_fx_depth_attempt(&mut self, jobs: &BackgroundJobs) {
        let Some(request) = self.creative_fx_state.depth_request.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxMaskMsg>();
        self.creative_fx_state.depth_estimation_rx = Some(rx);
        self.creative_fx_state.depth_retry_at = None;
        self.creative_fx_state.depth_queue_status = None;
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        self.creative_fx_state.depth_process_control = Some(control.clone());
        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let is_default_depth_cmd = request.cmd_bin.contains("depth_estimation.py")
                || request
                    .cmd_args
                    .iter()
                    .any(|a| a.contains("depth_estimation.py"));
            if is_default_depth_cmd {
                if let Ok(model_path) = crate::onnx_runtime::ensure_model(
                    crate::onnx_runtime::DEPTH_ANYTHING_V2_MODEL_NAME,
                    request.model_url.as_deref(),
                ) {
                    let source_rgba = request.snapshot.compose();
                    match super::super::background_removal::run_native_depth_map(
                        &source_rgba,
                        &model_path,
                    ) {
                        Ok(depth_rgba) => {
                            if control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                                return;
                            }
                            send_creative_fx_mask_result(&tx, &request.stamp, Ok(depth_rgba));
                            return;
                        }
                        Err(_) => {}
                    }
                }
            }

            let mut cmd = Command::new(&request.cmd_bin);
            cmd.args(&request.cmd_args);
            let result = request
                .snapshot
                .compose_png()
                .map_err(|error| format!("Failed to encode image to PNG: {error}"))
                .and_then(|png_bytes| {
                    super::super::background_removal::run_image_tool_subprocess(
                        cmd,
                        &png_bytes,
                        None,
                        Some(&control.cancel),
                        Some(&control.child_pid),
                        BgRemovalOperation::DepthMap,
                        &request.cmd_bin,
                    )
                    .and_then(super::super::background_removal::decode_image_tool_output)
                });
            if result.is_err() && control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            send_creative_fx_mask_result(&tx, &request.stamp, result);
        });
        self.creative_fx_state.depth_job_ticket = Some(ticket);
        self.creative_fx_state.depth_job_rx = Some(job_rx);
    }

    /// Spawn automatic FeyNoBg removal to provide an alpha mask for Creative FX.
    pub fn run_alpha_mask_for_fx(&mut self) {
        self.run_alpha_mask_for_fx_on(BackgroundJobs::global());
    }

    pub(super) fn run_alpha_mask_for_fx_on(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.alpha_mask_in_progress || self.current_rgba.is_empty() {
            return;
        }

        if !self.redaction_state.boxes.is_empty() {
            self.apply_redactions();
        }

        let snapshot = self.display_image_snapshot_request();

        let (cmd_bin, cmd_args) = super::super::background_removal::background_removal_command(
            self.bg_removal_cmd.as_deref(),
        );
        let stamp = self
            .creative_fx_state
            .mask_stamp(snapshot.input_stamp.clone());
        self.creative_fx_state.alpha_mask_stamp = Some(stamp.clone());
        self.creative_fx_state.alpha_mask_in_progress = true;
        self.creative_fx_state.alpha_mask_error = None;
        self.creative_fx_state.alpha_queue_status = None;
        self.creative_fx_state.alpha_retry_at = None;
        self.creative_fx_state.alpha_request = Some(CreativeFxMaskRequest {
            snapshot,
            cmd_bin,
            cmd_args,
            stamp,
            model_url: self.rmbg_model_url.clone(),
        });
        self.submit_creative_fx_alpha_attempt(jobs);
    }

    pub(super) fn submit_creative_fx_alpha_attempt(&mut self, jobs: &BackgroundJobs) {
        let Some(request) = self.creative_fx_state.alpha_request.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel::<CreativeFxMaskMsg>();
        self.creative_fx_state.alpha_mask_rx = Some(rx);
        self.creative_fx_state.alpha_retry_at = None;
        self.creative_fx_state.alpha_queue_status = None;
        let control = std::sync::Arc::new(ImageToolProcessControl::new());
        self.creative_fx_state.alpha_process_control = Some(control.clone());
        let (ticket, job_rx) = jobs.submit(std::time::Duration::MAX, move || {
            let is_default_bg_cmd = request.cmd_bin.contains("remove_bg.py")
                || request.cmd_args.iter().any(|a| a.contains("remove_bg.py"));
            if is_default_bg_cmd {
                if let Ok(model_path) = crate::onnx_runtime::ensure_model(
                    crate::onnx_runtime::FEYNOBG_FP16_MODEL_NAME,
                    request.model_url.as_deref(),
                ) {
                    let source_rgba = request.snapshot.compose();
                    match super::super::background_removal::run_native_background_removal(
                        &source_rgba,
                        &model_path,
                    ) {
                        Ok(cutout_rgba) => {
                            if control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                                return;
                            }
                            send_creative_fx_mask_result(&tx, &request.stamp, Ok(cutout_rgba));
                            return;
                        }
                        Err(_) => {}
                    }
                }
            }

            let mut cmd = Command::new(&request.cmd_bin);
            cmd.args(&request.cmd_args);
            let result = request
                .snapshot
                .compose_png()
                .map_err(|error| format!("Failed to encode image to PNG: {error}"))
                .and_then(|png_bytes| {
                    super::super::background_removal::run_image_tool_subprocess(
                        cmd,
                        &png_bytes,
                        None,
                        Some(&control.cancel),
                        Some(&control.child_pid),
                        BgRemovalOperation::BackgroundRemoval,
                        &request.cmd_bin,
                    )
                    .and_then(super::super::background_removal::decode_image_tool_output)
                });
            if result.is_err() && control.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            send_creative_fx_mask_result(&tx, &request.stamp, result);
        });
        self.creative_fx_state.alpha_job_ticket = Some(ticket);
        self.creative_fx_state.alpha_job_rx = Some(job_rx);
    }

    pub(super) fn poll_creative_fx_depth_mask(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.depth_estimation_in_progress
            && self
                .creative_fx_state
                .depth_estimation_stamp
                .as_ref()
                .map(|stamp| &stamp.input)
                != Some(&self.input_stamp())
        {
            if let Some(ticket) = self.creative_fx_state.depth_job_ticket.take() {
                ticket.cancel();
            }
            self.creative_fx_state.depth_job_rx = None;
            if let Some(control) = self.creative_fx_state.depth_process_control.take() {
                control.cancel();
            }
            self.creative_fx_state.depth_estimation_rx = None;
            self.creative_fx_state.depth_estimation_stamp = None;
            self.creative_fx_state.depth_request = None;
            self.creative_fx_state.depth_retry_at = None;
            self.creative_fx_state.depth_queue_status = None;
            self.creative_fx_state.depth_estimation_in_progress = false;
            return;
        }

        if self
            .creative_fx_state
            .depth_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.submit_creative_fx_depth_attempt(jobs);
        } else if self.creative_fx_state.depth_retry_at.is_some() {
            return;
        }

        if let Some(job_rx) = self.creative_fx_state.depth_job_rx.as_ref() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.creative_fx_state.depth_job_rx = None;
                    self.creative_fx_state.depth_job_ticket = None;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.creative_fx_state.depth_job_rx = None;
                    self.creative_fx_state.depth_job_ticket = None;
                    self.creative_fx_state.depth_estimation_rx = None;
                    self.creative_fx_state.depth_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                    self.creative_fx_state.depth_queue_status =
                        Some("Depth map waiting for workers; retrying shortly".into());
                    return;
                }
                Ok(Err(JobError::Cancelled)) => {
                    self.finish_creative_fx_depth_mask();
                    return;
                }
                Ok(Err(error)) => {
                    let should_report = self
                        .creative_fx_state
                        .depth_estimation_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    self.finish_creative_fx_depth_mask();
                    if should_report {
                        self.creative_fx_state.depth_estimation_error =
                            Some(format!("Depth map worker failed: {error}"));
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let should_report = self
                        .creative_fx_state
                        .depth_estimation_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    if let Some(control) = self.creative_fx_state.depth_process_control.take() {
                        control.cancel();
                    }
                    self.finish_creative_fx_depth_mask();
                    if should_report {
                        self.creative_fx_state.depth_estimation_error =
                            Some("Depth map worker stopped".into());
                    }
                    return;
                }
            }
        }
        let Some(rx) = self.creative_fx_state.depth_estimation_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(message) => {
                let stamp = message.stamp;
                let source_is_current = self.creative_fx_mask_source_matches(
                    self.creative_fx_state.depth_estimation_stamp.as_ref(),
                    &stamp,
                );
                let can_configure_selected =
                    source_is_current && self.creative_fx_mask_intent_matches(&stamp);
                match message.result {
                    Ok(depth_rgba) if source_is_current => {
                        self.accept_creative_fx_depth_mask(
                            stamp,
                            depth_rgba,
                            can_configure_selected,
                        );
                    }
                    Err(error) if can_configure_selected => {
                        self.creative_fx_state.depth_estimation_error = Some(error);
                    }
                    _ => {}
                }
                self.creative_fx_state.depth_estimation_in_progress = false;
                self.creative_fx_state.depth_estimation_stamp = None;
                self.creative_fx_state.depth_request = None;
                self.creative_fx_state.depth_retry_at = None;
                self.creative_fx_state.depth_queue_status = None;
                self.creative_fx_state.depth_process_control = None;
                self.creative_fx_state.depth_job_rx = None;
                self.creative_fx_state.depth_job_ticket = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.depth_estimation_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let should_report = self
                    .creative_fx_state
                    .depth_estimation_stamp
                    .as_ref()
                    .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                self.creative_fx_state.depth_estimation_in_progress = false;
                self.creative_fx_state.depth_estimation_stamp = None;
                self.creative_fx_state.depth_request = None;
                self.creative_fx_state.depth_retry_at = None;
                self.creative_fx_state.depth_queue_status = None;
                self.creative_fx_state.depth_process_control = None;
                self.creative_fx_state.depth_job_rx = None;
                self.creative_fx_state.depth_job_ticket = None;
                if should_report {
                    self.creative_fx_state.depth_estimation_error =
                        Some("Depth map worker stopped".into());
                }
            }
        }
    }

    pub(super) fn finish_creative_fx_depth_mask(&mut self) {
        if let Some(control) = self.creative_fx_state.depth_process_control.take() {
            control.cancel();
        }
        self.creative_fx_state.depth_estimation_in_progress = false;
        self.creative_fx_state.depth_estimation_rx = None;
        self.creative_fx_state.depth_estimation_stamp = None;
        self.creative_fx_state.depth_job_rx = None;
        self.creative_fx_state.depth_job_ticket = None;
        self.creative_fx_state.depth_request = None;
        self.creative_fx_state.depth_retry_at = None;
        self.creative_fx_state.depth_queue_status = None;
    }

    pub(super) fn poll_creative_fx_alpha_mask(&mut self, jobs: &BackgroundJobs) {
        if self.creative_fx_state.alpha_mask_in_progress
            && self
                .creative_fx_state
                .alpha_mask_stamp
                .as_ref()
                .map(|stamp| &stamp.input)
                != Some(&self.input_stamp())
        {
            if let Some(ticket) = self.creative_fx_state.alpha_job_ticket.take() {
                ticket.cancel();
            }
            self.creative_fx_state.alpha_job_rx = None;
            if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
                control.cancel();
            }
            self.creative_fx_state.alpha_mask_rx = None;
            self.creative_fx_state.alpha_mask_stamp = None;
            self.creative_fx_state.alpha_request = None;
            self.creative_fx_state.alpha_retry_at = None;
            self.creative_fx_state.alpha_queue_status = None;
            self.creative_fx_state.alpha_mask_in_progress = false;
            return;
        }

        if self
            .creative_fx_state
            .alpha_retry_at
            .is_some_and(|retry_at| std::time::Instant::now() >= retry_at)
        {
            self.submit_creative_fx_alpha_attempt(jobs);
        } else if self.creative_fx_state.alpha_retry_at.is_some() {
            return;
        }

        if let Some(job_rx) = self.creative_fx_state.alpha_job_rx.as_ref() {
            match job_rx.try_recv() {
                Ok(Ok(())) => {
                    self.creative_fx_state.alpha_job_rx = None;
                    self.creative_fx_state.alpha_job_ticket = None;
                }
                Ok(Err(JobError::QueueFull)) => {
                    self.creative_fx_state.alpha_job_rx = None;
                    self.creative_fx_state.alpha_job_ticket = None;
                    self.creative_fx_state.alpha_mask_rx = None;
                    self.creative_fx_state.alpha_retry_at =
                        Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                    self.creative_fx_state.alpha_queue_status =
                        Some("Alpha mask waiting for workers; retrying shortly".into());
                    return;
                }
                Ok(Err(JobError::Cancelled)) => {
                    self.finish_creative_fx_alpha_mask();
                    return;
                }
                Ok(Err(error)) => {
                    let should_report = self
                        .creative_fx_state
                        .alpha_mask_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    self.finish_creative_fx_alpha_mask();
                    if should_report {
                        self.creative_fx_state.alpha_mask_error =
                            Some(format!("Alpha mask worker failed: {error}"));
                    }
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let should_report = self
                        .creative_fx_state
                        .alpha_mask_stamp
                        .as_ref()
                        .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                    if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
                        control.cancel();
                    }
                    self.finish_creative_fx_alpha_mask();
                    if should_report {
                        self.creative_fx_state.alpha_mask_error =
                            Some("Alpha mask worker stopped".into());
                    }
                    return;
                }
            }
        }
        let Some(rx) = self.creative_fx_state.alpha_mask_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(message) => {
                let stamp = message.stamp;
                let source_is_current = self.creative_fx_mask_source_matches(
                    self.creative_fx_state.alpha_mask_stamp.as_ref(),
                    &stamp,
                );
                let can_configure_selected =
                    source_is_current && self.creative_fx_mask_intent_matches(&stamp);
                match message.result {
                    Ok(removal_result) if source_is_current => {
                        self.accept_creative_fx_alpha_mask(
                            stamp,
                            removal_result,
                            can_configure_selected,
                        );
                    }
                    Err(error) if can_configure_selected => {
                        self.creative_fx_state.alpha_mask_error = Some(error);
                    }
                    _ => {}
                }
                self.creative_fx_state.alpha_mask_in_progress = false;
                self.creative_fx_state.alpha_mask_stamp = None;
                self.creative_fx_state.alpha_process_control = None;
                self.creative_fx_state.alpha_request = None;
                self.creative_fx_state.alpha_retry_at = None;
                self.creative_fx_state.alpha_queue_status = None;
                self.creative_fx_state.alpha_job_rx = None;
                self.creative_fx_state.alpha_job_ticket = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.creative_fx_state.alpha_mask_rx = Some(rx);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let should_report = self
                    .creative_fx_state
                    .alpha_mask_stamp
                    .as_ref()
                    .is_some_and(|stamp| self.creative_fx_mask_intent_matches(stamp));
                self.creative_fx_state.alpha_mask_in_progress = false;
                self.creative_fx_state.alpha_mask_stamp = None;
                self.creative_fx_state.alpha_process_control = None;
                self.creative_fx_state.alpha_request = None;
                self.creative_fx_state.alpha_retry_at = None;
                self.creative_fx_state.alpha_queue_status = None;
                self.creative_fx_state.alpha_job_rx = None;
                self.creative_fx_state.alpha_job_ticket = None;
                if should_report {
                    self.creative_fx_state.alpha_mask_error =
                        Some("Alpha mask worker stopped".into());
                }
            }
        }
    }

    pub(super) fn finish_creative_fx_alpha_mask(&mut self) {
        if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
            control.cancel();
        }
        self.creative_fx_state.alpha_mask_in_progress = false;
        self.creative_fx_state.alpha_mask_rx = None;
        self.creative_fx_state.alpha_mask_stamp = None;
        self.creative_fx_state.alpha_job_rx = None;
        self.creative_fx_state.alpha_job_ticket = None;
        self.creative_fx_state.alpha_request = None;
        self.creative_fx_state.alpha_retry_at = None;
        self.creative_fx_state.alpha_queue_status = None;
    }

    pub(super) fn accept_creative_fx_depth_mask(
        &mut self,
        stamp: CreativeFxMaskStamp,
        depth_rgba: RgbaImage,
        can_configure_selected: bool,
    ) {
        self.creative_fx_state.cached_depth_map = Some(depth_rgba);
        self.creative_fx_state.depth_map_input_stamp = Some(stamp.input);
        self.creative_fx_state.depth_estimation_error = None;
        let active_uses_cache = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == CreativeFxMaskSource::DepthMap
        };
        if can_configure_selected {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = CreativeFxMaskSource::DepthMap;
            config.enabled = true;
        }
        if can_configure_selected || active_uses_cache {
            self.creative_fx_state.mask_cache_revision =
                self.creative_fx_state.mask_cache_revision.wrapping_add(1);
            self.creative_fx_state.mark_dirty();
            self.run_creative_fx_render();
        }
    }

    pub(super) fn accept_creative_fx_alpha_mask(
        &mut self,
        stamp: CreativeFxMaskStamp,
        removal_result: RgbaImage,
        can_configure_selected: bool,
    ) {
        self.creative_fx_state.cached_background_removal = Some(removal_result);
        self.creative_fx_state.background_removal_input_stamp = Some(stamp.input);
        self.creative_fx_state.alpha_mask_error = None;
        let active_uses_cache = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == CreativeFxMaskSource::BackgroundRemovalAlpha
        };
        if can_configure_selected {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = CreativeFxMaskSource::BackgroundRemovalAlpha;
            config.enabled = true;
        }
        if can_configure_selected || active_uses_cache {
            self.creative_fx_state.mask_cache_revision =
                self.creative_fx_state.mask_cache_revision.wrapping_add(1);
            self.creative_fx_state.mark_dirty();
            self.run_creative_fx_render();
        }
    }

    pub(super) fn toggle_creative_fx_mask_source(&mut self, source: CreativeFxMaskSource) {
        let active = {
            let config = self.creative_fx_state.depth_mask_for_active_fx();
            config.enabled && config.source == source
        };
        if active {
            self.creative_fx_state
                .depth_mask_for_active_fx_mut()
                .enabled = false;
            self.creative_fx_state.mark_dirty();
            return;
        }

        self.invalidate_stale_creative_fx_masks();
        let cached = match source {
            CreativeFxMaskSource::DepthMap => self.depth_mask_cache_is_current(),
            CreativeFxMaskSource::BackgroundRemovalAlpha => self.alpha_mask_cache_is_current(),
        };
        if cached {
            let config = self.creative_fx_state.depth_mask_for_active_fx_mut();
            config.source = source;
            config.enabled = true;
            self.creative_fx_state.mark_dirty();
        } else {
            match source {
                CreativeFxMaskSource::DepthMap => self.run_depth_estimation_for_fx(),
                CreativeFxMaskSource::BackgroundRemovalAlpha => self.run_alpha_mask_for_fx(),
            }
        }
    }

    pub(super) fn render_creative_fx_mask_sources(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        full_w: f32,
    ) {
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let base_c = hex_to_c32(&palette.harmonies.base);
        let diff_del = hex_to_c32(&palette.diff_del);
        let config = *self.creative_fx_state.depth_mask_for_active_fx();
        let depth_active = config.enabled && config.source == CreativeFxMaskSource::DepthMap;
        let alpha_active =
            config.enabled && config.source == CreativeFxMaskSource::BackgroundRemovalAlpha;
        self.invalidate_stale_creative_fx_masks();
        let has_depth = self.depth_mask_cache_is_current();
        let has_alpha = self.alpha_mask_cache_is_current();
        let depth_loading = self.creative_fx_state.depth_estimation_in_progress;
        let alpha_loading = self.creative_fx_state.alpha_mask_in_progress;
        let mask_loading = depth_loading || alpha_loading;

        ui.label(
            egui::RichText::new("Masking")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_high),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        if depth_loading {
            render_creative_fx_mask_loading(
                ui,
                self.creative_fx_state
                    .depth_queue_status
                    .as_deref()
                    .unwrap_or("Estimating depth map with Depth-Anything..."),
                base_c,
            );
            if let Some(retry_at) = self.creative_fx_state.depth_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        } else if alpha_loading {
            render_creative_fx_mask_loading(
                ui,
                self.creative_fx_state
                    .alpha_queue_status
                    .as_deref()
                    .unwrap_or("Removing background with FeyNoBg..."),
                base_c,
            );
            if let Some(retry_at) = self.creative_fx_state.alpha_retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
            } else {
                ctx.request_repaint();
            }
        }

        let depth_label = if depth_active {
            "Depth Mask: ON"
        } else if has_depth {
            "Depth map • cached"
        } else {
            "Use depth map as mask"
        };
        if render_creative_fx_mask_button(
            ui,
            crate::icons::Icon::Waves,
            depth_label,
            depth_active,
            mask_loading,
            full_w,
            palette,
        )
        .on_hover_text("Use AI monocular depth to mask this effect")
        .clicked()
        {
            self.toggle_creative_fx_mask_source(CreativeFxMaskSource::DepthMap);
        }

        ui.add_space(3.0);
        let alpha_label = if alpha_active {
            "Removal Alpha: ON"
        } else if has_alpha {
            "Removal alpha • cached"
        } else {
            "Use removal alpha as mask"
        };
        if render_creative_fx_mask_button(
            ui,
            crate::icons::Icon::Scissors,
            alpha_label,
            alpha_active,
            mask_loading,
            full_w,
            palette,
        )
        .on_hover_text("Use the previous Remove BG result, or run automatic FeyNoBg removal")
        .clicked()
        {
            self.toggle_creative_fx_mask_source(CreativeFxMaskSource::BackgroundRemovalAlpha);
        }

        if let Some(error) = &self.creative_fx_state.depth_estimation_error {
            ui.add_space(2.0);
            ui.add(
                crate::icons::Icon::Alert.label(
                    egui::RichText::new(error.to_string())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(diff_del),
                ),
            );
        }
        if let Some(error) = &self.creative_fx_state.alpha_mask_error {
            ui.add_space(2.0);
            ui.add(
                crate::icons::Icon::Alert.label(
                    egui::RichText::new(error.to_string())
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(diff_del),
                ),
            );
        }
    }

    pub(super) fn render_creative_fx_mask_options(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        cur_fx: CreativeFxType,
        full_w: f32,
        slider_w: f32,
    ) {
        let config = *self.creative_fx_state.depth_mask_for_active_fx();
        if !config.enabled {
            return;
        }

        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let f_high = hex_to_c32(&palette.colors.text_primary);
        ui.add_space(crate::ui_ext::SPACE_XS);
        let invert_text = match (config.source, config.invert) {
            (CreativeFxMaskSource::DepthMap, true) => "⇄ Invert Mask: ON (Far / Background)",
            (CreativeFxMaskSource::DepthMap, false) => "⇄ Invert Mask: OFF (Near / Foreground)",
            (CreativeFxMaskSource::BackgroundRemovalAlpha, true) => {
                "⇄ Invert Mask: ON (Background)"
            }
            (CreativeFxMaskSource::BackgroundRemovalAlpha, false) => {
                "⇄ Invert Mask: OFF (Foreground)"
            }
        };
        let invert_button = egui::Button::new(
            egui::RichText::new(invert_text)
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_high),
        )
        .fill(b_med)
        .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
        .min_size(Vec2::new(full_w, crate::ui_ext::CONTROL_HEIGHT));
        if ui
            .add(invert_button)
            .on_hover_text("Swap the foreground and background mask regions")
            .clicked()
        {
            self.creative_fx_state.depth_mask_for_active_fx_mut().invert = !config.invert;
            self.creative_fx_state.mark_dirty();
        }

        if cur_fx == CreativeFxType::BasicControls {
            self.render_basic_masked_zoom(ui, palette, slider_w);
        }

        let supports_depth_cutoff = matches!(
            cur_fx,
            CreativeFxType::BasicControls
                | CreativeFxType::PixelSort
                | CreativeFxType::CmykHalftone
                | CreativeFxType::DitherPalette
                | CreativeFxType::Ntsc
                | CreativeFxType::Structure
        );
        if config.source != CreativeFxMaskSource::DepthMap || !supports_depth_cutoff {
            return;
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let mut threshold = config.threshold;
        if render_context_slider(
            ui,
            "Depth Cutoff (0: Continuous gradient)",
            &mut threshold,
            0.0..=1.0,
            0.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state
                .depth_mask_for_active_fx_mut()
                .threshold = threshold;
            self.creative_fx_state.mark_dirty();
        }

        if threshold > 0.001 {
            let mut feather = config.feather;
            if render_context_slider(
                ui,
                "Feather Softness",
                &mut feather,
                0.01..=0.50,
                0.25,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state
                    .depth_mask_for_active_fx_mut()
                    .feather = feather;
                self.creative_fx_state.mark_dirty();
            }
        }
    }
}
