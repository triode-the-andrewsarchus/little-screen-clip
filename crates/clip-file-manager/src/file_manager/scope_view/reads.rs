use little_clip_core::engineering::{
    analysis::{self, Expression, Spectrum, Value},
    scope::Channel,
    waveforms::{Peak, PowerTotals, Recording, sample_span},
};
use std::{ops::Range, sync::atomic::AtomicBool};

pub(super) const DETAIL_SAMPLES: u64 = 4096;
const ANALYSIS_SAMPLES: u64 = 65_536;

pub(super) struct RawChannel {
    pub start: f64,
    pub step: f64,
    pub samples: Vec<f32>,
}

impl RawChannel {
    pub fn time(&self, index: usize) -> f64 {
        self.start + index as f64 * self.step
    }

    fn index_at(&self, time: f64) -> f64 {
        (time - self.start) / self.step
    }

    pub fn value_at(&self, time: f64) -> Option<f32> {
        let index = self.index_at(time);
        (index >= 0.0)
            .then(|| self.samples.get(index.round() as usize).copied())
            .flatten()
    }

    pub fn snap(&self, time: f64, radius: f64) -> Option<f64> {
        let index = analysis::nearest_zero_crossing(
            &self.samples,
            self.index_at(time),
            radius / self.step,
        )?;
        Some(self.start + index * self.step)
    }

    pub fn visible(&self, span: &Range<f64>) -> (f64, &[f32]) {
        let first = self.index_at(span.start).floor().max(0.0) as usize;
        let last = (self.index_at(span.end).ceil().max(0.0) as usize)
            .saturating_add(1)
            .min(self.samples.len());
        let first = first.min(last);
        (self.time(first), &self.samples[first..last])
    }
}

pub(super) fn detail(
    recording: &Recording,
    span: Range<f64>,
    cancel: &AtomicBool,
) -> Result<Vec<RawChannel>, String> {
    recording.checked(cancel, |capture| {
        capture
            .channels()
            .iter()
            .enumerate()
            .map(|(index, channel)| {
                let range = padded_span(channel, &span, 2);
                let range = range.start..range.end.min(range.start + DETAIL_SAMPLES * 3);
                let start = channel.time_at(range.start);
                Ok(RawChannel {
                    start,
                    step: channel.interval_seconds,
                    samples: capture.read_samples(index, range, cancel)?,
                })
            })
            .collect()
    })
}

pub(super) fn snap(
    recording: &Recording,
    channel: usize,
    time: f64,
    radius: f64,
    cancel: &AtomicBool,
) -> Result<Option<f64>, String> {
    recording.checked(cancel, |capture| {
        let meta = &capture.channels()[channel];
        let radius = radius.min(meta.interval_seconds * DETAIL_SAMPLES as f64);
        let range = padded_span(meta, &(time - radius..time + radius), 1);
        let start = meta.time_at(range.start);
        let raw = RawChannel {
            start,
            step: meta.interval_seconds,
            samples: capture.read_samples(channel, range, cancel)?,
        };
        Ok(raw.snap(time, radius))
    })
}

pub(super) struct Measurements {
    pub frequencies: Vec<Option<f64>>,
    pub raw: RawChannel,
    pub extrema: Peak,
    pub spectrum: Option<Spectrum>,
    pub math: Option<MathTrace>,
    pub limited: bool,
    pub power_totals: Option<PowerTotals>,
}

pub(super) enum MathTrace {
    Predicate(Vec<Range<f64>>),
    Numeric(RawChannel, Peak),
}

pub(super) struct AnalysisRequest {
    pub span: Range<f64>,
    pub active: usize,
    pub spectrum: Option<analysis::SpectrumWindow>,
    pub math: Option<Expression>,
}

pub(super) fn analyze(
    recording: &Recording,
    request: AnalysisRequest,
    cancel: &AtomicBool,
) -> Result<Measurements, String> {
    recording.checked(cancel, |capture| {
        let mut limited = false;
        let data = capture
            .channels()
            .iter()
            .enumerate()
            .map(|(index, channel)| {
                let span = sample_span(channel, request.span.clone());
                let length = span.end.saturating_sub(span.start);
                let start = if length > ANALYSIS_SAMPLES {
                    limited = true;
                    span.start + (length - ANALYSIS_SAMPLES) / 2
                } else {
                    span.start
                };
                let end = span.end.min(start.saturating_add(ANALYSIS_SAMPLES));
                Ok(RawChannel {
                    start: channel.time_at(start),
                    step: channel.interval_seconds,
                    samples: capture.read_samples(index, start..end, cancel)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let raw = &data[request.active];
        let frequencies = data
            .iter()
            .map(|channel| analysis::fundamental_hz(&channel.samples, channel.step.recip()))
            .collect();
        let spectrum = request.spectrum.and_then(|window| {
            analysis::spectrum_with_window(&raw.samples, raw.step.recip(), window)
        });
        let math = request
            .math
            .as_ref()
            .map(|expression| math_trace(expression, &data, request.active));
        let mut extrema = Peak::default();
        extrema.add(&raw.samples);
        let raw = data
            .into_iter()
            .nth(request.active)
            .ok_or("Missing scope channel")?;
        let power_totals = capture.power_totals(request.span, cancel).ok().flatten();
        Ok(Measurements {
            frequencies,
            raw,
            extrema,
            spectrum,
            math,
            limited,
            power_totals,
        })
    })
}

fn math_trace(expression: &Expression, data: &[RawChannel], index: usize) -> MathTrace {
    let active = &data[index];
    let samples = (0..active.samples.len()).map(|index| {
        let t = active.time(index);
        expression.evaluate(|channel| data.get(channel)?.value_at(t).map(f64::from))
    });
    if expression.is_predicate() {
        let mut runs = Vec::new();
        let mut start = None;
        for (index, value) in samples.enumerate() {
            match (start, value) {
                (None, Some(Value::Predicate(true))) => start = Some(index),
                (Some(first), Some(Value::Predicate(true))) => start = Some(first),
                (Some(first), _) => {
                    runs.push(active.time(first)..active.time(index));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(first) = start {
            runs.push(active.time(first)..active.time(active.samples.len()));
        }
        MathTrace::Predicate(runs)
    } else {
        let samples = samples
            .map(|value| match value {
                Some(Value::Numeric(value)) => value as f32,
                _ => f32::NAN,
            })
            .collect::<Vec<_>>();
        let mut extrema = Peak::default();
        extrema.add(&samples);
        MathTrace::Numeric(
            RawChannel {
                start: active.start,
                step: active.step,
                samples,
            },
            extrema,
        )
    }
}

fn padded_span(channel: &Channel, range: &Range<f64>, padding: u64) -> Range<u64> {
    let span = sample_span(channel, range.clone());
    span.start.saturating_sub(padding)..span.end.saturating_add(padding).min(channel.sample_count)
}
