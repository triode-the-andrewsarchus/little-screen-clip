use super::{Event, I2sChannel, Kind, median_interval};
use std::ops::Range;

/// One recorded PCM lane on its actual bit-clock timestamps.
#[derive(Debug)]
pub struct PcmLane {
    points: Vec<Point>,
    interval: f64,
}

#[derive(Clone, Copy, Debug)]
struct Point {
    time: f64,
    value: Option<f64>,
}

impl PcmLane {
    /// Extracts complete word intervals for one channel within the requested span.
    ///
    /// Width defines full scale, with the most negative word represented by -1.
    /// Missing or short words retain their timestamp with an absent value.
    /// A nominal step needs at least three word timestamps.
    pub fn from_events(events: &[Event], channel: I2sChannel, span: &Range<f64>) -> Option<Self> {
        let points: Vec<_> = events
            .iter()
            .filter(|event| span.contains(&event.time.start) && event.time.end <= span.end)
            .filter_map(|event| point(event, channel))
            .collect();
        if points.len() < 3 || points.iter().all(|point| point.value.is_none()) {
            return None;
        }
        let mut steps: Vec<_> = points
            .windows(2)
            .map(|pair| pair[1].time - pair[0].time)
            .collect();
        let interval = median_interval(&mut steps)?;
        Some(Self { points, interval })
    }

    pub fn interval(&self) -> f64 {
        self.interval
    }

    /// Interpolates neighboring measured words and preserves decode gaps.
    ///
    /// Intervals above 1.5 nominal periods are gaps. Exact word times need
    /// only the selected sample, including at the ends of the lane.
    pub fn value_at(&self, at: f64) -> Option<f64> {
        if !at.is_finite() {
            return None;
        }
        let next = self.points.partition_point(|point| point.time < at);
        let left = next.checked_sub(1).map(|index| self.points[index]);
        let right = self.points.get(next).copied();
        let epsilon = (self.interval * 1e-8).max(at.abs() * f64::EPSILON * 4.0);
        for point in [left, right].into_iter().flatten() {
            if (point.time - at).abs() <= epsilon {
                return point.value;
            }
        }
        let (left, right) = left.zip(right)?;
        let step = right.time - left.time;
        if step > self.interval * 1.5 {
            return None;
        }
        let (a, b) = left.value.zip(right.value)?;
        Some((b - a).mul_add((at - left.time) / step, a))
    }
}

fn point(event: &Event, lane: I2sChannel) -> Option<Point> {
    let value = match event.kind {
        Kind::I2sSample {
            channel,
            sample,
            bits,
        } if lane == channel && (8..=32).contains(&bits) => {
            sample.map(|value| f64::from(value) / f64::from(1_u32 << (bits - 1)))
        }
        Kind::I2sShortWord { channel, .. } if lane == channel => None,
        _ => return None,
    };
    Some(Point {
        time: event.time.start,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_captured_timestamps_and_retires_silent_or_unknown_gaps() {
        let events = [
            word(0.0, Some(-128)),
            word(1.0, Some(64)),
            word(2.0, None),
            word(3.0, Some(32)),
            word(6.0, Some(0)),
            word(7.1, Some(64)),
        ];
        let lane = PcmLane::from_events(&events, I2sChannel::Left, &(0.0..8.0)).unwrap();
        assert_eq!(lane.interval(), 1.0);
        assert_eq!(lane.value_at(0.0), Some(-1.0));
        assert_eq!(lane.value_at(0.5), Some(-0.25));
        assert_eq!(lane.value_at(1.0), Some(0.5));
        assert_eq!(lane.value_at(1.5), None);
        assert_eq!(lane.value_at(2.5), None);
        assert_eq!(lane.value_at(3.5), None);
        assert_eq!(lane.value_at(5.9), None);
        assert_eq!(lane.value_at(6.55), Some(0.25));
        assert_eq!(lane.value_at(8.0), None);
        assert!(PcmLane::from_events(&events, I2sChannel::Right, &(0.0..8.0)).is_none());
    }

    #[test]
    fn discards_words_crossing_selection_boundaries() {
        let events = (0..6)
            .map(|at| word(f64::from(at), Some(16)))
            .collect::<Vec<_>>();
        let lane = PcmLane::from_events(&events, I2sChannel::Left, &(0.1..4.1)).unwrap();
        assert_eq!(lane.value_at(0.0), None);
        assert_eq!(lane.value_at(1.0), Some(0.125));
        assert_eq!(lane.value_at(3.0), Some(0.125));
        assert_eq!(lane.value_at(4.0), None);
    }

    fn word(time: f64, sample: Option<i32>) -> Event {
        Event {
            time: time..time + 0.5,
            kind: Kind::I2sSample {
                channel: I2sChannel::Left,
                sample,
                bits: 8,
            },
        }
    }
}
