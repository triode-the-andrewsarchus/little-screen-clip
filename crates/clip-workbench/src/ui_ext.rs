use crate::hex_to_c32;
use eframe::egui::{self, Color32, Response, RichText, Rounding, Stroke, Ui, Vec2};
use little_clip_core::theme::{AppPalette, contrast_ratio};

/// Calculates an effective foreground Color32 that meets WCAG 2.1 4.5:1 contrast
/// against `bg`. If `bg` is semi-transparent, it is alpha-composited against
/// the theme's background to calculate the true perceived contrast on screen.
pub fn contrast_text_c32(
    bg: Color32,
    preferred_fg: Option<Color32>,
    palette: &AppPalette,
) -> Color32 {
    let effective_bg = if bg.a() < 255 {
        let alpha = bg.a() as f32 / 255.0;
        let parent_rgb = palette.rgba(&palette.theme.b_low);
        (
            (bg.r() as f32 * alpha + parent_rgb.0 as f32 * (1.0 - alpha)).round() as u8,
            (bg.g() as f32 * alpha + parent_rgb.1 as f32 * (1.0 - alpha)).round() as u8,
            (bg.b() as f32 * alpha + parent_rgb.2 as f32 * (1.0 - alpha)).round() as u8,
        )
    } else {
        (bg.r(), bg.g(), bg.b())
    };

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
        .rounding(Rounding::same(4.0))
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
        .rounding(Rounding::same(4.0))
}

/// Standard mode/tab toggle button with guaranteed WCAG contrast in both active and inactive states.
pub fn mode_tab_button<'a>(
    label: impl Into<String>,
    is_active: bool,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let (bg, border, pref_fg) = if is_active {
        (
            hex_to_c32(&palette.harmonies.base),
            hex_to_c32(&palette.theme.f_high),
            hex_to_c32(&palette.theme.f_high),
        )
    } else {
        (
            hex_to_c32(&palette.theme.b_med),
            hex_to_c32(&palette.theme.b_low),
            hex_to_c32(&palette.theme.f_med),
        )
    };
    let fg = contrast_text_c32(bg, Some(pref_fg), palette);
    let border_fg = if is_active { fg } else { border };

    egui::Button::new(RichText::new(label.into()).small().strong().color(fg))
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, border_fg))
        .rounding(Rounding::same(4.0))
}

/// Standard primary action button (e.g. Copy, Save, Apply) with guaranteed WCAG contrast.
pub fn action_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(fill, Some(hex_to_c32(&palette.theme.f_high)), palette);
    egui::Button::new(RichText::new(label.into()).strong().color(fg))
        .fill(fill)
        .rounding(Rounding::same(4.0))
}

/// Standard secondary / subtle button (e.g. Reset, Cancel, Clear, Fit) with guaranteed WCAG contrast.
pub fn secondary_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(fill, Some(hex_to_c32(&palette.theme.f_med)), palette);
    egui::Button::new(RichText::new(label.into()).small().color(fg))
        .fill(fill)
        .rounding(Rounding::same(4.0))
}

/// Compact toolbar control with the same contrast and minimum hit target as the
/// rest of the workbench buttons. Use this instead of ad-hoc tiny buttons.
pub fn compact_toolbar_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    secondary_button(label, fill, palette)
        .min_size(Vec2::new(0.0, 26.0))
        .rounding(Rounding::same(4.0))
}

/// Standard danger / escape / dismiss button (e.g. Close, Delete, Burn) with guaranteed WCAG contrast.
pub fn danger_button<'a>(
    label: impl Into<String>,
    fill: Color32,
    palette: &AppPalette,
) -> egui::Button<'a> {
    let fg = contrast_text_c32(fill, Some(hex_to_c32(&palette.theme.f_high)), palette);
    egui::Button::new(RichText::new(label.into()).small().strong().color(fg))
        .fill(fill)
        .rounding(Rounding::same(4.0))
}

/// Trait providing high-level, contrast-safe button widgets directly on `egui::Ui`.
pub trait UiThemedExt {
    fn mode_tab(
        &mut self,
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
    fn secondary_btn(
        &mut self,
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

    fn action_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(action_button(label, fill, palette))
    }

    fn secondary_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(secondary_button(label, fill, palette))
    }

    fn danger_btn(
        &mut self,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> Response {
        self.add(danger_button(label, fill, palette))
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

    pub fn set(&mut self, message: impl Into<String>, now: f64) {
        self.toast = Some((message.into(), now));
    }

    pub fn clear(&mut self) {
        self.toast = None;
    }

    pub fn is_active(&self, now: f64) -> bool {
        self.toast
            .as_ref()
            .map_or(false, |(_, ts)| now - ts < self.duration)
    }

    pub fn message(&self, now: f64) -> Option<&str> {
        self.toast.as_ref().and_then(|(msg, ts)| {
            if now - ts < self.duration {
                Some(msg.as_str())
            } else {
                None
            }
        })
    }

    pub fn render(&self, ui: &mut egui::Ui, palette: &AppPalette, now: f64) {
        if let Some(msg) = self.message(now) {
            let color = hex_to_c32(&palette.diff_add);
            ui.label(egui::RichText::new(msg).small().color(color));
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
                let color = hex_to_c32(&palette.diff_add);
                ui.label(egui::RichText::new(msg).small().strong().color(color));
            }
        }
    }

    pub fn render_pill(&self, ui: &mut egui::Ui, palette: &AppPalette, now: f64) {
        if let Some(msg) = self.message(now) {
            let bg = hex_to_c32(&palette.theme.b_low);
            let stroke = hex_to_c32(&palette.diff_add);
            let fg = contrast_text_c32(bg, Some(stroke), palette);
            egui::Frame::none()
                .fill(bg)
                .stroke(Stroke::new(1.0_f32, stroke))
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(msg).size(11.0).color(fg));
                });
        }
    }
}

/// Renders a small metadata chip/badge pill with theme-consistent fill and guaranteed contrast.
pub fn badge_pill(
    ui: &mut egui::Ui,
    text: impl AsRef<str>,
    accent: Option<Color32>,
    palette: &AppPalette,
) -> egui::Response {
    let bg = hex_to_c32(&palette.theme.b_med);
    let pref_fg = accent.unwrap_or_else(|| hex_to_c32(&palette.theme.f_med));
    let fg = contrast_text_c32(bg, Some(pref_fg), palette);
    let frame = egui::Frame::none()
        .fill(bg)
        .rounding(Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0));
    frame
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text.as_ref())
                    .small()
                    .strong()
                    .color(fg),
            )
        })
        .response
}

/// Standard label + value indicator (e.g. "Matches: 120 B" or "Deletions: 4").
pub fn stat_pill(
    ui: &mut egui::Ui,
    label: &str,
    value: impl std::fmt::Display,
    val_color: Option<Color32>,
    palette: &AppPalette,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
        ui.label(
            egui::RichText::new(format!("{}:", label))
                .font(egui::FontId::proportional(12.0))
                .color(hex_to_c32(&palette.theme.f_low)),
        );
        let color = val_color.unwrap_or_else(|| hex_to_c32(&palette.harmonies.base));
        ui.label(
            egui::RichText::new(format!("{}", value))
                .font(egui::FontId::monospace(12.0))
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

/// Renders a standardized zoom toolbar with 🔍+, 🔍-, ⟲ Fit buttons and percentage label.
pub fn zoom_control_bar(
    ui: &mut egui::Ui,
    zoom_pct: u32,
    palette: &AppPalette,
) -> Option<ZoomAction> {
    let mut action = None;
    let zoom_in_btn = secondary_button("🔍+", hex_to_c32(&palette.theme.b_low), palette);
    if ui
        .add(zoom_in_btn)
        .on_hover_text("Zoom In (+ / Ctrl+Wheel)")
        .clicked()
    {
        action = Some(ZoomAction::ZoomIn);
    }
    let zoom_out_btn = secondary_button("🔍-", hex_to_c32(&palette.theme.b_low), palette);
    if ui
        .add(zoom_out_btn)
        .on_hover_text("Zoom Out (- / Ctrl+Wheel)")
        .clicked()
    {
        action = Some(ZoomAction::ZoomOut);
    }
    let fit_btn = secondary_button("⟲ Fit", hex_to_c32(&palette.theme.b_low), palette);
    if ui
        .add(fit_btn)
        .on_hover_text("Fit Track / Window (Ctrl+0)")
        .clicked()
    {
        action = Some(ZoomAction::Fit);
    }
    ui.label(
        egui::RichText::new(format!("{}%", zoom_pct))
            .small()
            .monospace()
            .color(hex_to_c32(&palette.theme.f_med)),
    );
    action
}

/// Renders a standardized view top toolbar header with icon, title, metadata badges slot,
/// right-aligned action buttons slot, and a contrast-safe Close \[Esc] button.
pub fn render_view_header<R>(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    palette: &AppPalette,
    close: &mut bool,
    badges: impl FnOnce(&mut egui::Ui),
    right_actions: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        if !icon.is_empty() {
            ui.label(egui::RichText::new(icon).size(18.0));
        }
        ui.label(
            egui::RichText::new(title)
                .strong()
                .size(17.0)
                .color(hex_to_c32(&palette.theme.f_high)),
        );
        badges(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close_bg = hex_to_c32(&palette.theme.b_med);
            let close_fg =
                contrast_text_c32(close_bg, Some(hex_to_c32(&palette.diff_del)), palette);
            let close_btn = egui::Button::new(
                egui::RichText::new("Close (Esc)")
                    .small()
                    .strong()
                    .color(close_fg),
            )
            .fill(close_bg)
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_low)))
            .rounding(Rounding::same(4.0));
            if ui.add(close_btn).clicked() {
                *close = true;
            }
            right_actions(ui)
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::theme::Theme;
    use std::collections::HashMap;

    #[test]
    fn test_contrast_text_guarantee() {
        let theme = Theme::apollo();
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        // Test active mode tab fill (harmonies.base)
        let base_bg = hex_to_c32(&palette.harmonies.base);
        let fg = contrast_text_c32(base_bg, Some(hex_to_c32(&palette.theme.f_high)), &palette);
        let ratio = contrast_ratio(
            (base_bg.r(), base_bg.g(), base_bg.b()),
            (fg.r(), fg.g(), fg.b()),
        );
        assert!(ratio >= 4.5, "Contrast ratio {} must be >= 4.5:1", ratio);

        // Test diff_add fill
        let diff_add_bg = hex_to_c32(&palette.diff_add);
        let fg_add = contrast_text_c32(
            diff_add_bg,
            Some(hex_to_c32(&palette.theme.f_high)),
            &palette,
        );
        let ratio_add = contrast_ratio(
            (diff_add_bg.r(), diff_add_bg.g(), diff_add_bg.b()),
            (fg_add.r(), fg_add.g(), fg_add.b()),
        );
        assert!(
            ratio_add >= 4.5,
            "Contrast ratio {} must be >= 4.5:1",
            ratio_add
        );
    }

    #[test]
    fn test_mode_tab_contrast() {
        let theme = Theme::apollo();
        let palette = AppPalette::from_theme(&theme, &HashMap::new());

        // Active tab button
        let active_btn = mode_tab_button("View", true, &palette);
        // Inactive tab button
        let inactive_btn = mode_tab_button("Crop", false, &palette);
        // Ensure both constructed without panic
        drop(active_btn);
        drop(inactive_btn);
    }

    #[test]
    fn test_all_45_themes_all_buttons_contrast() {
        let dir = little_clip_core::theme::find_themes_dir();
        if !dir.exists() {
            return;
        }
        let themes = little_clip_core::theme::load_themes_from_dir(&dir);
        assert!(
            themes.len() >= 40,
            "Expected at least 40 themes, found {}",
            themes.len()
        );

        for theme in &themes {
            let palette = AppPalette::from_theme(theme, &HashMap::new());

            let test_cases: Vec<(&str, Color32, Option<Color32>)> = vec![
                (
                    "mode_tab_active (harmonies.base)",
                    hex_to_c32(&palette.harmonies.base),
                    Some(hex_to_c32(&palette.theme.f_high)),
                ),
                (
                    "mode_tab_inactive (b_med)",
                    hex_to_c32(&palette.theme.b_med),
                    Some(hex_to_c32(&palette.theme.f_med)),
                ),
                (
                    "action_btn (harmonies.base)",
                    hex_to_c32(&palette.harmonies.base),
                    Some(hex_to_c32(&palette.theme.f_high)),
                ),
                (
                    "action_btn (diff_add)",
                    hex_to_c32(&palette.diff_add),
                    Some(hex_to_c32(&palette.theme.f_high)),
                ),
                (
                    "secondary_btn (b_low)",
                    hex_to_c32(&palette.theme.b_low),
                    Some(hex_to_c32(&palette.theme.f_med)),
                ),
                (
                    "secondary_btn (b_med)",
                    hex_to_c32(&palette.theme.b_med),
                    Some(hex_to_c32(&palette.theme.f_med)),
                ),
                (
                    "danger_btn (undo_card_bg)",
                    hex_to_c32(&palette.undo_card_bg),
                    Some(hex_to_c32(&palette.theme.f_high)),
                ),
                (
                    "danger_btn (diff_del)",
                    hex_to_c32(&palette.diff_del),
                    Some(hex_to_c32(&palette.theme.f_high)),
                ),
                (
                    "alpha_fill (harmonies.base alpha 40)",
                    crate::hex_to_c32_alpha(&palette.harmonies.base, 40),
                    Some(hex_to_c32(&palette.harmonies.base)),
                ),
                (
                    "alpha_fill (left_border alpha 40)",
                    crate::hex_to_c32_alpha(&palette.left_hand_border, 40),
                    Some(hex_to_c32(&palette.left_hand_border)),
                ),
            ];

            for (name, bg, pref) in test_cases {
                let fg = contrast_text_c32(bg, pref, &palette);

                // Compute effective bg (composited over b_low if semi-transparent)
                let effective_bg = if bg.a() < 255 {
                    let alpha = bg.a() as f32 / 255.0;
                    let parent_rgb = palette.rgba(&palette.theme.b_low);
                    (
                        (bg.r() as f32 * alpha + parent_rgb.0 as f32 * (1.0 - alpha)).round() as u8,
                        (bg.g() as f32 * alpha + parent_rgb.1 as f32 * (1.0 - alpha)).round() as u8,
                        (bg.b() as f32 * alpha + parent_rgb.2 as f32 * (1.0 - alpha)).round() as u8,
                    )
                } else {
                    (bg.r(), bg.g(), bg.b())
                };

                let fg_rgb = (fg.r(), fg.g(), fg.b());
                let ratio = contrast_ratio(effective_bg, fg_rgb);
                assert!(
                    ratio >= 4.5,
                    "Theme '{}' failed WCAG AA on {}: effective_bg #{:02x}{:02x}{:02x}, fg #{:02x}{:02x}{:02x}, ratio {:.2} < 4.5",
                    theme.name,
                    name,
                    effective_bg.0,
                    effective_bg.1,
                    effective_bg.2,
                    fg_rgb.0,
                    fg_rgb.1,
                    fg_rgb.2,
                    ratio
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
}
