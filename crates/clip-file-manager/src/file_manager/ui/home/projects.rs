//! Project controls remain inline; project content stays in its own folder.
use super::*;

impl FileManager {
    pub(super) fn home_project_add_actions(&mut self, ui: &mut Ui, can_pin: bool) {
        let mut placement = None;
        ui.menu_button("Place a project", |ui| {
            if self.home_surface.projects.records().is_empty() {
                ui.weak("Create or register a project below first.");
            }
            for project in self.home_surface.projects.records() {
                ui.strong(&project.title);
                ui.label(project.folder.display().to_string());
                ui.horizontal(|ui| {
                    if ui.button("Project folder").clicked() {
                        placement = Some((project.clone(), false));
                        ui.close_menu();
                    }
                    if ui.button("TODO list").clicked() {
                        placement = Some((project.clone(), true));
                        ui.close_menu();
                    }
                });
                ui.separator();
            }
        });
        if let Some((project, tasks)) = placement {
            let session = &mut self.home_surface.session;
            let [x, y] = session.spawn;
            if tasks {
                session.edit(|board| {
                    board.cards.push(crate::file_manager::home::Card {
                        id: Uuid::new_v4(),
                        rect: [x - 260.0, y - 140.0, 520.0, 280.0],
                        content: Content::Project {
                            project: project.id,
                            title: project.title,
                        },
                    })
                });
            } else {
                self.home_surface
                    .add(vec![super::Pin::new(project.folder, true)]);
            }
            ui.close_menu();
        }
        let can_project = can_pin
            && self.home_surface.projects.ready()
            && self
                .directory
                .as_ref()
                .is_some_and(|directory| !directory.system);
        ui.separator();
        ui.strong("Project destination");
        ui.add(
            egui::Label::new(egui::RichText::new(self.path().display().to_string()).monospace())
                .wrap(true),
        );
        ui.weak("Applies to the folder shown above.");
        if ui
            .add_enabled(
                can_project,
                egui::Button::new("Create project in current folder…"),
            )
            .clicked()
        {
            let folder = self.path().to_path_buf();
            let title = folder
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "My project".into());
            self.home_surface.projects.form = Some((folder, title));
            ui.close_menu();
        }
        if ui
            .add_enabled(
                can_project,
                egui::Button::new("Register current folder's project"),
            )
            .clicked()
        {
            self.home_surface
                .projects
                .register(self.path().to_path_buf(), None);
            ui.close_menu();
        }
    }
    pub(super) fn home_projects_menu(&mut self, ui: &mut Ui) {
        let mut open = None;
        let projects = &mut self.home_surface.projects;
        ui.menu_button("Projects", |ui| {
            if projects.records().is_empty() {
                ui.weak("No registered projects");
            }
            let mut remove = None;
            for project in projects.records() {
                ui.horizontal(|ui| {
                    if ui
                        .button(&project.title)
                        .on_hover_text(project.folder.display().to_string())
                        .clicked()
                    {
                        open = Some(project.folder.clone());
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(projects.ready(), egui::Button::new("Unregister"))
                        .on_hover_text("Remove the local reference; keep every project file")
                        .clicked()
                    {
                        remove = Some(project.id);
                        ui.close_menu();
                    }
                });
            }
            if let Some(id) = remove {
                projects.remove(id);
            }
            ui.separator();
            if ui
                .add_enabled(!projects.busy(), egui::Button::new("Refresh project list"))
                .clicked()
            {
                projects.refresh();
                ui.close_menu();
            }
        });
        if let Some(folder) = open
            && self.home_surface.session.allow_leave()
        {
            self.navigate(folder, Navigation::New);
        }
    }
    pub(super) fn home_project_form(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let projects = &mut self.home_surface.projects;
        task_recovery(ui, projects);
        let ready = projects.ready();
        if let Some((folder, title)) = &mut projects.form {
            let mut create = false;
            let mut cancel = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong("Create a project here");
                ui.label(folder.display().to_string());
                ui.label("Existing README/TODO files will be kept. Missing documents and .fmproject will be created in this folder.");
                ui.horizontal(|ui| {
                    ui.label("Title");
                    ui.add(egui::TextEdit::singleline(title).char_limit(128).desired_width(300.0));
                    create = ui.add_enabled(ready && !title.trim().is_empty(), clip_ui::buttons::ButtonRole::Primary.button("Create project", palette)).clicked();
                    cancel = ui.add(clip_ui::buttons::ButtonRole::Secondary.button("Cancel", palette)).clicked();
                });
            });
            if create {
                let (folder, title) = projects.form.take().expect("visible project form");
                projects.register(folder, Some(title));
            } else if cancel {
                projects.form = None;
            }
        }
        if let Some(conflict) = &projects.conflict {
            let mut accept = false;
            let mut cancel = false;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.strong(format!("{} is registered at another location", conflict.project.title));
                ui.label(format!("Current: {}", conflict.previous.display()));
                ui.label(format!("Chosen: {}", conflict.project.folder.display()));
                ui.label("Use the chosen folder for this project ID? The previous folder and its files will be kept.");
                ui.horizontal(|ui| {
                    accept = ui.add_enabled(ready, clip_ui::buttons::ButtonRole::Primary.button("Use this location", palette)).clicked();
                    cancel = ui.add(clip_ui::buttons::ButtonRole::Secondary.button("Cancel", palette)).clicked();
                });
            });
            if accept {
                projects.relocate();
            } else if cancel {
                projects.conflict = None;
            }
        }
        if let Some(error) = &projects.error {
            ui.colored_label(ui.visuals().error_fg_color, format!("Projects: {error}"));
            if ui.button("Reload project registry").clicked() {
                projects.refresh();
            }
        }
    }
}

fn task_recovery(ui: &mut Ui, projects: &mut crate::file_manager::home::projects::Projects) {
    if let Some(error) = &projects.previews.write_error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("TODO save failed: {error}"),
        );
        if ui
            .add_enabled(
                !projects.previews.writing(),
                egui::Button::new("Reload task previews"),
            )
            .clicked()
        {
            let ids: Vec<_> = projects.records().iter().map(|record| record.id).collect();
            for id in ids {
                projects.previews.refresh(id);
            }
            projects.previews.write_error = None;
        }
    }
}
