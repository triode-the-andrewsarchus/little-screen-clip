use egui::{Color32, Mesh, Rect, Stroke, Ui, pos2, vec2};
use little_clip_core::geography::Point;

const COLORS: [[u8; 3]; 5] = [
    [65, 112, 248],
    [30, 192, 227],
    [64, 207, 128],
    [245, 215, 60],
    [247, 69, 56],
];

#[derive(Clone, Copy)]
pub(super) struct DoseScale(pub(super) [f64; 2]);

impl DoseScale {
    pub(super) fn color(self, dose: f64) -> Color32 {
        let [low, high] = self.0;
        let fraction = if low == high {
            0.5
        } else {
            ((dose - low) / (high - low)).clamp(0.0, 1.0)
        };
        let position = fraction * (COLORS.len() - 1) as f64;
        let index = (position as usize).min(COLORS.len() - 2);
        let mix = |channel: usize| {
            let left = f64::from(COLORS[index][channel]);
            let right = f64::from(COLORS[index + 1][channel]);
            (left + (right - left) * (position - index as f64)).round() as u8
        };
        Color32::from_rgb(mix(0), mix(1), mix(2))
    }

    pub(super) fn segment(self, a: Option<f64>, b: Option<f64>, fallback: Stroke) -> Stroke {
        let color = a
            .zip(b)
            .map_or(fallback.color, |(a, b)| self.color(a.midpoint(b)));
        Stroke::new(fallback.width, color)
    }

    pub(super) fn legend(self, ui: &mut Ui) {
        let [low, high] = self.0;
        ui.horizontal(|ui| {
            ui.label("Dose rate");
            ui.label(format_rate(low, "µSv/h"));
            let width = (ui.available_width() - 100.0).clamp(28.0, 260.0);
            let (rect, _) = ui.allocate_exact_size(vec2(width, 10.0), egui::Sense::hover());
            ui.painter().add(self.ramp(rect));
            if low != high {
                ui.label(format_rate(high, "µSv/h"));
            }
        });
    }

    fn ramp(self, rect: Rect) -> Mesh {
        let mut mesh = Mesh::default();
        for i in 0..COLORS.len() {
            let fraction = i as f64 / (COLORS.len() - 1) as f64;
            let x = egui::lerp(rect.x_range(), fraction as f32);
            let [low, high] = self.0;
            let color = self.color(low + (high - low) * fraction);
            mesh.colored_vertex(pos2(x, rect.top()), color);
            mesh.colored_vertex(pos2(x, rect.bottom()), color);
            if i > 0 {
                let n = 2 * i as u32;
                mesh.add_triangle(n - 2, n - 1, n);
                mesh.add_triangle(n - 1, n + 1, n);
            }
        }
        mesh
    }
}

pub(super) fn dose_rate(point: &Point) -> Option<f64> {
    point.radiation.and_then(|reading| reading.dose_rate_usv_h)
}

pub(super) fn show_reading(ui: &mut Ui, point: &Point) {
    let Some(reading) = point.radiation else {
        return;
    };
    if let Some(value) = reading.dose_rate_usv_h {
        ui.label(format!("Dose rate {}", format_rate(value, "µSv/h")));
    } else {
        ui.label("Dose rate unavailable");
    }
    if let Some(value) = reading.count_rate_cps {
        ui.label(format!("Count rate {}", format_rate(value, "cps")));
    }
    if let Some(value) = reading.accuracy_m {
        ui.label(format!("GPS accuracy {}", format_rate(value, "m")));
    }
}

fn format_rate(value: f64, unit: &str) -> String {
    if value != 0.0 && !(0.001..10_000.0).contains(&value) {
        format!("{value:.2e} {unit}")
    } else {
        format!("{value:.3} {unit}")
    }
}
