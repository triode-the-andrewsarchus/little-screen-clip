use super::*;

impl DiffViewerState {
    /// Triggers launching Meld or KDiff3 for visual comparison.
    pub fn trigger_escape_hatch(&mut self, now: f64) {
        self.trigger_escape_hatch_on(
            now,
            BackgroundJobs::global(),
            |command| little_clip_core::command_available(command),
            |app, left, right| {
                std::process::Command::new(app)
                    .arg(left)
                    .arg(right)
                    .spawn()
                    .is_ok()
            },
        );
    }

    pub(super) fn cancel_handoff(&mut self) {
        if let Some((ticket, _)) = self.handoff.take() {
            ticket.cancel();
        }
        self.handoff_cancel.take();
    }

    pub(super) fn trigger_escape_hatch_on(
        &mut self,
        now: f64,
        jobs: &BackgroundJobs,
        available: impl FnMut(&str) -> Option<std::path::PathBuf> + Send + 'static,
        spawn: impl FnMut(&std::path::Path, &std::path::Path, &std::path::Path) -> bool + Send + 'static,
    ) {
        self.cancel_handoff();
        let left = DiffHandoffTarget::from_target(&self.lhs, "lhs");
        let right = DiffHandoffTarget::from_target(&self.rhs, "rhs");
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job_cancelled = cancelled.clone();
        let (ticket, receiver) = jobs.submit(std::time::Duration::from_secs(30), move || {
            run_diff_handoff_with(left, right, &job_cancelled, available, spawn)
        });
        self.status_toast = Some(("Preparing comparison for external tool…".into(), now));
        self.handoff = Some((ticket, receiver));
        self.handoff_cancel = Some(HandoffCancelGuard(cancelled));
    }

    pub(super) fn poll_handoff(&mut self, now: f64, ctx: &egui::Context) {
        let Some((_, rx)) = &self.handoff else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Ok(message))) => {
                self.status_toast = Some((message.into(), now));
                self.handoff = None;
            }
            Ok(Ok(Err(error))) => {
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(crate::icons::Icon::Close, error.to_string()),
                    now,
                ));
                self.handoff = None;
            }
            Ok(Err(error)) => {
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        format!("External handoff failed: {error}"),
                    ),
                    now,
                ));
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_toast = Some((
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        "External handoff was cancelled".to_string(),
                    ),
                    now,
                ));
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(80))
            }
        }
    }
}
pub(super) fn launch_diff_tool_with(
    left_path: &std::path::Path,
    right_path: &std::path::Path,
    cancelled: &std::sync::atomic::AtomicBool,
    mut available: impl FnMut(&str) -> Option<std::path::PathBuf>,
    mut spawn: impl FnMut(&std::path::Path, &std::path::Path, &std::path::Path) -> bool,
) -> Result<Option<String>, String> {
    let mut launch_failures = Vec::new();
    for (command, success_message) in [
        ("meld", "Launched Meld comparison"),
        ("kdiff3", "Launched KDiff3 comparison"),
    ] {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err("External handoff cancelled".to_string());
        }
        if let Some(app) = available(command) {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("External handoff cancelled".to_string());
            }
            if spawn(&app, left_path, right_path) {
                return Ok(Some(success_message.to_string()));
            }
            launch_failures.push(command);
        }
    }
    if launch_failures.is_empty() {
        Ok(None)
    } else {
        Err(format!(
            "Failed to launch available comparison tool(s): {}",
            launch_failures.join(", ")
        ))
    }
}

pub(super) fn run_diff_handoff_with(
    left: DiffHandoffTarget,
    right: DiffHandoffTarget,
    cancelled: &std::sync::atomic::AtomicBool,
    available: impl FnMut(&str) -> Option<std::path::PathBuf>,
    spawn: impl FnMut(&std::path::Path, &std::path::Path, &std::path::Path) -> bool,
) -> Result<String, String> {
    let mut left_path = left.stage()?;
    let mut right_path = right.stage()?;
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("External handoff cancelled".to_string());
    }
    match launch_diff_tool_with(
        &left_path.path,
        &right_path.path,
        cancelled,
        available,
        spawn,
    )? {
        Some(message) => {
            left_path.preserve_for_external_tool();
            right_path.preserve_for_external_tool();
            Ok(message)
        }
        None => Ok("Neither Meld nor KDiff3 is installed".to_string()),
    }
}
