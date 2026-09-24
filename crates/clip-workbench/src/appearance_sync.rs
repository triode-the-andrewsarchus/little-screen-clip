//! Completion and feedback for applying appearance changes to the daemon.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::palette::AppPalette;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};

use crate::send_ipc_request_with_timeout;
use crate::ui_appearance::WorkbenchColors;

type ReloadReceiver = Receiver<Result<Result<(), String>, JobError>>;

#[derive(Default)]
pub(crate) struct AppearanceSync {
    pending: Option<(JobTicket, ReloadReceiver)>,
    error: Option<String>,
}

impl AppearanceSync {
    pub(crate) fn clear(&mut self) {
        if let Some((ticket, _)) = self.pending.take() {
            ticket.cancel();
        }
        self.error = None;
    }

    pub(crate) fn begin(&mut self) {
        self.pending = Some(BackgroundJobs::global().submit(Duration::from_secs(5), || {
            send_ipc_request_with_timeout(&IpcRequest::ReloadConfig, Some(Duration::from_secs(4)))
                .map_err(|error| error.to_string())
                .and_then(reload_result)
        }));
    }

    pub(crate) fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.pending = None;
        self.error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!("Could not update little screen: {error}")),
            Err(error) => Some(format!("Could not update little screen: {error}")),
        };
        if self.error.is_some() {
            ctx.request_repaint();
        }
    }

    pub(crate) fn render(&mut self, ctx: &egui::Context, palette: &AppPalette) {
        let Some(error) = self.error.as_deref() else {
            return;
        };
        let colors = WorkbenchColors::from_palette(palette);
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("appearance_reload_error"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(colors.panel)
                    .stroke(egui::Stroke::new(1.0_f32, colors.danger))
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(error).color(colors.text_primary));
                            dismiss = ui.button("Dismiss").clicked();
                        });
                    });
            });
        if dismiss {
            self.error = None;
        }
    }
}

impl Drop for AppearanceSync {
    fn drop(&mut self) {
        if let Some((ticket, _)) = &self.pending {
            ticket.cancel();
        }
    }
}

fn reload_result(response: IpcResponse) -> Result<(), String> {
    match response {
        IpcResponse::Success { .. } => Ok(()),
        IpcResponse::Error { error } => Err(error),
        other => Err(format!("unexpected daemon response: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn daemon_error_reaches_visible_feedback_state() {
        let (sender, receiver) = mpsc::channel();
        let (ticket, _) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        let mut sync = AppearanceSync {
            pending: Some((ticket, receiver)),
            error: None,
        };
        sender.send(Ok(Err("invalid config".to_owned()))).unwrap();
        sync.poll(&egui::Context::default());
        assert_eq!(
            sync.error.as_deref(),
            Some("Could not update little screen: invalid config")
        );
        assert!(sync.pending.is_none());
    }

    #[test]
    fn daemon_reload_failure_is_not_treated_as_success() {
        assert!(
            reload_result(IpcResponse::Success {
                message: "applied".to_owned(),
            })
            .is_ok()
        );
        assert_eq!(
            reload_result(IpcResponse::Error {
                error: "invalid config".to_owned(),
            }),
            Err("invalid config".to_owned())
        );
        assert!(reload_result(IpcResponse::LockStatus(false)).is_err());
    }
}
