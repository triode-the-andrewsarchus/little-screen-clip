//! Ordered, bounded waveform envelopes for SPICE previews.

use super::MAX_WAVEFORM_POINTS;
use crate::eda::check_cancelled;
use std::ops::Range;
use std::sync::atomic::AtomicBool;

/// One retained sample at its original sweep coordinate.
#[derive(Clone, Copy, Debug)]
pub struct SpicePreviewSample {
    pub x: f64,
    pub value: f64,
    pub phase_deg: f64,
    pub source_index: usize,
    run: usize,
}

impl SpicePreviewSample {
    /// Reports whether two samples in a sweep belong to the same continuous run.
    pub fn shares_run_with(&self, other: &Self) -> bool {
        self.run == other.run
    }

    /// Returns the displayed ordinate for magnitude or phase.
    pub fn ordinate(&self, phase: bool) -> f64 {
        if phase { self.phase_deg } else { self.value }
    }
}

#[derive(Clone, Debug)]
pub struct SpiceRawTrace {
    pub name: String,
    pub unit_type: String,
    pub min: f64,
    pub max: f64,
    samples: Vec<SpicePreviewSample>,
    sweep_ends: Vec<usize>,
}

impl SpiceRawTrace {
    pub(super) fn new(name: String, unit_type: String) -> Self {
        Self {
            name,
            unit_type,
            min: 0.0,
            max: 1.0,
            samples: Vec::new(),
            sweep_ends: Vec::new(),
        }
    }

    /// Returns preview samples in source order.
    pub fn samples(&self) -> &[SpicePreviewSample] {
        &self.samples
    }

    /// Returns independent sweeps in source order, including empty sweeps.
    pub fn sweeps(&self) -> impl Iterator<Item = &[SpicePreviewSample]> {
        self.sweep_ends.iter().scan(0, |start, &end| {
            Some(&self.samples[std::mem::replace(start, end)..end])
        })
    }

    /// Looks up a retained sample by its original row in the raw plot.
    pub fn sample_at(&self, source_index: usize) -> Option<&SpicePreviewSample> {
        self.samples
            .binary_search_by_key(&source_index, |sample| sample.source_index)
            .ok()
            .map(|index| &self.samples[index])
    }

    fn finish(&mut self) {
        let mut bounds = None;
        for sample in &self.samples {
            extend_range(&mut bounds, sample.value);
        }
        [self.min, self.max] = bounds.unwrap_or([0.0, 1.0]);
    }
}

pub(super) type Value = (f64, f64);

pub(super) fn displayed_value(re: f64, im: f64, complex: bool) -> Value {
    if !complex {
        (re, 0.0)
    } else if re.is_finite() && im.is_finite() {
        (
            20.0 * re.hypot(im).max(1e-18).log10(),
            im.atan2(re).to_degrees(),
        )
    } else {
        (f64::NAN, f64::NAN)
    }
}

pub(super) struct WaveformPreview {
    pub x_range: Option<[f64; 2]>,
    pub traces: Vec<SpiceRawTrace>,
}

pub(super) fn reduce(
    sweeps: &[Range<usize>],
    traces: Vec<SpiceRawTrace>,
    complex: bool,
    mut next_row: impl FnMut(usize, &mut [Option<Value>]) -> Result<f64, String>,
    cancelled: &AtomicBool,
) -> Result<WaveformPreview, String> {
    let mut preview = WaveformPreview {
        x_range: None,
        traces,
    };
    let mut row = vec![None; preview.traces.len()];
    let mut buckets: Vec<Bucket> = (0..row.len()).map(|_| Bucket::default()).collect();
    let mut runs = vec![0; row.len()];

    for (sweep, width) in bucket_widths(sweeps, complex) {
        for start in (sweep.start..sweep.end).step_by(width) {
            for index in start..(start + width).min(sweep.end) {
                if index.is_multiple_of(512) {
                    check_cancelled(cancelled, "SPICE waveform reduction cancelled")?;
                }
                let x = next_row(index, &mut row)?;
                if x.is_finite() {
                    extend_range(&mut preview.x_range, x);
                }
                for ((bucket, run), value) in buckets.iter_mut().zip(&mut runs).zip(&row) {
                    if let Some((value, phase_deg)) = *value
                        && x.is_finite()
                        && value.is_finite()
                        && phase_deg.is_finite()
                    {
                        bucket.push(SpicePreviewSample {
                            x,
                            value,
                            phase_deg,
                            source_index: index,
                            run: *run,
                        });
                    } else {
                        // A representative on either side of a gap keeps its run
                        // identity even when the missing row itself is dropped.
                        *run += 1;
                    }
                }
            }
            for (trace, bucket) in preview.traces.iter_mut().zip(&mut buckets) {
                std::mem::take(bucket).extend(&mut trace.samples);
            }
        }
        for trace in &mut preview.traces {
            trace.sweep_ends.push(trace.samples.len());
        }
    }
    preview.traces.iter_mut().for_each(SpiceRawTrace::finish);
    Ok(preview)
}

fn bucket_widths(
    sweeps: &[Range<usize>],
    complex: bool,
) -> impl Iterator<Item = (&Range<usize>, usize)> {
    let total = sweeps.last().map_or(0, |sweep| sweep.end);
    // Keep first/last and both signed extrema per bucket. Complex traces
    // also reserve two sample positions for phase extrema.
    let per_bucket = if complex { 6 } else { 4 };
    let spare = MAX_WAVEFORM_POINTS / per_bucket - sweeps.len();
    sweeps.iter().map(move |sweep| {
        let width = if total <= MAX_WAVEFORM_POINTS {
            1
        } else {
            // Every sweep gets a bucket. Allocate the rest by source length.
            let fraction =
                |position: usize| (spare as u128 * position as u128 / total as u128) as usize;
            let buckets = 1 + fraction(sweep.end) - fraction(sweep.start);
            sweep.len().div_ceil(buckets)
        };
        (sweep, width.max(1))
    })
}

fn extend_range(range: &mut Option<[f64; 2]>, x: f64) {
    let [min, max] = range.get_or_insert([x; 2]);
    *min = min.min(x);
    *max = max.max(x);
}

#[derive(Default)]
struct Bucket {
    first: Option<SpicePreviewSample>,
    last: Option<SpicePreviewSample>,
    low: Option<SpicePreviewSample>,
    high: Option<SpicePreviewSample>,
    low_phase: Option<SpicePreviewSample>,
    high_phase: Option<SpicePreviewSample>,
}

impl Bucket {
    fn push(&mut self, sample: SpicePreviewSample) {
        self.first.get_or_insert(sample);
        self.last = Some(sample);
        if self.low.is_none_or(|old| sample.value < old.value) {
            self.low = Some(sample);
        }
        if self.high.is_none_or(|old| sample.value > old.value) {
            self.high = Some(sample);
        }
        if self
            .low_phase
            .is_none_or(|old| sample.phase_deg < old.phase_deg)
        {
            self.low_phase = Some(sample);
        }
        if self
            .high_phase
            .is_none_or(|old| sample.phase_deg > old.phase_deg)
        {
            self.high_phase = Some(sample);
        }
    }

    fn extend(self, output: &mut Vec<SpicePreviewSample>) {
        let mut ordered = [
            self.first,
            self.last,
            self.low,
            self.high,
            self.low_phase,
            self.high_phase,
        ];
        ordered.sort_unstable_by_key(|entry| entry.map(|sample| sample.source_index));
        for sample in ordered.into_iter().flatten() {
            if output
                .last()
                .is_none_or(|last| last.source_index != sample.source_index)
            {
                output.push(sample);
            }
        }
    }
}
