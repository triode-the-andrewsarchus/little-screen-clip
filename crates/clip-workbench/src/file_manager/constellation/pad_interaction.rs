//! Pad relationships and gestures, independent of audio decoding.
use super::ConstellationState;

impl ConstellationState {
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
