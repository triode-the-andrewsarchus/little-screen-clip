//! Unified external application escape hatch launcher with automatic fallback.
//!
//! Handles spawning external tools (e.g. Krita, Audacity, Inkscape, LibreOffice Calc,
//! Okular, MuseScore, Blender, Meld, ImHex) non-destructively, falling back to `xdg-open`
//! when a configured application executable is not present on the system.

use crate::icons::Icon;
use crate::ui_ext::StatusToast;
use std::path::Path;
use std::process::Command;

/// Configuration for an external application escape hatch button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscapeHatch {
    pub icon: Option<Icon>,
    pub label: String,
    pub command: String,
    pub tooltip: String,
    pub is_primary: bool,
}

impl EscapeHatch {
    pub fn new(
        label: impl Into<String>,
        command: impl Into<String>,
        tooltip: impl Into<String>,
    ) -> Self {
        Self {
            icon: None,
            label: label.into(),
            command: command.into(),
            tooltip: tooltip.into(),
            is_primary: false,
        }
    }

    pub fn primary(
        label: impl Into<String>,
        command: impl Into<String>,
        tooltip: impl Into<String>,
    ) -> Self {
        Self {
            icon: None,
            label: label.into(),
            command: command.into(),
            tooltip: tooltip.into(),
            is_primary: true,
        }
    }

    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_primary(mut self, is_primary: bool) -> Self {
        self.is_primary = is_primary;
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

/// Non-destructive external application launcher.
pub struct EscapeHatchLauncher;

impl EscapeHatchLauncher {
    /// Launches an external tool with the given target file path, falling back to `xdg-open`.
    pub fn spawn_with_fallback(
        app_name: &str,
        command: &str,
        file_path: &Path,
    ) -> Result<String, String> {
        Self::spawn_with_fallback_command(app_name, command, file_path, "xdg-open")
    }

    pub(crate) fn spawn_with_fallback_command(
        app_name: &str,
        command: &str,
        file_path: &Path,
        fallback_command: &str,
    ) -> Result<String, String> {
        let mut cmd = Command::new(command);
        cmd.arg(file_path);

        match cmd.spawn() {
            Ok(_) => {
                let file_name = file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                Ok(format!("Launched {} with {}", app_name, file_name))
            }
            Err(_) => {
                // Try xdg-open fallback
                if Command::new(fallback_command)
                    .arg(file_path)
                    .spawn()
                    .is_ok()
                {
                    Ok("Opened in default application via xdg-open".to_string())
                } else {
                    Err(format!(
                        "Failed to launch '{}' (tool not installed)",
                        command
                    ))
                }
            }
        }
    }

    /// Launches an external tool with the given target file path.
    ///
    /// 1. Tries spawning the specified `command` with `file_path`.
    /// 2. If launching fails or the executable is missing, attempts `xdg-open` as a fallback.
    /// 3. Emits a user-facing toast notification via `toast`.
    /// 4. Does NOT terminate the workbench window or send `ViewportCommand::Close`.
    pub fn launch(
        app_name: &str,
        command: &str,
        file_path: &Path,
        toast: &mut StatusToast,
        now: f64,
    ) -> bool {
        match Self::spawn_with_fallback(app_name, command, file_path) {
            Ok(msg) => {
                toast.set(
                    crate::ui_ext::StatusMessage::new(Icon::ArrowRight, msg),
                    now,
                );
                true
            }
            Err(msg) => {
                toast.set(crate::ui_ext::StatusMessage::new(Icon::Alert, msg), now);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_hatch_builder() {
        let h = EscapeHatch::new("Krita", "krita", "Open in Krita").with_icon(Icon::Palette);
        assert_eq!(h.label, "Krita");
        assert_eq!(h.icon, Some(Icon::Palette));
        assert_eq!(h.command, "krita");
        assert!(!h.is_primary);

        let p = EscapeHatch::primary("Audacity", "audacity", "Open in Audacity")
            .with_icon(Icon::Volume);
        assert!(p.is_primary);
        assert_eq!(p.icon, Some(Icon::Volume));
    }

    #[test]
    fn configured_application_spawn_returns_launch_message() {
        let result = EscapeHatchLauncher::spawn_with_fallback_command(
            "Test Viewer",
            "/bin/true",
            Path::new("/tmp/sample.test"),
            "/bin/false",
        );

        assert_eq!(result, Ok("Launched Test Viewer with sample.test".into()));
    }

    #[test]
    fn fallback_spawn_returns_default_application_message() {
        let result = EscapeHatchLauncher::spawn_with_fallback_command(
            "Test Viewer",
            "/missing/test-viewer-executable",
            Path::new("/tmp/sample.test"),
            "/bin/true",
        );

        assert_eq!(
            result,
            Ok("Opened in default application via xdg-open".into())
        );
    }

    #[test]
    fn failed_configured_and_fallback_spawns_return_failure_message() {
        let result = EscapeHatchLauncher::spawn_with_fallback_command(
            "Test Viewer",
            "/missing/test-viewer-executable",
            Path::new("/tmp/sample.test"),
            "/missing/test-fallback-executable",
        );

        assert_eq!(
            result,
            Err("Failed to launch '/missing/test-viewer-executable' (tool not installed)".into())
        );
    }
}
