use crate::file_manager::{
    logic_view::protocols::TraceInput,
    scope_view::{FilterSamples, FilteredChannel, ThresholdInput, reads::RawChannel},
};
use little_clip_core::engineering::{
    analysis::{Binding, Clock, Dimension, SignalType, Value},
    logic::{Level, Trace},
    logic_decode::{PcmLane, PwmLane, PwmMeasurement},
    logic_preview,
    scope::Channel,
    waveforms::{Recording, sample_span, time_span},
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

const MAX_INPUT_SAMPLES: u64 = 1 << 20;
const MAX_EDGES: usize = MAX_INPUT_SAMPLES as usize;

#[derive(Clone)]
pub(super) struct Input {
    pub alias: String,
    pub label: String,
    clock: Clock,
    source: Source,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Signature {
    alias: String,
    clock: [u64; 3],
    generation: Option<u64>,
    missing: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SavedInput {
    pub alias: String,
    label: String,
    kind: SignalType,
    rate: Option<f64>,
}

#[derive(Clone)]
enum Source {
    Missing {
        kind: SignalType,
        rate: Option<f64>,
    },
    Analog {
        reader: Arc<Recording>,
        index: usize,
    },
    Filtered {
        dimension: Dimension,
        rate: f64,
        samples: Option<FilterSamples>,
    },
    Threshold {
        reader: Arc<ThresholdInput>,
        index: usize,
    },
    Digital {
        reader: Arc<logic_preview::Recording>,
        index: usize,
        rate: Option<f64>,
    },
    Measured {
        lane: Measurement,
        generation: u64,
    },
}

pub(super) enum Samples {
    Analog {
        channel: Channel,
        first: u64,
        values: Vec<f32>,
        clock: Clock,
    },
    Filtered {
        raw: Arc<RawChannel>,
        clock: Clock,
    },
    Digital {
        trace: Trace,
        native: Range<f64>,
        clock: Clock,
    },
    Measured {
        lane: Measurement,
        clock: Clock,
    },
    Empty,
}

#[derive(Clone)]
pub(super) enum Measurement {
    Pcm(Arc<PcmLane>),
    Pwm(Arc<PwmLane>, PwmMeasurement),
}

impl Measurement {
    fn dimension(&self) -> Dimension {
        match self {
            Self::Pcm(_) | Self::Pwm(_, PwmMeasurement::Duty) => Dimension::SCALAR,
            Self::Pwm(_, PwmMeasurement::Frequency) => Dimension::FREQUENCY,
            Self::Pwm(_, PwmMeasurement::ActiveTime | PwmMeasurement::Period) => Dimension::TIME,
        }
    }

    fn rate(&self) -> f64 {
        match self {
            Self::Pcm(lane) => lane.interval().recip(),
            Self::Pwm(lane, _) => lane.grid_rate(),
        }
    }

    fn value_at(&self, workspace: f64, clock: Clock) -> Option<f64> {
        let time = clock.to_native(workspace);
        match self {
            Self::Pcm(lane) => lane.value_at(time),
            Self::Pwm(lane, measure) => {
                let measured = lane.value_at(time, *measure)?;
                Some(match measure {
                    PwmMeasurement::Frequency => measured / clock.scale(),
                    PwmMeasurement::ActiveTime | PwmMeasurement::Period => measured * clock.scale(),
                    PwmMeasurement::Duty => measured,
                })
            }
        }
    }
}

impl Input {
    pub(super) fn snapshot(&self) -> SavedInput {
        SavedInput {
            alias: self.alias.clone(),
            label: self.label.clone(),
            kind: self.binding().signal_type,
            rate: self.native_rate(),
        }
    }

    pub(super) fn missing(saved: &SavedInput, clock: Clock) -> Self {
        Self {
            alias: saved.alias.clone(),
            label: format!("{} (unavailable)", saved.label),
            clock,
            source: Source::Missing {
                kind: saved.kind,
                rate: saved.rate,
            },
        }
    }

    pub(super) fn analog_alias(id: u64, index: usize) -> String {
        format!("s{id}_c{}", index + 1)
    }

    pub(super) fn analog(id: u64, clock: Clock, label: &str, reader: &Arc<Recording>) -> Vec<Self> {
        reader
            .channels()
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| {
                Dimension::from_unit(&channel.unit)?;
                Some(Self {
                    alias: Self::analog_alias(id, index),
                    label: format!("{label} · {}", channel.name),
                    clock,
                    source: Source::Analog {
                        reader: reader.clone(),
                        index,
                    },
                })
            })
            .collect()
    }

    pub(super) fn filtered(
        id: u64,
        clock: Clock,
        label: &str,
        channels: &[Channel],
        filtered: FilteredChannel<'_>,
    ) -> Option<Self> {
        let index = filtered.channel;
        let channel = channels.get(index)?;
        let name = filtered.name;
        Some(Self {
            alias: format!("s{id}_f{}", index + 1),
            label: format!("{label} · {name}"),
            clock,
            source: Source::Filtered {
                dimension: Dimension::from_unit(&channel.unit)?,
                rate: channel.interval_seconds.recip(),
                samples: filtered.samples,
            },
        })
    }

    pub(super) fn digital(
        id: u64,
        clock: Clock,
        label: &str,
        reader: &Arc<logic_preview::Recording>,
        lanes: &[little_clip_core::engineering::logic::Lane],
    ) -> Vec<Self> {
        lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| Self {
                alias: format!("s{id}_c{}", index + 1),
                label: format!("{label} · {}", lane.name),
                clock,
                source: Source::Digital {
                    reader: reader.clone(),
                    index,
                    rate: lane.sample_rate,
                },
            })
            .collect()
    }

    pub(super) fn threshold(
        id: u64,
        clock: Clock,
        label: &str,
        reader: &Arc<ThresholdInput>,
    ) -> Vec<Self> {
        reader
            .lanes()
            .iter()
            .enumerate()
            .map(|(index, lane)| Self {
                alias: format!("s{id}_d{}", index + 1),
                label: format!("{label} · {}", lane.name),
                clock,
                source: Source::Threshold {
                    reader: reader.clone(),
                    index,
                },
            })
            .collect()
    }

    pub(super) fn clock(&self) -> Clock {
        self.clock
    }

    pub(super) fn pcm(
        id: u64,
        clock: Clock,
        label: &str,
        channel: usize,
        generation: u64,
        lane: &Arc<PcmLane>,
    ) -> Self {
        let (suffix, side) = if channel == 0 {
            ("l", "Left")
        } else {
            ("r", "Right")
        };
        Self {
            alias: format!("s{id}_pcm_{suffix}"),
            label: format!("{label} · {side} PCM (full scale)"),
            clock,
            source: Source::Measured {
                lane: Measurement::Pcm(lane.clone()),
                generation,
            },
        }
    }

    pub(super) fn pwm(
        id: u64,
        clock: Clock,
        label: &str,
        generation: u64,
        lane: &Arc<PwmLane>,
    ) -> [Self; 4] {
        [
            (PwmMeasurement::Duty, "duty", "Duty (0–1)"),
            (PwmMeasurement::Frequency, "freq", "Frequency (Hz)"),
            (PwmMeasurement::ActiveTime, "active", "Active time (s)"),
            (PwmMeasurement::Period, "period", "Period (s)"),
        ]
        .map(|(measure, suffix, name)| Self {
            alias: format!("s{id}_pwm_{suffix}"),
            label: format!("{label} · PWM {name}"),
            clock,
            source: Source::Measured {
                lane: Measurement::Pwm(lane.clone(), measure),
                generation,
            },
        })
    }

    pub(super) fn signature(&self) -> Signature {
        let generation = match &self.source {
            Source::Filtered { samples, .. } => samples.as_ref().map(|samples| samples.generation),
            Source::Measured { generation, .. } => Some(*generation),
            Source::Threshold { reader, .. } => Some(reader.revision()),
            _ => None,
        };
        Signature {
            alias: self.alias.clone(),
            clock: self.clock.key(),
            generation,
            missing: matches!(self.source, Source::Missing { .. }),
        }
    }

    pub(super) fn is_awaiting_filter(&self) -> bool {
        matches!(self.source, Source::Filtered { samples: None, .. })
    }

    pub(super) fn unavailable(&self) -> bool {
        matches!(self.source, Source::Missing { .. })
    }

    pub(super) fn channel_names(inputs: &[Self]) -> std::collections::HashMap<String, &str> {
        let mut names = std::collections::HashMap::new();
        for input in inputs {
            let name = match &input.source {
                Source::Analog { reader, index } => reader.channels()[*index].name.to_lowercase(),
                Source::Threshold { reader, index } => {
                    format!("{}_d", reader.channel_name(*index).to_lowercase())
                }
                _ => continue,
            };
            names
                .entry(name)
                .and_modify(|alias| *alias = None)
                .or_insert(Some(input.alias.as_str()));
        }
        for input in inputs {
            names.remove(&input.alias);
        }
        names
            .into_iter()
            .filter_map(|(name, alias)| Some((name, alias?)))
            .collect()
    }

    pub(super) fn binding(&self) -> Binding<'_> {
        let signal_type = match &self.source {
            Source::Missing { kind, .. } => *kind,
            Source::Analog { reader, index } => {
                let unit = &reader.channels()[*index].unit;
                SignalType::Analog(Dimension::from_unit(unit).unwrap_or(Dimension::SCALAR))
            }
            Source::Filtered { dimension, .. } => SignalType::Analog(*dimension),
            Source::Digital { .. } | Source::Threshold { .. } => SignalType::Digital,
            Source::Measured { lane, .. } => SignalType::Analog(lane.dimension()),
        };
        Binding {
            name: &self.alias,
            signal_type,
        }
    }

    pub(super) fn rate(&self) -> Option<f64> {
        self.native_rate()
            .map(|rate| rate / self.clock.scale())
            .filter(|rate| rate.is_finite() && *rate > 0.0)
    }

    fn native_rate(&self) -> Option<f64> {
        match &self.source {
            Source::Missing { rate, .. } => *rate,
            Source::Analog { reader, index } => {
                Some(reader.channels()[*index].interval_seconds.recip())
            }
            Source::Filtered { rate, .. } => Some(*rate),
            Source::Digital { rate, .. } => *rate,
            Source::Threshold { reader, index } => reader.lanes()[*index].sample_rate,
            Source::Measured { lane, .. } => Some(lane.rate()),
        }
        .filter(|rate| rate.is_finite() && *rate > 0.0)
    }

    pub(super) fn read(&self, span: &Range<f64>, cancel: &AtomicBool) -> Result<Samples, String> {
        let native = self.clock.native_range(span);
        match &self.source {
            Source::Missing { .. } => Ok(Samples::Empty),
            Source::Analog { reader, index } => self.read_analog(reader, *index, native, cancel),
            Source::Filtered { samples, .. } => {
                Ok(samples
                    .as_ref()
                    .map_or(Samples::Empty, |samples| Samples::Filtered {
                        raw: samples.raw.clone(),
                        clock: self.clock,
                    }))
            }
            Source::Digital { reader, index, .. } => reader.checked(cancel, |capture| {
                self.read_logic(capture, *index, native, cancel)
            }),
            Source::Threshold { reader, index } => {
                self.read_logic(reader.as_ref(), *index, native, cancel)
            }
            Source::Measured { lane, .. } => Ok(Samples::Measured {
                lane: lane.clone(),
                clock: self.clock,
            }),
        }
    }

    fn read_logic(
        &self,
        input: &dyn TraceInput,
        index: usize,
        native: Range<f64>,
        cancel: &AtomicBool,
    ) -> Result<Samples, String> {
        let Some(bounds) = input.time_bounds(index) else {
            return Ok(Samples::Empty);
        };
        let native = native.start.max(bounds.start)..native.end.min(bounds.end);
        if native.is_empty() {
            return Ok(Samples::Empty);
        }
        let trace = input.read_edges(index, native.clone(), MAX_EDGES, cancel)?;
        Ok(Samples::Digital {
            trace,
            native,
            clock: self.clock,
        })
    }

    fn read_analog(
        &self,
        reader: &Recording,
        index: usize,
        native: Range<f64>,
        cancel: &AtomicBool,
    ) -> Result<Samples, String> {
        let channel = &reader.channels()[index];
        let bounds = time_span(std::slice::from_ref(channel));
        if native.start >= bounds.end || native.end <= bounds.start {
            return Ok(Samples::Empty);
        }
        let rows = sample_span(channel, native);
        let range =
            rows.start.saturating_sub(1)..rows.end.saturating_add(1).min(channel.sample_count);
        if range.end - range.start > MAX_INPUT_SAMPLES {
            return Err(format!(
                "Zoom in to read {} at its native rate.",
                self.label
            ));
        }
        let first = range.start;
        let values =
            reader.checked(cancel, |capture| capture.read_samples(index, range, cancel))?;
        Ok(Samples::Analog {
            channel: channel.clone(),
            first,
            values,
            clock: self.clock,
        })
    }
}

impl Samples {
    pub(super) fn at(&self, time: f64) -> Option<Value> {
        match self {
            Self::Analog {
                channel,
                first,
                values,
                clock,
            } => {
                let position =
                    (clock.to_native(time) - channel.time_at(*first)) / channel.interval_seconds;
                interpolate(values, position).map(Value::Numeric)
            }
            Self::Filtered { raw, clock } => {
                let position = (clock.to_native(time) - raw.start) / raw.step;
                interpolate(&raw.samples, position).map(Value::Numeric)
            }
            Self::Digital {
                trace,
                native,
                clock,
            } => {
                let time = clock.to_native(time);
                if !native.contains(&time) || time >= trace.inspected_until {
                    return None;
                }
                let last = trace
                    .edges
                    .partition_point(|edge| edge.time_seconds <= time);
                let level = last
                    .checked_sub(1)
                    .map(|index| trace.edges[index].level)
                    .or(trace.initial)?;
                match level {
                    Level::Low => Some(Value::Predicate(false)),
                    Level::High => Some(Value::Predicate(true)),
                    _ => None,
                }
            }
            Self::Measured { lane, clock } => lane.value_at(time, *clock).map(Value::Numeric),
            Self::Empty => None,
        }
    }
}

fn interpolate(values: &[f32], position: f64) -> Option<f64> {
    let nearest = position.round();
    // Floating-point projection of a point on its native grid can differ by
    // a fraction of a sample. Exact samples need only their own value.
    if (position - nearest).abs() < 1e-8 && nearest >= 0.0 {
        let value = f64::from(*values.get(nearest as usize)?);
        return value.is_finite().then_some(value);
    }
    if position < 0.0 || !position.is_finite() {
        return None;
    }
    let left = position.floor() as usize;
    let a = f64::from(*values.get(left)?);
    let b = f64::from(*values.get(left.checked_add(1)?)?);
    let value = (b - a).mul_add(position - left as f64, a);
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwm_inputs_keep_physical_units_and_follow_source_alignment() {
        use little_clip_core::engineering::{
            analysis::SignalGraph,
            logic_decode::{Event, Kind, PwmCycle},
        };
        let pin = PwmLane::from_events(
            &[Event {
                time: 0.002..0.003,
                kind: Kind::PwmCycle(PwmCycle {
                    active_seconds: 0.00025,
                    period_seconds: 0.001,
                }),
            }],
            &(0.0..0.004),
        )
        .unwrap();
        let pin = Arc::new(pin);
        let inputs = Input::pwm(2, Clock::offset(0.01), "Driver", 1, &pin);
        let bindings = inputs.each_ref().map(Input::binding);
        let graph = SignalGraph::compile(
            "gate = s2_pwm_freq > 500Hz\nvoltage = s2_pwm_active / s2_pwm_period * 4V",
            &bindings,
            0,
        )
        .unwrap();
        assert!(SignalGraph::compile("bad = s2_pwm_period + 1Hz", &bindings, 0).is_err());
        let cancel = AtomicBool::new(false);
        let data = inputs.map(|input| input.read(&(0.0..0.02), &cancel).unwrap());
        let mut evaluator = graph.evaluator();
        assert_eq!(
            evaluator.evaluate(|index| data[index].at(0.0125)),
            &[Some(Value::Predicate(true)), Some(Value::Numeric(1.0))]
        );
        assert_eq!(
            evaluator.evaluate(|index| data[index].at(0.011)),
            &[None, None]
        );

        let clock = Clock::anchored(0.0, 0.01, 2.0).unwrap();
        let mut frequency = Input::pwm(2, clock, "Driver", 1, &pin)[1].clone();
        let data = frequency.read(&(0.0..0.02), &cancel).unwrap();
        assert_eq!(data.at(0.015), Some(Value::Numeric(500.0)));
        let signature = frequency.signature();
        frequency.clock = Clock::offset(0.01);
        assert!(frequency.signature() != signature);
    }

    #[test]
    fn changed_filter_results_keep_their_name_but_invalidate_derived_readers() {
        let channels = [Channel {
            name: "CH1".into(),
            unit: "V".into(),
            sample_count: 8,
            start_seconds: 0.0,
            interval_seconds: 0.25,
        }];
        let raw = Arc::new(RawChannel {
            first: 2,
            start: 0.5,
            step: 0.25,
            samples: vec![1.0, 2.0, f32::NAN],
        });
        let input = |generation: Option<u64>| {
            Input::filtered(
                3,
                Clock::offset(1.0),
                "capture",
                &channels,
                FilteredChannel {
                    channel: 0,
                    name: "Example filter",
                    samples: generation.map(|generation| FilterSamples {
                        generation,
                        raw: raw.clone(),
                    }),
                },
            )
            .unwrap()
        };
        let pending = input(None::<u64>);
        let ready = input(Some(1));
        let changed = input(Some(2));
        assert_eq!(pending.alias, "s3_f1");
        assert_eq!(pending.alias, ready.alias);
        assert!(pending.signature() != ready.signature());
        assert!(ready.signature() != changed.signature());
        let cancelled = AtomicBool::new(false);
        let span = 1.0..2.5;
        let data = ready.read(&span, &cancelled).unwrap();
        assert!(matches!(data.at(1.625), Some(Value::Numeric(1.5))));
        assert!(data.at(1.4).is_none());
        assert!(data.at(1.9).is_none());
        assert!(pending.read(&span, &cancelled).unwrap().at(1.625).is_none());
    }

    #[test]
    fn interpolation_keeps_native_observations_and_does_not_bridge_missing_samples() {
        let samples = [1.0, 3.0, f32::NAN, 5.0];
        assert_eq!(interpolate(&samples, -1e-10), Some(1.0));
        assert_eq!(interpolate(&samples, 0.25), Some(1.5));
        assert_eq!(interpolate(&samples, 1.0), Some(3.0));
        assert_eq!(interpolate(&samples, 1.25), None);
        assert_eq!(interpolate(&samples, 2.75), None);
        assert_eq!(interpolate(&samples, 4.0), None);
    }
}
