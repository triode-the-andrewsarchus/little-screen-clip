use super::*;

impl WorkbenchApp {
    pub(crate) fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        let audio_active = self.is_audio_region_active();
        if self.handle_escape_shortcut(ctx, audio_active) {
            return;
        }
        self.handle_audio_strip_shortcuts(ctx, audio_active);
        self.handle_view_switching_shortcuts(ctx);
        self.handle_palette_shortcuts(ctx, audio_active);
    }

    pub(crate) fn handle_escape_shortcut(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
    ) -> bool {
        if !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return false;
        }
        match route_workbench_escape(
            &mut self.view,
            &mut self.canvas_state,
            &mut self.palette_search_needs_focus,
            audio_region_active,
        ) {
            WorkbenchEscapeRoute::Handled | WorkbenchEscapeRoute::ReturnToPalette => {}
            WorkbenchEscapeRoute::StopAudio => {
                if let Some(strip) = &mut self.rolling_audio_strip {
                    strip.selected_region = None;
                    strip.stop();
                }
            }
            WorkbenchEscapeRoute::CloseWorkbench => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        true
    }

    pub(crate) fn handle_audio_strip_shortcuts(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
    ) {
        if self.view != WorkbenchView::Palette {
            return;
        }
        self.handle_audio_strip_playback(ctx, audio_region_active);
        self.handle_audio_strip_copy(ctx, audio_region_active);
        self.handle_audio_strip_refresh(ctx);
    }

    pub(crate) fn handle_audio_strip_playback(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
    ) {
        if audio_strip_space_pressed(ctx, audio_region_active) {
            if let Some(strip) = &mut self.rolling_audio_strip {
                strip.toggle_playback();
            }
        }
    }

    pub(crate) fn handle_audio_strip_copy(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
    ) {
        if !audio_region_active || ctx.wants_keyboard_input() {
            return;
        }
        let copy_pressed = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::C));
        if !copy_pressed {
            return;
        }
        if let Some(strip) = &mut self.rolling_audio_strip {
            if strip.selected_region.is_some() {
                let now = ctx.input(|i| i.time);
                if strip.copy_selection(now) {
                    if let Ok(IpcResponse::State(s)) = send_ipc_request(&IpcRequest::GetState) {
                        self.state = s;
                    }
                    self.dirty_mirror = true;
                }
            }
        }
    }

    pub(crate) fn handle_audio_strip_refresh(&mut self, ctx: &egui::Context) {
        self.handle_audio_strip_refresh_with_load(ctx, RollingAudioStripState::load_snapshot);
    }

    pub(crate) fn handle_audio_strip_refresh_with_load(
        &mut self,
        ctx: &egui::Context,
        mut load_snapshot: impl FnMut(&mut RollingAudioStripState),
    ) {
        let refresh_pressed = ctx.input(|i| {
            i.key_pressed(egui::Key::F5) || (i.modifiers.command && i.key_pressed(egui::Key::R))
        });
        if !refresh_pressed {
            return;
        }
        if let Some(strip) = &mut self.rolling_audio_strip {
            load_snapshot(strip);
        }
    }

    pub(crate) fn handle_view_switching_shortcuts(&mut self, ctx: &egui::Context) {
        if self.view == WorkbenchView::NodeMachine
            && (self.canvas_state.catalog_open || self.canvas_state.save_dialog_open)
        {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.view = WorkbenchView::NodeMachine;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.view = match self.view {
                WorkbenchView::Palette => WorkbenchView::NodeMachine,
                WorkbenchView::NodeMachine | WorkbenchView::AutoActions => WorkbenchView::Palette,
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.view = match self.view {
                WorkbenchView::AutoActions => WorkbenchView::Palette,
                _ => WorkbenchView::AutoActions,
            };
        }
    }

    pub(crate) fn handle_palette_shortcuts(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
    ) {
        if self.view != WorkbenchView::Palette {
            return;
        }

        if self.search_query.is_empty() {
            self.handle_empty_search_shortcuts(ctx, audio_region_active);
        } else {
            self.handle_search_prefix_shortcuts(audio_region_active);
        }

        self.handle_palette_nav_and_execution(ctx);
    }

    pub(crate) fn handle_empty_search_shortcuts(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
    ) {
        self.handle_empty_search_shortcuts_with_swap(
            ctx,
            audio_region_active,
            Self::swap_selected_slots,
        );
    }

    pub(crate) fn handle_empty_search_shortcuts_with_swap(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
        mut swap_slots: impl FnMut(&mut Self),
    ) {
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            self.view = WorkbenchView::NodeMachine;
            return;
        }
        // The search field also accepts these keys as one-character commands.
        // Let the text edit own the key now; the prefix handler executes it once
        // on the next frame after the field has updated its text.
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !audio_region_active {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::RightHand]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior2]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num3)) {
            self.apply_quick_slot_selection(vec![
                SlotId::LeftHand,
                SlotId::LeftPrior1,
                SlotId::LeftPrior2,
            ]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand]);
        } else if ctx.input(|i| {
            i.modifiers.is_none() && (i.key_pressed(egui::Key::S) || i.key_pressed(egui::Key::X))
        }) && self.selected_slots.len() == 2
        {
            swap_slots(self);
        }
    }

    pub(crate) fn apply_quick_slot_selection(&mut self, slots: Vec<SlotId>) {
        self.selected_slots = slots;
        self.dirty_mirror = true;
        self.selected_action_index = 0;
    }

    pub(crate) fn swap_selected_slots(&mut self) {
        let a = self.selected_slots[0];
        let b = self.selected_slots[1];
        let _ = send_ipc_request(&IpcRequest::SwapSlots(a, b));
        self.state.swap_slots(a, b);
        self.dirty_mirror = true;
        self.status_msg = Some(format!("Swapped {} and {}", a.label(), b.label()));
    }

    pub(crate) fn handle_search_prefix_shortcuts(&mut self, audio_region_active: bool) {
        if self.search_query.starts_with(' ') {
            if !audio_region_active {
                self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::RightHand]);
            }
            self.search_query = self.search_query.trim_start().to_string();
        } else if self.search_query == "1" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1]);
            self.search_query.clear();
        } else if self.search_query == "2" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior2]);
            self.search_query.clear();
        } else if self.search_query == "3" {
            self.apply_quick_slot_selection(vec![
                SlotId::LeftHand,
                SlotId::LeftPrior1,
                SlotId::LeftPrior2,
            ]);
            self.search_query.clear();
        } else if self.search_query == "0" || self.search_query == "`" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand]);
            self.search_query.clear();
        } else if (self.search_query.trim() == "swap"
            || ((self.search_query == "s" || self.search_query == "x")
                && self.selected_slots.len() == 2))
            && self.selected_slots.len() == 2
        {
            self.swap_selected_slots();
            self.search_query.clear();
        }
    }

    pub(crate) fn handle_palette_nav_and_execution(&mut self, ctx: &egui::Context) {
        let filtered = self.filtered_actions();
        if !filtered.is_empty() && self.selected_action_index >= filtered.len() {
            self.selected_action_index = 0;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !filtered.is_empty() {
            self.selected_action_index = (self.selected_action_index + 1) % filtered.len();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !filtered.is_empty() {
            self.selected_action_index = if self.selected_action_index == 0 {
                filtered.len() - 1
            } else {
                self.selected_action_index - 1
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.execute_current_action(ctx);
        }
    }
}
