//! Unified 5-Zone StudioShell layout and presentation manager.
//!
//! Provides a standardized container for all preview and studio views,
//! enforcing visual consistency for headers, mode toolbars, multi-panel layouts,
//! and bottom status footers.

use super::escape_hatch::EscapeHatch;
use super::panel::{
    PANEL_MAX_HEIGHT, PANEL_MAX_WIDTH, PANEL_MIN_HEIGHT, PANEL_MIN_WIDTH, PanelState,
};
use crate::hex_to_c32;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::{
    CONTROL_CORNER_RADIUS, CONTROL_HEIGHT, CONTROL_ICON_SIZE, FOOTER_INSET_Y, PANEL_INSET,
    SPACE_MD, SPACE_SM, SPACE_XS, StatusToast, contrast_text_c32, equal_button_width,
};
use eframe::egui::{self, Color32, Rect, Rounding, Stroke, Vec2};
use little_clip_core::palette::AppPalette;

use crate::icons::Icon;

const MIN_CANVAS_WIDTH: f32 = 320.0;
const MIN_RAIL_WIDTH: f32 = 160.0;

/// Badge metadata item rendered in the header.
pub struct HeaderBadge {
    pub text: String,
    pub accent: Option<Color32>,
}

impl HeaderBadge {
    pub fn new(text: impl Into<String>, accent: Option<Color32>) -> Self {
        Self {
            text: text.into(),
            accent,
        }
    }
}

/// Exact rectangles for a studio tool's optional work areas.
///
/// Unlike an `egui` horizontal layout, these rectangles are reserved before
/// any child UI is built. A long label, button, palette, or matrix therefore
/// cannot silently widen a sidebar and steal space from the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioPanelRects {
    pub left: Option<Rect>,
    pub center: Rect,
    pub right: Option<Rect>,
    pub bottom: Option<Rect>,
}

/// Shared image/audio studio workspace geometry.
///
/// The preferred rails frame a flexible canvas. When those preferences no
/// longer fit, rails shrink; on compact hosts they move below the canvas.
/// Every region is optional except the center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioPanelLayout {
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
    pub gap: f32,
}

impl Default for StudioPanelLayout {
    fn default() -> Self {
        Self {
            left: false,
            right: false,
            bottom: false,
            left_width: super::panel::PANEL_LEFT_DEFAULT_WIDTH,
            right_width: super::panel::PANEL_RIGHT_DEFAULT_WIDTH,
            bottom_height: super::panel::PANEL_BOTTOM_DEFAULT_HEIGHT,
            gap: 8.0,
        }
    }
}

impl StudioPanelLayout {
    pub fn center_only() -> Self {
        Self::default()
    }

    /// Apply the user's preferred internal split sizes without changing which
    /// regions this workspace owns. The solver still shrinks them when needed.
    pub fn with_panel_sizes(mut self, state: &PanelState) -> Self {
        self.left_width = state.left_width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
        self.right_width = state.right_width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
        self.bottom_height = state
            .bottom_height
            .clamp(PANEL_MIN_HEIGHT, PANEL_MAX_HEIGHT);
        self
    }

    pub fn with_left(mut self) -> Self {
        self.left = true;
        self
    }

    pub fn with_right(mut self) -> Self {
        self.right = true;
        self
    }

    pub fn with_bottom(mut self) -> Self {
        self.bottom = true;
        self
    }

    /// Reserve the complete workspace and return exact, non-overlapping zones.
    pub fn allocate(self, ui: &mut egui::Ui) -> StudioPanelRects {
        let available = ui.available_rect_before_wrap();
        let rects = self.rects(available);
        ui.allocate_rect(available, egui::Sense::hover());
        rects
    }

    /// Draw and handle split grips in the gutters of an allocated workspace.
    /// Compact rails sit below the canvas, so width grips are only shown when
    /// the corresponding rail actually borders the canvas horizontally.
    pub fn show_resize_handles(
        ui: &mut egui::Ui,
        rects: &StudioPanelRects,
        state: &mut PanelState,
        palette: &AppPalette,
        id: impl std::hash::Hash,
    ) {
        let id = ui.id().with(id);
        if let Some(left) = rects.left
            && left.right() < rects.center.left()
            && left.bottom() > rects.center.top()
        {
            let gutter = Rect::from_center_size(
                egui::pos2((left.right() + rects.center.left()) * 0.5, left.center().y),
                Vec2::new(12.0, left.height()),
            );
            let response = Self::resize_handle(ui, gutter, id.with("left"), true, palette)
                .on_hover_text("Drag or use arrow keys to resize tools; double-click to reset");
            if response.double_clicked() {
                state.left_width = super::panel::PANEL_LEFT_DEFAULT_WIDTH;
            } else {
                state.left_width = (state.left_width + Self::resize_delta(ui, &response, true))
                    .clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
            }
        }
        if let Some(right) = rects.right
            && right.left() > rects.center.right()
            && right.bottom() > rects.center.top()
        {
            let gutter = Rect::from_center_size(
                egui::pos2(
                    (right.left() + rects.center.right()) * 0.5,
                    right.center().y,
                ),
                Vec2::new(12.0, right.height()),
            );
            let response = Self::resize_handle(ui, gutter, id.with("right"), true, palette)
                .on_hover_text("Drag or use arrow keys to resize inspector; double-click to reset");
            if response.double_clicked() {
                state.right_width = super::panel::PANEL_RIGHT_DEFAULT_WIDTH;
            } else {
                state.right_width = (state.right_width - Self::resize_delta(ui, &response, true))
                    .clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
            }
        }
        let content_bottom = rects
            .left
            .into_iter()
            .chain(rects.right)
            .fold(rects.center.bottom(), |edge, rail| edge.max(rail.bottom()));
        if let Some(bottom) = rects.bottom
            && bottom.top() > content_bottom
        {
            let gutter = Rect::from_center_size(
                egui::pos2(bottom.center().x, (bottom.top() + content_bottom) * 0.5),
                Vec2::new(bottom.width(), 12.0),
            );
            let response = Self::resize_handle(ui, gutter, id.with("bottom"), false, palette)
                .on_hover_text("Drag or use arrow keys to resize controls; double-click to reset");
            if response.double_clicked() {
                state.bottom_height = super::panel::PANEL_BOTTOM_DEFAULT_HEIGHT;
            } else {
                state.bottom_height = (state.bottom_height
                    - Self::resize_delta(ui, &response, false))
                .clamp(PANEL_MIN_HEIGHT, PANEL_MAX_HEIGHT);
            }
        }
    }

    fn resize_delta(ui: &egui::Ui, response: &egui::Response, horizontal: bool) -> f32 {
        if response.dragged() {
            return if horizontal {
                response.drag_delta().x
            } else {
                response.drag_delta().y
            };
        }
        if !response.has_focus() {
            return 0.0;
        }
        ui.input_mut(|input| {
            let (negative, positive) = if horizontal {
                (egui::Key::ArrowLeft, egui::Key::ArrowRight)
            } else {
                (egui::Key::ArrowUp, egui::Key::ArrowDown)
            };
            if input.consume_key(egui::Modifiers::NONE, negative) {
                -8.0
            } else if input.consume_key(egui::Modifiers::NONE, positive) {
                8.0
            } else {
                0.0
            }
        })
    }

    fn resize_handle(
        ui: &mut egui::Ui,
        rect: Rect,
        id: egui::Id,
        horizontal: bool,
        palette: &AppPalette,
    ) -> egui::Response {
        let response = ui
            .interact(rect, id, egui::Sense::click_and_drag())
            .on_hover_cursor(if horizontal {
                egui::CursorIcon::ResizeHorizontal
            } else {
                egui::CursorIcon::ResizeVertical
            });
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        let grip = Rect::from_center_size(
            rect.center(),
            if horizontal {
                Vec2::new(3.0, 34.0)
            } else {
                Vec2::new(34.0, 3.0)
            },
        );
        let color = if response.hovered() || response.dragged() {
            &palette.harmonies.base
        } else {
            &palette.colors.text_muted
        };
        ui.painter()
            .rect_filled(grip, Rounding::same(2.0), hex_to_c32(color));
        response
    }

    fn rects(self, available: Rect) -> StudioPanelRects {
        let gap = self.gap.max(0.0);
        let bottom_height = if self.bottom {
            self.bottom_height.min((available.height() * 0.3).max(0.0))
        } else {
            0.0
        };
        let bottom_gap = if bottom_height > 0.0 { gap } else { 0.0 };
        let main_bottom = (available.bottom() - bottom_height - bottom_gap).max(available.top());
        let main = Rect::from_min_max(available.min, egui::pos2(available.right(), main_bottom));
        let bottom = self.bottom.then(|| {
            Rect::from_min_max(
                egui::pos2(
                    available.left(),
                    (main_bottom + bottom_gap).min(available.bottom()),
                ),
                available.max,
            )
        });

        let count = u8::from(self.left) + u8::from(self.right);
        if count == 0 {
            return StudioPanelRects {
                left: None,
                center: main,
                right: None,
                bottom,
            };
        }

        let horizontal_gaps = gap * f32::from(count);
        let rail_space = main.width() - horizontal_gaps - MIN_CANVAS_WIDTH;
        if rail_space >= f32::from(count) * MIN_RAIL_WIDTH {
            return self.horizontal_rects(main, bottom, gap, rail_space);
        }

        self.compact_rects(main, bottom, gap, count)
    }

    fn horizontal_rects(
        self,
        main: Rect,
        bottom: Option<Rect>,
        gap: f32,
        rail_space: f32,
    ) -> StudioPanelRects {
        let count = u8::from(self.left) + u8::from(self.right);
        let left_preferred = if self.left {
            self.left_width.max(MIN_RAIL_WIDTH)
        } else {
            0.0
        };
        let right_preferred = if self.right {
            self.right_width.max(MIN_RAIL_WIDTH)
        } else {
            0.0
        };
        let extra = (left_preferred + right_preferred - f32::from(count) * MIN_RAIL_WIDTH).max(0.0);
        let fraction = if extra > 0.0 {
            ((rail_space - f32::from(count) * MIN_RAIL_WIDTH) / extra).min(1.0)
        } else {
            0.0
        };
        let left_width = if self.left {
            MIN_RAIL_WIDTH + (left_preferred - MIN_RAIL_WIDTH) * fraction
        } else {
            0.0
        };
        let right_width = if self.right {
            MIN_RAIL_WIDTH + (right_preferred - MIN_RAIL_WIDTH) * fraction
        } else {
            0.0
        };
        let left = self
            .left
            .then(|| Rect::from_min_size(main.min, Vec2::new(left_width, main.height())));
        let right = self.right.then(|| {
            Rect::from_min_size(
                egui::pos2(main.right() - right_width, main.top()),
                Vec2::new(right_width, main.height()),
            )
        });
        let center = Rect::from_min_max(
            egui::pos2(
                main.left() + left_width + if self.left { gap } else { 0.0 },
                main.top(),
            ),
            egui::pos2(
                main.right() - right_width - if self.right { gap } else { 0.0 },
                main.bottom(),
            ),
        );
        StudioPanelRects {
            left,
            center,
            right,
            bottom,
        }
    }

    fn compact_rects(
        self,
        main: Rect,
        bottom: Option<Rect>,
        gap: f32,
        count: u8,
    ) -> StudioPanelRects {
        // Compact hosts retain a full-width canvas and put scrollable tools in
        // a bounded row below it. This is also safe for unexpected narrow hosts.
        let rail_height = (main.height() * 0.3).min(180.0);
        let rail_gap = gap.min((main.height() - rail_height).max(0.0));
        let center_bottom = main.bottom() - rail_height - rail_gap;
        let center = Rect::from_min_max(main.min, egui::pos2(main.right(), center_bottom));
        let rail_top = center_bottom + rail_gap;
        let rail_width =
            ((main.width() - if count == 2 { gap } else { 0.0 }) / f32::from(count)).max(0.0);
        let left = self.left.then(|| {
            Rect::from_min_size(
                egui::pos2(main.left(), rail_top),
                Vec2::new(rail_width, rail_height),
            )
        });
        let right = self.right.then(|| {
            Rect::from_min_size(
                egui::pos2(main.right() - rail_width, rail_top),
                Vec2::new(rail_width, rail_height),
            )
        });
        StudioPanelRects {
            left,
            center,
            right,
            bottom,
        }
    }

    /// Render a canonical framed, vertically scrollable side panel.
    pub fn show_side(
        ui: &mut egui::Ui,
        rect: Rect,
        id: impl std::hash::Hash,
        palette: &AppPalette,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.set_clip_rect(rect);
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.surface_recessed))
                .stroke(Stroke::new(
                    1.0_f32,
                    hex_to_c32(&palette.colors.surface_panel),
                ))
                .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(SPACE_SM))
                .show(ui, |ui| {
                    ui.set_width((rect.width() - SPACE_SM * 2.0).max(1.0));
                    ui.set_max_width((rect.width() - SPACE_SM * 2.0).max(1.0));
                    ui.set_height((rect.height() - SPACE_SM * 2.0).max(1.0));
                    egui::ScrollArea::vertical()
                        .id_source(id)
                        .auto_shrink([false, false])
                        .show(ui, add_contents);
                });
        });
    }

    /// Render content in an exact clipped region, regardless of its panel role.
    pub fn show_region(ui: &mut egui::Ui, rect: Rect, add_contents: impl FnOnce(&mut egui::Ui)) {
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.set_clip_rect(rect);
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            add_contents(ui);
        });
    }

    /// Render the optional bottom drawer using the same panel treatment.
    pub fn show_bottom(
        ui: &mut egui::Ui,
        rect: Rect,
        palette: &AppPalette,
        add_contents: impl FnOnce(&mut egui::Ui),
    ) {
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.set_clip_rect(rect);
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.surface_recessed))
                .stroke(Stroke::new(
                    1.0_f32,
                    hex_to_c32(&palette.colors.surface_panel),
                ))
                .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(SPACE_SM))
                .show(ui, add_contents);
        });
    }
}

/// The standard StudioShell component builder.
pub struct StudioShell<'a> {
    pub title: String,
    compact_title: Option<String>,
    header_readout: Option<(String, f32)>,
    pub icon: Option<Icon>,
    pub palette: &'a AppPalette,
    pub close: &'a mut bool,
    pub badges: Vec<HeaderBadge>,
    pub escape_hatches: Vec<EscapeHatch>,
    pub footer_hints: Option<&'a str>,
    pub footer_hint_icon: Option<crate::icons::Icon>,
    pub footer_readout: Option<String>,
}

impl<'a> StudioShell<'a> {
    pub fn new(
        title: impl Into<String>,
        icon: impl Into<Option<Icon>>,
        palette: &'a AppPalette,
        close: &'a mut bool,
    ) -> Self {
        Self {
            title: title.into(),
            compact_title: None,
            header_readout: None,
            icon: icon.into(),
            palette,
            close,
            badges: Vec::new(),
            escape_hatches: Vec::new(),
            footer_hints: None,
            footer_hint_icon: None,
            footer_readout: None,
        }
    }

    pub fn with_badge(mut self, text: impl Into<String>, accent: Option<Color32>) -> Self {
        self.badges.push(HeaderBadge::new(text, accent));
        self
    }

    /// Provide a shorter title to show when the full title cannot fit beside header actions.
    pub fn with_compact_title(mut self, title: impl Into<String>) -> Self {
        self.compact_title = Some(title.into());
        self
    }

    /// Reserve a fixed-width readout at the horizontal center of the header.
    pub fn with_header_readout(mut self, text: impl Into<String>, width: f32) -> Self {
        self.header_readout = Some((text.into(), width));
        self
    }

    pub fn with_escape_hatch(mut self, hatch: EscapeHatch) -> Self {
        self.escape_hatches.push(hatch);
        self
    }

    pub fn with_footer_hint_icon(mut self, icon: crate::icons::Icon) -> Self {
        self.footer_hint_icon = Some(icon);
        self
    }

    pub fn with_footer_hints(mut self, hints: &'a str) -> Self {
        self.footer_hints = Some(hints);
        self
    }

    pub fn with_footer_readout(mut self, readout: impl Into<String>) -> Self {
        self.footer_readout = Some(readout.into());
        self
    }

    pub fn is_closed(&self) -> bool {
        *self.close
    }

    /// Renders Zone 1 (Top Header). Returns the index of any clicked escape hatch.
    pub fn render_header(
        &mut self,
        ui: &mut egui::Ui,
        right_actions: impl FnOnce(&mut egui::Ui),
    ) -> Option<usize> {
        self.render_header_with_title_actions(ui, |_| {}, right_actions)
    }

    /// Renders a compact control immediately after the title, before badges.
    pub fn render_header_with_title_actions(
        &mut self,
        ui: &mut egui::Ui,
        title_actions: impl FnOnce(&mut egui::Ui),
        right_actions: impl FnOnce(&mut egui::Ui),
    ) -> Option<usize> {
        let palette = self.palette;
        let colors = WorkbenchColors::from_palette(palette);
        let header_accent = match self.icon {
            Some(Icon::Image | Icon::Diff) => hex_to_c32(&palette.right_hand_border),
            Some(Icon::Midi | Icon::Model3d | Icon::BinaryDiff) => {
                hex_to_c32(&palette.harmonies.triadic_1)
            }
            Some(Icon::Text | Icon::Book | Icon::Code | Icon::Pdf) => {
                hex_to_c32(&palette.harmonies.tetradic_2)
            }
            _ => colors.accent,
        };
        let mut clicked_hatch = None;
        let hatch_width = equal_button_width(
            ui,
            &self
                .escape_hatches
                .iter()
                .map(|hatch| hatch.label.as_str())
                .collect::<Vec<_>>(),
            true,
        );

        egui::Frame::none()
            .fill(colors.panel)
            .inner_margin(egui::Margin::symmetric(PANEL_INSET, SPACE_SM))
            .stroke(Stroke::new(1.0_f32, colors.border))
            .show(ui, |ui| {
                let (row_rect, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), CONTROL_HEIGHT),
                    egui::Sense::hover(),
                );
                let readout_rect = self.header_readout.as_ref().map(|(_, width)| {
                    Rect::from_center_size(row_rect.center(), Vec2::new(*width, row_rect.height()))
                });
                if let (Some((text, _)), Some(rect)) = (&self.header_readout, readout_rect) {
                    ui.painter().with_clip_rect(rect).text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::monospace(14.0),
                        colors.accent,
                    );
                }
                // Allocate the functional controls first. Their width determines how much
                // space the title can use; a long filename must never draw over them.
                let mut action_ui =
                    ui.child_ui(row_rect, egui::Layout::right_to_left(egui::Align::Center));
                action_ui.set_clip_rect(row_rect);
                {
                    let ui = &mut action_ui;
                    // Place the actions at the far right of the title row.
                    for (idx, hatch) in self.escape_hatches.iter().enumerate().rev() {
                        let (bg, fg) = if hatch.is_primary {
                            (colors.accent, colors.text_primary)
                        } else {
                            (colors.panel, colors.text_secondary)
                        };
                        let contrast_fg = contrast_text_c32(bg, Some(fg), palette);
                        let text = egui::RichText::new(&hatch.label)
                            .strong()
                            .color(contrast_fg);
                        let btn = if let Some(icon) = hatch.icon {
                            egui::Button::image_and_text(
                                icon.image_tinted(CONTROL_ICON_SIZE, contrast_fg),
                                text,
                            )
                        } else {
                            egui::Button::new(text)
                        }
                        .fill(bg)
                        .stroke(Stroke::new(1.0_f32, colors.recessed))
                        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
                        .min_size(Vec2::new(hatch_width, CONTROL_HEIGHT));

                        if ui.add(btn).on_hover_text(&hatch.tooltip).clicked() {
                            clicked_hatch = Some(idx);
                        }
                        ui.add_space(SPACE_XS);
                    }

                    right_actions(ui);

                    let close_bg = colors.panel;
                    let close_fg = contrast_text_c32(close_bg, Some(colors.danger), palette);
                    let close_btn = egui::Button::image_and_text(
                        Icon::Close.image_tinted(CONTROL_ICON_SIZE, close_fg),
                        egui::RichText::new("Close (Esc)").strong().color(close_fg),
                    )
                    .fill(close_bg)
                    .stroke(Stroke::new(1.0_f32, colors.recessed))
                    .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
                    .min_size(Vec2::new(0.0, CONTROL_HEIGHT));

                    if ui.add(close_btn).clicked() {
                        *self.close = true;
                    }
                }

                let title_right = (action_ui.min_rect().left() - SPACE_SM).max(row_rect.left());
                let controls_left = readout_rect
                    .map(|rect| rect.right() + SPACE_SM)
                    .unwrap_or(row_rect.left())
                    .min(title_right);
                let controls_rect = Rect::from_min_max(
                    egui::pos2(controls_left, row_rect.top()),
                    egui::pos2(title_right, row_rect.bottom()),
                );
                let mut title_action_ui = ui.child_ui(
                    controls_rect,
                    egui::Layout::right_to_left(egui::Align::Center),
                );
                title_action_ui.set_clip_rect(controls_rect);
                title_actions(&mut title_action_ui);
                let badges_right = (title_action_ui.min_rect().left() - SPACE_SM).min(title_right);
                let badge_font = egui::TextStyle::Body.resolve(ui.style());
                let badge_widths: Vec<f32> = self
                    .badges
                    .iter()
                    .map(|badge| {
                        ui.fonts(|fonts| {
                            fonts
                                .layout_no_wrap(
                                    badge.text.clone(),
                                    badge_font.clone(),
                                    Color32::WHITE,
                                )
                                .size()
                                .x
                        }) + 12.0
                    })
                    .collect();
                let badges_width = badge_widths.iter().sum::<f32>()
                    + 4.0 * self.badges.len().saturating_sub(1) as f32;
                let badges_left = badges_right - badges_width;
                let badges_min_left = readout_rect
                    .map(|rect| rect.right() + SPACE_SM)
                    .unwrap_or(row_rect.left() + 80.0);
                let show_badges = badges_left >= badges_min_left;
                if show_badges {
                    let mut badge_x = badges_left;
                    for (badge, width) in self.badges.iter().zip(badge_widths) {
                        let badge_rect = Rect::from_min_size(
                            egui::pos2(badge_x, row_rect.top()),
                            Vec2::new(width, row_rect.height()),
                        );
                        let mut badge_ui = ui
                            .child_ui(badge_rect, egui::Layout::left_to_right(egui::Align::Center));
                        badge_ui.set_clip_rect(badge_rect);
                        crate::ui_ext::badge_pill(
                            &mut badge_ui,
                            &badge.text,
                            badge.accent,
                            palette,
                        );
                        badge_x += width + 4.0;
                    }
                }
                let name_right = (if show_badges {
                    badges_left
                } else {
                    badges_right
                } - 8.0)
                    .max(row_rect.left());
                let name_right = readout_rect
                    .map(|rect| name_right.min(rect.left() - SPACE_SM))
                    .unwrap_or(name_right)
                    .max(row_rect.left());
                let name_rect =
                    Rect::from_min_max(row_rect.min, egui::pos2(name_right, row_rect.bottom()));
                let mut name_ui =
                    ui.child_ui(name_rect, egui::Layout::left_to_right(egui::Align::Center));
                name_ui.set_clip_rect(name_rect);
                if let Some(icon) = self.icon {
                    let (icon_rect, _) = name_ui
                        .allocate_exact_size(Vec2::splat(CONTROL_HEIGHT), egui::Sense::hover());
                    name_ui.painter().rect_filled(
                        icon_rect,
                        Rounding::same(CONTROL_CORNER_RADIUS),
                        header_accent,
                    );
                    icon.image_tinted(
                        CONTROL_ICON_SIZE,
                        contrast_text_c32(header_accent, None, palette),
                    )
                    .paint_at(
                        &name_ui,
                        Rect::from_center_size(icon_rect.center(), Vec2::splat(CONTROL_ICON_SIZE)),
                    );
                }
                let title_font = egui::FontId::proportional(22.0);
                let visible_title = self
                    .compact_title
                    .as_deref()
                    .filter(|_| {
                        ui.fonts(|fonts| {
                            let full_width = fonts
                                .layout_no_wrap(
                                    self.title.clone(),
                                    title_font.clone(),
                                    colors.text_primary,
                                )
                                .size()
                                .x;
                            full_width > name_ui.available_width()
                        })
                    })
                    .unwrap_or(&self.title);
                let title_tooltip = if show_badges {
                    self.title.clone()
                } else {
                    format!(
                        "{}\n{}",
                        self.title,
                        self.badges
                            .iter()
                            .map(|badge| badge.text.as_str())
                            .collect::<Vec<_>>()
                            .join(" · ")
                    )
                };
                name_ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(visible_title)
                                .strong()
                                .size(22.0)
                                .color(colors.text_primary),
                        )
                        .truncate(true),
                    )
                    .on_hover_text(title_tooltip);
            });

        clicked_hatch
    }

    /// Renders Zone 4 (Status & Shortcut Help Footer).
    pub fn render_footer(
        &self,
        ui: &mut egui::Ui,
        toast: &Option<(crate::ui_ext::StatusMessage, f64)>,
        now: f64,
    ) {
        self.render_footer_with_center(ui, toast, now, 0.0, |_| {});
    }

    pub fn render_footer_with_center(
        &self,
        ui: &mut egui::Ui,
        toast: &Option<(crate::ui_ext::StatusMessage, f64)>,
        now: f64,
        center_width: f32,
        render_center: impl FnOnce(&mut egui::Ui),
    ) {
        let palette = self.palette;
        let colors = WorkbenchColors::from_palette(palette);

        egui::Frame::none()
            .fill(colors.recessed)
            .inner_margin(egui::Margin::symmetric(PANEL_INSET, FOOTER_INSET_Y))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let render_status = |ui: &mut egui::Ui| {
                        if let Some(hints) = self.footer_hints {
                            if let Some(icon) = self.footer_hint_icon {
                                icon.ui(ui, crate::ui_ext::CONTROL_ICON_SIZE, colors.text_muted);
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(hints).color(colors.text_muted),
                                )
                                .wrap(center_width == 0.0)
                                .truncate(center_width > 0.0),
                            )
                            .on_hover_text(hints);
                        }

                        if let Some(ref readout) = self.footer_readout {
                            ui.add_space(SPACE_MD);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(readout)
                                        .strong()
                                        .monospace()
                                        .color(colors.accent),
                                )
                                .wrap(true),
                            );
                        }
                    };
                    if center_width > 0.0 {
                        let side_width = ((ui.available_width()
                            - center_width
                            - 2.0 * ui.spacing().item_spacing.x)
                            / 2.0)
                            .max(0.0);
                        ui.allocate_ui_with_layout(
                            Vec2::new(side_width, 24.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(side_width);
                                render_status(ui);
                            },
                        );
                        ui.allocate_ui_with_layout(
                            Vec2::new(center_width, 24.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(center_width);
                                render_center(ui);
                            },
                        );
                    } else {
                        render_status(ui);
                    }

                    // Status toast
                    ui.allocate_ui_with_layout(
                        Vec2::new(
                            ui.available_width(),
                            ui.text_style_height(&egui::TextStyle::Body),
                        ),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| StatusToast::render_opt(toast, 4.5, ui, palette, now),
                    );
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_header_readout_keeps_its_position_and_truncates_titles() {
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        for width in [900.0, 1180.0, 1600.0] {
            for title in [
                "AUDIO STUDIO — short.wav",
                "AUDIO STUDIO — a very long filename that needs to be truncated before the centered timestamp.wav",
            ] {
                let ctx = egui::Context::default();
                crate::ui_ext::configure_workbench_style(&ctx);
                let readout = "01:01:01.25 / 02:02:02.50";
                let mut header_center = 0.0;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::new(width, 700.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            header_center = ui.available_rect_before_wrap().center().x;
                            StudioShell::new(title, Icon::Audio, &palette, &mut false)
                                .with_header_readout(readout, 242.0)
                                .with_badge("44.1 kHz", None)
                                .with_badge("WAV", None)
                                .with_escape_hatch(EscapeHatch::primary(
                                    "Audacity", "audacity", "Edit",
                                ))
                                .render_header_with_title_actions(
                                    ui,
                                    |ui| {
                                        ui.add_sized(
                                            [100.0, CONTROL_HEIGHT],
                                            egui::Label::new("Volume"),
                                        );
                                    },
                                    |_| {},
                                );
                        });
                    },
                );
                let texts: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| {
                        if let egui::epaint::Shape::Text(text) = &shape.shape {
                            Some((
                                text.galley.text(),
                                shape.clip_rect,
                                Rect::from_min_size(text.pos, text.galley.size()),
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                let (_, timer_clip, timer_text) =
                    texts.iter().find(|(text, _, _)| *text == readout).unwrap();
                assert!((timer_text.center().x - header_center).abs() < 1.0);
                assert_eq!(timer_clip.width(), 242.0);
                for (text, clip, rect) in &texts {
                    if *text == title {
                        assert!(clip.right() < timer_clip.left(), "filename overlaps timer");
                    } else if *text != readout {
                        let visible = rect.intersect(*clip);
                        assert!(
                            visible.right() <= timer_clip.left()
                                || visible.left() >= timer_clip.right(),
                            "header control overlaps timer: {text}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn footer_toasts_do_not_increase_the_bar_height() {
        for (width, center_width) in [(760.0, 0.0), (1200.0, 0.0), (760.0, 184.0), (1200.0, 184.0)]
        {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let mut close = false;
            let mut normal_height = 0.0;
            for message in [
                None,
                Some("Markup item deleted"),
                Some(
                    "Failed to save image: a long error message that should remain on one line instead of increasing the footer height",
                ),
                None,
            ] {
                for _ in 0..3 {
                    let mut height = 0.0;
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                egui::Pos2::ZERO,
                                Vec2::new(width, 600.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            let panel = egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
                                let shell = StudioShell::new("Image", None, &palette, &mut close)
                                    .with_footer_hints(
                                        "Drag: Draw shape | Del: Delete | Esc: Deselect",
                                    );
                                let toast = message.map(|message| (message.into(), 0.0));
                                shell.render_footer_with_center(
                                    ui,
                                    &toast,
                                    1.0,
                                    center_width,
                                    |ui| {
                                        ui.add_sized(
                                            Vec2::new(center_width, 24.0),
                                            egui::Label::new("100%"),
                                        );
                                    },
                                );
                            });
                            height = panel.response.rect.height();
                        },
                    );
                    if normal_height == 0.0 {
                        normal_height = height;
                    }
                    assert!(
                        (height - normal_height).abs() < 0.5,
                        "toast {message:?} changed the {width}-point footer from {normal_height} to {height}"
                    );
                }
            }
        }
    }

    #[test]
    fn footer_readout_wraps_without_clipping_at_compact_width() {
        let readout = "Selection: 0.50s - 1.50s (1.00s • Solo: Melody) • [Ctrl+C] Copy • [Space] Play • [Esc] Clear";
        for width in [760.0, 1200.0] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let mut close = false;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(width, 120.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let shell = StudioShell::new("MIDI", None, &palette, &mut close)
                            .with_footer_hints("[Esc] Close | [Space] Play")
                            .with_footer_readout(readout);
                        shell.render_footer(ui, &None, 0.0);
                    });
                },
            );
            let shape = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(shape) if shape.galley.job.text == readout => {
                        Some((shape, clipped.clip_rect))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("footer readout was not painted at {width} points"));
            let bounds = shape.0.galley.rect.translate(shape.0.pos.to_vec2());
            assert!(
                !shape.0.galley.elided,
                "footer readout was elided at {width} points"
            );
            assert!(
                bounds.left() >= shape.1.left() - 0.5 && bounds.right() <= shape.1.right() + 0.5,
                "footer readout exceeds {width}-point footer: {bounds:?} clipped to {:?}",
                shape.1
            );
        }
    }

    #[test]
    fn related_external_editor_actions_share_width() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let colors = WorkbenchColors::from_palette(&palette);
        let mut close = false;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut shell = StudioShell::new("Image", Icon::Image, &palette, &mut close)
                    .with_escape_hatch(EscapeHatch::new("Krita", "krita", "Edit"))
                    .with_escape_hatch(EscapeHatch::new("Inkscape", "inkscape", "Edit"))
                    .with_escape_hatch(EscapeHatch::new("Nomacs", "nomacs", "View"));
                shell.render_header(ui, |_| {});
            });
        });

        let hatch_widths: Vec<_> = ["Krita", "Inkscape", "Nomacs"]
            .iter()
            .map(|label| {
                let text_center = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == *label => {
                            Some(text.pos.x + text.galley.size().x * 0.5)
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing hatch label {label}"));
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Rect(rect)
                            if rect.fill == colors.panel
                                && (rect.rect.height() - crate::ui_ext::CONTROL_HEIGHT).abs()
                                    < 0.1
                                && rect.rect.left() <= text_center
                                && text_center <= rect.rect.right() =>
                        {
                            Some(rect.rect.width())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing button background for {label}"))
            })
            .collect();
        assert_eq!(hatch_widths.len(), 3);
        assert!(hatch_widths.iter().all(|width| *width == hatch_widths[0]));
    }

    #[test]
    fn long_image_header_keeps_actions_in_one_compact_row() {
        let long_filename = "a very long descriptive Unicode filename 日本語.webp";
        let full_title = format!("IMAGE STUDIO — {long_filename}");
        for width in [1177.0, 1766.0, 2500.0] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(width, 662.0),
            ));
            let mut close = false;
            let mut header_rect = None;
            let mut action_rects = Vec::new();
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let compact_title = if width == 1177.0 {
                        long_filename
                    } else {
                        "fixture.webp"
                    };
                    let mut shell =
                        StudioShell::new(&full_title, Icon::Image, &palette, &mut close)
                            .with_compact_title(compact_title)
                            .with_badge("6000×6000 (1:1)", None)
                            .with_badge("WEBP", None)
                            .with_badge("554.4 KB", None)
                            .with_escape_hatch(EscapeHatch::new("Krita", "krita", "Edit"))
                            .with_escape_hatch(EscapeHatch::new("Inkscape", "inkscape", "Edit"))
                            .with_escape_hatch(EscapeHatch::new("Nomacs", "nomacs", "View"));
                    let response = ui.scope(|ui| {
                        shell.render_header(ui, |ui| {
                            action_rects.push(
                                ui.add(crate::ui_ext::action_button(
                                    "Copy",
                                    Color32::LIGHT_BLUE,
                                    &palette,
                                ))
                                .rect,
                            );
                            action_rects.push(
                                ui.add(crate::ui_ext::action_button(
                                    "Save As...",
                                    Color32::GREEN,
                                    &palette,
                                ))
                                .rect,
                            );
                        });
                    });
                    header_rect = Some(response.response.rect);
                });
            });
            let rect = header_rect.unwrap();
            assert!(rect.height() <= 60.0, "header wrapped vertically: {rect:?}");
            assert!(
                rect.right() <= width,
                "header overflowed right edge: {rect:?}"
            );
            let visible_text = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == label => Some(
                            Rect::from_min_size(text.pos, text.galley.size())
                                .intersect(clipped.clip_rect),
                        ),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing header label: {label}"))
            };
            let title = if width == 1177.0 {
                long_filename
            } else {
                full_title.as_str()
            };
            let labels = [
                title,
                "6000×6000 (1:1)",
                "WEBP",
                "554.4 KB",
                "Close (Esc)",
                "Save As...",
                "Copy",
                "Krita",
                "Inkscape",
                "Nomacs",
            ];
            let visible: Vec<_> = labels.iter().map(|label| visible_text(label)).collect();
            for pair in visible.windows(2) {
                assert!(
                    pair[0].right() <= pair[1].left(),
                    "header labels overlap: {:?} and {:?}",
                    pair[0],
                    pair[1]
                );
            }
            assert!(visible.iter().all(|text| text.width() > 0.0));
            assert!(action_rects.iter().all(|action| action.top() == 16.0));
        }
    }

    fn run_layout(layout: StudioPanelLayout, size: Vec2) -> StudioPanelRects {
        let ctx = egui::Context::default();
        let mut result = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                result = Some(layout.allocate(ui));
            });
        });
        result.expect("layout should be allocated")
    }

    #[test]
    fn optional_panels_keep_canonical_dimensions() {
        let rects = run_layout(
            StudioPanelLayout::default().with_left().with_right(),
            Vec2::new(1_200.0, 700.0),
        );
        assert_eq!(rects.left.unwrap().width(), 240.0);
        assert_eq!(rects.right.unwrap().width(), 280.0);
        assert!(rects.bottom.is_none());
        assert!(rects.center.width() > 600.0);
    }

    #[test]
    fn preferred_split_sizes_apply_without_changing_the_outer_workspace() {
        let state = PanelState {
            left_width: 320.0,
            right_width: 210.0,
            bottom_height: 180.0,
            ..PanelState::default()
        };
        let layout = StudioPanelLayout::default()
            .with_left()
            .with_right()
            .with_bottom()
            .with_panel_sizes(&state);
        let rects = run_layout(layout, Vec2::new(1_200.0, 700.0));
        assert_eq!(rects.left.unwrap().width(), 320.0);
        assert_eq!(rects.right.unwrap().width(), 210.0);
        assert_eq!(rects.bottom.unwrap().height(), 180.0);
        assert!(rects.center.width() >= MIN_CANVAS_WIDTH);
        assert_eq!(state.left_width, 320.0);

        let compact = run_layout(layout, Vec2::new(700.0, 500.0));
        assert!(compact.center.width() >= MIN_CANVAS_WIDTH);
        assert_eq!(state.left_width, 320.0);
    }

    #[test]
    fn dragging_a_split_changes_only_the_internal_panel_preference() {
        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let layout = StudioPanelLayout::default().with_left();
        let size = Vec2::new(1_200.0, 700.0);
        let mut state = PanelState::default();
        let mut initial_rects = None;
        let frame = |input: egui::RawInput,
                     state: &mut PanelState,
                     output: &mut Option<StudioPanelRects>| {
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    let rects = layout.with_panel_sizes(state).allocate(ui);
                    *output = Some(rects);
                    StudioPanelLayout::show_resize_handles(
                        ui,
                        &rects,
                        state,
                        &palette,
                        "drag_test",
                    );
                });
            });
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        frame(input.clone(), &mut state, &mut initial_rects);
        let rects = initial_rects.unwrap();
        let left = rects.left.unwrap();
        let start = egui::pos2((left.right() + rects.center.left()) * 0.5, left.center().y);
        let mut press = input.clone();
        press.time = Some(0.1);
        press.events.push(egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        frame(press, &mut state, &mut initial_rects);

        let mut drag = input.clone();
        drag.time = Some(0.2);
        drag.events
            .push(egui::Event::PointerMoved(start + Vec2::new(40.0, 0.0)));
        frame(drag, &mut state, &mut initial_rects);
        assert!(state.left_width > crate::shell::PANEL_LEFT_DEFAULT_WIDTH);
        assert_eq!(initial_rects.unwrap().center.right(), rects.center.right());

        let mut release = input.clone();
        release.time = Some(0.3);
        release.events.push(egui::Event::PointerButton {
            pos: start + Vec2::new(40.0, 0.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        frame(release, &mut state, &mut initial_rects);
        let dragged_width = state.left_width;

        let mut key = input;
        key.time = Some(0.4);
        key.events.push(egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        frame(key, &mut state, &mut initial_rects);
        assert_eq!(state.left_width, dragged_width + 8.0);
    }

    #[test]
    fn omitted_panels_return_space_to_center() {
        let center_only = run_layout(StudioPanelLayout::center_only(), Vec2::new(900.0, 600.0));
        let left_only = run_layout(
            StudioPanelLayout::default().with_left(),
            Vec2::new(900.0, 600.0),
        );
        assert!(center_only.left.is_none());
        assert!(center_only.right.is_none());
        assert_eq!(center_only.center.width() - left_only.center.width(), 248.0);
    }

    #[test]
    fn bottom_panel_reduces_only_main_height() {
        let rects = run_layout(
            StudioPanelLayout::default()
                .with_left()
                .with_right()
                .with_bottom(),
            Vec2::new(1_200.0, 700.0),
        );
        assert_eq!(rects.bottom.unwrap().height(), 96.0);
        assert_eq!(rects.left.unwrap().height(), rects.center.height());
        assert_eq!(rects.right.unwrap().height(), rects.center.height());
    }

    #[test]
    fn constrained_panels_keep_canvas_usable_without_overlap() {
        let layout = StudioPanelLayout::default()
            .with_left()
            .with_right()
            .with_bottom();
        for size in [
            Vec2::new(900.0, 540.0),
            Vec2::new(680.0, 420.0),
            Vec2::new(480.0, 320.0),
        ] {
            let host = Rect::from_min_size(egui::Pos2::ZERO, size);
            let rects = layout.rects(host);
            let regions = [
                rects.left.unwrap(),
                rects.center,
                rects.right.unwrap(),
                rects.bottom.unwrap(),
            ];
            assert!(rects.center.width() >= 320.0);
            assert!(rects.center.height() > 0.0);
            for (index, first) in regions.iter().enumerate() {
                assert!(host.contains(first.min) && host.contains(first.max));
                for second in &regions[index + 1..] {
                    assert!(
                        first.intersect(*second).is_negative(),
                        "regions overlap at {size:?}: {first:?} {second:?}"
                    );
                }
            }
        }
    }
}
