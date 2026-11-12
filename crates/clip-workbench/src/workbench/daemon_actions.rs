//! Accepted daemon mutations outlive view changes and retain their completion.

use super::*;
use crate::desktop_ipc::PendingRequest;
pub(crate) use crate::desktop_ipc::action_result;

pub(crate) enum DaemonMutationKind {
    Swap,
    Pipeline,
    Action,
    AutoAction { enabled: bool },
}

pub(crate) struct DaemonMutation {
    kind: DaemonMutationKind,
    request: PendingRequest,
}

impl DaemonMutation {
    pub(crate) fn submit(request: IpcRequest, kind: DaemonMutationKind) -> Self {
        Self {
            kind,
            request: PendingRequest::submit(request),
        }
    }

    #[cfg(test)]
    fn submit_with(
        request: IpcRequest,
        kind: DaemonMutationKind,
        jobs: &BackgroundJobs,
        send: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) -> Self {
        Self {
            kind,
            request: PendingRequest::submit_with(request, jobs, send),
        }
    }

    pub(crate) fn poll(&self, ctx: &egui::Context) -> Option<IpcResponse> {
        let result = self.request.poll(ctx)?;
        Some(result.unwrap_or_else(|error| IpcResponse::Error { error }))
    }
}

impl WorkbenchApp {
    pub(crate) fn submit_daemon_mutation(&mut self, request: IpcRequest, kind: DaemonMutationKind) {
        if self.daemon_mutation.is_none() {
            self.daemon_mutation = Some(DaemonMutation::submit(request, kind));
        }
    }

    #[cfg(test)]
    pub(crate) fn submit_daemon_mutation_with(
        &mut self,
        request: IpcRequest,
        kind: DaemonMutationKind,
        jobs: &BackgroundJobs,
        send: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) {
        if self.daemon_mutation.is_none() {
            self.daemon_mutation = Some(DaemonMutation::submit_with(request, kind, jobs, send));
        }
    }

    pub(crate) fn poll_daemon_mutation(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.daemon_mutation else {
            return;
        };
        let Some(response) = job.poll(ctx) else {
            return;
        };
        let job = self.daemon_mutation.take().unwrap();
        match job.kind {
            DaemonMutationKind::Action => {
                self.apply_action_response(ctx, Ok(response), true);
            }
            DaemonMutationKind::AutoAction { enabled } => {
                if self.apply_action_response(ctx, Ok(response), false) {
                    self.config.auto_actions.trim_link_tracking = enabled;
                }
            }
            DaemonMutationKind::Pipeline => {
                self.canvas_state.apply_pipeline_response(response);
                let studio_targets = std::mem::take(&mut self.canvas_state.pending_open_studios);
                let external_targets =
                    std::mem::take(&mut self.canvas_state.pending_external_opens);
                for target in studio_targets {
                    self.open_pipeline_studio_target(target);
                }
                for target in external_targets {
                    if let Err(err) = launch_external_open_target(&target) {
                        self.set_status_message(err);
                    }
                }
            }
            DaemonMutationKind::Swap => match response {
                IpcResponse::Success { .. } => {
                    // The state poller reconciles slots. Never apply a speculative
                    // local swap against a possibly newer clipboard snapshot.
                    self.dirty_mirror = true;
                }
                IpcResponse::Error { error } => self.set_status_message(error),
                _ => self.set_status_message("Unexpected response to slot swap"),
            },
        }
        ctx.request_repaint();
    }

    fn open_pipeline_studio_target(&mut self, target: little_clip_core::ipc::StudioOpenTarget) {
        use little_clip_core::ipc::StudioTarget;
        match target.studio {
            StudioTarget::Audio => {
                if let Some(path) = target.path {
                    self.replace_viewer(Some(ViewerSession::Audio(Box::new(
                        AudioPreviewState::new(path, target.title)
                            .with_editor(self.config.audio.editor.clone()),
                    ))));
                }
            }
            StudioTarget::Image => {
                if let Some(path) = target.path {
                    self.start_image_load(ImageLoadSource::File(path));
                }
            }
            StudioTarget::Text => {
                if let Some(path) = target.path {
                    self.start_text_file_load(path);
                } else if let Some(text) = target.text {
                    self.replace_viewer(Some(ViewerSession::Text(TextPreviewState::from_text(
                        text,
                        Some(target.title),
                        &self.config,
                    ))));
                }
            }
            StudioTarget::Spreadsheet => {
                if let Some(path) = target.path {
                    self.start_spreadsheet_load(SpreadsheetLoadSource::File(path));
                } else if let Some(text) = target.text {
                    self.start_spreadsheet_load(SpreadsheetLoadSource::Text {
                        content: Arc::new(text),
                        title: target.title,
                    });
                }
            }
            StudioTarget::Midi => {
                if let Some(path) = target.path {
                    self.start_midi_load(path);
                }
            }
            StudioTarget::Pdf => {
                if let Some(path) = target.path {
                    self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(path))));
                }
            }
            StudioTarget::Model3d => {
                if let Some(path) = target.path {
                    self.replace_viewer(Some(scene_file_viewer(path, &self.config)));
                }
            }
        }
    }
}

fn launch_external_open_target(
    target: &little_clip_core::ipc::ExternalOpenTarget,
) -> Result<(), String> {
    let program = target.program.trim();
    let mut cmd = if program.is_empty() {
        #[cfg(target_os = "macos")]
        let c = std::process::Command::new("open");
        #[cfg(target_os = "windows")]
        let c = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", ""]);
            c
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let c = std::process::Command::new("xdg-open");
        c
    } else {
        let mut c = std::process::Command::new(program);
        c.args(&target.pre_args);
        c
    };
    cmd.arg(&target.file_path);
    if !program.is_empty() {
        cmd.args(&target.post_args);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| {
        let label = if program.is_empty() {
            "default handler"
        } else {
            program
        };
        format!("Failed to launch {label}: {e}")
    })
}
