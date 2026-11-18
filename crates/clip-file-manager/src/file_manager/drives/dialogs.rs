//! Drive dialogs keep destination selection and conflict review independent of menus.
use super::*;
mod catalog;
impl Dialog {
    fn title(&self) -> &str {
        match self {
            Self::Path { title, .. } => title,
            Self::Discard(_) => "Discard RAM copy",
            Self::Deletions { .. } => "Deletion propagation",
            Self::Policy { .. } | Self::PolicyLoading { .. } => "Catalog and indexing policy",
            Self::Details => "Drive operations and conflicts",
            Self::BackupLoading { .. } => "Checking backup destination",
            Self::BackupReview { .. } => "Review partition backup",
            Self::Catalog { .. } => "Review contents catalog",
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
            Self::Catalog { drive, limits } => catalog::review(ui, drive, limits, status, requests),
            Self::PolicyLoading { .. } => {
                ui.spinner();
                ui.label("Reading saved catalog and indexing settings…");
                true
            }
            Self::BackupLoading { .. } => {
                ui.spinner();
                ui.label("Checking the selected partition, destination and available space…");
                true
            }
            Self::BackupReview {
                preview,
                restore_mounted,
                remember_destination,
                read_only,
            } => backup_review(
                ui,
                preview,
                restore_mounted,
                remember_destination,
                read_only,
                requests,
            ),
            Self::Path {
                text,
                action,
                error,
                ..
            } => path_dialog(ui, current, status, text, action, error, requests),
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
            Self::Backup { drive, kind } => vec![DriveRequest::PreviewBackup {
                drive: drive.clone(),
                kind: *kind,
                destination: Some(path),
            }],
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
    status: &DriveStatus,
    text: &mut String,
    action: &PathAction,
    error: &mut Option<String>,
    requests: &mut Vec<DriveRequest>,
) -> bool {
    ui.label(action.description());
    if let PathAction::Pair { drive, .. } | PathAction::Backup { drive, .. } = action
        && let Some(drive) = status.drives.iter().find(|observed| &observed.id == drive)
    {
        fallback_identity(ui, drive);
    }
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

fn fallback_identity(ui: &mut Ui, drive: &Drive) {
    if drive.part_uuid.is_some() {
        return;
    }
    if let Some(uuid) = &drive.filesystem_uuid {
        ui.label(format!("No partition UUID. Using filesystem UUID: {uuid}"));
        ui.small(format!("Saved identity: {}", drive.id));
    } else {
        ui.label("No partition or filesystem UUID. Automatic sync is unavailable; each backup uses a separate unidentified folder.");
    }
}
fn backup_review(
    ui: &mut Ui,
    preview: &BackupPreview,
    restore_mounted: &mut bool,
    remember_destination: &mut bool,
    read_only: &mut bool,
    requests: &mut Vec<DriveRequest>,
) -> bool {
    ui.label(format!(
        "Partition: {} · {}",
        preview.drive.label,
        display_path(&preview.drive.device)
    ));
    ui.label(format!("Partition size: {} bytes", preview.drive.capacity));
    fallback_identity(ui, &preview.drive);
    ui.label(format!(
        "Destination: {}",
        display_path(&preview.destination)
    ));
    let bucket = preview.drive.backup_bucket();
    match &bucket {
        Ok(Some(key)) => {
            ui.label(format!(
                "Backup folder: {}",
                display_path(&preview.destination.join(key))
            ));
        }
        Ok(None) => {
            ui.small("Backup folder: a new unidentified folder for this backup.");
        }
        Err(error) => {
            ui.colored_label(egui::Color32::LIGHT_RED, *error);
        }
    }
    ui.label(format!(
        "Available: {} bytes",
        preview.space.available_bytes
    ));
    if let Some(bytes) = preview.space.full_image_bytes {
        ui.label(format!(
            "Full independent image estimate: {bytes} bytes, including 64 MiB working headroom"
        ));
    } else {
        ui.small("File backup allocation depends on file sizes and metadata. Free space is checked again when the job starts.");
    }
    if preview.space.btrfs {
        ui.label("Btrfs destination: completed versions can share unchanged blocks. Actual additional allocation depends on changes; snapshot/reflink permission failures are reported.");
        if preview
            .space
            .full_image_bytes
            .is_some_and(|bytes| bytes > preview.space.available_bytes)
        {
            ui.colored_label(egui::Color32::YELLOW, "A full independent image will not fit. This backup depends on sharing or sparse zero ranges and may run out of space.");
        }
    }
    if preview.kind == BackupKind::Image {
        ui.label("Enrolled changes will be flushed and the partition unmounted before imaging. Desktop authentication may be required.");
        ui.add_enabled(
            preview.drive.mount.is_some(),
            egui::Checkbox::new(restore_mounted, "Mount again after imaging"),
        );
    } else {
        ui.checkbox(read_only, "Temporarily mount read-only for collection");
        if *read_only {
            ui.label("Enrolled edits will be flushed before unmounting. Close open files on this partition. Collection uses a verified read-only filesystem; the original access mode is restored afterwards. Desktop authentication may be required.");
        } else {
            ui.label("Files are checked as they are copied. A writable source can change during the backup, so files may represent different moments. Files that keep changing stop the backup after three attempts.");
        }
        ui.small("Metadata the destination cannot preserve is recorded in the manifest and reported with the job result.");
    }
    ui.checkbox(
        remember_destination,
        "Update the backup directory setting for future backups",
    );
    ui.small("This review expires after five minutes. Changes to the selected partition or directory require a new review.");
    let confirmed = ui
        .add_enabled(bucket.is_ok(), egui::Button::new("Start backup"))
        .clicked();
    if confirmed {
        requests.push(if preview.kind == BackupKind::Files && *read_only {
            DriveRequest::StartReadOnlyBackup {
                token: preview.token.clone(),
                remember_destination: *remember_destination,
            }
        } else {
            DriveRequest::StartBackup {
                token: preview.token.clone(),
                restore_mounted: preview.kind == BackupKind::Image && *restore_mounted,
                remember_destination: *remember_destination,
            }
        });
    }
    !confirmed
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
    ui.label("Excluding metadata also removes saved descendant catalog records after mounted catalog views are unmounted. Catalogs retain their capture date and show a separate redaction date. These settings do not exclude files from backups or folder synchronization.");
    ui.small("Enrolled folder sync keeps the names and fingerprints it needs to track changes. Jobs → Measure local metadata shows its storage separately from saved catalogs.");
    ui.small(format!(
        "Catalog limit: {} entries, {} MiB. Parent exclusions also apply.",
        status.config.catalog_max_entries,
        status.config.catalog_max_bytes / (1024 * 1024)
    ));
}
fn details(ui: &mut Ui, status: &DriveStatus, requests: &mut Vec<DriveRequest>) {
    egui::ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
        if ui.button("Measure local metadata").clicked() {
            requests.push(DriveRequest::MeasureStorage);
        }
        ui.small("Folder sync keeps operational metadata even when indexing and catalogs are excluded. Measurement runs in the background and saves its result as a job.");
        for session in &status.ram {
            ui.group(|ui| {
                ui.label(ram_description(session));
                ui.label(session.save_status.as_deref().unwrap_or("No RAM copy saved elsewhere yet"));
                if let Some(saved) = &session.last_saved {
                    let date = recorded_time(saved.completed);
                    ui.small(format!("{}saved at {date}: {}", if saved.populated { "" } else { "Partial copy " }, display_path(&saved.path)));
                }
                ui.small("Saving elsewhere does not flush changes to the removable media.");
            });
        }
        for recovery in &status.recovery {
            ui.group(|ui| {
                ui.label("Prepared backup needs recovery");
                ui.label(&recovery.message);
                ui.small(format!("Retained data: {}", display_path(&recovery.artifact)));
                ui.label("Correct the reported problem and retry the backup. Prepared versions are checked before another backup starts.");
            });
        }
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
        for operation in &status.operations { operation_details(ui, operation, requests); }
        if status.next_offset.is_some() { ui.small("Showing recent jobs and snapshots. Older records remain available through clipctl drives status."); }
    });
}
fn operation_details(ui: &mut Ui, operation: &DriveOperation, requests: &mut Vec<DriveRequest>) {
    ui.group(|ui| {
        ui.label(format!("{} · {}", operation.title, operation.state));
        if let Some(finished) = operation.finished {
            ui.small(format!("Finished {}", recorded_time(finished)));
        }
        ui.small(format!(
            "{} files · {} bytes",
            operation.files, operation.bytes
        ));
        ui.label(&operation.message);
        if let Some(partition) = &operation.partition {
            ui.small(format!(
                "Partition: {} · {} bytes",
                partition.drive.label, partition.drive.capacity
            ));
            if let Some(destination) = &partition.destination {
                ui.small(format!("Backup destination: {}", display_path(destination)));
            }
            if partition.restore_mounted {
                ui.small(if partition.file_mode == FileBackupMode::ReadOnly {
                    "Restore original mount and access mode"
                } else {
                    "Restore mounted state requested"
                });
            }
            if let Some(path) = &partition.collection_mount {
                ui.small(format!("Collection mount: {}", display_path(path)));
            }
            if let Some(error) = &partition.restore_error {
                ui.label(format!("Remount: {error}"));
            }
        }
        if operation.finished.is_none() && ui.button("Cancel remaining work").clicked() {
            requests.push(DriveRequest::Cancel {
                operation: operation.id.clone(),
            });
        }
    });
}
impl Drives {
    pub(super) fn windows(&mut self, ctx: &Context, current: &std::path::Path) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if let Dialog::BackupReview { preview, .. } = &dialog {
                self.send(DriveRequest::DiscardBackupReview {
                    token: preview.token.clone(),
                });
            }
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
                    if let Dialog::BackupReview { preview, .. } = &dialog {
                        requests.push(DriveRequest::DiscardBackupReview {
                            token: preview.token.clone(),
                        });
                    }
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
