//! Semantic action buttons with centrally chosen surfaces and contrast.
//!
//! Use these roles for ordinary application actions. Navigation belongs to
//! [`crate::ui_tabs::TabBar`]; parameter choices and toggles remain distinct.
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::{self, StyledButton};
use little_clip_core::palette::AppPalette;

/// Selects an action's emphasis without exposing per-call color choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonRole {
    /// Emphasizes the main affirmative action in a group.
    Primary,
    /// Presents an ordinary action, including Cancel, Reset and navigation commands.
    Secondary,
    /// Identifies an action that deletes or replaces user content.
    Destructive,
}

impl ButtonRole {
    /// Creates a button using shared height, contrast and interaction behavior.
    ///
    /// # Examples
    ///
    /// ```
    /// use clip_ui::buttons::ButtonRole;
    /// let palette = little_clip_core::palette::AppPalette::default();
    /// let _save = ButtonRole::Primary.button("Save", &palette);
    /// ```
    pub fn button(self, label: impl Into<String>, palette: &AppPalette) -> StyledButton<'static> {
        self.button_with_icon(None, label, palette)
    }

    /// Creates a button with an optional embedded SVG icon.
    ///
    /// The caller retains enabled state, tooltip, sizing and action handling.
    pub fn button_with_icon(
        self,
        icon: Option<Icon>,
        label: impl Into<String>,
        palette: &AppPalette,
    ) -> StyledButton<'static> {
        let colors = WorkbenchColors::from_palette(palette);
        match self {
            Self::Primary => ui_ext::action_button_icon(icon, label, colors.accent, palette),
            Self::Secondary => ui_ext::secondary_button_icon(icon, label, colors.raised, palette),
            Self::Destructive => ui_ext::danger_button_icon(icon, label, colors.danger, palette),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_ext::{CONTROL_HEIGHT, configure_workbench_style};
    use little_clip_core::config::WorkbenchAppearance;

    #[test]
    fn shared_buttons_preserve_native_padding_and_compact_height() {
        let palette = AppPalette::default();
        let ctx = egui::Context::default();
        configure_workbench_style(&ctx);
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let native = ui.add(egui::Button::new("Action")).rect.size();
                        let shared = ui
                            .add(ButtonRole::Secondary.button("Action", &palette))
                            .rect
                            .size();
                        assert!(
                            (native - shared).length() < 0.001,
                            "standard padding changed: {native:?} / {shared:?}"
                        );
                        let native_small = ui.add(egui::Button::new("Action").small()).rect.size();
                        let shared_small = ui
                            .add(ButtonRole::Secondary.button("Action", &palette).small())
                            .rect
                            .size();
                        assert!(
                            (native_small - shared_small).length() < 0.001,
                            "compact padding changed: {native_small:?} / {shared_small:?}"
                        );
                        assert!(shared_small.y < CONTROL_HEIGHT);
                        let compact =
                            ui.add(ButtonRole::Secondary.button("Action", &palette).compact());
                        assert_eq!(compact.rect.height(), crate::ui_ext::COMPACT_CONTROL_HEIGHT);
                    });
                });
            });
        }
    }

    #[test]
    fn semantic_buttons_keep_native_keyboard_activation_and_disabled_behavior() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            for role in [
                ButtonRole::Primary,
                ButtonRole::Secondary,
                ButtonRole::Destructive,
            ] {
                for enabled in [true, false] {
                    let palette = AppPalette::for_appearance(appearance);
                    let ctx = egui::Context::default();
                    configure_workbench_style(&ctx);
                    crate::ui_appearance::apply_visuals(&ctx, &palette);
                    for frame in 0..3 {
                        let mut clicked = false;
                        let mut input = egui::RawInput::default();
                        if frame == 2 {
                            input.events.push(egui::Event::Key {
                                key: egui::Key::Enter,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: egui::Modifiers::NONE,
                            });
                        }
                        let _ = ctx.run(input, |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let response = ui.add_enabled(
                                    enabled,
                                    role.button_with_icon(Some(Icon::Copy), "Action", &palette),
                                );
                                assert_eq!(response.enabled(), enabled);
                                assert!(response.rect.height() >= CONTROL_HEIGHT);
                                if frame == 1 && enabled {
                                    response.request_focus();
                                }
                                clicked = response.clicked();
                            });
                        });
                        assert_eq!(clicked, frame == 2 && enabled, "{appearance:?} {role:?}");
                    }
                }
            }
        }
    }
}
