//! Pad relationships and gestures, independent of audio decoding.
use super::ConstellationState;

#[derive(Clone)]
pub(super) struct PadDrag {
    pub index: usize,
    pub generation: u64,
    pub before: super::sample_pack::PackSlot,
}

impl ConstellationState {
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
            self.checkpoint_bank(false);
        } else {
            self.browser_status = Some((
                "All mute groups are in use; start from an existing group.".into(),
                std::time::Instant::now(),
            ));
        }
    }

    pub(super) fn paint_mute_links(&self, ui: &egui::Ui, hovered: Option<usize>) {
        let active = hovered.or(self.choke_anchor.map(|(_, index)| index));
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
        for (rect, pad) in &self.pad_drop_targets {
            if *pad == index || (group != 0 && self.pack_bank.slots[*pad].choke_group == group) {
                painter.rect_stroke(rect.shrink(2.0), 4.0, egui::Stroke::new(2.0, color));
                if *pad != index {
                    let direction = (rect.center() - origin.center()).normalized();
                    let start =
                        origin.center() + direction * (origin.width().min(origin.height()) * 0.5);
                    let end = rect.center() - direction * (rect.width().min(rect.height()) * 0.5);
                    painter.line_segment([start, end], egui::Stroke::new(2.0, color));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pad_swap_preserves_instruments_keys_selection_and_rejects_stale_drags() {
        let mut state = ConstellationState::default();
        state.pack_bank.slots[0]
            .samples
            .push(super::super::sample_pack::SlotSample::new(
                "hit.wav".into(),
                "hit.wav".into(),
                None,
                1.0,
            ));
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
