//! Reusable controls shared by studio tool sidebars and drawers.

use crate::ui_appearance::WorkbenchColors;
use eframe::egui::{self, Pos2, Rect, Stroke, Vec2};
use little_clip_core::palette::AppPalette;

#[allow(clippy::too_many_arguments)]
pub fn render_context_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default_val: f32,
    suffix: &str,
    slider_width: f32,
    palette: &AppPalette,
) -> bool {
    let mut changed = false;
    let colors = WorkbenchColors::from_palette(palette);

    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).color(colors.text_secondary));

        ui.spacing_mut().slider_width = slider_width;
        let mut cur_val = *value;
        let slider = egui::Slider::new(&mut cur_val, range.clone())
            .show_value(true)
            .min_decimals(2)
            .max_decimals(2)
            .custom_formatter(|n, _| {
                if suffix.is_empty() {
                    format!("{:.2}", n)
                } else if suffix == "%" {
                    format!("{:.0}%", n * 100.0)
                } else {
                    format!("{:.2}{}", n, suffix)
                }
            });

        let slider_response = ui.add(slider);
        if slider_response.changed() {
            *value = cur_val;
            changed = true;
        }

        let def_display = if suffix == "%" {
            format!("{:.0}%", default_val * 100.0)
        } else if suffix.is_empty() {
            format!("{:.2}", default_val)
        } else {
            format!("{:.2}{}", default_val, suffix)
        };
        let slider_response = slider_response
            .on_hover_text(format!("Double-click to reset (default: {})", def_display));

        let track_rect = render_slider_track(
            ui,
            *value,
            range.clone(),
            default_val,
            slider_width,
            palette,
        );

        let interactive_rect = slider_response.rect.union(track_rect);
        if slider_double_clicked_in(ui, &slider_response, interactive_rect) {
            *value = default_val;
            changed = true;
        }
    });

    changed
}

/// Returns true if the primary mouse button was double-clicked over the slider.
pub fn slider_double_clicked(ui: &egui::Ui, slider_response: &egui::Response) -> bool {
    slider_double_clicked_in(ui, slider_response, slider_response.rect)
}

/// Returns true if the primary mouse button was double-clicked over the given interactive rect,
/// taking into account pointer position, clipping, occlusion, and avoiding resets during text editing.
pub fn slider_double_clicked_in(
    ui: &egui::Ui,
    slider_response: &egui::Response,
    interactive_rect: Rect,
) -> bool {
    if !ui.input(|i| {
        i.pointer
            .button_double_clicked(egui::PointerButton::Primary)
    }) {
        return false;
    }
    if slider_response.has_focus() {
        return false;
    }
    ui.rect_contains_pointer(interactive_rect.expand(2.0)) || slider_response.hovered()
}

/// Draw the range baseline, endpoint and default ticks, and current value below any slider.
/// The caller owns the interactive slider so this works in compact headers and full panels.
pub fn render_slider_track(
    ui: &mut egui::Ui,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    default_val: f32,
    slider_width: f32,
    palette: &AppPalette,
) -> Rect {
    let (track_rect, _) =
        ui.allocate_exact_size(Vec2::new(slider_width, 6.0), egui::Sense::hover());
    paint_slider_track(
        ui.painter(),
        track_rect,
        value,
        range,
        default_val,
        Some(default_val),
        2.5,
        palette,
    );
    track_rect
}

/// A compact interactive slider whose thumb and fill share the same track geometry.
/// Its fixed allocation prevents neighboring header controls from moving at the endpoints.
pub fn render_track_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    width: f32,
    palette: &AppPalette,
) -> egui::Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(width, 24.0), egui::Sense::click_and_drag());
    let track = Rect::from_min_max(
        Pos2::new(rect.min.x + 8.0, rect.center().y - 3.0),
        Pos2::new(rect.max.x - 8.0, rect.center().y + 3.0),
    );
    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let next = track_value_at_x(pointer.x, track, &range);
        if (*value - next).abs() > f32::EPSILON {
            *value = next;
            response.mark_changed();
        }
    }
    if response.has_focus() {
        let next = ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                Some(*range.start())
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::End) {
                Some(*range.end())
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                Some(*value - 1.0)
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                Some(*value + 1.0)
            } else {
                None
            }
        });
        if let Some(next) = next {
            *value = next.clamp(*range.start(), *range.end());
            response.mark_changed();
        }
    }
    paint_slider_track(
        ui.painter(),
        track,
        *value,
        range.clone(),
        *range.start(),
        None,
        5.0,
        palette,
    );
    response
}

fn track_value_at_x(x: f32, track: Rect, range: &std::ops::RangeInclusive<f32>) -> f32 {
    let fraction = ((x - track.min.x) / track.width().max(1.0)).clamp(0.0, 1.0);
    *range.start() + fraction * (*range.end() - *range.start())
}

fn paint_slider_track(
    painter: &egui::Painter,
    track_rect: Rect,
    value: f32,
    range: std::ops::RangeInclusive<f32>,
    fill_origin: f32,
    default_val: Option<f32>,
    thumb_radius: f32,
    palette: &AppPalette,
) {
    let colors = WorkbenchColors::from_palette(palette);
    let b_med = colors.panel;
    let base_c = colors.accent;
    let f_low = colors.text_muted;
    let min_val = *range.start();
    let max_val = *range.end();
    let span = (max_val - min_val).max(1e-5);

    let y_bar = track_rect.center().y;
    let x_start = track_rect.min.x;
    let x_end = track_rect.max.x;
    let x_mid = (x_start + x_end) * 0.5;

    // Baseline track
    painter.line_segment(
        [Pos2::new(x_start, y_bar), Pos2::new(x_end, y_bar)],
        Stroke::new(1.0_f32, b_med),
    );

    // Start tick (min)
    painter.line_segment(
        [
            Pos2::new(x_start, y_bar - 2.5),
            Pos2::new(x_start, y_bar + 2.5),
        ],
        Stroke::new(1.0_f32, f_low),
    );

    // End tick (max)
    painter.line_segment(
        [Pos2::new(x_end, y_bar - 2.5), Pos2::new(x_end, y_bar + 2.5)],
        Stroke::new(1.0_f32, f_low),
    );

    // Center tick (midpoint of range)
    painter.line_segment(
        [Pos2::new(x_mid, y_bar - 2.0), Pos2::new(x_mid, y_bar + 2.0)],
        Stroke::new(1.0_f32, f_low),
    );

    if let Some(default_val) = default_val {
        let def_norm = ((default_val - min_val) / span).clamp(0.0, 1.0);
        let x_def = x_start + def_norm * (x_end - x_start);
        painter.line_segment(
            [Pos2::new(x_def, y_bar - 3.5), Pos2::new(x_def, y_bar + 3.5)],
            Stroke::new(1.5_f32, base_c),
        );
    }

    // Gauge fill line from default to current value
    let cur_norm = ((value - min_val) / span).clamp(0.0, 1.0);
    let x_cur = x_start + cur_norm * (x_end - x_start);

    let origin_norm = ((fill_origin - min_val) / span).clamp(0.0, 1.0);
    let x_origin = x_start + origin_norm * (x_end - x_start);
    let (g_start, g_end) = (x_origin.min(x_cur), x_origin.max(x_cur));
    if (g_end - g_start).abs() > 0.5 {
        painter.line_segment(
            [Pos2::new(g_start, y_bar), Pos2::new(g_end, y_bar)],
            Stroke::new(2.5_f32, base_c),
        );
    }

    // Value indicator dot
    painter.circle_filled(Pos2::new(x_cur, y_bar), thumb_radius, base_c);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_slider_uses_the_painted_track_endpoints() {
        let track = Rect::from_min_max(Pos2::new(8.0, 0.0), Pos2::new(108.0, 6.0));
        assert_eq!(track_value_at_x(8.0, track, &(0.0..=100.0)), 0.0);
        assert_eq!(track_value_at_x(108.0, track, &(0.0..=100.0)), 100.0);
        assert_eq!(track_value_at_x(58.0, track, &(0.0..=100.0)), 50.0);
        assert_eq!(track_value_at_x(120.0, track, &(0.0..=100.0)), 100.0);
    }

    #[test]
    fn slider_double_click_resets_render_context_slider_to_default() {
        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let mut val = 0.85f32;
        let default_val = 0.0f32;

        // Pass 1: Initial layout
        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)));
        let _ = ctx.run(raw_input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        assert_eq!(val, 0.85);

        // Click 1 in the slider row below the full-size label.
        let click_pos = Pos2::new(50.0, 27.0);
        let mut click1_down = raw_input.clone();
        click1_down.time = Some(0.01);
        click1_down.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click1_down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        let mut click1_up = raw_input.clone();
        click1_up.time = Some(0.05);
        click1_up.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click1_up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        // Value changed by first click, not reset yet
        assert_ne!(val, 0.85);

        // Click 2 at (50.0, 20.0) within double-click threshold (e.g. 0.15s later)
        let mut click2_down = raw_input.clone();
        click2_down.time = Some(0.15);
        click2_down.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click2_down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        let mut click2_up = raw_input.clone();
        click2_up.time = Some(0.20);
        click2_up.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut changed_on_double_click = false;
        let _ = ctx.run(click2_up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                changed_on_double_click = render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        // Double click must have reset the value to default_val!
        assert!(changed_on_double_click);
        assert_eq!(val, default_val);
    }

    #[test]
    fn single_click_on_slider_does_not_reset_to_default() {
        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let mut val = 0.85f32;
        let default_val = 0.0f32;

        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)));

        // Pass 1: Initial layout
        let _ = ctx.run(raw_input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        // Single Click at (50.0, 20.0)
        let click_pos = Pos2::new(50.0, 20.0);
        let mut click_down = raw_input.clone();
        click_down.time = Some(0.01);
        click_down.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click_down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        let mut click_up = raw_input.clone();
        click_up.time = Some(0.05);
        click_up.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click_up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_context_slider(
                    ui,
                    "Exposure",
                    &mut val,
                    -1.0..=1.0,
                    default_val,
                    "",
                    120.0,
                    &palette,
                );
            });
        });

        // After a single click, value should NOT be default_val
        assert_ne!(val, default_val);
    }

    #[test]
    fn slider_double_clicked_helper_resets_standalone_slider() {
        let ctx = egui::Context::default();
        let mut seed = 500;
        let default_seed = 100;

        let mut raw_input = egui::RawInput::default();
        raw_input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0)));

        // Pass 1: Initial layout
        let _ = ctx.run(raw_input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let resp = ui.add(egui::Slider::new(&mut seed, 0..=1000));
                let is_dc = slider_double_clicked(ui, &resp);
                if is_dc {
                    seed = default_seed;
                }
            });
        });

        assert_eq!(seed, 500);

        // Click 1
        let click_pos = Pos2::new(50.0, 10.0);
        let mut click1_down = raw_input.clone();
        click1_down.time = Some(0.01);
        click1_down.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click1_down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let resp = ui.add(egui::Slider::new(&mut seed, 0..=1000));
                let is_dc = slider_double_clicked(ui, &resp);
                if is_dc {
                    seed = default_seed;
                }
            });
        });

        let mut click1_up = raw_input.clone();
        click1_up.time = Some(0.05);
        click1_up.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click1_up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let resp = ui.add(egui::Slider::new(&mut seed, 0..=1000));
                let is_dc = slider_double_clicked(ui, &resp);
                if is_dc {
                    seed = default_seed;
                }
            });
        });

        // Click 2
        let mut click2_down = raw_input.clone();
        click2_down.time = Some(0.15);
        click2_down.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click2_down, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let resp = ui.add(egui::Slider::new(&mut seed, 0..=1000));
                let is_dc = slider_double_clicked(ui, &resp);
                if is_dc {
                    seed = default_seed;
                }
            });
        });

        let mut click2_up = raw_input.clone();
        click2_up.time = Some(0.20);
        click2_up.events.push(egui::Event::PointerButton {
            pos: click_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(click2_up, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let resp = ui.add(egui::Slider::new(&mut seed, 0..=1000));
                let is_dc = slider_double_clicked(ui, &resp);
                if is_dc {
                    seed = default_seed;
                }
            });
        });

        assert_eq!(seed, default_seed);
    }
}
