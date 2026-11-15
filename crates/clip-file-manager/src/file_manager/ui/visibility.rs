//! Visibility is a background and explicit label, separate from status and sets.
use super::super::visibility::Rules;
use super::*;
impl FileManager {
    pub(super) fn visibility_controls(&mut self, ui: &mut Ui) {
        let mut enabled = !self.visibility.ignored;
        ui.menu_button(if enabled { "Rules: on" } else { "Rules: off" }, |ui| {
            ui.set_min_width(340.0);
            ui.label(format!(
                "{} inherited patterns",
                self.visibility.snapshot.count()
            ));
            if ui.checkbox(&mut enabled, "Apply .hide / .show").changed() {
                self.visibility.ignored = !enabled;
                self.rebuild_visible();
            }
            if ui.button("Reveal all files for recovery").clicked() {
                self.visibility.ignored = true;
                self.show_hidden = true;
                self.rebuild_visible();
                ui.close_menu();
            }
            if ui.button("Reload visibility rules").clicked() {
                self.visibility.reload();
            }
            if self.visibility.active() {
                ui.spinner();
            }
            let sources: Vec<_> = self
                .visibility
                .snapshot
                .sources()
                .map(PathBuf::from)
                .collect();
            egui::ScrollArea::vertical()
                .id_source("visibility-sources")
                .max_height(180.0)
                .show(ui, |ui| {
                    for path in sources {
                        let full = display_path(&path);
                        let label: String = full
                            .chars()
                            .rev()
                            .take(48)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                        if ui
                            .button(format!("Open {label}"))
                            .on_hover_text(full)
                            .clicked()
                        {
                            self.open_path(path);
                            ui.close_menu();
                        }
                    }
                });
            ui.small("Folder rules never change file permissions or content.");
        });
    }
    pub(super) fn visibility_feedback(&mut self, ui: &mut Ui) {
        if let Some(error) = &self.visibility.error {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Visibility rules: {error}"),
            );
            ui.small("Normal visibility is in use. Rules → Reveal all files can expose hidden rule files for repair.");
        }
    }
}
fn tint(ui: &Ui, show: bool) -> Color32 {
    match (ui.visuals().dark_mode, show) {
        (true, true) => Color32::from_rgb(27, 53, 49),
        (true, false) => Color32::from_rgb(67, 43, 34),
        (false, true) => Color32::from_rgb(219, 239, 230),
        (false, false) => Color32::from_rgb(248, 228, 212),
    }
}
pub(super) fn background(
    ui: &Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    rules: &Rules,
    mut colors: WorkbenchColors,
) -> WorkbenchColors {
    if let Some(effect) = rules.effect(&entry.path, entry.directory) {
        let fill = tint(ui, effect.show);
        ui.painter().rect_filled(rect, 3.0, fill);
        colors.raised = fill;
        colors.recessed = fill;
    }
    colors
}
pub(super) fn label(
    ui: &Ui,
    rect: Rect,
    entry: &super::super::filesystem::Entry,
    rules: &Rules,
    ignored: bool,
) -> f32 {
    let Some(effect) = rules.effect(&entry.path, entry.directory) else {
        return 0.0;
    };
    if rect.width() < 80.0 || rect.height() < 28.0 {
        return 0.0;
    }
    let width = if rect.width() < 180.0 {
        40.0
    } else if ignored {
        90.0
    } else {
        60.0
    };
    let label = if effect.show { "Shown" } else { "Hidden" };
    let label = if ignored && rect.width() >= 180.0 {
        format!("{label} · off")
    } else {
        label.into()
    };
    let badge = Rect::from_min_size(
        pos2(rect.right() - width - 4.0, rect.top() + 4.0),
        vec2(width, 18.0),
    );
    let painter = ui.painter_at(badge.intersect(ui.clip_rect()));
    painter.rect_filled(badge, 2.0, tint(ui, effect.show));
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        ui.visuals().text_color(),
    );
    ui.interact(
        badge,
        ui.id().with(("visibility-source", &entry.path)),
        Sense::hover(),
    )
    .on_hover_text(format!(
        "{} rule: {}{}",
        if effect.show { "Show" } else { "Hide" },
        display_path(effect.source),
        if ignored {
            " (ignored for recovery)"
        } else {
            ""
        }
    ));
    width + 6.0
}
