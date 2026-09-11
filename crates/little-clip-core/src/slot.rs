use crate::item::ClipItem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SlotId {
    Undo,
    LeftHand,
    LeftPrior1,
    LeftPrior2,
    LeftPrior3,
    RightPrior2,
    RightPrior1,
    RightHand,
}

impl SlotId {
    pub fn all() -> [SlotId; 8] {
        [
            SlotId::Undo,
            SlotId::LeftHand,
            SlotId::LeftPrior1,
            SlotId::LeftPrior2,
            SlotId::LeftPrior3,
            SlotId::RightPrior2,
            SlotId::RightPrior1,
            SlotId::RightHand,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            SlotId::Undo => "UNDO",
            SlotId::LeftHand => "LEFT HAND",
            SlotId::LeftPrior1 => "L PRIOR(1)",
            SlotId::LeftPrior2 => "L PRIOR(2)",
            SlotId::LeftPrior3 => "L PRIOR(3)",
            SlotId::RightPrior2 => "R PRIOR(2)",
            SlotId::RightPrior1 => "R PRIOR(1)",
            SlotId::RightHand => "RIGHT HAND",
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            SlotId::Undo => "Z",
            SlotId::LeftHand => "LH",
            SlotId::LeftPrior1 => "LP1",
            SlotId::LeftPrior2 => "LP2",
            SlotId::LeftPrior3 => "LP3",
            SlotId::RightPrior2 => "RP2",
            SlotId::RightPrior1 => "RP1",
            SlotId::RightHand => "RH",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<SlotId> {
        match s.to_lowercase().trim() {
            "undo" | "z" => Some(SlotId::Undo),
            "left" | "lh" | "lefthand" | "l" => Some(SlotId::LeftHand),
            "lp1" | "l1" | "leftprior1" | "left1" => Some(SlotId::LeftPrior1),
            "lp2" | "l2" | "leftprior2" | "left2" => Some(SlotId::LeftPrior2),
            "lp3" | "l3" | "leftprior3" | "left3" => Some(SlotId::LeftPrior3),
            "rp2" | "r2" | "rightprior2" | "right2" => Some(SlotId::RightPrior2),
            "rp1" | "r1" | "rightprior1" | "right1" => Some(SlotId::RightPrior1),
            "right" | "rh" | "righthand" | "r" => Some(SlotId::RightHand),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandSide {
    Left,
    Right,
}

impl Default for HandSide {
    fn default() -> Self {
        HandSide::Left
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlotManager {
    pub undo: Option<ClipItem>,
    pub left_hand: Option<ClipItem>,
    pub left_prior1: Option<ClipItem>,
    pub left_prior2: Option<ClipItem>,
    pub left_prior3: Option<ClipItem>,
    pub right_prior2: Option<ClipItem>,
    pub right_prior1: Option<ClipItem>,
    pub right_hand: Option<ClipItem>,
    pub cursor: SlotId,
    pub selected: Vec<SlotId>,
}

impl Default for SlotManager {
    fn default() -> Self {
        Self {
            undo: None,
            left_hand: None,
            left_prior1: None,
            left_prior2: None,
            left_prior3: None,
            right_prior2: None,
            right_prior1: None,
            right_hand: None,
            cursor: SlotId::LeftHand,
            selected: Vec::new(),
        }
    }
}

impl SlotManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: SlotId) -> Option<&ClipItem> {
        match id {
            SlotId::Undo => self.undo.as_ref(),
            SlotId::LeftHand => self.left_hand.as_ref(),
            SlotId::LeftPrior1 => self.left_prior1.as_ref(),
            SlotId::LeftPrior2 => self.left_prior2.as_ref(),
            SlotId::LeftPrior3 => self.left_prior3.as_ref(),
            SlotId::RightPrior2 => self.right_prior2.as_ref(),
            SlotId::RightPrior1 => self.right_prior1.as_ref(),
            SlotId::RightHand => self.right_hand.as_ref(),
        }
    }

    pub fn get_mut(&mut self, id: SlotId) -> Option<&mut Option<ClipItem>> {
        match id {
            SlotId::Undo => Some(&mut self.undo),
            SlotId::LeftHand => Some(&mut self.left_hand),
            SlotId::LeftPrior1 => Some(&mut self.left_prior1),
            SlotId::LeftPrior2 => Some(&mut self.left_prior2),
            SlotId::LeftPrior3 => Some(&mut self.left_prior3),
            SlotId::RightPrior2 => Some(&mut self.right_prior2),
            SlotId::RightPrior1 => Some(&mut self.right_prior1),
            SlotId::RightHand => Some(&mut self.right_hand),
        }
    }

    pub fn hand(&self) -> Option<&ClipItem> {
        self.left_hand.as_ref()
    }

    pub fn left_hand(&self) -> Option<&ClipItem> {
        self.left_hand.as_ref()
    }

    pub fn right_hand(&self) -> Option<&ClipItem> {
        self.right_hand.as_ref()
    }

    pub fn undo_slot(&self) -> Option<&ClipItem> {
        self.undo.as_ref()
    }

    /// Pushes a new item into Left Hand.
    /// Moves current left_prior2 to left_prior3,
    /// moves current left_prior1 to left_prior2,
    /// moves current left_hand to left_prior1,
    /// and places item into left_hand.
    /// Note: UNDO is NOT modified during normal copying/pushing.
    pub fn push_left(&mut self, item: ClipItem) {
        self.left_prior3 = self.left_prior2.take();
        self.left_prior2 = self.left_prior1.take();
        self.left_prior1 = self.left_hand.take();
        self.left_hand = Some(item);
    }

    /// Pushes a new item into Right Hand.
    /// Moves current right_prior1 to right_prior2,
    /// moves current right_hand to right_prior1,
    /// and places item into right_hand.
    /// Note: UNDO is NOT modified during normal copying/pushing.
    pub fn push_right(&mut self, item: ClipItem) {
        self.right_prior2 = self.right_prior1.take();
        self.right_prior1 = self.right_hand.take();
        self.right_hand = Some(item);
    }

    /// Helper for pushing to specified hand (defaults to Left).
    pub fn push_to_hand(&mut self, item: ClipItem, hand: HandSide) {
        match hand {
            HandSide::Left => self.push_left(item),
            HandSide::Right => self.push_right(item),
        }
    }

    /// Applies the result of an operation.
    /// Moves previous Left Hand contents into UNDO box,
    /// and places the result into Left Hand.
    pub fn apply_op_result(&mut self, result: ClipItem) {
        self.undo = self.left_hand.take();
        self.left_hand = Some(result);
    }

    /// Restores Left Hand from UNDO.
    pub fn undo(&mut self) -> bool {
        if self.undo.is_some() {
            self.swap_slots(SlotId::LeftHand, SlotId::Undo);
            true
        } else {
            false
        }
    }

    /// Swaps the contents of any two slots.
    pub fn swap_slots(&mut self, a: SlotId, b: SlotId) {
        if a == b {
            return;
        }
        let item_a = self.get(a).cloned();
        let item_b = self.get(b).cloned();

        if let Some(slot_a) = self.get_mut(a) {
            *slot_a = item_b;
        }
        if let Some(slot_b) = self.get_mut(b) {
            *slot_b = item_a;
        }
    }

    /// Clears the specified slot.
    pub fn clear(&mut self, slot: SlotId) {
        if let Some(s) = self.get_mut(slot) {
            *s = None;
        }
        self.selected.retain(|&id| id != slot);
    }

    pub fn toggle_selection(&mut self, slot: SlotId) {
        if let Some(pos) = self.selected.iter().position(|&s| s == slot) {
            self.selected.remove(pos);
        } else {
            self.selected.push(slot);
        }
    }

    pub fn is_selected(&self, slot: SlotId) -> bool {
        self.selected.contains(&slot)
    }

    pub fn set_cursor(&mut self, slot: SlotId) {
        self.cursor = slot;
    }

    /// Updates the pre-rendered thumbnail for an item anywhere in the manager's slots.
    pub fn update_thumbnail(&mut self, item_id: uuid::Uuid, thumb: crate::item::ImageThumbnail) -> bool {
        let mut updated = false;
        let slots = [
            &mut self.undo,
            &mut self.left_hand,
            &mut self.left_prior1,
            &mut self.left_prior2,
            &mut self.left_prior3,
            &mut self.right_prior2,
            &mut self.right_prior1,
            &mut self.right_hand,
        ];
        for slot in slots {
            if let Some(item) = slot {
                if item.id == item_id {
                    if let crate::item::ClipPayload::Image { ref mut thumbnail, .. } = item.payload {
                        *thumbnail = Some(thumb.clone());
                        updated = true;
                    }
                }
            }
        }
        updated
    }

    /// Updates the pre-rendered thumbnails for a multi-image item anywhere in the manager's slots.
    pub fn update_multi_image_thumbnails(
        &mut self,
        item_id: uuid::Uuid,
        thumbs: Vec<crate::item::ImageThumbnail>,
    ) -> bool {
        let mut updated = false;
        let slots = [
            &mut self.undo,
            &mut self.left_hand,
            &mut self.left_prior1,
            &mut self.left_prior2,
            &mut self.right_prior2,
            &mut self.right_prior1,
            &mut self.right_hand,
        ];
        for slot in slots {
            if let Some(item) = slot {
                if item.id == item_id {
                    if let crate::item::ClipPayload::MultiImage { ref mut thumbnails, .. } = item.payload {
                        *thumbnails = thumbs.clone();
                        updated = true;
                    }
                }
            }
        }
        updated
    }

    /// Updates the pre-rendered audio waveform for an item anywhere in the manager's slots.
    pub fn update_audio_waveform(&mut self, item_id: uuid::Uuid, waveform: crate::audio::AudioWaveform) -> bool {
        let mut updated = false;
        let slots = [
            &mut self.undo,
            &mut self.left_hand,
            &mut self.left_prior1,
            &mut self.left_prior2,
            &mut self.right_prior2,
            &mut self.right_prior1,
            &mut self.right_hand,
        ];
        for slot in slots {
            if let Some(item) = slot {
                if item.id == item_id {
                    if let crate::item::ClipPayload::AudioRef {
                        ref mut duration_ms,
                        ref mut sample_rate,
                        ref mut channels,
                        waveform: ref mut wf,
                        ..
                    } = item.payload {
                        *duration_ms = waveform.duration_ms;
                        *sample_rate = waveform.sample_rate;
                        *channels = waveform.channels;
                        *wf = Some(waveform.clone());
                        updated = true;
                    }
                }
            }
        }
        updated
    }

    /// Attaches Phase 2 spectral frequency colors to an existing waveform.
    pub fn attach_audio_spectral_colors(&mut self, item_id: uuid::Uuid, colors: Vec<[u8; 3]>) -> bool {
        let mut updated = false;
        let slots = [
            &mut self.undo,
            &mut self.left_hand,
            &mut self.left_prior1,
            &mut self.left_prior2,
            &mut self.right_prior2,
            &mut self.right_prior1,
            &mut self.right_hand,
        ];
        for slot in slots {
            if let Some(item) = slot {
                if item.id == item_id {
                    if let crate::item::ClipPayload::AudioRef { waveform: Some(ref mut wf), .. } = item.payload {
                        wf.spectral_colors = Some(colors.clone());
                        updated = true;
                    }
                }
            }
        }
        updated
    }

    /// Updates the animation thumbnail of an item across all slots
    pub fn update_animation_thumbnail(
        &mut self,
        item_id: uuid::Uuid,
        thumb: crate::animation::AnimationThumbnail,
    ) -> bool {
        let mut updated = false;
        let slots = [
            &mut self.undo,
            &mut self.left_hand,
            &mut self.left_prior1,
            &mut self.left_prior2,
            &mut self.right_prior2,
            &mut self.right_prior1,
            &mut self.right_hand,
        ];
        for slot in slots {
            if let Some(item) = slot {
                if item.id == item_id {
                    if let crate::item::ClipPayload::Animation {
                        ref mut thumbnail,
                        ref mut width,
                        ref mut height,
                        ref mut duration_ms,
                        ..
                    } = item.payload
                    {
                        *width = thumb.width;
                        *height = thumb.height;
                        if *duration_ms == 0 {
                            *duration_ms = thumb.loop_duration_ms;
                        }
                        *thumbnail = Some(thumb.clone());
                        updated = true;
                    }
                }
            }
        }
        updated
    }

    /// Checks if any of the visible slots currently contains an active animation with frames
    pub fn has_active_animation(&self) -> bool {
        let slots = [
            &self.undo,
            &self.left_hand,
            &self.left_prior1,
            &self.left_prior2,
            &self.right_prior2,
            &self.right_prior1,
            &self.right_hand,
        ];
        slots.iter().any(|slot| {
            if let Some(item) = slot {
                matches!(
                    item.payload,
                    crate::item::ClipPayload::Animation {
                        thumbnail: Some(ref t),
                        ..
                    } if t.frame_count > 1
                )
            } else {
                false
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ClipItem;

    #[test]
    fn test_two_stacks_push() {
        let mut mgr = SlotManager::new();

        // Push 1 to Left Hand
        mgr.push_left(ClipItem::from_text("L1"));
        assert_eq!(mgr.left_hand().unwrap().title, "L1");
        assert!(mgr.get(SlotId::LeftPrior1).is_none());
        assert!(mgr.undo_slot().is_none()); // Undo is untouched!

        // Push 2 to Left Hand
        mgr.push_left(ClipItem::from_text("L2"));
        assert_eq!(mgr.left_hand().unwrap().title, "L2");
        assert_eq!(mgr.get(SlotId::LeftPrior1).unwrap().title, "L1");
        assert!(mgr.get(SlotId::LeftPrior2).is_none());
        assert!(mgr.undo_slot().is_none());

        // Push 3 to Left Hand
        mgr.push_left(ClipItem::from_text("L3"));
        assert_eq!(mgr.left_hand().unwrap().title, "L3");
        assert_eq!(mgr.get(SlotId::LeftPrior1).unwrap().title, "L2");
        assert_eq!(mgr.get(SlotId::LeftPrior2).unwrap().title, "L1");
        assert!(mgr.undo_slot().is_none());

        // Push to Right Hand
        mgr.push_right(ClipItem::from_text("R1"));
        assert_eq!(mgr.right_hand().unwrap().title, "R1");
        assert!(mgr.get(SlotId::RightPrior1).is_none());

        mgr.push_right(ClipItem::from_text("R2"));
        assert_eq!(mgr.right_hand().unwrap().title, "R2");
        assert_eq!(mgr.get(SlotId::RightPrior1).unwrap().title, "R1");

        // Undo is still untouched
        assert!(mgr.undo_slot().is_none());

        // Now simulate an operation
        let op_result = ClipItem::from_text("RESULT");
        mgr.apply_op_result(op_result);

        // Result occupies Left Hand, and UNDO has previous Left Hand ("L3")
        assert_eq!(mgr.left_hand().unwrap().title, "RESULT");
        assert_eq!(mgr.undo_slot().unwrap().title, "L3");
        // Priors and Right Hand are NOT modified by the op
        assert_eq!(mgr.get(SlotId::LeftPrior1).unwrap().title, "L2");
        assert_eq!(mgr.right_hand().unwrap().title, "R2");

        // Test Undo rollback
        assert!(mgr.undo());
        assert_eq!(mgr.left_hand().unwrap().title, "L3");
        assert_eq!(mgr.undo_slot().unwrap().title, "RESULT");
    }

    #[test]
    fn test_json_serialization() {
        let mgr = SlotManager::new();
        let json = serde_json::to_string(&mgr).unwrap();
        assert!(json.contains("left_hand"));
        assert!(json.contains("right_hand"));
        assert!(json.contains("undo"));
    }
}
