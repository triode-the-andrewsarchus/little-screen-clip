//! Folder drops ask for intent inline; project creation reuses its guarded form.
use super::*;
use crate::file_manager::home::drops::Item;
impl FileManager {
    pub(super) fn home_folder_drop(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let Some(Item::Folder(folder, manifest)) = self.home_surface.drops.items.last() else {
            return;
        };
        let folder = folder.clone();
        let manifest = *manifest;
        let mut choice = None;
        ui.group(|ui| {
            ui.label("Add dropped folder");
            ui.add(egui::Label::new(display_path(&folder)).wrap(true));
            ui.add_enabled_ui(self.home_surface.ready(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(
                            clip_ui::buttons::ButtonRole::Secondary
                                .button("Folder preview", palette),
                        )
                        .clicked()
                    {
                        choice = Some(false);
                    }
                    if ui
                        .add_enabled(
                            self.home_surface.projects.ready(),
                            clip_ui::buttons::ButtonRole::Secondary.button("Project", palette),
                        )
                        .clicked()
                    {
                        choice = Some(true);
                    }
                })
            });
            if ui.button("Cancel drop").clicked() {
                self.home_surface.cancel_drop();
            }
        });
        if let Some(project) = choice {
            self.home_surface.drops.items.pop();
            if project {
                self.home_surface.drops.project = Some(folder.clone());
                if !self
                    .home_surface
                    .projects
                    .records()
                    .iter()
                    .any(|record| record.folder == folder)
                {
                    if manifest {
                        self.home_surface.projects.register(folder, None);
                    } else {
                        let title = folder.file_name().map_or_else(
                            || "Project".into(),
                            |name| name.to_string_lossy().into_owned(),
                        );
                        self.home_surface.projects.form = Some((folder, title));
                    }
                }
            } else {
                self.home_surface
                    .add_folder(folder, self.home_surface.drops.point);
            }
        }
    }
}
