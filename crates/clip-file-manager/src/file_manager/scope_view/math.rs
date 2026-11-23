use super::{
    plot,
    reads::{self, RawChannel},
};
use crate::file_manager::preview_job::Task;
use egui::{Color32, Context, Painter, Rect};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::{Expression, Value},
        scope::Channel,
        waveforms::{Capture, Envelope, Recording, sample_span},
    },
};
use std::{
    ops::Range,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const DETAIL_ROWS: u64 = 4096;
const DISPLAY_BINS: u64 = 1024;
const EXACT_LIMIT: u64 = 2_000_000;
const READ_ROWS: u64 = 8192;
const SAMPLED_ROWS: u64 = 48;
const HIGHLIGHT: Color32 = Color32::from_rgb(235, 242, 125);

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));

#[derive(Default)]
pub(super) struct Math {
    input: String,
    channel: usize,
    expression: Option<Expression>,
    error: Option<String>,
    simple: bool,
    ready: Option<Map>,
    job: Option<Pending>,
    failed: Option<Range<u64>>,
}

struct Pending {
    rows: Range<u64>,
    task: Task<Result<Map, String>>,
}

impl Math {
    pub fn compile(&mut self, input: &str, active: usize, channels: &[Channel]) {
        if self.input == input && self.channel == active {
            return;
        }
        self.input.clear();
        self.input.push_str(input);
        self.channel = active;
        self.simple = false;
        self.expression = None;
        self.error = None;
        self.ready = None;
        self.job = None;
        self.failed = None;
        if input.trim().is_empty() {
            return;
        }
        if channels[active].unit != "V" {
            self.error = Some("Voltage expressions require an active voltage channel".into());
            return;
        }
        let names: Vec<_> = channels
            .iter()
            .map(|channel| {
                if channel.unit == "V" {
                    channel.name.as_str()
                } else {
                    ""
                }
            })
            .collect();
        match Expression::parse(input, &names, active) {
            Ok(expression) => {
                self.simple = expression.is_predicate() && monotone_shorthand(input);
                self.expression = Some(expression);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn pending(&self) -> bool {
        self.job.is_some()
    }

    pub fn numeric(&self) -> Option<&Expression> {
        self.expression
            .as_ref()
            .filter(|expression| !expression.is_predicate())
    }

    pub fn poll(&mut self) {
        let Some(result) = self.job.as_ref().and_then(|job| job.task.poll()) else {
            return;
        };
        let Some(job) = self.job.take() else {
            return;
        };
        match result
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        {
            Ok(map) => {
                self.ready = Some(map);
                self.failed = None;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                self.failed = Some(job.rows);
            }
        }
    }

    pub fn request(&mut self, recording: &Arc<Recording>, ctx: &Context, view: &Range<f64>) {
        let Some(expression) = self.predicate() else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let channel = &recording.channels()[self.channel];
        let rows = sample_span(channel, view.clone());
        let size = rows.end.saturating_sub(rows.start);
        if size == 0 || self.simple && size > DETAIL_ROWS {
            return;
        }
        let ideal_step = bin_step(size);
        if self
            .ready
            .as_ref()
            .is_some_and(|map| map.covers(&rows) && map.step <= ideal_step.saturating_mul(2))
        {
            return;
        }
        let padding = if size <= DETAIL_ROWS {
            size / 2
        } else {
            size / 4
        };
        let rows = rows.start.saturating_sub(padding)
            ..rows.end.saturating_add(padding).min(channel.sample_count);
        if self.failed.as_ref() == Some(&rows) {
            return;
        }
        let request = rows.clone();
        let expression = expression.clone();
        let recording = recording.clone();
        let ctx = ctx.clone();
        let active = self.channel;
        self.job = Some(Pending {
            rows,
            task: Task::start(&JOBS, Duration::from_secs(45), move |cancel| {
                let result = recording.checked(cancel, |capture| {
                    read_predicate(capture, active, request, &expression, cancel)
                });
                ctx.request_repaint();
                result
            }),
        });
    }

    pub fn status(
        &self,
        channel: &Channel,
        view: &Range<f64>,
        envelope: &Envelope,
    ) -> Option<&'static str> {
        self.predicate()?;
        let rows = sample_span(channel, view.clone());
        if let Some(map) = self.map_for(&rows) {
            if map.sampled {
                return Some("Math sampled across the visible interval");
            }
            return (map.step > 1).then_some("Highlight shows bins with matches");
        }
        if self.simple {
            return Some(if envelope.exact {
                "Highlight shows bins with matches"
            } else {
                "Highlight uses sampled waveform bins"
            });
        }
        self.job.is_some().then_some("Checking visible interval…")
    }

    pub fn paint(
        &self,
        painter: &Painter,
        rect: Rect,
        view: &Range<f64>,
        channel: &Channel,
        envelope: &Envelope,
    ) {
        let Some(expression) = self.predicate() else {
            return;
        };
        let rows = sample_span(channel, view.clone());
        if rows.is_empty() {
            return;
        }
        if let Some(map) = self.map_for(&rows) {
            paint_runs(painter, rect, view, channel, map.runs.iter().cloned());
        } else if self.simple {
            let runs = envelope_matches(envelope, rows, rect.width().ceil() as usize, expression);
            paint_runs(painter, rect, view, channel, runs);
        }
    }

    fn predicate(&self) -> Option<&Expression> {
        self.expression
            .as_ref()
            .filter(|expression| expression.is_predicate())
    }

    fn map_for(&self, rows: &Range<u64>) -> Option<&Map> {
        self.ready.as_ref().filter(|map| map.covers(rows))
    }
}

/// Only ordering comparisons against a numeric literal are sound to test at
/// both extrema. All other valid expressions use direct sample evaluation.
fn monotone_shorthand(input: &str) -> bool {
    let input = input.trim_start();
    let Some(literal) = [">=", "<=", ">", "<"]
        .into_iter()
        .find_map(|op| input.strip_prefix(op))
    else {
        return false;
    };
    literal
        .trim()
        .trim_end_matches(char::is_alphabetic)
        .trim_end()
        .parse::<f64>()
        .is_ok_and(f64::is_finite)
}

fn envelope_matches(
    envelope: &Envelope,
    rows: Range<u64>,
    columns: usize,
    expression: &Expression,
) -> impl Iterator<Item = Range<u64>> {
    let length = rows.end - rows.start;
    let columns = columns.clamp(1, 2048);
    let peaks = envelope.columns(rows.clone(), columns);
    let width = peaks.len().max(1) as u64;
    peaks
        .into_iter()
        .enumerate()
        .filter_map(move |(index, peak)| {
            let yes = peak.valid()
                && [peak.min, peak.max].into_iter().any(|value| {
                    matches!(
                        expression.evaluate(|_| Some(f64::from(value))),
                        Some(Value::Predicate(true))
                    )
                });
            yes.then(|| {
                let index = index as u64;
                let edge = |n| rows.start + length / width * n + (length % width * n) / width;
                edge(index)..edge(index + 1)
            })
        })
}

fn paint_runs(
    painter: &Painter,
    rect: Rect,
    view: &Range<f64>,
    channel: &Channel,
    runs: impl Iterator<Item = Range<u64>>,
) {
    let pixel = (view.end - view.start) / f64::from(rect.width().max(1.0));
    let mut last: Option<Range<f64>> = None;
    let color = HIGHLIGHT.gamma_multiply(0.28);
    for run in runs {
        let span = channel.time_at(run.start)..channel.time_at(run.end);
        if span.end <= view.start || span.start >= view.end {
            continue;
        }
        if let Some(previous) = &mut last {
            if span.start - previous.end <= pixel {
                previous.end = span.end;
                continue;
            }
            plot::highlight(painter, rect, view, previous.clone(), color);
        }
        last = Some(span);
    }
    if let Some(span) = last {
        plot::highlight(painter, rect, view, span, color);
    }
}

struct Map {
    rows: Range<u64>,
    step: u64,
    sampled: bool,
    runs: Vec<Range<u64>>,
}

impl Map {
    fn covers(&self, rows: &Range<u64>) -> bool {
        self.rows.start <= rows.start && self.rows.end >= rows.end
    }
}

fn bin_step(count: u64) -> u64 {
    if count <= DETAIL_ROWS * 2 {
        1
    } else {
        count.div_ceil(DISPLAY_BINS).max(1)
    }
}

fn read_predicate(
    capture: &Capture,
    active: usize,
    rows: Range<u64>,
    expression: &Expression,
    cancel: &AtomicBool,
) -> Result<Map, String> {
    let channel = &capture.channels()[active];
    let length = rows.end - rows.start;
    let step = bin_step(length);
    let mut matches = vec![false; length.div_ceil(step) as usize];
    let fastest = capture
        .channels()
        .iter()
        .filter(|channel| channel.unit == "V")
        .map(|channel| channel.interval_seconds)
        .fold(channel.interval_seconds, f64::min);
    let stride =
        ((READ_ROWS as f64 * fastest / channel.interval_seconds) as u64).clamp(1, READ_ROWS);
    // Count the highest-rate channel in the exact-pass budget as well.
    let max_exact = (EXACT_LIMIT as f64 * fastest / channel.interval_seconds) as u64;
    let sampled = length > max_exact;
    let scan = Scan {
        capture,
        active,
        expression,
        cancel,
        stride,
    };
    if sampled {
        for bucket in chunks(rows.clone(), step) {
            let width = (bucket.end - bucket.start).min(SAMPLED_ROWS);
            let first = bucket.start + (bucket.end - bucket.start - width) / 2;
            scan.collect(first..first + width, rows.start, step, &mut matches)?;
        }
    } else {
        scan.collect(rows.clone(), rows.start, step, &mut matches)?;
    }
    let runs = match_runs(rows.clone(), step, matches);
    Ok(Map {
        rows,
        step,
        sampled,
        runs,
    })
}

struct Scan<'a> {
    capture: &'a Capture,
    active: usize,
    expression: &'a Expression,
    cancel: &'a AtomicBool,
    stride: u64,
}

impl Scan<'_> {
    fn collect(
        &self,
        rows: Range<u64>,
        first: u64,
        step: u64,
        bins: &mut [bool],
    ) -> Result<(), String> {
        let channels = self.capture.channels();
        let active = &channels[self.active];
        for chunk in chunks(rows, self.stride) {
            if self.cancel.load(Ordering::Relaxed) {
                return Err("Waveform math cancelled".into());
            }
            let times = active.time_at(chunk.start)..active.time_at(chunk.end);
            let data = channels
                .iter()
                .enumerate()
                .map(|(index, channel)| {
                    if channel.unit != "V" {
                        return Ok(None);
                    }
                    let rows = if index == self.active {
                        chunk.clone()
                    } else {
                        reads::padded_span(channel, &times, 1)
                    };
                    if rows.end.saturating_sub(rows.start) > READ_ROWS + 4 {
                        return Err(
                            "Voltage channels are too far apart in sample rate for math".into()
                        );
                    }
                    reads::read_raw(self.capture, index, rows, self.cancel).map(Some)
                })
                .collect::<Result<Vec<_>, String>>()?;
            for row in chunk {
                let bin = &mut bins[((row - first) / step) as usize];
                if !*bin {
                    *bin = matches_row(self.expression, &data, active.time_at(row));
                }
            }
        }
        Ok(())
    }
}

fn matches_row(expression: &Expression, channels: &[Option<RawChannel>], time: f64) -> bool {
    matches!(
        expression
            .evaluate(|index| { channels.get(index)?.as_ref()?.value_at(time).map(f64::from) }),
        Some(Value::Predicate(true))
    )
}

fn chunks(rows: Range<u64>, stride: u64) -> impl Iterator<Item = Range<u64>> {
    let mut start = rows.start;
    std::iter::from_fn(move || {
        let end = start.saturating_add(stride).min(rows.end);
        let chunk = start..end;
        start = end;
        (!chunk.is_empty()).then_some(chunk)
    })
}

fn match_runs(rows: Range<u64>, step: u64, bins: Vec<bool>) -> Vec<Range<u64>> {
    let mut runs = Vec::<Range<u64>>::new();
    for (index, _) in bins.into_iter().enumerate().filter(|(_, hit)| *hit) {
        let start = rows.start + index as u64 * step;
        let end = start.saturating_add(step).min(rows.end);
        if let Some(previous) = runs.last_mut()
            && previous.end == start
        {
            previous.end = end;
        } else {
            runs.push(start..end);
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::waveforms::Peak;

    #[test]
    fn broad_literal_highlight_retains_a_threshold_spike_outside_the_central_window() {
        let expression = Expression::parse("> 1V", &["CH1"], 0).unwrap();
        assert!(monotone_shorthand("> 1V"));
        assert!(monotone_shorthand("<= -1.5e-3 mV"));
        assert!(!monotone_shorthand("> CH1 - CH2"));
        assert!(!monotone_shorthand("> 1V && CH1 < 3V"));
        let mut peaks = vec![
            Peak {
                min: -0.5,
                max: 0.9
            };
            1024
        ];
        peaks[5].max = 1.8;
        let envelope = Envelope {
            start: 0,
            step: 1024,
            peaks,
            exact: true,
        };
        let runs = envelope_matches(&envelope, 0..1_048_576, 512, &expression).collect::<Vec<_>>();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0], 4096..6144);
    }

    #[test]
    fn exact_bins_preserve_middle_gaps_and_end_boundaries() {
        let runs = match_runs(18..37, 4, vec![false, true, true, false, true]);
        assert_eq!(runs, [22..30, 34..37]);
    }
}
