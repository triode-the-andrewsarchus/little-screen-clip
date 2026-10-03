use super::*;

impl WorkbenchApp {
    pub(crate) fn toggle_appearance(&mut self, ctx: &egui::Context) {
        self.toggle_appearance_with(ctx, |appearance| {
            AppConfig::save_workbench_appearance_to_file(appearance)
                .map_err(|error| error.to_string())
        });
    }

    pub(crate) fn handle_appearance_shortcut(&mut self, ctx: &egui::Context) {
        self.handle_appearance_shortcut_with(ctx, |appearance| {
            AppConfig::save_workbench_appearance_to_file(appearance)
                .map_err(|error| error.to_string())
        });
    }

    pub(crate) fn handle_appearance_shortcut_with(
        &mut self,
        ctx: &egui::Context,
        save: impl FnOnce(WorkbenchAppearance) -> Result<(), String>,
    ) {
        if ctx.input(|input| input.key_pressed(egui::Key::F4) || input.key_pressed(egui::Key::F6)) {
            self.toggle_appearance_with(ctx, save);
        }
    }

    pub(crate) fn toggle_appearance_with(
        &mut self,
        ctx: &egui::Context,
        save: impl FnOnce(WorkbenchAppearance) -> Result<(), String>,
    ) {
        self.appearance_sync.clear();
        let appearance = self.config.workbench.appearance.toggled();
        self.config.workbench.appearance = appearance;
        self.ui_palette = ui_appearance::palette(appearance);
        ui_appearance::apply_visuals(ctx, &self.ui_palette);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_config(self.config.clone());
        }
        self.mirror_error = None;
        self.dirty_mirror = true;
        match save(appearance) {
            Ok(()) => {
                self.appearance_sync.begin();
            }
            Err(err) => {
                self.appearance_sync
                    .set_error(format!("Could not save appearance: {err}"));
            }
        }
        ctx.request_repaint();
    }
}
