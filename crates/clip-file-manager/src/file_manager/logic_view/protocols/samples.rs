use super::{Event, Kind, active_event, event_label};
use crate::file_manager::scope_view::plot as timing;
use egui::{Align2, FontId, Mesh, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::logic_decode::{I2sChannel, PcmLane, PwmLane};
use std::{fmt::Write, ops::Range, sync::Arc};

#[derive(Default)]
pub(in crate::file_manager) struct Pins {
    pcm: PcmPins,
    pwm: PwmPins,
}

impl Pins {
    pub(in crate::file_manager) fn pcm(&self) -> impl Iterator<Item = (usize, u64, &Arc<PcmLane>)> {
        self.pcm
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| {
                lane.as_ref().map(|lane| (index, self.pcm.generation, lane))
            })
    }

    pub(in crate::file_manager) fn pwm(&self) -> Option<(u64, &Arc<PwmLane>)> {
        self.pwm
            .lane
            .as_ref()
            .map(|lane| (self.pwm.generation, lane))
    }

    pub(super) fn controls(
        &mut self,
        ui: &mut Ui,
        measure: Measure,
        events: &[Event],
        span: &Range<f64>,
    ) {
        match measure {
            Measure::Pcm => self.pcm.controls(ui, events, span),
            Measure::Duty => self.pwm.controls(ui, events, span),
        }
    }
}

#[derive(Default)]
struct PcmPins {
    generation: u64,
    lanes: [Option<Arc<PcmLane>>; 2],
    error: bool,
}

impl PcmPins {
    fn controls(&mut self, ui: &mut Ui, events: &[Event], span: &Range<f64>) {
        if ui
            .button("Pin PCM for equations")
            .on_hover_text(
                "Use visible decoded words, or the brushed region, as fixed full-scale PCM inputs.",
            )
            .clicked()
        {
            let lanes = [I2sChannel::Left, I2sChannel::Right]
                .map(|channel| PcmLane::from_events(events, channel, span).map(Arc::new));
            self.error = lanes.iter().all(Option::is_none);
            if !self.error {
                self.generation = self.generation.wrapping_add(1);
                self.lanes = lanes;
            }
        }
        if self.lanes.iter().any(Option::is_some) && ui.button("Clear PCM pins").clicked() {
            self.lanes = [None, None];
            self.error = false;
        }
        if self.error {
            ui.weak("Select at least three word times and one valid sample on a channel.");
        }
    }
}

#[derive(Default)]
struct PwmPins {
    generation: u64,
    lane: Option<Arc<PwmLane>>,
    error: bool,
}

impl PwmPins {
    fn controls(&mut self, ui: &mut Ui, events: &[Event], span: &Range<f64>) {
        if ui
            .button("Pin PWM for equations")
            .on_hover_text("Pin duty, frequency, active time and period from complete cycles in the visible or brushed region.")
            .clicked()
        {
            let lane = PwmLane::from_events(events, span);
            self.error = lane.is_none();
            if let Some(lane) = lane {
                self.generation = self.generation.wrapping_add(1);
                self.lane = Some(Arc::new(lane));
            }
        }
        if let Some(lane) = &self.lane {
            ui.weak(format!("{} cycles pinned", lane.cycle_count()));
            if ui.button("Clear PWM pins").clicked() {
                self.lane = None;
                self.error = false;
            }
        }
        if self.error {
            ui.weak("Select at least one complete PWM cycle.");
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Measure {
    Pcm,
    Duty,
}

impl Measure {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Pcm => "Sample plot",
            Self::Duty => "Duty plot",
        }
    }

    pub(super) fn copy_label(self) -> &'static str {
        match self {
            Self::Pcm => "Copy sample TSV",
            Self::Duty => "Copy cycle TSV",
        }
    }

    pub(super) fn tsv(self, events: &[Event]) -> String {
        match self {
            Self::Pcm => pcm_tsv(events),
            Self::Duty => duty_tsv(events),
        }
    }

    fn lanes(self) -> &'static [&'static str] {
        match self {
            Self::Pcm => &["Left", "Right"],
            Self::Duty => &["Duty"],
        }
    }

    fn sample(self, kind: Kind, lane: usize) -> Option<Option<f32>> {
        match self {
            Self::Pcm => {
                let sample = Sample::from_event(kind)?;
                let channel = usize::from(sample.channel == I2sChannel::Right);
                (lane == channel).then(|| {
                    sample.value.map(|value| {
                        let full_scale = f64::from(1_u32 << (sample.bits - 1));
                        (f64::from(value) / full_scale * 0.5 + 0.5) as f32
                    })
                })
            }
            Self::Duty => match kind {
                Kind::PwmCycle(cycle) => Some(Some(cycle.duty() as f32)),
                _ => None,
            },
        }
    }
}

pub(super) fn plot(
    ui: &mut Ui,
    span: &Range<f64>,
    cursor: Option<f64>,
    events: &[Event],
    measure: Measure,
) -> Option<usize> {
    let (outer, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
    let rect = Rect::from_min_max(outer.min + vec2(78.0, 0.0), outer.max - vec2(9.0, 2.0));
    let painter = ui.painter_at(outer);
    let clipped = painter.with_clip_rect(rect);
    clipped.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    if let Some(index) = active_event(events, cursor) {
        timing::highlight(
            &clipped,
            rect,
            span,
            events[index].time.clone(),
            ui.visuals().selection.bg_fill.gamma_multiply(0.24),
        );
    }
    let labels = measure.lanes();
    for (index, label) in labels.iter().enumerate() {
        let row = timing::lane(rect, index, labels.len()).shrink2(vec2(0.0, 2.0));
        baseline(ui, &painter, row, measure, label);
        samples(&clipped, row, span, events, measure, index);
    }
    let point = response.hover_pos().filter(|point| rect.contains(*point))?;
    let at = timing::at(rect, span, point.x);
    let index = active_event(events, Some(at))?;
    let event = &events[index];
    let lane = usize::from(labels.len() == 2 && point.y >= rect.center().y);
    measure.sample(event.kind, lane)?;
    let clicked = response.clicked();
    response.on_hover_text(event_label(event.kind));
    clicked.then_some(index)
}

fn baseline(ui: &Ui, painter: &Painter, row: Rect, measure: Measure, label: &str) {
    let ink = ui.visuals().weak_text_color();
    let grid: &[(f32, &str)] = match measure {
        Measure::Duty => &[(1.0, "100%"), (0.5, "50%"), (0.0, "0%")],
        Measure::Pcm => &[(0.5, label)],
    };
    for &(ratio, text) in grid {
        let y = sample_y(row, ratio);
        painter.hline(
            row.x_range(),
            y,
            Stroke::new(0.75, ink.gamma_multiply(0.33)),
        );
        painter.text(
            pos2(row.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            text,
            FontId::monospace(10.0),
            ui.visuals().text_color(),
        );
    }
}

fn sample_y(row: Rect, ratio: f32) -> f32 {
    row.center().y + row.height() * 0.9 * (0.5 - ratio)
}

fn samples(
    painter: &Painter,
    row: Rect,
    span: &Range<f64>,
    events: &[Event],
    measure: Measure,
    lane: usize,
) {
    let color = timing::color(lane);
    let dimmed = color.gamma_multiply(0.40);
    let mut mesh = Mesh::default();
    for event in events {
        let Some(value) = measure.sample(event.kind, lane) else {
            continue;
        };
        let left = timing::x_at(row, span, event.time.start);
        let right = timing::x_at(row, span, event.time.end).max(left + 1.0);
        let y = sample_y(row, value.unwrap_or(0.5));
        mesh.add_colored_rect(
            Rect::from_min_max(pos2(left, y - 1.0), pos2(right, y + 1.0)),
            if value.is_some() { color } else { dimmed },
        );
    }
    painter.add(mesh);
}

struct Sample {
    channel: I2sChannel,
    value: Option<i32>,
    bits: u8,
    received: u8,
}

impl Sample {
    fn from_event(kind: Kind) -> Option<Self> {
        match kind {
            Kind::I2sSample {
                channel,
                sample,
                bits,
            } => Some(Self {
                channel,
                value: sample,
                bits,
                received: bits,
            }),
            Kind::I2sShortWord {
                channel,
                received,
                bits,
            } => Some(Self {
                channel,
                value: None,
                bits,
                received,
            }),
            _ => None,
        }
    }
}

fn pcm_tsv(events: &[Event]) -> String {
    let mut text = String::from("start_s\tend_s\tchannel\tsample\tword_bits\treceived_bits\n");
    for event in events {
        if let Some(sample) = Sample::from_event(event.kind) {
            let value = sample
                .value
                .map_or_else(String::new, |value| value.to_string());
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{value}\t{}\t{}",
                event.time.start,
                event.time.end,
                sample.channel.label(),
                sample.bits,
                sample.received,
            );
        }
    }
    text
}

fn duty_tsv(events: &[Event]) -> String {
    let mut text =
        String::from("start_s\tend_s\tactive_seconds\tperiod_seconds\tfrequency_hz\tduty_ratio\n");
    for event in events {
        if let Kind::PwmCycle(cycle) = event.kind {
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}\t{}",
                event.time.start,
                event.time.end,
                cycle.active_seconds,
                cycle.period_seconds,
                cycle.frequency(),
                cycle.duty(),
            );
        }
    }
    text
}
