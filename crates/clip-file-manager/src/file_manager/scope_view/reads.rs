use little_clip_core::engineering::{
    analysis::{self, Expression, Spectrum, Value},
    scope::Channel,
    waveforms::{Capture, Peak, PowerTotals, Recording, sample_span},
};
use std::{ops::Range, sync::atomic::AtomicBool};

pub(super) const DETAIL_SAMPLES: u64 = 4096;
const ANALYSIS_SAMPLES: u64 = 65_536;

pub(super) struct RawChannel {
    pub first: u64,
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

    pub fn covers(&self, rows: &Range<u64>) -> bool {
        rows.is_empty()
            || self.first <= rows.start
                && rows.end <= self.first.saturating_add(self.samples.len() as u64)
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
                read_raw(capture, index, range, cancel)
            })
            .collect()
    })
}

pub(super) fn read_raw(
    capture: &Capture,
    index: usize,
    range: Range<u64>,
    cancel: &AtomicBool,
) -> Result<RawChannel, String> {
    let channel = &capture.channels()[index];
    Ok(RawChannel {
        first: range.start,
        start: channel.time_at(range.start),
        step: channel.interval_seconds,
        samples: capture.read_samples(index, range, cancel)?,
    })
}

pub(super) fn snap_window(
    recording: &Recording,
    channel: usize,
    range: Range<u64>,
    cancel: &AtomicBool,
) -> Result<RawChannel, String> {
    recording.checked(cancel, |capture| read_raw(capture, channel, range, cancel))
}

pub(super) struct Measurements {
    pub frequencies: Vec<Option<f64>>,
    pub raw: RawChannel,
    pub extrema: Peak,
    pub spectrum: Option<Spectrum>,
    pub math: Option<(RawChannel, Peak)>,
    pub limited: bool,
    pub power_totals: Option<PowerTotals>,
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
                read_raw(capture, index, start..end, cancel)
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

fn math_trace(expression: &Expression, data: &[RawChannel], index: usize) -> (RawChannel, Peak) {
    let active = &data[index];
    let samples = (0..active.samples.len())
        .map(|index| {
            let t = active.time(index);
            match expression.evaluate(|channel| data.get(channel)?.value_at(t).map(f64::from)) {
                Some(Value::Numeric(value)) => value as f32,
                _ => f32::NAN,
            }
        })
        .collect::<Vec<_>>();
    let mut extrema = Peak::default();
    extrema.add(&samples);
    (
        RawChannel {
            first: active.first,
            start: active.start,
            step: active.step,
            samples,
        },
        extrema,
    )
}

pub(super) fn padded_span(channel: &Channel, range: &Range<f64>, padding: u64) -> Range<u64> {
    let span = sample_span(channel, range.clone());
    span.start.saturating_sub(padding)..span.end.saturating_add(padding).min(channel.sample_count)
}
