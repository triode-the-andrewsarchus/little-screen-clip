use super::*;

impl WorkbenchApp {
    pub(crate) fn apply_rolling_audio_action(
        &mut self,
        action: RollingAudioAction,
        ctx: &egui::Context,
    ) {
        match action {
            RollingAudioAction::OpenEditor {
                snapshot_path,
                selected_region,
            } => {
                let Some((start, end)) = selected_region else {
                    let editor =
                        AudioPreviewState::new(snapshot_path, "Rolling Audio Snapshot".to_owned())
                            .with_editor(self.config.audio.editor.clone());
                    self.replace_viewer(Some(ViewerSession::Audio(editor)));
                    return;
                };
                if self.rolling_editor_job.is_some() {
                    self.set_status_message(
                        "A rolling audio editor handoff is already in progress",
                    );
                    return;
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                let worker_cancelled = Arc::clone(&cancelled);
                let source = snapshot_path;
                let (ticket, receiver) =
                    BackgroundJobs::global().submit(Duration::MAX, move || {
                        RollingAudioStripState::materialize_editor_selection(
                            &source,
                            start,
                            end,
                            &worker_cancelled,
                        )
                        .map(|path| {
                            let metadata = little_clip_core::audio::read_audio_metadata(&path);
                            (path, metadata)
                        })
                    });
                self.rolling_editor_job = Some(RollingEditorLoadJob {
                    ticket,
                    cancelled,
                    receiver,
                    editor: self.config.audio.editor.clone(),
                    ready: None,
                });
                self.set_status_message("Preparing selected rolling audio for the editor…");
                ctx.request_repaint();
            }
        }
    }

    pub(crate) fn poll_rolling_editor_load(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.rolling_editor_job else {
            return;
        };
        if job.ready.is_none() {
            match job.receiver.try_recv() {
                Ok(Ok(Ok(result))) => job.ready = Some(Ok(result)),
                Ok(Ok(Err(error))) => job.ready = Some(Err(error)),
                Ok(Err(error)) => job.ready = Some(Err(error.to_string())),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    job.ready = Some(Err("background job was cancelled".to_owned()));
                }
            }
        }
        if self.viewer.is_some() || !self.startup_viewers.is_empty() {
            self.set_status_message(
                "Selected rolling audio is ready; waiting for the active viewer to close",
            );
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        }
        let Some(mut job) = self.rolling_editor_job.take() else {
            return;
        };
        match job.ready.take().expect("ready result checked above") {
            Ok((path, metadata)) => {
                let editor = AudioPreviewState::new_with_metadata(
                    path,
                    "Rolling Audio Selection".to_owned(),
                    metadata,
                )
                .with_editor(job.editor.clone());
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
        let message = if active {
            self.rolling_editor_job
                .as_ref()
                .filter(|job| job.ready.is_some())
                .map(|_| "Selected audio is ready; waiting for the active viewer to close")
                .unwrap_or("Preparing selected rolling audio for the editor…")
                .to_owned()
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
                        if active {
                            ui.spinner();
                        }
                        ui.label(&message);
                        if active {
                            if ui.button("Cancel").clicked() {
                                self.rolling_editor_job = None;
                                self.status_msg = None;
                            }
                        } else {
                            dismiss = ui.button("Dismiss").clicked();
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
