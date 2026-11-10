//! Accepted daemon mutations outlive view changes and retain their completion.

use super::*;

pub(crate) enum DaemonMutationKind {
    Swap,
    Pipeline,
}

pub(crate) struct DaemonMutation {
    kind: DaemonMutationKind,
    receiver: Receiver<Result<Result<IpcResponse, String>, JobError>>,
}

impl WorkbenchApp {
    pub(crate) fn submit_daemon_mutation(&mut self, request: IpcRequest, kind: DaemonMutationKind) {
        self.submit_daemon_mutation_with(request, kind, BackgroundJobs::global(), |request| {
            send_ipc_request(&request).map_err(|error| error.to_string())
        });
    }

    pub(crate) fn submit_daemon_mutation_with(
        &mut self,
        request: IpcRequest,
        kind: DaemonMutationKind,
        jobs: &BackgroundJobs,
        send: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) {
        if self.daemon_mutation.is_some() {
            return;
        }
        // Job admission is bounded; once submitted, neither navigation nor a
        // post-operation deadline may discard the mutation's completion.
        let (_, receiver) = jobs.submit(Duration::MAX, move || send(request));
        self.daemon_mutation = Some(DaemonMutation { kind, receiver });
    }

    pub(crate) fn poll_daemon_mutation(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.daemon_mutation else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.request_repaint_after(Duration::from_millis(50));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Daemon worker disconnected; operation outcome is unknown".to_owned())
            }
        };
        let job = self.daemon_mutation.take().unwrap();
        let response = result.unwrap_or_else(|error| IpcResponse::Error { error });
        match job.kind {
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
