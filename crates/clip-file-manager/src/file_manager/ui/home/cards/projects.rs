//! A movable desk area displays only visible registered-project task previews.
use super::*;
use crate::file_manager::home::projects::{Projects, Record, Snapshot, View};

impl Renderer<'_, '_> {
    pub fn project_area(
        &self,
        ui: &mut Ui,
        card: &Card,
        projects: &mut Projects,
        actions: &mut Vec<Action>,
    ) {
        let rect = self.viewport.bounds(card.bounds());
        if !rect.intersects(self.viewport.rect) {
            return;
        }
        let scale = self.viewport.scale() as f32;
        let body = Rect::from_min_max(
            rect.min + vec2(14.0, 52.0) * scale,
            rect.max - vec2(14.0, 18.0) * scale,
        );
        let mut live = ui.child_ui(body, egui::Layout::top_down(egui::Align::Min));
        live.set_clip_rect(body.intersect(ui.clip_rect()));
        let style = live.style_mut();
        style.override_font_id = Some(FontId::proportional((16.0 * scale).clamp(11.0, 48.0)));
        style.spacing.interact_size = vec2(32.0, 24.0) * scale.max(0.6);
        style.spacing.icon_width = (16.0 * scale).max(10.0);
        style.spacing.icon_width_inner = (10.0 * scale).max(6.0);
        style.wrap = Some(false);
        let (records, previews) = projects.preview_parts();
        let records: Vec<_> = records
            .iter()
            .filter(|record| match card.content {
                Content::Project { project, .. } => record.id == project,
                _ => true,
            })
            .collect();
        if records.is_empty() {
            if matches!(card.content, Content::Project { .. }) {
                live.label("This project is no longer registered.");
                live.weak("Register its folder again to restore this view.");
            } else {
                live.label("Registered projects appear here.");
                live.weak("Use Add to create or register a project in its own folder.");
            }
            return;
        }
        live.ctx()
            .request_repaint_after(std::time::Duration::from_secs(2));
        let writable =
            self.enabled && !self.arrange && !previews.writing() && previews.write_error.is_none();
        egui::ScrollArea::vertical()
            .id_source(("home_projects", card.id))
            .min_scrolled_height(0.0)
            .max_height(live.available_height())
            .show_rows(
                &mut live,
                190.0 * scale.max(0.6),
                records.len(),
                |ui, range| {
                    for index in range {
                        let record = records[index];
                        let Some(view) = previews.request(record) else {
                            ui.weak("Scroll to load more project previews");
                            continue;
                        };
                        let (row, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 190.0 * scale.max(0.6)),
                            Sense::hover(),
                        );
                        let mut child = ui.child_ui(row, egui::Layout::top_down(egui::Align::Min));
                        child.set_clip_rect(row.intersect(ui.clip_rect()));
                        let (change, refresh) = child
                            .push_id((card.id, record.id), |ui| {
                                project_row(ui, record, view, writable, !self.arrange, actions)
                            })
                            .inner;
                        if refresh {
                            previews.refresh(record.id);
                        }
                        if let Some((index, checked)) = change {
                            previews.toggle(record.id, index, checked);
                        }
                    }
                },
            );
    }
}

fn project_row(
    ui: &mut Ui,
    record: &Record,
    view: &View,
    writable: bool,
    browse: bool,
    actions: &mut Vec<Action>,
) -> (Option<(usize, bool)>, bool) {
    let mut refresh = false;
    ui.horizontal(|ui| {
        let title = view
            .snapshot
            .as_ref()
            .map_or(record.title.as_str(), |snapshot| snapshot.title.as_str());
        if ui
            .add_enabled(browse, egui::Link::new(egui::RichText::new(title).strong()))
            .on_hover_text(record.folder.display().to_string())
            .clicked()
        {
            actions.push(Action::OpenFolder(record.folder.clone()));
        }
        refresh = ui.small_button("Reload TODO").clicked();
        if let Some(snapshot) = &view.snapshot
            && ui
                .add_enabled(browse, egui::Button::new("Open TODO").small())
                .clicked()
        {
            actions.push(Action::OpenDocument(snapshot.path.clone()));
        }
    });
    if let Some(error) = &view.error {
        ui.colored_label(Color32::from_rgb(174, 48, 54), error)
            .on_hover_text(error);
    }
    let change = if let Some(snapshot) = &view.snapshot {
        task_checkboxes(ui, snapshot, writable && view.error.is_none())
    } else {
        if view.error.is_none() {
            ui.spinner();
        }
        None
    };
    ui.separator();
    (change, refresh)
}
fn task_checkboxes(ui: &mut Ui, snapshot: &Snapshot, writable: bool) -> Option<(usize, bool)> {
    let mut change = None;
    ui.add_enabled_ui(writable, |ui| {
        for (index, task) in snapshot.tasks.iter().take(4).enumerate() {
            let mut checked = task.checked;
            if ui
                .checkbox(&mut checked, &task.label)
                .on_hover_text(&task.label)
                .changed()
            {
                change = Some((index, checked));
            }
        }
    });
    if snapshot.tasks.len() > 4 || snapshot.truncated {
        ui.weak(format!(
            "{}{} tasks · Open TODO for the complete document",
            snapshot.tasks.len(),
            if snapshot.truncated { "+" } else { "" }
        ));
    }
    if snapshot.tasks.is_empty() {
        ui.weak("No Markdown checkboxes in this document");
    }
    change
}
