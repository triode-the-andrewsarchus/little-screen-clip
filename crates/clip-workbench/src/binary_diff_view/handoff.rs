use super::*;

pub(super) struct HandoffCancelGuard(pub(super) std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for HandoffCancelGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

pub(super) struct StagedBinaryFile {
    path: std::path::PathBuf,
    remove_on_drop: bool,
}

impl StagedBinaryFile {
    fn preserve_for_external_tool(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for StagedBinaryFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BinaryToolLaunch {
    ImHex,
    XdgOpen,
    Unavailable,
}

pub(super) fn stage_binary_handoff(bytes: &[u8], side: &str) -> Result<std::path::PathBuf, String> {
    let path = std::env::temp_dir().join(format!(
        "clip_bin_{}_{}.bin",
        uuid::Uuid::new_v4().simple(),
        side
    ));
    stage_binary_handoff_at(bytes, side, path)
}

pub(super) fn stage_binary_handoff_at(
    bytes: &[u8],
    side: &str,
    path: std::path::PathBuf,
) -> Result<std::path::PathBuf, String> {
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Failed to write {side} temp file: {error}"))?;
    Ok(path)
}

pub(super) fn run_binary_handoff_with(
    left: &[u8],
    right: &[u8],
    cancelled: &std::sync::atomic::AtomicBool,
    mut stage: impl FnMut(&[u8], &str) -> Result<std::path::PathBuf, String>,
    available: impl FnMut(&str) -> Option<std::path::PathBuf>,
    spawn: impl FnMut(&std::path::Path, &std::path::Path, Option<&std::path::Path>) -> bool,
) -> Result<String, String> {
    let mut left_path = StagedBinaryFile {
        path: stage(left, "lhs")?,
        remove_on_drop: true,
    };
    let mut right_path = StagedBinaryFile {
        path: stage(right, "rhs")?,
        remove_on_drop: true,
    };
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("External handoff cancelled".to_string());
    }
    match launch_binary_diff_tool_with(
        &left_path.path,
        &right_path.path,
        cancelled,
        available,
        spawn,
    )? {
        BinaryToolLaunch::ImHex => {
            left_path.preserve_for_external_tool();
            right_path.preserve_for_external_tool();
            Ok("Launched ImHex hex editor".to_string())
        }
        BinaryToolLaunch::XdgOpen => {
            left_path.preserve_for_external_tool();
            Ok("Opened left binary file with system default".to_string())
        }
        BinaryToolLaunch::Unavailable => Ok("Neither ImHex nor xdg-open is installed".to_string()),
    }
}

pub(super) fn launch_binary_diff_tool_with(
    left_path: &std::path::Path,
    right_path: &std::path::Path,
    cancelled: &std::sync::atomic::AtomicBool,
    mut available: impl FnMut(&str) -> Option<std::path::PathBuf>,
    mut spawn: impl FnMut(&std::path::Path, &std::path::Path, Option<&std::path::Path>) -> bool,
) -> Result<BinaryToolLaunch, String> {
    let mut launch_failures = Vec::new();
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("External handoff cancelled".to_string());
    }
    if let Some(app) = available("imhex") {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err("External handoff cancelled".to_string());
        }
        if spawn(&app, left_path, Some(right_path)) {
            return Ok(BinaryToolLaunch::ImHex);
        }
        launch_failures.push("imhex");
    }
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("External handoff cancelled".to_string());
    }
    if let Some(app) = available("xdg-open") {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err("External handoff cancelled".to_string());
        }
        if spawn(&app, left_path, None) {
            return Ok(BinaryToolLaunch::XdgOpen);
        }
        launch_failures.push("xdg-open");
    }
    if launch_failures.is_empty() {
        Ok(BinaryToolLaunch::Unavailable)
    } else {
        Err(format!(
            "Failed to launch available external tool(s): {}",
            launch_failures.join(", ")
        ))
    }
}

/// Color highlighting mode for hexadecimal byte displays.
impl BinaryDiffViewState {
    /// Triggers opening the binary comparison in ImHex hex editor.
    pub fn trigger_escape_hatch(&mut self, now: f64) {
        self.trigger_escape_hatch_on(
            now,
            BackgroundJobs::global(),
            stage_binary_handoff,
            |command| little_clip_core::command_available(command),
            |app, left, right| {
                let mut command = std::process::Command::new(app);
                command.arg(left);
                if let Some(right) = right {
                    command.arg(right);
                }
                command.spawn().is_ok()
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
        stage: impl FnMut(&[u8], &str) -> Result<std::path::PathBuf, String> + Send + 'static,
        available: impl FnMut(&str) -> Option<std::path::PathBuf> + Send + 'static,
        spawn: impl FnMut(&std::path::Path, &std::path::Path, Option<&std::path::Path>) -> bool
        + Send
        + 'static,
    ) {
        self.cancel_handoff();
        let left = self.left_bytes.clone();
        let right = self.right_bytes.clone();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job_cancelled = cancelled.clone();
        let (ticket, rx) = jobs.submit(std::time::Duration::from_secs(30), move || {
            run_binary_handoff_with(
                left.as_slice(),
                right.as_slice(),
                &job_cancelled,
                stage,
                available,
                spawn,
            )
        });
        self.status_toast
            .set("Preparing binary files for external tool…", now);
        self.handoff = Some((ticket, rx));
        self.handoff_cancel = Some(HandoffCancelGuard(cancelled));
    }

    pub(super) fn poll_handoff(&mut self, now: f64, ctx: &egui::Context) {
        let Some((_, rx)) = &self.handoff else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(Ok(message))) => {
                self.status_toast.set(message, now);
                self.handoff = None;
            }
            Ok(Ok(Err(error))) => {
                self.status_toast.set(
                    crate::ui_ext::StatusMessage::new(crate::icons::Icon::Close, error.to_string()),
                    now,
                );
                self.handoff = None;
            }
            Ok(Err(error)) => {
                self.status_toast.set(
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        format!("External handoff failed: {error}"),
                    ),
                    now,
                );
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.status_toast.set(
                    crate::ui_ext::StatusMessage::new(
                        crate::icons::Icon::Close,
                        "External handoff was cancelled",
                    ),
                    now,
                );
                self.handoff = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(80))
            }
        }
    }
}
