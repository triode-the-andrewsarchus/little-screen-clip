mod controls;
mod input;
pub(in crate::file_manager) use controls::Settings;

pub(in crate::file_manager) use input::ThresholdInput;

use super::{JOBS, PlottedChannel, Task, display_value, plot, timeline};
use crate::file_manager::{
    logic_view::{
        plot as logic_plot,
        protocols::{EventAction, Panel, TraceInput, event_label},
    },
    signal_view::take_query_result,
};
use controls::ThresholdDraft;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, vec2};
use input::Configuration;
use little_clip_core::engineering::{
    analysis::Clock,
    logic::{Level, Trace},
    logic_decode::Event,
    waveforms::Recording,
};
use std::{ops::Range, sync::Arc, time::Duration};

const PREVIEW_EDGES: usize = 4096;

type Rows = Vec<(usize, Trace)>;

#[derive(Clone, PartialEq)]
struct Window {
    span: Range<f64>,
    revision: u64,
    shown: Vec<usize>,
}

pub(super) struct Digital {
    config: Arc<Configuration>,
    shown: Vec<bool>,
    open: bool,
    threshold_draft: Option<ThresholdDraft>,
    panel: Panel,
    pending: Option<(Window, Task<Result<Rows, String>>)>,
    ready: Option<(Window, Result<Rows, String>)>,
    retry_at: f64,
    cursor: Option<f64>,
}

impl Digital {
    pub(super) fn new(recording: &Recording) -> Self {
        let config = Arc::new(Configuration::new(recording));
        let shown = (0..config.lanes.len()).map(|index| index < 2).collect();
        Self {
            config,
            shown,
            open: false,
            threshold_draft: None,
            panel: Panel::default(),
            pending: None,
            ready: None,
            retry_at: 0.0,
            cursor: None,
        }
    }

    pub(super) fn input(&self, recording: Arc<Recording>) -> Arc<ThresholdInput> {
        Arc::new(ThresholdInput::new(recording, self.config.clone()))
    }

    pub(super) fn cursor(&self) -> Option<f64> {
        self.open.then_some(self.cursor).flatten()
    }

    pub(super) fn suspend(&mut self) {
        self.pending = None;
        self.panel.suspend_all();
    }

    fn event(&self, span: &Range<f64>) -> Option<&Event> {
        self.panel.cursor_event(span, Some(self.cursor()?))
    }

    fn event_inputs(&self, span: &Range<f64>) -> Option<(Range<f64>, Vec<usize>)> {
        let event = self.event(span)?;
        Some((event.time.clone(), self.panel.channels()?))
    }

    pub(super) fn highlight_analog_inputs(
        &self,
        painter: &egui::Painter,
        span: &Range<f64>,
        channels: &[PlottedChannel],
        stacked: bool,
    ) {
        if let Some((time, inputs)) = self.event_inputs(span) {
            let limit = if stacked { channels.len() } else { 1 };
            for channel in channels
                .iter()
                .filter(|lane| inputs.contains(&lane.index))
                .take(limit)
            {
                logic_plot::highlight_event(painter, channel.plot, span, time.clone());
            }
        }
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        recording: &Arc<Recording>,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        clock: Clock,
    ) {
        if !self.open {
            self.suspend();
            return;
        }
        if self.panel.controls(ui, &self.config.lanes) {
            self.cursor = None;
        }
        self.panel.return_controls(ui, span);
        let input = self.input(recording.clone());
        self.panel
            .update_all_materialized(ui.ctx(), span, input.revision(), input.clone());
        self.update_preview(ui, span, input);
        self.lanes(ui, recording, span, domain, clock);
        if let Some((index, action)) = self.annotations(ui, span) {
            self.panel.select_decoder(Some(index));
            self.cursor = Some(self.panel.follow_event(action, span, domain, 0.0));
            ui.ctx().request_repaint();
        }
        if let Some(event) = self.event(span) {
            let time = clock.workspace_range(&event.time);
            let duration = (event.time.end - event.time.start) * clock.scale();
            ui.horizontal_wrapped(|ui| {
                ui.strong(event_label(event.kind));
                ui.monospace(format!(
                    "{} – {}",
                    display_value(time.start, "s"),
                    display_value(time.end, "s"),
                ));
                ui.weak(format!("Duration {}", display_value(duration, "s")));
            });
        }
    }

    pub(super) fn table(
        &mut self,
        ui: &mut Ui,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        probe: Option<f64>,
        clock: Clock,
    ) {
        if !self.open {
            return;
        }
        if let Some(action) = self
            .panel
            .table(ui, span, probe.or(self.cursor), None, clock)
        {
            self.cursor = Some(self.panel.follow_event(action, span, domain, 0.0));
            ui.ctx().request_repaint();
        }
    }

    fn update_preview(&mut self, ui: &Ui, span: &Range<f64>, input: Arc<ThresholdInput>) {
        if let Some(output) = take_query_result(&mut self.pending) {
            self.ready = Some(output);
        }
        let key = Window {
            span: span.clone(),
            revision: self.config.revision,
            shown: self
                .shown
                .iter()
                .enumerate()
                .filter_map(|(index, &shown)| shown.then_some(index))
                .collect(),
        };
        if key.shown.is_empty() || self.ready.as_ref().is_some_and(|(old, _)| *old == key) {
            return;
        }
        if self.pending.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
            return;
        }
        let now = ui.input(|input| input.time);
        if self.retry_at > now {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.retry_at - now));
            return;
        }
        self.retry_at = now + 0.075;
        let repaint = ui.ctx().clone();
        let query = key.clone();
        let task = Task::start(&JOBS, Duration::from_secs(10), move |cancel| {
            let result = query
                .shown
                .into_iter()
                .map(|index| {
                    let trace =
                        input.read_edges(index, query.span.clone(), PREVIEW_EDGES, cancel)?;
                    Ok((index, trace))
                })
                .collect();
            repaint.request_repaint();
            result
        });
        self.pending = Some((key, task));
    }

    fn lanes(
        &mut self,
        ui: &mut Ui,
        recording: &Recording,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        clock: Clock,
    ) {
        let mut next_span = None;
        let mut wired = None;
        let event = self.event_inputs(span);
        for index in 0..self.config.lanes.len() {
            let result = ui
                .push_id(("scope-digital", index), |ui| {
                    self.lane_tools(ui, recording, index);
                    let time = event.as_ref().and_then(|(time, inputs)| {
                        (self.cursor.is_some() && inputs.contains(&index)).then_some(time)
                    });
                    self.shown[index].then(|| self.lane_plot(ui, index, span, clock, time))
                })
                .inner;
            if let Some((next, pick)) = result {
                next_span = next.or(next_span);
                if pick {
                    wired = Some(index);
                }
            }
        }
        match self.visible_window() {
            Some((_, Err(error))) => {
                ui.weak(error);
            }
            None if self.pending.is_some() => {
                ui.weak("Reading digital transitions…");
            }
            _ => {}
        }
        if let Some(index) = wired {
            self.panel.wire_lane(index);
            self.cursor = None;
            ui.ctx().request_repaint();
        }
        if let Some(next) = next_span {
            *span = timeline::fit(domain, next, 0.0);
            ui.ctx().request_repaint();
        }
    }

    fn visible_window(&self) -> Option<&(Window, Result<Rows, String>)> {
        self.ready
            .as_ref()
            .filter(|(key, _)| key.revision == self.config.revision)
    }

    fn lane_plot(
        &self,
        ui: &mut Ui,
        index: usize,
        span: &Range<f64>,
        clock: Clock,
        event: Option<&Range<f64>>,
    ) -> (Option<Range<f64>>, bool) {
        let (area, rect, response) = row(ui, ("threshold", index));
        if !ui.is_rect_visible(area) {
            return (None, false);
        }
        let lane = &self.config.lanes[index];
        let ink = plot::color(index);
        let painter = ui.painter_at(area);
        if let Some(time) = event {
            logic_plot::highlight_event(&painter, rect, span, time.clone());
        }
        let pick = if self.panel.wiring_active() {
            logic_plot::wire_label(ui, &painter, rect, &lane.name, index)
        } else {
            false
        };
        let trace = self.visible_window().and_then(|(key, output)| {
            let (_, trace) = output
                .as_ref()
                .ok()?
                .iter()
                .find(|(channel, _)| *channel == index)?;
            let from = key.span.start.max(lane.start_seconds);
            Some((from..trace.inspected_until, trace))
        });
        if let Some((time, trace)) = trace.filter(|(time, _)| !time.is_empty()) {
            paint_threshold_trace(
                &painter,
                &response,
                span,
                (&time, trace),
                &lane.name,
                clock,
                ink,
            );
        }
        (
            timeline::navigate(ui.ctx(), rect, &response, span, true),
            pick,
        )
    }

    fn annotations(&self, ui: &mut Ui, span: &Range<f64>) -> Option<(usize, EventAction)> {
        let mut picked = None;
        let selected = self.panel.picked_event(span);
        for (index, name, events) in self.panel.annotation_rows(span) {
            let (area, _, response) = row(ui, ("scope-decode", index));
            let painter = ui.painter_at(area);
            label(&painter, area, name, ui.visuals().weak_text_color());
            let selected = (self.panel.selected_index() == Some(index))
                .then_some(selected)
                .flatten();
            if let Some(action) =
                logic_plot::annotations(ui, &painter, response, span, events, selected)
            {
                picked = Some((index, action));
            }
        }
        picked
    }
}

fn paint_threshold_trace(
    painter: &egui::Painter,
    response: &egui::Response,
    span: &Range<f64>,
    (time, trace): (&Range<f64>, &Trace),
    name: &str,
    clock: Clock,
    color: Color32,
) {
    let rect = response.rect;
    let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    logic_plot::draw_trace(&clipped, rect, span, time, trace, color);
    let Some(point) = response.hover_pos() else {
        return;
    };
    let at = plot::at(rect, span, point.x);
    if !time.contains(&at) {
        return;
    }
    let edge = trace.edges.partition_point(|edge| edge.time_seconds <= at);
    let level = edge
        .checked_sub(1)
        .map(|edge| trace.edges[edge].level)
        .or(trace.initial);
    let time = super::display_value(clock.to_workspace(at), "s");
    response
        .clone()
        .on_hover_text(format!("{name} · {time} · {}", level_label(level)));
}

fn row(ui: &mut Ui, id: impl egui::AsIdSalt) -> (Rect, Rect, egui::Response) {
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), 23.0), Sense::hover());
    let rect = Rect::from_min_max(area.min + vec2(78.0, 0.0), area.max - vec2(9.0, 0.0));
    let response = ui.interact(rect, ui.make_persistent_id(id), Sense::click_and_drag());
    ui.painter()
        .rect_filled(area, 2.0, ui.visuals().faint_bg_color);
    (area, rect, response)
}

fn label(painter: &egui::Painter, area: Rect, label: &str, color: Color32) {
    let gutter = Rect::from_min_size(area.min, vec2(77.0, area.height()));
    painter
        .with_clip_rect(gutter.intersect(painter.clip_rect()))
        .text(
            gutter.left_center() + vec2(6.0, 0.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(11.0),
            color,
        );
}

fn level_label(level: Option<Level>) -> &'static str {
    match level {
        Some(Level::High) => "High",
        Some(Level::Low) => "Low",
        Some(Level::HighImpedance) => "Floating band",
        Some(Level::Unknown) => "Unknown",
        None => "No sample",
    }
}
