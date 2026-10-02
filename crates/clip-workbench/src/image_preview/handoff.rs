use super::*;

impl ImagePreviewState {
    /// Ensure the file exists on disk so external programs can open it.
    /// If image was modified, saves modified buffer to disk.
    pub fn ensure_disk_path(&self) -> Result<PathBuf, String> {
        let staging_dir = little_clip_core::staging::get_staging_dir()
            .map_err(|e| format!("Could not create image staging directory: {e}"))?;

        let is_modified = !self.undo_stack.is_empty();
        if !is_modified {
            if let Some(ref p) = self.path {
                if p.exists() {
                    return Ok(p.clone());
                }
            }
        }

        let ext = if self.is_svg && !is_modified {
            "svg"
        } else if self.mime.contains("jpeg") || self.mime.contains("jpg") {
            "jpg"
        } else if self.mime.contains("webp") {
            "webp"
        } else {
            "png"
        };

        let staging_path = staging_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
        if is_modified || self.raw_bytes.is_empty() {
            self.current_rgba
                .save(&staging_path)
                .map_err(|e| format!("Failed to save modified staging file: {}", e))?;
        } else {
            std::fs::write(&staging_path, self.raw_bytes.as_slice())
                .map_err(|e| format!("Failed to write staging file: {}", e))?;
        }

        Ok(staging_path)
    }

    pub fn launch_external_app(&mut self, app: &str, _ctx: &egui::Context, _close: &mut bool) {
        self.launch_external_app_with(app, |app_name, command, path| {
            crate::shell::EscapeHatchLauncher::spawn_with_fallback(app_name, command, path)
        });
    }

    pub(super) fn launch_external_app_with(
        &mut self,
        app: &str,
        launch: impl FnOnce(&str, &str, &std::path::Path) -> Result<String, String>,
    ) {
        let file_path = self.ensure_disk_path();
        self.launch_external_app_with_path(app, file_path, launch);
    }

    pub(super) fn launch_external_app_with_path(
        &mut self,
        app: &str,
        file_path: Result<PathBuf, String>,
        launch: impl FnOnce(&str, &str, &std::path::Path) -> Result<String, String>,
    ) {
        match file_path {
            Ok(file_path) => {
                match launch(app, app, &file_path) {
                    Ok(msg) => {
                        self.show_toast(crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::ArrowRight,
                            msg,
                        ));
                        // Workbench remains open so the user does not lose their session
                    }
                    Err(e) => {
                        self.show_toast(e);
                    }
                }
            }
            Err(e) => {
                self.show_toast(crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    format!("File error: {}", e),
                ));
            }
        }
    }
}
