use crate::{ScreenRenderer, file_preview::Iq};
use little_clip_core::engineering::waveforms::display_value;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

impl ScreenRenderer {
    pub(super) fn draw_iq_card(
        &self,
        pixmap: &mut Pixmap,
        iq: &Iq,
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 154.0;
        let height = (height - 171.0).max(25.0);
        let rate = iq
            .rate
            .map(|rate| display_value(rate, "Sa/s"))
            .unwrap_or_else(|| "Sample rate unknown".to_owned());
        let duration = iq
            .duration
            .map(|duration| display_value(duration, "s"))
            .unwrap_or_else(|| display_value(iq.power.samples.end as f64, "samples"));
        let mut detail = iq.center.map_or_else(
            || "I/Q power".to_owned(),
            |center| format!("I/Q power · {}", display_value(center, "Hz")),
        );
        if iq.power.bins.iter().any(|bin| !bin.exact()) {
            detail.push_str(" · sampled");
        }
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        self.draw_text_clipped(
            pixmap,
            &format!("{rate} · {duration}"),
            left,
            y + 121.0,
            15.0,
            ink,
            false,
            right,
        );
        self.draw_text_clipped(pixmap, &detail, left, y + 141.0, 13.0, ink, false, right);
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, height)
            .is_none()
        {
            return;
        }
        let min = iq
            .power
            .bins
            .iter()
            .filter_map(|bin| bin.minimum_dbfs)
            .filter(|v| v.is_finite())
            .min_by(f32::total_cmp)
            .unwrap_or(-120.0);
        let max = iq
            .power
            .bins
            .iter()
            .filter_map(|bin| bin.peak_dbfs)
            .filter(|v| v.is_finite())
            .max_by(f32::total_cmp)
            .unwrap_or(0.0);
        let scale = (max - min).max(1.0);
        let step = (right - left - 8.0) / iq.power.bins.len().max(1) as f32;
        let map = |power: f32| top + 4.0 + (max - power) / scale * (height - 8.0);
        let mut trace = PathBuilder::new();
        let mut mean = PathBuilder::new();
        let mut continuous = false;
        for (index, bin) in iq.power.bins.iter().enumerate() {
            let (Some(low), Some(high), Some(average)) =
                (bin.minimum_dbfs, bin.peak_dbfs, bin.mean_dbfs)
            else {
                continuous = false;
                continue;
            };
            if !low.is_finite() || !high.is_finite() || !average.is_finite() {
                continuous = false;
                continue;
            }
            let x = left + 4.0 + (index as f32 + 0.5) * step;
            trace.move_to(x, map(high));
            trace.line_to(x, map(low).max(map(high) + 0.5));
            if continuous {
                mean.line_to(x, map(average));
            } else {
                mean.move_to(x, map(average));
            }
            continuous = true;
        }
        for (path, color) in [
            (trace, Color::from_rgba8(72, 172, 210, 170)),
            (mean, Color::from_rgba8(102, 215, 244, 255)),
        ] {
            if let Some(path) = path.finish() {
                let mut paint = Paint::default();
                paint.set_color(color);
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
        }
    }
}
