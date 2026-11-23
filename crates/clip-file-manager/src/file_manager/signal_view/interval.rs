//! One native interval projected onto the clocks of the linked recordings.

mod measure;

use super::{Capture, Source, alignment, take_query_result, timeline};
use crate::file_manager::preview_job::Task;
use egui::{Context, Ui};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::Clock,
        waveforms::{Integral, PowerTotals, display_axis_value, display_value},
    },
};
use measure::{Figures, Input, Report, SampleReport};
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

#[derive(Default)]
pub(super) struct Shared {
    pub pin: Option<Pin>,
    reads: BTreeMap<u64, Query>,
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
    }

    pub fn remove_source(&mut self, id: u64) {
        self.reads.remove(&id);
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
                    if ui.button("Show shared range").clicked() {
                        focus(view, domain, range);
                    }
                } else {
                    ui.weak(format!("{} · Range source unavailable", origin.name()));
                }
                ui.button("Clear shared range").clicked()
            })
            .inner;
        if clear {
            self.set(None);
            return None;
        }
        projected
    }

    pub fn source(
        &mut self,
        ui: &mut Ui,
        source: &Source,
        shared: Option<&Range<f64>>,
        clock: Clock,
        view: &mut Range<f64>,
        domain: &Range<f64>,
    ) -> Option<Range<f64>> {
        let native = self.pin.as_ref()?.native_for(source.id, clock, shared?);
        if let Some(visit) = self.measurement_ui(ui, source, &native, clock) {
            focus(view, domain, &clock.workspace_range(&visit));
            ui.ctx().request_repaint();
        }
        Some(native)
    }

    fn measurement_ui(
        &mut self,
        ui: &mut Ui,
        source: &Source,
        requested: &Range<f64>,
        clock: Clock,
    ) -> Option<Range<f64>> {
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
        let query = self.reads.entry(source.id).or_default();
        query.update(
            ui.ctx(),
            Request {
                input,
                native: requested.clone(),
            },
        );
        let readout = ui.horizontal_wrapped(|ui| {
            ui.colored_label(timeline::shared_color(ui.visuals()), name);
            match query.result.as_ref().map(|(_, result)| result) {
                None => {
                    ui.weak("Measuring shared range…");
                    None
                }
                Some(Ok(None)) => {
                    ui.weak("Outside recording");
                    None
                }
                Some(Err(error)) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                    None
                }
                Some(Ok(Some(report))) => readings(ui, report, requested, clock),
            }
        });
        if readout.inner.is_some() {
            readout.response.scroll_to_me(Some(egui::Align::TOP));
        }
        readout.inner
    }
}

#[derive(Clone, PartialEq)]
struct Request {
    input: Input,
    native: Range<f64>,
}

type Measurement = Result<Option<Report>, String>;

#[derive(Default)]
struct Query {
    pending: Option<(Request, Task<Measurement>)>,
    result: Option<(Request, Measurement)>,
}

impl Query {
    fn update(&mut self, ctx: &Context, request: Request) {
        self.pending.take_if(|(key, _)| *key != request);
        self.result.take_if(|(key, _)| *key != request);
        if let Some(result) = take_query_result(&mut self.pending) {
            self.result = Some(result);
        }
        if self.pending.is_none() && self.result.is_none() {
            let load = request.clone();
            let repaint = ctx.clone();
            let job = Task::start(&JOBS, Duration::from_secs(20), move |cancel| {
                let result = load.input.read(load.native, cancel);
                repaint.request_repaint();
                result
            });
            self.pending = Some((request, job));
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
        ui.weak("Full time span");
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
        Figures::Samples(samples) => sample_readings(ui, samples, &report.examined, clock),
        Figures::Edges(count) => {
            ui.label(format!("{count} observed edges"));
            ui.button("Show edges")
                .clicked()
                .then(|| report.examined.clone())
        }
        Figures::Pwm {
            cycles,
            extrema: Some((low, high)),
        } => {
            ui.label(format!("{cycles} complete cycles"));
            let visit = ui
                .button(format!("Min duty {:.1}%", low.percent))
                .clicked()
                .then(|| low.cycle.clone());
            let high = ui
                .button(format!("Max duty {:.1}%", high.percent))
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
    clock: Clock,
) -> Option<Range<f64>> {
    let SampleReport {
        unit,
        valid,
        rows,
        extrema,
        power,
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
                .button(format!("{label} {value}"))
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
    if let Some(power) = power {
        power_readings(ui, power, clock);
    }
    if count > 0 && ui.button("Show samples").clicked() {
        visit = Some(examined.clone());
    }
    visit
}

fn power_readings(ui: &mut Ui, totals: &PowerTotals, clock: Clock) {
    let charge = totals.charge.as_ref();
    let energy = totals.energy.as_ref();
    if let Some(reading) = charge {
        let seconds = reading.time.end - reading.time.start;
        if seconds > 0.0 {
            integral_label(
                ui,
                reading,
                "Avg current",
                display_value(reading.value / seconds, "A"),
            );
        }
        integral_label(ui, reading, "Charge", milli_hours(reading, clock, "mAh"));
    }
    if let Some(reading) = energy {
        integral_label(ui, reading, "Energy", milli_hours(reading, clock, "mWh"));
    }
    match (charge, energy) {
        (Some(charge), Some(energy)) if charge.time != energy.time => {
            integral_time(ui, "Current interval", charge, clock);
            integral_time(ui, "Power interval", energy, clock);
        }
        (Some(reading), _) | (_, Some(reading)) => {
            integral_time(ui, "Integrated", reading, clock);
        }
        (None, None) => {
            ui.weak("Charge and energy unavailable");
        }
    }
}

fn integral_label(ui: &mut Ui, reading: &Integral, label: &str, value: String) {
    let quality = if reading.approximate { "≈ " } else { "" };
    let partial = if reading.partial { " (partial)" } else { "" };
    ui.label(format!("{label} {quality}{value}{partial}"));
}

fn milli_hours(reading: &Integral, clock: Clock, unit: &str) -> String {
    let value = reading.value * clock.scale() / 3.6;
    if value != 0.0 && value.abs() < 0.0001 {
        format!("{value:.3e} {unit}")
    } else {
        let number = format!("{value:.6}");
        format!(
            "{} {unit}",
            number.trim_end_matches('0').trim_end_matches('.')
        )
    }
}

fn integral_time(ui: &mut Ui, label: &str, reading: &Integral, clock: Clock) {
    ui.weak(format!(
        "{label} {}",
        times(&clock.workspace_range(&reading.time))
    ));
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
