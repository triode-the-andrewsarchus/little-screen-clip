//! Accepted daemon mutations outlive view changes and retain their completion.

use super::*;

pub(crate) enum DaemonMutationKind {
    Swap,
    Pipeline,
    Action,
    AutoAction { enabled: bool },
}

pub(crate) struct DaemonMutation {
    kind: DaemonMutationKind,
    receiver: Receiver<Result<Result<IpcResponse, String>, JobError>>,
}

impl DaemonMutation {
    pub(crate) fn submit(request: IpcRequest, kind: DaemonMutationKind) -> Self {
        Self::submit_with(request, kind, BackgroundJobs::global(), |request| {
            send_ipc_request(&request).map_err(|error| error.to_string())
        })
    }

    fn submit_with(
        request: IpcRequest,
        kind: DaemonMutationKind,
        jobs: &BackgroundJobs,
        send: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) -> Self {
        // Accepted writes keep their outcome: never cancel or expire after execution.
        let (_, receiver) = jobs.submit(Duration::MAX, move || send(request));
        Self { kind, receiver }
    }

    pub(crate) fn poll(&self, ctx: &egui::Context) -> Option<IpcResponse> {
        let result = match self.receiver.try_recv() {
            Ok(result) => result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.request_repaint_after(Duration::from_millis(50));
                return None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Daemon worker disconnected; operation outcome is unknown".to_owned())
            }
        };
        Some(result.unwrap_or_else(|error| IpcResponse::Error { error }))
    }
}

pub(crate) fn action_result(response: IpcResponse) -> Result<(), String> {
    match response {
        IpcResponse::OpResult { .. } | IpcResponse::Success { .. } => Ok(()),
        IpcResponse::Error { error } => Err(error),
        _ => Err("unexpected response from daemon".to_owned()),
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
