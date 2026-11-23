use super::rate_overlay::Overlay;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};
use little_clip_core::engineering::flowgraph::{Connection, Direction, Graph, Layout, State};

const NODE: Vec2 = Vec2::new(178.0, 52.0);
const OVERVIEW_EDGES: usize = 700;
const OVERVIEW_NODES: usize = 425;
const DETAIL_EDGES: usize = 4400;
const DETAIL_NODES: usize = 3000;

#[derive(Clone, Copy)]
pub(super) struct Camera {
    center: Option<Pos2>,
    zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: None,
            zoom: 1.0,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Transform {
    world: Pos2,
    screen: Pos2,
    scale: f32,
}

pub(super) struct Scene<'a> {
    pub graph: &'a Graph,
    pub layout: &'a Layout,
}

#[derive(Default)]
pub(super) struct Highlights<'a> {
    pub selected: Option<usize>,
    pub traced_blocks: &'a [bool],
    pub traced_links: &'a [bool],
    pub search: &'a [bool],
    pub rates: Option<&'a Overlay>,
}

impl Camera {
    pub(super) fn projection(self, rect: Rect, layout: &Layout) -> Transform {
        let min = pos2(layout.bounds[0][0], layout.bounds[0][1]);
        let max = pos2(layout.bounds[1][0], layout.bounds[1][1]) + NODE;
        let bounds = Rect::from_min_max(min, max);
        let margin = (rect.width().min(rect.height()) / 5.0).clamp(0.0, 10.0);
        let view = rect.shrink(margin);
        let size = bounds.size();
        let fit = (view.width() / size.x)
            .min(view.height() / size.y)
            .max(0.000_001);
        Transform {
            world: self.center.unwrap_or(bounds.center()),
            screen: view.center(),
            scale: (fit * self.zoom).min(4.0),
        }
    }

    pub(super) fn reveal(&mut self, rect: Rect, layout: &Layout, block: usize) {
        let Some(&[x, y]) = layout.positions.get(block) else {
            return;
        };
        let target = pos2(x, y) + NODE * 0.5;
        let projection = self.projection(rect, layout);
        if !rect.shrink(32.0).contains(projection.point(target)) || projection.scale < 0.56 {
            self.center = Some(target);
            self.zoom = (self.zoom * (0.75 / projection.scale).max(1.0)).clamp(0.30, 16384.0);
        }
    }

    pub(super) fn pan(&mut self, projection: Transform, delta: Vec2) {
        self.center = Some(projection.world - delta / projection.scale);
    }

    pub(super) fn zoom_at(&mut self, rect: Rect, layout: &Layout, pointer: Pos2, delta: f32) {
        let projection = self.projection(rect, layout);
        let world = projection.unproject(pointer);
        self.zoom = (self.zoom * (delta * 0.003).clamp(-1.0, 1.0).exp()).clamp(0.30, 16384.0);
        let next = self.projection(rect, layout);
        self.center = Some(world - (pointer - next.screen) / next.scale);
    }
}

impl Transform {
    fn point(self, world: Pos2) -> Pos2 {
        self.screen + (world - self.world) * self.scale
    }

    fn unproject(self, screen: Pos2) -> Pos2 {
        self.world + (screen - self.screen) / self.scale
    }

    fn block(self, [x, y]: [f32; 2]) -> Rect {
        Rect::from_min_size(
            self.point(pos2(x, y)),
            vec2(
                (NODE.x * self.scale).max(3.0),
                (NODE.y * self.scale).max(2.0),
            ),
        )
    }
}

impl Scene<'_> {
    pub(super) fn draw(
        &self,
        painter: &Painter,
        rect: Rect,
        transform: Transform,
        highlighted: &Highlights<'_>,
        colors: WorkbenchColors,
        overview: bool,
    ) {
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 4.0, colors.recessed);
        let edge_limit = if overview {
            OVERVIEW_EDGES
        } else {
            DETAIL_EDGES
        };
        let node_limit = if overview {
            OVERVIEW_NODES
        } else {
            DETAIL_NODES
        };
        let node_stride = self.layout.positions.len().div_ceil(node_limit).max(1);
        let edge_stride = self.graph.connections.len().div_ceil(edge_limit).max(1);
        self.edges(&painter, rect, transform, highlighted, colors, edge_stride);
        for (index, position) in self.layout.positions.iter().copied().enumerate() {
            let active = highlighted.selected == Some(index)
                || marked(highlighted.traced_blocks, index)
                || marked(highlighted.search, index);
            if !active && !index.is_multiple_of(node_stride) && transform.scale < 0.35 {
                continue;
            }
            let node = transform.block(position);
            if rect.intersects(node) {
                self.node(&painter, node, index, highlighted, colors, overview);
            }
        }
    }

    fn edges(
        &self,
        painter: &Painter,
        rect: Rect,
        transform: Transform,
        highlighted: &Highlights<'_>,
        colors: WorkbenchColors,
        stride: usize,
    ) {
        for (index, link) in self.graph.connections.iter().enumerate() {
            let active = marked(highlighted.traced_links, index)
                || highlighted
                    .selected
                    .is_some_and(|block| link.source.block == block || link.sink.block == block);
            if !active && !index.is_multiple_of(stride) && transform.scale < 0.35 {
                continue;
            }
            let [source, sink] = self.ports(link, transform);
            let extent = Rect::from_two_pos(source, sink).expand(32.0);
            if !extent.intersects(rect) {
                continue;
            }
            let annotation = highlighted.rates.and_then(|rates| rates.wire(index));
            let rate_color = annotation.and_then(|label| label.color(colors));
            let color = if let Some(color) = rate_color {
                color
            } else if active {
                colors.link
            } else {
                colors.text_muted.gamma_multiply(0.64)
            };
            let stroke = Stroke::new(if active { 1.7 } else { 1.0 }, color);
            if transform.scale < 0.30 {
                painter.line_segment([source, sink], stroke);
                continue;
            }
            let tangent = ((sink.x - source.x).abs() * 0.47).clamp(7.0, 90.0);
            let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                [
                    source,
                    source + vec2(tangent, 0.0),
                    sink - vec2(tangent, 0.0),
                    sink,
                ],
                false,
                egui::Color32::TRANSPARENT,
                stroke,
            );
            painter.add(shape);
            painter.line_segment([sink - vec2(4.0, 2.5), sink], stroke);
            painter.line_segment([sink - vec2(4.0, -2.5), sink], stroke);
            if let Some(label) = annotation
                && (active || self.graph.connections.len() <= 16)
            {
                label.paint(painter, source, sink, color);
            }
        }
    }

    fn ports(&self, wire: &Connection, transform: Transform) -> [Pos2; 2] {
        let [sx, sy] = self.layout.positions[wire.source.block];
        let [tx, ty] = self.layout.positions[wire.sink.block];
        let offset = |port: &str| match port.parse::<u16>() {
            Ok(number) => (f32::from(number.min(5)) + 1.0) * NODE.y / 7.0,
            _ => NODE.y * 0.72,
        };
        [
            transform.point(pos2(sx + NODE.x, sy + offset(&wire.source.port))),
            transform.point(pos2(tx, ty + offset(&wire.sink.port))),
        ]
    }

    fn node(
        &self,
        painter: &Painter,
        rect: Rect,
        index: usize,
        highlighted: &Highlights<'_>,
        colors: WorkbenchColors,
        overview: bool,
    ) {
        let block = &self.graph.blocks[index];
        let selected = highlighted.selected == Some(index);
        let color = if selected {
            colors.focus
        } else if marked(highlighted.traced_blocks, index) {
            colors.link
        } else if marked(highlighted.search, index) {
            colors.warning
        } else if block.state == State::Disabled {
            colors.text_muted
        } else if self.graph.adjacent(index, Direction::Upstream).is_empty() {
            colors.success
        } else {
            colors.accent
        };
        if rect.height() < 8.0 {
            painter.rect_filled(rect, 1.0, color);
            return;
        }
        let fill = if selected {
            colors.panel
        } else {
            colors.raised
        };
        painter.rect_filled(rect, 4.0, fill);
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(if selected { 2.0 } else { 1.0 }, color),
            StrokeKind::Inside,
        );
        if overview || rect.width() < 63.0 || rect.height() < 17.0 {
            return;
        }
        let painter = painter.with_clip_rect(rect.shrink(3.0));
        let scale = rect.width() / NODE.x;
        let fonts = (
            (scale * 12.0).clamp(9.0, 17.0),
            (scale * 10.0).clamp(8.0, 13.0),
        );
        painter.text(
            rect.center() + vec2(0.0, -rect.height() * 0.19),
            Align2::CENTER_CENTER,
            &block.name,
            FontId::proportional(fonts.0),
            colors.text_primary,
        );
        painter.text(
            rect.center() + vec2(0.0, rect.height() * 0.21),
            Align2::CENTER_CENTER,
            &block.kind,
            FontId::monospace(fonts.1),
            colors.text_muted,
        );
    }

    pub(super) fn pick(&self, transform: Transform, pointer: Pos2) -> Option<usize> {
        self.layout
            .positions
            .iter()
            .copied()
            .enumerate()
            .rev()
            .find_map(|(index, position)| {
                transform
                    .block(position)
                    .expand(2.0)
                    .contains(pointer)
                    .then_some(index)
            })
    }
}

fn marked(items: &[bool], index: usize) -> bool {
    items.get(index).copied().unwrap_or(false)
}
