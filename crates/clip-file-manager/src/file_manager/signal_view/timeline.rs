use egui::{Context, PointerButton, Rect, Response};
use little_clip_core::engineering::analysis::Clock;
use std::ops::Range;

use crate::file_manager::scope_view::plot;

pub(in crate::file_manager) struct Link {
    pub domain: Range<f64>,
    pub clock: Clock,
    pub probe: Option<f64>,
    pub reference: Option<f64>,
    pub landmark: Option<LandmarkMarker>,
}

#[derive(Clone, Copy)]
pub(in crate::file_manager) struct LandmarkMarker {
    pub time: f64,
    pub reference: bool,
}

impl Link {
    pub fn clock(link: Option<&Self>) -> Clock {
        link.map(|link| link.clock).unwrap_or_default()
    }

    pub fn paint_markers(&self, painter: &egui::Painter, rect: Rect, span: &Range<f64>) {
        if let Some(landmark) = self.landmark.filter(|mark| span.contains(&mark.time)) {
            let (label, color) = if landmark.reference {
                ("Reference landmark", egui::Color32::from_rgb(85, 181, 235))
            } else {
                ("Target landmark", egui::Color32::from_rgb(229, 173, 90))
            };
            paint_marker(painter, rect, span, landmark.time, label, color);
        }
        if self
            .landmark
            .is_none_or(|mark| Some(mark.time) != self.reference)
        {
            paint_reference(painter, rect, span, self.reference);
        }
    }
}

pub(in crate::file_manager) fn paint_reference(
    painter: &egui::Painter,
    rect: Rect,
    span: &Range<f64>,
    reference: Option<f64>,
) {
    if let Some(time) = reference.filter(|time| span.contains(time)) {
        let color = egui::Color32::from_rgb(222, 175, 247);
        paint_marker(painter, rect, span, time, "Reference", color);
    }
}

fn paint_marker(
    painter: &egui::Painter,
    rect: Rect,
    span: &Range<f64>,
    time: f64,
    label: &str,
    color: egui::Color32,
) {
    let x = plot::x_at(rect, span, time);
    painter.vline(x, rect.y_range(), egui::Stroke::new(1.3, color));
    let (label_x, align) = if x > rect.left() + rect.width() * 0.66 {
        (x - 4.0, egui::Align2::RIGHT_TOP)
    } else {
        (x + 4.0, egui::Align2::LEFT_TOP)
    };
    painter.text(
        egui::pos2(label_x, rect.top() + 3.0),
        align,
        label,
        egui::FontId::monospace(11.0),
        color,
    );
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
        let clock = Clock::offset(0.5);
        let aligned = clock.workspace_range(&native);
        assert_eq!(aligned, 0.375..0.875);
        assert_eq!(clock.native_range(&aligned), native);
    }

    #[test]
    fn zooming_near_the_late_edge_retains_the_requested_width() {
        assert_eq!(fit(&(0.0..4.0), 3.5..4.5, 0.125), 3.0..4.0);
    }
}
