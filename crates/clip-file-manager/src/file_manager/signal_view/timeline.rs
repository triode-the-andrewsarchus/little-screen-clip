use egui::{Context, PointerButton, Rect, Response};
use std::ops::Range;

use crate::file_manager::scope_view::plot;

// File-backed readers keep their own clock. Positive offsets move a source
// later on the workspace timeline without resampling it.
pub(in crate::file_manager) fn translate(span: &Range<f64>, offset: f64) -> Range<f64> {
    span.start + offset..span.end + offset
}

pub(in crate::file_manager) struct Link {
    pub domain: Range<f64>,
    pub offset: f64,
    pub probe: Option<f64>,
    pub reference: Option<f64>,
}

pub(in crate::file_manager) fn paint_reference(
    painter: &egui::Painter,
    rect: Rect,
    span: &Range<f64>,
    reference: Option<f64>,
) {
    if let Some(time) = reference.filter(|time| span.contains(time)) {
        let x = plot::x_at(rect, span, time);
        let color = egui::Color32::from_rgb(222, 175, 247);
        painter.vline(x, rect.y_range(), egui::Stroke::new(1.3, color));
        painter.text(
            egui::pos2(x + 4.0, rect.top() + 3.0),
            egui::Align2::LEFT_TOP,
            "Reference",
            egui::FontId::monospace(10.0),
            color,
        );
    }
}

pub(in crate::file_manager) fn fit(
    domain: &Range<f64>,
    requested: Range<f64>,
    interval: f64,
) -> Range<f64> {
    let duration = domain.end - domain.start;
    let width = (requested.end - requested.start).clamp((interval * 4.0).min(duration), duration);
    let left = requested
        .start
        .clamp(domain.start, (domain.end - width).max(domain.start));
    left..left + width
}

pub(in crate::file_manager) fn navigate(
    ctx: &Context,
    rect: Rect,
    response: &Response,
    span: &Range<f64>,
    linked: bool,
) -> Option<Range<f64>> {
    let pointer = response.hover_pos().filter(|p| rect.contains(*p))?;
    let (mut scroll, drag) = ctx.input(|input| (input.smooth_scroll_delta, input.pointer.delta()));
    if linked && ctx.input(|input| input.modifiers.shift) {
        // Leave Shift+wheel available to scroll the stack of source lanes.
        scroll = egui::Vec2::ZERO;
    } else if scroll != egui::Vec2::ZERO && linked {
        ctx.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    let drag = if response.dragged_by(PointerButton::Middle) {
        drag.x
    } else {
        0.0
    };
    let motion = -f64::from((scroll.x + drag) / rect.width().max(1.0)) * (span.end - span.start);
    (scroll.y != 0.0 || motion != 0.0).then(|| {
        let anchor = plot::at(rect, span, pointer.x);
        let scale = f64::from((-scroll.y * 0.012).exp()).clamp(0.1, 10.0);
        anchor + (span.start - anchor) * scale + motion
            ..anchor + (span.end - anchor) * scale + motion
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_trigger_time_shifts_to_workspace_and_back() {
        let native = -0.125..0.375;
        let aligned = translate(&native, 0.5);
        assert_eq!(aligned, 0.375..0.875);
        assert_eq!(translate(&aligned, -0.5), native);
    }

    #[test]
    fn zooming_near_the_late_edge_retains_the_requested_width() {
        assert_eq!(fit(&(0.0..4.0), 3.5..4.5, 0.125), 3.0..4.0);
    }
}
