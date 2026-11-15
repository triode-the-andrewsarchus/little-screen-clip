//! Reference failures describe an already finished file operation.
use super::super::reference_updates::Deferred;
use super::*;
fn recovery(ui: &mut Ui, updates: &mut Deferred, name: &str) -> bool {
    let Some(error) = updates.error.as_deref() else {
        return false;
    };
    ui.colored_label(
        ui.visuals().error_fg_color,
        format!("File operation finished; {name} references could not be updated: {error}"),
    );
    ui.horizontal(|ui| {
        if ui.button(format!("Retry {name} reference updates")).clicked() {updates.retry();return true;}
        if ui.button(format!("Skip pending {name} reference updates")).on_hover_text("Keep metadata already saved; leave remaining references unchanged. The completed file operation is not repeated.").clicked() {updates.skip();return true;}
        false
    }).inner
}
impl FileManager {
    pub(super) fn reference_update_feedback(&mut self, ui: &mut Ui) {
        if recovery(ui, &mut self.home_surface.relocations, "Home") {
            self.home_surface.error = None;
            self.home_surface.reload();
        }
        if recovery(ui, &mut self.organization.relocations, "organization") {
            self.organization.error = None;
            self.organization.refresh_if_clean();
        }
        if recovery(ui, &mut self.home_surface.projects.relocations, "project") {
            self.home_surface.projects.error = None;
            self.home_surface.projects.refresh();
        }
    }
}
