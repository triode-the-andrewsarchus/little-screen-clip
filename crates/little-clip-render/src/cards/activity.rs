use crate::ScreenRenderer;
use little_clip_core::engineering::{
    packets::{Kind, Summary},
    waveforms::display_value,
};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

impl ScreenRenderer {
    pub(super) fn draw_packet_activity(
        &self,
        pixmap: &mut Pixmap,
        capture: &Summary,
        bounds: [f32; 4],
    ) {
        let format = match capture.format.kind {
            Kind::Pcap => "PCAP",
            Kind::PcapNg => "PCAPNG",
        };
        let coverage = if capture.complete { "" } else { "First " };
        let activity = capture.activity();
        let time = activity
            .first()
            .zip(activity.last())
            .map(|(first, last)| display_value((last.end_ns - first.start_ns) as f64 * 1e-9, "s"));
        let count = format!("{coverage}{} packets", capture.packets.len());
        let info = format!(
            "{count} · {}{}",
            capture.interfaces.len(),
            if capture.interfaces.len() == 1 {
                " interface"
            } else {
                " interfaces"
            },
        );
        let heading = if let Some(time) = time {
            format!("{format} · {time}")
        } else {
            format.to_owned()
        };
        let bins = activity
            .iter()
            .map(|bin| [bin.packets, 0])
            .collect::<Vec<_>>();
        self.draw_activity(pixmap, [&heading, &info], &bins, bounds);
    }

    pub(super) fn draw_can_activity(
        &self,
        pixmap: &mut Pixmap,
        capture: &little_clip_core::engineering::can::Summary,
        bounds: [f32; 4],
    ) {
        let time = capture
            .time_bounds_ns
            .map(|[start, end]| display_value((end - start) as f64 * 1e-9, "s"));
        let heading = time.map_or_else(
            || capture.format.label().to_owned(),
            |t| format!("{} · {t}", capture.format.label()),
        );
        let coverage = if capture.complete { "" } else { "First " };
        let info = format!(
            "{coverage}{} frames · {} errors",
            capture.frames, capture.errors
        );
        let bins = capture
            .activity
            .iter()
            .map(|bin| {
                [
                    bin.frames.saturating_sub(bin.error_frames),
                    bin.error_frames,
                ]
            })
            .collect::<Vec<_>>();
        self.draw_activity(pixmap, [&heading, &info], &bins, bounds);
    }

    pub(super) fn draw_activity(
        &self,
        pixmap: &mut Pixmap,
        lines: [&str; 2],
        bins: &[[u64; 2]],
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 154.0;
        let plot_height = (height - 171.0).max(26.0);
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        for (line, shift) in lines.into_iter().zip([120.0, 141.0]) {
            self.draw_text_clipped(pixmap, line, left, y + shift, 15.0, ink, false, right);
        }
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, plot_height)
            .is_none()
        {
            return;
        }
        let max = bins
            .iter()
            .map(|&[ok, extra]| ok.saturating_add(extra))
            .max()
            .unwrap_or(1)
            .max(1);
        let step = (right - left - 8.0) / bins.len().max(1) as f32;
        let bottom = top + plot_height - 4.0;
        let colors = [
            Color::from_rgba8(79, 203, 218, 255),
            Color::from_rgba8(243, 138, 117, 255),
        ];
        for (index, values) in bins.iter().enumerate() {
            let x = left + 4.0 + index as f32 * step;
            let mut bottom = bottom;
            for (&value, color) in values.iter().zip(colors) {
                if value == 0 {
                    continue;
                }
                let height = (plot_height - 8.0) * value as f32 / max as f32;
                if let Some(rect) =
                    Rect::from_xywh(x, bottom - height, (step - 0.65).max(0.5), height)
                {
                    let mut paint = Paint::default();
                    paint.set_color(color);
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
                bottom -= height;
            }
        }
    }
}
