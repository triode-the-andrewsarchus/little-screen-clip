use super::super::*;

/// Keep numeric rows aligned and readable even when the analysis drawer is narrow.
fn draw_numeric_rows(
    ui: &mut egui::Ui,
    id: &str,
    rows: impl IntoIterator<Item = String>,
    color: Color32,
) {
    egui::ScrollArea::horizontal()
        .id_source(id)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.vertical(|ui| {
                for row in rows {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(row)
                                .font(egui::FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                                .color(color),
                        )
                        .wrap(false),
                    );
                }
            });
        });
}

#[derive(Debug, Clone, PartialEq)]
struct DcsRequestStamp {
    image: ImageInputStamp,
    params: DecorrelationStretchParams,
}

#[derive(Clone)]
struct DcsRequest {
    snapshot: DisplayImageSnapshotRequest,
    params: DecorrelationStretchParams,
    stamp: DcsRequestStamp,
}

struct DcsWorkerMsg {
    stamp: DcsRequestStamp,
    duration_ms: u128,
    result: Result<(DecorrelationStats, DecorrelationTransform, RgbaImage), String>,
}

pub struct DecorrelationStretchState {
    pub params: DecorrelationStretchParams,
    pub custom_mean: f64,
    pub custom_sigma: f64,
    pub stats: Option<DecorrelationStats>,
    pub transform: Option<DecorrelationTransform>,
    pub stretched_rgba: Option<RgbaImage>,
    pub stretched_texture: Option<egui::TextureHandle>,
    pub dirty_texture: bool,
    pub comparison_split: f32,
    pub is_dragging_split: bool,
    pub is_holding_original: bool,
    pub last_duration_ms: u128,
    pub params_dirty: bool,
    worker_rx:
        Option<std::sync::mpsc::Receiver<Result<Option<DcsWorkerMsg>, little_clip_core::JobError>>>,
    worker_ticket: Option<little_clip_core::JobTicket>,
    worker_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    request_stamp: Option<DcsRequestStamp>,
    preview_stamp: Option<DcsRequestStamp>,
    pub(crate) retry_at: Option<std::time::Instant>,
    retry_stamp: Option<DcsRequestStamp>,
    retry_request: Option<DcsRequest>,
    pub(crate) is_computing: bool,
    pub(crate) queue_status: Option<String>,
    pub(crate) error: Option<String>,
}

impl Default for DecorrelationStretchState {
    fn default() -> Self {
        Self {
            params: DecorrelationStretchParams::nasa(),
            custom_mean: 127.5,
            custom_sigma: 50.0,
            stats: None,
            transform: None,
            stretched_rgba: None,
            stretched_texture: None,
            dirty_texture: false,
            comparison_split: 0.50,
            is_dragging_split: false,
            is_holding_original: false,
            last_duration_ms: 0,
            params_dirty: false,
            worker_rx: None,
            worker_ticket: None,
            worker_cancel: None,
            request_stamp: None,
            preview_stamp: None,
            retry_at: None,
            retry_stamp: None,
            retry_request: None,
            is_computing: false,
            queue_status: None,
            error: None,
        }
    }
}

impl ImagePreviewState {
    pub fn update_decorrelation_stretch(&mut self) {
        self.update_decorrelation_stretch_on(little_clip_core::BackgroundJobs::global());
    }

    fn current_dcs_stamp(&self) -> DcsRequestStamp {
        DcsRequestStamp {
            image: self.input_stamp(),
            params: self.dcs_state.params,
        }
    }

    fn update_decorrelation_stretch_on(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        self.cancel_decorrelation_stretch();
        let snapshot = self.display_image_snapshot_request();
        let request = DcsRequest {
            stamp: DcsRequestStamp {
                image: snapshot.input_stamp.clone(),
                params: self.dcs_state.params,
            },
            snapshot,
            params: self.dcs_state.params,
        };
        self.submit_decorrelation_stretch(request, jobs);
    }

    fn submit_decorrelation_stretch(
        &mut self,
        request: DcsRequest,
        jobs: &little_clip_core::BackgroundJobs,
    ) {
        let request_stamp = request.stamp.clone();
        let worker_stamp = request_stamp.clone();
        let worker_request = request.clone();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::MAX, move || {
            let started = std::time::Instant::now();
            let result = if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                Err("Decorrelation stretch was cancelled".into())
            } else {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let image = worker_request.snapshot.compose();
                    run_decorrelation_stretch_cancellable(&image, &worker_request.params, &|| {
                        worker_cancel.load(std::sync::atomic::Ordering::Acquire)
                    })
                    .ok_or_else(|| "Decorrelation stretch was cancelled".to_string())
                }))
                .unwrap_or_else(|_| Err("Decorrelation stretch worker panicked".into()))
            };
            if worker_cancel.load(std::sync::atomic::Ordering::Acquire) {
                return None;
            }
            Some(DcsWorkerMsg {
                stamp: worker_stamp,
                duration_ms: started.elapsed().as_millis(),
                result,
            })
        });
        self.dcs_state.worker_ticket = Some(ticket);
        self.dcs_state.worker_rx = Some(rx);
        self.dcs_state.worker_cancel = Some(cancel);
        self.dcs_state.request_stamp = Some(request_stamp.clone());
        self.dcs_state.retry_request = Some(request);
        self.dcs_state.is_computing = true;
        self.dcs_state.queue_status = None;
        self.dcs_state.error = None;
        self.dcs_state.retry_at = None;
        self.dcs_state.retry_stamp = None;
        self.dcs_state.params_dirty = false;
    }

    pub(crate) fn poll_decorrelation_stretch(
        &mut self,
        ctx: &egui::Context,
        jobs: &little_clip_core::BackgroundJobs,
    ) {
        let current_stamp = self.current_dcs_stamp();
        if self
            .dcs_state
            .request_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &current_stamp)
        {
            self.cancel_decorrelation_stretch();
        }
        if self
            .dcs_state
            .retry_stamp
            .as_ref()
            .is_some_and(|stamp| stamp != &current_stamp)
        {
            self.dcs_state.retry_at = None;
            self.dcs_state.retry_stamp = None;
            self.dcs_state.retry_request = None;
            self.dcs_state.queue_status = None;
        }
        if let Some(retry_at) = self.dcs_state.retry_at {
            if std::time::Instant::now() < retry_at {
                ctx.request_repaint_after(
                    retry_at.saturating_duration_since(std::time::Instant::now()),
                );
                return;
            }
            if let Some(request) = self.dcs_state.retry_request.take() {
                self.dcs_state.retry_at = None;
                self.dcs_state.retry_stamp = None;
                self.submit_decorrelation_stretch(request, jobs);
            }
            return;
        }
        let Some(rx) = self.dcs_state.worker_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Some(message))) => {
                self.clear_dcs_worker();
                if message.stamp != current_stamp {
                    return;
                }
                match message.result {
                    Ok((stats, transform, stretched)) => {
                        self.dcs_state.stats = Some(stats);
                        self.dcs_state.transform = Some(transform);
                        self.dcs_state.stretched_rgba = Some(stretched);
                        self.dcs_state.preview_stamp = Some(message.stamp);
                        self.dcs_state.dirty_texture = true;
                        self.dcs_state.last_duration_ms = message.duration_ms;
                        self.dcs_state.error = None;
                    }
                    Err(error) => self.dcs_state.error = Some(error),
                }
            }
            Ok(Ok(None)) => self.clear_dcs_worker(),
            Ok(Err(little_clip_core::JobError::QueueFull)) => {
                self.dcs_state.worker_ticket = None;
                self.dcs_state.worker_cancel = None;
                self.dcs_state.worker_rx = None;
                self.dcs_state.request_stamp = None;
                self.dcs_state.is_computing = false;
                self.dcs_state.retry_stamp = Some(current_stamp);
                self.dcs_state.retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(500));
                self.dcs_state.queue_status =
                    Some("Background workers are busy; retrying shortly".into());
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            Ok(Err(little_clip_core::JobError::Cancelled)) => self.clear_dcs_worker(),
            Ok(Err(error)) => {
                self.clear_dcs_worker();
                self.dcs_state.error =
                    Some(format!("Decorrelation stretch worker failed: {error}"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.dcs_state.worker_rx = Some(rx);
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.clear_dcs_worker();
                self.dcs_state.error =
                    Some("Decorrelation stretch worker stopped unexpectedly".into());
            }
        }
    }

    fn clear_dcs_worker(&mut self) {
        self.dcs_state.worker_ticket = None;
        self.dcs_state.worker_cancel = None;
        self.dcs_state.worker_rx = None;
        self.dcs_state.request_stamp = None;
        self.dcs_state.is_computing = false;
        self.dcs_state.retry_request = None;
    }

    pub(crate) fn cancel_decorrelation_stretch(&mut self) {
        if let Some(ticket) = self.dcs_state.worker_ticket.take() {
            ticket.cancel();
        }
        if let Some(cancel) = self.dcs_state.worker_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
        self.dcs_state.worker_rx = None;
        self.dcs_state.request_stamp = None;
        self.dcs_state.retry_at = None;
        self.dcs_state.retry_stamp = None;
        self.dcs_state.retry_request = None;
        self.dcs_state.is_computing = false;
        self.dcs_state.queue_status = None;
    }

    pub(crate) fn dcs_preview_is_current(&self) -> bool {
        self.dcs_state.preview_stamp.as_ref() == Some(&self.current_dcs_stamp())
    }

    pub(crate) fn invalidate_decorrelation_stretch(&mut self) {
        self.cancel_decorrelation_stretch();
        self.dcs_state.stretched_rgba = None;
        self.dcs_state.stretched_texture = None;
        self.dcs_state.stats = None;
        self.dcs_state.transform = None;
        self.dcs_state.preview_stamp = None;
        self.dcs_state.dirty_texture = true;
    }

    pub fn apply_decorrelation_stretch(&mut self) {
        if self.dcs_state.preview_stamp.as_ref() != Some(&self.current_dcs_stamp()) {
            self.show_toast("Wait for the current stretch preview before applying");
            return;
        }
        if let Some(stretched) = self.dcs_state.stretched_rgba.clone() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.current_rgba = Arc::new(stretched);
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.resize_state.sync_from_image(&self.current_rgba);
            self.palette_state.is_extracted = false;
            self.update_decorrelation_stretch();
            self.show_toast("✔ Applied decorrelation stretch to canvas");
        }
    }

    pub(crate) fn render_decorrelation_stretch_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let surface = hex_to_c32(&palette.colors.surface_panel);
        let border = hex_to_c32(&palette.colors.surface_recessed);
        let text = hex_to_c32(&palette.colors.text_secondary);
        let muted = hex_to_c32(&palette.colors.text_muted);
        let success = hex_to_c32(&palette.diff_add);
        let mut recompute = false;

        ui.label(
            egui::RichText::new("Decorrelation stretch")
                .size(crate::ui_ext::HEADING_FONT_SIZE)
                .strong()
                .color(accent),
        );

        if self.dcs_state.is_computing {
            ui.label(egui::RichText::new("Computing decorrelation stretch…").color(muted));
        }
        if let Some(status) = &self.dcs_state.queue_status {
            ui.label(egui::RichText::new(status).color(muted));
        }
        if let Some(error) = &self.dcs_state.error {
            ui.label(egui::RichText::new(error).color(hex_to_c32(&palette.diff_del)));
        }
        ui.label(
            egui::RichText::new("Separate correlated color channels to reveal subtle variation.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("PRESET")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let preset_width =
            crate::ui_ext::wrapped_equal_button_width(ui, &["NASA / ASTER", "Preserve"], false, 2);
        ui.horizontal_wrapped(|ui| {
            for (preset, label, tooltip) in [
                (
                    DecorrelationStretchParams::nasa(),
                    "NASA / ASTER",
                    "Balanced color enhancement with mean 127.5 and spread 50",
                ),
                (
                    DecorrelationStretchParams::matlab_preserve(),
                    "Preserve",
                    "MATLAB-style stretch that preserves band means and spread",
                ),
            ] {
                let active = self.dcs_state.params == preset;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui
                    .add_sized([preset_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .on_hover_text(tooltip)
                    .clicked()
                    && !active
                {
                    self.dcs_state.params = preset;
                    if preset == DecorrelationStretchParams::nasa() {
                        self.dcs_state.custom_mean = 127.5;
                        self.dcs_state.custom_sigma = 50.0;
                    }
                    recompute = true;
                }
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("COLOR MODEL")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        let mode_width =
            crate::ui_ext::wrapped_equal_button_width(ui, &["Correlation", "Covariance"], false, 2);
        ui.horizontal_wrapped(|ui| {
            for (mode, label, tooltip) in [
                (
                    DecorrelationMatrixMode::Correlation,
                    "Correlation",
                    "Give each RGB band equal weight",
                ),
                (
                    DecorrelationMatrixMode::Covariance,
                    "Covariance",
                    "Weight RGB bands by their original variance",
                ),
            ] {
                let active = self.dcs_state.params.mode == mode;
                let fill = if active { accent } else { surface };
                let foreground = contrast_text_c32(fill, Some(text), palette);
                let button = egui::Button::new(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(foreground),
                )
                .fill(fill)
                .stroke(Stroke::new(
                    1.0_f32,
                    if active { foreground } else { border },
                ))
                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS));
                if ui
                    .add_sized([mode_width, crate::ui_ext::CONTROL_HEIGHT], button)
                    .on_hover_text(tooltip)
                    .clicked()
                    && !active
                {
                    self.dcs_state.params.mode = mode;
                    recompute = true;
                }
            }
        });

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OUTPUT DISTRIBUTION")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );

        let mut preserve_mean = matches!(self.dcs_state.params.target_mean, TargetValue::Preserve);
        if ui
            .checkbox(
                &mut preserve_mean,
                egui::RichText::new("Preserve original band means")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(text),
            )
            .changed()
        {
            self.dcs_state.params.target_mean = if preserve_mean {
                TargetValue::Preserve
            } else {
                TargetValue::Custom(self.dcs_state.custom_mean)
            };
            recompute = true;
        }
        if !preserve_mean {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Target mean")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.1}", self.dcs_state.custom_mean))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let response = ui.add(
                egui::Slider::new(&mut self.dcs_state.custom_mean, 0.0..=255.0).show_value(false),
            );
            if response.changed() {
                self.dcs_state.params.target_mean = TargetValue::Custom(self.dcs_state.custom_mean);
                self.dcs_state.params_dirty = true;
                recompute |= !response.dragged();
            }
            if response.drag_stopped() && self.dcs_state.params_dirty {
                recompute = true;
            }
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        let mut preserve_sigma =
            matches!(self.dcs_state.params.target_sigma, TargetValue::Preserve);
        if ui
            .checkbox(
                &mut preserve_sigma,
                egui::RichText::new("Preserve original band spread")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(text),
            )
            .changed()
        {
            self.dcs_state.params.target_sigma = if preserve_sigma {
                TargetValue::Preserve
            } else {
                TargetValue::Custom(self.dcs_state.custom_sigma)
            };
            recompute = true;
        }
        if !preserve_sigma {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Target spread")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.1}", self.dcs_state.custom_sigma))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            let response = ui.add(
                egui::Slider::new(&mut self.dcs_state.custom_sigma, 1.0..=200.0).show_value(false),
            );
            if response.changed() {
                self.dcs_state.params.target_sigma =
                    TargetValue::Custom(self.dcs_state.custom_sigma);
                self.dcs_state.params_dirty = true;
                recompute |= !response.dragged();
            }
            if response.drag_stopped() && self.dcs_state.params_dirty {
                recompute = true;
            }
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("SAMPLING & CLIPPING")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(text),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);

        ui.label(
            egui::RichText::new("Highlight clipping")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        let current_tol = self.dcs_state.params.tol;
        egui::ComboBox::from_id_source("dcs_tol_combo")
            .selected_text(match current_tol {
                value if value <= 0.0 => "Off".to_string(),
                value => format!("{:.1}% per tail", value * 100.0),
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (value, label) in [
                    (0.0, "Off"),
                    (0.005, "0.5% per tail"),
                    (0.01, "1% per tail"),
                    (0.02, "2% per tail"),
                ] {
                    if ui
                        .selectable_label((current_tol - value).abs() < 1e-4, label)
                        .clicked()
                    {
                        self.dcs_state.params.tol = value;
                        recompute = true;
                    }
                }
            });

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("Statistics sampling")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(muted),
        );
        let current_stride = self.dcs_state.params.stride;
        egui::ComboBox::from_id_source("dcs_stride_combo")
            .selected_text(match current_stride {
                0 => "Automatic".to_string(),
                1 => "Every pixel".to_string(),
                value => format!("Every {} pixels", value),
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (value, label) in [
                    (0, "Automatic"),
                    (1, "Every pixel"),
                    (2, "Every 2 pixels"),
                    (3, "Every 3 pixels"),
                    (5, "Every 5 pixels"),
                ] {
                    if ui
                        .selectable_label(current_stride == value, label)
                        .clicked()
                    {
                        self.dcs_state.params.stride = value;
                        recompute = true;
                    }
                }
            });

        ui.add_space(crate::ui_ext::SPACE_MD);
        let hold = crate::secondary_button("Hold to view original", surface, palette);
        let hold_response = ui
            .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], hold)
            .on_hover_text("Press and hold to temporarily hide the stretch");
        self.dcs_state.is_holding_original = hold_response.is_pointer_button_down_on();

        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        let apply = crate::action_button("Apply stretch (Enter)", success, palette);
        if ui
            .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], apply)
            .on_hover_text("Commit the current stretch to the image")
            .clicked()
        {
            self.apply_decorrelation_stretch();
        }

        ui.add_space(crate::ui_ext::SPACE_XS);
        let refresh = crate::secondary_button("Recompute preview", surface, palette);
        if ui
            .add_sized(
                [ui.available_width(), crate::ui_ext::CONTROL_HEIGHT],
                refresh,
            )
            .on_hover_text("Recalculate statistics and the color transform")
            .clicked()
        {
            self.update_decorrelation_stretch();
        }

        if !self.undo_stack.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_XS);
            let undo = crate::secondary_button("Undo last edit (Ctrl+Z)", surface, palette);
            if ui
                .add_sized([ui.available_width(), crate::ui_ext::CONTROL_HEIGHT], undo)
                .clicked()
            {
                self.undo();
                self.update_decorrelation_stretch();
            }
        }

        if recompute {
            self.update_decorrelation_stretch();
        }
    }

    pub(crate) fn render_decorrelation_stretch_stats_drawer(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
    ) {
        let f_high = hex_to_c32(&palette.colors.text_primary);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        let f_low = hex_to_c32(&palette.colors.text_muted);
        let diff_del = hex_to_c32(&palette.diff_del);
        let base_c = hex_to_c32(&palette.harmonies.base);

        ui.label(
            egui::RichText::new("Stretch analysis")
                .size(crate::ui_ext::HEADING_FONT_SIZE)
                .strong()
                .color(base_c),
        );
        ui.label(
            egui::RichText::new("Input statistics and the generated RGB transform.")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_low),
        );

        if !self.dcs_preview_is_current() {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.label(
                egui::RichText::new("Waiting for analysis with the current image and settings…")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_low),
            );
            return;
        }

        let (Some(stats), Some(transform)) = (&self.dcs_state.stats, &self.dcs_state.transform)
        else {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.label(
                egui::RichText::new("No analysis is available yet.")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .italics()
                    .color(f_med),
            );
            return;
        };

        let stride_used = if self.dcs_state.params.stride == 0 {
            auto_stride(self.current_rgba.width(), self.current_rgba.height())
        } else {
            self.dcs_state.params.stride
        };

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.label(
            egui::RichText::new("OVERVIEW")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        for (label, value) in [
            ("Samples", format!("{} px", stats.sample_count)),
            ("Compute", format!("{} ms", self.dcs_state.last_duration_ms)),
            ("Stride", stride_used.to_string()),
            (
                "Matrix",
                match self.dcs_state.params.mode {
                    DecorrelationMatrixMode::Correlation => "Correlation".to_string(),
                    DecorrelationMatrixMode::Covariance => "Covariance".to_string(),
                },
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(label)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_low),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(value)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(f_high),
                    );
                });
            });
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("CHANNEL STATISTICS")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        for (index, band) in ["R", "G", "B"].into_iter().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(band)
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(base_c),
                );
                ui.label(
                    egui::RichText::new(format!("Mean {:.1}", stats.mean[index]))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(f_high),
                );
                ui.label(
                    egui::RichText::new(format!("Spread {:.1}", stats.sd[index]))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .monospace()
                        .color(f_high),
                );
            });
        }

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("CORRELATION MATRIX")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_corr",
            stats
                .corr
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2])),
            f_high,
        );

        ui.add_space(10.0);
        ui.label(
            egui::RichText::new("COVARIANCE MATRIX")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_cov",
            stats
                .cov
                .map(|row| format!("[{:>7.1}  {:>7.1}  {:>7.1}]", row[0], row[1], row[2])),
            f_high,
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("PRINCIPAL COMPONENTS")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        for (index, value) in transform.eigenvalues.into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("λ{}", index + 1))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_low),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.5}", value))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(f_high),
                    );
                });
            });
        }

        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("EIGENVECTORS")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_eigenvectors",
            transform
                .eigenvectors
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2])),
            f_high,
        );

        ui.add_space(crate::ui_ext::SPACE_MD);
        ui.separator();
        ui.add_space(crate::ui_ext::SPACE_SM);
        ui.label(
            egui::RichText::new("RGB TRANSFORM")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(f_med),
        );
        draw_numeric_rows(
            ui,
            "dcs_transform",
            transform
                .transform
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2])),
            f_high,
        );
        ui.add_space(crate::ui_ext::SPACE_XS);
        ui.label(
            egui::RichText::new(format!(
                "Offset  [{:.1}, {:.1}, {:.1}]",
                transform.offset[0], transform.offset[1], transform.offset[2]
            ))
            .size(crate::ui_ext::BODY_FONT_SIZE)
            .monospace()
            .color(f_low),
        );

        if let Some(limits) = transform.tol_limits {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.label(
                egui::RichText::new("CLIPPING LIMITS")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(f_med),
            );
            for (index, band) in ["R", "G", "B"].into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(band)
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(f_low),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.1} – {:.1}",
                                limits[index][0], limits[index][1]
                            ))
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .monospace()
                            .color(f_high),
                        );
                    });
                });
            }
        }

        if !transform.warnings.is_empty() {
            ui.add_space(crate::ui_ext::SPACE_MD);
            ui.separator();
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.label(
                egui::RichText::new("DIAGNOSTICS")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(diff_del),
            );
            for warning in &transform.warnings {
                ui.label(
                    egui::RichText::new(format!("⚠ {}", warning))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(diff_del),
                );
            }
        }
    }

    pub(crate) fn render_decorrelation_stretch_comparison(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        _canvas_rect: Rect,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        // Ensure stretched texture is loaded
        if self.dcs_state.dirty_texture || self.dcs_state.stretched_texture.is_none() {
            if let Some(ref dcs_rgba) = self.dcs_state.stretched_rgba {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [dcs_rgba.width() as usize, dcs_rgba.height() as usize],
                    dcs_rgba.as_raw(),
                );
                self.dcs_state.stretched_texture = Some(ctx.load_texture(
                    "decorrelation_stretch_preview_tex",
                    color_img,
                    image_texture_options(dcs_rgba),
                ));
                self.dcs_state.dirty_texture = false;
            }
        }

        let base_c = hex_to_c32(&palette.harmonies.base);

        // If user is holding original (via Hold to Compare button), show full original
        if self.dcs_state.is_holding_original {
            if let Some(ref orig_tex) = self.texture {
                let mut full_mesh = egui::Mesh::with_texture(orig_tex.id());
                full_mesh.add_rect_with_uv(
                    img_rect,
                    Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                painter.add(egui::Shape::mesh(full_mesh));

                let bounds = img_rect.intersect(painter.clip_rect());
                crate::ui_ext::CanvasBadge {
                    heading: "ORIGINAL (HOLDING)",
                    value: None,
                    accent: None,
                }
                .paint(
                    painter,
                    bounds.min + Vec2::splat(8.0),
                    egui::Align2::LEFT_TOP,
                    bounds,
                    (bounds.width() - 16.0).max(1.0),
                    palette,
                );
            }
            return;
        }

        let split = self.dcs_state.comparison_split.clamp(0.0, 1.0);
        let split_x = img_rect.min.x + split * img_rect.width();

        // 1. Draw Left Side (Original / Before)
        if let Some(ref orig_tex) = self.texture {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut left_mesh = egui::Mesh::with_texture(orig_tex.id());
            left_mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(left_mesh));
        }

        // 2. Draw Right Side (Decorrelation Stretched / After)
        let dcs_tex_id = self
            .dcs_state
            .stretched_texture
            .as_ref()
            .map(|t| t.id())
            .or_else(|| self.texture.as_ref().map(|t| t.id()));
        if let Some(tex_id) = dcs_tex_id {
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut right_mesh = egui::Mesh::with_texture(tex_id);
            right_mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(right_mesh));
        }

        // 3. Draw Crisp Divider Line
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(3.0_f32, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(1.5_f32, Color32::WHITE),
        );

        // 4. Draggable Center Knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_radius = crate::ui_ext::paint_split_handle(painter, knob_center, palette);

        // 5. Draggable Handle Interaction
        let ptr_pos = response.hover_pos();
        let near_divider =
            crate::ui_ext::split_handle_hit_test(ptr_pos, img_rect, knob_center, knob_radius, 16.0);

        if near_divider || self.dcs_state.is_dragging_split {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            self.dcs_state.is_dragging_split = true;
        }

        if self.dcs_state.is_dragging_split {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let new_split = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    self.dcs_state.comparison_split = new_split;
                }
            }
            if response.drag_stopped() {
                self.dcs_state.is_dragging_split = false;
            }
        }

        // 6. Floating HUD Badges
        let bounds = img_rect.intersect(painter.clip_rect());
        let gap = 8.0;
        let max_width = ((bounds.width() - gap * 3.0) * 0.5).max(1.0);
        crate::ui_ext::CanvasBadge {
            heading: "ORIGINAL",
            value: None,
            accent: None,
        }
        .paint(
            painter,
            bounds.min + Vec2::splat(gap),
            egui::Align2::LEFT_TOP,
            bounds,
            max_width,
            palette,
        );
        let after_text = format!("DCS ({:.0} ms)", self.dcs_state.last_duration_ms);
        crate::ui_ext::CanvasBadge {
            heading: &after_text,
            value: None,
            accent: Some(base_c),
        }
        .paint(
            painter,
            Pos2::new(bounds.right() - gap, bounds.top() + gap),
            egui::Align2::RIGHT_TOP,
            bounds,
            max_width,
            palette,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> ImagePreviewState {
        let mut png = Vec::new();
        RgbaImage::from_fn(96, 72, |x, y| {
            image::Rgba([
                (x * 17 + y * 3) as u8,
                (y * 13 + x * 5) as u8,
                ((x + y) * 7) as u8,
                255,
            ])
        })
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        ImagePreviewState::from_bytes_with_meta(
            png,
            "image/png".into(),
            None,
            "DCS async test".into(),
            &little_clip_core::config::AppConfig::default(),
        )
        .unwrap()
    }

    fn wait_for_dcs(state: &mut ImagePreviewState, jobs: &little_clip_core::BackgroundJobs) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while (state.dcs_state.is_computing || state.dcs_state.retry_at.is_some())
            && std::time::Instant::now() < deadline
        {
            state.poll_decorrelation_stretch(&ctx, jobs);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        state.poll_decorrelation_stretch(&ctx, jobs);
        assert!(
            !state.dcs_state.is_computing,
            "DCS worker completes before deadline"
        );
        assert!(state.dcs_state.error.is_none());
    }

    #[test]
    fn bounded_dcs_output_matches_direct_current_result_and_can_apply() {
        let mut state = test_state();
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let base = state.current_display_image();
        let expected = little_clip_ops::decorrelation_stretch::run_decorrelation_stretch(
            &base,
            &state.dcs_state.params,
        );
        state.update_decorrelation_stretch_on(&jobs);
        assert!(state.dcs_state.is_computing);
        wait_for_dcs(&mut state, &jobs);
        assert_eq!(state.dcs_state.stats.as_ref(), Some(&expected.0));
        assert_eq!(state.dcs_state.transform.as_ref(), Some(&expected.1));
        assert_eq!(state.dcs_state.stretched_rgba.as_ref(), Some(&expected.2));
        assert!(state.dcs_preview_is_current());

        let output = state.dcs_state.stretched_rgba.clone().unwrap();
        state.apply_decorrelation_stretch();
        assert_eq!(state.current_rgba.as_ref(), &output);
        assert_eq!(state.undo_stack.len(), 1);
    }

    #[test]
    fn queue_full_shows_delayed_retry_and_reuses_snapshot_without_losing_preview() {
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_, filler_rx) = jobs.submit(std::time::Duration::MAX, || ());

        let mut state = test_state();
        let old_preview = RgbaImage::from_pixel(3, 3, image::Rgba([1, 2, 3, 255]));
        state.dcs_state.stretched_rgba = Some(old_preview.clone());
        state.update_decorrelation_stretch_on(&jobs);
        let retained = std::sync::Arc::clone(
            &state
                .dcs_state
                .retry_request
                .as_ref()
                .unwrap()
                .snapshot
                .pixels,
        );
        let ctx = egui::Context::default();
        state.poll_decorrelation_stretch(&ctx, &jobs);
        assert!(!state.dcs_state.is_computing);
        assert!(state.dcs_state.retry_at.is_some());
        assert!(
            state
                .dcs_state
                .queue_status
                .as_deref()
                .unwrap()
                .contains("busy")
        );
        assert_eq!(state.dcs_state.stretched_rgba.as_ref(), Some(&old_preview));
        assert!(std::sync::Arc::ptr_eq(
            &retained,
            &state
                .dcs_state
                .retry_request
                .as_ref()
                .unwrap()
                .snapshot
                .pixels
        ));
        let deadline = state.dcs_state.retry_at.unwrap();
        state.poll_decorrelation_stretch(&ctx, &jobs);
        assert_eq!(state.dcs_state.retry_at, Some(deadline));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        filler_rx.recv().unwrap().unwrap();
        state.dcs_state.retry_at = Some(std::time::Instant::now());
        state.poll_decorrelation_stretch(&ctx, &jobs);
        assert!(state.dcs_state.is_computing);
        wait_for_dcs(&mut state, &jobs);
        let expected = little_clip_ops::decorrelation_stretch::run_decorrelation_stretch(
            &state.current_display_image(),
            &state.dcs_state.params,
        );
        assert_eq!(state.dcs_state.stretched_rgba.as_ref(), Some(&expected.2));
        assert!(state.dcs_state.queue_status.is_none());
    }

    #[test]
    fn stale_parameters_cancel_queued_result_and_prevent_apply() {
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let mut state = test_state();
        state.update_decorrelation_stretch_on(&jobs);
        let cancel = state.dcs_state.worker_cancel.as_ref().unwrap().clone();
        state.dcs_state.params.tol = 0.01;
        state.poll_decorrelation_stretch(&egui::Context::default(), &jobs);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(!state.dcs_state.is_computing);
        assert!(state.dcs_state.retry_request.is_none());
        let before_apply = state.current_rgba.clone();
        state.dcs_state.stretched_rgba =
            Some(RgbaImage::from_pixel(2, 2, image::Rgba([9, 9, 9, 255])));
        state.apply_decorrelation_stretch();
        assert!(std::sync::Arc::ptr_eq(&before_apply, &state.current_rgba));

        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
    }

    #[test]
    fn input_change_cancels_queued_stretch_and_invalidates_preview() {
        let jobs = little_clip_core::BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let mut state = test_state();
        state.update_decorrelation_stretch_on(&jobs);
        let cancel = state.dcs_state.worker_cancel.as_ref().unwrap().clone();
        let worker_rx = state.dcs_state.worker_rx.take().unwrap();
        state.current_rgba =
            std::sync::Arc::new(RgbaImage::from_pixel(20, 20, image::Rgba([3, 4, 5, 255])));
        state.mark_pixels_changed();
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        assert!(state.dcs_state.stretched_rgba.is_none());
        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        assert!(matches!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
    }

    #[test]
    fn dropping_viewer_cancels_queued_dcs_job() {
        let jobs = little_clip_core::BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_, blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let mut state = test_state();
        state.update_decorrelation_stretch_on(&jobs);
        let cancel = state.dcs_state.worker_cancel.as_ref().unwrap().clone();
        let worker_rx = state.dcs_state.worker_rx.take().unwrap();
        drop(state);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
        release_tx.send(()).unwrap();
        blocker_rx.recv().unwrap().unwrap();
        assert!(matches!(
            worker_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            Err(little_clip_core::JobError::Cancelled)
        ));
    }

    #[test]
    fn rendered_matrix_rows_keep_three_decimal_precision_inside_compact_and_wide_drawers() {
        for width in [360.0, 1200.0] {
            let mut png = Vec::new();
            RgbaImage::from_fn(16, 16, |x, y| {
                image::Rgba([
                    (x * 11 + y * 3) as u8,
                    (y * 13 + x * 5) as u8,
                    ((x + y) * 7) as u8,
                    255,
                ])
            })
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
            let mut state = ImagePreviewState::from_bytes_with_meta(
                png,
                "image/png".into(),
                None,
                "DCS matrix render test".into(),
                &little_clip_core::config::AppConfig::default(),
            )
            .unwrap();
            let jobs = little_clip_core::BackgroundJobs::new(1, 1);
            state.update_decorrelation_stretch_on(&jobs);
            wait_for_dcs(&mut state, &jobs);
            let stats = state.dcs_state.stats.as_ref().unwrap();
            let expected_rows: Vec<_> = stats
                .corr
                .map(|row| format!("[{:>6.3}  {:>6.3}  {:>6.3}]", row[0], row[1], row[2]))
                .into_iter()
                .chain(
                    stats
                        .cov
                        .map(|row| format!("[{:>7.1}  {:>7.1}  {:>7.1}]", row[0], row[1], row[2])),
                )
                .collect();

            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette = AppPalette::default();
            let mut drawer_rect = None;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 1000.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        drawer_rect = Some(ui.available_rect_before_wrap());
                        state.render_decorrelation_stretch_stats_drawer(ui, &palette);
                    });
                },
            );

            let drawer_rect = drawer_rect.expect("stats drawer receives a viewport");
            let rendered_rows: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text().starts_with('[') => Some((
                        text.galley.text().to_owned(),
                        Rect::from_min_size(text.pos, text.galley.size()),
                    )),
                    _ => None,
                })
                .collect();
            assert!(
                rendered_rows.len() >= 6,
                "drawer emits all six correlation and covariance rows"
            );
            for expected in expected_rows {
                let (_, bounds) = rendered_rows
                    .iter()
                    .find(|(rendered, _)| rendered == &expected)
                    .unwrap_or_else(|| panic!("drawer renders precise matrix row {expected}"));
                assert!(
                    drawer_rect.contains_rect(*bounds),
                    "matrix row {expected} at {bounds:?} must stay inside {drawer_rect:?} at width {width}"
                );
                assert_eq!(expected.matches('.').count(), 3);
            }
        }
    }
}
