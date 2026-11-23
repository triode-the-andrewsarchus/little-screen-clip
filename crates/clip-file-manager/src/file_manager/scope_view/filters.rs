use super::{JOBS, Task, Viewer, plot, reads, spectrum_input::Input};
use crate::file_manager::signal_view::take_query_result;
use egui::{Align2, Color32, FontId, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    analysis::{FilterChain, FilterKind, FilterStage},
    scope::{Channel, MAX_SAMPLES_PER_READ},
    waveforms::{Envelope, Peak, Recording, display_value, sample_span},
};
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) const FILTER_COLOR: Color32 = Color32::from_rgb(221, 163, 93);
const MAX_STAGES: usize = 8;

#[derive(Default)]
pub(super) struct Filters {
    lanes: BTreeMap<usize, Lane>,
    task: Option<(Request, Task<Result<Output, String>>)>,
    retry_at: f64,
}

struct Lane {
    name: String,
    visible: bool,
    rows: Range<u64>,
    stages: Vec<FilterStage>,
    output: Option<Output>,
    completed: Option<Request>,
    error: Option<String>,
}

#[derive(Clone, PartialEq)]
struct Request {
    channel: usize,
    rows: Range<u64>,
    stages: Vec<FilterStage>,
}

struct Output {
    raw: Arc<reads::RawChannel>,
    overview: Envelope,
    bounds: Peak,
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
        let bounds = rate * 1e-7..=rate * (0.5 - 1e-7);
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
        let remove = ui
            .menu_button("Filter stages", |ui| {
                let mut remove = None;
                for (index, stage) in lane.stages.iter_mut().enumerate() {
                    ui.push_id(("filter-stage-controls", index), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(kind_label(stage.kind));
                            ui.add(
                                egui::DragValue::new(&mut stage.frequency_hz)
                                    .range(bounds.clone())
                                    .speed(rate / 5000.0)
                                    .suffix(" Hz"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut stage.q)
                                    .range(0.1..=25.0)
                                    .speed(0.04)
                                    .prefix("Q "),
                            )
                            .on_hover_text(
                                "Quality factor · higher Q narrows a band-pass or notch",
                            );
                            if ui
                                .small_button("×")
                                .on_hover_text("Remove filter")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    });
                }
                remove
            })
            .inner
            .flatten();
        if let Some(index) = remove {
            lane.stages.remove(index);
            if lane.stages.is_empty() {
                self.filters.lanes.remove(&self.active);
            }
        }
    }

    pub(super) fn can_add_filter(&self) -> bool {
        self.filters
            .lanes
            .get(&self.active)
            .is_none_or(|lane| lane.stages.len() < MAX_STAGES)
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
                stages: Vec::new(),
                output: None,
                completed: None,
                error: None,
            });
        let span = self.spectrum_span.clone().unwrap_or(0.0..rate * 0.5);
        let frequency = self
            .spectrum_marker
            .filter(|&marker| span.contains(&marker))
            .unwrap_or_else(|| stage_position(&span, &lane.stages))
            .clamp(rate * 1e-7, rate * (0.5 - 1e-7));
        lane.stages.push(FilterStage {
            kind,
            frequency_hz: frequency,
            q: match kind {
                FilterKind::LowPass | FilterKind::HighPass => std::f64::consts::FRAC_1_SQRT_2,
                FilterKind::BandPass | FilterKind::Notch => 4.0,
            },
        });
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
            let status = match &derived.output {
                Some(_) => "",
                None if derived.error.is_some() => " · unavailable",
                None => " · filtering…",
            };
            if let Some(output) = &derived.output {
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
        let painter = ui.painter_at(rect);
        if let Ok(chain) = FilterChain::new(rate, &lane.stages) {
            let points = (0..=160).filter_map(|index| {
                let fraction = f64::from(index) / 160.0;
                let frequency = span.start + fraction * (span.end - span.start);
                let gain = chain.gain_at(frequency)?;
                let db = (20.0 * gain.max(1e-6).log10()).clamp(-60.0, 24.0);
                Some(pos2(
                    plot::x_at(rect, span, frequency),
                    rect.bottom() - rect.height() * ((db + 60.0) / 84.0) as f32,
                ))
            });
            painter.add(egui::Shape::line(
                points.collect(),
                Stroke::new(1.2, FILTER_COLOR),
            ));
            for db in [-60, 0, 24] {
                let y = rect.bottom() - rect.height() * (db + 60) as f32 / 84.0;
                painter.text(
                    pos2(
                        rect.right() - 4.0,
                        y.clamp(rect.top() + 7.0, rect.bottom() - 7.0),
                    ),
                    Align2::RIGHT_CENTER,
                    format!("{db} dB gain"),
                    FontId::monospace(10.0),
                    FILTER_COLOR,
                );
            }
        }
        for (index, stage) in lane.stages.iter_mut().enumerate() {
            ui.push_id(("filter-handles", index), |ui| {
                stage_handles(ui, &painter, rect, span, rate, stage);
            });
        }
    }
}

fn stage_handles(
    ui: &mut Ui,
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    rate: f64,
    stage: &mut FilterStage,
) {
    let label = format!(
        "{} · {}",
        kind_label(stage.kind),
        display_value(stage.frequency_hz, "Hz")
    );
    if let Some(target) = handle(
        ui,
        painter,
        rect,
        span,
        stage.frequency_hz,
        ("center", 9.0),
        label,
    ) {
        stage.frequency_hz = target.clamp(rate * 1e-7, rate * (0.5 - 1e-7));
    }
    let Some(edges) = stage.band_edges(rate) else {
        return;
    };
    plot::highlight(
        painter,
        rect,
        span,
        edges[0]..edges[1],
        FILTER_COLOR.gamma_multiply(0.10),
    );
    for (name, frequency) in ["lower edge", "upper edge"].into_iter().zip(edges) {
        let label = format!(
            "{} {name} · {}",
            kind_label(stage.kind),
            display_value(frequency, "Hz")
        );
        if let Some(edge) = handle(ui, painter, rect, span, frequency, (name, 23.0), label)
            && let Some(q) = stage.q_for_edge(rate, edge)
        {
            stage.q = q;
        }
    }
}

fn handle(
    ui: &mut Ui,
    painter: &Painter,
    rect: Rect,
    span: &Range<f64>,
    frequency: f64,
    (id, y): (&str, f32),
    tooltip: String,
) -> Option<f64> {
    let x = plot::x_at(rect, span, frequency);
    if !rect.x_range().contains(x) {
        return None;
    }
    let target = Rect::from_center_size(pos2(x, rect.top() + y), vec2(16.0, 14.0));
    let response = ui
        .interact(target, ui.make_persistent_id(id), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_text(tooltip);
    painter.vline(x, rect.y_range(), Stroke::new(1.0, FILTER_COLOR));
    painter.rect_filled(target.shrink2(vec2(3.0, 3.0)), 2.0, FILTER_COLOR);
    response
        .dragged()
        .then(|| {
            response
                .interact_pointer_pos()
                .map(|pointer| plot::at(rect, span, pointer.x))
        })
        .flatten()
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
                .output
                .as_ref()
                .and_then(|output| output.raw.value_at(time));
            (index, lane.name.as_str(), value)
        })
    }

    pub(super) fn has_lane(&self, channel: usize) -> bool {
        self.lanes.contains_key(&channel)
    }

    pub(super) fn input(&self, channel: usize, unit: &str) -> Option<Input> {
        let lane = self.lanes.get(&channel)?;
        let output = lane.output.as_ref()?;
        Some(Input::derived(
            lane.name.clone(),
            unit.to_owned(),
            output.raw.clone(),
        ))
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
        let recording = recording.clone();
        let ctx = ui.ctx().clone();
        let load = request.clone();
        let task = Task::start(&JOBS, Duration::from_secs(20), move |cancel| {
            let result = load.run(&recording, cancel);
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
    fn run(&self, recording: &Recording, cancel: &AtomicBool) -> Result<Output, String> {
        let mut raw = recording.checked(cancel, |capture| {
            reads::read_raw(capture, self.channel, self.rows.clone(), cancel)
        })?;
        let mut chain = FilterChain::new(raw.step.recip(), &self.stages)?;
        for block in raw.samples.chunks_mut(4096) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Filtering was cancelled".into());
            }
            chain.process(block);
        }
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
            raw: Arc::new(raw),
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
    let (start, samples) = output.raw.visible(span);
    if samples.len() <= 4096 {
        plot::draw_raw(
            painter,
            lane,
            span,
            start,
            output.raw.step,
            samples,
            output.bounds,
            FILTER_COLOR,
        );
        return;
    }
    let filtered = channel.time_at(output.raw.first)..output.raw.time(output.raw.samples.len());
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

pub(super) fn filter_menu(ui: &mut Ui) -> Option<FilterKind> {
    [
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::BandPass,
        FilterKind::Notch,
    ]
    .into_iter()
    .find(|&kind| {
        let clicked = ui.button(kind_label(kind)).clicked();
        if clicked {
            ui.close();
        }
        clicked
    })
}

fn stage_position(span: &Range<f64>, stages: &[FilterStage]) -> f64 {
    let mut edges = stages
        .iter()
        .map(|stage| stage.frequency_hz)
        .filter(|frequency| span.contains(frequency))
        .collect::<Vec<_>>();
    edges.push(span.start);
    edges.push(span.end);
    edges.sort_by(f64::total_cmp);
    edges
        .windows(2)
        .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .map_or(span.start.midpoint(span.end), |pair| {
            pair[0].midpoint(pair[1])
        })
}

fn kind_label(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::LowPass => "Low-pass",
        FilterKind::HighPass => "High-pass",
        FilterKind::BandPass => "Band-pass",
        FilterKind::Notch => "Notch",
    }
}
