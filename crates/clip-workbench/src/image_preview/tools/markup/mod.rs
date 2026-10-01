mod overlay;
mod panels;
mod raster;

use super::super::*;
use super::crop::{CropHandle, get_box_handles};
use egui::{Align2, FontId};
use little_clip_render::ScreenRenderer;
use little_clip_render::tiny_skia;
use raster::measure_text_skia;
pub use raster::rasterize_markup;

/// Available screenshot markup shape tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkupToolType {
    /// Select, move, and edit placed shapes.
    Select,
    /// Draw directed pointer arrows.
    #[default]
    Arrow,
    /// Draw circular or elliptical callouts.
    Circle,
    /// Draw rectangular callout boxes.
    Rectangle,
    /// Draw straight lines.
    Line,
    /// Draw semi-transparent highlighter stripes.
    Highlighter,
    /// Drop auto-incrementing numbered badge callouts (1, 2, 3...).
    StepBadge,
    /// Drop expanding black boxes with white text.
    Text,
}

impl MarkupToolType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Select => "1  Select",
            Self::Arrow => "2  Arrow",
            Self::Circle => "3  Circle",
            Self::Rectangle => "4  Rectangle",
            Self::Line => "5  Line",
            Self::Highlighter => "6  Highlight",
            Self::StepBadge => "7  Step Badge",
            Self::Text => "8  Text",
        }
    }

    pub fn icon(&self) -> Icon {
        match self {
            Self::Select => Icon::Crosshair,
            Self::Arrow => Icon::ArrowRight,
            Self::Circle => Icon::ShapeCircle,
            Self::Rectangle => Icon::ShapeRect,
            Self::Line => Icon::ShapeLine,
            Self::Highlighter => Icon::Highlight,
            Self::StepBadge => Icon::Badge,
            Self::Text => Icon::Text,
        }
    }
}

/// A placed vector markup object stored in normalized [0.0, 1.0] image space.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkupShape {
    Arrow {
        start: Pos2,
        end: Pos2,
        color: Color32,
        width: f32,
    },
    Circle {
        rect: Rect,
        color: Color32,
        width: f32,
        filled: bool,
    },
    Rectangle {
        rect: Rect,
        color: Color32,
        width: f32,
        filled: bool,
        rounding: f32,
    },
    Line {
        start: Pos2,
        end: Pos2,
        color: Color32,
        width: f32,
    },
    Highlighter {
        rect: Rect,
        color: Color32,
    },
    StepBadge {
        center: Pos2,
        radius: f32,
        number: u32,
        color: Color32,
    },
    Text {
        pos: Pos2,
        text: String,
        font_size: f32,
    },
}

/// Infers the optimal font size from a target box height in pixels,
/// ensuring text fits as large as possible without spilling outside the box.
pub fn infer_font_size_from_box_height(box_h_px: f32) -> f32 {
    (box_h_px / 1.55).clamp(10.0, 300.0)
}

/// Calculates the automatic font size for a step badge number inside a circle of radius `badge_radius_px`.
pub fn step_badge_font_size(badge_radius_px: f32, number: u32) -> f32 {
    let digits = if number == 0 {
        1
    } else {
        (number as f64).log10().floor() as usize + 1
    };
    let scale: f32 = match digits {
        1 => 1.25_f32,
        2 => 0.95_f32,
        3 => 0.70_f32,
        _ => 0.55_f32,
    };
    (badge_radius_px * scale).max(8.0_f32)
}

impl MarkupShape {
    pub fn bounding_rect_norm(&self, img_w: f32, img_h: f32) -> Rect {
        match self {
            Self::Arrow { start, end, .. } | Self::Line { start, end, .. } => {
                Rect::from_two_pos(*start, *end)
            }
            Self::Circle { rect, .. }
            | Self::Rectangle { rect, .. }
            | Self::Highlighter { rect, .. } => *rect,
            Self::StepBadge { center, radius, .. } => {
                let r_px = if *radius <= 1.0_f32 {
                    *radius * img_h
                } else {
                    *radius
                };
                let rx_norm = r_px / img_w.max(1.0_f32);
                let ry_norm = r_px / img_h.max(1.0_f32);
                Rect::from_center_size(*center, Vec2::new(rx_norm * 2.0_f32, ry_norm * 2.0_f32))
            }
            Self::Text {
                pos,
                text,
                font_size,
            } => {
                let pad_x = (font_size * 0.35).clamp(6.0, 24.0);
                let pad_y = (font_size * 0.15).clamp(3.0, 12.0);
                let char_count = if text.is_empty() {
                    16
                } else {
                    text.len().max(1)
                };
                let text_w = (char_count as f32 * font_size * 0.58) + pad_x * 2.0;
                let text_h = font_size * 1.25 + pad_y * 2.0;
                Rect::from_min_size(
                    *pos,
                    Vec2::new(text_w / img_w.max(1.0), text_h / img_h.max(1.0)),
                )
            }
        }
    }

    pub fn translate(&mut self, delta: Vec2) {
        match self {
            Self::Arrow { start, end, .. } | Self::Line { start, end, .. } => {
                *start += delta;
                *end += delta;
            }
            Self::Circle { rect, .. }
            | Self::Rectangle { rect, .. }
            | Self::Highlighter { rect, .. } => {
                *rect = rect.translate(delta);
            }
            Self::StepBadge { center, .. } => {
                *center += delta;
            }
            Self::Text { pos, .. } => {
                *pos += delta;
            }
        }
    }

    #[cfg(test)]
    pub fn color(&self) -> Color32 {
        match self {
            Self::Arrow { color, .. }
            | Self::Circle { color, .. }
            | Self::Rectangle { color, .. }
            | Self::Line { color, .. }
            | Self::Highlighter { color, .. }
            | Self::StepBadge { color, .. } => *color,
            Self::Text { .. } => Color32::WHITE,
        }
    }

    pub fn set_color(&mut self, new_color: Color32) {
        match self {
            Self::Arrow { color, .. }
            | Self::Circle { color, .. }
            | Self::Rectangle { color, .. }
            | Self::Line { color, .. }
            | Self::Highlighter { color, .. }
            | Self::StepBadge { color, .. } => {
                *color = new_color;
            }
            Self::Text { .. } => {}
        }
    }

    #[cfg(test)]
    pub fn width(&self) -> Option<f32> {
        match self {
            Self::Arrow { width, .. }
            | Self::Circle { width, .. }
            | Self::Rectangle { width, .. }
            | Self::Line { width, .. } => Some(*width),
            _ => None,
        }
    }

    pub fn set_width(&mut self, new_width: f32) {
        match self {
            Self::Arrow { width, .. }
            | Self::Circle { width, .. }
            | Self::Rectangle { width, .. }
            | Self::Line { width, .. } => {
                *width = new_width;
            }
            _ => {}
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Arrow { .. } => "Arrow",
            Self::Circle { .. } => "Circle",
            Self::Rectangle { .. } => "Rectangle",
            Self::Line { .. } => "Line",
            Self::Highlighter { .. } => "Highlighter",
            Self::StepBadge { .. } => "Step Badge",
            Self::Text { .. } => "Text Box",
        }
    }

    pub fn type_icon(&self) -> Icon {
        match self {
            Self::Arrow { .. } => Icon::ArrowRight,
            Self::Circle { .. } => Icon::ShapeCircle,
            Self::Rectangle { .. } => Icon::ShapeRect,
            Self::Line { .. } => Icon::ShapeLine,
            Self::Highlighter { .. } => Icon::Highlight,
            Self::StepBadge { .. } => Icon::Badge,
            Self::Text { .. } => Icon::Text,
        }
    }
}

/// In-progress drag operation on canvas.
#[derive(Debug, Clone, PartialEq)]
pub enum MarkupDrag {
    Creating {
        tool: MarkupToolType,
        start_norm: Pos2,
        current_norm: Pos2,
    },
    Moving {
        idx: usize,
        start_ptr: Pos2,
        init_shape: MarkupShape,
    },
    Resizing {
        idx: usize,
        handle: CropHandle,
        start_ptr: Pos2,
        init_rect: Rect,
    },
    ArrowEndpoint {
        idx: usize,
        is_start: bool,
        start_ptr: Pos2,
        init_pos: Pos2,
    },
}

/// State tracking all markup tools, active palette, inspector, and vector layers.
#[derive(Debug, Clone)]
pub struct MarkupState {
    pub items: Vec<MarkupShape>,
    pub selected_idx: Option<usize>,
    pub active_tool: MarkupToolType,
    pub active_color: Color32,
    pub active_stroke_width: f32,
    pub active_font_size: f32,
    pub active_badge_radius: f32,
    pub active_filled: bool,
    pub next_badge_number: u32,
    pub editing_text_idx: Option<usize>,
    pub active_drag: Option<MarkupDrag>,
}

impl Default for MarkupState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected_idx: None,
            active_tool: MarkupToolType::Arrow,
            active_color: Color32::from_rgb(255, 42, 42), // Pure Red
            active_stroke_width: 4.0,
            active_font_size: 20.0,
            active_badge_radius: 28.0,
            active_filled: false,
            next_badge_number: 1,
            editing_text_idx: None,
            active_drag: None,
        }
    }
}

impl MarkupState {
    /// Palette swatch presets: 9 vibrant markup primaries + neutrals.
    pub const PALETTE_SWATCHES: [(&'static str, Color32); 9] = [
        ("Pure Red", Color32::from_rgb(255, 42, 42)),
        ("Yellow", Color32::from_rgb(255, 229, 0)),
        ("Neon Green", Color32::from_rgb(0, 230, 118)),
        ("Cyan", Color32::from_rgb(0, 210, 255)),
        ("Electric Blue", Color32::from_rgb(41, 121, 255)),
        ("Magenta", Color32::from_rgb(255, 0, 127)),
        ("Vivid Orange", Color32::from_rgb(255, 109, 0)),
        ("White", Color32::from_rgb(255, 255, 255)),
        ("Black", Color32::from_rgb(0, 0, 0)),
    ];

    /// Returns the lowest available positive integer (>= 1) not currently used by any placed StepBadge.
    /// Automatically fills in any holes/gaps left by deleted step badges.
    pub fn next_available_badge_number(&self) -> u32 {
        let mut used = std::collections::BTreeSet::new();
        for item in &self.items {
            if let MarkupShape::StepBadge { number, .. } = item {
                used.insert(*number);
            }
        }
        let mut candidate = 1_u32;
        while used.contains(&candidate) {
            candidate += 1;
        }
        candidate
    }

    /// Synchronizes `next_badge_number` with the next available badge number.
    pub fn sync_next_badge_number(&mut self) -> u32 {
        let next = self.next_available_badge_number();
        self.next_badge_number = next;
        next
    }
}

// ── Private Rendering Helpers ──

#[cfg(test)]
mod tests;
