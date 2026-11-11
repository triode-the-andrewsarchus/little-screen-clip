use super::*;
use clip_ui::buttons::ButtonRole;

impl WorkbenchApp {
    pub(crate) fn apply_rolling_audio_action(
        &mut self,
        action: RollingAudioAction,
        ctx: &egui::Context,
    ) {
        match action {
            RollingAudioAction::OpenEditor {
                source,
                snapshot_lease,
                duration_sec,
                selected_region,
            } => {
                if self.rolling_editor_job.is_some() {
                    self.set_status_message(
                        "A rolling audio editor handoff is already in progress",
                    );
                    return;
                }
                let title = if selected_region.is_some() {
                    "Rolling Audio Selection"
                } else {
                    "Rolling Audio Snapshot"
                }
                .to_owned();
                let task = little_clip_core::jobs::task::Task::start(
                    BackgroundJobs::global(),
                    Duration::from_secs(60),
                    move |cancel| {
                        let _snapshot_lease = snapshot_lease;
                        match selected_region {
                            Some((start, end)) => {
                                RollingAudioStripState::materialize_editor_selection(
                                    &source, start, end, cancel,
                                )
                            }
                            None => crate::audio_preview::prepare_audio_slice(
                                &source,
                                0.0,
                                duration_sec,
                                false,
                                cancel,
                            ),
                        }
                    },
                )
                .with_response_deadline(Duration::from_secs(60));
                self.rolling_editor_job = Some(RollingEditorLoadJob {
                    task,
                    editor: self.config.audio.editor.clone(),
                    title,
                    ready: None,
                });
                self.set_status_message("Preparing rolling audio for the editor…");
                ctx.request_repaint();
            }
        }
    }

    pub(crate) fn poll_rolling_editor_load(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.rolling_editor_job else {
            return;
        };
        if job.ready.is_none() {
            match job.task.poll() {
                Some(result) => {
                    job.ready = Some(
                        result
                            .map_err(|error| error.to_string())
                            .and_then(|result| result),
                    )
                }
                None => {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return;
                }
            }
        }

        if self.viewer.is_some()
            || !self.startup_viewers.is_empty()
            || !self.startup_file_loads.is_empty()
            || self.image_load_job.is_some()
            || self.text_load_job.is_some()
            || self.office_load_job.is_some()
            || self.spreadsheet_load_job.is_some()
            || self.midi_load_job.is_some()
        {
            let message = match job.ready.as_ref() {
                Some(Err(error)) => format!("Could not open rolling audio selection: {error}"),
                _ => "Rolling audio is ready; waiting for the active viewer to close".into(),
            };
            self.set_status_message(message);
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        }
        let Some(mut job) = self.rolling_editor_job.take() else {
            return;
        };
        match job.ready.take().expect("ready result checked above") {
            Ok(prepared) => {
                let editor = prepared.into_preview(job.title).with_editor(job.editor);
                self.status_msg = None;
                self.replace_viewer(Some(ViewerSession::Audio(editor)));
            }
            Err(error) => {
                self.set_status_message(format!("Could not open rolling audio selection: {error}"))
            }
        }
        ctx.request_repaint();
    }

    pub(crate) fn render_rolling_editor_status(&mut self, ctx: &egui::Context) {
        let active = self.rolling_editor_job.is_some();
        let pending = self
            .rolling_editor_job
            .as_ref()
            .is_some_and(|job| job.ready.is_none());
        let message = if let Some(job) = &self.rolling_editor_job {
            match job.ready.as_ref() {
                Some(Err(error)) => format!("Could not open rolling audio selection: {error}"),
                Some(Ok(_)) => {
                    "Rolling audio is ready; waiting for the active viewer to close".into()
                }
                None => "Preparing rolling audio for the editor…".into(),
            }
        } else {
            match self.status_msg.as_deref() {
                Some(message) if message.starts_with("Could not open rolling audio selection:") => {
                    message.to_owned()
                }
                _ => return,
            }
        };
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("rolling_audio_editor_handoff"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if pending {
                            ui.spinner();
                        }
                        ui.label(&message);
                        if active {
                            if ui
                                .add(ButtonRole::Secondary.button("Cancel", &self.ui_palette))
                                .clicked()
                            {
                                self.rolling_editor_job = None;
                                self.status_msg = None;
                            }
                        } else {
                            dismiss = ui
                                .add(ButtonRole::Secondary.button("Dismiss", &self.ui_palette))
                                .clicked();
                        }
                    });
                });
            });
        if dismiss {
            self.status_msg = None;
        }
        if active {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }
}
