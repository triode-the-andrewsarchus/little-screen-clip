use super::timeline::translate;
use crate::file_manager::scope_view::{FilterSamples, FilteredChannel, reads::RawChannel};
use little_clip_core::engineering::{
    analysis::{Binding, Dimension, SignalType, Value},
    logic::{Level, Trace},
    logic_decode::PcmLane,
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
    offset: f64,
    source: Source,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Signature {
    alias: String,
    offset: u64,
    generation: Option<u64>,
}

#[derive(Clone)]
enum Source {
    Analog {
        reader: Arc<Recording>,
        index: usize,
    },
    Filtered {
        dimension: Dimension,
        rate: f64,
        samples: Option<FilterSamples>,
    },
    Digital {
        reader: Arc<logic_preview::Recording>,
        index: usize,
        rate: Option<f64>,
    },
    Pcm {
        lane: Arc<PcmLane>,
        generation: u64,
    },
}

pub(super) enum Samples {
    Analog {
        channel: Channel,
        first: u64,
        values: Vec<f32>,
        offset: f64,
    },
    Filtered {
        raw: Arc<RawChannel>,
        offset: f64,
    },
    Digital {
        trace: Trace,
        native: Range<f64>,
        offset: f64,
    },
    Pcm {
        lane: Arc<PcmLane>,
        offset: f64,
    },
    Empty,
}

impl Input {
    pub(super) fn analog(id: u64, offset: f64, label: &str, reader: &Arc<Recording>) -> Vec<Self> {
        reader
            .channels()
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| {
                Dimension::from_unit(&channel.unit)?;
                Some(Self {
                    alias: format!("s{id}_c{}", index + 1),
                    label: format!("{label} · {}", channel.name),
                    offset,
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
        offset: f64,
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
            offset,
            source: Source::Filtered {
                dimension: Dimension::from_unit(&channel.unit)?,
                rate: channel.interval_seconds.recip(),
                samples: filtered.samples,
            },
        })
    }

    pub(super) fn digital(
        id: u64,
        offset: f64,
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
                offset,
                source: Source::Digital {
                    reader: reader.clone(),
                    index,
                    rate: lane.sample_rate,
                },
            })
            .collect()
    }

    pub(super) fn offset(&self) -> f64 {
        self.offset
    }

    pub(super) fn pcm(
        id: u64,
        offset: f64,
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
            offset,
            source: Source::Pcm {
                lane: lane.clone(),
                generation,
            },
        }
    }

    pub(super) fn signature(&self) -> Signature {
        let generation = match &self.source {
            Source::Filtered { samples, .. } => samples.as_ref().map(|samples| samples.generation),
            Source::Pcm { generation, .. } => Some(*generation),
            _ => None,
        };
        Signature {
            alias: self.alias.clone(),
            offset: self.offset.to_bits(),
            generation,
        }
    }

    pub(super) fn is_awaiting_filter(&self) -> bool {
        matches!(self.source, Source::Filtered { samples: None, .. })
    }

    pub(super) fn binding(&self) -> Binding<'_> {
        let signal_type = match &self.source {
            Source::Analog { reader, index } => {
                let unit = &reader.channels()[*index].unit;
                SignalType::Analog(Dimension::from_unit(unit).unwrap_or(Dimension::SCALAR))
            }
            Source::Filtered { dimension, .. } => SignalType::Analog(*dimension),
            Source::Digital { .. } => SignalType::Digital,
            Source::Pcm { .. } => SignalType::Analog(Dimension::SCALAR),
        };
        Binding {
            name: &self.alias,
            signal_type,
        }
    }

    pub(super) fn rate(&self) -> Option<f64> {
        match &self.source {
            Source::Analog { reader, index } => {
                Some(reader.channels()[*index].interval_seconds.recip())
            }
            Source::Filtered { rate, .. } => Some(*rate),
            Source::Digital { rate, .. } => *rate,
            Source::Pcm { lane, .. } => Some(lane.interval().recip()),
        }
        .filter(|rate| rate.is_finite() && *rate > 0.0)
    }

    pub(super) fn read(&self, span: &Range<f64>, cancel: &AtomicBool) -> Result<Samples, String> {
        let native = translate(span, -self.offset);
        match &self.source {
            Source::Analog { reader, index } => self.read_analog(reader, *index, native, cancel),
            Source::Filtered { samples, .. } => {
                Ok(samples
                    .as_ref()
                    .map_or(Samples::Empty, |samples| Samples::Filtered {
                        raw: samples.raw.clone(),
                        offset: self.offset,
                    }))
            }
            Source::Digital { reader, index, .. } => reader.checked(cancel, |capture| {
                let Some(bounds) = capture.time_bounds(*index) else {
                    return Ok(Samples::Empty);
                };
                let native = native.start.max(bounds.start)..native.end.min(bounds.end);
                if native.is_empty() {
                    return Ok(Samples::Empty);
                }
                let trace = capture.read_edges(*index, native.clone(), MAX_EDGES, cancel)?;
                Ok(Samples::Digital {
                    trace,
                    native,
                    offset: self.offset,
                })
            }),
            Source::Pcm { lane, .. } => Ok(Samples::Pcm {
                lane: lane.clone(),
                offset: self.offset,
            }),
        }
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
            offset: self.offset,
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
                offset,
            } => {
                let position = (time - offset - channel.time_at(*first)) / channel.interval_seconds;
                interpolate(values, position).map(Value::Numeric)
            }
            Self::Filtered { raw, offset } => {
                let position = (time - offset - raw.start) / raw.step;
                interpolate(&raw.samples, position).map(Value::Numeric)
            }
            Self::Digital {
                trace,
                native,
                offset,
            } => {
                let time = time - offset;
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
            Self::Pcm { lane, offset } => lane.value_at(time - offset).map(Value::Numeric),
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
                1.0,
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
