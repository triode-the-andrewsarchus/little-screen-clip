use super::{Event, Kind, Trace, driven, emit, validate_trace};

/// One complete pulse-width modulation cycle from an observed active edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PwmCycle {
    pub active_seconds: f64,
    pub period_seconds: f64,
}

impl PwmCycle {
    pub fn duty(self) -> f64 {
        self.active_seconds / self.period_seconds
    }

    pub fn frequency(self) -> f64 {
        self.period_seconds.recip()
    }
}

/// Measures complete cycles from successive active edges.
///
/// A leading or trailing partial cycle carries no period estimate. Unknown or
/// high-impedance observations retire the open cycle until the next known edge.
///
/// # Errors
///
/// Returns an error for malformed input or an oversized result.
pub fn pwm(trace: &Trace, active_high: bool) -> Result<Vec<Event>, String> {
    validate_trace(trace, None)?;
    let mut previous = trace.initial.and_then(driven);
    let mut open = None;
    let mut inactive_at = None;
    let mut events = Vec::new();
    for edge in &trace.edges {
        let level = driven(edge.level);
        let at = edge.time_seconds;
        match (previous, level) {
            (_, None) => {
                open = None;
                inactive_at = None;
            }
            (Some(before), Some(after)) if before != after => {
                if after == active_high {
                    if let Some((start, inactive)) = open.replace(at).zip(inactive_at.take()) {
                        complete(&mut events, start, inactive, at)?;
                    }
                } else if open.is_some() {
                    inactive_at = Some(at);
                }
            }
            _ => {}
        }
        previous = level;
    }
    Ok(events)
}

fn complete(events: &mut Vec<Event>, start: f64, inactive: f64, end: f64) -> Result<(), String> {
    let period_seconds = end - start;
    let active_seconds = inactive - start;
    if period_seconds.recip().is_finite()
        && period_seconds.is_finite()
        && active_seconds > 0.0
        && active_seconds < period_seconds
    {
        emit(
            events,
            start..end,
            Kind::PwmCycle(PwmCycle {
                active_seconds,
                period_seconds,
            }),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::logic::{Edge, Level};
    use Level::{High, Low, Unknown};

    #[test]
    fn incomplete_and_unknown_cycles_do_not_fabricate_periods() {
        let trace = trace(&[
            (0.25, Low),
            (1.0, High),
            (1.25, Low),
            (2.0, High),
            (2.25, Unknown),
            (2.75, Low),
            (3.0, High),
            (3.75, Low),
            (4.0, High),
            (4.5, Low),
        ]);
        let events = pwm(&trace, true).unwrap();
        assert_eq!(events, [cycle(1.0, 2.0, 0.25), cycle(3.0, 4.0, 0.75)]);
    }

    #[test]
    fn active_low_reports_the_low_duration_even_if_periods_change() {
        let trace = trace(&[
            (1.0, Low),
            (1.125, High),
            (2.0, Low),
            (2.5, High),
            (4.0, Low),
        ]);
        let events = pwm(&trace, false).unwrap();
        assert_eq!(events, [cycle(1.0, 2.0, 0.125), cycle(2.0, 4.0, 0.5)]);
    }

    fn trace(edges: &[(f64, Level)]) -> Trace {
        Trace {
            initial: Some(High),
            edges: edges
                .iter()
                .map(|&(time_seconds, level)| Edge {
                    time_seconds,
                    level,
                })
                .collect(),
            inspected_until: 8.0,
        }
    }

    fn cycle(start: f64, end: f64, active_seconds: f64) -> Event {
        Event {
            time: start..end,
            kind: Kind::PwmCycle(PwmCycle {
                active_seconds,
                period_seconds: end - start,
            }),
        }
    }
}
