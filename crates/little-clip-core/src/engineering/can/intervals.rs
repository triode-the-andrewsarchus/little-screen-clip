//! Timing between adjacent observations of one CAN identifier on one bus.

use super::{Capture, Channel, Frame, FrameKind, Identifier, Index, index};
use std::{cmp::Reverse, collections::BinaryHeap, ops::Range, sync::atomic::AtomicBool};

const BINS: i128 = 384;
const LONGEST: usize = 128;

/// A bus and a typed arbitration identifier. Standard and extended IDs are distinct.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stream {
    pub channel: Channel,
    pub id: Identifier,
}

impl Stream {
    /// Identifies the traffic stream of an arbitration frame.
    pub fn of(frame: &Frame) -> Option<Self> {
        if matches!(frame.kind, FrameKind::Error | FrameKind::Status) {
            return None;
        }
        Some(Self {
            channel: frame.channel.clone(),
            id: frame.id?,
        })
    }

    fn matches(&self, frame: &Frame) -> bool {
        frame.channel == self.channel
            && frame.id == Some(self.id)
            && !matches!(frame.kind, FrameKind::Error | FrameKind::Status)
    }
}

/// A timestamp and its exact zero-based row in the capture.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Endpoint {
    pub row: u64,
    pub time_ns: i128,
}

/// A nonnegative interval between adjacent matching frames in file order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Gap {
    pub earlier: Endpoint,
    pub later: Endpoint,
}

impl Gap {
    pub fn duration_ns(self) -> u128 {
        self.later.time_ns.abs_diff(self.earlier.time_ns)
    }
}

/// Observed range in one bucket on the later frame's time axis.
/// Each extreme retains its exact endpoints.
#[derive(Clone, Copy, Debug)]
pub struct Band {
    pub count: u64,
    pub shortest: Gap,
    pub longest: Gap,
}

impl Band {
    fn one(gap: Gap) -> Self {
        Self {
            count: 1,
            shortest: gap,
            longest: gap,
        }
    }

    fn include(&mut self, gap: Gap) {
        self.count += 1;
        if gap.duration_ns() < self.shortest.duration_ns() {
            self.shortest = gap;
        }
        if gap.duration_ns() > self.longest.duration_ns() {
            self.longest = gap;
        }
    }
}

/// Bounded observed timing statistics and exact anchors for browsing.
pub struct Report {
    pub frames_read: u64,
    pub observations: u64,
    pub intervals: u64,
    pub duplicates: u64,
    pub reversals: u64,
    /// Discontinuities introduced by skipping out-of-range checkpoints.
    pub boundaries: u64,
    pub truncated: bool,
    pub bands: Vec<Band>,
    /// Longest intervals in descending duration, with ties in file order.
    pub longest: Vec<Gap>,
}

impl Capture {
    /// Measures adjacent frames of `stream` inside the half-open time range.
    ///
    /// The scan follows file order. Matching frames outside the range and
    /// unchecked index blocks break adjacency. Timestamp reversals are counted
    /// separately, and duplicate timestamps contribute an interval of zero.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid index, invalid time range, I/O failure,
    /// source change, or cancellation.
    pub fn intervals(
        &self,
        index: &Index,
        stream: &Stream,
        time_ns: Range<i128>,
        cancel: &AtomicBool,
    ) -> Result<Report, String> {
        let mut analysis = Analysis::new(time_ns.clone());
        let scan = index::scan_candidates(self, index, &time_ns, false, cancel, |frame| {
            analysis.observe(stream, &frame);
            true
        })?;
        self.validate_identity()?;
        Ok(analysis.report(scan))
    }
}

type RankedGap = (u128, Reverse<u64>, Gap);

struct Analysis {
    time_ns: Range<i128>,
    bin_width_ns: i128,
    last_row: Option<u64>,
    previous: Option<Endpoint>,
    observations: u64,
    intervals: u64,
    duplicates: u64,
    reversals: u64,
    boundaries: u64,
    bands: Vec<Option<Band>>,
    longest: BinaryHeap<Reverse<RankedGap>>,
}

impl Analysis {
    fn new(time_ns: Range<i128>) -> Self {
        let width = time_ns.end.saturating_sub(time_ns.start).max(1);
        Self {
            time_ns,
            bin_width_ns: (width - 1) / BINS + 1,
            last_row: None,
            previous: None,
            observations: 0,
            intervals: 0,
            duplicates: 0,
            reversals: 0,
            boundaries: 0,
            bands: vec![None; BINS as usize],
            longest: BinaryHeap::new(),
        }
    }

    fn observe(&mut self, stream: &Stream, frame: &Frame) {
        if self
            .last_row
            .replace(frame.row)
            .is_some_and(|row| row.checked_add(1) != Some(frame.row))
        {
            self.boundaries += 1;
            self.previous = None;
        }
        if !stream.matches(frame) {
            return;
        }
        if !self.time_ns.contains(&frame.timestamp_ns) {
            self.previous = None;
            return;
        }
        let later = Endpoint {
            row: frame.row,
            time_ns: frame.timestamp_ns,
        };
        self.observations += 1;
        if let Some(earlier) = self.previous.replace(later) {
            if later.time_ns < earlier.time_ns {
                self.reversals += 1;
            } else {
                self.record(Gap { earlier, later });
            }
        }
    }

    fn record(&mut self, gap: Gap) {
        let duration = gap.duration_ns();
        self.intervals += 1;
        self.duplicates += u64::from(duration == 0);
        let column = gap.later.time_ns.saturating_sub(self.time_ns.start) / self.bin_width_ns;
        let band = &mut self.bands[column.clamp(0, BINS - 1) as usize];
        match band {
            Some(band) => band.include(gap),
            empty => *empty = Some(Band::one(gap)),
        }

        let key = (duration, Reverse(gap.later.row), gap);
        if self.longest.len() < LONGEST {
            self.longest.push(Reverse(key));
        } else if let Some(mut smallest) = self.longest.peek_mut()
            && key > smallest.0
        {
            *smallest = Reverse(key);
        }
    }

    fn report(self, scan: index::TimeScan) -> Report {
        Report {
            frames_read: scan.scanned,
            observations: self.observations,
            intervals: self.intervals,
            duplicates: self.duplicates,
            reversals: self.reversals,
            boundaries: self.boundaries,
            truncated: scan.truncated,
            bands: self.bands.into_iter().flatten().collect(),
            longest: self
                .longest
                .into_sorted_vec()
                .into_iter()
                .map(|Reverse((_, _, gap))| gap)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const HEADER: &str = "date Mon Nov 23 03:22:02 2026\nbase hex timestamps absolute\n";

    fn indexed_capture(source: &str) -> (Capture, Index) {
        let mut file = tempfile::tempfile().unwrap();
        write!(file, "{HEADER}{source}").unwrap();
        let capture = Capture::open(file).unwrap();
        let index = capture.index(&AtomicBool::new(false)).unwrap();
        (capture, index)
    }

    fn stream(bus: u16, extended: bool) -> Stream {
        Stream {
            channel: Channel::Number(bus),
            id: Identifier {
                value: 0x241,
                extended,
            },
        }
    }

    #[test]
    fn typed_streams_keep_exact_rows_duplicates_and_reversals() {
        let (capture, index) = indexed_capture(concat!(
            "-0.005 1 241 Rx d 1 11\n",
            "0.000 1 00000241x Rx d 1 22\n",
            "0.001 2 241 Rx d 1 33\n",
            "0.000 1 241 Rx d 1 44\n",
            "0.000 1 241 Rx d 1 55\n",
            "-0.001 1 241 Rx d 1 66\n",
            "0.003 1 00000241x Rx d 1 77\n",
            "0.004 2 241 Rx d 1 88\n",
            "0.007 1 241 Rx d 1 99\n",
        ));
        let cancel = AtomicBool::new(false);
        let inspect = |stream| {
            capture
                .intervals(&index, &stream, -5_000_000..8_000_000, &cancel)
                .unwrap()
        };
        let standard = inspect(stream(1, false));
        assert_eq!(
            (
                standard.observations,
                standard.intervals,
                standard.duplicates,
                standard.reversals
            ),
            (5, 3, 1, 1)
        );
        assert_eq!(
            standard
                .longest
                .iter()
                .map(|gap| gap.duration_ns())
                .collect::<Vec<_>>(),
            [8_000_000, 5_000_000, 0]
        );
        assert_eq!(standard.longest[0].earlier.row, 5);
        assert_eq!(standard.longest[0].later.row, 8);
        assert_eq!(capture.rows(&index, 8..9, &cancel).unwrap()[0].data, [0x99]);
        let extended = inspect(stream(1, true));
        assert_eq!(extended.longest[0].duration_ns(), 3_000_000);
        assert_eq!(extended.longest[0].earlier.row, 1);
        let other_bus = inspect(stream(2, false));
        assert_eq!(other_bus.longest[0].duration_ns(), 3_000_000);
        assert_eq!(other_bus.longest[0].earlier.row, 2);
    }

    #[test]
    fn time_filter_does_not_bridge_an_excluded_observation_or_checkpoint() {
        let mut rows =
            String::from("0.000 1 241 Rx d 1 00\n0.900 1 241 Rx d 1 00\n0.001 1 241 Rx d 1 00\n");
        rows.push_str(&"0.002 1 555 Rx d 1 00\n".repeat(253));
        rows.push_str(&"0.900 1 241 Rx d 1 00\n".repeat(256));
        rows.push_str("0.003 1 241 Rx d 1 00\n0.004 1 241 Rx d 1 00\n");
        let (capture, index) = indexed_capture(&rows);
        let cancel = AtomicBool::new(false);
        let report = capture
            .intervals(&index, &stream(1, false), 0..5_000_000, &cancel)
            .unwrap();
        assert_eq!(
            (report.observations, report.intervals, report.boundaries),
            (4, 1, 1)
        );
        assert_eq!(report.frames_read, 258);
        assert_eq!(report.longest[0].earlier.row, 512);
        assert_eq!(report.longest[0].later.row, 513);
    }

    #[test]
    fn endpoints_beyond_the_first_browser_page_are_exact() {
        let mut rows = "0.000 1 555 Rx d 1 FF\n".repeat(8192);
        rows.push_str("0.000 1 241 Rx d 1 12\n0.005 1 241 Rx d 1 34\n0.010 1 241 Rx d 1 56\n");
        let (capture, index) = indexed_capture(&rows);
        let cancel = AtomicBool::new(false);
        let report = capture
            .intervals(&index, &stream(1, false), 0..10_000_001, &cancel)
            .unwrap();
        assert_eq!(report.intervals, 2);
        assert_eq!(report.longest[0].duration_ns(), 5_000_000);
        assert_eq!(report.longest[1].later.row, 8194);
        let earlier = report.longest[1].earlier.row;
        let later = report.longest[1].later.row;
        let frames = capture.rows(&index, earlier..later + 1, &cancel).unwrap();
        assert_eq!(frames[0].data, [0x34]);
        assert_eq!(frames[1].data, [0x56]);
    }
}
