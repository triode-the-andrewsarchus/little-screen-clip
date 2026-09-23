//! Desktop appearance palettes. Hardware display themes remain in the renderer.

use little_clip_core::config::WorkbenchAppearance;
use little_clip_core::theme::{AppPalette, Theme};
use std::collections::HashMap;

pub fn palette(appearance: WorkbenchAppearance) -> AppPalette {
    let (background, f_high, f_med, f_low, f_inv, b_high, b_med, b_low, b_inv) = match appearance {
        WorkbenchAppearance::Dark => (
            "#151c24", "#f5f2e9", "#d6dce0", "#aab6c1", "#111923", "#243440", "#273541", "#1e2a35",
            "#ffb385",
        ),
        WorkbenchAppearance::Light => (
            "#f4f2ea", "#17242f", "#45505b", "#586977", "#fffdf7", "#fffaf1", "#e3e7e7", "#e9edeb",
            "#a44a37",
        ),
    };
    let theme = Theme {
        name: format!("workbench-{appearance:?}").to_lowercase(),
        background: background.into(),
        f_high: f_high.into(),
        f_med: f_med.into(),
        f_low: f_low.into(),
        f_inv: f_inv.into(),
        b_high: b_high.into(),
        b_med: b_med.into(),
        b_low: b_low.into(),
        b_inv: b_inv.into(),
    };
    AppPalette::from_theme(&theme, &HashMap::new())
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
                &palette.theme.background,
                &palette.theme.b_high,
                &palette.theme.b_med,
                &palette.theme.b_low,
            ] {
                let (r, g, b, _) = palette.rgba(surface);
                for foreground in [
                    &palette.theme.f_high,
                    &palette.theme.f_med,
                    &palette.theme.f_low,
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
}
