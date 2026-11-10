//! Completion and feedback for applying appearance changes to the daemon.

use std::time::Duration;

use clip_ui::buttons::ButtonRole;
use eframe::egui;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::palette::AppPalette;

use crate::desktop_ipc::PendingRequest;
use crate::ui_appearance::WorkbenchColors;

#[derive(Default)]
pub(crate) struct AppearanceSync {
    pending: Option<PendingRequest>,
    reload_requested: bool,
    local_error: Option<String>,
    error: Option<String>,
}

impl AppearanceSync {
    pub(crate) fn clear(&mut self) {
        self.error = None;
        self.local_error = None;
    }

    pub(crate) fn begin(&mut self) {
        if self.pending.is_some() {
            self.reload_requested = true;
        } else {
            self.pending = Some(PendingRequest::submit(IpcRequest::ReloadConfig));
        }
    }

    pub(crate) fn set_error(&mut self, error: String) {
        self.local_error = Some(error);
    }

    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        self.poll_with(ctx, || PendingRequest::submit(IpcRequest::ReloadConfig));
    }

    fn poll_with(&mut self, ctx: &egui::Context, next: impl FnOnce() -> PendingRequest) {
        let Some(job) = &self.pending else {
            return;
        };
        let Some(result) = job.try_result() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        };
        self.pending = None;
        self.error = result
            .and_then(reload_result)
            .err()
            .map(|error| format!("Could not update little screen: {error}"));
        if std::mem::take(&mut self.reload_requested) {
            self.pending = Some(next());
        }
        ctx.request_repaint();
    }

    pub(crate) fn render(&mut self, ctx: &egui::Context, palette: &AppPalette) {
        let Some(error) = self.local_error.as_deref().or(self.error.as_deref()) else {
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
                    .inner_margin(egui::Margin::symmetric(
                        crate::ui_ext::SPACE_MD,
                        crate::ui_ext::SPACE_SM,
                    ))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(error).color(colors.text_primary));
                            dismiss = ui
                                .add(ButtonRole::Secondary.button("Dismiss", palette))
                                .clicked();
                        });
                    });
            });
        if dismiss {
            self.clear();
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
    fn reloads_coalesce_without_discarding_completion_or_local_save_errors() {
        use little_clip_core::BackgroundJobs;
        let jobs = BackgroundJobs::new(1, 2);
        let (release, wait) = mpsc::channel();
        let mut sync = AppearanceSync {
            pending: Some(PendingRequest::start(&jobs, move || {
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(IpcResponse::Error {
                    error: "invalid config".into(),
                })
            })),
            ..Default::default()
        };
        sync.clear();
        sync.begin();
        sync.begin();
        assert!(sync.reload_requested);
        sync.set_error("Could not save newer appearance".into());
        release.send(()).unwrap();
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut followups = 0;
        while sync.pending.is_some() {
            sync.poll_with(&ctx, || {
                followups += 1;
                PendingRequest::start(&jobs, || {
                    Ok(IpcResponse::Success {
                        message: "reloaded".into(),
                    })
                })
            });
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(followups, 1);
        assert!(sync.error.is_none());
        assert_eq!(
            sync.local_error.as_deref(),
            Some("Could not save newer appearance")
        );
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
