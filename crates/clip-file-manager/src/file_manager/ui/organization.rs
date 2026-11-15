//! Session controls and shared annotations; no content reads or row-height changes.
use super::super::organization::{Action, Color, Observation, Snapshot};
use super::*;

pub(super) fn ink(color: Color, dark: bool) -> Color32 {
    match (color, dark) {
        (Color::Red, true) => Color32::from_rgb(244, 105, 112),
        (Color::Red, false) => Color32::from_rgb(174, 36, 46),
        (Color::Yellow, true) => Color32::from_rgb(229, 188, 55),
        (Color::Yellow, false) => Color32::from_rgb(139, 95, 0),
        (Color::Green, true) => Color32::from_rgb(89, 194, 135),
        (Color::Green, false) => Color32::from_rgb(23, 121, 73),
    }
}
pub(super) fn paint_session(
    ui: &Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    snapshot: &Snapshot,
    compact: bool,
) {
    let Some(color) = snapshot.color(entry) else {
        return;
    };
    let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
    painter.line_segment(
        [
            pos2(rect.left() + 4.0, rect.bottom() - 1.5),
            pos2(rect.right() - 16.0, rect.bottom() - 1.5),
        ],
        egui::Stroke::new(2.0_f32, ink(color, ui.visuals().dark_mode)),
    );
    painter.text(
        pos2(
            rect.right() - if compact { 27.0 } else { 5.0 },
            rect.center().y,
        ),
        egui::Align2::CENTER_CENTER,
        color.letter(),
        FontId::proportional(11.0),
        ui.visuals().text_color(),
    );
}
impl FileManager {
    pub(super) fn apply_session_action(
        &mut self,
        action: Option<(Observation, bool, Option<Color>)>,
    ) {
        if let Some((target, selected, color)) = action {
            if selected {
                self.assign_session_selection(color);
            } else {
                self.organization
                    .change(Action::Assign(vec![target], color));
            }
        }
    }
    pub(super) fn organization_targets(&self) -> Vec<Observation> {
        let Some(selected) = self.location().map(|location| &location.selection) else {
            return Vec::new();
        };
        self.displayed_entries()
            .into_iter()
            .filter(|entry| selected.contains(&entry.path))
            .filter_map(Observation::for_entry)
            .collect()
    }
    pub(super) fn assign_session_selection(&mut self, color: Option<Color>) {
        let targets = self.organization_targets();
        if targets.len() != self.selection_count() {
            self.organization.error = Some(
                "Refresh the selection before marking: some items have no current metadata".into(),
            );
            return;
        }
        self.organization.change(Action::Assign(targets, color));
    }
    pub(super) fn organization_menu(&mut self, ui: &mut Ui) {
        let can_assign = self.selection_count() > 0
            && self.archive_context().is_none()
            && !self.search.has_offline_results();
        let label = if self.organization.snapshot.enabled() {
            "Session: on"
        } else {
            "Session"
        };
        ui.menu_button(label, |ui| {
            ui.set_min_width(340.0);
            let ready = self.organization.ready();
            let snapshot = self.organization.snapshot.clone();
            let mut enabled = snapshot.enabled();
            if ui
                .add_enabled(
                    ready,
                    egui::Checkbox::new(&mut enabled, "Enable session marks"),
                )
                .changed()
            {
                self.organization.change(Action::Enable(enabled));
            }
            ui.small(format!(
                "{} · {} stored marks",
                snapshot.definitions().name,
                snapshot.count()
            ));
            ui.separator();
            ui.add_enabled_ui(ready && enabled && can_assign, |ui| {
                for color in Color::ALL {
                    let label = snapshot.definitions().label(color);
                    if ui
                        .button(
                            egui::RichText::new(format!("Mark selection: {label}"))
                                .color(ink(color, ui.visuals().dark_mode)),
                        )
                        .clicked()
                    {
                        self.assign_session_selection(Some(color));
                        ui.close_menu();
                    }
                }
                if ui.button("Remove selection marks").clicked() {
                    self.assign_session_selection(None);
                    ui.close_menu();
                }
            });
            self.session_definitions_ui(ui);
            ui.separator();
            self.session_recovery_ui(ui, &snapshot, ready);
        });
    }
    fn session_recovery_ui(&mut self, ui: &mut Ui, snapshot: &Snapshot, ready: bool) {
        if ui
            .add_enabled(
                ready && snapshot.count() > 0,
                egui::Button::new("Clear all session marks"),
            )
            .on_hover_text("Clear assignments only; Restore recovers this set of marks")
            .clicked()
        {
            self.organization.change(Action::Clear);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                ready && snapshot.can_restore(),
                egui::Button::new("Restore marks before last clear"),
            )
            .clicked()
        {
            self.organization
                .change(Action::Restore(snapshot.recovery_revision()));
            ui.close_menu();
        }
        if self.organization.active() {
            ui.spinner();
        }
        if ui
            .add_enabled(
                ready && snapshot.can_restore(),
                egui::Button::new("Forget cleared-mark backup"),
            )
            .clicked()
        {
            self.organization
                .change(Action::ForgetClear(snapshot.recovery_revision()));
            ui.close_menu();
        }
    }
    fn session_definitions_ui(&mut self, ui: &mut Ui) {
        ui.collapsing("Session name and color meanings", |ui| {
            let ready = self.organization.ready();
            let organization = &mut self.organization;
            let draft = organization
                .draft
                .get_or_insert_with(|| organization.snapshot.definitions().clone());
            let mut save = false;
            let mut cancel = false;
            ui.add_enabled_ui(ready, |ui| {
                ui.label("Session name");
                ui.add(egui::TextEdit::singleline(&mut draft.name).char_limit(32));
                for color in Color::ALL {
                    ui.horizontal(|ui| {
                        ui.label(color.name());
                        ui.add(
                            egui::TextEdit::singleline(&mut draft.meanings[color.index()])
                                .char_limit(32)
                                .desired_width(225.0),
                        );
                    });
                }
                ui.horizontal(|ui| {
                    save = ui.button("Save meanings").clicked();
                    cancel = ui.button("Discard definition edits").clicked();
                });
            });
            if save {
                organization.change(Action::Definitions);
            } else if cancel {
                organization.draft = None;
                organization.error = None;
            }
        });
    }
    pub(super) fn organization_feedback(&mut self, ui: &mut Ui) {
        if let Some(error) = self.organization.error.clone() {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Organization: {error}"),
            );
            if ui
                .add_enabled(
                    !self.organization.active(),
                    egui::Button::new("Reload organization state and discard definition edits"),
                )
                .clicked()
            {
                self.organization.reload();
            }
        }
    }
}
impl ListActions {
    pub(super) fn session_mark_menu(
        &mut self,
        ui: &mut Ui,
        entry: &super::super::filesystem::Entry,
        selected: bool,
    ) {
        let enabled = self.organization_ready
            && self.organization.enabled()
            && !self.metadata_only
            && !self.copy_only
            && entry.identity.is_some();
        ui.add_enabled_ui(enabled, |ui| {
            ui.menu_button("Session mark", |ui| {
                for color in Color::ALL {
                    if ui
                        .button(self.organization.definitions().label(color))
                        .clicked()
                    {
                        self.session_mark = Observation::for_entry(entry)
                            .map(|target| (target, selected, Some(color)));
                        ui.close_menu();
                    }
                }
                if ui.button("Remove session mark").clicked() {
                    self.session_mark =
                        Observation::for_entry(entry).map(|target| (target, selected, None));
                    ui.close_menu();
                }
            });
        });
    }
}
