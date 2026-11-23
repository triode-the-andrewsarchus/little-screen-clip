//! One native interval projected onto the clocks of the linked recordings.

mod measure;
mod power;

use super::{Capture, Source, alignment, evidence, take_query_result, timeline};
use crate::file_manager::preview_job::Task;
use egui::{Context, Ui};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::Clock,
        waveforms::{display_axis_value, display_value},
    },
};
use measure::{Figures, Input, Measurement, Report, SampleReport};
use std::{collections::BTreeMap, ops::Range, sync::LazyLock, time::Duration};

static JOBS: LazyLock<BackgroundJobs> =
    LazyLock::new(|| BackgroundJobs::new(2, super::MAX_SOURCES));

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Pin {
    pub source: u64,
    pub native: Range<f64>,
}

impl Pin {
    pub(super) fn project(
        &self,
        source: &Source,
        preview: Option<alignment::Suggestion>,
    ) -> Option<Range<f64>> {
        if matches!(source.capture, Capture::Unavailable(_)) {
            return None;
        }
        let projected = source.display_clock(preview).workspace_range(&self.native);
        (!projected.is_empty() && projected.start.is_finite() && projected.end.is_finite())
            .then_some(projected)
    }

    pub(super) fn native_for(
        &self,
        source: u64,
        clock: Clock,
        workspace: &Range<f64>,
    ) -> Range<f64> {
        if self.source == source {
            self.native.clone()
        } else {
            clock.native_range(workspace)
        }
    }
}

struct PowerVisit {
    source: u64,
    selection: power::Selection,
    request: Range<f64>,
    scope: power::Scope,
    clock: Clock,
    before: Range<f64>,
    scroll: f32,
    reveal: bool,
}

enum Reveal {
    Span(Range<f64>),
    Power(power::Selection),
}

#[derive(Default)]
pub(super) struct Shared {
    pub pin: Option<Pin>,
    reads: BTreeMap<u64, Query>,
    power_basis: power::Basis,
    power_comparison: Option<power::Comparison>,
    visit: Option<PowerVisit>,
    restore_scroll: Option<f32>,
}

impl Shared {
    pub fn restore(pin: Option<Pin>) -> Self {
        Self {
            pin,
            ..Default::default()
        }
    }

    pub fn set(&mut self, pin: Option<Pin>) {
        self.pin = pin;
        self.reads.clear();
        self.power_comparison = None;
        self.visit = None;
        self.restore_scroll = None;
    }

    pub fn invalidate_source(&mut self, id: u64) {
        self.reads.remove(&id);
        self.visit.take_if(|visit| visit.source == id);
    }

    pub fn remove_source(&mut self, id: u64) {
        self.invalidate_source(id);
        if self.pin.as_ref().is_some_and(|pin| pin.source == id) {
            self.set(None);
        }
    }

    pub fn controls(
        &mut self,
        ui: &mut Ui,
        sources: &[Source],
        preview: Option<alignment::Suggestion>,
        view: &mut Range<f64>,
        domain: &Range<f64>,
    ) -> Option<Range<f64>> {
        let pin = self.pin.as_ref()?;
        let origin = sources.iter().find(|source| source.id == pin.source)?;
        let projected = pin.project(origin, preview);
        let clear = ui
            .horizontal_wrapped(|ui| {
                ui.colored_label(timeline::shared_color(ui.visuals()), "Shared range");
                if let Some(range) = &projected {
                    ui.monospace(times(range));
                    ui.weak(format!("From {}", origin.name()));
                    if ui.small_button("Show shared range").clicked() {
                        self.visit = None;
                        focus(view, domain, range);
                    }
                } else {
                    ui.weak(format!("{} · Range source unavailable", origin.name()));
                }
                ui.small_button("Clear shared range").clicked()
            })
            .inner;
        if clear {
            self.set(None);
            return None;
        }
        self.power_controls(ui, sources, preview, projected.as_ref(), view, domain);
        self.visit_controls(ui, sources, view, domain);
        projected
    }

    fn power_controls(
        &mut self,
        ui: &mut Ui,
        sources: &[Source],
        preview: Option<alignment::Suggestion>,
        projected: Option<&Range<f64>>,
        view: &mut Range<f64>,
        domain: &Range<f64>,
    ) {
        self.power_comparison =
            projected.and_then(|range| power::Comparison::from_sources(sources, preview, range));
        if let Some(comparison) = &self.power_comparison {
            let before = self.power_basis;
            comparison.controls(ui, &mut self.power_basis);
            if self.power_basis != before {
                self.end_visit(ui, view, domain);
            }
        }
    }

    pub fn source(
        &mut self,
        ui: &mut Ui,
        source: &Source,
        shared: Option<&Range<f64>>,
        clock: Clock,
        view: &mut Range<f64>,
        domain: &Range<f64>,
        scroll: f32,
    ) -> Option<Range<f64>> {
        let native = self.pin.as_ref()?.native_for(source.id, clock, shared?);
        let scope = self
            .power_comparison
            .as_ref()
            .filter(|_| self.power_basis == power::Basis::Common)
            .map_or(power::Scope::Each, |comparison| {
                comparison.scope(source, clock)
            });
        self.visit.take_if(|visit| {
            visit.source == source.id
                && (visit.request != native || visit.scope != scope || visit.clock != clock)
        });
        match self.measurement_ui(ui, source, &native, &scope, clock) {
            Some(Reveal::Span(span)) => {
                self.visit = None;
                focus(view, domain, &clock.workspace_range(&span));
                ui.ctx().request_repaint();
            }
            Some(Reveal::Power(selection)) => {
                let (before, scroll) = self.visit.take().map_or_else(
                    || (view.clone(), scroll),
                    |visit| (visit.before, visit.scroll),
                );
                focus(view, domain, &clock.workspace_range(&selection.native));
                self.visit = Some(PowerVisit {
                    source: source.id,
                    selection,
                    request: native.clone(),
                    scope,
                    clock,
                    before,
                    scroll,
                    reveal: true,
                });
                ui.ctx().request_repaint();
            }
            None => {}
        }
        Some(native)
    }

    pub fn evidence(&mut self, source: u64) -> Option<(evidence::Marker, bool)> {
        let visit = self.visit.as_mut().filter(|visit| visit.source == source)?;
        Some((
            evidence::Marker {
                channel: visit.selection.channel,
                native: visit.selection.native.clone(),
            },
            std::mem::take(&mut visit.reveal),
        ))
    }

    pub fn return_scroll(&mut self) -> Option<f32> {
        self.restore_scroll.take()
    }

    fn visit_controls(
        &mut self,
        ui: &mut Ui,
        sources: &[Source],
        view: &mut Range<f64>,
        domain: &Range<f64>,
    ) {
        let Some(visit) = &self.visit else { return };
        let Some(source) = sources.iter().find(|source| source.id == visit.source) else {
            return;
        };
        let back = ui
            .horizontal_wrapped(|ui| {
                ui.strong(format!("{} · {}", source.name(), visit.selection.signal));
                ui.monospace(times(&visit.clock.workspace_range(&visit.selection.native)));
                ui.small_button("Back to shared readings").clicked()
            })
            .inner;
        if back {
            self.end_visit(ui, view, domain);
        }
    }

    fn end_visit(&mut self, ui: &Ui, view: &mut Range<f64>, domain: &Range<f64>) {
        if let Some(visit) = self.visit.take() {
            *view = timeline::fit(domain, visit.before, 0.0);
            self.restore_scroll = Some(visit.scroll);
            ui.ctx().request_repaint();
        }
    }

    fn measurement_ui(
        &mut self,
        ui: &mut Ui,
        source: &Source,
        requested: &Range<f64>,
        scope: &power::Scope,
        clock: Clock,
    ) -> Option<Reveal> {
        let (input, name) = match &source.capture {
            Capture::Analog(view) => {
                let channel = view.measurement_channel();
                let recording = view.recording();
                (
                    Input::Samples(recording.clone(), channel),
                    &recording.channels()[channel].name,
                )
            }
            Capture::Digital(view) => {
                let (recording, lanes) = view.recording();
                let (lane, polarity, refined) = view.measurement_channel();
                if !refined {
                    ui.weak("Finding logic capture extent…");
                    return None;
                }
                (
                    Input::Logic {
                        recording: recording.clone(),
                        lane,
                        polarity,
                    },
                    &lanes.get(lane)?.name,
                )
            }
            Capture::Unavailable(_) => return None,
        };
        let wants_power = scope.has_windows()
            && matches!(&input, Input::Samples(recording, _) if recording.power_recording());
        let request = Request {
            input,
            native: requested.clone(),
            power: scope.clone(),
        };
        let query = self.reads.entry(source.id).or_default();
        query.update(ui.ctx(), &request);
        let result = query.result.as_ref();
        let waveform = result
            .filter(|(key, outcome)| *key == request || outcome.is_ok())
            .map(|(_, outcome)| outcome);
        let totals = result
            .filter(|(key, _)| *key == request)
            .map(|(_, outcome)| outcome);
        let readout = ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 22.0;
            ui.spacing_mut().button_padding.y = 2.0;
            let span = ui
                .horizontal_wrapped(|ui| {
                    ui.colored_label(
                        timeline::shared_color(ui.visuals()),
                        format!("{} · {name}", source.name()),
                    );
                    match waveform {
                        None => {
                            ui.weak("Measuring shared range…");
                            None
                        }
                        Some(Err(error)) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                            None
                        }
                        Some(Ok(measured)) => match measured.waveform.as_ref() {
                            Some(report) => readings(ui, report, requested, clock),
                            None => {
                                ui.weak("Outside channel coverage");
                                None
                            }
                        },
                    }
                })
                .inner;
            let power = if wants_power {
                match totals {
                    Some(Ok(result)) => result
                        .power
                        .as_ref()
                        .and_then(|power| power::readings(ui, power, scope, clock)),
                    Some(Err(_)) => None,
                    None => {
                        ui.weak("Measuring power totals…");
                        None
                    }
                }
            } else {
                None
            };
            power.map(Reveal::Power).or_else(|| span.map(Reveal::Span))
        });
        if matches!(readout.inner, Some(Reveal::Span(_))) {
            readout.response.scroll_to_me(Some(egui::Align::TOP));
        }
        readout.inner
    }
}

#[derive(Clone, PartialEq)]
struct Request {
    input: Input,
    native: Range<f64>,
    power: power::Scope,
}

impl Request {
    fn same_waveform(&self, other: &Self) -> bool {
        self.input == other.input && self.native == other.native
    }
}

type Outcome = Result<Measurement, String>;

#[derive(Default)]
struct Query {
    pending: Option<(Request, Task<Outcome>)>,
    result: Option<(Request, Outcome)>,
}

impl Query {
    fn update(&mut self, ctx: &Context, request: &Request) {
        self.pending.take_if(|(key, _)| *key != *request);
        self.result.take_if(|(key, _)| !key.same_waveform(request));
        if let Some(result) = take_query_result(&mut self.pending) {
            self.result = Some(result);
        }
        if self.pending.is_none() && self.result.as_ref().is_none_or(|(key, _)| key != request) {
            let load = request.clone();
            let repaint = ctx.clone();
            let job = Task::start(&JOBS, Duration::from_secs(20), move |cancel| {
                let result = load.input.read(load.native, load.power, cancel);
                repaint.request_repaint();
                result
            });
            self.pending = Some((request.clone(), job));
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(24));
        }
    }
}

fn readings(
    ui: &mut Ui,
    report: &Report,
    requested: &Range<f64>,
    clock: Clock,
) -> Option<Range<f64>> {
    if report.coverage == *requested {
        ui.weak("Full coverage");
    } else {
        ui.weak(format!(
            "Coverage {}",
            times(&clock.workspace_range(&report.coverage))
        ));
    }
    if report.examined != report.coverage {
        let label = match report.figures {
            Figures::Samples(_) => "Sample window",
            _ => "Edge scan",
        };
        ui.weak(format!(
            "{label} {}",
            times(&clock.workspace_range(&report.examined))
        ));
    }
    match &report.figures {
        Figures::Samples(samples) => sample_readings(ui, samples, &report.examined),
        Figures::Edges(count) => {
            ui.label(format!("{count} observed edges"));
            ui.small_button("Show edges")
                .clicked()
                .then(|| report.examined.clone())
        }
        Figures::Pwm {
            cycles,
            extrema: Some((low, high)),
        } => {
            ui.label(format!("{cycles} complete cycles"));
            let visit = ui
                .small_button(format!("Min duty {:.1}%", low.percent))
                .clicked()
                .then(|| low.cycle.clone());
            let high = ui
                .small_button(format!("Max duty {:.1}%", high.percent))
                .clicked()
                .then(|| high.cycle.clone());
            visit.or(high)
        }
        Figures::Pwm { extrema: None, .. } => {
            ui.weak("No complete PWM cycles");
            None
        }
    }
}

fn sample_readings(
    ui: &mut Ui,
    samples: &SampleReport,
    examined: &Range<f64>,
) -> Option<Range<f64>> {
    let SampleReport {
        unit,
        valid,
        rows,
        extrema,
        ..
    } = samples;
    let count = rows.end - rows.start;
    let mut visit = None;
    if let Some((low, high)) = extrema {
        ui.label(format!(
            "Pk–Pk {}",
            display_value(f64::from(high.value) - f64::from(low.value), unit)
        ));
        for (label, sample) in [("Min", low), ("Max", high)] {
            let value = display_value(f64::from(sample.value), unit);
            if ui
                .small_button(format!("{label} {value}"))
                .on_hover_text(format!("Show sample {}", sample.sample))
                .clicked()
            {
                visit = Some(sample.view.clone());
            }
        }
    }
    if *valid as u64 == count {
        ui.weak(format!("{count} samples"));
    } else {
        ui.weak(format!("{valid} of {count} samples present"));
    }
    if count > 0 && ui.small_button("Show samples").clicked() {
        visit = Some(examined.clone());
    }
    visit
}

fn times(range: &Range<f64>) -> String {
    let resolution = (range.end - range.start) * 0.001;
    format!(
        "{} to {}",
        display_axis_value(range.start, resolution, "s"),
        display_axis_value(range.end, resolution, "s")
    )
}

pub(super) fn focus(view: &mut Range<f64>, domain: &Range<f64>, evidence: &Range<f64>) {
    if evidence.is_empty() {
        return;
    }
    let padding = (evidence.end - evidence.start) * 0.08;
    *view = timeline::fit(
        domain,
        evidence.start - padding..evidence.end + padding,
        0.0,
    );
}

fn overlap(a: &Range<f64>, b: &Range<f64>) -> Option<Range<f64>> {
    let common = a.start.max(b.start)..a.end.min(b.end);
    (!common.is_empty()).then_some(common)
}
