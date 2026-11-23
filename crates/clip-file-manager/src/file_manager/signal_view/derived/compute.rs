use super::super::{
    inputs::{Input, Samples},
    spectra,
};
use crate::file_manager::scope_view::reads::RawChannel;
use little_clip_core::engineering::{
    analysis::{SignalGraph, SignalType, Value},
    logic::{self, Bin, Edge, Level, Trace},
    scope::Channel,
    waveforms::{self, Peak},
};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_ROWS: f64 = 131_072.0;
const DISPLAY_BINS: usize = 1024;

pub(super) struct Frame {
    pub span: Range<f64>,
    pub step: f64,
    pub lanes: Vec<Lane>,
    pub digital: Vec<logic::Lane>,
}

pub(super) enum Lane {
    Analog {
        channel: Channel,
        raw: Arc<RawChannel>,
        envelope: waveforms::Envelope,
        bounds: Peak,
    },
    Digital {
        name: String,
        raw: Arc<RawChannel>,
        trace: Trace,
        envelope: logic::Envelope,
    },
}

impl Lane {
    pub(super) fn name(&self) -> &str {
        match self {
            Self::Analog { channel, .. } => &channel.name,
            Self::Digital { name, .. } => name,
        }
    }

    pub(super) fn spectrum_input(&self) -> spectra::Input {
        match self {
            Self::Analog { channel, raw, .. } => {
                spectra::Input::derived(channel.name.clone(), channel.unit.clone(), raw.clone())
            }
            Self::Digital { name, raw, .. } => {
                spectra::Input::derived(name.clone(), String::new(), raw.clone())
            }
        }
    }
}

pub(super) fn calculate(
    source: &str,
    inputs: &[Input],
    span: Range<f64>,
    rate: Option<f64>,
    cancel: &AtomicBool,
) -> Result<Frame, String> {
    let bindings = inputs.iter().map(Input::binding).collect::<Vec<_>>();
    let graph = SignalGraph::compile(source, &bindings, 0).map_err(|error| error.to_string())?;
    let rate = rate
        .or_else(|| {
            graph
                .used_inputs()
                .iter()
                .filter_map(|&index| inputs[index].rate())
                .max_by(f64::total_cmp)
        })
        .or_else(|| inputs.first().and_then(Input::rate))
        .ok_or("Set a sample rate for this event-based capture.")?;
    let (range, count) = grid(span, rate)?;
    let mut samples: Vec<_> = (0..inputs.len()).map(|_| Samples::Empty).collect();
    for &index in graph.used_inputs() {
        samples[index] = inputs[index].read(&range, cancel)?;
    }
    let step = rate.recip();
    let columns = evaluate(&graph, &samples, range.start, step, count, cancel)?;
    let lanes = graph
        .lanes()
        .iter()
        .zip(columns)
        .map(|(lane, values)| match lane.signal_type {
            SignalType::Analog(dimension) => analog(
                lane.name.clone(),
                dimension.to_string(),
                &range,
                step,
                values,
            ),
            SignalType::Digital => digital(lane.name.clone(), &range, step, values),
        })
        .collect();
    let digital = graph
        .lanes()
        .iter()
        .filter(|lane| lane.signal_type == SignalType::Digital)
        .map(|lane| logic::Lane {
            name: lane.name.clone(),
            start_seconds: range.start,
            end_seconds: range.end,
            sample_rate: Some(rate),
        })
        .collect();
    Ok(Frame {
        span: range,
        step,
        lanes,
        digital,
    })
}

fn evaluate(
    graph: &SignalGraph,
    samples: &[Samples],
    start: f64,
    step: f64,
    count: usize,
    cancel: &AtomicBool,
) -> Result<Vec<Vec<f32>>, String> {
    let mut evaluator = graph.evaluator();
    let mut columns = graph
        .lanes()
        .iter()
        .map(|_| Vec::with_capacity(count))
        .collect::<Vec<_>>();
    for row in 0..count {
        if row % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return Err("Derived lane calculation cancelled".into());
        }
        let time = (row as f64).mul_add(step, start);
        for (column, result) in columns
            .iter_mut()
            .zip(evaluator.evaluate(|index| samples[index].at(time)))
        {
            let sample = match result {
                Some(Value::Numeric(value)) => *value as f32,
                Some(Value::Predicate(level)) => f32::from(u8::from(*level)),
                None => f32::NAN,
            };
            column.push(if sample.is_finite() { sample } else { f32::NAN });
        }
    }
    Ok(columns)
}

fn grid(span: Range<f64>, rate: f64) -> Result<(Range<f64>, usize), String> {
    let first = (span.start * rate).ceil();
    let last = (span.end * rate).ceil();
    let count = last - first;
    if !(1.0..=MAX_ROWS).contains(&count) || !rate.is_finite() || rate <= 0.0 {
        return Err(format!(
            "Zoom to a region of 1 to {MAX_ROWS:.0} derived samples or lower the sample rate."
        ));
    }
    Ok((first / rate..last / rate, count as usize))
}

pub(super) fn analog(
    name: String,
    unit: String,
    span: &Range<f64>,
    step: f64,
    values: Vec<f32>,
) -> Lane {
    let channel = Channel {
        name,
        unit,
        sample_count: values.len() as u64,
        start_seconds: span.start,
        interval_seconds: step,
    };
    let step = values.len().div_ceil(DISPLAY_BINS).max(1);
    let peaks = values
        .chunks(step)
        .map(|chunk| {
            let mut peak = Peak::default();
            peak.add(chunk);
            peak
        })
        .collect();
    let envelope = waveforms::Envelope {
        start: 0,
        step: step as u64,
        peaks,
        exact: true,
    };
    let bounds = envelope.extrema();
    Lane::Analog {
        raw: Arc::new(RawChannel {
            first: 0,
            start: span.start,
            step: channel.interval_seconds,
            samples: values,
        }),
        channel,
        envelope,
        bounds,
    }
}

fn digital(name: String, span: &Range<f64>, step: f64, values: Vec<f32>) -> Lane {
    let value = |index| level(values[index]);
    let mut edges = Vec::new();
    let mut previous = value(0);
    for index in 1..values.len() {
        let next = value(index);
        if next != previous {
            edges.push(Edge {
                time_seconds: (index as f64).mul_add(step, span.start),
                level: next,
            });
            previous = next;
        }
    }
    let trace = Trace {
        initial: Some(value(0)),
        edges,
        inspected_until: span.end,
    };
    let count = values.len().min(DISPLAY_BINS);
    let bins = (0..count)
        .map(|index| {
            let first = index * values.len() / count;
            let end = (index + 1) * values.len() / count;
            let mut bin = Bin {
                first: Some(value(first)),
                last: Some(value(end - 1)),
                exact: true,
                ..Default::default()
            };
            for sample in first..end {
                match value(sample) {
                    Level::High => bin.high = true,
                    Level::Low => bin.low = true,
                    _ => bin.unknown = true,
                }
            }
            bin
        })
        .collect();
    let envelope = logic::Envelope {
        time: span.clone(),
        bins,
    };
    Lane::Digital {
        name,
        raw: Arc::new(RawChannel {
            first: 0,
            start: span.start,
            step,
            samples: values,
        }),
        trace,
        envelope,
    }
}

fn level(sample: f32) -> Level {
    match sample {
        0.0 => Level::Low,
        1.0 => Level::High,
        _ => Level::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panning_preserves_the_same_derived_sample_clock() {
        let (first, n) = grid(-1.52..-0.51, 4.0).unwrap();
        let (next, m) = grid(-1.27..-0.15, 4.0).unwrap();
        assert_eq!((first, n), (-1.5..-0.5, 4));
        assert_eq!((next, m), (-1.25..0.0, 5));
    }

    #[test]
    fn derived_digital_gaps_keep_their_exact_transition_times() {
        let Lane::Digital { trace, .. } = digital(
            "gate".into(),
            &(0.0..1.0),
            0.25,
            vec![0.0, 1.0, f32::NAN, 1.0],
        ) else {
            unreachable!()
        };
        assert_eq!(trace.initial, Some(Level::Low));
        assert_eq!(
            trace.edges[1],
            Edge {
                time_seconds: 0.5,
                level: Level::Unknown
            }
        );
    }
}
