//! Standalone Little Screen (Trofeo USB hardware display) interactive mirror widget.
//!
//! Provides pixel-accurate slot layout geometry matching `little-clip-render`,
//! handling slot selection, inspection, and drag-and-drop slot swapping.

use crate::hex_to_c32;
use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::SlotId;
use little_clip_core::palette::AppPalette;

pub const SCREEN_WIDTH: f32 = little_clip_render::SCREEN_WIDTH as f32;
pub const SCREEN_HEIGHT: f32 = little_clip_render::SCREEN_HEIGHT as f32;

/// Keep the physical display's aspect ratio while reserving room for workbench content.
pub fn fitted_mirror_size(available: Vec2) -> Vec2 {
    let aspect = SCREEN_HEIGHT / SCREEN_WIDTH;
    let width = available
        .x
        .max(0.0)
        .min((available.y.max(0.0) * 0.4) / aspect);
    Vec2::new(width, width * aspect)
}

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
    Context(SlotId),
    DropSwap { from: SlotId, to: SlotId },
}

/// Standalone display mirror widget that renders the hardware display preview.
pub struct ScreenMirrorWidget<'a> {
    pub mirror_texture: Option<&'a egui::TextureHandle>,
    pub left_priors: usize,
    pub dragging_slot: &'a mut Option<SlotId>,
    pub selected_slots: &'a [SlotId],
    height_scale: f32,
    native_data: Option<&'a super::native_mirror::NativeMirrorData>,
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
            height_scale: 1.0,
            native_data: None,
        }
    }

    pub(crate) fn with_native(
        mut self,
        data: Option<&'a super::native_mirror::NativeMirrorData>,
    ) -> Self {
        self.native_data = data;
        self
    }

    /// Caps the mirror height relative to the normal screen-adaptive size.
    pub fn with_height_scale(mut self, scale: f32) -> Self {
        self.height_scale = scale.clamp(0.1, 1.0);
        self
    }

    pub fn show(mut self, ui: &mut egui::Ui, palette: &AppPalette) -> Option<MirrorAction> {
        // A top panel computes its own height from its children, so its reported
        // available height can be zero before this widget has allocated space.
        let width = ui.available_width();
        let size = fitted_mirror_size(Vec2::new(
            width,
            ui.ctx().screen_rect().height() * self.height_scale,
        ));
        let (space, _) = ui.allocate_exact_size(Vec2::new(width, size.y), egui::Sense::hover());
        let rect = Rect::from_center_size(space.center(), size);
        let response = ui.interact(
            rect,
            ui.make_persistent_id("screen_mirror"),
            egui::Sense::click_and_drag(),
        );

        let hovered_slot = response.hover_pos().map(|pos| {
            let native_x =
                ((pos.x - rect.min.x) / rect.width() * SCREEN_WIDTH).clamp(0.0, SCREEN_WIDTH - 1.0);
            slot_at_x(native_x, self.left_priors)
        });
        self.paint(ui, palette, rect, hovered_slot);
        self.handle_interaction(ui, &response, hovered_slot)
    }

    fn paint(&self, ui: &egui::Ui, palette: &AppPalette, rect: Rect, hovered_slot: Option<SlotId>) {
        let painter = ui.painter_at(rect);
        if let Some(data) = self
            .native_data
            .filter(|data| data.is_dark == palette.is_dark)
        {
            super::native_mirror::paint(
                ui,
                palette,
                rect,
                data,
                self.mirror_texture,
                self.left_priors,
            );
        } else if let Some(tex) = self.mirror_texture {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.rect_filled(
                rect,
                Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS),
                hex_to_c32(&palette.colors.surface_recessed),
            );
        }

        // Scale factor from native screen coordinates to UI rect
        let scale_x = rect.width() / SCREEN_WIDTH;
        let scale_y = rect.height() / SCREEN_HEIGHT;

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
                    Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS),
                    Stroke::new(2.0_f32, stroke_color),
                );
            } else if is_hovered && !is_dragging {
                let stroke_color = hex_to_c32(&palette.colors.text_secondary);
                painter.rect_stroke(
                    slot_screen_rect,
                    Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS),
                    Stroke::new(1.0_f32, stroke_color),
                );
            }
        }
    }

    fn handle_interaction(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        hovered_slot: Option<SlotId>,
    ) -> Option<MirrorAction> {
        let mut action = if response.secondary_clicked() {
            hovered_slot.map(MirrorAction::Context)
        } else if response.double_clicked() {
            hovered_slot.map(MirrorAction::DoubleClick)
        } else if response.clicked() {
            hovered_slot.map(MirrorAction::Click)
        } else {
            None
        };

        if response.drag_started() && hovered_slot != Some(SlotId::Undo) {
            *self.dragging_slot = hovered_slot;
        }

        if response.dragged() {
            if let Some(source) = *self.dragging_slot {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("slot_drag_tip"), |ui| {
                    ui.label(format!(
                        "Dragging {} (drop on another slot to swap)",
                        source.label()
                    ));
                });
            }
        }

        if response.drag_stopped() {
            action = self
                .dragging_slot
                .take()
                .zip(hovered_slot)
                .and_then(|(from, to)| {
                    (from != to && to != SlotId::Undo)
                        .then_some(MirrorAction::DropSwap { from, to })
                })
                .or(action);
        }

        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_uses_width_on_standard_host_and_height_on_ultrawide_host() {
        let standard = fitted_mirror_size(Vec2::new(900.0, 600.0));
        assert_eq!(standard.x, 900.0);
        let ultrawide = fitted_mirror_size(Vec2::new(2_000.0, 700.0));
        assert!(ultrawide.x < 2_000.0);
        assert!((ultrawide.y - 280.0).abs() < 0.01);
        assert!((ultrawide.y / ultrawide.x - SCREEN_HEIGHT / SCREEN_WIDTH).abs() < 0.0001);
    }

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

#[cfg(test)]
mod native_input_tests {
    use super::super::native_mirror::NativeMirrorData;
    use super::*;

    fn frame(
        ctx: &egui::Context,
        width: f32,
        events: Vec<egui::Event>,
        data: Option<&NativeMirrorData>,
        dragging: &mut Option<SlotId>,
        left_priors: usize,
    ) -> Option<MirrorAction> {
        let mut action = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 600.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        action = ScreenMirrorWidget::new(None, left_priors, dragging, &[])
                            .with_native(data)
                            .show(ui, &crate::ui_appearance::palette(Default::default()));
                    });
            },
        );
        action
    }

    #[test]
    fn native_and_reference_clicks_target_the_same_slots_at_both_layout_widths() {
        let data = NativeMirrorData::prepare(&Default::default(), Vec::new(), true);
        for width in [900.0, 1600.0] {
            for priors in [2, 3] {
                for (slot, native) in slot_rects(priors) {
                    for prepared in [None, Some(&data)] {
                        let ctx = egui::Context::default();
                        let mut dragging = None;
                        frame(&ctx, width, Vec::new(), prepared, &mut dragging, priors);
                        let size = fitted_mirror_size(Vec2::new(width, 600.0));
                        let point = Pos2::new(
                            (width - size.x) * 0.5 + native.center().x * size.x / SCREEN_WIDTH,
                            size.y * 0.5,
                        );
                        frame(
                            &ctx,
                            width,
                            vec![
                                egui::Event::PointerMoved(point),
                                egui::Event::PointerButton {
                                    pos: point,
                                    button: egui::PointerButton::Primary,
                                    pressed: true,
                                    modifiers: Default::default(),
                                },
                            ],
                            prepared,
                            &mut dragging,
                            priors,
                        );
                        let action = frame(
                            &ctx,
                            width,
                            vec![egui::Event::PointerButton {
                                pos: point,
                                button: egui::PointerButton::Primary,
                                pressed: false,
                                modifiers: Default::default(),
                            }],
                            prepared,
                            &mut dragging,
                            priors,
                        );
                        assert_eq!(
                            action,
                            Some(MirrorAction::Click(slot)),
                            "width={width}, priors={priors}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn native_and_reference_drags_emit_the_same_swap() {
        let data = NativeMirrorData::prepare(&Default::default(), Vec::new(), true);
        for prepared in [None, Some(&data)] {
            let ctx = egui::Context::default();
            let mut dragging = None;
            frame(&ctx, 900.0, Vec::new(), prepared, &mut dragging, 2);
            let start = Pos2::new(200.0, 100.0);
            let end = Pos2::new(800.0, 100.0);
            frame(
                &ctx,
                900.0,
                vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
                prepared,
                &mut dragging,
                2,
            );
            frame(
                &ctx,
                900.0,
                vec![egui::Event::PointerMoved(start + Vec2::new(10.0, 0.0))],
                prepared,
                &mut dragging,
                2,
            );
            frame(
                &ctx,
                900.0,
                vec![egui::Event::PointerMoved(end)],
                prepared,
                &mut dragging,
                2,
            );
            let action = frame(
                &ctx,
                900.0,
                vec![egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                prepared,
                &mut dragging,
                2,
            );
            assert_eq!(
                action,
                Some(MirrorAction::DropSwap {
                    from: SlotId::LeftHand,
                    to: SlotId::RightHand
                })
            );
        }
    }
}
