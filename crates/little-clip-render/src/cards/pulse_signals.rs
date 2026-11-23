use crate::ScreenRenderer;
use little_clip_core::engineering::{pulses::Capture, waveforms::display_value};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

impl ScreenRenderer {
    pub(super) fn draw_pulse_signals(
        &self,
        pixmap: &mut Pixmap,
        capture: &Capture,
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 150.0;
        let height = (height - 167.0).max(26.0);
        let title = capture.format.title();
        let total = capture.signals.len();
        let coverage = if capture.complete { "" } else { " · prefix" };
        let subtitle = if total == 1 {
            let signal = &capture.signals[0];
            if signal.pulses.is_empty() {
                format!("{title}{coverage}")
            } else {
                format!("{title} · {} pulses{coverage}", signal.pulses.len())
            }
        } else {
            format!("{title} · {total} signals{coverage}")
        };
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        self.draw_text_clipped(pixmap, &subtitle, left, y + 126.0, 15.0, ink, false, right);
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, height)
            .is_none()
        {
            return;
        }
        let shown = total.min(4);
        let lane = height / shown as f32;
        let split = left + (width * 0.42).clamp(58.0, 128.0);
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(239, 187, 98, 255));
        for (index, signal) in capture.signals.iter().take(shown).enumerate() {
            let center = top + (index as f32 + 0.5) * lane;
            self.draw_text_clipped(
                pixmap,
                signal.label().as_ref(),
                left + 6.0,
                center - 7.0,
                13.0,
                ink,
                true,
                split - 3.0,
            );
            if signal.pulses.is_empty() {
                if let Some(protocol) = &signal.protocol {
                    self.draw_text_clipped(
                        pixmap,
                        protocol,
                        split + 3.0,
                        center - 7.0,
                        12.0,
                        ink,
                        false,
                        right - 4.0,
                    );
                }
                continue;
            }
            let samples = &signal.pulses;
            let stride = samples.len().div_ceil(256);
            let length = signal.duration_us().max(1) as f32;
            let t = |us: u64| split + us as f32 / length * (right - split - 5.0);
            let mut path = PathBuilder::new();
            let offset = lane * 0.24;
            for segment in samples.chunks(stride) {
                let first = segment[0];
                let last = segment[segment.len() - 1];
                if segment.iter().any(|sample| sample.high != first.high) {
                    path.move_to(t(first.start_us), center - offset);
                    path.line_to(t(first.start_us), center + offset);
                } else {
                    let y = center + if first.high { -offset } else { offset };
                    path.move_to(t(first.start_us), y);
                    path.line_to(t(last.end_us()), y);
                }
            }
            if let Some(path) = path.finish() {
                pixmap.stroke_path(
                    &path,
                    &paint,
                    &Stroke {
                        width: 1.25,
                        ..Default::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
            if shown == 1 {
                let duration = display_value(length as f64 * 1e-6, "s");
                self.draw_text_clipped(
                    pixmap,
                    &duration,
                    split + 3.0,
                    top + height - 18.0,
                    12.0,
                    ink,
                    false,
                    right - 4.0,
                );
            }
        }
    }
}
