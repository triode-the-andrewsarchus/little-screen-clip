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
        let Some(result) = self.request.try_result() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.request_repaint_after(Duration::from_millis(50));
            return None;
        };
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
            DaemonMutationKind::Pipeline => self.canvas_state.apply_pipeline_response(response),
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
}
