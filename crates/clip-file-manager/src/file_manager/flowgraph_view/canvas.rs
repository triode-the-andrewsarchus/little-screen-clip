use super::{Selection, rate_overlay::Overlay};
use crate::ui_appearance::WorkbenchColors;
use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2,
    epaint::CubicBezierShape, pos2, vec2,
};
use little_clip_core::engineering::flowgraph::{
    BLOCK_SIZE, Connection, Direction, Graph, Layout, State,
};

const NODE: Vec2 = Vec2::new(BLOCK_SIZE[0], BLOCK_SIZE[1]);
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
    pub selected: Option<Selection>,
    pub hovered_wire: Option<usize>,
    pub traced_blocks: &'a [bool],
    pub traced_links: &'a [bool],
    pub search: &'a [bool],
    pub rates: Option<&'a Overlay>,
}

impl Highlights<'_> {
    fn wire_active(&self, edge: usize, link: &Connection) -> bool {
        marked(self.traced_links, edge)
            || match self.selected {
                Some(Selection::Block(block)) => {
                    link.source.block == block || link.sink.block == block
                }
                Some(Selection::Wire(wire)) => wire == edge,
                None => false,
            }
    }
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

    pub(super) fn reveal_wire(&mut self, rect: Rect, graph: &Graph, layout: &Layout, index: usize) {
        let link = &graph.connections[index];
        let [sx, sy] = layout.positions[link.source.block];
        let [tx, ty] = layout.positions[link.sink.block];
        let world = Rect::from_min_max(
            pos2(sx.min(tx), sy.min(ty)),
            pos2(sx.max(tx), sy.max(ty)) + NODE,
        );
        let projection = self.projection(rect, layout);
        let view = rect.shrink(16.0);
        let shown = Rect::from_min_max(projection.point(world.min), projection.point(world.max));
        if view.contains_rect(shown) && projection.scale >= 0.56 {
            return;
        }
        let scale = (view.width() / world.width())
            .min(view.height() / world.height())
            .max(0.000_001);
        let desired = projection.scale.max(0.75).min(scale);
        self.zoom = (self.zoom * desired / projection.scale).clamp(0.30, 16384.0);
        self.center = Some(world.center());
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
    fn wire_endpoints(&self, selected: Option<Selection>) -> Option<[usize; 2]> {
        let Some(Selection::Wire(edge)) = selected else {
            return None;
        };
        let link = self.graph.connections.get(edge)?;
        Some([link.source.block, link.sink.block])
    }

    pub(super) fn pick_wire(
        &self,
        transform: Transform,
        pointer: Pos2,
        highlights: &Highlights<'_>,
    ) -> Option<usize> {
        const RADIUS: f32 = 7.0;
        let mut nearest = None;
        let mut best = RADIUS * RADIUS;
        for (index, path) in self.visible_wires(transform, highlights, DETAIL_EDGES) {
            if path.bounds().expand(RADIUS).contains(pointer) {
                let distance = path.distance_squared(pointer);
                if distance < best {
                    nearest = Some(index);
                    best = distance;
                }
            }
        }
        nearest
    }

    fn visible_wires<'a>(
        &'a self,
        transform: Transform,
        highlights: &'a Highlights<'a>,
        limit: usize,
    ) -> impl Iterator<Item = (usize, WirePath)> + 'a {
        let stride = self.graph.connections.len().div_ceil(limit).max(1);
        self.graph
            .connections
            .iter()
            .enumerate()
            .filter_map(move |(index, link)| {
                if transform.scale < 0.35
                    && !index.is_multiple_of(stride)
                    && !highlights.wire_active(index, link)
                {
                    return None;
                }
                let [source, sink] =
                    self.layout.wire_anchors[index].map(|[x, y]| transform.point(pos2(x, y)));
                Some((
                    index,
                    WirePath {
                        source,
                        sink,
                        cubic: transform.scale >= 0.30,
                    },
                ))
            })
    }

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
        self.edges(&painter, rect, transform, highlighted, colors, edge_limit);
        let endpoints = self.wire_endpoints(highlighted.selected);
        for (index, position) in self.layout.positions.iter().copied().enumerate() {
            let active = highlighted.selected == Some(Selection::Block(index))
                || endpoints.is_some_and(|ports| ports.contains(&index))
                || marked(highlighted.traced_blocks, index)
                || marked(highlighted.search, index);
            if !active && !index.is_multiple_of(node_stride) && transform.scale < 0.35 {
                continue;
            }
            let node = transform.block(position);
            if rect.intersects(node) {
                let endpoint = endpoints.is_some_and(|ports| ports.contains(&index));
                self.node(
                    &painter,
                    node,
                    index,
                    highlighted,
                    colors,
                    overview,
                    endpoint,
                );
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
        limit: usize,
    ) {
        for (index, path) in self.visible_wires(transform, highlighted, limit) {
            if !path.bounds().expand(6.0).intersects(rect) {
                continue;
            }
            let active = highlighted.wire_active(index, &self.graph.connections[index]);
            let selected = highlighted.selected == Some(Selection::Wire(index));
            let hovered = highlighted.hovered_wire == Some(index);
            let annotation = highlighted.rates.and_then(|rates| rates.wire(index));
            let color = if selected {
                colors.focus
            } else if hovered {
                colors.link
            } else if let Some(rate) = annotation.and_then(|label| label.color(colors)) {
                rate
            } else if active {
                colors.link
            } else {
                colors.text_muted.gamma_multiply(0.64)
            };
            let width = if selected || hovered {
                2.4
            } else if active {
                1.7
            } else {
                1.0
            };
            path.paint(painter, Stroke::new(width, color));
            if selected {
                painter.circle_filled(path.source, 3.0, color);
                painter.circle_filled(path.sink, 3.0, color);
            }
            if let Some(label) = annotation
                && path.cubic
                && (active || self.graph.connections.len() <= 16)
            {
                label.paint(painter, path.source, path.sink, color);
            }
        }
    }

    fn node(
        &self,
        painter: &Painter,
        rect: Rect,
        index: usize,
        highlighted: &Highlights<'_>,
        colors: WorkbenchColors,
        overview: bool,
        endpoint: bool,
    ) {
        let block = &self.graph.blocks[index];
        let selected = highlighted.selected == Some(Selection::Block(index));
        let color = if selected {
            colors.focus
        } else if endpoint || marked(highlighted.traced_blocks, index) {
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

#[derive(Clone, Copy)]
struct WirePath {
    source: Pos2,
    sink: Pos2,
    cubic: bool,
}

impl WirePath {
    fn tangent(self) -> f32 {
        ((self.sink.x - self.source.x).abs() * 0.47).clamp(7.0, 90.0)
    }

    fn curve(self, stroke: Stroke) -> CubicBezierShape {
        let tangent = self.tangent();
        CubicBezierShape::from_points_stroke(
            [
                self.source,
                self.source + vec2(tangent, 0.0),
                self.sink - vec2(tangent, 0.0),
                self.sink,
            ],
            false,
            Color32::TRANSPARENT,
            stroke,
        )
    }

    fn bounds(self) -> Rect {
        let rect = Rect::from_two_pos(self.source, self.sink);
        if self.cubic {
            rect.expand2(vec2(self.tangent(), 0.0))
        } else {
            rect
        }
    }

    fn distance_squared(self, pointer: Pos2) -> f32 {
        if !self.cubic {
            return segment_distance_squared(pointer, self.source, self.sink);
        }
        let mut previous = self.source;
        let mut best = f32::INFINITY;
        self.curve(Stroke::NONE)
            .for_each_flattened_with_t(0.65, &mut |point, _| {
                best = best.min(segment_distance_squared(pointer, previous, point));
                previous = point;
            });
        best
    }

    fn paint(self, painter: &Painter, stroke: Stroke) {
        if self.cubic {
            painter.add(self.curve(stroke));
            painter.line_segment([self.sink - vec2(4.0, 2.5), self.sink], stroke);
            painter.line_segment([self.sink - vec2(4.0, -2.5), self.sink], stroke);
        } else {
            painter.line_segment([self.source, self.sink], stroke);
        }
    }
}

fn segment_distance_squared(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length = segment.length_sq();
    if length == 0.0 {
        return point.distance_sq(start);
    }
    let t = ((point - start).dot(segment) / length).clamp(0.0, 1.0);
    point.distance_sq(start + t * segment)
}

fn marked(items: &[bool], index: usize) -> bool {
    items.get(index).copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::flowgraph;
    use std::{io::Write, sync::atomic::AtomicBool};

    #[test]
    fn picking_uses_the_drawn_curve_and_distinguishes_adjacent_ports() {
        let mut file = tempfile::tempfile().unwrap();
        write!(
            file,
            r#"options:
  parameters: {{id: pick_test}}
blocks:
- {{name: source, id: analog_sig_source_x, states: {{coordinate: [0, 0]}}}}
- {{name: sink, id: blocks_message_debug, states: {{coordinate: [400, 230]}}}}
connections:
- [source, 0, sink, 8]
- [source, out, sink, print]
metadata: {{file_format: 1}}
"#,
        )
        .unwrap();
        let graph = flowgraph::read(file, &AtomicBool::new(false)).unwrap();
        let layout = graph.layout();
        let scene = Scene {
            graph: &graph,
            layout: &layout,
        };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(980.0, 530.0));
        let transform = Camera::default().projection(rect, &layout);
        let highlights = Highlights::default();
        for (index, path) in scene.visible_wires(transform, &highlights, DETAIL_EDGES) {
            let point = path.curve(Stroke::NONE).sample(0.22);
            assert_eq!(scene.pick_wire(transform, point, &highlights), Some(index));
        }
        assert_eq!(
            scene.pick_wire(transform, rect.right_top(), &highlights),
            None
        );
    }

    #[test]
    #[ignore = "manual CPU frame profile; set FLOWGRAPH_BENCH_PATHS to saved captures"]
    fn connection_frame_profile() {
        use std::{fs::File, hint::black_box, time::Instant};

        let paths = std::env::var_os("FLOWGRAPH_BENCH_PATHS").expect("FLOWGRAPH_BENCH_PATHS");
        for path in std::env::split_paths(&paths) {
            let start = Instant::now();
            let graph =
                flowgraph::read(File::open(&path).unwrap(), &AtomicBool::new(false)).unwrap();
            let parse = start.elapsed();
            let start = Instant::now();
            let layout = graph.layout();
            let layout_time = start.elapsed();
            if graph.connections.is_empty() {
                continue;
            }
            let edge = graph.connections.len() / 2;
            let start = Instant::now();
            let rates = Overlay::new(&graph);
            let rate_time = start.elapsed();
            let scene = Scene {
                graph: &graph,
                layout: &layout,
            };
            let size = vec2(1020.0, 780.0);
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let style = little_clip_core::palette::AppPalette::default();
            let colors = WorkbenchColors::from_palette(&style);
            let context = egui::Context::default();
            let highlights = Highlights {
                selected: Some(Selection::Wire(edge)),
                rates: Some(&rates),
                ..Default::default()
            };
            println!(
                "{} blocks / {} connections, parse {parse:?}, layout {layout_time:?}, rates {rate_time:?}",
                graph.blocks.len(),
                graph.connections.len()
            );
            for (view, focus) in [("Fit", false), ("Pan around connection", true)] {
                let mut camera = Camera::default();
                if focus {
                    camera.reveal_wire(rect, &graph, &layout, edge);
                }
                let mut pick = Vec::new();
                let mut paint = Vec::new();
                let mut tessellate = Vec::new();
                for tick in 0..65 {
                    let projection = camera.projection(rect, &layout);
                    let delta = (tick as f32 * 0.20).sin();
                    camera.pan(projection, vec2(delta, 0.0));
                    let [a, b] =
                        layout.wire_anchors[edge].map(|[x, y]| projection.point(pos2(x, y)));
                    let start = Instant::now();
                    black_box(scene.pick_wire(projection, a.lerp(b, 0.5), &highlights));
                    pick.push(start.elapsed());
                    let start = Instant::now();
                    let frame = clip_ui::test_support::run_frame(
                        &context,
                        egui::RawInput {
                            screen_rect: Some(rect),
                            ..Default::default()
                        },
                        |ctx| {
                            ctx.root_ui(|ui| {
                                scene.draw(
                                    ui.painter(),
                                    rect,
                                    projection,
                                    &highlights,
                                    colors,
                                    false,
                                )
                            })
                        },
                    );
                    paint.push(start.elapsed());
                    let start = Instant::now();
                    black_box(context.tessellate(frame.shapes, frame.pixels_per_point));
                    tessellate.push(start.elapsed());
                }
                for sample in [&mut pick, &mut paint, &mut tessellate] {
                    sample.sort_unstable();
                }
                println!(
                    "{view} p95, pick {:?}, draw/pass {:?}, tessellate {:?}",
                    pick[61], paint[61], tessellate[61]
                );
            }
        }
    }
}
