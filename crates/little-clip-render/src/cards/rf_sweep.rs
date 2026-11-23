//! Compact view of the first measured trace in a Touchstone sweep.

use crate::ScreenRenderer;
use little_clip_core::engineering::{
    touchstone::{Parameter, Summary},
    waveforms::{Peak, display_value},
};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

impl ScreenRenderer {
    pub(super) fn draw_rf_sweep_card(
        &self,
        pixmap: &mut Pixmap,
        sweep: &Summary,
        [x, y, width, height]: [f32; 4],
    ) {
        let first = sweep.points[0].frequency_hz;
        let last = sweep.points[sweep.points.len() - 1].frequency_hz;
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 154.0;
        let plot_height = (y + height - top - 19.0).max(26.0);
        let trace = sweep.metadata.parameter_label(0, 0).unwrap_or_default();
        let decibels = sweep.metadata.parameter == Parameter::S;
        let mode = if decibels { "dB" } else { "Magnitude" };
        let completeness = if sweep.complete {
            ""
        } else {
            " · first points"
        };
        let ports = sweep.metadata.ports;
        let noun = if ports == 1 { "port" } else { "ports" };
        let label = format!("{ports} {noun} · {trace} {mode}{completeness}");
        let frequency = format!(
            "{} to {}",
            display_value(first, "Hz"),
            display_value(last, "Hz")
        );
        for (line, offset) in [(label, 121.0), (frequency, 141.0)] {
            self.draw_text_clipped(
                pixmap,
                &line,
                left,
                y + offset,
                15.0,
                self.color_from_hex(&self.palette.colors.text_primary),
                false,
                right,
            );
        }
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, plot_height)
            .is_none()
        {
            return;
        }
        let points = sweep.points.iter().filter_map(|point| {
            let magnitude = point.value(0, 0)?.norm();
            let value = if decibels {
                20.0 * magnitude.max(1e-12).log10()
            } else {
                magnitude
            };
            Some((point.frequency_hz, value as f32))
        });
        let mut bounds = Peak::default();
        for (_, value) in points.clone() {
            bounds.add(&[value]);
        }
        if !bounds.valid() {
            return;
        }
        let x_span = (last - first).max(f64::MIN_POSITIVE);
        let y_span = (bounds.max - bounds.min).max(f32::MIN_POSITIVE);
        let mut path = PathBuilder::new();
        for (index, (frequency, magnitude)) in points.enumerate() {
            let px = left + 4.0 + ((frequency - first) / x_span) as f32 * (right - left - 8.0);
            let ratio = if bounds.min == bounds.max {
                0.5
            } else {
                (magnitude - bounds.min) / y_span
            };
            let py = top + 4.0 + (1.0 - ratio) * (plot_height - 8.0);
            if index == 0 {
                path.move_to(px, py);
                path.line_to(px + 0.5, py);
            } else {
                path.line_to(px, py);
            }
        }
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(102, 207, 252, 255));
            pixmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: 1.6,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
}
