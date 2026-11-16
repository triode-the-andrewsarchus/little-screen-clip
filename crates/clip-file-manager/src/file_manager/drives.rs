//! File-manager controls for retained daemon storage operations.
use super::{FileManager, Navigation, display_path, mounts::MountEntry};
use egui::{Context, Ui};
use little_clip_core::{
    drives::*,
    ipc::{IpcRequest, IpcResponse},
    jobs::{BackgroundJobs, task::Task},
};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};
mod dialogs;
#[cfg(test)]
mod tests;

static IO: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 16));

fn recorded_time(timestamp: u64) -> String {
    i64::try_from(timestamp)
        .ok()
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "unknown time".into())
}

fn catalog_description(catalog: &Catalog) -> String {
    let mut text = format!(
        "Captured {} · {} entries · {}",
        recorded_time(catalog.captured),
        catalog.entries,
        catalog.coverage
    );
    if let Some(redacted) = catalog.redacted {
        text.push_str(&format!("\nRedacted {}", recorded_time(redacted)));
    }
    if catalog.pending_redactions != 0 {
        text.push_str("\nMetadata cleanup pending. Unmount this catalog's existing view to release its saved snapshot.");
    }
    if let Some(error) = &catalog.redaction_error {
        text.push_str(&format!("\nCleanup will retry: {error}"));
    }
    text
}

enum PathAction {
    Directory,
    Backup { drive: String, kind: BackupKind },
    Pair { drive: String, relative: PathBuf },
    SaveRam { session: String },
}
enum Dialog {
    Path {
        title: String,
        text: String,
        action: PathAction,
        error: Option<String>,
    },
    Discard(String),
    Deletions {
        pair: String,
        enabled: bool,
    },
    Policy {
        drive: String,
        relative: PathBuf,
        catalog: bool,
        index: bool,
    },
    Details,
    BackupLoading {
        generation: u64,
    },
    BackupReview {
        preview: Box<BackupPreview>,
        restore_mounted: bool,
        remember_destination: bool,
    },
}
struct PendingRequest {
    generation: u64,
    request: DriveRequest,
    task: Task<Result<DriveReply, String>>,
}
struct StatusPoll {
    reply: Result<DriveReply, String>,
    policy: Option<Arc<IndexPolicy>>,
}
pub(super) struct Drives {
    pub status: DriveStatus,
    poll: Option<Task<StatusPoll>>,
    policy: Option<Arc<IndexPolicy>>,
    policy_observed: bool,
    policy_changed: bool,
    request: Option<PendingRequest>,
    queue: VecDeque<(u64, DriveRequest)>,
    preview_generation: u64,
    last_poll: Instant,
    dialog: Option<Dialog>,
    seen: HashSet<String>,
    pub notice: Option<(String, bool)>,
    initialized: bool,
    restore_mounted: bool,
    pub error: Option<String>,
}
impl Default for Drives {
    fn default() -> Self {
        Self {
            status: Default::default(),
            poll: None,
            policy: None,
            policy_observed: false,
            policy_changed: false,
            request: None,
            queue: VecDeque::new(),
            preview_generation: 0,
            last_poll: Instant::now() - Duration::from_secs(5),
            dialog: None,
            seen: HashSet::new(),
            notice: None,
            initialized: false,
            restore_mounted: false,
            error: None,
        }
    }
}
fn ipc(request: DriveRequest) -> Result<DriveReply, String> {
    ipc_with(request, |request| {
        crate::desktop_ipc::request(
            request,
            Some(Duration::from_secs(3)),
            4 * 1024 * 1024,
            1024 * 1024,
        )
        .map_err(|error| error.to_string())
    })
}
fn ipc_with(
    request: DriveRequest,
    mut transport: impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
) -> Result<DriveReply, String> {
    let request = IpcRequest::drive(request, uuid::Uuid::new_v4());
    let first = transport(&request);
    let response = if first.is_err() && matches!(request, IpcRequest::SubmitDrive { .. }) {
        // Only transport failure is retried, with the identical identity/payload.
        // A server rejection is a response and never authorizes another operation.
        transport(&request)
    } else {
        first
    };
    let response = response.map_err(|error| match &request {
        IpcRequest::SubmitDrive { request_id, .. } => format!("Drive admission reply unavailable: {error}. Request {request_id} can be retried with the same identity; inspect Jobs before starting new work"),
        _ => error,
    })?;
    match response {
        IpcResponse::Drives(reply) => Ok(reply),
        IpcResponse::Error { error } => Err(error),
        _ => Err("Unexpected drive service response".into()),
    }
}
impl Drives {
    fn send(&mut self, request: DriveRequest) {
        if self.queue.len() < 16 {
            if matches!(request, DriveRequest::PreviewBackup { .. }) {
                self.preview_generation = self.preview_generation.wrapping_add(1);
                self.dialog = Some(Dialog::BackupLoading {
                    generation: self.preview_generation,
                });
            }
            self.queue.push_back((self.preview_generation, request));
        } else {
            self.notice = Some(("Drive request queue is full".into(), true));
        }
    }
    pub fn tick(&mut self) {
        if let Some((generation, request, result)) = self.request.as_ref().and_then(|pending| {
            pending.task.poll().map(|result| {
                (
                    pending.generation,
                    pending.request.clone(),
                    result
                        .map_err(|error| error.to_string())
                        .and_then(|result| result),
                )
            })
        }) {
            self.request = None;
            self.accept_reply(generation, request, result);
        }
        if self.request.is_none()
            && let Some((generation, request)) = self.queue.pop_front()
        {
            let input = request.clone();
            self.request = Some(PendingRequest {
                generation,
                request,
                task: Task::start_unbounded(&IO, move |_| ipc(input)),
            });
        }
        if let Some(result) = self.poll.as_ref().and_then(Task::poll) {
            self.poll = None;
            match result {
                Ok(result) => {
                    self.accept_policy(result.policy);
                    match result.reply {
                        Ok(DriveReply::Status(status)) => self.accept_status(*status),
                        Err(error) => self.error = Some(error),
                        _ => self.error = Some("Drive service did not return its status".into()),
                    }
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if self.poll.is_none() && self.last_poll.elapsed() >= Duration::from_secs(1) {
            self.poll = Some(Task::start(&IO, Duration::from_secs(5), |_| {
                let reply = ipc(DriveRequest::Status { offset: 0 });
                StatusPoll {
                    reply,
                    policy: index_policy(),
                }
            }));
            self.last_poll = Instant::now();
        }
    }
    fn accept_policy(&mut self, policy: Option<Arc<IndexPolicy>>) {
        let unchanged = match (&self.policy, &policy) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        let initial_empty = !self.policy_observed
            && policy
                .as_ref()
                .is_some_and(|policy| policy.excluded.is_empty());
        self.policy_changed |= !initial_empty && (!self.policy_observed || !unchanged);
        self.policy_observed = true;
        self.policy = policy;
    }
    fn accept_reply(
        &mut self,
        generation: u64,
        request: DriveRequest,
        result: Result<DriveReply, String>,
    ) {
        let preview_active = matches!(self.dialog, Some(Dialog::BackupLoading { generation: current }) if current == generation);
        if matches!(request, DriveRequest::PreviewBackup { .. }) && result.is_err() {
            if !preview_active {
                return;
            }
            self.dialog = None;
        }
        match result {
            Ok(DriveReply::Accepted(_)) => self.last_poll = Instant::now() - Duration::from_secs(2),
            Ok(DriveReply::Done(_))
                if matches!(request, DriveRequest::DiscardBackupReview { .. }) => {}
            Ok(DriveReply::Done(text)) => self.notice = Some((text, false)),
            Ok(DriveReply::BackupPreview(preview)) => {
                if preview_active {
                    self.dialog = Some(Dialog::BackupReview {
                        restore_mounted: self.restore_mounted && preview.drive.mount.is_some(),
                        remember_destination: self.status.backup_directory.as_ref()
                            != Some(&preview.destination),
                        preview,
                    });
                } else {
                    self.send(DriveRequest::DiscardBackupReview {
                        token: preview.token,
                    });
                }
            }
            Err(error) => self.notice = Some((error, true)),
            _ => (),
        }
    }
    fn accept_status(&mut self, status: DriveStatus) {
        self.error = None;
        for operation in &status.operations {
            if operation.finished.is_some()
                && self.seen.insert(operation.id.clone())
                && self.initialized
            {
                self.notice = Some((
                    format!("{}: {}", operation.title, operation.message),
                    operation.state != "complete",
                ));
            }
        }
        for conflict in status.conflicts.iter().filter(|c| !c.resolved) {
            if self.seen.insert(format!("conflict:{}", conflict.id)) {
                self.notice=Some(("Drive sync conflict: both portable and local variants were preserved. Open Drive jobs to review.".into(),true));
            }
        }
        self.initialized = true;
        if self.seen.len() > 2048 {
            self.seen.retain(|id| {
                status.operations.iter().any(|o| o.id == *id)
                    || status
                        .conflicts
                        .iter()
                        .any(|c| format!("conflict:{}", c.id) == *id)
            });
        }
        self.status = status;
    }
    pub fn open_details(&mut self) {
        self.dialog = Some(Dialog::Details);
    }
    pub fn modal(&self) -> bool {
        self.dialog.is_some()
    }
    fn backup(&mut self, drive: &Drive, kind: BackupKind) {
        if self.status.backup_directory.is_none() {
            self.dialog = Some(Dialog::Path {
                error: None,
                title: "Choose backup directory".into(),
                text: String::new(),
                action: PathAction::Backup {
                    drive: drive.id.clone(),
                    kind,
                },
            });
        } else {
            self.send(DriveRequest::PreviewBackup {
                drive: drive.id.clone(),
                kind,
                destination: None,
            });
        }
    }
    fn menu(&mut self, ui: &mut Ui, drive: &Drive) -> Option<PathBuf> {
        let mut open = None;
        ui.separator();
        if let Some(error) = &self.error {
            ui.label(format!("Drive service unavailable: {error}"));
            return None;
        }
        self.ram_copy_menu(ui, drive, &mut open);
        ui.menu_button("Back up partition", |ui| {
            self.backup_menu(ui, drive, &mut open)
        });
        ui.menu_button("Keep in sync", |ui| self.sync_menu(ui, drive));
        ui.menu_button("Contents catalog", |ui| {
            self.catalog_menu(ui, drive, &mut open)
        });
        if ui.button("Drive operations and conflicts…").clicked() {
            self.dialog = Some(Dialog::Details);
            ui.close_menu();
        }
        open
    }
    fn ram_copy_menu(&mut self, ui: &mut Ui, drive: &Drive, open: &mut Option<PathBuf>) {
        let ram = self
            .status
            .ram
            .iter()
            .find(|s| s.drive == drive.id)
            .cloned();
        if let Some(session) = ram {
            if ui
                .add_enabled(session.browsable(), egui::Button::new("Browse RAM copy"))
                .clicked()
            {
                *open = Some(session.path);
                ui.close_menu();
            }
        } else {
            let reason = if drive.ambiguous {
                Some("Drive identity is ambiguous".to_string())
            } else if drive.mount.is_none() {
                Some("Mount the partition first".into())
            } else if drive.drive_capacity == 0
                || drive.drive_capacity >= self.status.config.ram_capacity_limit
            {
                Some("Drive exceeds the configured RAM-copy capacity limit".into())
            } else {
                self.status.ram_unavailable.clone()
            };
            let response = ui.add_enabled(reason.is_none(), egui::Button::new("Copy to RAM"));
            if response.clicked() {
                self.send(DriveRequest::CopyToRam {
                    drive: drive.id.clone(),
                });
                ui.close_menu();
            }
            if let Some(reason) = reason {
                response.on_hover_text(reason);
            }
        }
    }
    fn sync_menu(&mut self, ui: &mut Ui, drive: &Drive) {
        if ui.button("Pair with a folder…").clicked() {
            self.dialog = Some(Dialog::Path {
                error: None,
                title: "Keep this drive in sync with a folder".into(),
                text: String::new(),
                action: PathAction::Pair {
                    drive: drive.id.clone(),
                    relative: PathBuf::new(),
                },
            });
            ui.close_menu();
        }
        let pairs = self
            .status
            .pairs
            .iter()
            .filter(|p| p.drive == drive.id && !p.ram)
            .cloned()
            .collect::<Vec<_>>();
        for pair in pairs {
            ui.separator();
            ui.label(display_path(&pair.local));
            ui.small(&pair.status);
            if ui.button("Sync now").clicked() {
                self.send(DriveRequest::Sync {
                    pair: pair.id.clone(),
                });
                ui.close_menu();
            }
            if ui
                .button(if pair.paused {
                    "Resume automatic sync"
                } else {
                    "Pause automatic sync"
                })
                .clicked()
            {
                self.send(DriveRequest::Pause {
                    pair: pair.id.clone(),
                    paused: !pair.paused,
                });
                ui.close_menu();
            }
            if ui
                .button(if pair.deletions {
                    "Disable deletion propagation…"
                } else {
                    "Enable deletion propagation…"
                })
                .clicked()
            {
                self.dialog = Some(Dialog::Deletions {
                    pair: pair.id,
                    enabled: !pair.deletions,
                });
                ui.close_menu();
            }
        }
        let conflicts = self
            .status
            .conflicts
            .iter()
            .filter(|c| {
                !c.resolved
                    && self
                        .status
                        .pairs
                        .iter()
                        .any(|p| p.id == c.pair && p.drive == drive.id)
            })
            .count();
        if ui
            .button(format!("Review conflicts… ({conflicts})"))
            .clicked()
        {
            self.dialog = Some(Dialog::Details);
            ui.close_menu();
        }
    }
    fn catalog_menu(&mut self, ui: &mut Ui, drive: &Drive, open: &mut Option<PathBuf>) {
        if ui
            .add_enabled(
                drive.mount.is_some(),
                egui::Button::new("Save / update catalog…"),
            )
            .clicked()
        {
            self.send(DriveRequest::Catalog {
                drive: display_path(&drive.device),
            });
            ui.close_menu();
        }
        let catalogs = self
            .status
            .catalogs
            .iter()
            .filter(|c| c.drive == drive.id)
            .cloned()
            .collect::<Vec<_>>();
        for catalog in catalogs {
            ui.separator();
            ui.small(catalog_description(&catalog));
            if let Some(path) = &catalog.mount {
                if ui.button("Browse mounted catalog").clicked() {
                    *open = Some(path.clone());
                    ui.close_menu();
                }
                if ui.button("Unmount saved catalog").clicked() {
                    self.send(DriveRequest::UnmountCatalog {
                        catalog: catalog.id.clone(),
                    });
                    ui.close_menu();
                }
            } else if ui.button("Mount saved catalog").clicked() {
                self.send(DriveRequest::MountCatalog {
                    catalog: catalog.id.clone(),
                });
                ui.close_menu();
            }
        }
        if ui.button("Catalog and indexing settings…").clicked() {
            self.dialog = Some(Dialog::Policy {
                drive: drive.id.clone(),
                relative: PathBuf::new(),
                catalog: true,
                index: true,
            });
            ui.close_menu();
        }
    }
    fn image_menu(&mut self, ui: &mut Ui, drive: &Drive) {
        ui.separator();
        ui.label(format!("Partition image: {} bytes", drive.capacity));
        let mut restore = self.restore_mounted && drive.mount.is_some();
        if ui
            .add_enabled(
                drive.mount.is_some(),
                egui::Checkbox::new(&mut restore, "Mount again after imaging"),
            )
            .on_hover_text("Only if this image job unmounted the partition. Restoration also runs after image failure or cancellation; changed devices are refused.")
            .changed()
        {
            self.restore_mounted = restore;
        }
        if ui
            .add_enabled(!drive.ambiguous, egui::Button::new("As partition image…"))
            .on_hover_text("Flushes enrolled changes and unmounts the partition before imaging. Desktop authentication may be required.")
            .clicked()
        {
            self.backup(drive, BackupKind::Image);
            ui.close_menu();
        }
    }
    fn backup_menu(&mut self, ui: &mut Ui, drive: &Drive, open: &mut Option<PathBuf>) {
        if ui
            .add_enabled(drive.mount.is_some(), egui::Button::new("As files…"))
            .clicked()
        {
            self.backup(drive, BackupKind::Files);
            ui.close_menu();
        }
        self.image_menu(ui, drive);
        if ui
            .add_enabled(
                self.status.backup_directory.is_some(),
                egui::Button::new("Browse saved backups…"),
            )
            .clicked()
        {
            *open = self
                .status
                .backup_directory
                .clone()
                .map(|p| drive.part_uuid.as_ref().map_or(p.clone(), |id| p.join(id)));
            ui.close_menu();
        }
        let images = self
            .status
            .backups
            .iter()
            .filter(|v| v.drive == drive.id && v.kind == BackupKind::Image)
            .cloned()
            .collect::<Vec<_>>();
        if !images.is_empty() {
            ui.menu_button("Read-only image backups", |ui| {
                for version in images {
                    let date = recorded_time(version.created);
                    ui.menu_button(date, |ui| {
                        if let Some(path) = &version.mount {
                            if ui.button("Browse image contents").clicked() {
                                *open = Some(path.clone());
                                ui.close_menu();
                            }
                        } else if ui.button("Mount image read-only").clicked() {
                            self.send(DriveRequest::MountImage {
                                backup: version.id.clone(),
                            });
                            ui.close_menu();
                        }
                        if version.loop_object.is_some() && ui.button("Unmount image").clicked() {
                            self.send(DriveRequest::UnmountImage {
                                backup: version.id.clone(),
                            });
                            ui.close_menu();
                        }
                    });
                }
            });
        }
        if ui.add_enabled(self.status.config.backup_keep_versions.is_some(),egui::Button::new("Prune old completed versions")).on_hover_text("Uses the configured number of versions to retain. Mounted catalog providers are kept.").clicked(){self.send(DriveRequest::PruneBackups{drive:drive.id.clone()});ui.close_menu();}
        if ui.button("Change backup directory…").clicked() {
            self.dialog = Some(Dialog::Path {
                error: None,
                title: "Choose backup directory".into(),
                text: self
                    .status
                    .backup_directory
                    .as_ref()
                    .map(|p| display_path(p))
                    .unwrap_or_default(),
                action: PathAction::Directory,
            });
            ui.close_menu();
        }
    }
}
impl FileManager {
    pub(super) fn tick_drives(&mut self) {
        self.drives.tick();
        if std::mem::take(&mut self.drives.policy_changed) {
            self.shelf.invalidate_index_policy();
            self.similarity_canvas = Default::default();
            self.constellation.invalidate_index_policy();
        }
        // The daemon is authoritative for managed volumes; keep the standalone
        // scanner as the fallback and for non-block/FUSE mounts.
        for drive in &self.drives.status.drives {
            if drive.filesystem == "swap"
                || drive.mount.as_ref().is_some_and(|p| {
                    p == std::path::Path::new("/")
                        || p.starts_with("/boot")
                        || p.starts_with("/snap")
                })
            {
                continue;
            }
            if !self
                .mounts
                .entries
                .iter()
                .any(|entry| entry.device_path.as_ref() == Some(&drive.device))
            {
                self.mounts.entries.insert(
                    0,
                    super::mounts::MountEntry {
                        id: drive.id.clone(),
                        label: drive.label.clone(),
                        detail: format!(
                            "{} · {}",
                            drive.filesystem,
                            little_clip_core::file_inventory::format_size(drive.capacity)
                        ),
                        mount_point: drive.mount.clone(),
                        device_path: Some(drive.device.clone()),
                        filesystem: Some(drive.filesystem.clone()),
                        is_mounted: drive.mount.is_some(),
                        is_removable: drive.removable,
                        kind: if drive.removable {
                            super::mounts::MountKind::Removable
                        } else {
                            super::mounts::MountKind::Drive
                        },
                    },
                );
            }
        }
        if let Some((text, failed)) = self.drives.notice.take() {
            self.set_status(
                if failed {
                    crate::icons::Icon::Alert
                } else {
                    crate::icons::Icon::Drive
                },
                text,
                failed,
            );
        }
    }
    pub(super) fn drive_context_menu(&mut self, ui: &mut Ui, entry: &MountEntry) {
        let drive = self
            .drives
            .status
            .drives
            .iter()
            .find(|d| entry.device_path.as_ref() == Some(&d.device))
            .cloned();
        if let Some(drive) = drive
            && let Some(open) = self.drives.menu(ui, &drive)
        {
            self.navigate(open, Navigation::New);
        }
    }
    pub(super) fn ram_mounts_ui(&mut self, ui: &mut Ui) {
        let mut open = None;
        for session in self.drives.status.ram.clone() {
            let label = self
                .drives
                .status
                .drives
                .iter()
                .find(|d| d.id == session.drive)
                .map_or("Portable drive", |d| d.label.as_str());
            let response = ui.add(egui::SelectableLabel::new(
                self.path().starts_with(&session.path),
                format!(
                    "RAM: {label}{}",
                    if session.connected { "" } else { " (offline)" }
                ),
            ));
            if response.clicked() && session.browsable() {
                open = Some(session.path.clone());
            }
            response.clone().on_hover_text(&session.status);
            response.context_menu(|ui| {
                if ui
                    .add_enabled(session.browsable(), egui::Button::new("Open"))
                    .clicked()
                {
                    open = Some(session.path.clone());
                    ui.close_menu();
                }
                if ui
                    .add_enabled(
                        session.connected,
                        egui::Button::new("Flush pending changes to drive"),
                    )
                    .clicked()
                {
                    self.drives.send(DriveRequest::FlushRam {
                        session: session.id.clone(),
                    });
                    ui.close_menu();
                }
                if ui.button("Save RAM copy elsewhere…").clicked() {
                    self.drives.dialog = Some(Dialog::Path {
                        error: None,
                        title: "Save RAM copy elsewhere".into(),
                        text: String::new(),
                        action: PathAction::SaveRam {
                            session: session.id.clone(),
                        },
                    });
                    ui.close_menu();
                }
                if ui.button("Discard RAM copy…").clicked() {
                    self.drives.dialog = Some(Dialog::Discard(session.id.clone()));
                    ui.close_menu();
                }
            });
        }
        self.catalog_mounts_ui(ui, &mut open);
        if let Some(path) = open {
            self.navigate(path, Navigation::New);
        }
    }
    fn catalog_mounts_ui(&mut self, ui: &mut Ui, open: &mut Option<PathBuf>) {
        let mut shown = HashSet::new();
        for catalog in self.drives.status.catalogs.clone() {
            if !shown.insert(catalog.drive.clone()) {
                continue;
            }
            let response = ui.selectable_label(
                catalog
                    .mount
                    .as_ref()
                    .is_some_and(|p| self.path().starts_with(p)),
                format!("Catalog: {}", catalog.label),
            );
            if response.clicked() {
                if let Some(path) = &catalog.mount {
                    *open = Some(path.clone());
                } else {
                    self.drives.send(DriveRequest::MountCatalog {
                        catalog: catalog.id.clone(),
                    });
                }
            }
            response
                .clone()
                .on_hover_text(catalog_description(&catalog));
            // egui 0.27 keys context menus by Response::id. Keep that key
            // independent of the label's ordinal auto-ID as status rows change.
            let mut menu_response = response.clone();
            menu_response.id = ui.make_persistent_id(("saved-catalog-menu", &catalog.id));
            menu_response.context_menu(|ui| {
                ui.set_width((ui.ctx().screen_rect().width() - 32.0).clamp(0.0, 400.0));
                ui.small(catalog_description(&catalog));
                if let Some(path) = &catalog.mount {
                    if ui.button("Open saved catalog").clicked() {
                        *open = Some(path.clone());
                        ui.close_menu();
                    }
                    if ui.button("Unmount saved catalog").clicked() {
                        self.drives.send(DriveRequest::UnmountCatalog {
                            catalog: catalog.id.clone(),
                        });
                        ui.close_menu();
                    }
                } else if ui.button("Mount saved catalog").clicked() {
                    self.drives.send(DriveRequest::MountCatalog {
                        catalog: catalog.id.clone(),
                    });
                    ui.close_menu();
                }
            });
        }
    }
    pub(super) fn eject_enrolled_drive(&mut self, entry: &MountEntry) -> bool {
        let drive = self
            .drives
            .status
            .drives
            .iter()
            .find(|d| Some(&d.device) == entry.device_path.as_ref())
            .cloned();
        if let Some(drive) = drive
            && self.drives.status.pairs.iter().any(|p| p.drive == drive.id)
        {
            self.drives.send(DriveRequest::Eject { drive: drive.id });
            return true;
        }
        false
    }
    pub(super) fn drive_windows(&mut self, ctx: &Context) {
        let current = self.path().to_owned();
        self.drives.windows(ctx, &current);
    }
    pub(super) fn open_folder_drive_policy(&mut self, path: &std::path::Path) {
        let drive = self
            .drives
            .status
            .drives
            .iter()
            .find(|d| d.mount.as_ref().is_some_and(|m| path.starts_with(m)))
            .cloned();
        if let Some(drive) = drive
            && let Some(mount) = &drive.mount
        {
            let relative = path
                .strip_prefix(mount)
                .unwrap_or(std::path::Path::new(""))
                .to_owned();
            let rule = self
                .drives
                .status
                .rules
                .iter()
                .find(|r| r.drive == drive.id && r.relative == relative);
            self.drives.dialog = Some(Dialog::Policy {
                drive: drive.id,
                relative,
                catalog: rule.is_none_or(|r| r.catalog),
                index: rule.is_none_or(|r| r.index),
            });
        }
    }
}
