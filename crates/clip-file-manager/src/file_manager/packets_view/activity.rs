use egui::{Align2, FontId, Painter, Rect, Stroke, pos2};
use little_clip_core::engineering::packets::ActivityBin;
use std::ops::Range;

use crate::ui_appearance::WorkbenchColors;

pub(super) type Bounds = [i128; 2];

pub(super) fn bounds(bins: &[ActivityBin]) -> Option<Bounds> {
    Some([bins.first()?.start_ns, bins.last()?.end_ns])
}

pub(super) fn at(rect: Rect, [start, end]: Bounds, x: f32) -> i128 {
    let fraction = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    start + ((end - start) as f64 * f64::from(fraction)) as i128
}

pub(super) fn x_at(rect: Rect, [start, end]: Bounds, time: i128) -> f32 {
    let fraction = (time - start) as f64 / (end - start).max(1) as f64;
    rect.left() + rect.width() * fraction as f32
}

pub(super) fn nearest(bins: &[ActivityBin], ns: i128) -> Option<ActivityBin> {
    let next = bins.partition_point(|bin| bin.end_ns <= ns);
    match (
        next.checked_sub(1).and_then(|last| bins.get(last)),
        bins.get(next),
    ) {
        (Some(previous), Some(next)) if ns - previous.end_ns < next.start_ns - ns => {
            Some(*previous)
        }
        (_, Some(next)) => Some(*next),
        (previous, None) => previous.copied(),
    }
}

pub(super) fn source_span(bins: &[ActivityBin], [start, end]: Bounds) -> Option<Range<u64>> {
    let low = start.min(end);
    let high = start.max(end);
    let mut first = u64::MAX;
    let mut last = 0;
    let position = bins.partition_point(|bin| bin.end_ns <= low);
    for bin in bins[position..]
        .iter()
        .take_while(|bin| bin.start_ns <= high)
    {
        first = first.min(bin.first_row);
        last = last.max(bin.last_row);
    }
    (first <= last).then(|| first..last + 1)
}

pub(super) fn draw(painter: &Painter, rect: Rect, bins: &[ActivityBin], colors: WorkbenchColors) {
    painter.rect_filled(rect, 3.0, colors.recessed);
    let Some(bounds) = bounds(bins) else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No timed packets",
            FontId::proportional(11.0),
            colors.text_muted,
        );
        return;
    };
    let mut mesh = egui::Mesh::default();
    let highest = bins.iter().map(|bin| bin.packets).max().unwrap_or(1);
    let baseline = rect.bottom() - 2.0;
    let height = (rect.height() - 6.0).max(0.0);
    for bin in bins {
        let start = x_at(rect, bounds, bin.start_ns);
        let width = (x_at(rect, bounds, bin.end_ns) - start).max(1.0);
        let count = ((bin.packets as f64).ln_1p() / (highest as f64).ln_1p()) as f32;
        mesh.add_colored_rect(
            Rect::from_min_max(
                pos2(start, baseline - height * count),
                pos2((start + width).min(rect.right()), baseline),
            ),
            colors.accent,
        );
    }
    painter.with_clip_rect(rect).add(mesh);
}

pub(super) fn mark(
    painter: &Painter,
    rect: Rect,
    bounds: Bounds,
    time: i128,
    colors: WorkbenchColors,
) {
    let x = x_at(rect, bounds, time);
    if (rect.left()..=rect.right()).contains(&x) {
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.5, colors.focus),
        );
    }
}

pub(super) fn highlight(
    painter: &Painter,
    rect: Rect,
    domain: Bounds,
    band: Bounds,
    colors: WorkbenchColors,
) {
    let left = x_at(rect, domain, band[0].min(band[1]));
    let right = x_at(rect, domain, band[0].max(band[1]));
    painter.rect_filled(
        Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
        0.0,
        colors.focus.gamma_multiply(0.30),
    );
}

pub(super) fn with_mark(
    painter: &Painter,
    rect: Rect,
    bins: &[ActivityBin],
    time: Option<i128>,
    colors: WorkbenchColors,
) {
    draw(painter, rect, bins, colors);
    if let Some((domain, time)) = bounds(bins).zip(time) {
        mark(painter, rect, domain, time, colors);
    }
}

pub(super) fn relative(ns: i128) -> String {
    if ns.abs() >= 1_000_000_000 {
        format!("{:.6} s", ns as f64 / 1_000_000_000.0)
    } else if ns.abs() >= 1_000_000 {
        format!("{:.3} ms", ns as f64 / 1_000_000.0)
    } else if ns.abs() >= 1_000 {
        format!("{:.3} µs", ns as f64 / 1_000.0)
    } else {
        format!("{ns} ns")
    }
}
