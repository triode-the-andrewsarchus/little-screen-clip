use super::super::evidence::{TraceTarget, Window};
use super::compute::{Frame, Lane};
use crate::file_manager::logic_view::{
    plot,
    protocols::{EventAction, Panel, TraceInput, event_label},
};
use egui::{Rect, Sense, Ui};
use little_clip_core::engineering::{
    analysis::InputHistory, logic::Trace, waveforms::display_value,
};
use std::{
    collections::BTreeMap,
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
    visit: Option<Visit>,
    reveal_event: bool,
    reveal_input: Option<TraceTarget>,
}

struct Visit {
    origin: Range<f64>,
    label: String,
    window: Range<f64>,
    target: TraceTarget,
}

impl Decoders {
    pub(super) fn save(&self) -> crate::file_manager::logic_view::protocols::SessionSettings {
        self.panel.session_settings()
    }

    pub(super) fn restore(
        &mut self,
        state: crate::file_manager::logic_view::protocols::SessionSettings,
    ) -> Result<(), String> {
        self.panel
            .restore_session(state, None, &AtomicBool::new(false))
    }

    pub(super) fn remap(&mut self, digital: &[Option<usize>]) {
        self.panel.remap_channels(digital);
        self.retire_frame();
    }

    pub(super) fn retire_frame(&mut self) {
        self.frame = None;
        self.cursor = None;
        self.visit = None;
        self.reveal_input = None;
        self.reveal_event = false;
    }

    pub(super) fn controls(&mut self, ui: &mut Ui, frame: &Arc<Frame>, span: &Range<f64>) {
        if frame.digital.is_empty() {
            return;
        }
        let added = self.panel.decoder_buttons(ui, &frame.digital);
        if self.panel.controls(ui, &frame.digital) || added {
            self.retire_frame();
        }
        if self.panel.edges() {
            return;
        }
        if self.frame.as_ref().is_none_or(|old| {
            self.visit.is_none() && !self.panel.inspecting() && !Arc::ptr_eq(old, frame)
        }) {
            self.frame = Some(frame.clone());
            self.revision = self.revision.wrapping_add(1);
        }
        let source: Arc<dyn TraceInput> = self.frame.as_ref().unwrap_or(frame).clone();
        self.panel.update_materialized(
            ui.ctx(),
            self.visit
                .as_ref()
                .map_or(span, |visit| &visit.origin)
                .clone(),
            self.revision,
            source,
        );
    }

    pub(super) fn active(&self) -> bool {
        !self.panel.edges()
    }

    pub(super) fn paint(
        &self,
        ui: &Ui,
        band: Rect,
        plots: impl IntoIterator<Item = (usize, Rect)>,
        span: &Range<f64>,
    ) -> Option<EventAction> {
        let decode_span = self.decode_span(span);
        if let Some(event) = self.panel.cursor_event(decode_span, self.cursor)
            && let Some(frame) = &self.frame
        {
            let wired = self.wired(frame);
            for (index, rect) in plots {
                if !wired.contains(&index) {
                    continue;
                }
                plot::highlight_event(ui.painter(), rect, span, event.time.clone());
            }
        }
        let response = ui.interact(
            band,
            ui.make_persistent_id("equation-annotations"),
            Sense::click(),
        );
        plot::annotations(
            ui,
            ui.painter(),
            response,
            span,
            self.panel.events(decode_span),
            self.panel.picked_event(decode_span),
        )
    }

    fn decode_span<'a>(&'a self, span: &'a Range<f64>) -> &'a Range<f64> {
        self.visit.as_ref().map_or(span, |visit| &visit.origin)
    }

    fn wired(&self, frame: &Frame) -> Vec<usize> {
        let channels = self.panel.channels().unwrap_or_default();
        frame
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| matches!(lane, Lane::Digital { .. }))
            .enumerate()
            .filter_map(|(wire, (index, _))| channels.contains(&wire).then_some(index))
            .collect()
    }

    pub(super) fn source_windows(&self, span: &Range<f64>) -> Vec<Window> {
        let Some(frame) = &self.frame else {
            return Vec::new();
        };
        let Some(event) = self.panel.picked_event(self.decode_span(span)) else {
            return Vec::new();
        };
        let mut ages: BTreeMap<usize, InputHistory> = BTreeMap::new();
        for lane in self.wired(frame) {
            for (&input, &age) in &frame.history[lane] {
                ages.entry(input)
                    .and_modify(|entry| entry.merge(age))
                    .or_insert(age);
            }
        }
        ages.into_iter()
            .map(|(input, age)| {
                frame.inputs[input].window(&event.time, age, &frame.span, frame.step)
            })
            .collect()
    }

    pub(super) fn return_to_event(&mut self, ui: &mut Ui, span: &mut Range<f64>) {
        let Some(visit) = &self.visit else { return };
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Back to decoded event").clicked() {
                *span = visit.origin.clone();
                self.reveal_event = true;
            }
            ui.weak(format!(
                "{} · {} to {}",
                visit.label,
                display_value(visit.window.start, "s"),
                display_value(visit.window.end, "s"),
            ));
        });
        if self.reveal_event {
            self.visit = None;
        }
    }

    pub(super) fn visiting_input(&self) -> Option<TraceTarget> {
        self.visit.as_ref().map(|visit| visit.target)
    }

    pub(super) fn take_source_target(&mut self) -> Option<TraceTarget> {
        self.reveal_input.take()
    }

    fn source_buttons(&mut self, ui: &mut Ui, span: &mut Range<f64>, domain: &Range<f64>) {
        let inputs = self.source_windows(span);
        if inputs.is_empty() {
            return;
        }
        let Some(event) = self.panel.picked_event(self.decode_span(span)) else {
            return;
        };
        let response = ui.horizontal_wrapped(|ui| {
            ui.strong(event_label(event.kind));
            ui.separator();
            ui.weak("Equation inputs").on_hover_text(
                "Windows that can contribute to the selected event, including delayed terms. Click a channel to see its waveform.",
            );
            let mut picked = None;
            for input in inputs {
                if input.button(ui) {
                    picked = Some(input);
                }
            }
            picked
        });
        if std::mem::take(&mut self.reveal_event) {
            response.response.scroll_to_me(Some(egui::Align::Center));
        }
        let Some(input) = response.inner else { return };
        let (Some(target), Some(window)) = (input.target, input.span) else {
            return;
        };
        self.visit = Some(Visit {
            origin: self.decode_span(span).clone(),
            label: input.label,
            window: window.clone(),
            target,
        });
        self.reveal_input = Some(target);
        if window.start < span.start || window.end > span.end {
            let center = window.start.midpoint(window.end);
            let half = ((span.end - span.start) * 0.5).max((window.end - window.start) * 0.65);
            *span = super::timeline::fit(domain, center - half..center + half, 0.0);
        }
        ui.ctx().request_repaint();
    }

    pub(super) fn cursor(&self) -> Option<f64> {
        self.cursor
    }

    pub(super) fn return_controls(&mut self, ui: &mut Ui, span: &mut Range<f64>) {
        if self.visit.is_none() {
            self.panel.return_controls(ui, span);
        }
    }

    pub(super) fn table(
        &mut self,
        ui: &mut Ui,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        probe: Option<f64>,
        step: f64,
    ) {
        if !self.active() {
            return;
        }
        self.source_buttons(ui, span, domain);
        if let Some(action) = self.panel.table(
            ui,
            self.visit.as_ref().map_or(span, |visit| &visit.origin),
            probe.or(self.cursor),
            None,
            Default::default(),
        ) {
            self.follow_event(ui, action, span, domain, step);
        }
    }

    pub(super) fn follow_event(
        &mut self,
        ui: &Ui,
        action: EventAction,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        step: f64,
    ) {
        if let Some(visit) = self.visit.take() {
            *span = visit.origin;
        }
        self.cursor = Some(self.panel.follow_event(action, span, domain, step));
        ui.ctx().request_repaint();
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
