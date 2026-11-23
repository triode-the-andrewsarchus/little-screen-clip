use crate::file_manager::scope_view::plot::{lane, x_at};
use egui::{Color32, Mesh, Painter, Rect, Stroke, pos2};
use little_clip_core::engineering::{
    logic::{Envelope, Level, Trace},
    logic_preview::Overview,
};
use std::ops::Range;

pub(super) fn draw_overview(painter: &Painter, rect: Rect, overview: &Overview, color: Color32) {
    let count = overview.lanes_previewed.len();
    for (index, envelope) in overview.lanes_previewed.iter().enumerate() {
        draw_envelope(
            painter,
            lane(rect, index, count).shrink2(egui::vec2(0.0, 1.0)),
            &overview.span,
            envelope,
            color,
        );
    }
}

pub(super) fn draw_envelope(
    painter: &Painter,
    rect: Rect,
    visible: &Range<f64>,
    envelope: &Envelope,
    color: Color32,
) {
    let step = (envelope.time.end - envelope.time.start) / envelope.bins.len().max(1) as f64;
    let mut mesh = Mesh::default();
    for (index, bin) in envelope.bins.iter().enumerate() {
        let start = envelope.time.start + step * index as f64;
        let left = x_at(rect, visible, start).max(rect.left());
        let right = x_at(rect, visible, start + step).min(rect.right());
        if right <= left {
            continue;
        }
        let ink = if bin.exact {
            color
        } else {
            color.gamma_multiply(0.6)
        };
        for (present, level) in [
            (bin.high, Some(Level::High)),
            (bin.low, Some(Level::Low)),
            (bin.unknown || bin.high_impedance, None),
        ] {
            if present {
                let y = y_at(rect, level);
                mesh.add_colored_rect(
                    Rect::from_min_max(pos2(left, y - 0.5), pos2(right, y + 0.5)),
                    ink,
                );
            }
        }
        if bin.high && bin.low {
            mesh.add_colored_rect(
                Rect::from_min_max(
                    pos2(left, y_at(rect, Some(Level::High))),
                    pos2(right, y_at(rect, Some(Level::Low))),
                ),
                ink.gamma_multiply(0.35),
            );
        }
    }
    painter.add(mesh);
}

pub(super) fn draw_trace(
    painter: &Painter,
    rect: Rect,
    visible: &Range<f64>,
    time: &Range<f64>,
    trace: &Trace,
    color: Color32,
) {
    let mut current = trace.initial;
    let mut points = vec![pos2(x_at(rect, visible, time.start), y_at(rect, current))];
    for edge in &trace.edges {
        let x = x_at(rect, visible, edge.time_seconds);
        points.push(pos2(x, y_at(rect, current)));
        current = Some(edge.level);
        points.push(pos2(x, y_at(rect, current)));
    }
    points.push(pos2(
        x_at(rect, visible, trace.inspected_until.min(time.end)),
        y_at(rect, current),
    ));
    painter.add(egui::Shape::line(points, Stroke::new(1.25, color)));
}

fn y_at(rect: Rect, level: Option<Level>) -> f32 {
    match level {
        Some(Level::High) => rect.top() + rect.height() * 0.14,
        Some(Level::Low) => rect.bottom() - rect.height() * 0.14,
        _ => rect.center().y,
    }
}
