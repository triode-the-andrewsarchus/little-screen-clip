use super::{Event, Kind, PwmCycle};
use std::ops::Range;

/// A quantity measured over a completed PWM cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PwmMeasurement {
    Duty,
    Frequency,
    ActiveTime,
    Period,
}

impl PwmMeasurement {
    fn value(self, cycle: PwmCycle) -> f64 {
        match self {
            Self::Duty => cycle.duty(),
            Self::Frequency => cycle.frequency(),
            Self::ActiveTime => cycle.active_seconds,
            Self::Period => cycle.period_seconds,
        }
    }
}

/// Immutable cycle observations on the recording's native time axis.
#[derive(Debug)]
pub struct PwmLane {
    cycles: Vec<Cycle>,
    interval: f64,
}

#[derive(Debug)]
struct Cycle {
    span: Range<f64>,
    measurement: PwmCycle,
}

impl PwmLane {
    /// Pins only complete decoded cycles entirely inside the requested span.
    pub fn from_events(events: &[Event], span: &Range<f64>) -> Option<Self> {
        let cycles: Vec<_> = events
            .iter()
            .filter(|event| span.contains(&event.time.start) && event.time.end <= span.end)
            .filter_map(|event| {
                let Kind::PwmCycle(measurement) = event.kind else {
                    return None;
                };
                Some(Cycle {
                    span: event.time.clone(),
                    measurement,
                })
            })
            .collect();
        if cycles
            .windows(2)
            .any(|pair| pair[0].span.end > pair[1].span.start)
        {
            return None;
        }
        let mut intervals: Vec<_> = cycles
            .iter()
            .map(|cycle| cycle.measurement.period_seconds)
            .collect();
        let interval = super::super::median_interval(&mut intervals)?;
        Some(Self { cycles, interval })
    }

    pub fn cycle_count(&self) -> usize {
        self.cycles.len()
    }

    /// Suggested uniform equation grid with four points per typical cycle.
    pub fn grid_rate(&self) -> f64 {
        4.0 / self.interval
    }

    /// Holds a measurement over its half-open cycle. Unobserved gaps stay absent.
    pub fn value_at(&self, time: f64, measurement: PwmMeasurement) -> Option<f64> {
        let index = self
            .cycles
            .partition_point(|cycle| cycle.span.start <= time)
            .checked_sub(1)?;
        let cycle = &self.cycles[index];
        cycle
            .span
            .contains(&time)
            .then(|| measurement.value(cycle.measurement))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_cycle_values_preserve_gaps_widths_and_selection_limits() {
        let events = [
            cycle(0.0..2.0, 1.0),
            cycle(2.0..3.0, 0.25),
            cycle(5.0..7.0, 1.5),
            cycle(7.0..8.0, 0.5),
        ];
        let lane = PwmLane::from_events(&events, &(1.0..7.5)).unwrap();
        assert_eq!(lane.cycle_count(), 2);
        assert_eq!(lane.grid_rate(), 4.0);
        assert_eq!(lane.value_at(2.0, PwmMeasurement::Duty), Some(0.25));
        assert_eq!(lane.value_at(2.75, PwmMeasurement::Period), Some(1.0));
        assert_eq!(lane.value_at(5.0, PwmMeasurement::Frequency), Some(0.5));
        assert_eq!(lane.value_at(6.75, PwmMeasurement::ActiveTime), Some(1.5));
        for time in [1.5, 3.0, 4.0, 7.0, 7.5] {
            assert_eq!(lane.value_at(time, PwmMeasurement::Duty), None);
        }
        assert!(PwmLane::from_events(&events, &(3.0..5.0)).is_none());
    }

    fn cycle(time: Range<f64>, active_seconds: f64) -> Event {
        let period_seconds = time.end - time.start;
        Event {
            time,
            kind: Kind::PwmCycle(PwmCycle {
                active_seconds,
                period_seconds,
            }),
        }
    }
}
