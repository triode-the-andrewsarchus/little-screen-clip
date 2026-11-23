use super::ScopeTrace;
use super::{
    JOBS, Task, Viewer, plot, reads,
    spectrum_input::{Input, LaneKind},
};
use crate::file_manager::signal_view::{
    filter_editor::{StageFocus, StageList, add_buttons, reveal_frequency},
    take_query_result,
};
use egui::{Painter, Rect, Ui};
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

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SavedFilter {
    channel: usize,
    settings: LaneSettings,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct LaneSettings {
    name: String,
    visible: bool,
    rows: Range<u64>,
    stages: StageList,
}

#[derive(Default)]
pub(super) struct Filters {
    lanes: BTreeMap<usize, Lane>,
    task: Option<(Request, Task<Result<Output, String>>)>,
    retry_at: f64,
    generation: u64,
    pub(super) reveal_spectrum: bool,
}

struct Lane {
    settings: LaneSettings,
    editor: StageFocus,
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
    fn new(settings: LaneSettings) -> Self {
        Self {
            settings,
            editor: StageFocus::default(),
            output: None,
            completed: None,
            error: None,
        }
    }

    fn matches(&self, key: &Request) -> bool {
        key.rows == self.settings.rows && key.stages == self.settings.stages
    }

    fn finished(&self) -> bool {
        self.completed.as_ref().is_some_and(|key| self.matches(key))
    }

    fn ready(&self) -> Option<&Output> {
        self.output.as_ref().filter(|_| self.finished())
    }
}

impl Viewer {
    pub(super) fn filter_controls(&mut self, ui: &mut Ui) {
        let kind = ui
            .horizontal_wrapped(|ui| add_buttons(ui, self.can_add_filter()))
            .inner;
        if let Some(kind) = kind {
            self.add_filter(kind);
        }
        let channel = &self.overview.channels[self.active];
        let rate = channel.interval_seconds.recip();
        let rows = self.filter_sample_range();
        let Some(lane) = self.filters.lanes.get_mut(&self.active) else {
            return;
        };
        ui.push_id(("scope-stage-editor", self.active), |ui| {
            if let Some(frequency) = lane.settings.stages.controls(ui, &mut lane.editor, rate) {
                reveal_frequency(&mut self.spectrum_span, frequency, rate);
            }
        });
        if lane.settings.stages.is_empty() {
            self.filters.lanes.remove(&self.active);
            self.trace_fft.remove(&ScopeTrace::Filtered(self.active));
            if self.spectra.kind == LaneKind::Filtered {
                self.set_spectrum_kind(LaneKind::Recorded);
            }
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut lane.settings.name)
                    .id_salt(("filtered-lane-name", self.active))
                    .hint_text("Lane name")
                    .desired_width(110.0)
                    .char_limit(48),
            );
            ui.weak(format!(
                "Filter range  {} to {}",
                display_value(channel.time_at(lane.settings.rows.start), "s"),
                display_value(channel.time_at(lane.settings.rows.end), "s")
            ));
            if ui
                .add_enabled(
                    lane.settings.rows != rows,
                    egui::Button::new("Use spectrum range").small(),
                )
                .clicked()
            {
                lane.settings.rows = rows;
            }
            if let Some(error) = &lane.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }

    fn filter_sample_range(&self) -> Range<u64> {
        let channel = &self.overview.channels[self.active];
        self.spectrum_interval()
            .map_or(0..channel.sample_count, |range| sample_span(channel, range))
    }

    fn can_add_filter(&self) -> bool {
        self.filters
            .lanes
            .get(&self.active)
            .is_none_or(|lane| lane.settings.stages.can_add())
    }

    pub(super) fn add_filter(&mut self, kind: FilterKind) {
        if !self.can_add_filter() {
            return;
        }
        let channel = &self.overview.channels[self.active];
        let rate = channel.interval_seconds.recip();
        let rows = self.filter_sample_range();
        let lane = self.filters.lanes.entry(self.active).or_insert_with(|| {
            Lane::new(LaneSettings {
                name: format!("{} filtered", channel.name),
                visible: true,
                rows,
                stages: StageList::default(),
            })
        });
        let span = self.spectrum_span.clone().unwrap_or(0.0..rate * 0.5);
        if let Some(index) = lane
            .settings
            .stages
            .add(kind, rate, &span, self.spectrum_marker)
        {
            lane.editor.select(index);
            self.filters.reveal_spectrum = true;
        }
    }

    pub(super) fn draw_filters(
        &mut self,
        ui: &mut Ui,
        painter: &Painter,
        clip: &Painter,
        rect: Rect,
    ) {
        let first = self.physical_lanes() + usize::from(self.math_state.numeric().is_some());
        let indices = self.filters.visible_indices().collect::<Vec<_>>();
        for (slot, index) in indices.into_iter().enumerate() {
            let trace = ScopeTrace::Filtered(index);
            let lane = self.lane(rect, first + slot);
            if !painter.clip_rect().intersects(lane.frame) {
                if let Some(state) = self.trace_fft.get_mut(&trace) {
                    state.suspend();
                }
                continue;
            }
            let channel = &self.overview.channels[index];
            let derived = &self.filters.lanes[&index];
            let output = derived.ready();
            let status = match output {
                Some(_) => "",
                None if derived.error.is_some() => " · unavailable",
                None => " · filtering…",
            };
            let mut heading = lane.heading(ui, ("scope-filter-lane", index));
            let toggle = self.trace_fft_toggle(&mut heading, trace);
            heading.colored_label(FILTER_COLOR, format!("{}{status}", derived.settings.name));
            if self.trace_fft.contains_key(&trace) {
                self.paint_trace_fft(ui, lane.plot, trace);
            } else if let Some(output) = output {
                draw_output(
                    &clip.with_clip_rect(lane.plot),
                    lane.plot,
                    &self.span,
                    channel,
                    output,
                    output.bounds,
                );
                lane.scale(painter, output.bounds, &channel.unit, FILTER_COLOR);
            }
            if toggle {
                self.toggle_trace_fft(trace);
                ui.ctx().request_repaint();
            }
        }
    }

    pub(super) fn paint_comparison_filter(
        &self,
        painter: &Painter,
        index: usize,
        rect: Rect,
        bounds: Peak,
    ) {
        let Some(output) = self
            .filters
            .lanes
            .get(&index)
            .filter(|lane| lane.settings.visible)
            .and_then(Lane::ready)
        else {
            return;
        };
        let channel = &self.overview.channels[index];
        draw_output(
            &painter.with_clip_rect(rect.intersect(painter.clip_rect())),
            rect,
            &self.span,
            channel,
            output,
            bounds,
        );
    }

    pub(super) fn draw_filter_response(&mut self, ui: &mut Ui, rect: Rect, span: &Range<f64>) {
        let Some(lane) = self.filters.lanes.get_mut(&self.active) else {
            return;
        };
        let rate = self.overview.channels[self.active].interval_seconds.recip();
        ui.push_id(("scope-stage-plot", self.active), |ui| {
            lane.settings
                .stages
                .draw(ui, &mut lane.editor, rect, span, rate);
        });
    }
}

impl Filters {
    pub(super) fn save(&self) -> Vec<SavedFilter> {
        self.lanes
            .iter()
            .map(|(&channel, lane)| SavedFilter {
                channel,
                settings: lane.settings.clone(),
            })
            .collect()
    }

    pub(super) fn restore(
        &mut self,
        filters: Vec<SavedFilter>,
        channels: &[Channel],
    ) -> Result<(), String> {
        if filters.len() > channels.len() {
            return Err("Too many saved scope filters".into());
        }
        let mut lanes = BTreeMap::new();
        for filter in filters {
            filter.settings.stages.validate()?;
            if channels.get(filter.channel).is_none_or(|channel| {
                filter.settings.rows.start > filter.settings.rows.end
                    || filter.settings.rows.end > channel.sample_count
            }) || filter.settings.name.len() > 192
            {
                return Err("Invalid scope filter channel or sample range".into());
            }
            let old = lanes.insert(filter.channel, Lane::new(filter.settings));
            if old.is_some() {
                return Err("Duplicate scope filter channel".into());
            }
        }
        self.lanes = lanes;
        Ok(())
    }

    pub(super) fn any(&self) -> bool {
        !self.lanes.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.visible().count()
    }

    fn visible(&self) -> impl Iterator<Item = (&usize, &Lane)> {
        self.lanes.iter().filter(|(_, lane)| lane.settings.visible)
    }

    pub(super) fn visible_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.visible().map(|(&index, _)| index)
    }

    pub(super) fn visibility_controls(&mut self, ui: &mut Ui) {
        for lane in self.lanes.values_mut() {
            let action = if lane.settings.visible {
                "Hide"
            } else {
                "Show"
            };
            if ui
                .button(format!("{action} {}", lane.settings.name))
                .clicked()
            {
                lane.settings.visible = !lane.settings.visible;
            }
        }
    }

    pub(super) fn probes(
        &self,
        time: f64,
    ) -> impl Iterator<Item = (usize, usize, &str, Peak, f32)> {
        self.visible()
            .enumerate()
            .filter_map(move |(slot, (&index, lane))| {
                let output = lane.ready()?;
                let value = output.samples.raw.value_at(time)?;
                Some((
                    slot,
                    index,
                    lane.settings.name.as_str(),
                    output.bounds,
                    value,
                ))
            })
    }

    pub(super) fn has_lane(&self, channel: usize) -> bool {
        self.lanes.contains_key(&channel)
    }

    pub(super) fn input(&self, channel: usize, unit: &str) -> Option<Input> {
        let lane = self.lanes.get(&channel)?;
        let output = lane.ready()?;
        Some(Input::derived(
            lane.settings.name.clone(),
            unit.to_owned(),
            output.samples.raw.clone(),
        ))
    }

    pub(super) fn channels(&self) -> impl Iterator<Item = FilteredChannel<'_>> {
        self.lanes.iter().map(|(&channel, lane)| FilteredChannel {
            channel,
            name: &lane.settings.name,
            samples: lane.ready().map(|output| output.samples.clone()),
        })
    }

    pub(super) fn update(&mut self, ui: &Ui, recording: &Arc<Recording>, active: usize) {
        self.poll();
        if self.task.as_ref().is_some_and(|(key, _)| {
            self.lanes
                .get(&key.channel)
                .is_some_and(|lane| lane.matches(key))
        }) {
            return;
        }
        self.task = None;
        let Some(channel) = self.next_channel(active) else {
            return;
        };
        let now = ui.input(|input| input.time);
        if now < self.retry_at {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.retry_at - now));
            return;
        }
        let lane = self.lanes.get_mut(&channel).expect("pending filter");
        let request = Request {
            channel,
            rows: lane.settings.rows.clone(),
            stages: lane.settings.stages.clone(),
        };
        lane.output = None;
        lane.error = None;
        let rows = request.rows.end.saturating_sub(request.rows.start);
        if rows == 0 || rows > MAX_SAMPLES_PER_READ {
            lane.completed = Some(request);
            lane.error = Some(format!(
                "Choose a range of 1 to {MAX_SAMPLES_PER_READ} samples."
            ));
            ui.ctx().request_repaint();
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

    fn next_channel(&self, active: usize) -> Option<usize> {
        if self.lanes.get(&active).is_some_and(|lane| !lane.finished()) {
            return Some(active);
        }
        self.lanes
            .iter()
            .find_map(|(&channel, lane)| (!lane.finished()).then_some(channel))
    }

    fn poll(&mut self) {
        let Some((key, result)) = take_query_result(&mut self.task) else {
            return;
        };
        let Some(lane) = self.lanes.get_mut(&key.channel) else {
            return;
        };
        if !lane.matches(&key) {
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
    bounds: Peak,
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
            bounds,
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
            bounds,
            FILTER_COLOR,
        );
    }
}
