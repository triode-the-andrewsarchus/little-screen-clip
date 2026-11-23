mod compute;
mod decoders;
mod equations;
mod filtering;
mod frequency;
mod measurements;

use super::{
    inputs::{Input, Signature},
    take_query_result, timeline,
};
use crate::file_manager::{logic_view, preview_copy, preview_job::Task, scope_view::plot};
use compute::{Frame, Lane};
use egui::{Color32, Context, FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::{BackgroundJobs, engineering::waveforms::display_value};
use std::{ops::Range, sync::Arc, time::Duration};

#[derive(Default)]
pub(super) struct Derived {
    equations: equations::Book,
    decoders: decoders::Decoders,
    frequency: frequency::Frequency,
    measurements: measurements::View,
    program: String,
    program_rate: Option<f64>,
    history_span: Option<Range<f64>>,
    selected: Option<Range<f64>>,
    reveal_equation: Option<u64>,
    revision: u64,
    request_after: f64,
    pending: Option<(Key, Task<Result<Arc<Frame>, String>>)>,
    accepted: Option<(Key, Result<Arc<Frame>, String>)>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SessionSettings {
    equations: Vec<equations::Equation>,
    sample_rate: Option<f64>,
    history_span: Option<Range<f64>>,
    selected: Option<Range<f64>>,
    frequency: frequency::SavedFrequency,
    measurements: measurements::SavedMeasurements,
    decoders: logic_view::protocols::SessionSettings,
}

impl SessionSettings {
    pub(super) fn equation_count(&self) -> usize {
        self.equations.len()
    }
}

#[derive(Clone, PartialEq)]
struct Key {
    revision: u64,
    inputs: Vec<Signature>,
    span: Range<f64>,
}

impl Derived {
    pub(super) fn session_settings(&self) -> SessionSettings {
        SessionSettings {
            equations: self.equations.rows.clone(),
            sample_rate: self.program_rate,
            history_span: self.history_span.clone(),
            selected: self.selected.clone(),
            frequency: self.frequency.save(),
            measurements: self.measurements.save(),
            decoders: self.decoders.save(),
        }
    }

    pub(super) fn restore_session(&mut self, state: SessionSettings) -> Result<(), String> {
        if state
            .sample_rate
            .is_some_and(|rate| !(1e-6..=1e15).contains(&rate))
            || state
                .history_span
                .as_ref()
                .is_some_and(|span| span.is_empty())
        {
            return Err("Invalid saved equation clock or history interval".into());
        }
        let count = state.equations.len();
        self.program = self.equations.restore(state.equations)?;
        self.program_rate = state.sample_rate;
        self.history_span = state.history_span;
        self.selected = state.selected.filter(|span| !span.is_empty());
        self.frequency.restore(state.frequency, count)?;
        self.measurements.restore(state.measurements, count);
        self.decoders.restore(state.decoders)?;
        self.invalidate();
        Ok(())
    }

    pub(super) fn open(&self) -> bool {
        self.equations.open()
    }

    pub(super) fn calculating(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn button(&mut self, ui: &mut Ui) {
        if ui
            .add_enabled(!self.equations.editing(), egui::Button::new("Add equation"))
            .clicked()
        {
            self.equations.add();
        }
    }

    fn rate_controls(&mut self, ui: &mut Ui) {
        let before = self.program_rate;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Derived lanes");
            self.button(ui);
            let mut custom = self.program_rate.is_some();
            if ui.checkbox(&mut custom, "Set sample rate").changed() {
                self.program_rate = custom.then_some(100_000.0);
            }
            if let Some(rate) = &mut self.program_rate {
                let speed = (*rate * 0.005).max(1.0);
                ui.add(
                    egui::DragValue::new(rate)
                        .range(1e-6..=1e15)
                        .speed(speed)
                        .suffix(" Hz")
                        .update_while_editing(false),
                );
            } else {
                ui.weak("Highest input rate");
            }
            if let Some(history) = &self.history_span {
                ui.weak(format!(
                    "History starts at {}",
                    display_value(history.start, "s")
                ));
            }
            if self.pending.is_some() {
                ui.spinner();
                ui.weak("Calculating…");
            }
        });
        if before != self.program_rate {
            self.invalidate();
        }
    }

    fn apply(&mut self, action: equations::Action, inputs: &[Input], span: &Range<f64>) -> bool {
        let bindings = inputs.iter().map(Input::binding).collect::<Vec<_>>();
        let Some(change) = self.equations.apply(action, &bindings) else {
            return false;
        };
        self.program = change.program;
        if change.requires_history {
            self.history_span.get_or_insert_with(|| span.clone());
        } else {
            self.history_span = None;
        }
        self.frequency.remap(&change.lanes);
        self.measurements.remap(&change.lanes);
        self.decoders.remap(&change.digital);
        self.reveal_equation = change.reveal;
        self.invalidate();
        true
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.accepted = None;
        self.pending = None;
        self.request_after = 0.0;
    }

    pub(super) fn update(&mut self, ctx: &Context, inputs: &[Input], span: &Range<f64>) {
        self.poll();
        if self.program.is_empty() || self.pending.is_some() {
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
        shared: Option<&Range<f64>>,
    ) -> Option<f64> {
        if !self.open() {
            return None;
        }
        self.rate_controls(ui);
        let returned = super::frequency_plot::origin_controls(
            ui,
            &mut self.frequency.spectra,
            span,
            Default::default(),
        );
        self.update(ui.ctx(), inputs, span);
        let frame = self.current_frame(ui, inputs);
        if let Some(frame) = &frame {
            self.frequency.update_filters(ui, frame);
            let region = self.selected.clone().unwrap_or_else(|| span.clone());
            self.measurements
                .ui(ui, frame, region, self.selected.is_some());
            self.decoders.controls(ui, frame, span);
        }
        let annotations = frame.is_some() && self.decoders.active() && !self.frequency.only();
        let band = annotations.then(|| {
            ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover())
                .0
        });
        let (mut plots, action) = self.equation_plots(ui, inputs, frame.as_deref());
        if let Some(action) = action {
            if self.apply(action, inputs, span) {
                ui.ctx().request_repaint();
                return None;
            }
            ui.ctx().request_repaint();
        }
        let frame = frame?;
        let pointed = self.time_plot(
            ui, &mut plots, &frame, span, domain, probe, reference, band, shared,
        );
        let reveal = self.frequency.take_reveal();
        let spectrum = ui.scope(|ui| {
            self.frequency
                .content(ui, &frame, self.selected.clone(), span)
        });
        if reveal || returned {
            spectrum.response.scroll_to_me(None);
        }
        if let Some(window) = spectrum.inner {
            self.frequency
                .spectra
                .inspect(window, span, domain, frame.step);
            ui.ctx().request_repaint();
        }
        pointed
    }

    fn current_frame(&self, ui: &mut Ui, inputs: &[Input]) -> Option<Arc<Frame>> {
        let (key, result) = self.accepted.as_ref()?;
        if key.inputs.len() != inputs.len()
            || key
                .inputs
                .iter()
                .zip(inputs)
                .any(|(old, input)| *old != input.signature())
        {
            return None;
        }
        match result {
            Ok(frame) => Some(frame.clone()),
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                None
            }
        }
    }

    fn equation_plots(
        &mut self,
        ui: &mut Ui,
        inputs: &[Input],
        frame: Option<&Frame>,
    ) -> (Vec<TracePlot>, Option<equations::Action>) {
        let mut plots = Vec::new();
        let mut action = None;
        let show_time = frame.is_some() && !self.frequency.only();
        for index in 0..self.equations.rows.len() {
            let id = self.equations.rows[index].id;
            let row = ui.push_id(id, |ui| {
                let equation = &self.equations.rows[index];
                let editing = self.equations.editing();
                let (edit, remove) = ui
                    .horizontal_wrapped(|ui| {
                        ui.colored_label(plot::color(index + 1), &equation.name);
                        if frame.is_some() {
                            self.frequency.button(ui, index);
                        }
                        let edit = ui
                            .add_enabled(!editing, egui::Button::new("Edit").small())
                            .clicked();
                        let remove = ui
                            .add_enabled(!editing, egui::Button::new("Remove").small())
                            .clicked();
                        (edit, remove)
                    })
                    .inner;
                ui.add(egui::Label::new(egui::RichText::new(&equation.text).monospace()).wrap());
                if edit {
                    self.equations.edit(index);
                }
                if remove {
                    action = Some(equations::Action::Remove(id));
                }
                if let Some(error) = self.equations.row_error(id) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                let response = self.equations.editor(ui, Some(id), inputs);
                action = response.or(action.take());
                if show_time {
                    plots.push(TracePlot::allocate(ui));
                }
            });
            if frame.is_some() && self.reveal_equation == Some(id) {
                row.response.scroll_to_me(Some(egui::Align::Min));
                self.reveal_equation = None;
            }
        }
        let response = self.equations.editor(ui, None, inputs);
        (plots, response.or(action))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "plot geometry and shared timeline"
    )]
    fn time_plot(
        &mut self,
        ui: &mut Ui,
        plots: &mut Vec<TracePlot>,
        frame: &Frame,
        span: &mut Range<f64>,
        domain: &Range<f64>,
        probe: Option<f64>,
        reference: Option<f64>,
        annotations: Option<Rect>,
        shared: Option<&Range<f64>>,
    ) -> Option<f64> {
        if self.frequency.only() || plots.is_empty() {
            return None;
        }
        let filtered = self.frequency.filtered_lanes(frame);
        for lane in &filtered {
            ui.strong(&lane.name);
            plots.push(TracePlot::allocate(ui));
        }
        let rect = plots.first()?.rect.union(plots.last()?.rect);
        let mut pointed = None;
        for plot in plots.iter() {
            Self::select_interval(ui, &plot.response, span, &mut self.selected);
            if let Some(requested) =
                timeline::navigate(ui.ctx(), plot.rect, &plot.response, span, true)
            {
                *span = timeline::fit(domain, requested, frame.step);
                ui.ctx().request_repaint();
            }
            if let Some(point) = plot.response.hover_pos() {
                pointed = Some(plot::at(plot.rect, span, point.x));
            }
        }
        let cursor = pointed.or(self.decoders.cursor());
        frame.paint(
            ui,
            plots,
            span,
            self.selected.as_ref(),
            &filtered,
            cursor.or(probe),
            reference,
        );
        for plot in plots.iter().filter(|plot| ui.is_rect_visible(plot.rect)) {
            self.measurements.paint(ui.painter(), plot.rect, span);
            timeline::paint_shared(ui.painter(), plot.rect, span, shared);
        }
        if let Some(index) = self.frequency.origin_lane()
            && let Some(plot) = plots.get(index)
        {
            let painter = ui
                .painter()
                .with_clip_rect(plot.rect.intersect(ui.clip_rect()));
            super::frequency_plot::paint_origin(
                ui,
                &painter,
                plot.rect,
                span,
                &self.frequency.spectra,
            );
        }
        if self.measurements.take_reveal() {
            ui.scroll_to_rect(rect, Some(egui::Align::Max));
        }
        if let Some(band) = annotations {
            let band = Rect::from_min_max(
                pos2(rect.left(), band.top()),
                pos2(rect.right(), band.bottom()),
            );
            self.decoders.paint(
                ui,
                ui.painter(),
                band,
                plots.iter().map(|plot| plot.rect),
                span,
            );
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

struct TracePlot {
    rect: Rect,
    response: Response,
}

impl TracePlot {
    fn allocate(ui: &mut Ui) -> Self {
        let (area, reserved) =
            ui.allocate_exact_size(vec2(ui.available_width(), 65.0), Sense::hover());
        let rect = Rect::from_min_max(area.min + vec2(78.0, 7.0), area.max - vec2(9.0, 7.0));
        let response = ui.interact(
            rect,
            reserved.id.with("derived-time"),
            Sense::click_and_drag(),
        );
        Self { rect, response }
    }
}

impl Frame {
    #[expect(
        clippy::too_many_arguments,
        reason = "plot rows share a timeline and selection"
    )]
    fn paint(
        &self,
        ui: &mut Ui,
        plots: &[TracePlot],
        span: &Range<f64>,
        selected: Option<&Range<f64>>,
        filtered: &[filtering::PlotLane<'_>],
        probe: Option<f64>,
        reference: Option<f64>,
    ) {
        let last = plots.last().expect("nonempty plot rows").rect;
        ui.allocate_space(vec2(0.0, 14.0));
        let painter = ui.painter();
        let rects = plots
            .iter()
            .map(|plot| plot.rect)
            .filter(|rect| ui.is_rect_visible(*rect));
        for rect in rects.clone() {
            painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        }
        let ink = ui.visuals().weak_text_color();
        plot::x_axis_with_grid(
            painter,
            last,
            span,
            "s",
            ink,
            rects.map(|rect| rect.y_range()),
        );
        let recorded = self
            .lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| (Some(lane), plot::color(index + 1)));
        let filtered = filtered
            .iter()
            .map(|lane| (lane.values, super::filter_editor::FILTER_COLOR));
        let probe = probe.filter(|time| span.contains(time));
        for (row, (lane, color)) in plots.iter().zip(recorded.chain(filtered)) {
            let rect = row.rect;
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let clipped = painter.with_clip_rect(rect);
            if let Some(selected) = selected {
                plot::highlight(
                    &clipped,
                    rect,
                    span,
                    selected.clone(),
                    plot::color(1).gamma_multiply(0.22),
                );
            }
            if let Some(lane) = lane {
                if let Lane::Analog {
                    channel, bounds, ..
                } = lane
                {
                    plot::paint_y_scale(painter, rect, *bounds, &channel.unit, color);
                }
                lane.paint(&clipped, rect, span, &self.span, color);
            }
            if let Some(time) = probe {
                let x = plot::x_at(rect, span, time);
                clipped.vline(x, rect.y_range(), Stroke::new(1.0, ink));
                if let Some(value) = lane
                    .and_then(|lane| lane.value(time))
                    .filter(|value| !value.is_empty())
                {
                    let galley = painter.layout_no_wrap(value, FontId::monospace(11.0), color);
                    let left = (x + 4.0)
                        .min(rect.right() - galley.size().x - 6.0)
                        .max(rect.left());
                    let label = Rect::from_min_size(
                        pos2(left, rect.top() + 3.0),
                        galley.size() + vec2(6.0, 4.0),
                    );
                    clipped.rect_filled(label, 3.0, ui.visuals().window_fill());
                    clipped.galley(label.min + vec2(3.0, 2.0), galley, Color32::PLACEHOLDER);
                }
            }
            timeline::paint_reference(&clipped, rect, span, reference);
        }
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
