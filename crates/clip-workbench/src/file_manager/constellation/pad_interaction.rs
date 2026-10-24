//! Pad relationships and gestures, independent of audio decoding.
use super::ConstellationState;

#[derive(Clone)]
pub(super) struct PadDrag {
    pub index: usize,
    pub generation: u64,
    pub before: super::sample_pack::PackSlot,
}

pub(crate) struct StudioRequest {
    pub path: std::path::PathBuf,
    pub trim: (f32, f32),
    generation: u64,
    pad: usize,
    layer: usize,
    before: super::sample_pack::SlotSample,
}

impl ConstellationState {
    pub(super) fn mute_group_hint(&self, hovered: Option<usize>) -> String {
        if self.choke_anchor.is_some() {
            "Keep Shift held · right-click pads to join · release to finish".to_owned()
        } else if let Some(index) =
            hovered.filter(|index| self.pack_bank.slots[*index].choke_group != 0)
        {
            let group = self.pack_bank.slots[index].choke_group;
            let keys: Vec<_> = self
                .pack_bank
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.choke_group == group)
                .map(|(index, _)| self.pad_keys[index].to_ascii_uppercase().to_string())
                .collect();
            format!("Mute group {group} · {} · Alt: details", keys.join(" "))
        } else {
            "Shift + right-click pads to link a mute group".to_owned()
        }
    }

    pub(super) fn handle_pad_response(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        response: &egui::Response,
        title: Option<String>,
    ) {
        let rect = response.rect;
        let painter = ui.painter();
        let visuals = ui.visuals().clone();
        // egui 0.27 consumes even a mismatched payload in take_payload.
        if egui::DragAndDrop::has_payload_of_type::<crate::file_manager::transfers::FileDrag>(
            ui.ctx(),
        ) && let Some(payload) =
            response.dnd_release_payload::<crate::file_manager::transfers::FileDrag>()
        {
            self.drop_on_pad(
                index,
                payload.paths.clone(),
                ui.input(|input| input.modifiers.shift),
                None,
            );
        }
        if response.drag_started() {
            egui::DragAndDrop::set_payload(
                ui.ctx(),
                PadDrag {
                    index,
                    generation: self.bank_generation,
                    before: self.pack_bank.slots[index].clone(),
                },
            );
        }
        if response.dnd_hover_payload::<PadDrag>().is_some() {
            painter.rect_stroke(
                rect.shrink(1.0),
                4.0,
                egui::Stroke::new(2.0_f32, visuals.selection.bg_fill),
            );
        }
        if egui::DragAndDrop::has_payload_of_type::<PadDrag>(ui.ctx())
            && let Some(drag) = response.dnd_release_payload::<PadDrag>()
        {
            self.swap_pads(&drag, index);
        }

        let response = if let Some(title) = title {
            response.clone().on_hover_text(title)
        } else {
            response.clone()
        };
        if response.secondary_clicked() && ui.input(|input| input.modifiers.shift) {
            self.join_mute_group(index);
        }
        if !ui.input(|input| input.modifiers.shift) {
            response.context_menu(|ui| {
                if ui.button("Leave mute group").clicked() {
                    self.leave_mute_group(index);
                    ui.close_menu();
                }
            });
        }
        if response.clicked() {
            self.selected_slot = index;
            if ui.input(|input| input.modifiers.shift) {
                self.assign_selected_to_pad(index);
            } else {
                self.reveal_pad_layer(index);
                self.trigger_slot(index);
            }
        }
    }

    /// Cycles layer focus independently for each pad without altering its instrument.
    pub(super) fn reveal_pad_layer(&mut self, index: usize) {
        let count = self.pack_bank.slots[index].samples.len();
        if count == 0 {
            return;
        }
        let layer = self.pad_click_layers[index] % count;
        self.pad_click_layers[index] = (layer + 1) % count;
        self.selected_layers[index] = layer;
        let path = self.pack_bank.slots[index].samples[layer].file_path.clone();
        if self.samples.iter().any(|sample| sample.file_path == path) {
            self.selected_path = Some(path.clone());
            self.pending_map_reveal = Some(path.clone());
            self.visited_samples.clear();
            self.visited_samples.insert(path);
        } else {
            self.browser_status = Some((
                "This layer is not in this folder's indexed map.".into(),
                std::time::Instant::now(),
            ));
        }
    }

    pub(super) fn request_pad_studio(&mut self, pad: usize, layer: usize) {
        if let Some(before) = self.pack_bank.slots[pad].samples.get(layer).cloned() {
            self.studio_request = Some(StudioRequest {
                path: before.file_path.clone(),
                trim: (before.start_sec, before.end_sec),
                generation: self.bank_generation,
                pad,
                layer,
                before,
            });
            self.stop_playback();
        }
    }

    /// Replaces only the originating layer after a successful non-overwriting export.
    pub(crate) fn accept_studio_edit(&mut self, request: StudioRequest, path: std::path::PathBuf) {
        if request.generation != self.bank_generation
            || self.pack_bank.slots[request.pad].samples.get(request.layer) != Some(&request.before)
        {
            self.browser_status = Some((
                format!("Pad changed; edited file retained at {}", path.display()),
                std::time::Instant::now(),
            ));
            return;
        }
        self.checkpoint_bank(false);
        let mut layer = request.before;
        layer.file_path = path.clone();
        layer.filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        layer.start_sec = 0.0;
        layer.duration = 15.0; // Cache warming replaces this bound with actual decoded duration.
        layer.end_sec = 15.0;
        layer.cached_audio = None;
        layer.prepared_pitch = None;
        self.failed_audio.remove(&path);
        self.pack_bank.slots[request.pad].samples[request.layer] = layer;
        self.checkpoint_bank(false);
    }

    /// Moves complete pad instruments while leaving configured key positions fixed.
    pub(super) fn swap_pads(&mut self, drag: &PadDrag, target: usize) {
        if drag.index == target
            || drag.generation != self.bank_generation
            || self.pack_bank.slots.get(drag.index) != Some(&drag.before)
            || target >= self.pack_bank.slots.len()
        {
            return;
        }
        self.checkpoint_bank(false);
        let mut rack = self.playback.lock().unwrap();
        for index in [drag.index, target] {
            rack.voices[index]
                .drain(..)
                .for_each(|player| player.stop());
        }
        drop(rack);
        self.pack_bank.slots.swap(drag.index, target);
        self.pad_click_layers.swap(drag.index, target);
        for index in [drag.index, target] {
            self.pack_bank.slots[index].key_char = super::sample_pack::SLOT_KEY_CHARS[index];
        }
        if self.selected_slot == drag.index {
            self.selected_slot = target;
        } else if self.selected_slot == target {
            self.selected_slot = drag.index;
        }
        self.choke_anchor = None;
        self.bank_generation = self.bank_generation.wrapping_add(1);
        self.checkpoint_bank(false);
    }

    /// Joins consecutive Shift-right-clicked pads without merging unrelated groups.
    pub(super) fn join_mute_group(&mut self, index: usize) {
        let Some((generation, anchor)) = self.choke_anchor else {
            self.choke_anchor = Some((self.bank_generation, index));
            return;
        };
        if generation != self.bank_generation || anchor == index {
            self.choke_anchor = Some((self.bank_generation, index));
            return;
        }
        let anchor_group = self.pack_bank.slots[anchor].choke_group;
        let target_group = self.pack_bank.slots[index].choke_group;
        let group = if anchor_group != 0 {
            Some(anchor_group)
        } else if target_group != 0 {
            Some(target_group)
        } else {
            (1..=8).find(|group| {
                !self
                    .pack_bank
                    .slots
                    .iter()
                    .any(|slot| slot.choke_group == *group)
            })
        };
        if let Some(group) = group {
            self.checkpoint_bank(false);
            self.pack_bank.slots[anchor].choke_group = group;
            self.pack_bank.slots[index].choke_group = group;
            self.pack_bank.prune_mute_groups();
            self.checkpoint_bank(false);
        } else {
            self.browser_status = Some((
                "All mute groups are in use; start from an existing group.".into(),
                std::time::Instant::now(),
            ));
        }
    }

    pub(super) fn leave_mute_group(&mut self, index: usize) {
        self.checkpoint_bank(false);
        self.pack_bank.slots[index].choke_group = 0;
        self.pack_bank.prune_mute_groups();
        self.checkpoint_bank(false);
    }

    pub(super) fn paint_mute_links(
        &self,
        ui: &egui::Ui,
        hovered: Option<usize>,
        background: egui::layers::ShapeIdx,
    ) {
        let active = self.choke_anchor.map(|(_, index)| index).or(hovered);
        let Some(index) = active else {
            return;
        };
        let group = self.pack_bank.slots[index].choke_group;
        if group == 0 && self.choke_anchor.is_none() {
            return;
        }
        let Some((origin, _)) = self.pad_drop_targets.iter().find(|(_, pad)| *pad == index) else {
            return;
        };
        let color = egui::Color32::from_rgb(225, 80, 85);
        let painter = ui.painter();
        let mut lines = Vec::new();
        for (rect, pad) in &self.pad_drop_targets {
            if *pad == index || (group != 0 && self.pack_bank.slots[*pad].choke_group == group) {
                painter.rect_stroke(rect.shrink(2.0), 4.0, egui::Stroke::new(2.0_f32, color));
                if *pad != index {
                    let direction = (rect.center() - origin.center()).normalized();
                    let start =
                        origin.center() + direction * (origin.width().min(origin.height()) * 0.5);
                    let end = rect.center() - direction * (rect.width().min(rect.height()) * 0.5);
                    lines.push(egui::Shape::line_segment(
                        [start, end],
                        egui::Stroke::new(2.0_f32, color),
                    ));
                }
            }
        }
        painter.set(background, egui::Shape::Vec(lines));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_hit() -> ConstellationState {
        let mut state = ConstellationState::default();
        state.pack_bank.slots[0]
            .samples
            .push(super::super::sample_pack::SlotSample::new(
                "hit.wav".into(),
                "hit.wav".into(),
                None,
                1.0,
            ));
        state
    }
    #[test]
    fn mute_links_are_below_every_pad_face() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.pack_bank.slots[0].choke_group = 1;
        state.pack_bank.slots[2].choke_group = 1;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let background = ui.painter().add(egui::Shape::Noop);
                state.pad_drop_targets = (0..3)
                    .map(|index| {
                        (
                            egui::Rect::from_min_size(
                                egui::pos2(20.0 + index as f32 * 70.0, 20.0),
                                egui::vec2(60.0, 60.0),
                            ),
                            index,
                        )
                    })
                    .collect();
                for (rect, _) in &state.pad_drop_targets {
                    ui.painter().rect_filled(*rect, 4.0, egui::Color32::GRAY);
                }
                state.paint_mute_links(ui, Some(0), background);
            });
        });
        let links = output
            .shapes
            .iter()
            .position(|shape| matches!(&shape.shape,egui::Shape::Vec(shapes) if !shapes.is_empty()))
            .unwrap();
        let first_face=output.shapes.iter().position(|shape| matches!(&shape.shape,egui::Shape::Rect(rect) if rect.fill==egui::Color32::GRAY)).unwrap();
        assert!(
            links < first_face,
            "even a link crossing a third pad must stay behind its face"
        );
    }

    #[test]
    fn pad_release_is_not_consumed_by_the_file_drop_handler() {
        let ctx = egui::Context::default();
        let mut state = state_with_hit();
        let drag = PadDrag {
            index: 0,
            generation: state.bank_generation,
            before: state.pack_bank.slots[0].clone(),
        };
        let pos = egui::pos2(50.0, 50.0);
        for pressed in [true, false] {
            if !pressed {
                egui::DragAndDrop::set_payload(&ctx, drag.clone());
            }
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let (_, response) =
                            ui.allocate_exact_size(egui::vec2(100.0, 100.0), egui::Sense::hover());
                        state.handle_pad_response(ui, 5, &response, None);
                    });
                },
            );
        }
        assert!(state.pack_bank.slots[0].samples.is_empty());
        assert_eq!(state.pack_bank.slots[5].samples.len(), 1);
    }

    #[test]
    fn studio_return_replaces_one_layer_and_rejects_changed_targets() {
        let mut state = ConstellationState::default();
        let layer = super::super::sample_pack::SlotSample::new(
            "old.wav".into(),
            "old.wav".into(),
            None,
            1.0,
        );
        state.pack_bank.slots[0].samples = vec![layer.clone(), layer];
        state.request_pad_studio(0, 1);
        let request = state.studio_request.take().unwrap();
        state.accept_studio_edit(request, "old.edited.wav".into());
        assert_eq!(state.pack_bank.slots[0].samples[0].filename, "old.wav");
        assert_eq!(
            state.pack_bank.slots[0].samples[1].filename,
            "old.edited.wav"
        );
        state.request_pad_studio(0, 0);
        let request = state.studio_request.take().unwrap();
        state.pack_bank.slots[0].samples[0].volume = 0.5;
        state.accept_studio_edit(request, "stale.wav".into());
        assert_eq!(state.pack_bank.slots[0].samples[0].filename, "old.wav");
    }

    #[test]
    fn pad_swap_preserves_instruments_keys_selection_and_rejects_stale_drags() {
        let mut state = state_with_hit();
        state.pack_bank.slots[0].choke_group = 2;
        let drag = PadDrag {
            index: 0,
            generation: state.bank_generation,
            before: state.pack_bank.slots[0].clone(),
        };
        state.swap_pads(&drag, 5);
        assert_eq!(state.selected_slot, 5);
        assert_eq!(state.pack_bank.slots[5].choke_group, 2);
        assert_eq!(state.pack_bank.slots[5].samples.len(), 1);
        assert_eq!(
            state.pack_bank.slots[5].key_char,
            super::super::sample_pack::SLOT_KEY_CHARS[5]
        );
        state.swap_pads(&drag, 8);
        assert!(state.pack_bank.slots[8].samples.is_empty());
        state.undo_bank(false);
        assert_eq!(state.pack_bank.slots[0].samples.len(), 1);
        assert!(state.pack_bank.slots[5].samples.is_empty());
    }

    #[test]
    fn leaving_or_moving_members_dissolves_singletons_and_undo_restores_partners() {
        let mut state = ConstellationState::default();
        state.join_mute_group(0);
        state.join_mute_group(1);
        state.join_mute_group(2);
        state.leave_mute_group(1);
        assert_eq!(state.pack_bank.slots[0].choke_group, 1);
        state.leave_mute_group(2);
        assert_eq!(state.pack_bank.slots[0].choke_group, 0);
        state.undo_bank(false);
        assert_eq!(state.pack_bank.slots[0].choke_group, 1);
        assert_eq!(state.pack_bank.slots[2].choke_group, 1);
        state.choke_anchor = None;
        state.join_mute_group(3);
        state.join_mute_group(4);
        state.join_mute_group(0); // Move one of the two members to group 2.
        assert_eq!(state.pack_bank.slots[0].choke_group, 2);
        assert_eq!(state.pack_bank.slots[2].choke_group, 0);
        state.clear_slot(3);
        state.clear_slot(4);
        assert_eq!(state.pack_bank.slots[0].choke_group, 0);
    }

    #[test]
    fn consecutive_links_adopt_existing_groups_and_undo_individually() {
        let mut state = ConstellationState::default();
        state.pack_bank.slots[3].choke_group = 4;
        state.join_mute_group(0);
        assert_eq!(state.pack_bank.slots[0].choke_group, 0);
        state.join_mute_group(3);
        state.join_mute_group(7);
        assert_eq!(state.pack_bank.slots[0].choke_group, 4);
        assert_eq!(state.pack_bank.slots[7].choke_group, 4);
        state.undo_bank(false);
        assert_eq!(state.pack_bank.slots[7].choke_group, 0);
        assert_eq!(state.pack_bank.slots[0].choke_group, 4);
    }
}
