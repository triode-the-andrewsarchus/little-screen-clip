//! Sorting-set controls share organization persistence and the browser row surface.
use super::super::organization::{Action, Filter, Group, SetChange};
use super::*;

// Bound every picker, including context menus, for the full 256-group budget.
fn choose_set(
    ui: &mut Ui,
    snapshot: &super::super::organization::Snapshot,
    selected: Option<uuid::Uuid>,
) -> Option<uuid::Uuid> {
    let mut choice = None;
    egui::ScrollArea::vertical()
        .id_source("sorting-set-choices")
        .max_height(180.0)
        .show(ui, |ui| {
            for group in &snapshot.sets.groups {
                let full = snapshot.sets.label(group.id);
                let label: String = full.chars().take(48).collect();
                if ui
                    .selectable_label(selected == Some(group.id), label)
                    .on_hover_text(full)
                    .clicked()
                {
                    choice = Some(group.id);
                }
            }
        });
    choice
}

impl FileManager {
    pub(super) fn sorting_sets_menu(&mut self, ui: &mut Ui) {
        let snapshot = self.organization.snapshot.clone();
        let filter = self.organization.filter;
        let label = match filter {
            Filter::All => "Sets".into(),
            Filter::Unassigned => "Sets: unassigned".into(),
            Filter::Group(id) => format!(
                "Sets: {}",
                snapshot.sets.group(id).map_or_else(String::new, |g| g
                    .name
                    .chars()
                    .take(20)
                    .collect())
            ),
        };
        ui.menu_button(label, |ui| {
            ui.set_min_width(290.0);
            ui.small("Filter files · folders remain available for navigation");
            let mut next = filter;
            ui.selectable_value(&mut next, Filter::All, "All files");
            ui.selectable_value(&mut next, Filter::Unassigned, "Unassigned files");
            let selected = if let Filter::Group(id) = filter {
                Some(id)
            } else {
                None
            };
            if let Some(id) = choose_set(ui, &snapshot, selected) {
                next = Filter::Group(id);
            }
            if next != filter {
                self.organization.filter = next;
                self.rebuild_visible();
            }
            ui.separator();
            self.set_assignment_controls(ui, &snapshot);
            ui.separator();
            ui.add_enabled_ui(
                self.organization.ready() && self.organization.set_editor.is_none(),
                |ui| {
                    if ui.button("New root set").clicked() {
                        self.organization.set_editor = Some((None, Group::new("New set", None)));
                        ui.close_menu();
                    }
                    if let Filter::Group(id) = filter
                        && let Some(group) = snapshot.sets.group(id)
                    {
                        if ui.button("Subdivide this set").clicked() {
                            self.organization.set_editor =
                                Some((None, Group::new("New subset", Some(id))));
                            ui.close_menu();
                        }
                        if ui.button("Edit this set").clicked() {
                            self.organization.set_editor =
                                Some((Some(group.clone()), group.clone()));
                            ui.close_menu();
                        }
                        if ui
                            .button("Delete this empty set")
                            .on_hover_text("Remove its assignments and child sets first")
                            .clicked()
                        {
                            self.organization
                                .change(Action::Sets(SetChange::Remove(id)));
                            ui.close_menu();
                        }
                    }
                },
            );
        });
    }
    fn set_assignment_controls(
        &mut self,
        ui: &mut Ui,
        snapshot: &super::super::organization::Snapshot,
    ) {
        let enabled = self.organization.ready()
            && self.selection_count() > 0
            && self.archive_context().is_none()
            && !self.search.has_offline_results();
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Assign selection to set", |ui| {
                if let Some(id) = choose_set(ui, snapshot, None) {
                    self.assign_set_selection(Some(id));
                    ui.close_menu();
                }
            });
            if ui.button("Remove selection from sets").clicked() {
                self.assign_set_selection(None);
                ui.close_menu();
            }
        });
    }
    pub(super) fn assign_set_selection(&mut self, group: Option<uuid::Uuid>) {
        let targets = self.organization_targets();
        if targets.len() != self.selection_count() {
            self.organization.error = Some(
                "Refresh the selection before classifying: some items have no current metadata"
                    .into(),
            );
            return;
        }
        self.organization.change(Action::Sets(SetChange::Assign {
            paths: targets.into_iter().map(|t| t.path).collect(),
            group,
        }));
    }
    pub(super) fn apply_set_action(&mut self, action: Option<(PathBuf, bool, Option<uuid::Uuid>)>) {
        if let Some((path, selected, group)) = action {
            if selected {
                self.assign_set_selection(group);
            } else {
                self.organization.change(Action::Sets(SetChange::Assign {
                    paths: vec![path],
                    group,
                }));
            }
        }
    }
    pub(super) fn sorting_set_editor(&mut self, ui: &mut Ui) {
        let ready = self.organization.ready();
        let Some((before, after)) = &mut self.organization.set_editor else {
            return;
        };
        let mut save = false;
        let mut cancel = false;
        ui.group(|ui| {
            ui.label(if before.is_some() {
                "Edit sorting set"
            } else {
                "Create sorting set"
            });
            ui.add_enabled_ui(ready, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut after.name)
                            .char_limit(32)
                            .desired_width(200.0),
                    );
                    ui.color_edit_button_srgb(&mut after.color);
                    save = ui.button("Save set").clicked();
                    cancel = ui.button("Discard set edits").clicked();
                });
            });
        });
        if save {
            let change = match before.clone() {
                Some(before) => SetChange::Edit {
                    before,
                    after: after.clone(),
                },
                None => SetChange::Create(after.clone()),
            };
            self.organization.change(Action::Sets(change));
        } else if cancel {
            self.organization.set_editor = None;
            self.organization.error = None;
        }
    }
}
impl ListActions {
    pub(super) fn sorting_set_context(
        &mut self,
        ui: &mut Ui,
        entry: &super::super::filesystem::Entry,
        selected: bool,
    ) {
        ui.add_enabled_ui(
            self.organization_ready
                && !self.metadata_only
                && !self.copy_only
                && entry.identity.is_some(),
            |ui| {
                ui.menu_button("Sorting set", |ui| {
                    if let Some(id) = choose_set(ui, &self.organization, None) {
                        self.set_assignment = Some((entry.path.clone(), selected, Some(id)));
                        ui.close_menu();
                    }
                    if ui.button("Remove from sets").clicked() {
                        self.set_assignment = Some((entry.path.clone(), selected, None));
                        ui.close_menu();
                    }
                });
            },
        );
    }
}
pub(super) fn paint_set(
    ui: &Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    snapshot: &super::super::organization::Snapshot,
) {
    let Some(group) = snapshot.sets.assigned(&entry.path) else {
        return;
    };
    let font = FontId::proportional(11.0);
    let text = ui.painter().layout_no_wrap(
        group.name.chars().take(20).collect(),
        font,
        ui.visuals().text_color(),
    );
    let width = (text.size().x + 12.0).min(rect.width() / 3.0);
    let badge = Rect::from_min_size(
        pos2(rect.right() - width - 22.0, rect.top() + 2.0),
        vec2(width, 16.0),
    );
    let painter = ui.painter_at(badge.intersect(ui.clip_rect()));
    painter.rect(
        badge,
        3.0,
        ui.visuals().window_fill(),
        egui::Stroke::new(
            1.5_f32,
            Color32::from_rgb(group.color[0], group.color[1], group.color[2]),
        ),
    );
    painter.galley(badge.min + vec2(5.0, 1.0), text, ui.visuals().text_color());
    ui.interact(
        badge,
        ui.id().with(("sorting-set", &entry.path)),
        egui::Sense::hover(),
    )
    .on_hover_text(format!("Sorting set: {}", snapshot.sets.label(group.id)));
}
