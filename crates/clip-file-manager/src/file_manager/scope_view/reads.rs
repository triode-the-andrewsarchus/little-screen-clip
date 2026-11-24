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
pub(in crate::file_manager) const ANALYSIS_SAMPLES: u64 = 65_536;
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

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(super) struct RangePeak {
    pub amplitude: f32,
    pub observed: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MathRangePeak {
    equation: String,
    channel: usize,
    value: RangePeak,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct RangePeaks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_peaks: Vec<Option<RangePeak>>,
    // Older recipes cached derived peaks without identifying their inputs.
    // Read the raw peaks from those recipes and ignore their derived values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    math_range_peak: Option<MathRangePeak>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    filter_range_peaks: Vec<(super::filters::Request, RangePeak)>,
}

impl RangePeaks {
    pub fn get(
        &self,
        trace: super::ScopeTrace,
        math: Option<(&str, usize)>,
        filters: &super::filters::Filters,
    ) -> Option<RangePeak> {
        match trace {
            super::ScopeTrace::Recorded(index) => self.channel_peaks.get(index).copied().flatten(),
            super::ScopeTrace::Math => {
                let peak = self.math_range_peak.as_ref()?;
                (math == Some((peak.equation.as_str(), peak.channel))).then_some(peak.value)
            }
            super::ScopeTrace::Filtered(index) => {
                self.filter_range_peaks.iter().find_map(|(key, peak)| {
                    (key.channel == index && filters.matches(key)).then_some(*peak)
                })
            }
        }
    }
}

pub(super) struct Measurements {
    pub frequencies: Vec<Option<f64>>,
    pub peaks: RangePeaks,
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
    pub math: Option<(Expression, Range<f64>, String)>,
    pub filters: Vec<(super::filters::Request, Arc<RawChannel>)>,
}

pub(super) fn analyze(
    recording: &Recording,
    request: AnalysisRequest,
    cancel: &AtomicBool,
) -> Result<Measurements, String> {
    recording.checked(cancel, |capture| {
        let (data, limited) = analysis_data(capture, &request.span, ANALYSIS_SAMPLES, cancel)?;

        let frequencies = data
            .iter()
            .map(|channel| analysis::fundamental_hz(&channel.samples, channel.step.recip()))
            .collect();
        let math = request
            .math
            .as_ref()
            .map(|(expression, span, _)| {
                if *span == request.span && !limited {
                    Ok(math_trace(expression, &data, request.active))
                } else {
                    let (viewport, _) = analysis_data(capture, span, MATH_SAMPLES, cancel)?;
                    Ok::<_, String>(math_trace(expression, &viewport, request.active))
                }
            })
            .transpose()?;
        let (stats, channel_peaks) =
            range_peaks(capture, &request, &data).ok_or("Missing scope channel")?;
        let peaks = RangePeaks {
            channel_peaks,
            math_range_peak: request.math.zip(math.as_ref()).and_then(
                |((_, _, equation), (samples, _))| {
                    Some(MathRangePeak {
                        equation,
                        channel: request.active,
                        value: derived_peak(samples, &request.span)?,
                    })
                },
            ),
            filter_range_peaks: request
                .filters
                .into_iter()
                .filter_map(|(key, samples)| Some((key, derived_peak(&samples, &request.span)?)))
                .collect(),
        };
        let raw = data
            .into_iter()
            .nth(request.active)
            .ok_or("Missing scope channel")?;
        let power_totals = capture.power_totals(request.span, cancel).ok().flatten();
        Ok(Measurements {
            frequencies,
            peaks,
            raw,
            extrema: stats.peak,
            valid_samples: stats.valid,
            math,
            limited,
            power_totals,
        })
    })
}

fn range_peaks(
    capture: &Capture,
    request: &AnalysisRequest,
    data: &[RawChannel],
) -> Option<(SampleStats, Vec<Option<RangePeak>>)> {
    let mut active = None;
    let mut peaks = Vec::with_capacity(data.len());
    for (index, (raw, channel)) in data.iter().zip(capture.channels()).enumerate() {
        let measured = sample_extrema(&raw.samples);
        let amplitude = measured.peak.max - measured.peak.min;
        let observed = !raw.covers(&sample_span(channel, request.span.clone()))
            || measured.valid != raw.samples.len()
            || request.span.start < channel.start_seconds
            || request.span.end > channel.time_at(channel.sample_count);
        peaks.push(amplitude.is_finite().then_some(RangePeak {
            amplitude,
            observed,
        }));
        if index == request.active {
            active = Some(measured);
        }
    }
    Some((active?, peaks))
}

fn derived_peak(raw: &RawChannel, span: &Range<f64>) -> Option<RangePeak> {
    let index = |at: f64| {
        let lower = raw
            .index_at(at)
            .floor()
            .clamp(0.0, raw.samples.len() as f64) as usize;
        lower + usize::from(lower < raw.samples.len() && raw.time(lower) < at)
    };
    let measured = index(span.start) as u64..index(span.end) as u64;
    let rows = centered_rows(measured.clone(), ANALYSIS_SAMPLES);
    let samples = &raw.samples[rows.start as usize..rows.end as usize];
    let stats = sample_extrema(samples);
    let amplitude = stats.peak.max - stats.peak.min;
    let observed = rows != measured
        || stats.valid != samples.len()
        || span.start < raw.start
        || span.end > raw.time(raw.samples.len());
    amplitude.is_finite().then_some(RangePeak {
        amplitude,
        observed,
    })
}

pub(in crate::file_manager) struct SampleStats {
    pub peak: Peak,
    pub valid: usize,
    pub extrema: [usize; 2],
}

pub(in crate::file_manager) fn sample_extrema(samples: &[f32]) -> SampleStats {
    let mut stats = SampleStats {
        peak: Peak::default(),
        valid: 0,
        extrema: [0; 2],
    };
    for (index, &sample) in samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.is_finite())
    {
        if sample < stats.peak.min {
            stats.peak.min = sample;
            stats.extrema[0] = index;
        }
        if sample > stats.peak.max {
            stats.peak.max = sample;
            stats.extrema[1] = index;
        }
        stats.valid += 1;
    }
    stats
}

pub(in crate::file_manager) fn centered_rows(rows: Range<u64>, limit: u64) -> Range<u64> {
    let count = rows.end.saturating_sub(rows.start);
    let excess = count.saturating_sub(limit);
    let start = rows.start + excess / 2;
    start..start + count.min(limit)
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
            let rows = centered_rows(span.clone(), limit);
            limited |= span != rows;
            read_raw(capture, index, rows, cancel)
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
