//! Reusable before/after split slider comparison widget.
//!
//! Provides interactive comparison between original and processed textures
//! with draggable dividing knob, contrast divider stroke, texture UV partitioning,
//! and double-click center reset.

use crate::hex_to_c32;
use crate::ui_ext::{CONTROL_HEIGHT, MONO_FONT_SIZE, contrast_text_c32};
use eframe::egui::{self, Color32, Pos2, Rect, Response, Stroke};
use little_clip_core::palette::AppPalette;

/// Reusable interactive before/after split comparison slider.
pub struct SplitCompareView<'a> {
    pub orig_tex: &'a egui::TextureHandle,
    pub proc_tex: Option<&'a egui::TextureHandle>,
    pub split_pos: &'a mut f32, // 0.0 ..= 1.0
    pub is_dragging: &'a mut bool,
}

impl<'a> SplitCompareView<'a> {
    pub fn new(
        orig_tex: &'a egui::TextureHandle,
        proc_tex: Option<&'a egui::TextureHandle>,
        split_pos: &'a mut f32,
        is_dragging: &'a mut bool,
    ) -> Self {
        Self {
            orig_tex,
            proc_tex,
            split_pos,
            is_dragging,
        }
    }

    /// Paints the split comparison inside `img_rect` and handles dragging of the split knob.
    pub fn paint(
        self,
        ui: &mut egui::Ui,
        response: &Response,
        painter: &egui::Painter,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let base_c = hex_to_c32(&palette.harmonies.base);

        let split = (*self.split_pos).clamp(0.02, 0.98);
        let split_x = img_rect.min.x + split * img_rect.width();

        // Left side: original image
        {
            let left_rect = Rect::from_min_max(img_rect.min, Pos2::new(split_x, img_rect.max.y));
            let left_uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(split, 1.0));
            let mut mesh = egui::Mesh::with_texture(self.orig_tex.id());
            mesh.add_rect_with_uv(left_rect, left_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        }

        // Right side: processed preview (or original if not ready)
        {
            let tex_id = self
                .proc_tex
                .map(|t| t.id())
                .unwrap_or_else(|| self.orig_tex.id());
            let right_rect = Rect::from_min_max(Pos2::new(split_x, img_rect.min.y), img_rect.max);
            let right_uv = Rect::from_min_max(Pos2::new(split, 0.0), Pos2::new(1.0, 1.0));
            let mut mesh = egui::Mesh::with_texture(tex_id);
            mesh.add_rect_with_uv(right_rect, right_uv, Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        }

        // Divider line (outer shadow + center light line)
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(3.0_f32, Color32::from_black_alpha(150)),
        );
        painter.line_segment(
            [
                Pos2::new(split_x, img_rect.min.y),
                Pos2::new(split_x, img_rect.max.y),
            ],
            Stroke::new(1.5_f32, Color32::WHITE),
        );

        // Draggable knob
        let knob_center = Pos2::new(split_x, img_rect.center().y);
        let knob_fg = contrast_text_c32(
            b_low,
            Some(hex_to_c32(&palette.colors.text_primary)),
            palette,
        );
        painter.circle(
            knob_center,
            CONTROL_HEIGHT / 2.0,
            b_low,
            Stroke::new(2.0_f32, base_c),
        );
        painter.text(
            knob_center,
            egui::Align2::CENTER_CENTER,
            "◀ ▶",
            egui::FontId::monospace(MONO_FONT_SIZE),
            knob_fg,
        );

        // Drag and click interaction
        let ptr_pos = response
            .hover_pos()
            .or_else(|| response.interact_pointer_pos());
        let near_divider = if let Some(pos) = ptr_pos {
            (pos.x - split_x).abs() <= CONTROL_HEIGHT / 2.0
                && pos.y >= img_rect.min.y
                && pos.y <= img_rect.max.y
        } else {
            false
        };

        if near_divider || *self.is_dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }

        if near_divider && response.drag_started() {
            *self.is_dragging = true;
        }

        if *self.is_dragging {
            if response.dragged() {
                if let Some(pos) = ptr_pos {
                    let norm = ((pos.x - img_rect.min.x) / img_rect.width()).clamp(0.01, 0.99);
                    *self.split_pos = norm;
                }
            }
            if response.drag_stopped() {
                *self.is_dragging = false;
            }
        }

        // Double click resets to 50/50
        if near_divider && response.double_clicked() {
            *self.split_pos = 0.5;
        }
    }
}
