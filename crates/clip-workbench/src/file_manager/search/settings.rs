//! Search preferences are edited locally and persisted by the index coordinator.
use super::{policy, service::Command, store::RootState};
use crate::file_manager::{FileManager, filesystem};
use little_clip_core::config::FileSearchConfig;

#[derive(Default)]
pub(in crate::file_manager) struct Settings {
    pub open: bool,
    draft: FileSearchConfig,
    locations: String,
    exclusions: String,
    error: Option<String>,
    backup_address: String,
    restore_address: String,
    restore_confirm: bool,
    maintenance_pending: bool,
    clear_confirm: bool,
    recover_confirm: bool,
}

#[derive(Default)]
struct ViewState {
    roots: Vec<RootState>,
    message: String,
    error: Option<String>,
    busy: bool,
    paused: bool,
    has_index: bool,
    outcome: Option<Result<String, String>>,
}

pub(super) fn validate(config: &FileSearchConfig) -> Result<(), String> {
    policy::exclusions(&config.excluded_paths)?;
    if !(16..=1_048_576).contains(&config.limits.max_mib)
        || !(1..=100_000_000).contains(&config.limits.max_vectors)
    {
        return Err("Index limits require 16..1048576 MiB and 1..100000000 vectors".into());
    }
    if config.always_search.len() > 32 {
        return Err("Use at most 32 automatic-search locations".into());
    }
    for address in &config.always_search {
        if address.len() > 8192 {
            return Err("An automatic-search address is too long".into());
        }
        let path = filesystem::decode_path(address)?;
        if !path.is_absolute() {
            return Err("Automatic-search locations must be absolute folder addresses".into());
        }
        if let Some(reason) = policy::excluded(&path) {
            return Err(format!("{}: {reason}", filesystem::display_path(&path)));
        }
    }
    if config
        .semantic_floors
        .values()
        .any(|floor| !floor.is_finite() || !(-1.0..=1.0).contains(floor))
    {
        return Err("Semantic relevance thresholds must be finite values from -1 to 1".into());
    }
    Ok(())
}

impl Settings {
    fn restore_ui(&mut self, ui: &mut egui::Ui, busy: bool) -> Option<Command> {
        let mut command = None;
        ui.label("Restore source");
        ui.add(egui::TextEdit::singleline(&mut self.restore_address).desired_width(f32::INFINITY));
        if self.restore_confirm {
            ui.label("Replace the generated index with this backup and pause indexing? Configured folders will be kept. Restored files require fresh verification.");
            if ui
                .add_enabled(!busy, egui::Button::new("Restore index and pause"))
                .clicked()
            {
                match filesystem::decode_path(&self.restore_address) {
                    Ok(path) if path.is_absolute() && path.file_name().is_some() => {
                        self.restore_confirm = false;
                        command = Some(Command::Restore(path));
                    }
                    _ => self.error = Some("Enter an absolute backup filename".into()),
                }
            }
            if ui.button("Cancel restore").clicked() {
                self.restore_confirm = false;
            }
        } else if ui
            .add_enabled(!busy, egui::Button::new("Restore index…"))
            .clicked()
        {
            self.restore_confirm = true;
            self.clear_confirm = false;
            self.recover_confirm = false;
        }
        command
    }

    fn recovery_ui(&mut self, ui: &mut egui::Ui, busy: bool, has_index: bool) -> Option<Command> {
        if self.recover_confirm {
            ui.label("Pause indexing and create an empty cache? The previous database and SQLite sidecars will be preserved in a private recovery folder. Configured folders, models and source files are kept. Use this when the index cannot be opened.");
            if ui
                .add_enabled(!busy, egui::Button::new("Preserve old cache and reset"))
                .clicked()
            {
                self.recover_confirm = false;
                return Some(Command::Recover);
            }
            if ui.button("Cancel recovery").clicked() {
                self.recover_confirm = false;
            }
        } else if ui
            .add_enabled(
                !busy && has_index,
                egui::Button::new("Recover unreadable index…"),
            )
            .clicked()
        {
            self.recover_confirm = true;
            self.clear_confirm = false;
            self.restore_confirm = false;
        }
        None
    }

    fn maintenance_ui(
        &mut self,
        ui: &mut egui::Ui,
        busy: bool,
        has_index: bool,
        outcome: Option<&Result<String, String>>,
    ) -> Option<Command> {
        let mut command = None;
        ui.label("Backup destination");
        ui.add(egui::TextEdit::singleline(&mut self.backup_address).desired_width(f32::INFINITY));
        if ui
            .add_enabled(!busy && has_index, egui::Button::new("Back up index"))
            .clicked()
        {
            match filesystem::decode_path(&self.backup_address) {
                Ok(path) if path.is_absolute() && path.file_name().is_some() => {
                    command = Some(Command::Backup(path))
                }
                _ => self.error = Some("Enter an absolute unused backup filename".into()),
            }
        }
        if busy {
            ui.small("Index maintenance in progress…");
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.clear_confirm {
            ui.label("Clear generated inventory and embeddings, then pause indexing? Enrolled folders and models will be kept. Source files are unchanged.");
            if ui
                .add_enabled(!busy, egui::Button::new("Clear index and pause"))
                .clicked()
            {
                self.clear_confirm = false;
                command = Some(Command::Clear);
            }
            if ui.button("Cancel clear").clicked() {
                self.clear_confirm = false;
            }
        } else if ui
            .add_enabled(
                !busy && has_index,
                egui::Button::new("Clear generated index…"),
            )
            .clicked()
        {
            self.clear_confirm = true;
            self.restore_confirm = false;
            self.recover_confirm = false;
        }
        command = self.restore_ui(ui, busy).or(command);
        command = self.recovery_ui(ui, busy, has_index).or(command);
        if let Some(outcome) = outcome {
            match outcome {
                Ok(message) => {
                    ui.small(format!("Last maintenance: {message}"));
                }
                Err(error) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Last maintenance failed: {error}"),
                    );
                }
            }
        }
        command
    }
    fn show(&mut self, ctx: &egui::Context, state: &ViewState) -> Option<Command> {
        if self.maintenance_pending && !state.busy {
            self.draft.paused = state.paused;
            self.maintenance_pending = false;
        }
        let roots = &state.roots;
        let status = &state.message;
        let error = state.error.as_deref();
        let busy = state.busy;
        let has_index = state.has_index;
        let outcome = state.outcome.as_ref();
        let mut open = self.open;
        let mut command = None;
        egui::Window::new("Search settings").open(&mut open).default_width(540.0).default_height(600.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                ui.label("Nothing is indexed until you enroll a folder with its Index button.");
                ui.checkbox(&mut self.draft.paused, "Pause background indexing");
                ui.checkbox(&mut self.draft.include_hidden, "Include eligible hidden files when indexing");
                ui.checkbox(&mut self.draft.keep_model_loaded, "Keep the last-used model loaded").on_hover_text("Otherwise release the model after five idle minutes. Only one model is resident at a time.");
                ui.small("Protected folders and credential files remain excluded.");
                ui.separator();
                ui.label("Automatic filename-search locations");
                ui.small("One absolute folder address per line. These locations are searched after local matching fails; saving them does not index them.");
                ui.add(egui::TextEdit::multiline(&mut self.locations).desired_rows(3).desired_width(f32::INFINITY));
                ui.collapsing("Semantic relevance thresholds", |ui| {
                    ui.small("Higher values omit weaker meaning matches. Filename and keyword matches remain available. Changing thresholds does not rebuild vectors.");
                    for modality in ["text", "image", "audio"] {
                        ui.add(egui::Slider::new(self.draft.semantic_floors.entry(modality.into()).or_insert_with(|| FileSearchConfig::default_semantic_floor(modality)), -1.0..=1.0).text(modality));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Save preferences").clicked() {
                        self.draft.excluded_paths = self.exclusions.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect();
                        self.draft.always_search = self.locations.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect();
                        match validate(&self.draft) {
                            Ok(()) => { self.error = None; command = Some(Command::Preferences(self.draft.clone())); }
                            Err(error) => self.error = Some(error),
                        }
                    }
                    if ui.button("Close").clicked() { self.open = false; }
                });
                if let Some(error) = self.error.as_deref().or(error) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                } else if !status.is_empty() { ui.small(status); }
                ui.separator();
                ui.collapsing("Excluded search paths", |ui| {
                    ui.small("One exact absolute file or folder address per line. A folder excludes its descendants. Use the resolved address from the browser; no glob syntax. Source files are unchanged.");
                    ui.add(egui::TextEdit::multiline(&mut self.exclusions).id_source("search_excluded_paths").desired_rows(3).desired_width(f32::INFINITY));
                    ui.small("Apply with Save preferences above. Protected exclusions cannot be removed.");
                });
                ui.separator();
                ui.collapsing("Offline results", |ui| {
                    ui.checkbox(&mut self.draft.show_offline_results, "Show offline results");
                    ui.small("Embedding queries can include last-known matches from unavailable indexed folders. Results use a metadata list; reconnect and refresh before opening, copying or collecting them. Save preferences to apply.");
                });
                ui.label("Indexed folders");
                if roots.is_empty() { ui.small("No enrolled folders"); }
                for root in roots {
                    ui.push_id(root.id, |ui| {
                        ui.label(filesystem::display_path(&root.path));
                        ui.small(format!("{} · {} files · {} vectors", root.state, root.files, root.vectors));
                        if let Some(error) = &root.error { ui.small(error); }
                        ui.horizontal(|ui| {
                            if ui.button("Refresh index").clicked() { command = Some(Command::Rebuild(root.path.clone())); }
                            if ui.button("Remove indexed folder").clicked() { command = Some(Command::Remove(root.id)); }
                        });
                    });
                }
                ui.collapsing("Index storage limits", |ui| {
                    ui.small("Soft limits for the database, WAL and pending writes. Model weights are separate. Reaching a limit pauses indexing and preserves existing data.");
                    ui.horizontal(|ui| { ui.label("MiB"); ui.add(egui::DragValue::new(&mut self.draft.limits.max_mib).clamp_range(16..=1_048_576)); });
                    ui.horizontal(|ui| { ui.label("Vectors"); ui.add(egui::DragValue::new(&mut self.draft.limits.max_vectors).clamp_range(1..=100_000_000)); });
                });
                ui.collapsing("Index maintenance", |ui| {
                    command = self.maintenance_ui(ui, busy, has_index, outcome).or(command.take());
                });
            });
        });
        self.open &= open;
        command
    }
}

impl FileManager {
    pub(in crate::file_manager) fn open_search_settings(&mut self) {
        let error = self.search_service().err();
        let draft = self
            .search
            .service
            .as_ref()
            .map(|service| service.snapshot.lock().unwrap().settings.clone())
            .unwrap_or_else(|| self.search.settings.clone());
        self.search.preferences = Settings {
            open: true,
            locations: draft.always_search.join("\n"),
            exclusions: draft.excluded_paths.join("\n"),
            draft,
            error,
            backup_address: filesystem::display_path(
                &self.path().join("file-search-backup.sqlite"),
            ),
            clear_confirm: false,
            recover_confirm: false,
            restore_address: String::new(),
            restore_confirm: false,
            maintenance_pending: false,
        };
    }
    pub(in crate::file_manager) fn search_settings_ui(&mut self, ctx: &egui::Context) {
        if !self.search.preferences.open {
            return;
        }
        let state = self
            .search
            .service
            .as_ref()
            .map(|service| {
                let snapshot = service.snapshot.lock().unwrap();
                ViewState {
                    roots: snapshot.roots.clone(),
                    message: snapshot.message.clone(),
                    error: snapshot.capacity.clone().or_else(|| snapshot.error.clone()),
                    busy: snapshot.maintenance,
                    paused: snapshot.settings.paused,
                    has_index: snapshot.has_index,
                    outcome: snapshot.maintenance_outcome.clone(),
                }
            })
            .unwrap_or_default();
        if let Some(command) = self.search.preferences.show(ctx, &state) {
            let maintenance = matches!(
                &command,
                Command::Restore(_) | Command::Clear | Command::Recover
            );
            match self
                .search_service()
                .and_then(|service| service.send(command))
            {
                Ok(()) => self.search.preferences.maintenance_pending |= maintenance,
                Err(error) => self.search.preferences.error = Some(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_completion_uses_acknowledged_pause_including_failure() {
        let context = egui::Context::default();
        let mut settings = Settings::default();
        for paused in [false, true] {
            settings.maintenance_pending = true;
            let busy = ViewState {
                busy: true,
                paused,
                ..Default::default()
            };
            let _ = context.run(Default::default(), |context| {
                settings.show(context, &busy);
            });
            assert!(settings.maintenance_pending);
            let completed = ViewState {
                paused,
                ..Default::default()
            };
            let _ = context.run(Default::default(), |context| {
                settings.show(context, &completed);
            });
            assert!(!settings.maintenance_pending);
            assert_eq!(settings.draft.paused, paused);
        }
    }

    #[test]
    fn preferences_cannot_opt_in_protected_locations_or_invalid_thresholds() {
        let mut config = FileSearchConfig::default();
        assert!(validate(&config).is_ok());
        for path in [
            "/proc",
            "/home/test/.ssh",
            "/home/test/project/.git",
            "relative",
        ] {
            config.always_search = vec![path.into()];
            assert!(validate(&config).is_err(), "{path}");
        }
        config.always_search = vec!["/home/test/Documents".into()];
        assert!(validate(&config).is_ok());
        for floor in [f32::NAN, f32::INFINITY, -1.1, 1.1] {
            config.semantic_floors.insert("text".into(), floor);
            assert!(validate(&config).is_err());
        }
        config.semantic_floors.clear();
        config.limits.max_vectors = 0;
        assert!(validate(&config).is_err());
        config.limits.max_vectors = 1;
        config.limits.max_mib = 15;
        assert!(validate(&config).is_err());
    }
}
