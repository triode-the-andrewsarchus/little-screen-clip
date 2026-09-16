//! Reusable controls shared by studio tool sidebars and drawers.

use crate::hex_to_c32;
use eframe::egui::{self, Pos2, Stroke, Vec2};
use little_clip_core::theme::AppPalette;

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
    let b_med = hex_to_c32(&palette.theme.b_med);
    let base_c = hex_to_c32(&palette.harmonies.base);
    let f_med = hex_to_c32(&palette.theme.f_med);
    let f_low = hex_to_c32(&palette.theme.f_low);

    ui.vertical(|ui| {
        ui.label(egui::RichText::new(label).small().color(f_med));

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

        let response = ui.add(slider);
        if response.changed() {
            *value = cur_val;
            changed = true;
        }

        if response.double_clicked() {
            *value = default_val;
            changed = true;
        }

        let def_display = if suffix == "%" {
            format!("{:.0}%", default_val * 100.0)
        } else if suffix.is_empty() {
            format!("{:.2}", default_val)
        } else {
            format!("{:.2}{}", default_val, suffix)
        };
        response.on_hover_text(format!("Double-click to reset (default: {})", def_display));

        // Background context track bar beneath slider
        let min_val = *range.start();
        let max_val = *range.end();
        let span = (max_val - min_val).max(1e-5);

        let (track_rect, _) =
            ui.allocate_exact_size(Vec2::new(slider_width, 6.0), egui::Sense::hover());
        let painter = ui.painter();

        let y_bar = track_rect.center().y;
        let x_start = track_rect.min.x;
        let x_end = track_rect.max.x;
        let x_mid = (x_start + x_end) * 0.5;

        // Baseline track
        painter.line_segment(
            [Pos2::new(x_start, y_bar), Pos2::new(x_end, y_bar)],
            Stroke::new(1.0, b_med),
        );

        // Start tick (min)
        painter.line_segment(
            [
                Pos2::new(x_start, y_bar - 2.5),
                Pos2::new(x_start, y_bar + 2.5),
            ],
            Stroke::new(1.0, f_low),
        );

        // End tick (max)
        painter.line_segment(
            [Pos2::new(x_end, y_bar - 2.5), Pos2::new(x_end, y_bar + 2.5)],
            Stroke::new(1.0, f_low),
        );

        // Center tick (midpoint of range)
        painter.line_segment(
            [Pos2::new(x_mid, y_bar - 2.0), Pos2::new(x_mid, y_bar + 2.0)],
            Stroke::new(1.0, f_low),
        );

        // Default tick (in theme accent)
        let def_norm = ((default_val - min_val) / span).clamp(0.0, 1.0);
        let x_def = x_start + def_norm * (x_end - x_start);
        painter.line_segment(
            [Pos2::new(x_def, y_bar - 3.5), Pos2::new(x_def, y_bar + 3.5)],
            Stroke::new(1.5, base_c),
        );

        // Gauge fill line from default to current value
        let cur_norm = ((*value - min_val) / span).clamp(0.0, 1.0);
        let x_cur = x_start + cur_norm * (x_end - x_start);

        let (g_start, g_end) = if x_cur >= x_def {
            (x_def, x_cur)
        } else {
            (x_cur, x_def)
        };
        if (g_end - g_start).abs() > 0.5 {
            painter.line_segment(
                [Pos2::new(g_start, y_bar), Pos2::new(g_end, y_bar)],
                Stroke::new(2.0, base_c),
            );
        }

        // Value indicator dot
        painter.circle_filled(Pos2::new(x_cur, y_bar), 2.5, base_c);
    });

    changed
}
