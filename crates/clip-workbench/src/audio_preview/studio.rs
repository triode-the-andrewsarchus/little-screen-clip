use super::*;

impl AudioPreviewState {
    #[cfg(test)]
    pub(super) fn handle_studio_shortcuts(
        &mut self,
        ctx: &egui::Context,
        close_requested: &mut bool,
    ) -> bool {
        self.handle_studio_shortcuts_with_start(ctx, close_requested, Self::start_stem_separation)
    }

    pub(super) fn handle_studio_shortcuts_with_start(
        &mut self,
        ctx: &egui::Context,
        close_requested: &mut bool,
        mut start_stem_separation: impl FnMut(&mut Self),
    ) -> bool {
        let dialog_open = self.save_dialog_open || self.stem_separation.save_dialog_open;
        let tool_shortcuts_enabled = !dialog_open && !ctx.wants_keyboard_input();
        if tool_shortcuts_enabled {
            // Priority matches the former sequential checks when a frame has
            // more than one pressed key; modified keys belong to other controls.
            let mode = ctx.input(|i| {
                let keys = [
                    (egui::Key::H, AudioToolMode::HfAnalysis),
                    (egui::Key::D, AudioToolMode::DigitalDecode),
                    (egui::Key::S, AudioToolMode::StemSeparation),
                    (egui::Key::I, AudioToolMode::Inspect),
                ];
                (!i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.mac_cmd)
                    .then(|| {
                        keys.into_iter()
                            .find_map(|(key, mode)| i.key_pressed(key).then_some(mode))
                    })
                    .flatten()
            });
            if let Some(mode) = mode {
                self.active_mode = mode;
            }
        }
        // Nested save dialogs handle Escape themselves, leaving the studio open.
        if !dialog_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop();
            *close_requested = true;
            return true;
        }
        let space_pressed = ctx.input(|i| i.key_pressed(egui::Key::Space));
        if should_start_stem_separation_on_space(
            space_pressed,
            tool_shortcuts_enabled,
            self.active_mode,
            self.stem_separation.stems.is_empty(),
            self.stem_separation.in_progress,
        ) {
            start_stem_separation(self);
        }
        false
    }

    /// Renders shared studio chrome, then the selected audio tool workspace.
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        self.render_ui_with_start(
            ui,
            ctx,
            palette,
            close_requested,
            Self::start_stem_separation,
        );
    }

    pub(super) fn render_ui_with_start(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
        mut start_stem_separation: impl FnMut(&mut Self),
    ) {
        let mode_before_frame = self.active_mode;
        self.poll_slice_copy_job(ctx, ctx.input(|input| input.time));
        self.poll_save_job(ctx, ctx.input(|input| input.time));
        self.poll_digital_decode(ctx);
        if self.handle_studio_shortcuts_with_start(ctx, close_requested, &mut start_stem_separation)
        {
            return;
        }
        layout::AudioWorkspace::show(ui, palette, |ui| {
            self.render_audio_header(ui, ctx, palette, close_requested);
            ui.add_space(crate::ui_ext::SPACE_XS);
            self.render_audio_tool_bar(ui, palette);
            if mode_before_frame == AudioToolMode::StemSeparation
                && self.active_mode != AudioToolMode::StemSeparation
            {
                self.cancel_stem_separation();
            } else if mode_before_frame != AudioToolMode::StemSeparation
                && self.active_mode == AudioToolMode::StemSeparation
                && self.stem_separation.stems.is_empty()
                && !self.stem_separation.in_progress
            {
                start_stem_separation(self);
            }
            ui.add_space(crate::ui_ext::SPACE_SM);
            match self.active_mode {
                AudioToolMode::Inspect => {
                    self.render_standard_ui(ui, ctx, palette, close_requested, None)
                }
                AudioToolMode::StemSeparation => {
                    self.render_stem_separation_ui(ui, ctx, palette, close_requested)
                }
                AudioToolMode::DigitalDecode => {
                    self.render_digital_decode_ui(ui, ctx, palette, close_requested)
                }
                AudioToolMode::HfAnalysis => {
                    self.render_hf_analysis_ui(ui, ctx, palette, close_requested)
                }
            }
        });
    }
}
