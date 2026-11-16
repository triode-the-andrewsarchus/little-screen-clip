//! Drive dialogs keep destination selection and conflict review independent of menus.
use super::*;
impl Dialog {
    fn title(&self) -> &str {
        match self {
            Self::Path { title, .. } => title,
            Self::Discard(_) => "Discard RAM copy",
            Self::Deletions { .. } => "Deletion propagation",
            Self::Policy { .. } => "Catalog and indexing policy",
            Self::Details => "Drive operations and conflicts",
        }
    }
    fn show(
        &mut self,
        ui: &mut Ui,
        current: &std::path::Path,
        status: &DriveStatus,
        requests: &mut Vec<DriveRequest>,
    ) -> bool {
        match self {
            Self::Path {
                text,
                action,
                error,
                ..
            } => path_dialog(ui, current, text, action, error, requests),
            Self::Discard(session) => {
                ui.label("This removes the RAM copy, including any edits not yet written to the drive. Save it elsewhere first if you need those edits.");
                let confirmed = ui.button("Discard RAM copy").clicked();
                if confirmed {
                    requests.push(DriveRequest::DiscardRam {
                        session: session.clone(),
                        confirmed: true,
                    });
                }
                !confirmed
            }
            Self::Deletions { pair, enabled } => {
                ui.label(if *enabled { "After a successful initial merge, confirmed deletions will propagate to the other folder. Missing media and failed scans never count as deletions." } else { "Missing files will again be restored from the other folder." });
                let confirmed = ui.button("Apply deletion policy").clicked();
                if confirmed {
                    requests.push(DriveRequest::Deletions {
                        pair: pair.clone(),
                        enabled: *enabled,
                    });
                }
                !confirmed
            }
            Self::Policy {
                drive,
                relative,
                catalog,
                index,
            } => {
                policy_dialog(ui, status, relative, catalog, index);
                let confirmed = ui.button("Save policy").clicked();
                if confirmed {
                    requests.push(DriveRequest::Rule {
                        drive: drive.clone(),
                        relative: relative.clone(),
                        catalog: *catalog,
                        index: *index,
                    });
                }
                !confirmed
            }
            Self::Details => {
                details(ui, status, requests);
                true
            }
        }
    }
}
impl PathAction {
    fn description(&self) -> &'static str {
        match self {
            Self::Directory => {
                "Choose the directory for future backups. This does not start a backup."
            }
            Self::Backup {
                kind: BackupKind::Image,
                ..
            } => {
                "Choose a directory for versioned partition images. The partition will be unmounted before imaging."
            }
            Self::Backup { .. } => {
                "Choose a directory for versioned backups. Each partition gets its own UUID folder."
            }
            Self::Pair { .. } => {
                "Both folders will initially receive all files. Concurrent edits are preserved as portable/local variants. Deletions start disabled."
            }
            Self::SaveRam { .. } => "A new folder will be created here containing the RAM copy.",
        }
    }
    fn requests(&self, path: PathBuf) -> Vec<DriveRequest> {
        match self {
            Self::Directory => vec![DriveRequest::BackupDirectory { path }],
            Self::Backup { drive, kind } => vec![
                DriveRequest::BackupDirectory { path },
                DriveRequest::Backup {
                    drive: drive.clone(),
                    kind: *kind,
                },
            ],
            Self::Pair { drive, relative } => vec![DriveRequest::Pair {
                drive: drive.clone(),
                relative: relative.clone(),
                local: path,
            }],
            Self::SaveRam { session } => vec![DriveRequest::SaveRam {
                session: session.clone(),
                destination: path,
            }],
        }
    }
}
fn path_dialog(
    ui: &mut Ui,
    current: &std::path::Path,
    text: &mut String,
    action: &PathAction,
    error: &mut Option<String>,
    requests: &mut Vec<DriveRequest>,
) -> bool {
    ui.label(action.description());
    ui.text_edit_singleline(text);
    if ui.button("Use current folder").clicked() {
        *text = display_path(current);
        *error = None;
    }
    if ui.button("Continue").clicked() {
        match little_clip_core::path_address::decode_path(text) {
            Ok(path) if path.is_absolute() => {
                requests.extend(action.requests(path));
                return false;
            }
            _ => *error = Some("Enter an absolute directory path".into()),
        }
    }
    if let Some(error) = error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
    true
}
fn policy_dialog(
    ui: &mut Ui,
    status: &DriveStatus,
    relative: &std::path::Path,
    catalog: &mut bool,
    index: &mut bool,
) {
    ui.label(format!(
        "Applies to {}",
        if relative.as_os_str().is_empty() {
            "the whole drive".into()
        } else {
            display_path(relative)
        }
    ));
    ui.checkbox(catalog, "Include metadata in future contents catalogs");
    ui.checkbox(index, "Allow explicitly configured search indexing");
    ui.label("These settings do not exclude files from backups or folder synchronization. Existing catalog snapshots keep their captured date.");
    ui.small(format!(
        "Catalog limit: {} entries, {} MiB. Parent exclusions also apply.",
        status.config.catalog_max_entries,
        status.config.catalog_max_bytes / (1024 * 1024)
    ));
}
fn details(ui: &mut Ui, status: &DriveStatus, requests: &mut Vec<DriveRequest>) {
    egui::ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
        for conflict in status.conflicts.iter().filter(|c| !c.resolved) {
            ui.group(|ui| {
                ui.label(format!("Conflict: {}", display_path(&conflict.path)));
                ui.label(&conflict.message);
                ui.small(format!("Portable: {}", display_path(&conflict.portable)));
                ui.small(format!("Local: {}", display_path(&conflict.local)));
                if ui.add_enabled(conflict.complete, egui::Button::new("Acknowledge; keep variants")).clicked() {
                    requests.push(DriveRequest::Resolve { conflict: conflict.id.clone() });
                }
            });
        }
        for pair in &status.pairs { ui.label(format!("{} · {}", display_path(&pair.local), pair.status)); }
        for operation in &status.operations {
            ui.group(|ui| {
                ui.label(format!("{} · {}", operation.title, operation.state));
                ui.small(format!("{} files · {} bytes", operation.files, operation.bytes));
                ui.label(&operation.message);
                if operation.finished.is_none() && ui.button("Cancel remaining work").clicked() {
                    requests.push(DriveRequest::Cancel { operation: operation.id.clone() });
                }
            });
        }
        if status.next_offset.is_some() { ui.small("Showing recent jobs and snapshots. Older records remain available through clipctl drives status."); }
    });
}
impl Drives {
    pub(super) fn windows(&mut self, ctx: &Context, current: &std::path::Path) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return;
        }
        let mut keep = true;
        let mut requests = Vec::new();
        egui::Window::new(dialog.title())
            .id(egui::Id::new("portable_drive_dialog"))
            .collapsible(false)
            .resizable(true)
            .default_width(540.0)
            .show(ctx, |ui| {
                if matches!(dialog, Dialog::Details)
                    && let Some(error) = &self.error
                {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("Drive service unavailable: {error}"),
                    );
                }
                keep = dialog.show(ui, current, &self.status, &mut requests);
                ui.separator();
                if ui
                    .button(if matches!(dialog, Dialog::Details) {
                        "Close"
                    } else {
                        "Cancel"
                    })
                    .clicked()
                {
                    keep = false;
                }
            });
        for request in requests {
            self.send(request);
        }
        if keep {
            self.dialog = Some(dialog);
        }
    }
}
