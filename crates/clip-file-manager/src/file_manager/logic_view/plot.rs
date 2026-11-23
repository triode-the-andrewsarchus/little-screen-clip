use crate::file_manager::scope_view::plot::{highlight, lane, x_at};
use egui::{Color32, Mesh, Painter, Rect, Stroke, pos2};
use little_clip_core::engineering::{
    logic::{Envelope, Level, Trace},
    logic_preview::Overview,
};
use std::ops::Range;

pub(in crate::file_manager) fn highlight_event(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    time: Range<f64>,
) {
    highlight(
        painter,
        rect,
        span,
        time,
        Color32::from_rgba_unmultiplied(241, 189, 76, 75),
    );
}

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

pub(in crate::file_manager) fn draw_envelope(
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

pub(in crate::file_manager) fn draw_trace(
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

pub(in crate::file_manager) fn wire_label(
    ui: &egui::Ui,
    painter: &Painter,
    lane: Rect,
    name: &str,
    index: usize,
) -> bool {
    let half = (lane.height() * 0.5 - 2.0).clamp(3.0, 12.0);
    let rect = Rect::from_min_max(
        pos2(lane.left() - 74.0, lane.center().y - half),
        pos2(lane.left() - 4.0, lane.center().y + half),
    );
    let response = ui.interact(
        rect,
        ui.scope_id().with(("wire-logic-lane", index)),
        egui::Sense::click(),
    );
    let style = ui.style().interact(&response);
    painter.rect(
        rect,
        2.0,
        style.weak_bg_fill,
        style.bg_stroke,
        egui::StrokeKind::Inside,
    );
    painter.with_clip_rect(rect.shrink(2.0)).text(
        rect.right_center() - egui::vec2(4.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        name.rsplit('.').next().unwrap_or(name),
        egui::FontId::monospace(11.0),
        style.text_color(),
    );
    response.on_hover_text(format!("Choose {name}")).clicked()
}

pub(in crate::file_manager) fn annotations(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    events: &[little_clip_core::engineering::logic_decode::Event],
    color: Color32,
    ink: Color32,
) {
    let painter = painter.with_clip_rect(rect);
    let mut represented_until = rect.left() - 2.0;
    for event in events.iter().filter(|event| {
        event.time.start < span.end
            && (event.time.end > span.start || event.time.start == span.start)
    }) {
        let left = x_at(rect, span, event.time.start).max(rect.left());
        let right = x_at(rect, span, event.time.end)
            .max(left + 2.0)
            .min(rect.right());
        if left < represented_until || right <= left {
            continue;
        }
        let segment = Rect::from_min_max(
            pos2(left, rect.top() + 3.0),
            pos2(right, rect.bottom() - 2.0),
        );
        painter.rect_filled(segment, 2.0, color.gamma_multiply(0.26));
        if segment.width() > 65.0 {
            painter.with_clip_rect(segment).text(
                segment.left_center() + egui::vec2(3.0, 0.0),
                egui::Align2::LEFT_CENTER,
                super::protocols::event_label(event.kind),
                egui::FontId::monospace(10.0),
                ink,
            );
        }
        represented_until = right;
    }
}
