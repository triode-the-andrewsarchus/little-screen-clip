use crate::ScreenRenderer;
use little_clip_core::engineering::flowgraph::{Preview, State};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

impl ScreenRenderer {
    pub(super) fn draw_flowgraph_card(
        &self,
        pixmap: &mut Pixmap,
        preview: &Preview,
        [x, y, width, height]: [f32; 4],
    ) {
        let graph = &preview.graph;
        let left = x + 14.0;
        let right = x + width - 14.0;
        let top = y + 153.0;
        let view_height = (height - 170.0).max(30.0);
        let ink = self.color_from_hex(&self.palette.colors.text_primary);
        let title = if graph.title.is_empty() {
            graph.format.label()
        } else {
            &graph.title
        };
        let coverage = if preview.complete { "" } else { " · prefix" };
        let metadata = format!(
            "{} blocks · {} connections{coverage}",
            graph.blocks.len(),
            graph.connections.len()
        );
        self.draw_text_clipped(pixmap, title, left, y + 119.0, 15.0, ink, false, right);
        self.draw_text_clipped(pixmap, &metadata, left, y + 140.0, 13.0, ink, false, right);
        if self
            .draw_card_viewport_frame(pixmap, left, top, right - left, view_height)
            .is_none()
        {
            return;
        }

        let [[min_x, min_y], [max_x, max_y]] = preview.layout.bounds;
        let panel_width = (right - left - 18.0).max(1.0);
        let panel_height = (view_height - 20.0).max(1.0);
        let span_x = max_x - min_x;
        let span_y = max_y - min_y;
        let scale = (panel_width / span_x.max(1.0)).min(panel_height / span_y.max(1.0));
        let origin_x = left + 9.0 + (panel_width - scale * span_x) * 0.5;
        let origin_y = top + 10.0 + (panel_height - scale * span_y) * 0.5;
        let position = |idx: usize| {
            let [px, py] = preview.layout.positions[idx];
            (
                origin_x + (px - min_x) * scale,
                origin_y + (py - min_y) * scale,
            )
        };
        let mut wires = PathBuilder::new();
        for wire in &graph.connections {
            let (x0, y0) = position(wire.source.block);
            let (x1, y1) = position(wire.sink.block);
            wires.move_to(x0, y0);
            wires.line_to(x1, y1);
        }
        if let Some(wires) = wires.finish() {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(100, 174, 200, 194));
            pixmap.stroke_path(
                &wires,
                &paint,
                &Stroke {
                    width: 1.1,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
        let node_width = if graph.blocks.len() > 128 { 4.0 } else { 10.0 };
        for (index, block) in graph.blocks.iter().enumerate() {
            let (px, py) = position(index);
            let color = match block.state {
                State::Enabled => Color::from_rgba8(98, 204, 234, 255),
                State::Bypassed => Color::from_rgba8(230, 183, 110, 255),
                _ => Color::from_rgba8(124, 132, 148, 255),
            };
            let mut paint = Paint::default();
            paint.set_color(color);
            if let Some(node) = Rect::from_xywh(px - node_width * 0.5, py - 2.5, node_width, 5.0) {
                pixmap.fill_rect(node, &paint, Transform::identity(), None);
            }
        }
    }
}
