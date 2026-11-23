use crate::ScreenRenderer;
use little_clip_core::engineering::tags::{Direction, Document, Memory, RowRole, SignalPreview};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

impl ScreenRenderer {
    pub(super) fn draw_tags_card(&self, pixmap: &mut Pixmap, document: &Document, area: [f32; 4]) {
        match document {
            Document::Memory(memory) => self.draw_tag_memory(pixmap, memory, area),
            Document::Trace(trace) => {
                let prefix = if trace.complete { "" } else { "First " };
                let title = format!("{prefix}{} frames · Reader and tag", trace.frames.len());
                let width = trace.frames.len().div_ceil(96).max(1);
                let bins = trace
                    .frames
                    .chunks(width)
                    .map(|frames| {
                        let readers = frames
                            .iter()
                            .filter(|frame| frame.direction == Direction::Reader)
                            .count() as u64;
                        [readers, frames.len() as u64 - readers]
                    })
                    .collect::<Vec<_>>();
                self.draw_activity(pixmap, ["Proxmark trace", &title], &bins, area);
            }
            Document::Signal(signal) => self.draw_tag_signal(pixmap, signal, area),
        }
    }

    fn draw_tag_memory(
        &self,
        pixmap: &mut Pixmap,
        memory: &Memory,
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 153.0;
        let height = (height - 169.0).max(24.0);
        let [known, total] = memory.coverage();
        let detail = if let Some(check) = memory.rom_check {
            format!(
                "{} · CRC {}",
                memory.protocol,
                if check.valid() { "valid" } else { "mismatch" }
            )
        } else {
            format!("{} · {known}/{total} bytes", memory.protocol)
        };
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        self.draw_text_clipped(
            pixmap,
            memory.format.title(),
            left,
            y + 121.0,
            15.0,
            ink,
            false,
            right,
        );
        self.draw_text_clipped(pixmap, &detail, left, y + 141.0, 14.0, ink, false, right);
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, height)
            .is_none()
        {
            return;
        }
        let stride = memory.rows.len().div_ceil(128).max(1);
        let count = memory.rows.len().div_ceil(stride);
        let columns = count.clamp(1, 16);
        let rows = count.div_ceil(columns).max(1);
        let cell_width = (right - left - 12.0) / columns as f32;
        let cell_height = (height - 12.0) / rows as f32;
        for (index, group) in memory.rows.chunks(stride).enumerate() {
            let complete = group.iter().all(|row| row.known_bytes() == row.bytes.len());
            let trailer = group.iter().any(|row| row.role == RowRole::SectorTrailer);
            let color = match (complete, trailer) {
                (true, true) => Color::from_rgba8(242, 184, 102, 255),
                (true, false) => Color::from_rgba8(99, 206, 197, 255),
                (false, _) => Color::from_rgba8(110, 118, 134, 255),
            };
            let px = left + 6.0 + (index % columns) as f32 * cell_width;
            let py = top + 6.0 + (index / columns) as f32 * cell_height;
            if let Some(rect) = Rect::from_xywh(
                px,
                py,
                (cell_width - 2.0).max(1.0),
                (cell_height - 2.0).max(1.0),
            ) {
                let mut paint = Paint::default();
                paint.set_color(color);
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
    }

    fn draw_tag_signal(
        &self,
        pixmap: &mut Pixmap,
        signal: &SignalPreview,
        [x, y, width, height]: [f32; 4],
    ) {
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 154.0;
        let height = (height - 170.0).max(26.0);
        let prefix = if signal.complete { "" } else { "First " };
        let label = format!("{prefix}{} GraphBuffer samples", signal.samples.len());
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        self.draw_text_clipped(pixmap, &label, left, y + 128.0, 15.0, ink, false, right);
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, height)
            .is_none()
        {
            return;
        }
        let Some(low) = signal.samples.iter().copied().min() else {
            return;
        };
        let high = signal.samples.iter().copied().max().unwrap_or(low);
        let range = (f32::from(high) - f32::from(low)).max(1.0);
        let n = signal.samples.len().div_ceil(192).max(1);
        let step = (right - left - 8.0) / signal.samples.len().div_ceil(n).max(1) as f32;
        let map = |sample: i16| {
            top + 4.0 + (f32::from(high) - f32::from(sample)) / range * (height - 8.0)
        };
        let mut path = PathBuilder::new();
        for (index, chunk) in signal.samples.chunks(n).enumerate() {
            let x = left + 4.0 + (index as f32 + 0.5) * step;
            let upper = map(chunk.iter().copied().max().unwrap_or(high));
            let lower = map(chunk.iter().copied().min().unwrap_or(low));
            path.move_to(x, upper);
            path.line_to(x, lower.max(upper + 0.7));
        }
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(99, 206, 197, 255));
            pixmap.stroke_path(
                &path,
                &paint,
                &Stroke {
                    width: 1.4,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }
}
