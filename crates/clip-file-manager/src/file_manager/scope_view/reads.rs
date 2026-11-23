use little_clip_core::engineering::{
    analysis::{self, Expression, Value},
    scope::Channel,
    waveforms::{Capture, Peak, PowerTotals, Recording, sample_span},
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) const DETAIL_SAMPLES: u64 = 4096;
const ANALYSIS_SAMPLES: u64 = 65_536;
const MATH_SAMPLES: u64 = 262_144;

pub(in crate::file_manager) struct RawChannel {
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

    pub fn sample_at(&self, row: u64) -> Option<f32> {
        let index = usize::try_from(row.checked_sub(self.first)?).ok()?;
        self.samples.get(index).copied()
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
    pub valid_samples: usize,
    pub math: Option<(Arc<RawChannel>, Peak)>,
    pub limited: bool,
    pub power_totals: Option<PowerTotals>,
}

pub(super) struct AnalysisRequest {
    pub span: Range<f64>,
    pub active: usize,
    pub math: Option<(Expression, Range<f64>)>,
}

pub(super) fn analyze(
    recording: &Recording,
    request: AnalysisRequest,
    cancel: &AtomicBool,
) -> Result<Measurements, String> {
    recording.checked(cancel, |capture| {
        let (data, limited) = analysis_data(capture, &request.span, ANALYSIS_SAMPLES, cancel)?;

        let raw = &data[request.active];
        let frequencies = data
            .iter()
            .map(|channel| analysis::fundamental_hz(&channel.samples, channel.step.recip()))
            .collect();
        let math = request
            .math
            .as_ref()
            .map(|(expression, span)| {
                if *span == request.span && !limited {
                    Ok(math_trace(expression, &data, request.active))
                } else {
                    let (viewport, _) = analysis_data(capture, span, MATH_SAMPLES, cancel)?;
                    Ok::<_, String>(math_trace(expression, &viewport, request.active))
                }
            })
            .transpose()?;
        let mut extrema = Peak::default();
        let mut valid_samples = 0;
        for &value in raw.samples.iter().filter(|value| value.is_finite()) {
            valid_samples += 1;
            extrema.add(std::slice::from_ref(&value));
        }
        let raw = data
            .into_iter()
            .nth(request.active)
            .ok_or("Missing scope channel")?;
        let power_totals = capture.power_totals(request.span, cancel).ok().flatten();
        Ok(Measurements {
            frequencies,
            raw,
            extrema,
            valid_samples,
            math,
            limited,
            power_totals,
        })
    })
}

fn analysis_data(
    capture: &Capture,
    span: &Range<f64>,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<(Vec<RawChannel>, bool), String> {
    let mut limited = false;
    let data = capture
        .channels()
        .iter()
        .enumerate()
        .map(|(index, channel)| {
            let span = sample_span(channel, span.clone());
            let length = span.end.saturating_sub(span.start);
            let start = if length > limit {
                limited = true;
                span.start + (length - limit) / 2
            } else {
                span.start
            };
            read_raw(
                capture,
                index,
                start..span.end.min(start.saturating_add(limit)),
                cancel,
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((data, limited))
}

fn math_trace(
    expression: &Expression,
    data: &[RawChannel],
    index: usize,
) -> (Arc<RawChannel>, Peak) {
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
        Arc::new(RawChannel {
            first: active.first,
            start: active.start,
            step: active.step,
            samples,
        }),
        extrema,
    )
}

pub(super) fn padded_span(channel: &Channel, range: &Range<f64>, padding: u64) -> Range<u64> {
    let span = sample_span(channel, range.clone());
    span.start.saturating_sub(padding)..span.end.saturating_add(padding).min(channel.sample_count)
}
