use crate::ui_appearance::WorkbenchColors;
use egui::{Painter, Rect, Stroke, pos2, vec2};
use little_clip_core::engineering::{
    can::{ActivityBin, Summary},
    waveforms::display_value,
};
use std::ops::Range;

pub(super) type Domain = Range<i128>;

pub(super) fn domain(summary: &Summary) -> Domain {
    let [first, last] = summary.time_bounds_ns.unwrap_or([0, 0]);
    first..last + 1
}

pub(super) fn time_label(time: i128) -> String {
    display_value(time as f64 * 1e-9, "s")
}

pub(super) fn at(rect: Rect, span: &Domain, x: f32) -> i128 {
    let fraction = ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    span.start + ((span.end - span.start) as f64 * f64::from(fraction)).round() as i128
}

fn x(rect: Rect, span: &Domain, time: i128) -> f32 {
    let fraction = (time - span.start) as f64 / (span.end - span.start).max(1) as f64;
    rect.left() + rect.width() * (fraction as f32).clamp(0.0, 1.0)
}

pub(super) fn nearest(bins: &[ActivityBin], time: i128) -> Option<&ActivityBin> {
    let index = bins.partition_point(|bin| bin.time_ns.end <= time);
    match (
        index.checked_sub(1).and_then(|i| bins.get(i)),
        bins.get(index),
    ) {
        (Some(before), Some(after)) if time < after.time_ns.start => {
            if time - before.time_ns.end <= after.time_ns.start - time {
                Some(before)
            } else {
                Some(after)
            }
        }
        (_, Some(after)) => Some(after),
        (before, None) => before,
    }
}

pub(super) fn zoom(full: &Domain, span: &Domain, anchor: i128, delta: f32) -> Domain {
    let full_width = full.end - full.start;
    let old = (span.end - span.start).max(1);
    let factor = f64::from((-delta * 0.003).clamp(-1.2, 1.2).exp());
    let new = ((old as f64 * factor).round() as i128).clamp(full_width.min(1_000), full_width);
    let fraction = (anchor - span.start) as f64 / old as f64;
    let start =
        (anchor - (new as f64 * fraction).round() as i128).clamp(full.start, full.end - new);
    start..start + new
}

pub(super) fn draw(
    painter: &Painter,
    rect: Rect,
    span: &Domain,
    bins: &[ActivityBin],
    colors: WorkbenchColors,
) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    painter.rect_filled(rect, 3.0, colors.recessed);
    let first = bins.partition_point(|bin| bin.time_ns.end <= span.start);
    let count = bins[first..].partition_point(|bin| bin.time_ns.start < span.end);
    let bins = &bins[first..first + count];
    let peak = bins.iter().map(|bin| bin.frames).max().unwrap_or(1) as f32;
    for bin in bins {
        let left = x(rect, span, bin.time_ns.start);
        let right = x(rect, span, bin.time_ns.end.max(bin.time_ns.start));
        let width = (right - left).max(1.0).min(rect.right() - left);
        let height = ((bin.frames as f32).ln_1p() / peak.ln_1p() * rect.height()).max(1.0);
        let bar = Rect::from_min_size(pos2(left, rect.bottom() - height), vec2(width, height));
        painter.rect_filled(bar, 0.0, colors.accent.gamma_multiply(0.82));
        if bin.error_frames > 0 {
            let tip = Rect::from_min_size(bar.min, vec2(width, height.min(3.0)));
            painter.rect_filled(tip, 0.0, colors.warning);
        }
    }
}

pub(super) fn highlight(
    painter: &Painter,
    rect: Rect,
    span: &Domain,
    selected: &Domain,
    colors: WorkbenchColors,
) {
    if selected.end <= span.start || selected.start >= span.end {
        return;
    }
    let left = x(rect, span, selected.start);
    let right = x(rect, span, selected.end);
    painter.rect_filled(
        Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
        0.0,
        colors.focus.gamma_multiply(0.18),
    );
    for x in [left, right] {
        painter.vline(x, rect.y_range(), Stroke::new(1.0, colors.focus));
    }
}

pub(super) fn marker(
    painter: &Painter,
    rect: Rect,
    span: &Domain,
    time: i128,
    colors: WorkbenchColors,
) {
    if span.contains(&time) {
        painter.vline(
            x(rect, span, time),
            rect.y_range(),
            Stroke::new(1.5, colors.focus),
        );
    }
}
