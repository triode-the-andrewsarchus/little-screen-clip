//! Standalone Little Screen (Trofeo USB hardware display) interactive mirror widget.
//!
//! Provides pixel-accurate slot layout geometry matching `little-clip-render`,
//! handling slot selection, inspection, and drag-and-drop slot swapping.

use crate::hex_to_c32;
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::SlotId;
use little_clip_core::theme::AppPalette;

pub const SCREEN_WIDTH: f32 = 1920.0;
pub const SCREEN_HEIGHT: f32 = 462.0;

/// Returns the sequence of slot IDs and their native pixel bounding boxes (0..1920, 0..462).
pub fn slot_rects(left_priors: usize) -> Vec<(SlotId, Rect)> {
    little_clip_render::compute_slot_layout(left_priors)
        .into_iter()
        .map(|slot| {
            (
                slot.slot_id,
                Rect::from_min_size(
                    Pos2::new(slot.x as f32, slot.y as f32),
                    Vec2::new(slot.width as f32, slot.height as f32),
                ),
            )
        })
        .collect()
}

/// Resolves a native screen x-coordinate (0.0 ..= 1920.0) into the corresponding `SlotId`.
pub fn slot_at_x(native_x: f32, left_priors: usize) -> SlotId {
    for (slot_id, rect) in slot_rects(left_priors) {
        if native_x >= rect.min.x && native_x < rect.max.x {
            return slot_id;
        }
    }
    SlotId::RightHand
}

/// Actions emitted by the screen mirror widget during user interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorAction {
    Click(SlotId),
    DoubleClick(SlotId),
    DropSwap { from: SlotId, to: SlotId },
}

/// Standalone display mirror widget that renders the hardware display preview.
pub struct ScreenMirrorWidget<'a> {
    pub mirror_texture: Option<&'a egui::TextureHandle>,
    pub left_priors: usize,
    pub dragging_slot: &'a mut Option<SlotId>,
    pub selected_slots: &'a [SlotId],
}

impl<'a> ScreenMirrorWidget<'a> {
    pub fn new(
        mirror_texture: Option<&'a egui::TextureHandle>,
        left_priors: usize,
        dragging_slot: &'a mut Option<SlotId>,
        selected_slots: &'a [SlotId],
    ) -> Self {
        Self {
            mirror_texture,
            left_priors,
            dragging_slot,
            selected_slots,
        }
    }

    pub fn show(self, ui: &mut egui::Ui, palette: &AppPalette) -> Option<MirrorAction> {
        let avail_w = ui.available_width();
        let aspect = SCREEN_HEIGHT / SCREEN_WIDTH;
        let target_h = avail_w * aspect;

        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(avail_w, target_h), egui::Sense::click_and_drag());

        let painter = ui.painter_at(rect);
        let mut action = None;

        // 1. Draw texture background
        if let Some(tex) = self.mirror_texture {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.rect_filled(rect, Rounding::same(4.0), hex_to_c32(&palette.theme.b_low));
        }

        // Scale factor from native screen coordinates to UI rect
        let scale_x = rect.width() / SCREEN_WIDTH;
        let scale_y = rect.height() / SCREEN_HEIGHT;

        // 2. Draw active selections and hover states
        let hover_pos = response.hover_pos();
        let hovered_slot = hover_pos.map(|p| {
            let nat_x = ((p.x - rect.min.x) / scale_x).clamp(0.0, SCREEN_WIDTH - 1.0);
            slot_at_x(nat_x, self.left_priors)
        });

        let slots = slot_rects(self.left_priors);
        for (slot_id, nat_rect) in &slots {
            let slot_screen_rect = Rect::from_min_max(
                Pos2::new(
                    rect.min.x + nat_rect.min.x * scale_x,
                    rect.min.y + nat_rect.min.y * scale_y,
                ),
                Pos2::new(
                    rect.min.x + nat_rect.max.x * scale_x,
                    rect.min.y + nat_rect.max.y * scale_y,
                ),
            );

            let is_selected = self.selected_slots.contains(slot_id);
            let is_hovered = hovered_slot == Some(*slot_id);
            let is_dragging = *self.dragging_slot == Some(*slot_id);

            if is_selected {
                let stroke_color = hex_to_c32(&palette.harmonies.base);
                painter.rect_stroke(
                    slot_screen_rect,
                    Rounding::same(4.0),
                    Stroke::new(2.0_f32, stroke_color),
                );
            } else if is_hovered && !is_dragging {
                let stroke_color = hex_to_c32(&palette.theme.f_med);
                painter.rect_stroke(
                    slot_screen_rect,
                    Rounding::same(4.0),
                    Stroke::new(1.0_f32, stroke_color),
                );
            }
        }

        // 3. Interactions: click, double click, and drag-and-drop
        if response.clicked() {
            if let Some(s) = hovered_slot {
                action = Some(MirrorAction::Click(s));
            }
        }
        if response.double_clicked() {
            if let Some(s) = hovered_slot {
                action = Some(MirrorAction::DoubleClick(s));
            }
        }

        if response.drag_started() {
            if let Some(s) = hovered_slot {
                *self.dragging_slot = Some(s);
            }
        }

        if response.drag_stopped() {
            if let (Some(from_slot), Some(to_slot)) = (*self.dragging_slot, hovered_slot) {
                if from_slot != to_slot {
                    action = Some(MirrorAction::DropSwap {
                        from: from_slot,
                        to: to_slot,
                    });
                }
            }
            *self.dragging_slot = None;
        }

        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_rects_geometry() {
        let rects_2p = slot_rects(2);
        assert_eq!(rects_2p.len(), 7);
        assert_eq!(rects_2p[0].0, SlotId::Undo);
        assert_eq!(rects_2p[1].0, SlotId::LeftHand);
        assert_eq!(rects_2p[4].0, SlotId::RightPrior2);
        assert_eq!(rects_2p[6].0, SlotId::RightHand);

        let rects_3p = slot_rects(3);
        assert_eq!(rects_3p[4].0, SlotId::LeftPrior3);
    }

    #[test]
    fn test_slot_at_x_lookup() {
        assert_eq!(slot_at_x(100.0, 2), SlotId::Undo);
        assert_eq!(slot_at_x(300.0, 2), SlotId::LeftHand);
        assert_eq!(slot_at_x(700.0, 2), SlotId::LeftPrior1);
        assert_eq!(slot_at_x(900.0, 2), SlotId::LeftPrior2);
        assert_eq!(slot_at_x(1200.0, 2), SlotId::RightPrior2);
        assert_eq!(slot_at_x(1200.0, 3), SlotId::LeftPrior3);
        assert_eq!(slot_at_x(1400.0, 2), SlotId::RightPrior1);
        assert_eq!(slot_at_x(1700.0, 2), SlotId::RightHand);
    }
}
