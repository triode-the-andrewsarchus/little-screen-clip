//! Unified 5-Zone StudioShell layout and presentation manager.
//!
//! Provides a standardized container for all preview and studio views,
//! enforcing visual consistency for headers, mode toolbars, multi-panel layouts,
//! and bottom status footers.

use super::escape_hatch::EscapeHatch;
use super::panel::PanelState;
use crate::hex_to_c32;
use crate::ui_ext::{
    CONTROL_HEIGHT, CONTROL_ICON_SIZE, StatusToast, ZoomAction, contrast_text_c32,
    equal_button_width, mode_tab_button, zoom_control_bar,
};
use eframe::egui::{self, Color32, Rect, Rounding, Stroke, Vec2};
use little_clip_core::theme::AppPalette;

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

/// Container for the 3-panel / 4-panel workspace layout closures.
pub struct PanelBody<'a, 'b> {
    pub left: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
    pub center: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
    pub right: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
    pub bottom: Option<Box<dyn FnOnce(&mut egui::Ui) + 'a>>,
    _marker: std::marker::PhantomData<&'b ()>,
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
                .fill(hex_to_c32(&palette.theme.b_low))
                .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_width((rect.width() - 16.0).max(1.0));
                    ui.set_max_width((rect.width() - 16.0).max(1.0));
                    ui.set_height((rect.height() - 16.0).max(1.0));
                    egui::ScrollArea::vertical()
                        .id_source(id)
                        .auto_shrink([false, false])
                        .show(ui, add_contents);
                });
        });
    }

    /// Render uncluttered center content inside its exact canvas rectangle.
    pub fn show_center(ui: &mut egui::Ui, rect: Rect, add_contents: impl FnOnce(&mut egui::Ui)) {
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
                .fill(hex_to_c32(&palette.theme.b_low))
                .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::same(8.0))
                .show(ui, add_contents);
        });
    }
}

impl<'a, 'b> PanelBody<'a, 'b> {
    pub fn new() -> Self {
        Self {
            left: None,
            center: None,
            right: None,
            bottom: None,
            _marker: std::marker::PhantomData,
        }
    }

    pub fn left(&mut self, f: impl FnOnce(&mut egui::Ui) + 'a) {
        self.left = Some(Box::new(f));
    }

    pub fn center(&mut self, f: impl FnOnce(&mut egui::Ui) + 'a) {
        self.center = Some(Box::new(f));
    }

    pub fn right(&mut self, f: impl FnOnce(&mut egui::Ui) + 'a) {
        self.right = Some(Box::new(f));
    }

    pub fn bottom(&mut self, f: impl FnOnce(&mut egui::Ui) + 'a) {
        self.bottom = Some(Box::new(f));
    }
}

/// The standard StudioShell component builder.
pub struct StudioShell<'a> {
    pub title: String,
    pub icon: Option<Icon>,
    pub palette: &'a AppPalette,
    pub close: &'a mut bool,
    pub badges: Vec<HeaderBadge>,
    pub escape_hatches: Vec<EscapeHatch>,
    pub footer_hints: Option<&'a str>,
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
            icon: icon.into(),
            palette,
            close,
            badges: Vec::new(),
            escape_hatches: Vec::new(),
            footer_hints: None,
            footer_readout: None,
        }
    }

    pub fn with_badge(mut self, text: impl Into<String>, accent: Option<Color32>) -> Self {
        self.badges.push(HeaderBadge::new(text, accent));
        self
    }

    pub fn with_escape_hatch(mut self, hatch: EscapeHatch) -> Self {
        self.escape_hatches.push(hatch);
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
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 8.0))
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if let Some(icon) = self.icon {
                        ui.add(icon.image_tinted(18.0, hex_to_c32(&palette.harmonies.base)));
                    }

                    ui.label(
                        egui::RichText::new(&self.title)
                            .strong()
                            .size(22.0)
                            .color(hex_to_c32(&palette.theme.f_high)),
                    );

                    ui.add_space(8.0);
                    title_actions(ui);

                    // Badges
                    for b in &self.badges {
                        crate::ui_ext::badge_pill(ui, &b.text, b.accent, palette);
                        ui.add_space(4.0);
                    }
                });

                ui.horizontal_wrapped(|ui| {
                    // Close remains reachable even when action groups wrap.
                    let close_bg = hex_to_c32(&palette.theme.b_med);
                    let close_fg =
                        contrast_text_c32(close_bg, Some(hex_to_c32(&palette.diff_del)), palette);
                    let close_btn = egui::Button::image_and_text(
                        Icon::Close.image_tinted(CONTROL_ICON_SIZE, close_fg),
                        egui::RichText::new("Close (Esc)").strong().color(close_fg),
                    )
                    .fill(close_bg)
                    .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_low)))
                    .rounding(Rounding::same(4.0))
                    .min_size(Vec2::new(0.0, CONTROL_HEIGHT));

                    if ui.add(close_btn).clicked() {
                        *self.close = true;
                    }

                    right_actions(ui);

                    // Escape Hatches
                    for (idx, hatch) in self.escape_hatches.iter().enumerate() {
                        let (bg, fg) = if hatch.is_primary {
                            (
                                hex_to_c32(&palette.harmonies.base),
                                hex_to_c32(&palette.theme.f_high),
                            )
                        } else {
                            (
                                hex_to_c32(&palette.theme.b_med),
                                hex_to_c32(&palette.theme.f_med),
                            )
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
                        .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_low)))
                        .rounding(Rounding::same(4.0))
                        .min_size(Vec2::new(hatch_width, CONTROL_HEIGHT));

                        if ui.add(btn).on_hover_text(&hatch.tooltip).clicked() {
                            clicked_hatch = Some(idx);
                        }
                        ui.add_space(4.0);
                    }
                });
            });

        clicked_hatch
    }

    /// Renders Zone 2 (Mode & Viewport Toolbar).
    pub fn render_mode_bar<M: Copy + PartialEq>(
        &self,
        ui: &mut egui::Ui,
        modes: &[M],
        current_mode: M,
        mode_label: impl Fn(M) -> &'static str,
        mut on_mode_select: impl FnMut(M),
        zoom_pct: Option<u32>,
        mut on_zoom: impl FnMut(ZoomAction),
    ) {
        let palette = self.palette;

        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for &mode in modes {
                        let is_active = mode == current_mode;
                        let btn = mode_tab_button(mode_label(mode), is_active, palette);
                        if ui.add(btn).clicked() {
                            on_mode_select(mode);
                        }
                    }

                    // Right side zoom controls if requested
                    if let Some(pct) = zoom_pct {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(action) = zoom_control_bar(ui, pct, palette) {
                                on_zoom(action);
                            }
                        });
                    }
                });
            });
    }

    /// Renders Zone 4 (Status & Shortcut Help Footer).
    pub fn render_footer(&self, ui: &mut egui::Ui, toast: &Option<(String, f64)>, now: f64) {
        let palette = self.palette;

        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(hints) = self.footer_hints {
                        ui.label(
                            egui::RichText::new(hints)
                                .small()
                                .color(hex_to_c32(&palette.theme.f_low)),
                        );
                    }

                    if let Some(ref readout) = self.footer_readout {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(readout)
                                .small()
                                .strong()
                                .monospace()
                                .color(hex_to_c32(&palette.harmonies.base)),
                        );
                    }

                    // Status toast
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        StatusToast::render_opt(toast, 4.5, ui, palette, now);
                    });
                });
            });
    }

    /// Renders Zone 3 in a standardized 3-panel / 4-panel layout with collapsible sidebars and bottom drawer.
    pub fn render_panels(
        &self,
        ui: &mut egui::Ui,
        panel_state: &mut PanelState,
        build_panels: impl FnOnce(&mut PanelBody),
    ) {
        let mut body = PanelBody::new();
        build_panels(&mut body);

        let left_visible = body.left.is_some() && !panel_state.left_collapsed;
        let right_visible = body.right.is_some() && !panel_state.right_collapsed;
        let bottom_visible = body.bottom.is_some() && !panel_state.bottom_collapsed;
        let layout = StudioPanelLayout {
            left: left_visible,
            right: right_visible,
            bottom: bottom_visible,
            left_width: panel_state.effective_left_width(),
            right_width: panel_state.effective_right_width(),
            bottom_height: panel_state.effective_bottom_height(),
            gap: 8.0,
        };
        let panels = layout.allocate(ui);

        if let (Some(rect), Some(left_fn)) = (panels.left, body.left.take()) {
            StudioPanelLayout::show_side(ui, rect, "studio_shell_left", self.palette, left_fn);
        }
        if let Some(center_fn) = body.center.take() {
            StudioPanelLayout::show_center(ui, panels.center, center_fn);
        }
        if let (Some(rect), Some(right_fn)) = (panels.right, body.right.take()) {
            StudioPanelLayout::show_side(ui, rect, "studio_shell_right", self.palette, right_fn);
        }
        if let (Some(rect), Some(bottom_fn)) = (panels.bottom, body.bottom.take()) {
            StudioPanelLayout::show_bottom(ui, rect, self.palette, bottom_fn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
