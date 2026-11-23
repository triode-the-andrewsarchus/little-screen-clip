//! Bounded scope windows and peak-preserving waveform overviews.

use super::{
    jls,
    scope::{self, Channel},
};
use crate::file_source::{FileSource, ReadCheck};
use std::{
    fs::File,
    io::Write,
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const CHUNK: u64 = 16384;
const PREVIEW_BINS: u64 = 192;
const PREVIEW_WINDOW: u64 = 48;
const INDEX_BINS: u64 = 16384;
const MAX_EXPORT_ROWS: u64 = 1_000_000;

/// One interval's measured extrema in the channel's physical unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

impl Default for Peak {
    fn default() -> Self {
        Self {
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }
}

impl Peak {
    pub fn add(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
        }
    }

    pub fn valid(self) -> bool {
        self.min.is_finite() && self.max.is_finite() && self.min <= self.max
    }

    pub fn merge(&mut self, other: Self) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }
}

/// Equally spaced summaries over a half-open sample interval.
pub struct Envelope {
    pub start: u64,
    pub step: u64,
    pub peaks: Vec<Peak>,
    /// Every sample in each peak interval contributed to its extrema.
    pub exact: bool,
}

impl Envelope {
    /// Derive bounded display columns without losing a recorded peak.
    pub fn columns(&self, range: Range<u64>, columns: usize) -> Vec<Peak> {
        let length = range.end.saturating_sub(range.start);
        let n = (columns as u64).max(1).min(length);
        if n == 0 {
            return Vec::new();
        }
        let mut output = Vec::with_capacity(n as usize);
        for index in 0..n {
            let start = range.start + boundary(length, index, n);
            let end = range.start + boundary(length, index + 1, n);
            let first = (start.saturating_sub(self.start) / self.step).min(self.peaks.len() as u64);
            let last = end
                .saturating_sub(self.start)
                .div_ceil(self.step)
                .min(self.peaks.len() as u64);
            let mut merged = Peak::default();
            for peak in &self.peaks[first as usize..last as usize] {
                merged.merge(*peak);
            }
            output.push(merged);
        }
        output
    }

    pub fn extrema(&self) -> Peak {
        self.peaks
            .iter()
            .copied()
            .fold(Peak::default(), |mut a, b| {
                a.merge(b);
                a
            })
    }
}

/// Compact sampled view for the little screen and Files strip.
pub struct Overview {
    pub model: String,
    pub channels: Vec<Channel>,
    pub envelopes: Vec<Envelope>,
}

impl Overview {
    pub fn duration_seconds(&self) -> f64 {
        time_span(&self.channels).end - time_span(&self.channels).start
    }

    pub fn sampled(&self) -> bool {
        self.envelopes.iter().any(|envelope| !envelope.exact)
    }
}

pub fn time_span(channels: &[Channel]) -> Range<f64> {
    let mut first = f64::INFINITY;
    let mut last = f64::NEG_INFINITY;
    for channel in channels {
        first = first.min(channel.start_seconds);
        last = last.max(channel.time_at(channel.sample_count));
    }
    first..last.max(first + f64::EPSILON)
}

/// Formats an engineering value using a power-of-1000 SI prefix.
pub fn display_value(value: f64, unit: &str) -> String {
    if !value.is_finite() {
        return format!("- {unit}");
    }
    let (scale, prefix) = [
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
    ]
    .into_iter()
    .find(|(scale, _)| value.abs() >= *scale)
    .unwrap_or(if value == 0.0 {
        (1.0, "")
    } else {
        (1e-12, "p")
    });
    let scaled = value / scale;
    let decimals = if scaled.abs() >= 100.0 { 1 } else { 3 };
    let number = format!("{scaled:.decimals$}");
    format!(
        "{} {prefix}{unit}",
        number.trim_end_matches('0').trim_end_matches('.')
    )
}

pub fn sample_span(channel: &Channel, time: Range<f64>) -> Range<u64> {
    let index = |time: f64| {
        let lower = ((time - channel.start_seconds) / channel.interval_seconds)
            .floor()
            .clamp(0.0, channel.sample_count as f64) as u64;
        if lower < channel.sample_count && channel.time_at(lower) < time {
            lower + 1
        } else {
            lower
        }
    };
    index(time.start)..index(time.end)
}

/// Source adapter shared by scope and power recording views.
pub enum Capture {
    Scope(scope::Capture),
    Power(jls::Capture),
}

#[derive(Default)]
pub struct PowerTotals {
    pub charge: Option<Integral>,
    pub energy: Option<Integral>,
}

pub struct Integral {
    pub value: f64,
    pub approximate: bool,
}

impl Capture {
    pub fn model(&self) -> &str {
        match self {
            Self::Scope(capture) => capture.model(),
            Self::Power(capture) => capture.model(),
        }
    }

    pub fn channels(&self) -> &[Channel] {
        match self {
            Self::Scope(capture) => capture.channels(),
            Self::Power(capture) => capture.channels(),
        }
    }

    pub fn read_samples(
        &self,
        index: usize,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<f32>, String> {
        match self {
            Self::Scope(capture) => capture.read_samples(index, range, cancel),
            Self::Power(capture) => capture.read_samples(index, range, cancel),
        }
    }

    pub fn power_totals(
        &self,
        span: Range<f64>,
        cancel: &AtomicBool,
    ) -> Result<Option<PowerTotals>, String> {
        let Self::Power(capture) = self else {
            return Ok(None);
        };
        let mut totals = PowerTotals::default();
        for (unit, output) in [("A", &mut totals.charge), ("W", &mut totals.energy)] {
            let Some((index, channel)) = capture
                .channels()
                .iter()
                .enumerate()
                .find(|(_, channel)| channel.unit == unit)
            else {
                continue;
            };
            if span.start < channel.start_seconds
                || span.end > channel.time_at(channel.sample_count)
            {
                continue;
            }
            let range = sample_span(channel, span.clone());
            if range.is_empty() {
                continue;
            }
            *output = if let Some(value) = capture.integral(index, range.clone(), cancel)? {
                Some(Integral {
                    value,
                    approximate: false,
                })
            } else {
                capture
                    .summary_integral(index, range, cancel)?
                    .map(|value| Integral {
                        value,
                        approximate: true,
                    })
            };
        }
        Ok(Some(totals))
    }

    fn envelope(
        &self,
        index: usize,
        range: Range<u64>,
        bins: u64,
        complete_scope: bool,
        cancel: &AtomicBool,
    ) -> Result<Envelope, String> {
        match self {
            Self::Scope(_) => {
                let step = (range.end - range.start).div_ceil(bins).max(1);
                summarize(
                    self,
                    index,
                    range,
                    step,
                    complete_scope || step <= PREVIEW_WINDOW,
                    cancel,
                )
            }
            Self::Power(capture) => capture.envelope(index, range, bins as usize, cancel),
        }
    }
}

/// Retains the file observation across deferred waveform reads.
pub struct Recording {
    source: FileSource,
    capture: Capture,
    file: File,
    check: ReadCheck,
}

impl Recording {
    pub fn open(path: &Path, check: ReadCheck, cancel: &AtomicBool) -> Result<Self, String> {
        Self::open_expected(path, None, check, cancel)
    }

    pub fn open_expected(
        path: &Path,
        expected: Option<&FileSource>,
        check: ReadCheck,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let (file, source) = check.open(path, expected, cancel)?;
        let input = file.try_clone().map_err(|error| error.to_string())?;
        let capture = if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jls"))
        {
            Capture::Power(jls::Capture::open(input)?)
        } else {
            Capture::Scope(scope::Capture::open(input)?)
        };
        source.check_file(&file)?;
        check.validate(cancel)?;
        Ok(Self {
            source,
            capture,
            file,
            check,
        })
    }

    pub fn set_read_check(&mut self, check: ReadCheck) {
        self.check = check;
    }

    pub fn source(&self) -> &FileSource {
        &self.source
    }

    pub fn channels(&self) -> &[Channel] {
        self.capture.channels()
    }

    pub fn power_recording(&self) -> bool {
        matches!(self.capture, Capture::Power(_))
    }

    pub fn checked<T>(
        &self,
        cancel: &AtomicBool,
        read: impl FnOnce(&Capture) -> Result<T, String>,
    ) -> Result<T, String> {
        source_read(&self.source, &self.file, &self.check, cancel, || {
            read(&self.capture)
        })
    }

    pub fn overview(&self, cancel: &AtomicBool) -> Result<Overview, String> {
        self.checked(cancel, |capture| {
            let envelopes = capture
                .channels()
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    capture.envelope(index, 0..channel.sample_count, PREVIEW_BINS, false, cancel)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Overview {
                model: capture.model().to_owned(),
                channels: capture.channels().to_vec(),
                envelopes,
            })
        })
    }

    /// Refined overview from stored summaries or a bounded working set.
    pub fn index(&self, cancel: &AtomicBool) -> Result<Vec<Envelope>, String> {
        self.checked(cancel, |capture| {
            capture
                .channels()
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    capture.envelope(index, 0..channel.sample_count, INDEX_BINS, true, cancel)
                })
                .collect()
        })
    }

    /// Download bounded native summaries for a window of a power recording.
    pub fn power_envelopes(
        &self,
        span: Range<f64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<Envelope>, String> {
        self.checked(cancel, |capture| {
            let Capture::Power(capture) = capture else {
                return Err("This waveform has no native summary index".into());
            };
            capture
                .channels()
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    capture.envelope(index, sample_span(channel, span.clone()), 2048, cancel)
                })
                .collect()
        })
    }

    /// A selected channel region as exact time and calibrated sample columns.
    pub fn export_channel(
        &self,
        channel_index: usize,
        time: Range<f64>,
        cancel: &AtomicBool,
        mut output: impl Write,
    ) -> Result<u64, String> {
        self.checked(cancel, |capture| {
            let channel = capture
                .channels()
                .get(channel_index)
                .ok_or("Missing channel")?;
            let span = sample_span(channel, time);
            let rows = span.end.saturating_sub(span.start);
            if rows == 0 || rows > MAX_EXPORT_ROWS {
                return Err("Select a region of one to one million samples to copy".into());
            }
            writeln!(output, "Time (s),{} ({})", channel.name, channel.unit)
                .map_err(|error| error.to_string())?;
            for chunk in chunks(span, CHUNK) {
                let start = chunk.start;
                let samples = capture.read_samples(channel_index, chunk, cancel)?;
                for (offset, sample) in samples.into_iter().enumerate() {
                    writeln!(
                        output,
                        "{},{}",
                        channel.time_at(start + offset as u64),
                        sample
                    )
                    .map_err(|error| error.to_string())?;
                }
            }
            Ok(rows)
        })
    }
}

fn source_read<T>(
    source: &FileSource,
    file: &File,
    check: &ReadCheck,
    cancel: &AtomicBool,
    read: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Waveform read cancelled".into());
    }
    check.validate(cancel)?;
    source.check_file(file)?;
    let result = read()?;
    source.check_file(file)?;
    check.validate(cancel)?;
    Ok(result)
}

fn summarize(
    capture: &Capture,
    channel: usize,
    range: Range<u64>,
    step: u64,
    exact: bool,
    cancel: &AtomicBool,
) -> Result<Envelope, String> {
    let count = range.end.saturating_sub(range.start);
    let mut peaks = vec![Peak::default(); count.div_ceil(step) as usize];
    if exact {
        for chunk in chunks(range.clone(), CHUNK) {
            let mut index = chunk.start - range.start;
            let input = capture.read_samples(channel, chunk, cancel)?;
            let mut samples = input.as_slice();
            while !samples.is_empty() {
                let take = (step - index % step).min(samples.len() as u64) as usize;
                peaks[(index / step) as usize].add(&samples[..take]);
                index += take as u64;
                samples = &samples[take..];
            }
        }
    } else {
        for (bucket, peak) in chunks(range.clone(), step).zip(&mut peaks) {
            let center = bucket.start + (bucket.end - bucket.start) / 2;
            let start = center.saturating_sub(PREVIEW_WINDOW / 2).max(bucket.start);
            let end = (start + PREVIEW_WINDOW).min(bucket.end);
            peak.add(&capture.read_samples(channel, start..end, cancel)?);
        }
    }
    Ok(Envelope {
        start: range.start,
        step,
        peaks,
        exact,
    })
}

fn chunks(range: Range<u64>, chunk: u64) -> impl Iterator<Item = Range<u64>> {
    let mut offset = range.start;
    std::iter::from_fn(move || {
        if offset >= range.end {
            return None;
        }
        let next = offset.saturating_add(chunk).min(range.end);
        let result = offset..next;
        offset = next;
        Some(result)
    })
}

fn boundary(length: u64, index: u64, n: u64) -> u64 {
    length / n * index + (length % n * index) / n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_grid_boundaries_survive_round_tripping_through_time() {
        let channel = Channel {
            name: "CH1".into(),
            unit: "V".into(),
            sample_count: 2_000_000,
            start_seconds: -0.007123456789,
            interval_seconds: 2.7e-9,
        };
        for start in (0..channel.sample_count - 1).step_by(37) {
            let time = channel.time_at(start)..channel.time_at(start + 1);
            assert_eq!(sample_span(&channel, time), start..start + 1);
        }
    }

    #[test]
    fn display_columns_preserve_short_peaks_at_wide_zooms() {
        let envelope = Envelope {
            start: 0,
            step: 10,
            peaks: vec![
                Peak {
                    min: -1.0,
                    max: 1.0,
                },
                Peak {
                    min: -2.0,
                    max: 40.0,
                },
                Peak {
                    min: -3.0,
                    max: 4.0,
                },
            ],
            exact: true,
        };
        let columns = envelope.columns(0..30, 1);
        assert_eq!(
            columns,
            [Peak {
                min: -3.0,
                max: 40.0
            }]
        );
        assert_eq!(
            envelope.columns(10..20, 1),
            [Peak {
                min: -2.0,
                max: 40.0
            }]
        );
    }
}
