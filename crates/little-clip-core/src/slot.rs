//! Slot and clipboard history manager.
//!
//! Defines the dual-stack clipboard slots (`LeftHand`, `RightHand`, prior history stacks, and `Undo`),
//! along with navigation, selection, and payload mutation routines.

use crate::item::ClipItem;
use serde::{Deserialize, Serialize};

/// Identifies a specific UI slot or history position in the dual-stack model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SlotId {
    /// Holds the item overwritten by the last destructive operation.
    Undo,
    /// Primary active item in the Left Hand stack.
    LeftHand,
    /// Most recent history item in the Left Hand stack.
    LeftPrior1,
    /// Second most recent history item in the Left Hand stack.
    LeftPrior2,
    /// Third most recent history item in the Left Hand stack.
    LeftPrior3,
    /// Second most recent history item in the Right Hand stack.
    RightPrior2,
    /// Most recent history item in the Right Hand stack.
    RightPrior1,
    /// Primary active item in the Right Hand stack.
    RightHand,
}

impl SlotId {
    /// Returns an array of all available `SlotId` variants in display order.
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

    /// Returns a human-readable display label for the slot.
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

    /// Returns a compact short label (e.g. for badges and CLI output).
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

    /// Parses a loose string representation into a `SlotId`.
    ///
    /// Accepts abbreviations such as `"l"`, `"lh"`, `"rp1"`, `"undo"`, `"z"`.
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

/// Identifies which hand (stack) a clipboard operation targets.
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

/// Manages dual-stack clipboard history, undo state, and UI cursor/selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlotManager {
    /// Item preserved during the most recent operation.
    pub undo: Option<ClipItem>,
    /// Active item in the Left Hand.
    pub left_hand: Option<ClipItem>,
    /// Preceding item in Left Hand history.
    pub left_prior1: Option<ClipItem>,
    /// Second preceding item in Left Hand history.
    pub left_prior2: Option<ClipItem>,
    /// Third preceding item in Left Hand history.
    pub left_prior3: Option<ClipItem>,
    /// Second preceding item in Right Hand history.
    pub right_prior2: Option<ClipItem>,
    /// Preceding item in Right Hand history.
    pub right_prior1: Option<ClipItem>,
    /// Active item in the Right Hand.
    pub right_hand: Option<ClipItem>,
    /// Currently focused slot for UI navigation.
    pub cursor: SlotId,
    /// List of multi-selected slots for batch operations.
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
    /// Creates a new empty `SlotManager` with the cursor at `LeftHand`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a reference to the item in the specified slot, if present.
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

    /// Returns a mutable reference to the `Option<ClipItem>` slot storage.
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

    /// Returns a reference to the active Left Hand item (primary clipboard item).
    pub fn hand(&self) -> Option<&ClipItem> {
        self.left_hand.as_ref()
    }

    /// Returns a reference to the active Left Hand item.
    pub fn left_hand(&self) -> Option<&ClipItem> {
        self.left_hand.as_ref()
    }

    /// Returns a reference to the active Right Hand item.
    pub fn right_hand(&self) -> Option<&ClipItem> {
        self.right_hand.as_ref()
    }

    /// Returns a reference to the item in the Undo slot.
    pub fn undo_slot(&self) -> Option<&ClipItem> {
        self.undo.as_ref()
    }

    /// Pushes a new item into Left Hand.
    ///
    /// Cascades existing Left Hand history:
    /// `left_prior2` -> `left_prior3`, `left_prior1` -> `left_prior2`, `left_hand` -> `left_prior1`.
    /// Note: `Undo` is NOT modified during normal copy pushes.
    pub fn push_left(&mut self, item: ClipItem) {
        self.left_prior3 = self.left_prior2.take();
        self.left_prior2 = self.left_prior1.take();
        self.left_prior1 = self.left_hand.take();
        self.left_hand = Some(item);
    }

    /// Pushes a new item into Right Hand.
    ///
    /// Cascades existing Right Hand history:
    /// `right_prior1` -> `right_prior2`, `right_hand` -> `right_prior1`.
    /// Note: `Undo` is NOT modified during normal copy pushes.
    pub fn push_right(&mut self, item: ClipItem) {
        self.right_prior2 = self.right_prior1.take();
        self.right_prior1 = self.right_hand.take();
        self.right_hand = Some(item);
    }

    /// Pushes an item to the specified hand side.
    pub fn push_to_hand(&mut self, item: ClipItem, hand: HandSide) {
        match hand {
            HandSide::Left => self.push_left(item),
            HandSide::Right => self.push_right(item),
        }
    }

    /// Applies the result of a transformation operation.
    ///
    /// Moves previous Left Hand contents into the `Undo` slot and places the result into `LeftHand`.
    pub fn apply_op_result(&mut self, result: ClipItem) {
        self.undo = self.left_hand.take();
        self.left_hand = Some(result);
    }

    /// Restores Left Hand from Undo, swapping the current Left Hand into Undo.
    ///
    /// Returns `true` if an item was present in Undo, `false` otherwise.
    pub fn undo(&mut self) -> bool {
        if self.undo.is_some() {
            self.swap_slots(SlotId::LeftHand, SlotId::Undo);
            true
        } else {
            false
        }
    }

    /// Enforces configured depth limits on prior history slots.
    pub fn apply_config(&mut self, config: &crate::config::SlotsConfig) {
        if config.left_priors < 3 {
            self.left_prior3 = None;
        }
        if config.left_priors < 2 {
            self.left_prior2 = None;
        }
        if config.left_priors < 1 {
            self.left_prior1 = None;
        }
        if config.right_priors < 2 {
            self.right_prior2 = None;
        }
        if config.right_priors < 1 {
            self.right_prior1 = None;
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

    /// Clears the specified slot and removes it from selection if present.
    pub fn clear(&mut self, slot: SlotId) {
        if let Some(s) = self.get_mut(slot) {
            *s = None;
        }
        self.selected.retain(|&id| id != slot);
    }

    /// Toggles the multi-selection status of a slot.
    pub fn toggle_selection(&mut self, slot: SlotId) {
        if let Some(pos) = self.selected.iter().position(|&s| s == slot) {
            self.selected.remove(pos);
        } else {
            self.selected.push(slot);
        }
    }

    /// Returns `true` if the given slot is currently in the multi-selection set.
    pub fn is_selected(&self, slot: SlotId) -> bool {
        self.selected.contains(&slot)
    }

    /// Sets the active UI cursor slot.
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
            &mut self.left_prior3,
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
            &mut self.left_prior3,
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
            &mut self.left_prior3,
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
            &mut self.left_prior3,
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
            &self.left_prior3,
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

    #[test]
    fn test_left_prior3_async_updates() {
        let mut mgr = SlotManager::new();
        let item = ClipItem::from_text("L1");
        let item_id = item.id;
        mgr.push_left(item); // left_hand
        mgr.push_left(ClipItem::from_text("L2")); // L1 -> left_prior1
        mgr.push_left(ClipItem::from_text("L3")); // L1 -> left_prior2
        mgr.push_left(ClipItem::from_text("L4")); // L1 -> left_prior3

        assert_eq!(mgr.get(SlotId::LeftPrior3).unwrap().id, item_id);

        // Test multi-image thumbnail update reaching left_prior3
        let thumb = crate::item::ImageThumbnail {
            width: 100,
            height: 100,
            premultiplied_rgba: vec![255; 100 * 100 * 4],
        };
        // Replace payload in left_prior3 with multi-image
        if let Some(slot) = mgr.get_mut(SlotId::LeftPrior3) {
            if let Some(it) = slot {
                it.payload = crate::item::ClipPayload::MultiImage {
                    paths: vec![std::path::PathBuf::from("a.png")],
                    total_count: 1,
                    preview_indices: vec![0],
                    thumbnails: Vec::new(),
                };
            }
        }
        assert!(mgr.update_multi_image_thumbnails(item_id, vec![thumb]));
        if let Some(crate::item::ClipPayload::MultiImage { thumbnails, .. }) =
            mgr.get(SlotId::LeftPrior3).map(|i| &i.payload)
        {
            assert_eq!(thumbnails.len(), 1);
        } else {
            panic!("Expected MultiImage payload in LeftPrior3");
        }
    }

    #[test]
    fn test_slots_config_depth_limits() {
        let mut mgr = SlotManager::new();
        mgr.push_left(ClipItem::from_text("L1"));
        mgr.push_left(ClipItem::from_text("L2"));
        mgr.push_left(ClipItem::from_text("L3"));
        mgr.push_left(ClipItem::from_text("L4"));
        assert!(mgr.get(SlotId::LeftPrior3).is_some());

        let cfg = crate::config::SlotsConfig {
            left_priors: 2,
            right_priors: 1,
        };
        mgr.apply_config(&cfg);
        assert!(mgr.get(SlotId::LeftPrior3).is_none());
        assert!(mgr.get(SlotId::LeftPrior2).is_some());
    }
}

