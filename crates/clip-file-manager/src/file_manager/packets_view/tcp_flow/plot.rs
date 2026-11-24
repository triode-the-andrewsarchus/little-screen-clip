use super::{Mode, TcpPoint, display_value, note};
use crate::{file_manager::scope_view::plot as axis, ui_appearance::WorkbenchColors};
use egui::{Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use std::ops::Range;

struct Plot<'a> {
    points: &'a [TcpPoint],
    origin: i128,
    time: Range<f64>,
    acks: bool,
}

struct Lane {
    axis: LaneAxis,
    rect: Rect,
    values: Range<i64>,
}

#[derive(Clone, Copy)]
enum LaneAxis {
    Sequence(usize),
    Delay,
}

#[derive(Clone, Copy)]
enum Glyph {
    Segment,
    Ack,
    Delay,
}

struct Mark {
    low: i64,
    high: i64,
    glyph: Glyph,
    sender: usize,
}

pub(super) fn draw(
    ui: &mut Ui,
    points: &[TcpPoint],
    selected: Option<u64>,
    origin: Option<i128>,
    colors: WorkbenchColors,
    mode: Mode,
    acks: bool,
) -> Option<u64> {
    let Some(plot) = Plot::new(points, origin, acks) else {
        ui.weak("No timestamped TCP frames in this connection.");
        return None;
    };
    let height = (ui.available_height() * 0.33).clamp(122.0, 220.0);
    let (outer, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let rect = Rect::from_min_max(outer.min + vec2(85.0, 3.0), outer.max - vec2(9.0, 17.0));
    let painter = ui.painter_at(outer);
    let clipped = painter.with_clip_rect(rect);
    clipped.rect_filled(rect, 3.0, colors.recessed);
    let pointer = response.hover_pos().filter(|point| rect.contains(*point));
    let mut near = None::<(&TcpPoint, f32)>;
    let axes: &[LaneAxis] = match mode {
        Mode::Sequence => &[LaneAxis::Sequence(0), LaneAxis::Sequence(1)],
        Mode::Delay => &[LaneAxis::Delay],
    };
    for (index, measure) in axes.iter().copied().enumerate() {
        let row = axis::lane(rect, index, axes.len()).shrink2(vec2(0.0, 2.5));
        let lane = Lane::new(row, measure, &plot);
        lane.axis(&painter, colors);
        if let Some(hit) = lane.paint(&clipped, &plot, pointer)
            && near.is_none_or(|(_, old)| hit.1 < old)
        {
            near = Some(hit);
        }
    }
    axis::axes(&painter, rect, &plot.time, colors.text_muted);
    plot.paint_selected(&clipped, rect, selected, colors.focus);
    if mode == Mode::Delay && !points.iter().any(|point| point.ack_delay.is_some()) {
        clipped.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No unambiguous ACK delays in this range",
            FontId::proportional(13.0),
            colors.text_muted,
        );
    }
    if let Some((point, _)) = near {
        let picked = response.clicked().then_some(point.row);
        response.on_hover_text(readout(point, mode));
        return picked;
    }
    None
}

impl<'a> Plot<'a> {
    fn new(points: &'a [TcpPoint], origin: Option<i128>, acks: bool) -> Option<Self> {
        let mut timed = points.iter().filter_map(|point| point.time_ns);
        let first = timed.next()?;
        let (min, max) = timed.fold((first, first), |(a, b), time| (a.min(time), b.max(time)));
        let origin = origin.unwrap_or(min);
        let start = (min - origin) as f64 / 1e9;
        let end = (max - origin) as f64 / 1e9;
        let padding = ((end - start) * 0.02).max(1e-9);
        Some(Self {
            points,
            origin,
            time: start - padding..end + padding,
            acks,
        })
    }

    fn x_for(&self, rect: Rect, point: &TcpPoint) -> Option<f32> {
        let time = (point.time_ns? - self.origin) as f64 / 1e9;
        Some(axis::x_at(rect, &self.time, time))
    }

    fn paint_selected(&self, painter: &Painter, rect: Rect, selected: Option<u64>, color: Color32) {
        if let Some(row) = selected
            && let Some(point) = super::find_row(self.points, row)
            && let Some(x) = self.x_for(rect, point)
        {
            painter.vline(x, rect.y_range(), Stroke::new(1.25, color));
        }
    }
}

impl Lane {
    fn new(rect: Rect, axis: LaneAxis, plot: &Plot<'_>) -> Self {
        let (min, max) = plot
            .points
            .iter()
            .filter(|point| point.time_ns.is_some())
            .filter_map(|point| Mark::for_lane(point, axis, plot.acks))
            .fold((0, 0), |(low, high), mark| {
                (low.min(mark.low), high.max(mark.high))
            });
        Self {
            axis,
            rect,
            values: min..max.max(min + 1),
        }
    }

    fn axis(&self, painter: &Painter, colors: WorkbenchColors) {
        let label = match self.axis {
            LaneAxis::Sequence(sender) => ["A → B", "B → A"][sender],
            LaneAxis::Delay => "ACK delay",
        };
        painter.text(
            self.rect.left_center() - vec2(5.0, 0.0),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(11.0),
            colors.text_primary,
        );
        for (y, align, value) in [
            (self.rect.top(), Align2::RIGHT_TOP, self.values.end),
            (self.rect.bottom(), Align2::RIGHT_BOTTOM, self.values.start),
        ] {
            painter.hline(
                self.rect.x_range(),
                y,
                Stroke::new(0.7, colors.text_muted.gamma_multiply(0.2)),
            );
            let value = match self.axis {
                LaneAxis::Sequence(_) => format!("{value} B"),
                LaneAxis::Delay => display_value(value as f64 / 1e9, "s"),
            };
            painter.text(
                pos2(self.rect.left() - 5.0, y),
                align,
                value,
                FontId::monospace(10.0),
                colors.text_muted,
            );
        }
    }

    fn paint<'a>(
        &self,
        painter: &Painter,
        plot: &Plot<'a>,
        pointer: Option<Pos2>,
    ) -> Option<(&'a TcpPoint, f32)> {
        let mut near = None::<(&TcpPoint, f32)>;
        let clipped = painter.with_clip_rect(self.rect);
        let mut marks = Mesh::default();
        for point in plot.points {
            let Some(mark) = Mark::for_lane(point, self.axis, plot.acks) else {
                continue;
            };
            let Some(x) = plot.x_for(self.rect, point) else {
                continue;
            };
            let top = self.y(mark.high);
            let bottom = self.y(mark.low).max(top + 1.0).min(self.rect.bottom());
            let color = axis::color(mark.sender);
            let inset = self.rect.shrink(4.0);
            let dot = inset.clamp(pos2(x, top));
            match mark.glyph {
                Glyph::Ack => ack_mark(&mut marks, dot, color.gamma_multiply(0.8)),
                Glyph::Delay => {
                    marks.add_colored_rect(Rect::from_center_size(dot, vec2(6.0, 6.0)), color)
                }
                Glyph::Segment => marks.add_colored_rect(
                    Rect::from_min_max(pos2(x - 1.0, top - 0.5), pos2(x + 1.0, bottom + 0.5)),
                    color,
                ),
            }
            if let Some(pointer) = pointer.filter(|point| self.rect.contains(*point)) {
                let nearest = match mark.glyph {
                    Glyph::Segment => pos2(x, pointer.y.clamp(top, bottom)),
                    Glyph::Ack | Glyph::Delay => dot,
                };
                let distance = pointer.distance_sq(nearest);
                if distance <= 12.0_f32.powi(2) && near.is_none_or(|(_, old)| distance < old) {
                    near = Some((point, distance));
                }
            }
        }
        clipped.add(marks);
        near
    }

    fn y(&self, number: i64) -> f32 {
        let ratio = (number as f64 - self.values.start as f64)
            / (self.values.end as f64 - self.values.start as f64).max(1.0);
        self.rect.bottom() - self.rect.height() * ratio as f32
    }
}

fn ack_mark(mesh: &mut Mesh, at: Pos2, color: Color32) {
    let first = mesh.vertices.len() as u32;
    let corners = [
        vec2(0.0, -1.0),
        vec2(1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(-1.0, 0.0),
    ];
    for radius in [3.7, 1.6] {
        for corner in corners {
            mesh.colored_vertex(at + corner * radius, color);
        }
    }
    for corner in 0..4 {
        let inner = first + 4;
        let next = (corner + 1) % 4;
        mesh.add_triangle(first + corner, first + next, inner + corner);
        mesh.add_triangle(first + next, inner + next, inner + corner);
    }
}

impl Mark {
    fn for_lane(point: &TcpPoint, axis: LaneAxis, acks: bool) -> Option<Self> {
        match axis {
            LaneAxis::Sequence(sender) if sender == point.sender => {
                let low = point.sequence?;
                Some(Self {
                    low,
                    high: point.sequence_end.unwrap_or(low),
                    glyph: Glyph::Segment,
                    sender,
                })
            }
            LaneAxis::Sequence(sender) if acks => {
                let ack = point.acknowledged?;
                Some(Self {
                    low: ack,
                    high: ack,
                    glyph: Glyph::Ack,
                    sender,
                })
            }
            LaneAxis::Sequence(_) => None,
            LaneAxis::Delay => {
                let ns = i64::try_from(point.ack_delay?.nanoseconds).ok()?;
                Some(Self {
                    low: ns,
                    high: ns,
                    glyph: Glyph::Delay,
                    sender: 1 - point.sender,
                })
            }
        }
    }
}

fn readout(point: &TcpPoint, mode: Mode) -> String {
    if mode == Mode::Delay
        && let Some(delay) = point.ack_delay
    {
        let direction = ["A → B", "B → A"][1 - point.sender];
        return format!(
            "{direction} · {}\nSegment frame {} · ACK frame {}",
            display_value(delay.nanoseconds as f64 / 1e9, "s"),
            delay.segment_row + 1,
            point.row + 1,
        );
    }
    let direction = ["A → B", "B → A"][point.sender];
    let segment = point.segment;
    let format = |number: Option<i64>| number.map_or_else(|| "—".into(), |value| value.to_string());
    let mut description = format!(
        "Frame {} · {direction}\nRelative sequence {} → {} · ACK {}\nRaw window {} · Flags {:#x}",
        point.row + 1,
        format(point.sequence),
        format(point.sequence_end),
        format(point.acknowledged),
        segment.advertised_window,
        segment.flags,
    );
    let note = note(point.position);
    if !note.is_empty() {
        description.push('\n');
        description.push_str(note);
    }
    description
}
