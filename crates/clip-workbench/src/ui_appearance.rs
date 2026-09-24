//! Shared Light/Dark appearance roles for workbench chrome.

use eframe::egui::{self, Color32};
use little_clip_core::config::WorkbenchAppearance;
use little_clip_core::theme::AppPalette;

use crate::hex_to_c32;
use crate::ui_ext::contrast_text_c32;

/// Semantic desktop roles. The legacy `AppPalette` bridge below keeps existing
/// viewers working while their controls move away from display-theme names.
#[derive(Debug, Clone, Copy)]
pub struct WorkbenchColors {
    pub canvas: Color32,
    pub panel: Color32,
    pub recessed: Color32,
    pub raised: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub focus: Color32,
    pub link: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
}

impl WorkbenchColors {
    pub fn from_palette(palette: &AppPalette) -> Self {
        Self {
            canvas: hex_to_c32(&palette.colors.canvas),
            panel: hex_to_c32(&palette.colors.surface_panel),
            recessed: hex_to_c32(&palette.colors.surface_recessed),
            raised: hex_to_c32(&palette.colors.surface_raised),
            text_primary: hex_to_c32(&palette.colors.text_primary),
            text_secondary: hex_to_c32(&palette.colors.text_secondary),
            text_muted: hex_to_c32(&palette.colors.text_muted),
            border: hex_to_c32(&palette.chrome_border),
            accent: hex_to_c32(&palette.left_hand_border),
            focus: hex_to_c32(&palette.cursor_border),
            link: hex_to_c32(&palette.url_color),
            danger: hex_to_c32(&palette.diff_del),
            success: hex_to_c32(&palette.diff_add),
            warning: hex_to_c32(&palette.harmonies.warm_attention),
        }
    }
}

pub fn palette(appearance: WorkbenchAppearance) -> AppPalette {
    AppPalette::for_appearance(appearance)
}

/// Apply desktop widget states from semantic workbench roles.
pub fn apply_visuals(ctx: &egui::Context, palette: &AppPalette) {
    let colors = WorkbenchColors::from_palette(palette);
    let mut visuals = if palette.is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.panel_fill = colors.canvas;
    visuals.window_fill = colors.panel;
    visuals.extreme_bg_color = colors.raised;

    visuals.widgets.noninteractive.bg_fill = colors.panel;
    visuals.widgets.noninteractive.fg_stroke.color =
        contrast_text_c32(colors.panel, Some(colors.text_primary), palette);
    visuals.widgets.noninteractive.bg_stroke.color = colors.border;

    visuals.widgets.inactive.bg_fill = colors.recessed;
    visuals.widgets.inactive.fg_stroke.color =
        contrast_text_c32(colors.recessed, Some(colors.text_secondary), palette);
    visuals.widgets.inactive.bg_stroke.color = colors.border;

    visuals.widgets.hovered.bg_fill = colors.panel;
    visuals.widgets.hovered.fg_stroke.color =
        contrast_text_c32(colors.panel, Some(colors.text_primary), palette);
    visuals.widgets.hovered.bg_stroke.color = colors.accent;

    visuals.widgets.active.bg_fill = colors.raised;
    visuals.widgets.active.fg_stroke.color =
        contrast_text_c32(colors.raised, Some(colors.text_primary), palette);
    visuals.widgets.active.bg_stroke.color = colors.focus;

    visuals.widgets.open.bg_fill = colors.raised;
    visuals.widgets.open.fg_stroke.color =
        contrast_text_c32(colors.raised, Some(colors.text_primary), palette);
    visuals.widgets.open.bg_stroke.color = colors.focus;

    visuals.selection.bg_fill = colors.accent;
    visuals.selection.stroke.color =
        contrast_text_c32(colors.accent, Some(colors.text_primary), palette);
    visuals.hyperlink_color = colors.link;
    visuals.warn_fg_color = colors.warning;
    visuals.error_fg_color = colors.danger;
    visuals.text_cursor.color = colors.focus;

    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::theme::contrast_ratio;

    #[test]
    fn text_roles_remain_readable_on_every_workbench_surface() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = palette(appearance);
            assert_eq!(palette.is_dark, appearance == WorkbenchAppearance::Dark);
            for surface in [
                &palette.colors.canvas,
                &palette.colors.surface_raised,
                &palette.colors.surface_panel,
                &palette.colors.surface_recessed,
            ] {
                let (r, g, b, _) = palette.rgba(surface);
                for foreground in [
                    &palette.colors.text_primary,
                    &palette.colors.text_secondary,
                    &palette.colors.text_muted,
                ] {
                    let (fr, fg, fb, _) = palette.rgba(foreground);
                    assert!(
                        contrast_ratio((r, g, b), (fr, fg, fb)) >= 4.5,
                        "{appearance:?} {foreground} on {surface} lacks contrast"
                    );
                }
            }
        }
    }

    #[test]
    fn widget_foregrounds_meet_contrast_on_their_actual_fills() {
        fn ratio(a: Color32, b: Color32) -> f32 {
            contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
        }

        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = palette(appearance);
            let colors = WorkbenchColors::from_palette(&palette);
            let ctx = egui::Context::default();
            apply_visuals(&ctx, &palette);
            let visuals = ctx.style().visuals.clone();
            assert!(ratio(colors.canvas, colors.text_primary) >= 4.5);
            assert!(ratio(colors.panel, colors.text_secondary) >= 4.5);
            assert!(ratio(colors.recessed, colors.text_muted) >= 4.5);
            assert!(ratio(colors.recessed, colors.accent) >= 3.0);
            assert!(ratio(colors.recessed, colors.border) >= 3.0);
            assert!(ratio(colors.panel, colors.border) >= 3.0);
            assert!(ratio(colors.panel, colors.link) >= 4.5);
            assert!(ratio(colors.panel, colors.accent) >= 3.0);
            for (name, foreground) in [
                ("warning", colors.warning),
                ("danger", colors.danger),
                ("success", colors.success),
            ] {
                let contrast = ratio(colors.panel, foreground);
                assert!(
                    contrast >= 4.5,
                    "{appearance:?} {name} text contrast is {contrast:.2}"
                );
            }
            assert_eq!(visuals.warn_fg_color, colors.warning);
            assert_eq!(visuals.error_fg_color, colors.danger);
            assert_eq!(visuals.text_cursor.color, colors.focus);
            for widget in [
                visuals.widgets.noninteractive,
                visuals.widgets.inactive,
                visuals.widgets.hovered,
                visuals.widgets.active,
                visuals.widgets.open,
            ] {
                assert!(
                    ratio(widget.bg_fill, widget.fg_stroke.color) >= 4.5,
                    "{appearance:?} widget foreground lacks contrast"
                );
            }
            assert!(ratio(visuals.selection.bg_fill, visuals.selection.stroke.color) >= 4.5);
        }
    }
}
