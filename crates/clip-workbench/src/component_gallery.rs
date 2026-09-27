use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::BODY_FONT_SIZE;
use crate::ui_ext::{
    self, action_button, badge_pill, mode_tab_button, secondary_button, themed_button,
};
use eframe::egui::{self, Color32, RichText, Rounding, Stroke, Vec2};
use little_clip_core::palette::AppPalette;

/// Interactive samples of shared workbench components for visual review.
pub struct ComponentGalleryState {
    query: String,
    notes: String,
    level: f32,
    selected_mode: usize,
    enabled: bool,
}

impl Default for ComponentGalleryState {
    fn default() -> Self {
        Self {
            query: "Sample text".to_owned(),
            notes: "Multiline fields should remain comfortable to read.".to_owned(),
            level: 0.62,
            selected_mode: 0,
            enabled: true,
        }
    }
}

impl ComponentGalleryState {
    pub fn render_ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        let colors = WorkbenchColors::from_palette(palette);
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Frame::none()
                .fill(colors.panel)
                .rounding(Rounding::same(ui_ext::PANEL_CORNER_RADIUS))
                .inner_margin(ui_ext::PANEL_INSET)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.heading("Component Gallery");
                        ui.add_space(ui_ext::SPACE_SM);
                        badge_pill(ui, "STYLE CHECK", Some(colors.accent), palette);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(secondary_button("Close (Esc)", colors.recessed, palette))
                                .clicked()
                            {
                                *close = true;
                            }
                            ui.label(
                                RichText::new("F6 switches Light / Dark")
                                    .size(BODY_FONT_SIZE)
                                    .color(colors.text_muted),
                            );
                        });
                    });
                    ui.label(
                        RichText::new("Interactive samples of the shared workbench controls")
                            .color(colors.text_secondary),
                    );
                });

            ui.add_space(ui_ext::SPACE_MD);
            ui.columns(2, |columns| {
                render_section(&mut columns[0], "Buttons and modes", palette, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(action_button("Primary action", colors.accent, palette));
                        ui.add(secondary_button("Secondary", colors.recessed, palette));
                        ui.add(themed_button("Destructive", colors.danger, None, palette));
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled(
                            false,
                            secondary_button("Disabled", colors.recessed, palette),
                        );
                        ui.add(secondary_button(
                            "Ordinary control",
                            colors.recessed,
                            palette,
                        ));
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (index, label) in ["Normal", "Alternate", "Expert"].iter().enumerate() {
                            if ui
                                .add(mode_tab_button(
                                    *label,
                                    self.selected_mode == index,
                                    palette,
                                ))
                                .clicked()
                            {
                                self.selected_mode = index;
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        badge_pill(ui, "Ready", Some(colors.success), palette);
                        badge_pill(ui, "Working", Some(colors.warning), palette);
                        badge_pill(ui, "Unavailable", Some(colors.recessed), palette);
                    });
                });

                render_section(&mut columns[1], "Fields and choices", palette, |ui| {
                    ui.label("Single-line field");
                    ui.add_sized(
                        Vec2::new(ui.available_width(), ui_ext::CONTROL_HEIGHT),
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("Type to preview field styling"),
                    );
                    ui.label("Multiline field");
                    ui.add_sized(
                        Vec2::new(ui.available_width(), 76.0),
                        egui::TextEdit::multiline(&mut self.notes),
                    );
                    ui.checkbox(&mut self.enabled, "Enabled setting");
                    ui.horizontal_wrapped(|ui| {
                        let mut choice = self.selected_mode == 1;
                        if ui.radio_value(&mut choice, true, "Radio choice").clicked() && choice {
                            self.selected_mode = 1;
                        }
                        ui.label("Choice controls use a shared readable baseline.");
                    });
                    ui.add(egui::Slider::new(&mut self.level, 0.0..=1.0).text("Level"));
                    ui.add(egui::Slider::new(&mut self.level, 0.0..=1.0).show_value(false));
                });

                render_section(&mut columns[0], "Bars and status", palette, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Audio preview").strong());
                        ui.add(
                            egui::ProgressBar::new(self.level)
                                .desired_width(220.0)
                                .text(format!("{:.0}%", self.level * 100.0)),
                        );
                    });
                    status_sample(ui, "Success", "Changes saved", colors.success, palette);
                    status_sample(
                        ui,
                        "Notice",
                        "Waiting for a source",
                        colors.warning,
                        palette,
                    );
                    status_sample(
                        ui,
                        "Error",
                        "Could not load preview",
                        colors.danger,
                        palette,
                    );
                });

                render_section(&mut columns[1], "Panels and toolbar", palette, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("IMAGE STUDIO").strong());
                        ui.label(RichText::new("— sample.webp").color(colors.text_muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add(secondary_button("Close", colors.recessed, palette));
                            ui.add(action_button("Save", colors.accent, palette));
                        });
                    });
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        for label in ["View", "Crop", "Markup", "Resize", "Convert"] {
                            ui.add(secondary_button(label, colors.recessed, palette));
                        }
                    });
                    egui::Frame::none()
                        .fill(colors.recessed)
                        .rounding(Rounding::same(ui_ext::CARD_CORNER_RADIUS))
                        .stroke(Stroke::new(1.0_f32, colors.border))
                        .inner_margin(ui_ext::SPACE_SM)
                        .show(ui, |ui| {
                            ui.label(RichText::new("Nested card").strong());
                            ui.label("Panel hierarchy, spacing, and edge contrast sample.");
                        });
                });
            });

            ui.add_space(ui_ext::SPACE_MD);
            ui.label(
                RichText::new("Esc closes this page. F6 previews the other appearance.")
                    .color(colors.text_muted),
            );
        });
    }
}

fn render_section(
    ui: &mut egui::Ui,
    title: &str,
    palette: &AppPalette,
    contents: impl FnOnce(&mut egui::Ui),
) {
    let colors = WorkbenchColors::from_palette(palette);
    egui::Frame::none()
        .fill(colors.panel)
        .rounding(Rounding::same(ui_ext::PANEL_CORNER_RADIUS))
        .stroke(Stroke::new(1.0_f32, colors.border))
        .inner_margin(ui_ext::PANEL_INSET)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading(title);
            ui.add_space(ui_ext::SPACE_SM);
            contents(ui);
        });
    ui.add_space(ui_ext::SPACE_MD);
}

fn status_sample(
    ui: &mut egui::Ui,
    label: &str,
    message: &str,
    accent: Color32,
    palette: &AppPalette,
) {
    let colors = WorkbenchColors::from_palette(palette);
    egui::Frame::none()
        .fill(colors.recessed)
        .rounding(Rounding::same(ui_ext::CONTROL_CORNER_RADIUS))
        .stroke(Stroke::new(1.0_f32, accent))
        .inner_margin(Vec2::splat(ui_ext::SPACE_SM))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).strong().color(accent));
                ui.label(message);
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_appearance;
    use little_clip_core::config::WorkbenchAppearance;

    #[test]
    fn gallery_renders_in_light_and_dark_at_compact_and_wide_sizes() {
        for appearance in [WorkbenchAppearance::Light, WorkbenchAppearance::Dark] {
            for size in [egui::vec2(900.0, 700.0), egui::vec2(1760.0, 1000.0)] {
                let ctx = egui::Context::default();
                ui_ext::configure_workbench_style(&ctx);
                let palette = ui_appearance::palette(appearance);
                ui_appearance::apply_visuals(&ctx, &palette);
                let mut gallery = ComponentGalleryState::default();
                let mut close = false;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            gallery.render_ui(ui, &palette, &mut close);
                        });
                    },
                );
                assert!(
                    output.shapes.len() > 30,
                    "gallery rendered only {} shapes at {size:?}",
                    output.shapes.len()
                );
                assert!(!close);
            }
        }
    }
}
