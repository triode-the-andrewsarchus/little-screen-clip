use crate::hex_to_c32;
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;
use eframe::egui::{
    self, Color32, FontFamily, FontId, Pos2, Rect, Response, RichText, Rounding, Stroke, TextStyle,
    Ui, Vec2,
};
use little_clip_core::palette::{AppPalette, contrast_ratio};

pub const CONTROL_HEIGHT: f32 = 36.0;
pub const CONTROL_ICON_SIZE: f32 = 18.0;
pub const BODY_FONT_SIZE: f32 = 16.0;
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 8.0;
pub const SPACE_MD: f32 = 12.0;
pub const BUTTON_PADDING_X: f32 = 10.0;
pub const BUTTON_PADDING_Y: f32 = 6.0;
pub const ITEM_SPACING_X: f32 = 8.0;
pub const ITEM_SPACING_Y: f32 = 6.0;
pub const BADGE_PADDING_X: f32 = 6.0;
pub const BADGE_PADDING_Y: f32 = 2.0;
pub const PANEL_INSET_X: f32 = 14.0;
pub const CONTROL_GROUP_INSET_Y: f32 = 6.0;
pub const CONTROL_ROW_INSET_Y: f32 = 5.0;
pub const FOOTER_INSET_Y: f32 = 6.0;
pub const BADGE_CORNER_RADIUS: f32 = 3.0;

/// Keeps the regular help text available while explaining why a disabled control cannot run.
pub fn on_hover_text_with_disabled_reason(
    response: egui::Response,
    hover_text: impl Into<egui::WidgetText>,
    disabled_reason: impl Into<egui::WidgetText>,
) -> egui::Response {
    response
        .on_hover_text(hover_text)
        .on_disabled_hover_text(disabled_reason)
}
pub const HEADING_FONT_SIZE: f32 = 22.0;
pub const SUBHEADING_FONT_SIZE: f32 = 19.0;
pub const MONO_FONT_SIZE: f32 = 15.0;
pub const CONTROL_CORNER_RADIUS: f32 = 4.0;

/// A readable, content-sized annotation on an image or other preview canvas.
pub struct CanvasBadge<'a> {
    pub heading: &'a str,
    pub value: Option<&'a str>,
    pub accent: Option<Color32>,
}

impl CanvasBadge<'_> {
    pub fn paint(
        &self,
        painter: &egui::Painter,
        position: Pos2,
        anchor: egui::Align2,
        bounds: Rect,
        max_width: f32,
        palette: &AppPalette,
    ) -> Rect {
        let bounds = bounds.intersect(painter.clip_rect());
        if !bounds.is_positive() {
            return Rect::NOTHING;
        }
        let painter = painter.with_clip_rect(bounds);
        let background = hex_to_c32(&palette.colors.surface_recessed);
        let heading_color = contrast_text_c32(
            background,
            Some(hex_to_c32(&palette.colors.text_secondary)),
            palette,
        );
        let value_color = contrast_text_c32(
            background,
            self.accent
                .or(Some(hex_to_c32(&palette.colors.text_primary))),
            palette,
        );
        let padding = Vec2::new(8.0, 5.0);
        let heading_font = FontId::proportional(BODY_FONT_SIZE);
        let value_font = FontId::monospace(MONO_FONT_SIZE);
        let natural_width = painter
            .layout_no_wrap(self.heading.to_owned(), heading_font.clone(), heading_color)
            .size()
            .x
            .max(
                self.value
                    .map(|value| {
                        painter
                            .layout_no_wrap(value.to_owned(), value_font.clone(), value_color)
                            .size()
                            .x
                    })
                    .unwrap_or(0.0),
            );
        let width = (natural_width + padding.x * 2.0)
            .min(max_width)
            .min(bounds.width())
            .max(1.0);
        let text_width = (width - padding.x * 2.0).max(1.0);
        let heading = painter.layout(
            self.heading.to_owned(),
            heading_font,
            heading_color,
            text_width,
        );
        let value = self
            .value
            .map(|value| painter.layout(value.to_owned(), value_font, value_color, text_width));
        let heading_height = heading.size().y;
        let value_height = value
            .as_ref()
            .map(|galley| galley.size().y + 2.0)
            .unwrap_or(0.0);
        let size = Vec2::new(width, heading_height + value_height + padding.y * 2.0);
        let rect = clamp_overlay_rect(anchor.anchor_size(position, size).min, size, bounds);
        painter.rect_filled(rect, Rounding::same(CONTROL_CORNER_RADIUS), background);
        painter.rect_stroke(
            rect,
            Rounding::same(CONTROL_CORNER_RADIUS),
            Stroke::new(
                1.0_f32,
                self.accent
                    .unwrap_or_else(|| hex_to_c32(&palette.colors.surface_panel)),
            ),
        );
        let text_min = rect.min + padding;
        painter.galley(text_min, heading, heading_color);
        if let Some(value) = value {
            painter.galley(
                text_min + Vec2::new(0.0, heading_height + 2.0),
                value,
                value_color,
            );
        }
        rect
    }
}

/// Draws the shared comparison handle and returns its matching hit radius.
pub fn paint_split_handle(painter: &egui::Painter, center: Pos2, palette: &AppPalette) -> f32 {
    let background = hex_to_c32(&palette.colors.surface_recessed);
    let foreground = contrast_text_c32(
        background,
        Some(hex_to_c32(&palette.colors.text_primary)),
        palette,
    );
    let galley = painter.layout_no_wrap(
        "◀ ▶".to_owned(),
        FontId::monospace(MONO_FONT_SIZE),
        foreground,
    );
    let radius = (CONTROL_HEIGHT * 0.5).max(galley.size().length() * 0.5 + 3.0);
    painter.circle(
        center,
        radius,
        background,
        Stroke::new(2.0_f32, hex_to_c32(&palette.harmonies.base)),
    );
    painter.galley(center - galley.size() * 0.5, galley, foreground);
    radius
}

/// Hit-tests both the divider strip and its measured circular handle.
pub fn split_handle_hit_test(
    pointer: Option<Pos2>,
    image: Rect,
    center: Pos2,
    radius: f32,
    strip_half_width: f32,
) -> bool {
    pointer.is_some_and(|point| {
        ((point.x - center.x).abs() <= strip_half_width
            && point.y >= image.top()
            && point.y <= image.bottom())
            || point.distance(center) <= radius
    })
}

/// Places a floating overlay inside its visible canvas whenever it fits.
pub fn clamp_overlay_rect(ideal_min: Pos2, size: Vec2, bounds: Rect) -> Rect {
    let max_x = (bounds.max.x - size.x).max(bounds.min.x);
    let max_y = (bounds.max.y - size.y).max(bounds.min.y);
    Rect::from_min_size(
        Pos2::new(
            ideal_min.x.clamp(bounds.min.x, max_x),
            ideal_min.y.clamp(bounds.min.y, max_y),
        ),
        size,
    )
}

/// Set readable workbench defaults. Secondary text uses color and weight for
/// hierarchy; `.small()` must remain as legible as ordinary control labels.
pub fn configure_workbench_style(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(BODY_FONT_SIZE, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(BODY_FONT_SIZE, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(BODY_FONT_SIZE, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(HEADING_FONT_SIZE, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(MONO_FONT_SIZE, FontFamily::Monospace),
        );
        style.spacing.interact_size.y = CONTROL_HEIGHT;
        style.spacing.button_padding = Vec2::new(BUTTON_PADDING_X, BUTTON_PADDING_Y);
        style.spacing.item_spacing = Vec2::new(ITEM_SPACING_X, ITEM_SPACING_Y);
    });
}

/// Minimum width shared by related buttons, measured from their longest label.
pub fn equal_button_width(ui: &Ui, labels: &[&str], with_icon: bool) -> f32 {
    let font = TextStyle::Button.resolve(ui.style());
    let text_width = ui.fonts(|fonts| {
        labels
            .iter()
            .map(|label| {
                fonts
                    .layout_no_wrap((*label).to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
    });
    text_width
        + 2.0 * ui.spacing().button_padding.x
        + if with_icon {
            CONTROL_ICON_SIZE + ui.spacing().icon_spacing
        } else {
            0.0
        }
}

/// Equal button width that fills available space while allowing a wrapped row.
pub fn wrapped_equal_button_width(
    ui: &Ui,
    labels: &[&str],
    with_icon: bool,
    max_columns: usize,
) -> f32 {
    let minimum = equal_button_width(ui, labels, with_icon);
    let spacing = ui.spacing().item_spacing.x;
    let available = ui.available_width().max(1.0);
    let columns = (((available + spacing) / (minimum + spacing)).floor() as usize)
        .clamp(1, max_columns.min(labels.len()).max(1));
    (available - spacing * (columns - 1) as f32) / columns as f32
}

/// Calculates an effective foreground Color32 that meets WCAG 2.1 4.5:1 contrast
/// against `bg`. If `bg` is semi-transparent, it is alpha-composited against
/// the theme's panel background. Call `contrast_text_c32_on` for another surface.
pub fn contrast_text_c32(
    bg: Color32,
    preferred_fg: Option<Color32>,
    palette: &AppPalette,
) -> Color32 {
    contrast_text_c32_on(
        bg,
        WorkbenchColors::from_palette(palette).recessed,
        preferred_fg,
        palette,
    )
}

/// Choose readable text for a fill painted over a known opaque surface.
pub fn contrast_text_c32_on(
    bg: Color32,
    surface: Color32,
    preferred_fg: Option<Color32>,
    palette: &AppPalette,
) -> Color32 {
    let effective_bg = composited_rgb(bg, surface);

    if let Some(pref) = preferred_fg {
        let pref_rgb = (pref.r(), pref.g(), pref.b());
        if contrast_ratio(effective_bg, pref_rgb) >= 4.5 {
            return pref;
        }
    }

    let hex_bg = format!(
        "#{:02x}{:02x}{:02x}",
        effective_bg.0, effective_bg.1, effective_bg.2
    );
    let pref_hex = preferred_fg.map(|c| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()));
    let fg_hex = palette.contrast_pair_fg(&hex_bg, pref_hex.as_deref());
    hex_to_c32(&fg_hex)
}

fn composited_rgb(fill: Color32, surface: Color32) -> (u8, u8, u8) {
    let foreground = egui::Rgba::from(fill);
    let background = egui::Rgba::from(surface);
    let remaining = 1.0 - foreground.a();
    let result = Color32::from(egui::Rgba::from_rgb(
        foreground.r() + background.r() * remaining,
        foreground.g() + background.g() * remaining,
        foreground.b() + background.b() * remaining,
    ));
    (result.r(), result.g(), result.b())
}

/// Builds an `egui::Button` where the text color is GUARANTEED to meet WCAG 2.1 4.5:1
/// contrast against the button's background `fill`.
pub fn themed_button<'a>(
    text: impl Into<String>,
    fill: Color32,
    preferred_fg: Option<Color32>,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(fill, preferred_fg, palette);
    egui::Button::new(RichText::new(text.into()).color(fg))
        .fill(fill)
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
}

/// Applies contrast-guaranteed text color to an existing `RichText` for a given `fill`.
pub fn themed_button_rich<'a>(
    rich: RichText,
    fill: Color32,
    preferred_fg: Option<Color32>,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(fill, preferred_fg, palette);
    egui::Button::new(rich.color(fg))
        .fill(fill)
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
}

/// Standard mode/tab toggle button with guaranteed WCAG contrast in both active and inactive states.
pub fn mode_tab_button<'a>(
    label: impl Into<String>,
    is_active: bool,
    palette: &AppPalette,
) -> egui::Button<'a> {
    mode_tab_button_icon(None, label, is_active, palette)
}

/// Mode/tab toggle button with optional SVG icon and guaranteed WCAG contrast in both active and inactive states.
pub fn mode_tab_button_icon<'a>(
    icon: Option<Icon>,
    label: impl Into<String>,
    is_active: bool,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let colors = WorkbenchColors::from_palette(palette);
    let (bg, border, pref_fg) = if is_active {
        (colors.accent, colors.text_primary, colors.text_primary)
    } else {
        (colors.panel, colors.recessed, colors.text_secondary)
    };
    let fg = contrast_text_c32(bg, Some(pref_fg), palette);
    let border_fg = if is_active { fg } else { border };
    let text = RichText::new(label.into()).strong().color(fg);

    let btn = if let Some(ic) = icon {
        egui::Button::image_and_text(ic.image_tinted(CONTROL_ICON_SIZE, fg), text)
    } else {
        egui::Button::new(text)
    };

    btn.fill(bg)
        .stroke(Stroke::new(1.0_f32, border_fg))
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT))
}

/// Standard primary action button (e.g. Copy, Save, Apply) with guaranteed WCAG contrast.
pub fn action_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    action_button_icon(None, label, fill, palette)
}

/// Primary action button with an optional SVG icon.
pub fn action_button_icon<'a>(
    icon: Option<Icon>,
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(
        fill,
        Some(WorkbenchColors::from_palette(palette).text_primary),
        palette,
    );
    let text = RichText::new(label.into()).strong().color(fg);
    let btn = if let Some(ic) = icon {
        egui::Button::image_and_text(ic.image_tinted(CONTROL_ICON_SIZE, fg), text)
    } else {
        egui::Button::new(text)
    };
    btn.fill(fill)
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT))
}

/// Standard secondary / subtle button (e.g. Reset, Cancel, Clear, Fit) with guaranteed WCAG contrast.
pub fn secondary_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    secondary_button_icon(None, label, fill, palette)
}

/// Secondary button with an optional SVG icon.
pub fn secondary_button_icon<'a>(
    icon: Option<Icon>,
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(
        fill,
        Some(WorkbenchColors::from_palette(palette).text_secondary),
        palette,
    );
    let text = RichText::new(label.into()).color(fg);
    let btn = if let Some(ic) = icon {
        egui::Button::image_and_text(ic.image_tinted(CONTROL_ICON_SIZE, fg), text)
    } else {
        egui::Button::new(text)
    };
    btn.fill(fill)
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT))
}

/// Compact toolbar control with the same contrast and minimum hit target as the
/// rest of the workbench buttons. Use this instead of ad-hoc tiny buttons.
pub fn compact_toolbar_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    compact_toolbar_button_icon(None, label, fill, palette)
}

/// Compact toolbar control with an optional SVG icon.
pub fn compact_toolbar_button_icon<'a>(
    icon: Option<Icon>,
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    secondary_button_icon(icon, label, fill, palette)
}

/// Standard danger / escape / dismiss button (e.g. Close, Delete, Burn) with guaranteed WCAG contrast.
pub fn danger_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    danger_button_icon(None, label, fill, palette)
}

/// Danger button with an optional SVG icon.
pub fn danger_button_icon<'a>(
    icon: Option<Icon>,
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(
        fill,
        Some(WorkbenchColors::from_palette(palette).text_primary),
        palette,
    );
    let text = RichText::new(label.into()).strong().color(fg);
    let btn = if let Some(ic) = icon {
        egui::Button::image_and_text(ic.image_tinted(CONTROL_ICON_SIZE, fg), text)
    } else {
        egui::Button::new(text)
    };
    btn.fill(fill)
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT))
}

/// Standalone icon button without text (useful for compact toolbars, zoom controls, playback).
pub fn icon_button<'a>(
    icon: Icon,
    size: f32,
    fill: Color32,
    preferred_fg: Option<Color32>,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let colors = WorkbenchColors::from_palette(palette);
    let fg = contrast_text_c32(fill, preferred_fg.or(Some(colors.text_primary)), palette);
    egui::Button::image(icon.image_tinted(size, fg))
        .fill(fill)
        .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
        .min_size(Vec2::splat(CONTROL_HEIGHT))
}

/// Trait providing high-level, contrast-safe button widgets directly on `egui::Ui`.
pub trait UiThemedExt {
    fn mode_tab(
        &mut self,
        label: impl Into<String>,
        is_active: bool,
        palette: &AppPalette,
    ) -> Response;
    fn mode_tab_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        is_active: bool,
        palette: &AppPalette,
    ) -> Response;
    fn action_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response;
    fn action_btn_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response;
    fn secondary_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response;
    fn secondary_btn_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response;
    fn danger_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response;
    fn danger_btn_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response;
    fn icon_btn(
        &mut self,
        icon: Icon,
        size: f32,
        fill: Color32,
        preferred_fg: Option<Color32>,
        palette: &AppPalette,
    ) -> Response;
    fn contrast_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        preferred_fg: Option<Color32>,
        palette: &AppPalette,
    ) -> Response;
}

impl UiThemedExt for Ui {
    fn mode_tab(
        &mut self,
        label: impl Into<String>,
        is_active: bool,
        palette: &AppPalette,
    ) -> Response {
        self.add(mode_tab_button(label, is_active, palette))
    }

    fn mode_tab_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        is_active: bool,
        palette: &AppPalette,
    ) -> Response {
        self.add(mode_tab_button_icon(icon, label, is_active, palette))
    }

    fn action_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(action_button(label, fill, palette))
    }

    fn action_btn_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(action_button_icon(icon, label, fill, palette))
    }

    fn secondary_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(secondary_button(label, fill, palette))
    }

    fn secondary_btn_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(secondary_button_icon(icon, label, fill, palette))
    }

    fn danger_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(danger_button(label, fill, palette))
    }

    fn danger_btn_icon(
        &mut self,
        icon: Option<Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(danger_button_icon(icon, label, fill, palette))
    }

    fn icon_btn(
        &mut self,
        icon: Icon,
        size: f32,
        fill: Color32,
        preferred_fg: Option<Color32>,
        palette: &AppPalette,
    ) -> Response {
        self.add(icon_button(icon, size, fill, preferred_fg, palette))
    }

    fn contrast_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        preferred_fg: Option<Color32>,
        palette: &AppPalette,
    ) -> Response {
        self.add(themed_button(label, fill, preferred_fg, palette))
    }
}

/// Unified status toast manager with automatic time-based dismissal.
#[derive(Debug, Clone)]
pub struct StatusToast {
    toast: Option<(String, f64)>,
    duration: f64,
}

impl Default for StatusToast {
    fn default() -> Self {
        Self {
            toast: None,
            duration: 4.0,
        }
    }
}

impl StatusToast {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_duration(duration: f64) -> Self {
        Self {
            toast: None,
            duration,
        }
    }

    pub fn set(&mut self, msg: impl Into<String>, now: f64) {
        self.toast = Some((msg.into(), now));
    }

    pub fn clear(&mut self) {
        self.toast = None;
    }

    pub fn is_active(&self, now: f64) -> bool {
        match &self.toast {
            Some((_, timestamp)) => now - *timestamp < self.duration,
            None => false,
        }
    }

    pub fn message(&self, now: f64) -> Option<&str> {
        match &self.toast {
            Some((msg, timestamp)) if now - *timestamp < self.duration => Some(msg.as_str()),
            _ => None,
        }
    }

    pub fn render_opt(
        toast: &Option<(String, f64)>,
        duration: f64,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        now: f64,
    ) {
        if let Some((msg, ts)) = toast {
            if now - ts < duration {
                let color = WorkbenchColors::from_palette(palette).success;
                ui.label(egui::RichText::new(msg).strong().color(color));
            }
        }
    }

    pub fn render_pill(&self, ui: &mut egui::Ui, palette: &AppPalette, now: f64) {
        if let Some(msg) = self.message(now) {
            let colors = WorkbenchColors::from_palette(palette);
            let bg = colors.recessed;
            let stroke = colors.success;
            let fg = contrast_text_c32(bg, Some(stroke), palette);
            egui::Frame::none()
                .fill(bg)
                .stroke(Stroke::new(1.0_f32, stroke))
                .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::symmetric(SPACE_SM, SPACE_XS))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).color(fg));
                });
        }
    }

    pub fn render(&mut self, ui: &mut egui::Ui, now: f64, palette: &AppPalette) {
        if let Some(msg) = self.message(now) {
            let colors = WorkbenchColors::from_palette(palette);
            let bg = colors.accent;
            let fg = contrast_text_c32(bg, Some(colors.text_primary), palette);
            egui::Frame::none()
                .fill(bg)
                .rounding(Rounding::same(CONTROL_CORNER_RADIUS))
                .inner_margin(egui::Margin::symmetric(BUTTON_PADDING_X, SPACE_XS))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).strong().color(fg));
                });
        }
    }
}

/// Standard label + value indicator (e.g. "Matches: 120 B" or "Deletions: 4").
pub fn stat_pill(
    ui: &mut egui::Ui,
    label: &str,
    value: impl std::fmt::Display,
    val_color: Option<Color32>,
    palette: &AppPalette,
) {
    let colors = WorkbenchColors::from_palette(palette);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(SPACE_XS, 0.0);
        ui.label(egui::RichText::new(format!("{}:", label)).color(colors.text_muted));
        let color = val_color.unwrap_or(colors.accent);
        ui.label(
            egui::RichText::new(format!("{}", value))
                .monospace()
                .strong()
                .color(color),
        );
    });
}

/// Renders a standardized status pill for badges and counts.
pub fn badge_pill(ui: &mut egui::Ui, label: &str, accent: Option<Color32>, palette: &AppPalette) {
    let colors = WorkbenchColors::from_palette(palette);
    let bg = accent.unwrap_or(colors.recessed);
    let fg = contrast_text_c32(bg, Some(colors.text_primary), palette);
    egui::Frame::none()
        .fill(bg)
        .rounding(Rounding::same(BADGE_CORNER_RADIUS))
        .inner_margin(egui::Margin::symmetric(BADGE_PADDING_X, BADGE_PADDING_Y))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).color(fg));
        });
}

/// Renders a standard labeled readout pair in sidebars (e.g. "Dimensions", "Duration").
pub fn sidebar_readout(
    ui: &mut egui::Ui,
    label: &str,
    value: impl std::fmt::Display,
    val_color: Option<Color32>,
    palette: &AppPalette,
) {
    let colors = WorkbenchColors::from_palette(palette);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(colors.text_muted));
        let color = val_color.unwrap_or(colors.accent);
        ui.label(
            egui::RichText::new(format!("{}", value))
                .monospace()
                .strong()
                .color(color),
        );
    });
}

/// Actions emitted by the zoom control toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomAction {
    ZoomIn,
    ZoomOut,
    Fit,
}

/// Renders a standardized zoom toolbar with ZoomIn, ZoomOut, ZoomFit SVG icons and percentage label.
pub fn zoom_control_bar(
    ui: &mut egui::Ui,
    zoom_pct: u32,
    palette: &AppPalette,
) -> Option<ZoomAction> {
    let mut action = None;
    let colors = WorkbenchColors::from_palette(palette);
    let btn_bg = colors.recessed;
    let zoom_in_btn = icon_button(Icon::ZoomIn, CONTROL_ICON_SIZE, btn_bg, None, palette);
    if ui
        .add(zoom_in_btn)
        .on_hover_text("Zoom In (+ / Ctrl+Wheel)")
        .clicked()
    {
        action = Some(ZoomAction::ZoomIn);
    }
    let zoom_out_btn = icon_button(Icon::ZoomOut, CONTROL_ICON_SIZE, btn_bg, None, palette);
    if ui
        .add(zoom_out_btn)
        .on_hover_text("Zoom Out (- / Ctrl+Wheel)")
        .clicked()
    {
        action = Some(ZoomAction::ZoomOut);
    }
    let fit_btn = secondary_button_icon(Some(Icon::ZoomFit), "Fit", btn_bg, palette);
    if ui
        .add(fit_btn)
        .on_hover_text("Fit Track / Window (Ctrl+0)")
        .clicked()
    {
        action = Some(ZoomAction::Fit);
    }
    ui.label(
        egui::RichText::new(format!("{}%", zoom_pct))
            .monospace()
            .color(colors.text_secondary),
    );
    action
}

static ZENITY_AVAILABLE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    if std::path::Path::new("/usr/bin/zenity").exists() {
        return true;
    }
    std::process::Command::new("zenity")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
});

/// Returns whether the `zenity` native dialog utility is installed and available.
/// The result is cached after the first check to prevent frame drops on the UI thread.
pub fn is_zenity_available() -> bool {
    *ZENITY_AVAILABLE
}

static SHARED_AUDIO_SINK: std::sync::LazyLock<Option<rodio::stream::MixerDeviceSink>> =
    std::sync::LazyLock::new(|| {
        rodio::stream::DeviceSinkBuilder::open_default_sink()
            .ok()
            .map(|mut s| {
                s.log_on_drop(false);
                s
            })
    });

/// Creates a new `rodio::Player` connected to the shared application audio sink.
/// This avoids opening competing audio hardware sinks across preview widgets.
pub fn new_shared_audio_player() -> Option<rodio::Player> {
    SHARED_AUDIO_SINK
        .as_ref()
        .map(|s| rodio::Player::connect_new(s.mixer()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::config::WorkbenchAppearance;

    fn rendered_text(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn enabled_help_and_disabled_reason_are_both_hoverable() {
        fn draw(
            ctx: &egui::Context,
            time: f64,
            pointer: Option<Pos2>,
            enabled: bool,
        ) -> (egui::FullOutput, Rect) {
            let mut button_rect = Rect::NOTHING;
            let output = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                    events: pointer
                        .map(|pos| vec![egui::Event::PointerMoved(pos)])
                        .unwrap_or_default(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui.add_enabled(enabled, egui::Button::new("Open"));
                        button_rect = response.rect;
                        on_hover_text_with_disabled_reason(
                            response,
                            "Open the viewer",
                            "Wait for loading to finish",
                        );
                    });
                },
            );
            (output, button_rect)
        }

        for (enabled, expected) in [
            (true, "Open the viewer"),
            (false, "Wait for loading to finish"),
        ] {
            let ctx = egui::Context::default();
            ctx.style_mut(|style| style.interaction.tooltip_delay = 0.0);
            let (_, button_rect) = draw(&ctx, 0.0, None, enabled);
            let _ = draw(&ctx, 1.0, Some(button_rect.center()), enabled);
            let (output, _) = draw(&ctx, 1.1, Some(button_rect.center()), enabled);
            let texts = rendered_text(&output);
            assert!(
                texts.iter().any(|text| text == expected),
                "expected hover text {expected:?}, got {texts:?}"
            );
        }
    }

    #[test]
    fn canvas_badges_fit_compact_views_without_expanding_on_wide_canvases() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let bounds =
                        Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(320.0, 140.0));
                    let value = "WEBP · 554.4 KB";
                    let before = CanvasBadge {
                        heading: "BEFORE",
                        value: Some(value),
                        accent: None,
                    };
                    let after = CanvasBadge {
                        heading: "AFTER",
                        value: Some(value),
                        accent: Some(hex_to_c32(&palette.harmonies.base)),
                    };
                    let left = before.paint(
                        ui.painter(),
                        bounds.min + Vec2::splat(8.0),
                        egui::Align2::LEFT_TOP,
                        bounds,
                        148.0,
                        &palette,
                    );
                    let right = after.paint(
                        ui.painter(),
                        bounds.max - Vec2::new(8.0, bounds.height() - 8.0),
                        egui::Align2::RIGHT_TOP,
                        bounds,
                        148.0,
                        &palette,
                    );
                    assert!(bounds.contains_rect(left) && bounds.contains_rect(right));
                    assert!(left.right() < right.left());
                    let wide =
                        Rect::from_min_size(Pos2::new(20.0, 200.0), Vec2::new(1000.0, 200.0));
                    let badge = before.paint(
                        ui.painter(),
                        wide.min,
                        egui::Align2::LEFT_TOP,
                        wide,
                        480.0,
                        &palette,
                    );
                    assert!(
                        badge.width() < 200.0,
                        "a short badge must retain its natural width"
                    );
                });
            });
            let background = hex_to_c32(&palette.colors.surface_recessed);
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    for section in &text.galley.job.sections {
                        assert!(
                            contrast_ratio(
                                (
                                    section.format.color.r(),
                                    section.format.color.g(),
                                    section.format.color.b()
                                ),
                                (background.r(), background.g(), background.b())
                            ) >= 4.5
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn split_handle_hit_radius_contains_the_readable_icon() {
        let ctx = egui::Context::default();
        let mut radius = 0.0;
        let center = Pos2::new(100.0, 100.0);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                radius = paint_split_handle(ui.painter(), center, &AppPalette::default());
            });
        });
        let icon = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "◀ ▶" => Some(text),
                _ => None,
            })
            .unwrap();
        let icon_rect = Rect::from_min_size(icon.pos, icon.galley.size());
        assert!(icon_rect.left_top().distance(center) <= radius);
        assert!(icon_rect.right_bottom().distance(center) <= radius);
        assert!(radius >= CONTROL_HEIGHT * 0.5);
        let image = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(160.0));
        assert!(split_handle_hit_test(
            Some(center + Vec2::new(radius * 0.95, 0.0)),
            image,
            center,
            radius,
            16.0
        ));
        assert!(!split_handle_hit_test(
            Some(center + Vec2::splat(radius)),
            image,
            center,
            radius,
            16.0
        ));
        assert!(split_handle_hit_test(
            Some(Pos2::new(center.x, image.top())),
            image,
            center,
            radius,
            16.0
        ));
        assert!(!split_handle_hit_test(None, image, center, radius, 16.0));
    }

    #[test]
    fn shared_control_gallery_keeps_readable_type_and_equal_hit_heights() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let colors = WorkbenchColors::from_palette(&palette);
            let ctx = egui::Context::default();
            configure_workbench_style(&ctx);
            crate::ui_appearance::apply_visuals(&ctx, &palette);
            let style = ctx.style();
            assert!(TextStyle::Small.resolve(&style).size >= BODY_FONT_SIZE);
            assert!(TextStyle::Button.resolve(&style).size >= BODY_FONT_SIZE);
            drop(style);

            let mut input = egui::RawInput::default();
            input.screen_rect = Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(900.0, 300.0),
            ));
            let mut heights = Vec::new();
            let mut input_text = String::new();
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        heights.push(
                            ui.add(mode_tab_button("Inspect", true, &palette))
                                .rect
                                .height(),
                        );
                        heights.push(
                            ui.add(action_button("Copy", colors.success, &palette))
                                .rect
                                .height(),
                        );
                        heights.push(
                            ui.add(secondary_button("Reset", colors.recessed, &palette))
                                .rect
                                .height(),
                        );
                        heights.push(
                            ui.add(danger_button("Delete", colors.danger, &palette))
                                .rect
                                .height(),
                        );
                        heights.push(
                            ui.add(icon_button(
                                Icon::Close,
                                CONTROL_ICON_SIZE,
                                colors.panel,
                                None,
                                &palette,
                            ))
                            .rect
                            .height(),
                        );
                        heights.push(
                            ui.add_sized(
                                [150.0, CONTROL_HEIGHT],
                                egui::TextEdit::singleline(&mut input_text),
                            )
                            .rect
                            .height(),
                        );
                    });
                });
            });
            assert_eq!(heights.len(), 6);
            assert!(
                heights.iter().all(|&height| height >= CONTROL_HEIGHT),
                "{appearance:?} control gallery has a short hit target: {heights:?}"
            );
        }
    }

    #[test]
    fn translucent_fill_is_composited_in_linear_light() {
        let red = Color32::from_rgba_unmultiplied(255, 0, 0, 128);
        let over_white = composited_rgb(red, Color32::WHITE);
        let over_black = composited_rgb(red, Color32::BLACK);
        assert_eq!(over_white.0, 255);
        assert!((186..=189).contains(&over_white.1));
        assert_eq!(over_white.1, over_white.2);
        assert!((186..=189).contains(&over_black.0));
        assert_eq!((over_black.1, over_black.2), (0, 0));
    }

    #[test]
    fn test_contrast_text_guarantee() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            for surface in [&palette.harmonies.base, &palette.diff_add] {
                let bg = hex_to_c32(surface);
                let fg =
                    contrast_text_c32(bg, Some(hex_to_c32(&palette.colors.text_primary)), &palette);
                let ratio = contrast_ratio((bg.r(), bg.g(), bg.b()), (fg.r(), fg.g(), fg.b()));
                assert!(
                    ratio >= 4.5,
                    "{appearance:?}: contrast ratio {ratio} must be >= 4.5:1"
                );
            }
        }
    }

    #[test]
    fn test_mode_tab_contrast() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let active_btn = mode_tab_button("View", true, &palette);
            let inactive_btn = mode_tab_button("Crop", false, &palette);
            drop(active_btn);
            drop(inactive_btn);
        }
    }

    #[test]
    fn fixed_appearance_button_surfaces_meet_wcag_contrast() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let test_cases = [
                (
                    "active tab",
                    hex_to_c32(&palette.harmonies.base),
                    Some(hex_to_c32(&palette.colors.text_primary)),
                ),
                (
                    "inactive tab",
                    hex_to_c32(&palette.colors.surface_panel),
                    Some(hex_to_c32(&palette.colors.text_secondary)),
                ),
                (
                    "add",
                    hex_to_c32(&palette.diff_add),
                    Some(hex_to_c32(&palette.colors.text_primary)),
                ),
                (
                    "secondary",
                    hex_to_c32(&palette.colors.surface_recessed),
                    Some(hex_to_c32(&palette.colors.text_secondary)),
                ),
                (
                    "delete",
                    hex_to_c32(&palette.diff_del),
                    Some(hex_to_c32(&palette.colors.text_primary)),
                ),
                (
                    "translucent accent",
                    crate::hex_to_c32_alpha(&palette.harmonies.base, 40),
                    Some(hex_to_c32(&palette.harmonies.base)),
                ),
            ];

            for (name, bg, preferred) in test_cases {
                let fg = contrast_text_c32(bg, preferred, &palette);
                let effective_bg = composited_rgb(bg, hex_to_c32(&palette.colors.surface_recessed));
                let ratio = contrast_ratio(effective_bg, (fg.r(), fg.g(), fg.b()));
                assert!(
                    ratio >= 4.5,
                    "{appearance:?} failed WCAG AA on {name}: {ratio:.2} < 4.5"
                );
            }
        }
    }

    #[test]
    fn test_status_toast() {
        let mut toast = StatusToast::new();
        assert!(!toast.is_active(10.0));
        assert_eq!(toast.message(10.0), None);

        toast.set("Copied item!", 10.0);
        assert!(toast.is_active(12.0));
        assert_eq!(toast.message(12.0), Some("Copied item!"));

        // After 4s duration, it expires
        assert!(!toast.is_active(14.5));
        assert_eq!(toast.message(14.5), None);

        // Manual clear
        toast.set("New toast", 20.0);
        assert!(toast.is_active(20.5));
        toast.clear();
        assert!(!toast.is_active(20.5));
    }

    #[test]
    fn test_zoom_action_equality() {
        assert_eq!(ZoomAction::ZoomIn, ZoomAction::ZoomIn);
        assert_ne!(ZoomAction::ZoomIn, ZoomAction::ZoomOut);
        assert_ne!(ZoomAction::ZoomOut, ZoomAction::Fit);
    }

    #[test]
    fn floating_overlay_origin_stays_inside_canvas_even_when_badge_is_wide() {
        let bounds = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(100.0, 80.0));
        let fitted = clamp_overlay_rect(Pos2::new(100.0, 90.0), Vec2::new(30.0, 20.0), bounds);
        assert_eq!(fitted.max, bounds.max);

        let oversized = clamp_overlay_rect(Pos2::new(100.0, 90.0), Vec2::new(140.0, 20.0), bounds);
        assert_eq!(oversized.min.x, bounds.min.x);
        assert_eq!(oversized.max.y, bounds.max.y);
    }
}
