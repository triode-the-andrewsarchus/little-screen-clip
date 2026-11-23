//! Recorded evidence behind a selected transaction from derived equations.

use crate::file_manager::scope_view::plot;
use egui::{Color32, Painter, Rect, RichText, Stroke, Ui, Visuals};
use little_clip_core::engineering::{analysis::InputHistory, waveforms::display_value};
use std::ops::Range;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct TraceTarget {
    pub source: u64,
    pub channel: usize,
}

#[derive(Clone)]
pub(super) struct InputEvidence {
    pub label: String,
    pub target: Option<TraceTarget>,
    pub coverage: Option<Range<f64>>,
    pub note: &'static str,
}

#[derive(Clone, PartialEq)]
pub(super) struct Window {
    pub label: String,
    pub target: Option<TraceTarget>,
    pub span: Option<Range<f64>>,
    age: InputHistory,
    note: &'static str,
}

impl InputEvidence {
    pub fn window(
        &self,
        event: &Range<f64>,
        age: InputHistory,
        grid: &Range<f64>,
        step: f64,
    ) -> Window {
        let from = age.oldest.map_or(grid.start, |oldest| {
            event.start - (f64::from(oldest) + 1.0) * step
        });
        let to = event.end.max(event.start + step) - f64::from(age.newest) * step;
        let span = self.coverage.as_ref().and_then(|coverage| {
            let span = from.max(grid.start).max(coverage.start)..to.min(grid.end).min(coverage.end);
            (!span.is_empty()).then_some(span)
        });
        Window {
            label: self.label.clone(),
            target: self.target,
            span,
            age,
            note: self.note,
        }
    }
}

impl Window {
    pub fn button(&self, ui: &mut Ui) -> bool {
        let Some(span) = &self.span else {
            ui.weak(&self.label).on_hover_text(self.note);
            return false;
        };
        let feedback = if self.age.oldest.is_none() {
            " Feedback can use samples back to the start of the equation grid."
        } else {
            ""
        };
        let tooltip = format!(
            "Workspace time {} to {}. {}{feedback}",
            display_value(span.start, "s"),
            display_value(span.end, "s"),
            self.note,
        );
        if self.target.is_some() {
            let label = match self.age.oldest {
                None => format!("{} · Feedback", self.label),
                Some(0) => self.label.clone(),
                Some(_) => format!("{} · Delayed", self.label),
            };
            ui.small_button(RichText::new(label).color(color(ui.visuals())))
                .on_hover_text(tooltip)
                .clicked()
        } else {
            ui.weak(&self.label).on_hover_text(tooltip);
            false
        }
    }
}

#[derive(Clone)]
pub(in crate::file_manager) struct Marker {
    pub channel: usize,
    pub native: Range<f64>,
}

impl Marker {
    pub fn paint(&self, painter: &Painter, rect: Rect, visible: &Range<f64>) {
        let span = self.native.start.max(visible.start)..self.native.end.min(visible.end);
        if !span.is_empty() {
            let color = color(&painter.ctx().global_style().visuals);
            let left = plot::x_at(rect, visible, span.start);
            let right = plot::x_at(rect, visible, span.end);
            plot::highlight(painter, rect, visible, span, color.gamma_multiply(0.20));
            painter.hline(left..=right, rect.top() + 1.0, Stroke::new(1.5, color));
        }
    }
}

fn color(visuals: &Visuals) -> Color32 {
    if visuals.dark_mode {
        Color32::from_rgb(105, 225, 181)
    } else {
        Color32::from_rgb(0, 104, 75)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_input_windows_are_clipped_to_the_grid_and_available_source_data() {
        let input = InputEvidence {
            label: "CH1".into(),
            target: Some(TraceTarget {
                source: 8,
                channel: 0,
            }),
            coverage: Some(3.0..12.0),
            note: "",
        };
        let delayed = InputHistory {
            newest: 2,
            oldest: Some(4),
        };
        assert_eq!(
            input.window(&(9.0..12.0), delayed, &(0.0..16.0), 1.0).span,
            Some(4.0..10.0)
        );
        let feedback = InputHistory {
            newest: 2,
            oldest: None,
        };
        assert_eq!(
            input.window(&(9.0..12.0), feedback, &(4.0..16.0), 1.0).span,
            Some(4.0..10.0)
        );
        assert_eq!(
            input.window(&(0.0..2.0), delayed, &(0.0..16.0), 1.0).span,
            None
        );
    }
}
