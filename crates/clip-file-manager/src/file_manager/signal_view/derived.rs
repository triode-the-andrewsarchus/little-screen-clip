mod compute;
mod decoders;
mod filtering;
mod frequency;
mod measurements;

use super::{
    inputs::{Input, Signature},
    take_query_result, timeline,
};
use crate::file_manager::{logic_view, preview_copy, preview_job::Task, scope_view::plot};
use compute::{Frame, Lane};
use egui::{Align2, Color32, Context, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::{
    BackgroundJobs,
    engineering::{analysis::SignalGraph, waveforms::display_value},
};
use std::{ops::Range, sync::Arc, time::Duration};

#[derive(Default)]
pub(super) struct Derived {
    editor: bool,
    decoders: decoders::Decoders,
    frequency: frequency::Frequency,
    measurements: measurements::View,
    draft: String,
    draft_error: Option<String>,
    rate: Option<f64>,
    program: String,
    program_rate: Option<f64>,
    history_span: Option<Range<f64>>,
    selected: Option<Range<f64>>,
    show_output: bool,
    revision: u64,
    request_after: f64,
    pending: Option<(Key, Task<Result<Arc<Frame>, String>>)>,
    accepted: Option<(Key, Result<Arc<Frame>, String>)>,
}

#[derive(Clone, PartialEq)]
struct Key {
    revision: u64,
    inputs: Vec<Signature>,
    span: Range<f64>,
}

impl Derived {
    pub(super) fn active(&self) -> bool {
        !self.program.is_empty()
    }

    pub(super) fn editor_open(&self) -> bool {
        self.editor
    }

    pub(super) fn button(&mut self, ui: &mut Ui) {
        if ui.selectable_label(self.editor, "Derived lanes").clicked() {
            self.editor = !self.editor;
        }
    }

    pub(super) fn editor(&mut self, ui: &Ui, inputs: &[Input], span: &Range<f64>) {
        if !self.editor {
            return;
        }
        let mut open = self.editor;
        egui::Window::new("Derived lanes")
            .id(egui::Id::unique("signal-derived-lane-editor"))
            .open(&mut open)
            .default_width(490.0)
            .show(ui.ctx(), |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.draft)
                        .id_salt("signal-equation-editor")
                        .font(egui::TextStyle::Monospace)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(5)
                        .char_limit(12_000)
                        .hint_text("name = expression"),
                ).on_hover_text("Use [n-1] for the previous sample. Declare the unit for feedback, for example smooth: V = 0.9 * smooth[n-1] + 0.1 * s1_c1");
                ui.collapsing("Signal inputs", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            ui.spacing_mut().interact_size.y = 22.0;
                            for input in inputs {
                                ui.horizontal(|ui| {
                                    if ui
                                        .small_button(&input.alias)
                                        .on_hover_text("Append to equation")
                                        .clicked()
                                    {
                                        let remaining =
                                            12_000usize.saturating_sub(self.draft.chars().count());
                                        self.draft.extend(input.alias.chars().take(remaining));
                                    }
                                    ui.label(&input.label);
                                    if input.is_awaiting_filter() {
                                        ui.weak("Awaiting filtered samples");
                                    }
                                });
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    let mut custom = self.rate.is_some();
                    if ui.checkbox(&mut custom, "Set sample rate").changed() {
                        self.rate = custom.then_some(100_000.0);
                    }
                    if let Some(rate) = &mut self.rate {
                        let speed = (*rate * 0.005).max(1.0);
                        ui.add(
                            egui::DragValue::new(rate)
                                .range(1e-6..=1e15)
                                .speed(speed)
                                .suffix(" Hz"),
                        );
                    } else {
                        ui.weak("Highest input rate");
                    }
                    if ui.button("Apply").clicked() {
                        self.apply(inputs, span);
                    }
                });
                if let Some(history) = &self.history_span {
                    ui.weak(format!(
                        "History starts at {} · Declared lanes start at zero",
                        display_value(history.start, "s")
                    ));
                }
                if let Some(error) = self.draft_error.as_ref().or_else(|| {
                    self.accepted
                        .as_ref()
                        .and_then(|(_, result)| result.as_ref().err())
                }) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            });
        self.editor = open;
    }

    fn apply(&mut self, inputs: &[Input], span: &Range<f64>) {
        let program = self.draft.trim();
        let bindings = inputs.iter().map(Input::binding).collect::<Vec<_>>();
        self.history_span = if program.is_empty() {
            None
        } else {
            match SignalGraph::compile(program, &bindings, 0) {
                Ok(graph) => graph.requires_history().then(|| span.clone()),
                Err(error) => {
                    self.draft_error = Some(error.to_string());
                    return;
                }
            }
        };
        self.draft_error = None;
        self.program = program.to_owned();
        self.program_rate = self.rate;
        self.revision = self.revision.wrapping_add(1);
        self.show_output = self.active();
        self.accepted = None;
        self.pending = None;
        self.request_after = 0.0;
        self.decoders = decoders::Decoders::default();
        self.frequency = frequency::Frequency::default();
        self.measurements.reset();
        self.selected = None;
    }

    pub(super) fn update(&mut self, ctx: &Context, inputs: &[Input], span: &Range<f64>) {
        self.poll();
        if !self.active() || self.pending.is_some() {
            return;
        }
        let span = self.history_span.as_ref().unwrap_or(span);
        let key = Key {
            revision: self.revision,
            inputs: inputs.iter().map(Input::signature).collect(),
            span: span.clone(),
        };
        if self
            .accepted
            .as_ref()
            .is_some_and(|(previous, _)| previous == &key)
        {
            return;
        }
        let now = ctx.input(|input| input.time);
        if now < self.request_after {
            ctx.request_repaint_after(Duration::from_secs_f64(self.request_after - now));
            return;
        }
        self.request_after = now + 0.12;
        let input = inputs.to_owned();
        let span = span.clone();
        let program = self.program.clone();
        let rate = self.program_rate;
        let repaint = ctx.clone();
        let task = Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(20),
            move |cancel| {
                let output = compute::calculate(&program, &input, span, rate, cancel).map(Arc::new);
                repaint.request_repaint();
                output
            },
        );
        self.pending = Some((key, task));
        ctx.request_repaint_after(Duration::from_millis(35));
    }

    fn poll(&mut self) {
        if let Some(result) = take_query_result(&mut self.pending) {
            self.accepted = Some(result);
        }
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        inputs: &[Input],
        probe: Option<f64>,
        reference: Option<f64>,
    ) -> Option<f64> {
        if !self.active() {
            return None;
        }
        self.update(ui.ctx(), inputs, span);
        let title = ui
            .horizontal(|ui| {
                ui.strong("Derived lanes");
                if self.pending.is_some() {
                    ui.spinner();
                    ui.weak("Calculating…");
                }
            })
            .response;
        if std::mem::take(&mut self.show_output) {
            title.scroll_to_me(Some(egui::Align::Center));
        }
        let (key, result) = self.accepted.as_ref()?;
        if key
            .inputs
            .iter()
            .zip(inputs)
            .any(|(old, input)| *old != input.signature())
            || key.inputs.len() != inputs.len()
        {
            return None;
        }
        let frame = match result {
            Ok(frame) => frame.clone(),
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return None;
            }
        };
        self.frequency.controls(ui, &frame);
        let pointed = if self.frequency.only() {
            None
        } else {
            let region = self.selected.clone().unwrap_or_else(|| span.clone());
            self.measurements
                .ui(ui, &frame, region, self.selected.is_some());
            self.decoders.controls(ui, &frame, span);
            self.time_plot(ui, &frame, span, domain, probe, reference)
        };
        if let Some(interval) = self
            .frequency
            .content(ui, &frame, self.selected.clone(), span)
        {
            *span = timeline::fit(domain, interval, frame.step);
            ui.ctx().request_repaint();
        }
        pointed
    }

    fn time_plot(
        &mut self,
        ui: &mut Ui,
        frame: &Frame,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        probe: Option<f64>,
        reference: Option<f64>,
    ) -> Option<f64> {
        let annotations = if self.decoders.active() { 24.0 } else { 3.0 };
        let filtered = self.frequency.filtered_lanes(frame);
        let lane_count = frame.lanes.len() + filtered.len();
        let height = (lane_count as f32 * 48.0).max(100.0) + annotations;
        let (area, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), height + 17.0), Sense::hover());
        let rect = Rect::from_min_max(
            area.min + vec2(78.0, annotations),
            area.max - vec2(9.0, 17.0),
        );
        let response = ui.interact(
            rect,
            ui.make_persistent_id("derived signals"),
            Sense::click_and_drag(),
        );
        Self::select_interval(ui, &response, span, &mut self.selected);
        if let Some(requested) = timeline::navigate(ui.ctx(), rect, &response, span, true) {
            *span = timeline::fit(domain, requested, frame.step);
            ui.ctx().request_repaint();
        }
        let pointed = response
            .hover_pos()
            .filter(|p| rect.contains(*p))
            .map(|p| plot::at(rect, span, p.x));
        let cursor = pointed.or(self.decoders.cursor());
        frame.paint(
            ui,
            rect,
            span,
            self.selected.as_ref(),
            &filtered,
            cursor.or(probe),
            reference,
        );
        self.measurements.paint(ui.painter(), rect, span);
        if self.measurements.take_reveal() {
            ui.scroll_to_rect(area, Some(egui::Align::Max));
        }
        if self.decoders.active() {
            let band = Rect::from_min_max(pos2(rect.left(), area.top()), rect.right_top());
            self.decoders.paint(ui, ui.painter(), band, rect, span);
            self.decoders.table(ui, span, pointed);
        }
        pointed.or(self.decoders.cursor())
    }

    fn select_interval(
        ui: &Ui,
        response: &egui::Response,
        visible: &Range<f64>,
        selected: &mut Option<Range<f64>>,
    ) {
        let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
        let (_, drag) = preview_copy::drag_range(ui, response, response.id, modified);
        if let Some((left, right)) = drag {
            let width = visible.end - visible.start;
            let start = width.mul_add(f64::from(left), visible.start);
            let end = width.mul_add(f64::from(right), visible.start);
            *selected = (start < end).then_some(start..end);
        } else if response.secondary_clicked() {
            *selected = None;
        }
        if let Some(selected) = selected.as_ref() {
            response.clone().on_hover_text(format!(
                "{} selected · Right-click to clear",
                display_value(selected.end - selected.start, "s")
            ));
        } else {
            response
                .clone()
                .on_hover_text("Drag to select a time range · Scroll to zoom");
        }
    }
}

impl Frame {
    fn paint(
        &self,
        ui: &Ui,
        rect: Rect,
        span: &Range<f64>,
        selected: Option<&Range<f64>>,
        filtered: &[filtering::PlotLane<'_>],
        probe: Option<f64>,
        reference: Option<f64>,
    ) {
        let ink = ui.visuals().weak_text_color();
        let painter = ui.painter();
        let clipped = painter.with_clip_rect(rect.intersect(ui.clip_rect()));
        clipped.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        plot::axes(painter, rect, span, ink);
        if let Some(selected) = selected {
            plot::highlight(
                &clipped,
                rect,
                span,
                selected.clone(),
                plot::color(1).gamma_multiply(0.22),
            );
        }
        let count = self.lanes.len() + filtered.len();
        let recorded = self
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| (lane.name(), Some(lane), plot::color(index + 1)));
        let filtered = filtered.iter().map(|lane| {
            (
                lane.name.as_str(),
                lane.values,
                super::filter_editor::FILTER_COLOR,
            )
        });
        for (index, (name, lane, color)) in recorded.chain(filtered).enumerate() {
            let row = plot::lane(rect, index, count);
            if let Some(lane) = lane {
                lane.paint(
                    &clipped,
                    row.shrink2(vec2(0.0, 4.0)),
                    span,
                    &self.span,
                    color,
                );
            }
            clipped.hline(
                row.x_range(),
                row.bottom(),
                Stroke::new(0.7, ink.gamma_multiply(0.3)),
            );
            painter.text(
                pos2(row.left() - 5.0, row.center().y),
                Align2::RIGHT_CENTER,
                name,
                FontId::monospace(10.0),
                color,
            );
            if let Some(time) = probe.filter(|time| span.contains(time)) {
                let x = plot::x_at(row, span, time);
                let value = lane
                    .and_then(|lane| lane.value(time))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "—".into());
                let galley = painter.layout_no_wrap(value, FontId::monospace(11.0), color);
                let label_x = (x + 4.0)
                    .min(row.right() - galley.size().x - 4.0)
                    .max(row.left());
                let label = Rect::from_min_size(
                    pos2(label_x, row.top() + 3.0),
                    galley.size() + vec2(6.0, 4.0),
                );
                clipped.rect_filled(label, 3.0, ui.visuals().window_fill());
                clipped.galley(label.min + vec2(3.0, 2.0), galley, Color32::PLACEHOLDER);
            }
        }
        if let Some(time) = probe.filter(|time| span.contains(time)) {
            clipped.vline(
                plot::x_at(rect, span, time),
                rect.y_range(),
                Stroke::new(1.0, ink),
            );
        }
        timeline::paint_reference(&clipped, rect, span, reference);
    }
}

impl Lane {
    fn paint(
        &self,
        painter: &egui::Painter,
        row: Rect,
        visible: &Range<f64>,
        captured: &Range<f64>,
        color: Color32,
    ) {
        match self {
            Self::Analog {
                channel,
                raw,
                envelope,
                bounds,
            } => {
                if raw.samples.len() <= 4096 {
                    plot::draw_raw(
                        painter,
                        row,
                        visible,
                        channel.start_seconds,
                        channel.interval_seconds,
                        &raw.samples,
                        *bounds,
                        color,
                    );
                } else {
                    plot::draw_envelope(painter, row, visible, channel, envelope, *bounds, color);
                }
            }
            Self::Digital {
                trace, envelope, ..
            } => {
                if trace.edges.len() <= 4096 {
                    logic_view::plot::draw_trace(painter, row, visible, captured, trace, color);
                } else {
                    logic_view::plot::draw_envelope(painter, row, visible, envelope, color);
                }
            }
        }
    }

    fn value(&self, time: f64) -> Option<String> {
        match self {
            Self::Analog { channel, raw, .. } => raw
                .value_at(time)
                .map(|value| display_value(f64::from(value), &channel.unit)),
            Self::Digital {
                trace, envelope, ..
            } => {
                if !envelope.time.contains(&time) {
                    return None;
                }
                let edge = trace
                    .edges
                    .partition_point(|edge| edge.time_seconds <= time);
                let level = edge
                    .checked_sub(1)
                    .map(|index| trace.edges[index].level)
                    .or(trace.initial)?;
                Some(logic_view::level_label(level).to_owned())
            }
        }
    }
}
