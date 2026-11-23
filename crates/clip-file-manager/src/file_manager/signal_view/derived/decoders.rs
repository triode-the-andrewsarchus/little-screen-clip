use super::compute::{Frame, Lane};
use crate::file_manager::logic_view::{
    plot,
    protocols::{Panel, TraceInput},
};
use egui::{Painter, Rect, Ui};
use little_clip_core::engineering::logic::Trace;
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub(super) struct Decoders {
    panel: Panel,
    frame: Option<Arc<Frame>>,
    revision: u64,
    cursor: Option<f64>,
}

impl Decoders {
    pub(super) fn controls(&mut self, ui: &mut Ui, frame: &Arc<Frame>, span: &Range<f64>) {
        if frame.digital.is_empty() {
            return;
        }
        self.panel.add_menu(ui, &frame.digital);
        if self.panel.edges() {
            return;
        }
        self.panel.controls(ui, &frame.digital);
        if self
            .frame
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, frame))
        {
            self.frame = Some(frame.clone());
            self.revision = self.revision.wrapping_add(1);
        }
        let source: Arc<dyn TraceInput> = frame.clone();
        self.panel
            .update_materialized(ui.ctx(), span.clone(), self.revision, source);
    }

    pub(super) fn active(&self) -> bool {
        !self.panel.edges()
    }

    pub(super) fn paint(
        &self,
        ui: &Ui,
        painter: &Painter,
        band: Rect,
        plot_rect: Rect,
        span: &Range<f64>,
    ) {
        if let Some(event) = self.panel.cursor_event(span, self.cursor) {
            crate::file_manager::scope_view::plot::highlight(
                painter,
                plot_rect,
                span,
                event.time.clone(),
                ui.visuals().selection.bg_fill.gamma_multiply(0.24),
            );
        }
        plot::annotations(
            painter,
            band,
            span,
            self.panel.events(span),
            ui.visuals().selection.bg_fill,
            ui.visuals().text_color(),
        );
    }

    pub(super) fn cursor(&self) -> Option<f64> {
        self.cursor
    }

    pub(super) fn table(&mut self, ui: &mut Ui, span: &Range<f64>, probe: Option<f64>) {
        if !self.active() {
            return;
        }
        if let Some(event) = self.panel.table(ui, span, probe.or(self.cursor), None) {
            self.cursor = Some(event.time.start);
            ui.ctx().request_repaint();
        }
    }
}

impl TraceInput for Frame {
    fn time_bounds(&self, index: usize) -> Option<Range<f64>> {
        self.digital.get(index).map(|_| self.span.clone())
    }

    fn read_edges(
        &self,
        index: usize,
        time: Range<f64>,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Trace, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Decoding cancelled".into());
        }
        let trace = self
            .lanes
            .iter()
            .filter_map(|lane| match lane {
                Lane::Digital { trace, .. } => Some(trace),
                Lane::Analog { .. } => None,
            })
            .nth(index)
            .ok_or("The derived digital channel is unavailable")?;
        let span = time.start.max(self.span.start)..time.end.min(self.span.end);
        if span.is_empty() {
            return Err("No derived samples in this window".into());
        }
        Ok(clip_trace(trace, span, limit))
    }
}

fn clip_trace(trace: &Trace, window: Range<f64>, limit: usize) -> Trace {
    let first = trace
        .edges
        .partition_point(|edge| edge.time_seconds <= window.start);
    let end = trace
        .edges
        .partition_point(|edge| edge.time_seconds < window.end)
        .max(first);
    let copied = end.min(first.saturating_add(limit));
    let initial = first
        .checked_sub(1)
        .map(|index| trace.edges[index].level)
        .or(trace.initial);
    let inspected_until = if copied < end {
        trace.edges[copied].time_seconds
    } else {
        trace.inspected_until.min(window.end)
    };
    Trace {
        initial,
        edges: trace.edges[first..copied].to_vec(),
        inspected_until,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::logic::{Edge, Level};

    #[test]
    fn decoder_window_keeps_the_state_before_its_first_edge_and_reports_truncation() {
        let trace = Trace {
            initial: Some(Level::Low),
            edges: vec![
                Edge {
                    time_seconds: 1.0,
                    level: Level::High,
                },
                Edge {
                    time_seconds: 2.0,
                    level: Level::Unknown,
                },
                Edge {
                    time_seconds: 3.0,
                    level: Level::Low,
                },
            ],
            inspected_until: 8.0,
        };
        let cropped = clip_trace(&trace, 1.0..7.0, 1);
        assert_eq!(cropped.initial, Some(Level::High));
        assert_eq!(cropped.edges, trace.edges[1..2]);
        assert_eq!(cropped.inspected_until, 3.0);
    }
}
