use crate::ScreenRenderer;
use little_clip_core::engineering::{logic_preview::Overview, waveforms::display_value};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

impl ScreenRenderer {
    pub(super) fn draw_logic_capture(
        &self,
        pixmap: &mut Pixmap,
        capture: &Overview,
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 150.0;
        let height = (height - 164.0).max(26.0);
        let span = capture.span.end - capture.span.start;
        let sampled = if capture.sampled() { " · sampled" } else { "" };
        let title = format!(
            "{} · {} ch · {}{sampled}",
            capture.label,
            capture.lanes.len(),
            display_value(span, "s")
        );
        self.draw_text_clipped(
            pixmap,
            &title,
            left,
            y + 126.0,
            15.0,
            self.color_from_hex(&self.palette.colors.text_primary),
            false,
            right,
        );
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, height)
            .is_none()
        {
            return;
        }
        let shown = capture.lanes_previewed.len().min(4);
        let lane_height = height / shown.max(1) as f32;
        let chart_left = left + (width * 0.24).clamp(38.0, 80.0);
        let color = Color::from_rgba8(130, 225, 163, 255);
        let mut paint = Paint::default();
        paint.set_color(color);
        for (index, envelope) in capture.lanes_previewed.iter().take(shown).enumerate() {
            let center = top + (index as f32 + 0.5) * lane_height;
            self.draw_text_clipped(
                pixmap,
                &capture.lanes[index].name,
                left + 4.0,
                center - 6.0,
                12.0,
                color,
                false,
                chart_left - 4.0,
            );
            let high = center - lane_height * 0.26;
            let low = center + lane_height * 0.26;
            let mut trace = PathBuilder::new();
            let step =
                (envelope.time.end - envelope.time.start) / envelope.bins.len().max(1) as f64;
            let x_at = |seconds: f64| {
                chart_left
                    + ((seconds - capture.span.start) / span.max(f64::MIN_POSITIVE)) as f32
                        * (right - chart_left - 4.0)
            };
            for (column, bin) in envelope.bins.iter().enumerate() {
                let time = envelope.time.start + column as f64 * step;
                let start = x_at(time);
                let end = x_at(time + step).max(start + 0.4);
                for (visible, y) in [
                    (bin.high, high),
                    (bin.low, low),
                    (bin.unknown || bin.high_impedance, center),
                ] {
                    if visible {
                        trace.move_to(start, y);
                        trace.line_to(end, y);
                    }
                }
                if bin.low && bin.high {
                    trace.move_to(start, high);
                    trace.line_to(start, low);
                }
            }
            if let Some(trace) = trace.finish() {
                pixmap.stroke_path(
                    &trace,
                    &paint,
                    &Stroke {
                        width: 1.2,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}
