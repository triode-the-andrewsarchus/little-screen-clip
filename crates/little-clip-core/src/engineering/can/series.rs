use super::{Capture, Channel, Frame, Index, dbc, index};
use std::{collections::BTreeMap, ops::Range, sync::atomic::AtomicBool};

const EXACT_POINTS: usize = 8192;
const BINS: i128 = 1024;

/// Identifies one physical value on one bus and the time interval to inspect.
pub struct Query<'a> {
    pub time_ns: Range<i128>,
    pub channel: &'a Channel,
    pub message: &'a dbc::Message,
    pub field: usize,
}

/// A decoded value or gap with the exact source timestamp and ordinal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub row: u64,
    pub time_ns: i128,
    pub decoded: Option<dbc::DecodedValue>,
}

/// Exact points for a small range or retained extrema for a dense range.
pub struct Series {
    pub points: Vec<Point>,
    pub observations: u64,
    pub frames_read: u64,
    pub reduced: bool,
    pub truncated: bool,
}

impl Capture {
    /// Reads a bounded time interval for one recorded DBC signal.
    ///
    /// # Errors
    /// Returns an error for an unrelated index, invalid signal, I/O failure, or cancellation.
    pub fn signal_series(
        &self,
        index: &Index,
        query: Query<'_>,
        cancel: &AtomicBool,
    ) -> Result<Series, String> {
        if query.field >= query.message.signals.len() || query.time_ns.start > query.time_ns.end {
            return Err("Invalid CAN signal or time interval".into());
        }
        let mut plot = Plot::new(&query.time_ns);
        let scan = index::scan_time(self, index, &query.time_ns, cancel, |frame| {
            if frame.channel == *query.channel && frame.id == Some(query.message.id) {
                plot.observe(&frame, query.message.decode(query.field, &frame));
            }
            true
        })?;
        self.validate_identity()?;
        Ok(plot.finish(scan.scanned, scan.truncated))
    }
}

struct Plot {
    start_ns: i128,
    bin_width_ns: i128,
    bins: BTreeMap<i128, Bucket>,
    exact: Vec<Point>,
    observations: u64,
    matching: u64,
}

#[derive(Clone, Copy)]
struct Bucket {
    first: Point,
    last: Point,
    lower: Point,
    upper: Point,
}

impl Plot {
    fn new(span: &Range<i128>) -> Self {
        let width = span.end.saturating_sub(span.start);
        let bin_width_ns = (width / BINS + i128::from(width % BINS != 0)).max(1);
        Self {
            start_ns: span.start,
            bin_width_ns,
            bins: BTreeMap::new(),
            exact: Vec::new(),
            observations: 0,
            matching: 0,
        }
    }

    fn observe(&mut self, frame: &Frame, decoded: Option<dbc::DecodedValue>) {
        let point = Point {
            row: frame.row,
            time_ns: frame.timestamp_ns,
            decoded,
        };
        self.matching += 1;
        if self.exact.len() < EXACT_POINTS {
            self.exact.push(point);
        }
        if let Some(reading) = decoded {
            self.observations += 1;
            let position = frame.timestamp_ns.saturating_sub(self.start_ns) / self.bin_width_ns;
            let bin = self
                .bins
                .entry(position.clamp(0, BINS - 1))
                .or_insert(Bucket {
                    first: point,
                    last: point,
                    lower: point,
                    upper: point,
                });
            bin.include(point, reading.value);
        }
    }

    fn finish(self, frames_read: u64, truncated: bool) -> Series {
        let reduced = self.matching > EXACT_POINTS as u64;
        let mut points = if reduced {
            self.bins
                .into_values()
                .flat_map(|bin| [bin.first, bin.lower, bin.upper, bin.last])
                .collect()
        } else {
            self.exact
        };
        points.sort_unstable_by_key(|point| (point.time_ns, point.row));
        points.dedup_by_key(|point| point.row);
        Series {
            points,
            observations: self.observations,
            frames_read,
            reduced,
            truncated,
        }
    }
}

impl Bucket {
    fn include(&mut self, point: Point, value: f64) {
        let ordered = |point: Point| (point.time_ns, point.row);
        if ordered(point) < ordered(self.first) {
            self.first = point;
        }
        if ordered(point) > ordered(self.last) {
            self.last = point;
        }
        if self.lower.decoded.is_some_and(|lower| value < lower.value) {
            self.lower = point;
        }
        if self.upper.decoded.is_some_and(|upper| value > upper.value) {
            self.upper = point;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::can::{FrameKind, Identifier};
    use std::io::Write;

    #[test]
    fn captures_exact_times_and_mux_gaps_on_the_requested_bus() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(
            b"date Mon Nov 2 03:22:02 2026
base hex timestamps absolute
0.004 1 123 Rx d 2 01 0A
0.001 1 123 Rx d 2 01 06
0.002 2 123 Rx d 2 01 0B
0.003 1 123 Rx d 2 00 05
",
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let capture = Capture::open(file).unwrap();
        let index = capture.index(&cancel).unwrap();
        let definition = br#"VERSION ""

NS_ :

BS_:

BU_: Node

BO_ 291 Console: 2 Node
 SG_ Mode M : 0|4@1+ (1,0) [0|15] "" Node
 SG_ Power m1 : 8|8@1+ (0.5,-1) [0|127] "W" Node
"#;
        let schema = dbc::Database::parse(definition, &cancel).unwrap();
        let message = schema
            .message(Identifier::new(0x123, false).unwrap())
            .unwrap();
        let trace = capture
            .signal_series(
                &index,
                Query {
                    time_ns: 0..4_000_000,
                    channel: &Channel::Number(1),
                    message,
                    field: 1,
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(trace.observations, 1);
        assert!(!trace.reduced);
        assert!(!trace.truncated);
        assert_eq!(trace.points.len(), 2);
        assert_eq!(trace.points[0].row, 1);
        assert_eq!(trace.points[0].time_ns, 1_000_000);
        assert_eq!(trace.points[0].decoded.unwrap().value, 2.0);
        assert_eq!(trace.points[1].row, 3);
        assert!(trace.points[1].decoded.is_none());
    }

    #[test]
    fn dense_series_retains_the_original_extreme_frames() {
        let mut plot = Plot::new(&(0..10_000));
        let count = 10_000;
        for row in 0..count {
            let mut frame = Frame::new(
                i128::from(count - row - 1),
                Channel::Unknown,
                FrameKind::Classic,
            );
            frame.row = row;
            let value = match row {
                4371 => 90,
                8743 => -33,
                _ => 0,
            };
            plot.observe(
                &frame,
                Some(dbc::DecodedValue {
                    raw: dbc::RawValue::Signed(value),
                    value: value as f64,
                }),
            );
        }
        let trace = plot.finish(count, false);
        assert!(trace.reduced);
        assert_eq!(trace.observations, count);
        assert!(trace.points.len() <= 4096);
        assert!(
            trace
                .points
                .windows(2)
                .all(|pair| pair[0].time_ns < pair[1].time_ns)
        );
        assert!(
            trace
                .points
                .iter()
                .any(|point| point.row == 4371 && point.decoded.unwrap().value == 90.0)
        );
        assert!(
            trace
                .points
                .iter()
                .any(|point| point.row == 8743 && point.decoded.unwrap().value == -33.0)
        );
    }
}
