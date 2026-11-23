use super::{JOBS, Task, Viewer, plot, reads, spectrum_input::Input};
use crate::file_manager::signal_view::{
    filter_editor::{StageList, filter_menu},
    take_query_result,
};
use egui::{Color32, Painter, Rect, Stroke, Ui, vec2};
use little_clip_core::engineering::{
    analysis::FilterKind,
    scope::{Channel, MAX_SAMPLES_PER_READ},
    waveforms::{Envelope, Peak, Recording, display_value, sample_span},
};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

pub(super) use crate::file_manager::signal_view::filter_editor::FILTER_COLOR;

#[derive(Default)]
pub(super) struct Filters {
    lanes: BTreeMap<usize, Lane>,
    task: Option<(Request, Task<Result<Output, String>>)>,
    retry_at: f64,
    generation: u64,
}

struct Lane {
    name: String,
    visible: bool,
    rows: Range<u64>,
    stages: StageList,
    output: Option<Output>,
    completed: Option<Request>,
    error: Option<String>,
}

#[derive(Clone, PartialEq)]
struct Request {
    channel: usize,
    rows: Range<u64>,
    stages: StageList,
}

struct Output {
    samples: FilterSamples,
    overview: Envelope,
    bounds: Peak,
}

pub(in crate::file_manager) struct FilteredChannel<'a> {
    pub channel: usize,
    pub name: &'a str,
    pub samples: Option<FilterSamples>,
}

#[derive(Clone)]
pub(in crate::file_manager) struct FilterSamples {
    pub generation: u64,
    pub raw: Arc<reads::RawChannel>,
}

impl Lane {
    fn ready(&self) -> Option<&Output> {
        self.completed
            .as_ref()
            .filter(|key| key.rows == self.rows && key.stages == self.stages)?;
        self.output.as_ref()
    }
}

impl Viewer {
    pub(super) fn filter_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| self.filter_controls_row(ui));
    }

    fn filter_controls_row(&mut self, ui: &mut Ui) {
        let kind = ui
            .add_enabled_ui(self.can_add_filter(), |ui| {
                ui.menu_button("Add filter", filter_menu).inner.flatten()
            })
            .inner;
        if let Some(kind) = kind {
            self.add_filter(kind);
        }
        let channel = &self.overview.channels[self.active];
        let rate = channel.interval_seconds.recip();
        let rows = self
            .spectrum_interval()
            .map_or(0..channel.sample_count, |range| sample_span(channel, range));
        let Some(lane) = self.filters.lanes.get_mut(&self.active) else {
            return;
        };
        ui.add(
            egui::TextEdit::singleline(&mut lane.name)
                .id_salt(("filtered-lane-name", self.active))
                .hint_text("Lane name")
                .desired_width(110.0)
                .char_limit(48),
        );
        if ui
            .add_enabled(lane.rows != rows, egui::Button::new("Use current range"))
            .on_hover_text("Use the interval selected in the spectrum controls")
            .clicked()
        {
            lane.rows = rows;
        }
        ui.weak(format!(
            "{} to {}",
            display_value(channel.time_at(lane.rows.start), "s"),
            display_value(channel.time_at(lane.rows.end), "s")
        ));
        if let Some(error) = &lane.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        lane.stages.controls(ui, rate);
        if lane.stages.is_empty() {
            self.filters.lanes.remove(&self.active);
        }
    }

    pub(super) fn can_add_filter(&self) -> bool {
        self.filters
            .lanes
            .get(&self.active)
            .is_none_or(|lane| lane.stages.can_add())
    }

    pub(super) fn add_filter(&mut self, kind: FilterKind) {
        if !self.can_add_filter() {
            return;
        }
        let channel = &self.overview.channels[self.active];
        let rate = channel.interval_seconds.recip();
        let rows = self
            .spectrum_interval()
            .map_or(0..channel.sample_count, |range| sample_span(channel, range));
        let lane = self
            .filters
            .lanes
            .entry(self.active)
            .or_insert_with(|| Lane {
                name: format!("{} filtered", channel.name),
                visible: true,
                rows,
                stages: StageList::default(),
                output: None,
                completed: None,
                error: None,
            });
        let span = self.spectrum_span.clone().unwrap_or(0.0..rate * 0.5);
        lane.stages.add(kind, rate, &span, self.spectrum_marker);
    }

    pub(super) fn draw_filters(&self, painter: &Painter, clip: &Painter, rect: Rect, ink: Color32) {
        let first = self.physical_lanes() + usize::from(self.math_state.numeric().is_some());
        for (slot, (&index, derived)) in self.filters.visible().enumerate() {
            let lane = plot::lane(rect, first + slot, self.lane_count()).shrink2(vec2(0.0, 5.0));
            let channel = &self.overview.channels[index];
            clip.hline(
                lane.x_range(),
                lane.top(),
                Stroke::new(1.0, ink.gamma_multiply(0.32)),
            );
            let output = derived.ready();
            let status = match output {
                Some(_) => "",
                None if derived.error.is_some() => " · unavailable",
                None => " · filtering…",
            };
            if let Some(output) = output {
                draw_output(clip, lane, &self.span, channel, output);
                super::axes_y(painter, lane, output.bounds, &channel.unit, ink);
            }
            super::hover::selection_label(
                clip,
                lane,
                lane.left(),
                lane.top() + 3.0,
                &format!("{}{status}", derived.name),
                FILTER_COLOR,
                Color32::from_black_alpha(210),
            );
        }
    }

    pub(super) fn draw_filter_response(&mut self, ui: &mut Ui, rect: Rect, span: &Range<f64>) {
        let Some(lane) = self.filters.lanes.get_mut(&self.active) else {
            return;
        };
        let rate = self.overview.channels[self.active].interval_seconds.recip();
        lane.stages.draw(ui, rect, span, rate);
    }
}

impl Filters {
    pub(super) fn len(&self) -> usize {
        self.visible().count()
    }

    fn visible(&self) -> impl Iterator<Item = (&usize, &Lane)> {
        self.lanes.iter().filter(|(_, lane)| lane.visible)
    }

    pub(super) fn visibility_controls(&mut self, ui: &mut Ui) {
        for lane in self.lanes.values_mut() {
            ui.checkbox(&mut lane.visible, &lane.name);
        }
    }

    pub(super) fn values(&self, time: f64) -> impl Iterator<Item = (usize, &str, Option<f32>)> {
        self.visible().map(move |(&index, lane)| {
            let value = lane
                .ready()
                .and_then(|output| output.samples.raw.value_at(time));
            (index, lane.name.as_str(), value)
        })
    }

    pub(super) fn has_lane(&self, channel: usize) -> bool {
        self.lanes.contains_key(&channel)
    }

    pub(super) fn input(&self, channel: usize, unit: &str) -> Option<Input> {
        let lane = self.lanes.get(&channel)?;
        let output = lane.ready()?;
        Some(Input::derived(
            lane.name.clone(),
            unit.to_owned(),
            output.samples.raw.clone(),
        ))
    }

    pub(super) fn channels(&self) -> impl Iterator<Item = FilteredChannel<'_>> {
        self.lanes.iter().map(|(&channel, lane)| FilteredChannel {
            channel,
            name: &lane.name,
            samples: lane.ready().map(|output| output.samples.clone()),
        })
    }

    pub(super) fn update(&mut self, ui: &Ui, recording: &Arc<Recording>, active: usize) {
        self.poll();
        let Some(lane) = self.lanes.get_mut(&active) else {
            return;
        };
        let request = Request {
            channel: active,
            rows: lane.rows.clone(),
            stages: lane.stages.clone(),
        };
        if lane.completed.as_ref() == Some(&request) {
            return;
        }
        if self
            .task
            .as_ref()
            .is_some_and(|(key, _)| key == &request || key.channel != active)
        {
            return;
        }
        let now = ui.input(|input| input.time);
        if now < self.retry_at {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.retry_at - now));
            return;
        }
        self.task = None;
        lane.output = None;
        lane.error = None;
        let rows = request.rows.end.saturating_sub(request.rows.start);
        if rows == 0 || rows > MAX_SAMPLES_PER_READ {
            lane.completed = Some(request);
            lane.error = Some(format!(
                "Choose a range of 1 to {MAX_SAMPLES_PER_READ} samples."
            ));
            return;
        }
        self.retry_at = now + 0.08;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let recording = recording.clone();
        let ctx = ui.ctx().clone();
        let load = request.clone();
        let task = Task::start(&JOBS, Duration::from_secs(20), move |cancel| {
            let result = load.run(&recording, generation, cancel);
            ctx.request_repaint();
            result
        });
        self.task = Some((request, task));
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    fn poll(&mut self) {
        let Some((key, result)) = take_query_result(&mut self.task) else {
            return;
        };
        let Some(lane) = self.lanes.get_mut(&key.channel) else {
            return;
        };
        if lane.rows != key.rows || lane.stages != key.stages {
            return;
        }
        lane.completed = Some(key);
        match result {
            Ok(output) => {
                lane.output = Some(output);
                lane.error = None;
            }
            Err(error) => {
                lane.output = None;
                lane.error = Some(error);
            }
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.task.is_some()
    }
}

impl Request {
    fn run(
        &self,
        recording: &Recording,
        generation: u64,
        cancel: &AtomicBool,
    ) -> Result<Output, String> {
        let mut raw = recording.checked(cancel, |capture| {
            reads::read_raw(capture, self.channel, self.rows.clone(), cancel)
        })?;
        self.stages
            .apply(raw.step.recip(), &mut raw.samples, cancel)?;
        let step = raw.samples.len().div_ceil(2048).max(1);
        let peaks = raw
            .samples
            .chunks(step)
            .map(|values| {
                let mut peak = Peak::default();
                peak.add(values);
                peak
            })
            .collect();
        let overview = Envelope {
            start: raw.first,
            step: step as u64,
            peaks,
            exact: true,
        };
        let bounds = overview.extrema();
        Ok(Output {
            samples: FilterSamples {
                generation,
                raw: Arc::new(raw),
            },
            overview,
            bounds,
        })
    }
}

fn draw_output(
    painter: &Painter,
    lane: Rect,
    span: &Range<f64>,
    channel: &Channel,
    output: &Output,
) {
    let (start, samples) = output.samples.raw.visible(span);
    if samples.len() <= 4096 {
        plot::draw_raw(
            painter,
            lane,
            span,
            start,
            output.samples.raw.step,
            samples,
            output.bounds,
            FILTER_COLOR,
        );
        return;
    }
    let filtered = channel.time_at(output.samples.raw.first)
        ..output.samples.raw.time(output.samples.raw.samples.len());
    let region = Rect::from_x_y_ranges(
        plot::x_at(lane, span, filtered.start)..=plot::x_at(lane, span, filtered.end),
        lane.y_range(),
    )
    .intersect(lane);
    if !region.is_negative() {
        plot::draw_envelope(
            &painter.with_clip_rect(region),
            lane,
            span,
            channel,
            &output.overview,
            output.bounds,
            FILTER_COLOR,
        );
    }
}
