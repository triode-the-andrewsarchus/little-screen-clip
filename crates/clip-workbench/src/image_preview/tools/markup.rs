use super::super::*;
use super::crop::{CropHandle, get_box_handles};
use egui::{Align2, FontId};
use little_clip_render::ScreenRenderer;
use little_clip_render::tiny_skia;

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
    #[allow(dead_code)]
    pub fn bounding_rect(&self) -> Rect {
        self.bounding_rect_norm(1000.0, 1000.0)
    }

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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn radius(&self) -> Option<f32> {
        match self {
            Self::StepBadge { radius, .. } => Some(*radius),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn set_radius(&mut self, new_radius: f32) {
        if let Self::StepBadge { radius, .. } = self {
            *radius = new_radius;
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

impl ImagePreviewState {
    /// Renders the Left-Hand studio pane: Tool picker & color swatches.
    pub(crate) fn render_markup_left_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_med = hex_to_c32(&palette.theme.b_med);
        let f_high = hex_to_c32(&palette.theme.f_high);
        let f_med = hex_to_c32(&palette.theme.f_med);
        let base_accent = hex_to_c32(&palette.harmonies.base);

        ui.vertical(|ui| {
            ui.heading(
                egui::RichText::new("✏ Markup Tools")
                    .strong()
                    .size(15.0)
                    .color(f_high),
            );
            ui.label(
                egui::RichText::new("Vector screenshot markup tools")
                    .small()
                    .color(f_med),
            );
            ui.add_space(8.0);

            ui.label(
                egui::RichText::new("SHAPES & TOOLS")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .small()
                    .color(f_med),
            );
            ui.add_space(4.0);

            let tools = [
                MarkupToolType::Select,
                MarkupToolType::Arrow,
                MarkupToolType::Circle,
                MarkupToolType::Rectangle,
                MarkupToolType::Line,
                MarkupToolType::Highlighter,
                MarkupToolType::StepBadge,
                MarkupToolType::Text,
            ];

            for tool in tools {
                let is_active = self.markup_state.active_tool == tool;
                let btn = crate::ui_ext::mode_tab_button_icon(Some(tool.icon()), tool.label(), is_active, palette)
                    .min_size(Vec2::new(ui.available_width(), 28.0));

                if ui.add(btn).clicked() {
                    self.markup_state.active_tool = tool;
                    if tool != MarkupToolType::Text {
                        self.markup_state.editing_text_idx = None;
                    }
                }
                ui.add_space(2.0);
            }

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);

            ui.label(
                egui::RichText::new("COLOR PALETTE")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .small()
                    .color(f_med),
            );
            ui.add_space(6.0);

            // Swatches grid: 3 columns
            egui::Grid::new("markup_color_swatches")
                .spacing(Vec2::new(6.0, 6.0))
                .show(ui, |ui| {
                    for (i, (name, col)) in MarkupState::PALETTE_SWATCHES.iter().enumerate() {
                        let is_active = self.markup_state.active_color == *col;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());

                        // Swatch background
                        ui.painter().rect_filled(rect, Rounding::same(4.0), *col);
                        // Swatch border
                        let border_stroke = if is_active {
                            Stroke::new(2.5_f32, base_accent)
                        } else {
                            Stroke::new(1.0_f32, b_med)
                        };
                        ui.painter().rect_stroke(rect, Rounding::same(4.0), border_stroke);

                        if resp.on_hover_text(*name).clicked() {
                            self.markup_state.active_color = *col;
                            if let Some(sel) = self.markup_state.selected_idx {
                                if sel < self.markup_state.items.len() {
                                    self.markup_state.items[sel].set_color(*col);
                                }
                            }
                        }

                        if (i + 1) % 3 == 0 {
                            ui.end_row();
                        }
                    }
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Custom:").small().color(f_med));
                let mut custom_c = self.markup_state.active_color;
                if egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut custom_c,
                    egui::color_picker::Alpha::Opaque,
                )
                .changed()
                {
                    self.markup_state.active_color = custom_c;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            self.markup_state.items[sel].set_color(custom_c);
                        }
                    }
                }
            });

            ui.add_space(14.0);
            ui.separator();
            ui.add_space(6.0);

            ui.label(
                egui::RichText::new("SHORTCUTS")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .small()
                    .color(f_med),
            );
            ui.label(
                egui::RichText::new("• Del / Backspace: Delete item\n• Esc: Deselect / Exit edit\n• Enter: Flatten / Burn")
                    .small()
                    .color(hex_to_c32(&palette.theme.f_low)),
            );
        });
    }

    /// Renders the Right-Hand studio pane: Active shape properties, layer list, and burn action.
    pub(crate) fn render_markup_right_panel(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_med = hex_to_c32(&palette.theme.b_med);
        let b_low = hex_to_c32(&palette.theme.b_low);
        let f_high = hex_to_c32(&palette.theme.f_high);
        let f_med = hex_to_c32(&palette.theme.f_med);

        ui.vertical(|ui| {
            ui.heading(
                egui::RichText::new("Properties & Layers")
                    .strong()
                    .size(15.0)
                    .color(f_high),
            );
            ui.add_space(8.0);

            // ── Section 1: Properties ──
            ui.label(
                egui::RichText::new("STYLE & SIZING")
                    .extra_letter_spacing(1.0)
                    .strong()
                    .small()
                    .color(f_med),
            );
            ui.add_space(4.0);

            // Stroke Width Slider
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Stroke:").small().color(f_med));
                let mut stroke_w = self.markup_state.active_stroke_width;
                if ui
                    .add(egui::Slider::new(&mut stroke_w, 1.0..=32.0).suffix(" px"))
                    .changed()
                {
                    self.markup_state.active_stroke_width = stroke_w;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            self.markup_state.items[sel].set_width(stroke_w);
                        }
                    }
                }
            });

            // Font Size Slider
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Font:").small().color(f_med));
                let mut font_sz = self.markup_state.active_font_size;
                if ui
                    .add(egui::Slider::new(&mut font_sz, 10.0..=200.0).suffix(" px"))
                    .changed()
                {
                    self.markup_state.active_font_size = font_sz;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            if let MarkupShape::Text { font_size, .. } =
                                &mut self.markup_state.items[sel]
                            {
                                *font_size = font_sz;
                            }
                        }
                    }
                }
            });

            // Fill Toggle (for circle / rect)
            ui.horizontal(|ui| {
                let mut filled = self.markup_state.active_filled;
                if ui.checkbox(&mut filled, "Fill shapes").changed() {
                    self.markup_state.active_filled = filled;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            match &mut self.markup_state.items[sel] {
                                MarkupShape::Circle { filled: f, .. }
                                | MarkupShape::Rectangle { filled: f, .. } => {
                                    *f = filled;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            });

            // Step Badge counter
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Next Badge:").small().color(f_med));
                let next_num = self.markup_state.sync_next_badge_number();
                ui.label(
                    egui::RichText::new(format!("#{next_num}"))
                        .strong()
                        .color(f_high),
                );
                if ui
                    .button("Reset")
                    .on_hover_text("Reset next badge number to lowest available positive integer")
                    .clicked()
                {
                    self.markup_state.sync_next_badge_number();
                }
            });

            // Badge Size Slider
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Badge:").small().color(f_med));
                let mut badge_r = self.markup_state.active_badge_radius;
                if ui
                    .add(egui::Slider::new(&mut badge_r, 12.0..=300.0).suffix(" px"))
                    .changed()
                {
                    self.markup_state.active_badge_radius = badge_r;
                    if let Some(sel) = self.markup_state.selected_idx {
                        if sel < self.markup_state.items.len() {
                            if let MarkupShape::StepBadge { radius, .. } =
                                &mut self.markup_state.items[sel]
                            {
                                *radius = badge_r;
                            }
                        }
                    }
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Section 2: Layers / Placed Items List ──
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("LAYERS")
                        .extra_letter_spacing(1.0)
                        .strong()
                        .small()
                        .color(f_med),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{} items", self.markup_state.items.len()))
                            .small()
                            .color(f_med),
                    );
                });
            });
            ui.add_space(4.0);

            let list_h = (ui.available_height() - 90.0).max(60.0);
            egui::ScrollArea::vertical()
                .max_height(list_h)
                .show(ui, |ui| {
                    if self.markup_state.items.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(20.0);
                            ui.label(
                                egui::RichText::new(
                                    "No markup shapes placed.\nClick & drag on image to draw.",
                                )
                                .small()
                                .color(hex_to_c32(&palette.theme.f_low)),
                            );
                        });
                    } else {
                        let mut to_remove = None;
                        let mut move_up = None;
                        let mut move_down = None;

                        for (i, item) in self.markup_state.items.iter().enumerate().rev() {
                            let is_selected = self.markup_state.selected_idx == Some(i);
                            let row_bg = if is_selected {
                                hex_to_c32(&palette.theme.b_med)
                            } else {
                                b_low
                            };

                            egui::Frame::none()
                                .fill(row_bg)
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if is_selected {
                                        hex_to_c32(&palette.harmonies.base)
                                    } else {
                                        b_med
                                    },
                                ))
                                .rounding(Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        // Shape icon & name
                                        let label_text = match item {
                                            MarkupShape::Text { text, .. } => {
                                                if text.is_empty() {
                                                    "Text Box".to_string()
                                                } else {
                                                    format!("\"{}\"", text)
                                                }
                                            }
                                            MarkupShape::StepBadge { number, .. } => {
                                                format!("Badge #{}", number)
                                            }
                                            _ => item.type_name().to_string(),
                                        };

                                        let icon_tint = if is_selected {
                                            hex_to_c32(&palette.harmonies.base)
                                        } else {
                                            f_high
                                        };
                                        ui.add(item.type_icon().image_tinted(14.0, icon_tint));

                                        if ui.selectable_label(is_selected, label_text).clicked() {
                                            self.markup_state.selected_idx = Some(i);
                                        }

                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui
                                                    .add(crate::ui_ext::icon_button(
                                                        Icon::Trash,
                                                        12.0,
                                                        b_med,
                                                        None,
                                                        palette,
                                                    ))
                                                    .on_hover_text("Delete")
                                                    .clicked()
                                                {
                                                    to_remove = Some(i);
                                                }
                                                if i + 1 < self.markup_state.items.len() {
                                                    if ui
                                                        .add(crate::ui_ext::icon_button(
                                                            Icon::ArrowUp,
                                                            12.0,
                                                            b_med,
                                                            None,
                                                            palette,
                                                        ))
                                                        .on_hover_text("Bring forward")
                                                        .clicked()
                                                    {
                                                        move_up = Some(i);
                                                    }
                                                }
                                                if i > 0 {
                                                    if ui
                                                        .add(crate::ui_ext::icon_button(
                                                            Icon::ArrowDown,
                                                            12.0,
                                                            b_med,
                                                            None,
                                                            palette,
                                                        ))
                                                        .on_hover_text("Send backward")
                                                        .clicked()
                                                    {
                                                        move_down = Some(i);
                                                    }
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(2.0);
                        }

                        if let Some(idx) = to_remove {
                            self.markup_state.items.remove(idx);
                            if self.markup_state.selected_idx == Some(idx) {
                                self.markup_state.selected_idx = None;
                            } else if let Some(sel) = self.markup_state.selected_idx {
                                if sel > idx {
                                    self.markup_state.selected_idx = Some(sel - 1);
                                }
                            }
                            self.markup_state.sync_next_badge_number();
                        }
                        if let Some(idx) = move_up {
                            self.markup_state.items.swap(idx, idx + 1);
                            self.markup_state.selected_idx = Some(idx + 1);
                        }
                        if let Some(idx) = move_down {
                            self.markup_state.items.swap(idx, idx - 1);
                            self.markup_state.selected_idx = Some(idx - 1);
                        }
                    }
                });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Section 3: Global Actions (Clear All & Flatten) ──
            ui.vertical_centered_justified(|ui| {
                let has_items = !self.markup_state.items.is_empty();

                // Burn / Flatten button
                let burn_bg = if has_items {
                    hex_to_c32(&palette.diff_del)
                } else {
                    b_med
                };
                let burn_btn = crate::ui_ext::action_button_icon(
                    Some(Icon::Check),
                    "Flatten / Burn Layers (Enter)",
                    burn_bg,
                    palette,
                );

                if ui
                    .add_enabled(has_items, burn_btn)
                    .on_hover_text(
                        "Permanently rasterizes vector markup layers into image pixels (Enter)",
                    )
                    .clicked()
                {
                    self.apply_flatten_markup();
                }

                ui.add_space(4.0);

                // Clear All button
                let clear_btn = crate::ui_ext::danger_button_icon(
                    Some(Icon::Close),
                    "Clear All",
                    b_low,
                    palette,
                );

                if ui.add_enabled(has_items, clear_btn).clicked() {
                    self.markup_state.items.clear();
                    self.markup_state.selected_idx = None;
                    self.markup_state.editing_text_idx = None;
                    self.markup_state.sync_next_badge_number();
                    self.show_toast("Cleared all markup items");
                }
            });
        });
    }

    /// Burns all vector markup layers into `current_rgba` permanently, pushing to undo stack.
    pub fn apply_flatten_markup(&mut self) {
        if self.markup_state.items.is_empty() {
            return;
        }

        self.undo_stack.push(self.current_rgba.clone());
        self.redo_stack.clear();

        let count = self.markup_state.items.len();
        self.current_rgba = rasterize_markup(&self.current_rgba, &self.markup_state.items);
        self.markup_state.items.clear();
        self.markup_state.selected_idx = None;
        self.markup_state.editing_text_idx = None;
        self.markup_state.active_drag = None;
        self.markup_state.sync_next_badge_number();
        self.dirty_texture = true;

        self.show_toast(format!(
            "🔥 Flattened {} markup layers into image pixels",
            count
        ));
    }

    /// Renders placed markup layers passively on any preview viewport without interaction.
    pub(crate) fn render_markup_passive(&self, painter: &egui::Painter, img_rect: Rect) {
        let to_screen = |norm_p: Pos2| -> Pos2 {
            Pos2::new(
                img_rect.min.x + norm_p.x * img_rect.width(),
                img_rect.min.y + norm_p.y * img_rect.height(),
            )
        };
        let to_screen_rect = |norm_r: Rect| -> Rect {
            Rect::from_min_max(to_screen(norm_r.min), to_screen(norm_r.max))
        };
        let zoom_scale =
            (img_rect.width() / self.current_rgba.width().max(1) as f32).clamp(0.1_f32, 10.0_f32);

        for shape in &self.markup_state.items {
            match shape {
                MarkupShape::Arrow {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    draw_arrow_painter(painter, s_screen, e_screen, *color, sw);
                }
                MarkupShape::Line {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    painter.line_segment([s_screen, e_screen], Stroke::new(sw, *color));
                }
                MarkupShape::Rectangle {
                    rect,
                    color,
                    width,
                    filled,
                    rounding,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    if *filled {
                        painter.rect_filled(s_rect, Rounding::same(*rounding), *color);
                    } else {
                        painter.rect_stroke(
                            s_rect,
                            Rounding::same(*rounding),
                            Stroke::new(sw, *color),
                        );
                    }
                }
                MarkupShape::Circle {
                    rect,
                    color,
                    width,
                    filled,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5_f32);
                    let center = s_rect.center();
                    let radius = s_rect.width().min(s_rect.height()) * 0.5_f32;

                    if *filled {
                        painter.circle_filled(center, radius, *color);
                    } else {
                        painter.circle_stroke(center, radius, Stroke::new(sw, *color));
                    }
                }
                MarkupShape::Highlighter { rect, color } => {
                    let s_rect = to_screen_rect(*rect);
                    let highlight_col =
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90);
                    painter.rect_filled(s_rect, Rounding::same(2.0_f32), highlight_col);
                }
                MarkupShape::StepBadge {
                    center,
                    radius,
                    number,
                    color,
                } => {
                    let c_screen = to_screen(*center);
                    let img_h = self.current_rgba.height().max(1) as f32;
                    let r_px = if *radius <= 1.0_f32 {
                        *radius * img_h
                    } else {
                        *radius
                    };
                    let badge_radius = (r_px * zoom_scale).max(8.0_f32);
                    painter.circle_filled(c_screen, badge_radius, *color);
                    let stroke_w = (badge_radius * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    painter.circle_stroke(
                        c_screen,
                        badge_radius,
                        Stroke::new(
                            stroke_w,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        ),
                    );
                    let auto_font_size = step_badge_font_size(badge_radius, *number);
                    painter.text(
                        c_screen,
                        Align2::CENTER_CENTER,
                        number.to_string(),
                        FontId::proportional(auto_font_size),
                        Color32::WHITE,
                    );
                }
                MarkupShape::Text {
                    pos,
                    text,
                    font_size,
                } => {
                    if text.is_empty() {
                        continue;
                    }
                    let p_screen = to_screen(*pos);
                    let scaled_font_size = (*font_size * zoom_scale).max(11.0_f32);
                    let galley = painter.layout_no_wrap(
                        text.to_string(),
                        FontId::proportional(scaled_font_size),
                        Color32::WHITE,
                    );
                    let pad_x = (scaled_font_size * 0.35).clamp(6.0_f32, 24.0_f32);
                    let pad_y = (scaled_font_size * 0.15).clamp(3.0_f32, 12.0_f32);
                    let box_rect = Rect::from_min_size(
                        p_screen,
                        galley.size() + Vec2::new(pad_x * 2.0_f32, pad_y * 2.0_f32),
                    );

                    painter.rect_filled(
                        box_rect,
                        Rounding::same(3.0_f32),
                        Color32::from_rgba_unmultiplied(12, 12, 12, 245),
                    );
                    painter.rect_stroke(
                        box_rect,
                        Rounding::same(3.0_f32),
                        Stroke::new(1.0_f32, Color32::from_gray(70)),
                    );
                    painter.galley(p_screen + Vec2::new(pad_x, pad_y), galley, Color32::WHITE);
                }
            }
        }
    }

    /// Interactive canvas overlay rendering all placed shapes, handles, and inline text editing.
    pub(crate) fn render_markup_overlay(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        response: &egui::Response,
        img_rect: Rect,
        palette: &AppPalette,
    ) {
        let to_screen = |norm_p: Pos2| -> Pos2 {
            Pos2::new(
                img_rect.min.x + norm_p.x * img_rect.width(),
                img_rect.min.y + norm_p.y * img_rect.height(),
            )
        };
        let to_screen_rect = |norm_r: Rect| -> Rect {
            Rect::from_min_max(to_screen(norm_r.min), to_screen(norm_r.max))
        };
        let to_norm = |screen_p: Pos2| -> Pos2 {
            Pos2::new(
                ((screen_p.x - img_rect.min.x) / img_rect.width()).clamp(0.0, 1.0),
                ((screen_p.y - img_rect.min.y) / img_rect.height()).clamp(0.0, 1.0),
            )
        };

        let img_w = self.current_rgba.width().max(1) as f32;
        let img_h = self.current_rgba.height().max(1) as f32;
        let zoom_scale = (img_rect.width() / img_w).clamp(0.1, 10.0);
        let base_accent = hex_to_c32(&palette.harmonies.base);
        let handle_radius = 5.0;
        let hit_dist = 12.0;

        // ── 1. Draw Placed Shapes ──
        for (i, shape) in self.markup_state.items.iter_mut().enumerate() {
            let is_selected = self.markup_state.selected_idx == Some(i);

            match shape {
                MarkupShape::Arrow {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5);
                    draw_arrow_painter(painter, s_screen, e_screen, *color, sw);

                    if is_selected {
                        draw_handle(painter, s_screen, handle_radius, base_accent);
                        draw_handle(painter, e_screen, handle_radius, base_accent);
                    }
                }
                MarkupShape::Line {
                    start,
                    end,
                    color,
                    width,
                } => {
                    let s_screen = to_screen(*start);
                    let e_screen = to_screen(*end);
                    let sw = (*width * zoom_scale).max(1.5);
                    painter.line_segment([s_screen, e_screen], Stroke::new(sw, *color));

                    if is_selected {
                        draw_handle(painter, s_screen, handle_radius, base_accent);
                        draw_handle(painter, e_screen, handle_radius, base_accent);
                    }
                }
                MarkupShape::Rectangle {
                    rect,
                    color,
                    width,
                    filled,
                    rounding,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5);
                    if *filled {
                        painter.rect_filled(s_rect, Rounding::same(*rounding), *color);
                    } else {
                        painter.rect_stroke(
                            s_rect,
                            Rounding::same(*rounding),
                            Stroke::new(sw, *color),
                        );
                    }

                    if is_selected {
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::Circle {
                    rect,
                    color,
                    width,
                    filled,
                } => {
                    let s_rect = to_screen_rect(*rect);
                    let sw = (*width * zoom_scale).max(1.5);
                    let center = s_rect.center();
                    let radius = s_rect.width().min(s_rect.height()) * 0.5;

                    if *filled {
                        painter.circle_filled(center, radius, *color);
                    } else {
                        painter.circle_stroke(center, radius, Stroke::new(sw, *color));
                    }

                    if is_selected {
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::Highlighter { rect, color } => {
                    let s_rect = to_screen_rect(*rect);
                    let highlight_col =
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90);
                    painter.rect_filled(s_rect, Rounding::same(2.0), highlight_col);

                    if is_selected {
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::StepBadge {
                    center,
                    radius,
                    number,
                    color,
                } => {
                    let c_screen = to_screen(*center);
                    let r_px = if *radius <= 1.0_f32 {
                        *radius * img_h
                    } else {
                        *radius
                    };
                    let badge_radius = (r_px * zoom_scale).max(8.0_f32);
                    painter.circle_filled(c_screen, badge_radius, *color);
                    let stroke_w = (badge_radius * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    painter.circle_stroke(
                        c_screen,
                        badge_radius,
                        Stroke::new(
                            stroke_w,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        ),
                    );
                    let auto_font_size = step_badge_font_size(badge_radius, *number);
                    painter.text(
                        c_screen,
                        Align2::CENTER_CENTER,
                        number.to_string(),
                        FontId::proportional(auto_font_size),
                        Color32::WHITE,
                    );

                    if is_selected {
                        let s_rect =
                            Rect::from_center_size(c_screen, Vec2::splat(badge_radius * 2.0_f32));
                        draw_selection_bounding_box(painter, s_rect, base_accent, handle_radius);
                    }
                }
                MarkupShape::Text {
                    pos,
                    text,
                    font_size,
                } => {
                    let p_screen = to_screen(*pos);
                    let scaled_font_size = (*font_size * zoom_scale).max(11.0);
                    let is_editing = self.markup_state.editing_text_idx == Some(i);

                    let display_str = if text.is_empty() {
                        "Type text here..."
                    } else {
                        text.as_str()
                    };

                    let galley = painter.layout_no_wrap(
                        display_str.to_string(),
                        FontId::proportional(scaled_font_size),
                        if text.is_empty() {
                            Color32::from_gray(160)
                        } else {
                            Color32::WHITE
                        },
                    );

                    let pad_x = (scaled_font_size * 0.35).clamp(6.0, 24.0);
                    let pad_y = (scaled_font_size * 0.15).clamp(3.0, 12.0);
                    let edit_w = (galley.size().x + scaled_font_size * 0.5).max(60.0);
                    let box_w = if is_editing {
                        (galley.size().x + pad_x * 2.0).max(edit_w + pad_x * 2.0)
                    } else {
                        galley.size().x + pad_x * 2.0
                    };
                    let box_h = galley.size().y + pad_y * 2.0;
                    let box_rect = Rect::from_min_size(p_screen, Vec2::new(box_w, box_h));

                    // Solid black expanding box
                    painter.rect_filled(
                        box_rect,
                        Rounding::same(3.0),
                        Color32::from_rgba_unmultiplied(12, 12, 12, 245),
                    );
                    painter.rect_stroke(
                        box_rect,
                        Rounding::same(3.0),
                        Stroke::new(
                            1.0_f32,
                            if is_editing || is_selected {
                                base_accent
                            } else {
                                Color32::from_gray(70)
                            },
                        ),
                    );

                    if is_editing {
                        let mut temp_text = text.clone();
                        let text_edit = egui::TextEdit::singleline(&mut temp_text)
                            .font(FontId::proportional(scaled_font_size))
                            .text_color(Color32::WHITE)
                            .frame(false)
                            .desired_width(edit_w);

                        let edit_rect = Rect::from_min_size(
                            p_screen + Vec2::new(pad_x, pad_y),
                            Vec2::new(edit_w, galley.size().y.max(scaled_font_size)),
                        );

                        let res = ui.put(edit_rect, text_edit);
                        res.request_focus();
                        if res.changed() {
                            *text = temp_text;
                        }

                        if ui.input(|inp| {
                            inp.key_pressed(egui::Key::Enter) || inp.key_pressed(egui::Key::Escape)
                        }) {
                            self.markup_state.editing_text_idx = None;
                        }
                    } else {
                        painter.galley(
                            p_screen + Vec2::new(pad_x, pad_y),
                            galley,
                            if text.is_empty() {
                                Color32::from_gray(160)
                            } else {
                                Color32::WHITE
                            },
                        );

                        if is_selected {
                            draw_selection_bounding_box(
                                painter,
                                box_rect,
                                base_accent,
                                handle_radius,
                            );
                        }
                    }
                }
            }
        }

        // ── 2. Draw In-Progress Drag / Creation ──
        if let Some(MarkupDrag::Creating {
            tool,
            start_norm,
            current_norm,
        }) = self.markup_state.active_drag
        {
            let s_screen = to_screen(start_norm);
            let c_screen = to_screen(current_norm);
            let active_col = self.markup_state.active_color;
            let sw = (self.markup_state.active_stroke_width * zoom_scale).max(1.5);

            match tool {
                MarkupToolType::Arrow => {
                    draw_arrow_painter(painter, s_screen, c_screen, active_col, sw);
                }
                MarkupToolType::Line => {
                    painter.line_segment([s_screen, c_screen], Stroke::new(sw, active_col));
                }
                MarkupToolType::Rectangle => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    if self.markup_state.active_filled {
                        painter.rect_filled(r, Rounding::same(3.0), active_col);
                    } else {
                        painter.rect_stroke(r, Rounding::same(3.0), Stroke::new(sw, active_col));
                    }
                }
                MarkupToolType::Circle => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    let radius = r.width().min(r.height()) * 0.5;
                    if self.markup_state.active_filled {
                        painter.circle_filled(r.center(), radius, active_col);
                    } else {
                        painter.circle_stroke(r.center(), radius, Stroke::new(sw, active_col));
                    }
                }
                MarkupToolType::Highlighter => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    let highlight_col = Color32::from_rgba_unmultiplied(
                        active_col.r(),
                        active_col.g(),
                        active_col.b(),
                        90,
                    );
                    painter.rect_filled(r, Rounding::same(2.0), highlight_col);
                }
                MarkupToolType::Text => {
                    let r = Rect::from_two_pos(s_screen, c_screen);
                    let box_h_img = (current_norm.y - start_norm.y).abs() * img_h;
                    let font_size = if box_h_img >= 10.0 {
                        infer_font_size_from_box_height(box_h_img)
                    } else {
                        self.markup_state.active_font_size
                    };
                    let scaled_font_size = (font_size * zoom_scale).max(9.0);

                    // Solid dark background for the box preview
                    painter.rect_filled(
                        r,
                        Rounding::same(3.0),
                        Color32::from_rgba_unmultiplied(12, 12, 12, 240),
                    );
                    painter.rect_stroke(r, Rounding::same(3.0), Stroke::new(1.5_f32, base_accent));

                    // Live preview of text fitting inside the box height
                    if r.height() >= 12.0 {
                        let galley = painter.layout_no_wrap(
                            "Aa Text".to_string(),
                            FontId::proportional(scaled_font_size),
                            Color32::from_gray(180),
                        );
                        let pad_x = (scaled_font_size * 0.35).clamp(6.0, 24.0);
                        let text_pos =
                            Pos2::new(r.min.x + pad_x, r.center().y - galley.size().y * 0.5);
                        painter.galley(text_pos, galley, Color32::from_gray(180));
                    }
                }
                MarkupToolType::StepBadge => {
                    let drag_dist = s_screen.distance(c_screen);
                    let radius_screen = if drag_dist >= 8.0_f32 {
                        drag_dist
                    } else {
                        (self.markup_state.active_badge_radius * zoom_scale).max(10.0_f32)
                    };

                    painter.circle_filled(s_screen, radius_screen, active_col);
                    let stroke_w = (radius_screen * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    painter.circle_stroke(
                        s_screen,
                        radius_screen,
                        Stroke::new(
                            stroke_w,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 220),
                        ),
                    );
                    let next_num = self.markup_state.next_available_badge_number();
                    let auto_font_size = step_badge_font_size(radius_screen, next_num);
                    painter.text(
                        s_screen,
                        Align2::CENTER_CENTER,
                        next_num.to_string(),
                        FontId::proportional(auto_font_size),
                        Color32::WHITE,
                    );
                }
                _ => {}
            }
        }

        // ── 3. Cursor management ──
        if let Some(ptr_pos) = response.hover_pos() {
            if self.markup_state.active_tool == MarkupToolType::Select {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Default);
            } else if self.markup_state.active_tool == MarkupToolType::Text {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Text);
            } else {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
            }

            // Check hovered handles on selected shape
            if let Some(sel_idx) = self.markup_state.selected_idx {
                if sel_idx < self.markup_state.items.len() {
                    let shape = &self.markup_state.items[sel_idx];
                    match shape {
                        MarkupShape::Arrow { start, end, .. }
                        | MarkupShape::Line { start, end, .. } => {
                            let s = to_screen(*start);
                            let e = to_screen(*end);
                            if s.distance(ptr_pos) <= hit_dist || e.distance(ptr_pos) <= hit_dist {
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            }
                        }
                        _ => {
                            let s_rect = to_screen_rect(shape.bounding_rect_norm(img_w, img_h));
                            let handles = get_box_handles(s_rect);
                            if let Some((_, _, cur)) = handles
                                .iter()
                                .find(|(_, pos, _)| pos.distance(ptr_pos) <= hit_dist)
                            {
                                ui.output_mut(|o| o.cursor_icon = *cur);
                            } else if s_rect.contains(ptr_pos) {
                                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Move);
                            }
                        }
                    }
                }
            }
        }

        // ── 4. Mouse Input Handling ──
        if response.drag_started() {
            if let Some(ptr) = response.interact_pointer_pos() {
                let mut handled = false;

                // 4a. Check if clicking on active selection handles
                if let Some(sel_idx) = self.markup_state.selected_idx {
                    if sel_idx < self.markup_state.items.len() {
                        let shape = &self.markup_state.items[sel_idx];
                        match shape {
                            MarkupShape::Arrow { start, end, .. }
                            | MarkupShape::Line { start, end, .. } => {
                                let s = to_screen(*start);
                                let e = to_screen(*end);
                                if s.distance(ptr) <= hit_dist {
                                    self.markup_state.active_drag =
                                        Some(MarkupDrag::ArrowEndpoint {
                                            idx: sel_idx,
                                            is_start: true,
                                            start_ptr: ptr,
                                            init_pos: *start,
                                        });
                                    handled = true;
                                } else if e.distance(ptr) <= hit_dist {
                                    self.markup_state.active_drag =
                                        Some(MarkupDrag::ArrowEndpoint {
                                            idx: sel_idx,
                                            is_start: false,
                                            start_ptr: ptr,
                                            init_pos: *end,
                                        });
                                    handled = true;
                                }
                            }
                            _ => {
                                let s_rect = to_screen_rect(shape.bounding_rect_norm(img_w, img_h));
                                let handles = get_box_handles(s_rect);
                                if let Some((h, pos, _)) =
                                    handles.iter().find(|(_, p, _)| p.distance(ptr) <= hit_dist)
                                {
                                    self.markup_state.active_drag = Some(MarkupDrag::Resizing {
                                        idx: sel_idx,
                                        handle: *h,
                                        start_ptr: *pos,
                                        init_rect: shape.bounding_rect_norm(img_w, img_h),
                                    });
                                    handled = true;
                                }
                            }
                        }
                    }
                }

                // 4b. Check if clicking an existing shape body
                if !handled {
                    let clicked_shape =
                        self.markup_state
                            .items
                            .iter()
                            .enumerate()
                            .rev()
                            .find_map(|(i, s)| {
                                let s_rect = to_screen_rect(s.bounding_rect_norm(img_w, img_h));
                                if s_rect.contains(ptr) { Some(i) } else { None }
                            });

                    if let Some(found_idx) = clicked_shape {
                        self.markup_state.selected_idx = Some(found_idx);
                        let init_shape = self.markup_state.items[found_idx].clone();
                        self.markup_state.active_drag = Some(MarkupDrag::Moving {
                            idx: found_idx,
                            start_ptr: ptr,
                            init_shape,
                        });
                        handled = true;

                        if let MarkupShape::Text { .. } = &self.markup_state.items[found_idx] {
                            if self.markup_state.active_tool == MarkupToolType::Text
                                || response.double_clicked()
                            {
                                self.markup_state.editing_text_idx = Some(found_idx);
                            }
                        }
                    }
                }

                // 4c. Start creating a new shape if in drawing tool
                if !handled && img_rect.contains(ptr) {
                    let norm = to_norm(ptr);
                    match self.markup_state.active_tool {
                        MarkupToolType::Select => {
                            self.markup_state.selected_idx = None;
                            self.markup_state.editing_text_idx = None;
                        }
                        drawing_tool => {
                            self.markup_state.selected_idx = None;
                            self.markup_state.editing_text_idx = None;
                            self.markup_state.active_drag = Some(MarkupDrag::Creating {
                                tool: drawing_tool,
                                start_norm: norm,
                                current_norm: norm,
                            });
                        }
                    }
                }
            }
        }

        if response.dragged() {
            if let Some(ptr) = response.interact_pointer_pos() {
                match self.markup_state.active_drag {
                    Some(MarkupDrag::Creating {
                        tool, start_norm, ..
                    }) => {
                        let current_norm = to_norm(ptr);
                        self.markup_state.active_drag = Some(MarkupDrag::Creating {
                            tool,
                            start_norm,
                            current_norm,
                        });
                    }
                    Some(MarkupDrag::Moving {
                        idx,
                        start_ptr,
                        ref init_shape,
                    }) => {
                        let delta_screen = ptr - start_ptr;
                        let delta_norm = Vec2::new(
                            delta_screen.x / img_rect.width(),
                            delta_screen.y / img_rect.height(),
                        );
                        if idx < self.markup_state.items.len() {
                            let mut updated = init_shape.clone();
                            updated.translate(delta_norm);
                            self.markup_state.items[idx] = updated;
                        }
                    }
                    Some(MarkupDrag::ArrowEndpoint {
                        idx,
                        is_start,
                        start_ptr,
                        init_pos,
                    }) => {
                        let delta_screen = ptr - start_ptr;
                        let delta_norm = Vec2::new(
                            delta_screen.x / img_rect.width(),
                            delta_screen.y / img_rect.height(),
                        );
                        let new_pos =
                            (init_pos + delta_norm).clamp(Pos2::ZERO, Pos2::new(1.0, 1.0));
                        if idx < self.markup_state.items.len() {
                            match &mut self.markup_state.items[idx] {
                                MarkupShape::Arrow { start, end, .. }
                                | MarkupShape::Line { start, end, .. } => {
                                    if is_start {
                                        *start = new_pos;
                                    } else {
                                        *end = new_pos;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(MarkupDrag::Resizing {
                        idx,
                        handle,
                        start_ptr,
                        init_rect,
                    }) => {
                        let delta_screen = ptr - start_ptr;
                        let delta_norm_x = delta_screen.x / img_rect.width();
                        let delta_norm_y = delta_screen.y / img_rect.height();

                        let mut min_x = init_rect.min.x;
                        let mut max_x = init_rect.max.x;
                        let mut min_y = init_rect.min.y;
                        let mut max_y = init_rect.max.y;

                        match handle {
                            CropHandle::TopLeft => {
                                min_x = (min_x + delta_norm_x).clamp(0.0, max_x - 0.01);
                                min_y = (min_y + delta_norm_y).clamp(0.0, max_y - 0.01);
                            }
                            CropHandle::TopRight => {
                                max_x = (max_x + delta_norm_x).clamp(min_x + 0.01, 1.0);
                                min_y = (min_y + delta_norm_y).clamp(0.0, max_y - 0.01);
                            }
                            CropHandle::BottomLeft => {
                                min_x = (min_x + delta_norm_x).clamp(0.0, max_x - 0.01);
                                max_y = (max_y + delta_norm_y).clamp(min_y + 0.01, 1.0);
                            }
                            CropHandle::BottomRight => {
                                max_x = (max_x + delta_norm_x).clamp(min_x + 0.01, 1.0);
                                max_y = (max_y + delta_norm_y).clamp(min_y + 0.01, 1.0);
                            }
                            CropHandle::Left => {
                                min_x = (min_x + delta_norm_x).clamp(0.0, max_x - 0.01);
                            }
                            CropHandle::Right => {
                                max_x = (max_x + delta_norm_x).clamp(min_x + 0.01, 1.0);
                            }
                            CropHandle::Top => {
                                min_y = (min_y + delta_norm_y).clamp(0.0, max_y - 0.01);
                            }
                            CropHandle::Bottom => {
                                max_y = (max_y + delta_norm_y).clamp(min_y + 0.01, 1.0);
                            }
                            _ => {}
                        }

                        let new_rect =
                            Rect::from_min_max(Pos2::new(min_x, min_y), Pos2::new(max_x, max_y));
                        if idx < self.markup_state.items.len() {
                            match &mut self.markup_state.items[idx] {
                                MarkupShape::Circle { rect, .. }
                                | MarkupShape::Rectangle { rect, .. }
                                | MarkupShape::Highlighter { rect, .. } => {
                                    *rect = new_rect;
                                }
                                MarkupShape::StepBadge { center, radius, .. } => {
                                    *center = new_rect.center();
                                    let new_r_norm = new_rect.width().min(new_rect.height()) * 0.5;
                                    let new_r_px = new_r_norm * img_h;
                                    if new_r_px >= 8.0 {
                                        *radius = new_r_px;
                                        self.markup_state.active_badge_radius = new_r_px;
                                    }
                                }
                                MarkupShape::Text { pos, font_size, .. } => {
                                    *pos = new_rect.min;
                                    let new_h_px = new_rect.height() * img_h;
                                    if new_h_px >= 10.0 {
                                        *font_size = infer_font_size_from_box_height(new_h_px);
                                        self.markup_state.active_font_size = *font_size;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    None => {}
                }
            }
        }

        if response.drag_stopped() {
            if let Some(MarkupDrag::Creating {
                tool,
                start_norm,
                current_norm,
            }) = self.markup_state.active_drag
            {
                let active_col = self.markup_state.active_color;
                let stroke_w = self.markup_state.active_stroke_width;
                let filled = self.markup_state.active_filled;

                let min_p = Pos2::new(
                    start_norm.x.min(current_norm.x),
                    start_norm.y.min(current_norm.y),
                );
                let max_p = Pos2::new(
                    start_norm.x.max(current_norm.x),
                    start_norm.y.max(current_norm.y),
                );
                let rect = Rect::from_min_max(min_p, max_p);

                let shape = match tool {
                    MarkupToolType::Arrow => Some(MarkupShape::Arrow {
                        start: start_norm,
                        end: current_norm,
                        color: active_col,
                        width: stroke_w,
                    }),
                    MarkupToolType::Line => Some(MarkupShape::Line {
                        start: start_norm,
                        end: current_norm,
                        color: active_col,
                        width: stroke_w,
                    }),
                    MarkupToolType::Rectangle => Some(MarkupShape::Rectangle {
                        rect,
                        color: active_col,
                        width: stroke_w,
                        filled,
                        rounding: 3.0,
                    }),
                    MarkupToolType::Circle => Some(MarkupShape::Circle {
                        rect,
                        color: active_col,
                        width: stroke_w,
                        filled,
                    }),
                    MarkupToolType::Highlighter => Some(MarkupShape::Highlighter {
                        rect,
                        color: active_col,
                    }),
                    MarkupToolType::Text => {
                        let box_h_img = rect.height() * img_h;
                        let font_size = if box_h_img >= 10.0 {
                            infer_font_size_from_box_height(box_h_img)
                        } else {
                            self.markup_state.active_font_size
                        };
                        self.markup_state.active_font_size = font_size;
                        Some(MarkupShape::Text {
                            pos: rect.min,
                            text: String::new(),
                            font_size,
                        })
                    }
                    MarkupToolType::StepBadge => {
                        let s_screen = to_screen(start_norm);
                        let c_screen = to_screen(current_norm);
                        let drag_dist = s_screen.distance(c_screen);
                        let radius_px = if drag_dist >= 8.0_f32 {
                            let r_img = drag_dist / zoom_scale;
                            self.markup_state.active_badge_radius = r_img;
                            r_img
                        } else {
                            self.markup_state.active_badge_radius
                        };

                        let num = self.markup_state.next_available_badge_number();
                        Some(MarkupShape::StepBadge {
                            center: start_norm,
                            radius: radius_px,
                            number: num,
                            color: active_col,
                        })
                    }
                    _ => None,
                };

                if let Some(new_shape) = shape {
                    let is_text = matches!(new_shape, MarkupShape::Text { .. });
                    self.markup_state.items.push(new_shape);
                    self.markup_state.sync_next_badge_number();
                    let new_idx = self.markup_state.items.len() - 1;
                    self.markup_state.selected_idx = Some(new_idx);
                    if is_text {
                        self.markup_state.editing_text_idx = Some(new_idx);
                    }
                }
            }
            self.markup_state.active_drag = None;
        }

        // Cleanup empty text shapes when not actively editing
        if self.markup_state.editing_text_idx.is_none() {
            let before_len = self.markup_state.items.len();
            self.markup_state.items.retain(|item| match item {
                MarkupShape::Text { text, .. } => !text.trim().is_empty(),
                _ => true,
            });
            if self.markup_state.items.len() < before_len {
                if let Some(sel) = self.markup_state.selected_idx {
                    if sel >= self.markup_state.items.len() {
                        self.markup_state.selected_idx = None;
                    }
                }
            }
        }
    }
}

// ── Private Rendering Helpers ──

fn draw_handle(painter: &egui::Painter, center: Pos2, radius: f32, fill_col: Color32) {
    let r = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    painter.rect_filled(r, Rounding::same(1.5_f32), fill_col);
    painter.rect_stroke(
        r,
        Rounding::same(1.5_f32),
        Stroke::new(1.0_f32, Color32::BLACK),
    );
}

fn draw_selection_bounding_box(
    painter: &egui::Painter,
    rect: Rect,
    accent: Color32,
    handle_radius: f32,
) {
    painter.rect_stroke(rect, Rounding::ZERO, Stroke::new(1.5_f32, accent));
    let handles = get_box_handles(rect);
    for (_, pos, _) in &handles {
        draw_handle(painter, *pos, handle_radius, Color32::WHITE);
    }
}

fn draw_arrow_painter(painter: &egui::Painter, start: Pos2, end: Pos2, color: Color32, width: f32) {
    let dir = end - start;
    let len = dir.length();
    if len < 2.0 {
        return;
    }

    let norm_dir = dir / len;
    let angle = dir.y.atan2(dir.x);
    let head_len = (width * 3.5).clamp(12.0, 36.0);
    let head_angle = 30.0_f32.to_radians();

    // Shorten line slightly so it ends flush behind arrow tip
    let line_end = end - norm_dir * (head_len * 0.4);
    painter.line_segment([start, line_end], Stroke::new(width, color));

    let p1 = end - Vec2::angled(angle - head_angle) * head_len;
    let p2 = end - Vec2::angled(angle + head_angle) * head_len;

    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(end, color);
    mesh.colored_vertex(p1, color);
    mesh.colored_vertex(p2, color);
    mesh.add_triangle(0, 1, 2);
    painter.add(egui::Shape::mesh(mesh));
}

// ── Tiny-Skia & Fontdue Image Rasterization ──

/// Rasterizes all placed vector markup layers onto a clean clone of the input image.
pub fn rasterize_markup(base: &RgbaImage, items: &[MarkupShape]) -> RgbaImage {
    if items.is_empty() {
        return base.clone();
    }
    let (w, h) = (base.width(), base.height());
    if w == 0 || h == 0 {
        return base.clone();
    }

    let mut pixmap = match tiny_skia::Pixmap::new(w, h) {
        Some(p) => p,
        None => return base.clone(),
    };

    let (font_reg, font_bold, _, _) = ScreenRenderer::load_fonts();
    let font_ref = font_bold.as_ref().or(font_reg.as_ref());

    for item in items {
        match item {
            MarkupShape::Arrow {
                start,
                end,
                color,
                width,
            } => {
                let sx = start.x * w as f32;
                let sy = start.y * h as f32;
                let ex = end.x * w as f32;
                let ey = end.y * h as f32;

                let mut stroke = tiny_skia::Stroke::default();
                stroke.width = *width;
                stroke.line_cap = tiny_skia::LineCap::Round;

                let mut pb = tiny_skia::PathBuilder::new();
                pb.move_to(sx, sy);
                pb.line_to(ex, ey);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.stroke_path(
                        &path,
                        &paint,
                        &stroke,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }

                // Arrow head
                let angle = (ey - sy).atan2(ex - sx);
                let head_len = (*width * 3.5).clamp(12.0, 48.0);
                let head_angle = 30.0_f32.to_radians();
                let p1x = ex - (angle - head_angle).cos() * head_len;
                let p1y = ey - (angle - head_angle).sin() * head_len;
                let p2x = ex - (angle + head_angle).cos() * head_len;
                let p2y = ey - (angle + head_angle).sin() * head_len;

                let mut head_pb = tiny_skia::PathBuilder::new();
                head_pb.move_to(ex, ey);
                head_pb.line_to(p1x, p1y);
                head_pb.line_to(p2x, p2y);
                head_pb.close();
                if let Some(head_path) = head_pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.fill_path(
                        &head_path,
                        &paint,
                        tiny_skia::FillRule::Winding,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
            MarkupShape::Line {
                start,
                end,
                color,
                width,
            } => {
                let sx = start.x * w as f32;
                let sy = start.y * h as f32;
                let ex = end.x * w as f32;
                let ey = end.y * h as f32;

                let mut stroke = tiny_skia::Stroke::default();
                stroke.width = *width;
                stroke.line_cap = tiny_skia::LineCap::Round;

                let mut pb = tiny_skia::PathBuilder::new();
                pb.move_to(sx, sy);
                pb.line_to(ex, ey);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.stroke_path(
                        &path,
                        &paint,
                        &stroke,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
            MarkupShape::Rectangle {
                rect,
                color,
                width,
                filled,
                ..
            } => {
                let rx = rect.min.x * w as f32;
                let ry = rect.min.y * h as f32;
                let rw = (rect.width() * w as f32).max(1.0);
                let rh = (rect.height() * h as f32).max(1.0);

                if let Some(skia_rect) = tiny_skia::Rect::from_xywh(rx, ry, rw, rh) {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    if *filled {
                        pixmap.fill_rect(skia_rect, &paint, tiny_skia::Transform::identity(), None);
                    } else {
                        let mut stroke = tiny_skia::Stroke::default();
                        stroke.width = *width;
                        let path = tiny_skia::PathBuilder::from_rect(skia_rect);
                        pixmap.stroke_path(
                            &path,
                            &paint,
                            &stroke,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
            }
            MarkupShape::Circle {
                rect,
                color,
                width,
                filled,
            } => {
                let cx = (rect.min.x + rect.width() * 0.5) * w as f32;
                let cy = (rect.min.y + rect.height() * 0.5) * h as f32;
                let rad = ((rect.width().min(rect.height()) * 0.5) * w as f32).max(1.0);

                let mut pb = tiny_skia::PathBuilder::new();
                pb.push_circle(cx, cy, rad);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    if *filled {
                        pixmap.fill_path(
                            &path,
                            &paint,
                            tiny_skia::FillRule::Winding,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    } else {
                        let mut stroke = tiny_skia::Stroke::default();
                        stroke.width = *width;
                        pixmap.stroke_path(
                            &path,
                            &paint,
                            &stroke,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
            }
            MarkupShape::Highlighter { rect, color } => {
                let rx = rect.min.x * w as f32;
                let ry = rect.min.y * h as f32;
                let rw = (rect.width() * w as f32).max(1.0);
                let rh = (rect.height() * h as f32).max(1.0);

                if let Some(skia_rect) = tiny_skia::Rect::from_xywh(rx, ry, rw, rh) {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), 90);
                    pixmap.fill_rect(skia_rect, &paint, tiny_skia::Transform::identity(), None);
                }
            }
            MarkupShape::StepBadge {
                center,
                radius,
                number,
                color,
            } => {
                let cx = center.x * w as f32;
                let cy = center.y * h as f32;
                let r_px = if *radius <= 1.0_f32 {
                    *radius * h as f32
                } else {
                    *radius
                };
                let badge_rad = r_px.max(8.0_f32);

                let mut pb = tiny_skia::PathBuilder::new();
                pb.push_circle(cx, cy, badge_rad);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.fill_path(
                        &path,
                        &paint,
                        tiny_skia::FillRule::Winding,
                        tiny_skia::Transform::identity(),
                        None,
                    );

                    let mut stroke = tiny_skia::Stroke::default();
                    stroke.width = (badge_rad * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    let mut border_paint = tiny_skia::Paint::default();
                    border_paint.set_color_rgba8(255, 255, 255, 220);
                    pixmap.stroke_path(
                        &path,
                        &border_paint,
                        &stroke,
                        tiny_skia::Transform::identity(),
                        None,
                    );

                    if let Some(font) = font_ref {
                        let text_str = number.to_string();
                        let font_size = step_badge_font_size(badge_rad, *number);
                        draw_text_skia(
                            &mut pixmap,
                            font,
                            &text_str,
                            cx,
                            cy - font_size * 0.46_f32,
                            font_size,
                            [255, 255, 255, 255],
                            true,
                        );
                    }
                }
            }
            MarkupShape::Text {
                pos,
                text,
                font_size,
            } => {
                if text.is_empty() {
                    continue;
                }
                let tx = pos.x * w as f32;
                let ty = pos.y * h as f32;
                let pad_x = (*font_size * 0.35).clamp(6.0, 24.0);
                let pad_y = (*font_size * 0.15).clamp(3.0, 12.0);

                if let Some(font) = font_ref {
                    let text_w = measure_text_skia(font, text, *font_size);
                    let text_h = *font_size * 1.25;
                    let box_w = text_w + pad_x * 2.0;
                    let box_h = text_h + pad_y * 2.0;

                    if let Some(box_rect) = tiny_skia::Rect::from_xywh(tx, ty, box_w, box_h) {
                        let mut bg_paint = tiny_skia::Paint::default();
                        bg_paint.set_color_rgba8(12, 12, 12, 245);
                        pixmap.fill_rect(
                            box_rect,
                            &bg_paint,
                            tiny_skia::Transform::identity(),
                            None,
                        );

                        let mut stroke = tiny_skia::Stroke::default();
                        stroke.width = 1.5;
                        let mut stroke_paint = tiny_skia::Paint::default();
                        stroke_paint.set_color_rgba8(75, 75, 75, 255);
                        let border_path = tiny_skia::PathBuilder::from_rect(box_rect);
                        pixmap.stroke_path(
                            &border_path,
                            &stroke_paint,
                            &stroke,
                            tiny_skia::Transform::identity(),
                            None,
                        );

                        draw_text_skia(
                            &mut pixmap,
                            font,
                            text,
                            tx + pad_x,
                            ty + pad_y,
                            *font_size,
                            [255, 255, 255, 255],
                            false,
                        );
                    }
                }
            }
        }
    }

    // Blend pixmap cleanly onto clone of base image
    let mut out = base.clone();
    for y in 0..h {
        for x in 0..w {
            if let Some(pixel) = pixmap.pixel(x, y) {
                let a = pixel.alpha() as u32;
                if a > 0 {
                    let r = pixel.red() as u32;
                    let g = pixel.green() as u32;
                    let b = pixel.blue() as u32;
                    let dst = out.get_pixel_mut(x, y);
                    let inv_a = 255 - a;
                    dst[0] = ((r * 255 + dst[0] as u32 * inv_a) / 255).min(255) as u8;
                    dst[1] = ((g * 255 + dst[1] as u32 * inv_a) / 255).min(255) as u8;
                    dst[2] = ((b * 255 + dst[2] as u32 * inv_a) / 255).min(255) as u8;
                    dst[3] = (a + (dst[3] as u32 * inv_a) / 255).min(255) as u8;
                }
            }
        }
    }
    out
}

fn draw_text_skia(
    pixmap: &mut tiny_skia::Pixmap,
    font: &little_clip_render::fontdue::Font,
    text: &str,
    mut start_x: f32,
    base_y: f32,
    font_size: f32,
    color: [u8; 4],
    center_align: bool,
) {
    if center_align {
        let total_w = measure_text_skia(font, text, font_size);
        start_x -= total_w * 0.5;
    }
    let mut cur_x = start_x;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, font_size);
        let gx = cur_x + metrics.xmin as f32;
        let gy = base_y + font_size - metrics.ymin as f32 - metrics.height as f32;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let alpha = bitmap[row * metrics.width + col];
                if alpha > 0 {
                    let px = (gx + col as f32).round() as i32;
                    let py = (gy + row as f32).round() as i32;
                    if px >= 0
                        && px < pixmap.width() as i32
                        && py >= 0
                        && py < pixmap.height() as i32
                    {
                        let a = (alpha as u32 * color[3] as u32) / 255;
                        let r = (color[0] as u32 * a) / 255;
                        let g = (color[1] as u32 * a) / 255;
                        let b = (color[2] as u32 * a) / 255;
                        let idx = py as usize * pixmap.width() as usize + px as usize;
                        let dst_pix = &mut pixmap.pixels_mut()[idx];
                        let cur_a = dst_pix.alpha() as u32;
                        let inv_a = 255 - a;
                        let out_r = (r + (dst_pix.red() as u32 * inv_a) / 255).min(255) as u8;
                        let out_g = (g + (dst_pix.green() as u32 * inv_a) / 255).min(255) as u8;
                        let out_b = (b + (dst_pix.blue() as u32 * inv_a) / 255).min(255) as u8;
                        let out_a = (a + (cur_a * inv_a) / 255).min(255) as u8;
                        let c = tiny_skia::Color::from_rgba8(out_r, out_g, out_b, out_a)
                            .to_color_u8()
                            .premultiply();
                        *dst_pix = c;
                    }
                }
            }
        }
        cur_x += metrics.advance_width;
    }
}

fn measure_text_skia(font: &little_clip_render::fontdue::Font, text: &str, font_size: f32) -> f32 {
    let mut total_w = 0.0;
    for ch in text.chars() {
        let metrics = font.metrics(ch, font_size);
        total_w += metrics.advance_width;
    }
    total_w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markup_defaults() {
        let state = MarkupState::default();
        assert_eq!(state.active_tool, MarkupToolType::Arrow);
        assert_eq!(state.active_color, Color32::from_rgb(255, 42, 42));
        assert_eq!(state.active_stroke_width, 4.0);
        assert_eq!(state.active_font_size, 20.0);
        assert_eq!(state.next_badge_number, 1);
        assert!(state.items.is_empty());
    }

    #[test]
    fn test_markup_shape_operations() {
        let mut shape = MarkupShape::Arrow {
            start: Pos2::new(0.1, 0.1),
            end: Pos2::new(0.5, 0.5),
            color: Color32::RED,
            width: 4.0,
        };

        assert_eq!(shape.type_name(), "Arrow");
        assert_eq!(shape.type_icon(), Icon::ArrowRight);
        assert_eq!(shape.color(), Color32::RED);
        assert_eq!(shape.width(), Some(4.0));

        shape.set_color(Color32::GREEN);
        assert_eq!(shape.color(), Color32::GREEN);

        shape.set_width(8.0);
        assert_eq!(shape.width(), Some(8.0));

        shape.translate(Vec2::new(0.1, 0.2));
        match shape {
            MarkupShape::Arrow { start, end, .. } => {
                assert!((start.x - 0.2).abs() < 1e-5);
                assert!((start.y - 0.3).abs() < 1e-5);
                assert!((end.x - 0.6).abs() < 1e-5);
                assert!((end.y - 0.7).abs() < 1e-5);
            }
            _ => panic!("Expected arrow"),
        }
    }

    #[test]
    fn test_markup_badge_auto_increment() {
        let mut state = MarkupState::default();
        assert_eq!(state.next_badge_number, 1);
        assert_eq!(state.next_available_badge_number(), 1);

        // Create badges 1, 2, 3, 4, 5
        for _ in 1..=5 {
            let num = state.next_available_badge_number();
            state.items.push(MarkupShape::StepBadge {
                center: Pos2::new(0.5, 0.5),
                radius: 28.0,
                number: num,
                color: state.active_color,
            });
            state.sync_next_badge_number();
        }
        assert_eq!(state.items.len(), 5);
        assert_eq!(state.next_badge_number, 6);

        // Delete 2 and 5 (leaving 1, 3, 4)
        state.items.retain(|item| {
            if let MarkupShape::StepBadge { number, .. } = item {
                *number != 2 && *number != 5
            } else {
                true
            }
        });
        state.sync_next_badge_number();
        assert_eq!(state.items.len(), 3);

        // Next badge created must fill hole 2
        assert_eq!(state.next_available_badge_number(), 2);
        assert_eq!(state.next_badge_number, 2);
        let num2 = state.next_available_badge_number();
        state.items.push(MarkupShape::StepBadge {
            center: Pos2::new(0.5, 0.5),
            radius: 28.0,
            number: num2,
            color: state.active_color,
        });
        state.sync_next_badge_number();
        assert_eq!(num2, 2);

        // Next badge created must fill hole 5
        assert_eq!(state.next_available_badge_number(), 5);
        assert_eq!(state.next_badge_number, 5);
        let num5 = state.next_available_badge_number();
        state.items.push(MarkupShape::StepBadge {
            center: Pos2::new(0.5, 0.5),
            radius: 28.0,
            number: num5,
            color: state.active_color,
        });
        state.sync_next_badge_number();
        assert_eq!(num5, 5);

        // Next badge must move onto 6
        assert_eq!(state.next_available_badge_number(), 6);
        assert_eq!(state.next_badge_number, 6);
        let num6 = state.next_available_badge_number();
        state.items.push(MarkupShape::StepBadge {
            center: Pos2::new(0.5, 0.5),
            radius: 28.0,
            number: num6,
            color: state.active_color,
        });
        state.sync_next_badge_number();
        assert_eq!(num6, 6);

        // Next badge is 7
        assert_eq!(state.next_available_badge_number(), 7);
    }

    #[test]
    fn test_markup_rasterize_to_image() {
        let base = RgbaImage::new(100, 100);
        let items = vec![
            MarkupShape::Arrow {
                start: Pos2::new(0.1, 0.1),
                end: Pos2::new(0.8, 0.8),
                color: Color32::RED,
                width: 4.0,
            },
            MarkupShape::Rectangle {
                rect: Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.6, 0.6)),
                color: Color32::YELLOW,
                width: 2.0,
                filled: true,
                rounding: 0.0,
            },
            MarkupShape::Text {
                pos: Pos2::new(0.1, 0.5),
                text: "Hello".to_string(),
                font_size: 16.0,
            },
        ];

        let out = rasterize_markup(&base, &items);
        assert_eq!(out.width(), 100);
        assert_eq!(out.height(), 100);
    }

    #[test]
    fn test_infer_font_size_from_box_height() {
        // A box height of 31px should infer ~20px font
        let f20 = infer_font_size_from_box_height(31.0);
        assert!((f20 - 20.0).abs() < 1.0);

        // A box height of 62px should infer ~40px font
        let f40 = infer_font_size_from_box_height(62.0);
        assert!((f40 - 40.0).abs() < 1.0);

        // Clamping ensures reasonable bounds
        assert_eq!(infer_font_size_from_box_height(5.0), 10.0);
        assert_eq!(infer_font_size_from_box_height(1000.0), 300.0);
    }

    #[test]
    fn test_markup_text_bounding_rect_expands_with_typing() {
        let short_text = MarkupShape::Text {
            pos: Pos2::new(0.1, 0.1),
            text: "Hi".to_string(),
            font_size: 20.0,
        };
        let long_text = MarkupShape::Text {
            pos: Pos2::new(0.1, 0.1),
            text: "This is a significantly longer annotation string".to_string(),
            font_size: 20.0,
        };

        let rect_short = short_text.bounding_rect_norm(1000.0, 1000.0);
        let rect_long = long_text.bounding_rect_norm(1000.0, 1000.0);

        // Heights should be identical because font_size is unchanged
        assert!((rect_short.height() - rect_long.height()).abs() < 1e-4);
        // Width must grow as user types more characters
        assert!(rect_long.width() > rect_short.width());
    }

    #[test]
    fn test_step_badge_font_size() {
        // Single digit gets 1.25x radius
        let font_1 = step_badge_font_size(24.0, 5);
        assert!((font_1 - 30.0).abs() < 1e-3);

        // Double digit gets 0.95x radius to fit cleanly inside circle
        let font_2 = step_badge_font_size(24.0, 42);
        assert!((font_2 - 22.8).abs() < 1e-3);

        // Triple digit gets 0.70x radius
        let font_3 = step_badge_font_size(24.0, 100);
        assert!((font_3 - 16.8).abs() < 1e-3);

        // Clamping ensures readable minimum
        assert_eq!(step_badge_font_size(4.0, 1), 8.0);
    }

    #[test]
    fn test_step_badge_scale_high_res() {
        let badge = MarkupShape::StepBadge {
            center: Pos2::new(0.5, 0.5),
            radius: 80.0,
            number: 1,
            color: Color32::RED,
        };

        // Bounding rect on 4K image (3840x2160)
        let rect_4k = badge.bounding_rect_norm(3840.0, 2160.0);
        let screen_w = rect_4k.width() * 3840.0;
        let screen_h = rect_4k.height() * 2160.0;
        assert!((screen_w - 160.0).abs() < 1e-3);
        assert!((screen_h - 160.0).abs() < 1e-3);

        let font_size = step_badge_font_size(80.0, 1);
        assert_eq!(font_size, 100.0);
    }
}
