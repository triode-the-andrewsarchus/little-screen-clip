use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, pos2};
use little_clip_core::engineering::{
    scope::Channel,
    waveforms::{Envelope, Overview, Peak, display_value, sample_span, time_span},
};
use std::ops::Range;

const COLORS: [Color32; 4] = [
    Color32::from_rgb(80, 190, 248),
    Color32::from_rgb(249, 191, 72),
    Color32::from_rgb(104, 211, 143),
    Color32::from_rgb(238, 131, 184),
];

pub(super) fn color(index: usize) -> Color32 {
    COLORS[index % COLORS.len()]
}

pub(super) fn fraction(range: &Range<f64>, x: f64) -> f32 {
    ((x - range.start) / (range.end - range.start).max(f64::MIN_POSITIVE)) as f32
}

pub(in crate::file_manager) fn at(rect: Rect, range: &Range<f64>, x: f32) -> f64 {
    range.start
        + f64::from(((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0))
            * (range.end - range.start)
}

pub(in crate::file_manager) fn x_at(rect: Rect, range: &Range<f64>, time: f64) -> f32 {
    rect.left() + rect.width() * fraction(range, time)
}

pub(in crate::file_manager) fn lane(rect: Rect, index: usize, count: usize) -> Rect {
    let height = rect.height() / count.max(1) as f32;
    Rect::from_min_size(
        pos2(rect.left(), rect.top() + index as f32 * height),
        egui::vec2(rect.width(), height),
    )
}

pub(in crate::file_manager) fn axes(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    foreground: Color32,
) {
    for bin in 0..=6 {
        let fraction = bin as f32 / 6.0;
        let x = rect.left() + rect.width() * fraction;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, foreground.gamma_multiply(0.21)),
        );
        let align = match bin {
            0 => Align2::LEFT_TOP,
            6 => Align2::RIGHT_TOP,
            _ => Align2::CENTER_TOP,
        };
        painter.text(
            pos2(x, rect.bottom() + 2.0),
            align,
            display_value(
                span.start + f64::from(fraction) * (span.end - span.start),
                "s",
            ),
            FontId::monospace(10.0),
            foreground,
        );
    }
}

pub(super) fn overview(painter: &Painter, rect: Rect, overview: &Overview) {
    let span = time_span(&overview.channels);
    for (index, (channel, envelope)) in overview
        .channels
        .iter()
        .zip(&overview.envelopes)
        .enumerate()
    {
        let lane = lane(rect, index, overview.channels.len()).shrink2(egui::vec2(0.0, 2.0));
        draw_envelope(
            painter,
            lane,
            &span,
            channel,
            envelope,
            envelope.extrema(),
            color(index),
        );
    }
}

pub(super) fn draw_envelope(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    channel: &Channel,
    envelope: &Envelope,
    bounds: Peak,
    stroke: Color32,
) {
    if !bounds.valid() {
        return;
    }
    let samples = sample_span(channel, span.clone());
    let first = samples.start;
    let count = samples.end.saturating_sub(first);
    if count == 0 {
        return;
    }
    let width = (rect.width() * painter.ctx().pixels_per_point())
        .ceil()
        .clamp(1.0, 2048.0) as usize;
    let bins = envelope.columns(samples, width);
    let dt = count as f64 / bins.len().max(1) as f64;
    let mut mesh = egui::Mesh::default();
    let mut line = Vec::with_capacity(bins.len());
    for (index, peak) in bins.into_iter().enumerate() {
        if !peak.valid() {
            continue;
        }
        let time = channel.start_seconds
            + (first as f64 + (index as f64 + 0.5) * dt) * channel.interval_seconds;
        let x = x_at(rect, span, time);
        let (top, bottom) = (y_at(rect, bounds, peak.max), y_at(rect, bounds, peak.min));
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(x - 0.45, top), pos2(x + 0.45, bottom.max(top + 0.8))),
            stroke,
        );
        line.push(pos2(x, (top + bottom) * 0.5));
    }
    painter.add(mesh);
    if line.len() > 1 {
        painter.add(egui::Shape::line(
            line,
            Stroke::new(1.05, stroke.gamma_multiply(0.84)),
        ));
    }
}

pub(super) fn draw_raw(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    start: f64,
    step: f64,
    values: &[f32],
    bounds: Peak,
    color: Color32,
) {
    if values.is_empty() || !bounds.valid() {
        return;
    }
    let mut first = 0;
    for segment in values.split(|value| !value.is_finite()) {
        let points = segment
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                let time = start + (first + index) as f64 * step;
                pos2(x_at(rect, span, time), y_at(rect, bounds, value))
            })
            .collect::<Vec<Pos2>>();
        if points.len() == 1 {
            painter.circle_filled(points[0], 2.0, color);
        } else if !points.is_empty() {
            painter.add(egui::Shape::line(points, Stroke::new(1.3, color)));
        }
        first += segment.len() + 1;
    }
}

pub(super) fn y_at(rect: Rect, bounds: Peak, value: f32) -> f32 {
    let scale = (bounds.max - bounds.min).max(f32::MIN_POSITIVE);
    if bounds.min == bounds.max {
        rect.center().y
    } else {
        rect.bottom() - ((value - bounds.min) / scale).clamp(0.0, 1.0) * rect.height()
    }
}

pub(in crate::file_manager) fn highlight(
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    selection: Range<f64>,
    color: Color32,
) {
    let left = x_at(rect, span, selection.start).max(rect.left());
    let right = x_at(rect, span, selection.end).min(rect.right());
    if left <= right {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(left, rect.top()),
                pos2(right.max(left + 0.75), rect.bottom()),
            ),
            0.0,
            color,
        );
    }
}
