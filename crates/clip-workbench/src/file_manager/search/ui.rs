use crate::file_manager::{
    FileManager,
    ui::{key, list_id},
};
use egui::{Id, Key, Modifiers, Ui};

pub(in crate::file_manager) fn input_id() -> Id {
    Id::new("file_manager_search")
}
impl FileManager {
    pub(in crate::file_manager) fn search_columns(
        &mut self,
        ui: &mut Ui,
        header: egui::Rect,
        colors: crate::ui_appearance::WorkbenchColors,
    ) -> crate::file_manager::ui::ListColumns {
        use super::query::{Sort, SortField};
        use crate::file_manager::ui::{ListColumns, paint_text};
        let count = self.search.answers.count() + usize::from(self.search.images.active());
        let columns = if count == 1 {
            ListColumns::new(header).with_extra()
        } else {
            ListColumns::new(header).with_extras(count)
        };
        self.answer_headers(
            ui,
            header,
            columns,
            usize::from(self.search.images.active()),
            colors,
        );
        if !self.search.images.active() {
            return columns;
        }
        let Some(cell) = columns.extra_cell(header) else {
            return columns;
        };
        let field = if self.search.images.heading() == "Distance" {
            SortField::Similarity
        } else {
            SortField::Dhash
        };
        let active = self
            .search
            .applied
            .as_ref()
            .and_then(|query| query.sort)
            .filter(|sort| {
                sort.field == field
                    && self
                        .search
                        .answers
                        .current()
                        .is_none_or(|folder| folder.sort.is_none())
            });
        paint_text(
            ui,
            cell,
            0.0,
            cell.width(),
            self.search.images.heading(),
            if active.is_some() {
                colors.accent
            } else {
                colors.text_secondary
            },
        );
        if let Some(sort) = active {
            let icon = if sort.descending {
                crate::icons::Icon::ArrowDown
            } else {
                crate::icons::Icon::ArrowUp
            };
            icon.paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    egui::pos2(cell.left() + 80.0, cell.center().y),
                    egui::vec2(14.0, 14.0),
                ),
                colors.accent,
            );
        }
        if ui
            .interact(cell, Id::new("image_hash_sort"), egui::Sense::click())
            .on_hover_text("Sort by image hash/distance; click again to reverse")
            .clicked()
        {
            if let Some(folder) = self.search.answers.current_mut() {
                folder.sort = None;
            }
            if let Some(query) = &mut self.search.applied {
                query.sort = Some(Sort {
                    field,
                    descending: active.is_some_and(|sort| !sort.descending),
                });
            }
            self.rebuild_visible();
        }
        columns
    }
    pub(in crate::file_manager) fn search_navigation_keys(&mut self, ctx: &egui::Context) -> bool {
        // egui menu_button menus are separate from Memory's popup registry.
        // Leave Escape/Enter to the menu and its text fields while it is open.
        if self.search.controls.visible {
            return true;
        }
        let focused = ctx.memory(|memory| memory.focused());
        let find_target = [None, Some(list_id()), Some(input_id())].contains(&focused)
            && !ctx.memory(|memory| memory.any_popup_open());
        if find_target && key(ctx, Modifiers::CTRL, Key::F) {
            let literal = self.search.literal;
            self.search.open(self.search.draft.clone());
            self.search.literal = literal;
            self.type_navigation.clear();
            return true;
        }
        if self.search.open && find_target && key(ctx, Modifiers::NONE, Key::Escape) {
            self.close_search();
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
            return true;
        }
        if self.search.focus || focused == Some(input_id()) {
            return true;
        }
        self.type_navigation_keys(ctx);
        self.search.focus
    }

    pub(in crate::file_manager) fn search_ui(&mut self, ui: &mut Ui) {
        if !self.search.open {
            return;
        }
        let mut submit = false;
        let mut close = false;
        ui.horizontal(|ui| {
            ui.label(if self.search.automatic {
                "Find · automatic scope"
            } else {
                "Find"
            });
            let output = egui::TextEdit::singleline(&mut self.search.draft)
                .id(input_id())
                .hint_text(":image,10kb..2MB name  ·  :emb:\"description\"")
                .desired_width((ui.available_width() - 300.0).max(80.0))
                .show(ui);
            if self.search.focus {
                self.search.focus = false;
                output.response.request_focus();
            }
            if self.search.automatic && output.response.changed() {
                self.search.edited(ui.input(|input| input.time));
            }
            submit =
                output.response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            if ui.add(egui::Button::new("Literal").selected(self.search.literal))
                .on_hover_text("Treat the whole input as a filename, including a leading colon. Changes apply when you press Search.")
                .clicked() {
                self.search.literal = !self.search.literal;
                self.search.focus = true;
                self.search.automatic = false;
                self.search.edit_at = None;
            }
            let mut options_visible = false;
            ui.menu_button("Options", |ui| {
                options_visible = true;
                if let Some(command) = self.search.controls.ui(ui, &self.search.draft, self.search.literal) {
                    self.search.draft = command;
                    self.search.literal = false;
                    self.search.automatic = false;
                    self.search.edit_at = None;
                    self.search.error = None;
                    self.search.focus = true;
                    ui.close_menu();
                }
            });
            self.search.controls.visible = options_visible;
            submit |= ui.button("Search").clicked();
            close = ui.button("Close").clicked();
        });
        let live = self.search.live_submission(ui.ctx()) && !submit;
        submit |= live;
        self.search_status(ui);
        if submit {
            ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
            self.search.edit_at = None;
            self.apply_search();
            if !live {
                ui.memory_mut(|memory| memory.request_focus(list_id()));
            }
        }
        if close {
            self.close_search();
            ui.memory_mut(|memory| memory.request_focus(list_id()));
        }
        if let Some(service) = &self.search.service {
            service.set_repaint_context(ui.ctx());
        }
    }

    fn search_status(&mut self, ui: &mut Ui) {
        if let Some(error) = &self.search.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        } else if self.search.edit_at.is_some() {
            ui.small("Updating filename search…");
        } else if self.search.pending {
            let checked = self.search.service.as_ref().and_then(|service| {
                service
                    .snapshot
                    .lock()
                    .unwrap()
                    .checked_segments(self.search.request_id)
            });
            if let Some(checked) = checked {
                ui.small(format!("Searching · {checked} indexed parts checked…"));
            } else {
                ui.small(if self.search.automatic {
                    "No local matches · searching subtree and configured locations…"
                } else {
                    "Searching…"
                });
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.search.images.active() {
            if let Some(directory) = &self.directory {
                ui.small(self.search.images.status(directory, &self.visible));
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.search.applied.is_some() {
            ui.small(format!(
                "{} of {} matching entries · {}",
                self.visible.len(),
                self.search.matching_count,
                if self.search.message.is_empty() {
                    "Current folder"
                } else {
                    &self.search.message
                }
            ));
            if self.visible.len() < self.search.matching_count && ui.button("Load more").clicked() {
                self.search.result_pages = self.search.result_pages.saturating_add(1);
                self.rebuild_visible();
            }
        }
    }

    pub(in crate::file_manager) fn index_ui(&mut self, ui: &mut Ui) {
        use super::{policy, service::Command};
        let path = self.path().to_path_buf();
        let (roots, message, error, backend) = self
            .search
            .service
            .as_ref()
            .map(|service| {
                let snapshot = service.snapshot.lock().unwrap();
                (
                    snapshot.roots.clone(),
                    snapshot.message.clone(),
                    snapshot.capacity.clone().or_else(|| snapshot.error.clone()),
                    snapshot.backend.clone(),
                )
            })
            .unwrap_or_default();
        let mount = self
            .directory
            .as_ref()
            .filter(|directory| directory.path == path)
            .and_then(|directory| directory.mount);
        let root = roots
            .iter()
            .filter(|root| root.covers(&path, mount))
            .max_by_key(|root| root.path.components().count());
        let user_excluded = policy::exclusions(&self.search.settings.excluded_paths)
            .map_or(true, |exclusions| {
                policy::excluded_by_user(&path, &exclusions)
            });
        let blocked =
            policy::excluded(&path).or(user_excluded.then_some("Excluded in Search settings"));
        let color = match root
            .filter(|_| blocked.is_none())
            .map(|root| root.state.as_str())
        {
            Some("ready") if mount.is_some() && root.is_some_and(|root| root.mount == mount) => {
                egui::Color32::from_rgb(70, 185, 95)
            }
            Some(_) => egui::Color32::from_rgb(225, 180, 55),
            None => egui::Color32::from_rgb(225, 80, 80),
        };
        let tooltip = blocked.map(str::to_owned).unwrap_or_else(|| match root {
            None => "Not indexed. Click to enroll this folder and its eligible subfolders.".into(),
            Some(root) => format!(
                "{} · {} files · {} vectors\n{}\n{}",
                root.state,
                root.files,
                root.vectors,
                root.error.as_deref().unwrap_or_default(),
                message
            ),
        });
        let response = ui
            .add_enabled(
                blocked.is_none(),
                egui::Button::new(egui::RichText::new("Index").color(color)),
            )
            .on_hover_text(tooltip);
        let popup = ui.make_persistent_id("folder_index_settings");
        let mut command = None;
        if response.clicked() {
            if root.is_none() {
                command = Some(Command::Enroll(path.clone()));
            }
            ui.memory_mut(|memory| memory.toggle_popup(popup));
        }
        egui::popup::popup_below_widget(ui, popup, &response, |ui| {
            command = index_menu(ui, root, &message, error.as_deref(), backend.as_deref())
                .or(command.take());
        });
        if let Some(command) = command
            && let Err(error) = self
                .search_service()
                .and_then(|service| service.send(command))
        {
            self.search.error = Some(error);
        }
        if roots.iter().any(|root| root.state != "ready") || self.search.pending {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

impl super::image_order::ImageOrder {
    pub(in crate::file_manager) fn paint_cell(
        &self,
        ui: &Ui,
        columns: crate::file_manager::ui::ListColumns,
        row: egui::Rect,
        entry: &crate::file_manager::filesystem::Entry,
        colors: crate::ui_appearance::WorkbenchColors,
    ) {
        if !self.active() {
            return;
        }
        if let Some(cell) = columns.extra_cell(row) {
            crate::file_manager::ui::paint_text(
                ui,
                cell,
                0.0,
                cell.width() - 8.0,
                &self.cell(entry),
                colors.text_secondary,
            );
        }
    }
}

fn index_menu(
    ui: &mut Ui,
    root: Option<&super::store::RootState>,
    message: &str,
    error: Option<&str>,
    backend: Option<&str>,
) -> Option<super::service::Command> {
    use super::service::Command;
    let mut command = None;
    ui.set_max_width(380.0);
    ui.label("Local semantic search");
    if let Some(backend) = backend {
        ui.small(format!("Inference: {backend}"));
    }
    if !message.is_empty() {
        ui.small(message);
    }
    if let Some(error) = error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.separator();
    if let Some(root) = root {
        ui.label(format!("{} files · {} vectors", root.files, root.vectors));
        if ui.button("Rebuild index").clicked() {
            command = Some(Command::Rebuild(root.path.clone()));
            ui.close_menu();
        }
        if ui.button("Remove indexed folder").clicked() {
            command = Some(Command::Remove(root.id));
            ui.close_menu();
        }
    }
    if ui.button("Install embedding model and runtime").on_hover_text("Downloads checksum-verified model weights (about 865 MB) and builds the pinned local inference runtime.").clicked() {
                command = Some(Command::Setup); ui.close_menu();
            }
    if ui.button("Install answer model").on_hover_text("Downloads the pinned local Qwen3.5-9B Q8 model (about 9.8 GB) for temporary question/answer columns.").clicked() {
        command = Some(Command::SetupAnswer); ui.close_menu();
    }
    command
}
