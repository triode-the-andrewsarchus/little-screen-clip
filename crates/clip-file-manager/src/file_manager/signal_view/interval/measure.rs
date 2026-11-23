use super::overlap;
use crate::file_manager::{
    logic_view::protocols::DECODE_EDGES,
    scope_view::reads::{ANALYSIS_SAMPLES, centered_rows, sample_extrema},
};
use little_clip_core::engineering::{
    logic_decode::{self, Kind},
    logic_preview,
    waveforms::{self, PowerTotals, sample_span},
};
use std::{
    ops::Range,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone)]
pub(super) enum Input {
    Samples(Arc<waveforms::Recording>, usize),
    Logic {
        recording: Arc<logic_preview::Recording>,
        lane: usize,
        polarity: Option<bool>,
    },
}

impl PartialEq for Input {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Samples(a, channel), Self::Samples(b, other_channel)) => {
                Arc::ptr_eq(a, b) && channel == other_channel
            }
            (
                Self::Logic {
                    recording: a,
                    lane: i,
                    polarity: p,
                },
                Self::Logic {
                    recording: b,
                    lane: j,
                    polarity: q,
                },
            ) => Arc::ptr_eq(a, b) && i == j && p == q,
            _ => false,
        }
    }
}

pub(super) struct Measurement {
    pub waveform: Option<Report>,
    pub power: Option<PowerTotals>,
}

pub(super) struct Report {
    pub coverage: Range<f64>,
    pub examined: Range<f64>,
    pub figures: Figures,
}

pub(super) enum Figures {
    Samples(SampleReport),
    Edges(usize),
    Pwm {
        cycles: usize,
        extrema: Option<(Duty, Duty)>,
    },
}

pub(super) struct SampleReport {
    pub unit: String,
    pub valid: usize,
    pub extrema: Option<(Extreme, Extreme)>,
    pub rows: Range<u64>,
}

pub(super) struct Extreme {
    pub value: f32,
    pub sample: u64,
    pub view: Range<f64>,
}

#[derive(Clone)]
pub(super) struct Duty {
    pub percent: f64,
    pub cycle: Range<f64>,
}

impl Input {
    pub fn read(self, requested: Range<f64>, cancel: &AtomicBool) -> Result<Measurement, String> {
        match self {
            Self::Samples(recording, index) => recording.checked(cancel, |capture| {
                let power = capture.power_totals(requested.clone(), cancel)?;
                let waveform = samples(capture, index, requested, cancel)?;
                Ok(Measurement { waveform, power })
            }),
            Self::Logic {
                recording,
                lane,
                polarity,
            } => recording.checked(cancel, |capture| {
                Ok(Measurement {
                    waveform: logic(capture, lane, polarity, requested, cancel)?,
                    power: None,
                })
            }),
        }
    }
}

fn samples(
    capture: &waveforms::Capture,
    index: usize,
    requested: Range<f64>,
    cancel: &AtomicBool,
) -> Result<Option<Report>, String> {
    let channel = &capture.channels()[index];
    let domain = channel.start_seconds..channel.time_at(channel.sample_count);
    let Some(coverage) = overlap(&domain, &requested) else {
        return Ok(None);
    };
    let range = sample_span(channel, coverage.clone());
    let rows = centered_rows(range.clone(), ANALYSIS_SAMPLES);
    let values = capture.read_samples(index, rows.clone(), cancel)?;
    let stats = sample_extrema(&values);
    let examined = if rows != range {
        channel.time_at(rows.start)..channel.time_at(rows.end)
    } else {
        coverage.clone()
    };
    let extrema = stats.peak.valid().then(|| {
        let [low, high] = stats.extrema.map(|index| {
            let row = rows.start + index as u64;
            let view = channel.time_at(row.saturating_sub(8))
                ..channel.time_at(row.saturating_add(9).min(channel.sample_count));
            Extreme {
                value: values[index],
                sample: row + 1,
                view,
            }
        });
        (low, high)
    });
    Ok(Some(Report {
        coverage,
        examined,
        figures: Figures::Samples(SampleReport {
            unit: channel.unit.clone(),
            valid: stats.valid,
            extrema,
            rows,
        }),
    }))
}

fn logic(
    capture: &little_clip_core::engineering::logic::Capture,
    lane: usize,
    polarity: Option<bool>,
    requested: Range<f64>,
    cancel: &AtomicBool,
) -> Result<Option<Report>, String> {
    let domain = capture
        .time_bounds(lane)
        .ok_or("This logic channel is unavailable.")?;
    let Some(coverage) = overlap(&domain, &requested) else {
        return Ok(None);
    };
    let trace = capture.read_edges(lane, coverage.clone(), DECODE_EDGES, cancel)?;
    let examined = coverage.start..trace.inspected_until.min(coverage.end);
    let figures = match polarity {
        Some(active_high) => {
            let events = logic_decode::pwm(&trace, active_high)?;
            let mut duties = events.into_iter().filter_map(|event| {
                let Kind::PwmCycle(cycle) = event.kind else {
                    return None;
                };
                Some(Duty {
                    percent: 100.0 * cycle.duty(),
                    cycle: event.time,
                })
            });
            let first = duties.next();
            let mut cycles = usize::from(first.is_some());
            let extrema = first.map(|first| {
                duties.fold((first.clone(), first), |(low, high), duty| {
                    cycles += 1;
                    let low = if duty.percent < low.percent {
                        duty.clone()
                    } else {
                        low
                    };
                    let high = if duty.percent > high.percent {
                        duty
                    } else {
                        high
                    };
                    (low, high)
                })
            });
            Figures::Pwm { cycles, extrema }
        }
        None => Figures::Edges(trace.edges.len()),
    };
    Ok(Some(Report {
        coverage,
        examined,
        figures,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwm_extrema_reveal_complete_cycles_inside_trimmed_capture() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trimmed.vcd");
        std::fs::write(
            &path,
            concat!(
                "$timescale 1us $end\n$scope module bench $end\n",
                "$var wire 1 ! PWM $end\n$upscope $end\n$enddefinitions $end\n",
                "#0\n0!\n#1000\n1!\n#1250\n0!\n#2000\n1!\n#2750\n0!\n",
                "#3000\n1!\n#3100\nx!\n#3500\n0!\n#4000\n1!\n#4500\n0!\n#5000\n",
            ),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let recording = Arc::new(logic_preview::Recording::open(&path, &cancel).unwrap());
        let input = Input::Logic {
            recording,
            lane: 0,
            polarity: Some(true),
        };
        let report = input
            .clone()
            .read(0.0005..0.008, &cancel)
            .unwrap()
            .waveform
            .unwrap();
        assert_eq!(report.coverage.start, 0.0005);
        assert!(report.coverage.end < 0.008);
        let Figures::Pwm {
            cycles,
            extrema: Some((low, high)),
        } = report.figures
        else {
            panic!("complete PWM cycles");
        };
        assert_eq!(cycles, 2);
        assert!((low.percent - 25.0).abs() < 1e-10);
        assert!((high.percent - 75.0).abs() < 1e-10);
        assert!((low.cycle.start - 0.001).abs() < 1e-12);
        assert!((high.cycle.start - 0.002).abs() < 1e-12);
        assert!(input.read(0.009..0.01, &cancel).unwrap().waveform.is_none());
    }
}
