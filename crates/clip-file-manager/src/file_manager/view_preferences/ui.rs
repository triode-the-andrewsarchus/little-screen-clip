use super::{
    super::{BrowserView, FileManager, filesystem::display_path},
    Preference, Preferences,
};
use clip_ui::buttons::ButtonRole;
use little_clip_core::{BackgroundJobs, palette::AppPalette};
use std::path::Path;

impl BrowserView {
    pub(in crate::file_manager) fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Columns => "Columns",
            Self::Photos => "Photos",
            Self::Constellation => "Constellation",
        }
    }
}
enum MenuAction {
    Remember(Preference),
    Inheritance(Preference),
    Clear,
    Reload,
}
impl Preferences {
    fn description(&self, ui: &mut egui::Ui, path: &Path, current: BrowserView) {
        ui.label(format!("Current view: {}", current.label()));
        if let Some((origin, pref)) = self.state.effective(path) {
            let description = if origin == path {
                format!("Saved here: {}", pref.view.label())
            } else {
                format!(
                    "Inherited {} from {}",
                    pref.view.label(),
                    display_path(origin)
                )
            };
            ui.add(egui::Label::new(&description).truncate(true))
                .on_hover_text(description);
        } else {
            ui.label("No saved view for this folder.");
        }
    }
    fn choices(
        ui: &mut egui::Ui,
        view: BrowserView,
        exact: Option<Preference>,
        ready: bool,
        overridden: bool,
        palette: &AppPalette,
    ) -> Option<MenuAction> {
        if ui
            .add_enabled(
                ready,
                ButtonRole::Primary.button("Remember current view", palette),
            )
            .clicked()
        {
            return Some(MenuAction::Remember(Preference {
                view,
                descendants: exact.is_some_and(|p| p.descendants),
            }));
        }
        if let Some(mut preference) = exact
            && ui
                .add_enabled(
                    ready,
                    egui::Checkbox::new(
                        &mut preference.descendants,
                        "Use saved view in subfolders",
                    ),
                )
                .changed()
        {
            return Some(MenuAction::Inheritance(preference));
        }
        if ui
            .add_enabled(
                ready && overridden,
                ButtonRole::Secondary.button("Use inherited/default view", palette),
            )
            .clicked()
        {
            return Some(MenuAction::Clear);
        }
        None
    }
    fn menu_body(
        &self,
        ui: &mut egui::Ui,
        path: &Path,
        view: BrowserView,
        manual: bool,
        available: bool,
        palette: &AppPalette,
    ) -> Option<MenuAction> {
        ui.set_width(320.0);
        self.description(ui, path, view);
        let exact = self.state.exact(path);
        let mut action = Self::choices(
            ui,
            view,
            exact,
            available && self.ready(),
            manual || exact.is_some(),
            palette,
        );
        ui.small("Saved by folder path on this computer. No files are added to your folders.");
        if self.active() {
            ui.spinner();
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if ui
            .add_enabled(
                !self.active(),
                ButtonRole::Secondary.button("Reload preferences", palette),
            )
            .clicked()
        {
            action = Some(MenuAction::Reload);
        }
        action
    }
}
impl FileManager {
    pub(in crate::file_manager) fn tick_view_preferences(&mut self, jobs: &BackgroundJobs) {
        let update = self.view_preferences.tick(jobs);
        let mut explicit = false;
        if let Some((path, epoch)) = update.applied
            && path == self.path()
            && epoch == self.view_epoch
            && let Some(location) = self.location_mut()
        {
            location.manual_view = None;
            explicit = true;
        }
        if update.changed && self.directory.is_some() && !self.search.has_result_sources() {
            self.apply_saved_view(explicit);
        }
        if let Some(error) = self.view_preferences.report.take() {
            self.set_status(crate::icons::Icon::Alert, error, true);
        }
    }
    fn apply_saved_view(&mut self, explicit: bool) {
        if self.pending.is_some() || self.directory.is_none() || self.search.has_result_sources() {
            return;
        }
        let previous = self.view;
        self.activate_media_view();
        if self.view != previous {
            if explicit && let Some(location) = self.location_mut() {
                location.scroll = 0.0;
            }
            self.restore_scroll = true;
        }
        self.rebuild_list_offsets();
    }
    pub(in crate::file_manager) fn folder_view_settings(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
    ) {
        let path = self.path().to_path_buf();
        let manual = self
            .location()
            .is_some_and(|location| location.manual_view.is_some());
        let available =
            self.directory.is_some() && self.pending.is_none() && !self.search.has_result_sources();
        let action = ui
            .menu_button("Folder view", |ui| {
                self.view_preferences
                    .menu_body(ui, &path, self.view, manual, available, palette)
            })
            .inner
            .flatten();
        match action {
            Some(MenuAction::Remember(preference)) => {
                self.view_preferences
                    .change(path, Some(preference), Some(self.view_epoch))
            }
            Some(MenuAction::Inheritance(preference)) => {
                self.view_preferences.change(path, Some(preference), None)
            }
            Some(MenuAction::Clear) => {
                if self.view_preferences.state.exact(&path).is_some() {
                    self.view_preferences
                        .change(path, None, Some(self.view_epoch));
                } else {
                    if let Some(location) = self.location_mut() {
                        location.manual_view = None;
                    }
                    self.apply_saved_view(true);
                }
            }
            Some(MenuAction::Reload) => self.view_preferences.reload(),
            None => {}
        }
    }
}
