use crate::engineering::packets::{InterfaceId, PacketHeader};
use std::{
    collections::BTreeMap,
    ops::{Bound, Range},
};

const MAX_PENDING: usize = 4096;

/// Time between a recorded segment and the first recorded ACK reaching its end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckDelay {
    pub segment_row: u64,
    pub nanoseconds: i128,
}

#[derive(Clone, Copy)]
pub(super) struct Stamp {
    pub row: u64,
    pub time_ns: i128,
    interface: InterfaceId,
}

impl Stamp {
    pub(super) fn from_header(header: PacketHeader) -> Option<Self> {
        Some(Self {
            row: header.row,
            time_ns: header.time?.unix_nanoseconds(),
            interface: header.interface,
        })
    }
}

#[derive(Default)]
pub(super) struct Outstanding {
    acknowledged: Option<i64>,
    // End sequence is the cumulative ACK that completes this candidate.
    segments: BTreeMap<i64, Segment>,
}

struct Segment {
    start: i64,
    stamp: Stamp,
}

impl Outstanding {
    pub(super) fn send(&mut self, range: Range<i64>, stamp: Option<Stamp>, eligible: bool) {
        if range.is_empty() {
            return;
        }
        let mut overlap = false;
        while let Some((&end, candidate)) = self
            .segments
            .range((Bound::Excluded(range.start), Bound::Unbounded))
            .next()
            && candidate.start < range.end
        {
            self.segments.remove(&end);
            overlap = true;
        }
        if eligible
            && !overlap
            && self.acknowledged.is_none_or(|end| end <= range.start)
            && let Some(stamp) = stamp
        {
            if self.segments.len() == MAX_PENDING {
                self.segments.pop_first();
            }
            self.segments.insert(
                range.end,
                Segment {
                    start: range.start,
                    stamp,
                },
            );
        }
    }

    pub(super) fn receive(&mut self, end: i64, stamp: Option<Stamp>) -> Option<AckDelay> {
        if self.acknowledged.is_some_and(|previous| end <= previous) {
            return None;
        }
        self.acknowledged = Some(end);
        let matched = self.segments.remove(&end);
        while self
            .segments
            .first_key_value()
            .is_some_and(|(&key, _)| key < end)
        {
            self.segments.pop_first();
        }
        let (segment, ack) = matched.zip(stamp)?;
        let ns = ack.time_ns.checked_sub(segment.stamp.time_ns)?;
        (segment.stamp.interface == ack.interface && ns >= 0).then_some(AckDelay {
            segment_row: segment.stamp.row,
            nanoseconds: ns,
        })
    }

    pub(super) fn invalidate(&mut self) {
        self.segments.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_only_the_new_exact_end_of_a_cumulative_ack() {
        let mut stream = Outstanding::default();
        stream.send(0..10, Some(stamp(1, 100, 0)), true);
        stream.send(10..20, Some(stamp(2, 120, 0)), true);
        let delay = stream.receive(20, Some(stamp(3, 139, 0))).unwrap();
        assert_eq!(
            delay,
            AckDelay {
                segment_row: 2,
                nanoseconds: 19
            }
        );
        assert!(stream.receive(20, Some(stamp(4, 150, 0))).is_none());
        assert!(stream.receive(10, Some(stamp(5, 155, 0))).is_none());
    }

    #[test]
    fn repeat_and_partial_overlap_retire_all_ambiguous_candidates() {
        let mut stream = Outstanding::default();
        stream.send(1..10, Some(stamp(1, 100, 0)), true);
        stream.send(10..20, Some(stamp(2, 111, 0)), true);
        stream.send(8..12, Some(stamp(3, 125, 0)), false);
        assert!(stream.receive(20, Some(stamp(4, 135, 0))).is_none());
        stream.send(20..31, Some(stamp(5, 155, 0)), true);
        stream.send(20..31, Some(stamp(6, 165, 0)), false);
        assert!(stream.receive(31, Some(stamp(7, 180, 0))).is_none());
    }

    #[test]
    fn timestamps_cannot_cross_interface_clocks_or_go_backward() {
        let mut stream = Outstanding::default();
        stream.send(1..10, Some(stamp(1, 100, 0)), true);
        assert!(stream.receive(10, Some(stamp(2, 130, 1))).is_none());
        stream.send(10..20, Some(stamp(3, 320, 1)), true);
        assert!(stream.receive(20, Some(stamp(4, 310, 1))).is_none());
        stream.send(20..30, Some(stamp(5, 330, 1)), true);
        assert_eq!(
            stream.receive(30, Some(stamp(6, 330, 1))),
            Some(AckDelay {
                segment_row: 5,
                nanoseconds: 0
            })
        );
    }

    fn stamp(row: u64, time_ns: i128, interface: u32) -> Stamp {
        Stamp {
            row,
            time_ns,
            interface: InterfaceId {
                section: 0,
                ordinal: interface,
            },
        }
    }
}
