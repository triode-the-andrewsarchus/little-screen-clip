use super::{ConversationKey, PacketRow, TcpSegment};

const FIN: u16 = 0x01;
const SYN: u16 = 0x02;
const RESET: u16 = 0x04;
const ACK: u16 = 0x10;

/// The start of a segment relative to the highest previously observed end.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SequencePosition {
    #[default]
    Unknown,
    AtPreviousEnd,
    AbovePreviousEnd,
    BelowPreviousEnd,
}

/// TCP coordinates for one source row. Each direction has its own sequence origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TcpPoint {
    pub row: u64,
    pub time_ns: Option<i128>,
    pub connection: u32,
    pub sender: usize,
    pub segment: TcpSegment,
    pub sequence: Option<i64>,
    pub sequence_end: Option<i64>,
    pub acknowledged: Option<i64>,
    pub position: SequencePosition,
}

/// Projects a selected endpoint pair onto TCP sequence axes in capture order.
///
/// Capture gaps and observed sequence overlap describe the available packets.
/// They do not by themselves distinguish network loss, retransmission, capture
/// loss, or reordering.
pub struct TcpTracker {
    key: ConversationKey,
    connection: u32,
    peers: [Direction; 2],
    closed: bool,
    past_syn: bool,
}

impl TcpTracker {
    pub fn new(key: ConversationKey) -> Self {
        Self {
            key,
            connection: 0,
            peers: [Direction::default(); 2],
            closed: false,
            past_syn: false,
        }
    }

    pub fn observe(&mut self, row: &PacketRow) -> Option<TcpPoint> {
        let (key, sender) = ConversationKey::identify(row.protocol)?;
        let segment = row.protocol.tcp.filter(|_| key == self.key)?;
        let opens = segment.flags & (SYN | ACK) == SYN;
        if opens && self.restarts(sender, segment.sequence) {
            self.connection = self.connection.saturating_add(1);
            self.peers = [Direction::default(); 2];
            self.closed = false;
            self.past_syn = false;
        }

        let acknowledged = (segment.flags & ACK != 0)
            .then(|| self.peers[1 - sender].relative(segment.acknowledgment))
            .flatten();
        let placement = self.peers[sender].observe(segment);
        self.past_syn |= segment.flags & SYN == 0;
        self.closed |= segment.flags & RESET != 0 || self.peers.iter().all(|peer| peer.finished);
        Some(TcpPoint {
            row: row.header.row,
            time_ns: row.header.time.map(|time| time.unix_nanoseconds()),
            connection: self.connection,
            sender,
            segment,
            sequence: placement.start,
            sequence_end: placement.end,
            acknowledged,
            position: placement.position,
        })
    }

    fn restarts(&self, sender: usize, initial: u32) -> bool {
        let old = self.peers[sender].syn;
        self.closed || (old != Some(initial) && (old.is_some() || self.past_syn))
    }
}

#[derive(Clone, Copy, Default)]
struct Direction {
    origin: Option<u32>,
    syn: Option<u32>,
    finished: bool,
    highest_end: Option<i64>,
}

#[derive(Default)]
struct Placement {
    start: Option<i64>,
    end: Option<i64>,
    position: SequencePosition,
}

impl Direction {
    fn relative(&self, raw: u32) -> Option<i64> {
        let origin = self.origin?;
        let reference = self.highest_end.unwrap_or(0);
        // The low 32 bits place the reference back in TCP's wrapping sequence space.
        let reference_raw = origin.wrapping_add(reference as u32);
        let distance = raw.wrapping_sub(reference_raw) as i32;
        if distance == i32::MIN {
            return None;
        }
        reference.checked_add(i64::from(distance))
    }

    fn observe(&mut self, segment: TcpSegment) -> Placement {
        self.origin.get_or_insert(segment.sequence);
        if segment.flags & SYN != 0 {
            self.syn.get_or_insert(segment.sequence);
        }
        self.finished |= segment.flags & FIN != 0;
        let Some(start) = self.relative(segment.sequence) else {
            return Placement::default();
        };
        let end = length(segment).and_then(|length| start.checked_add(length));
        let position = match (end, self.highest_end) {
            (Some(end), Some(previous)) if end > start => match start.cmp(&previous) {
                std::cmp::Ordering::Less => SequencePosition::BelowPreviousEnd,
                std::cmp::Ordering::Equal => SequencePosition::AtPreviousEnd,
                std::cmp::Ordering::Greater => SequencePosition::AbovePreviousEnd,
            },
            _ => SequencePosition::Unknown,
        };
        if let Some(end) = end {
            self.highest_end = Some(self.highest_end.map_or(end, |previous| end.max(previous)));
        }
        Placement {
            start: Some(start),
            end,
            position,
        }
    }
}

fn length(segment: TcpSegment) -> Option<i64> {
    let payload = i64::from(segment.payload_len?);
    Some(payload + i64::from(segment.flags & SYN != 0) + i64::from(segment.flags & FIN != 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::packets::{
        Endpoint, InterfaceId, PacketHeader, Protocol, ProtocolFields,
    };
    use std::net::Ipv4Addr;

    #[test]
    fn sequence_wrap_and_reverse_ack_use_independent_origins() {
        let syn = row(false, u32::MAX - 2, 0, SYN, 0);
        let (key, _) = ConversationKey::identify(syn.protocol).unwrap();
        let mut tracker = TcpTracker::new(key);
        let initial = tracker.observe(&syn).unwrap();
        assert_eq!((initial.sequence, initial.sequence_end), (Some(0), Some(1)));
        let reverse = tracker
            .observe(&row(true, 100, u32::MAX - 1, SYN | ACK, 0))
            .unwrap();
        assert_eq!(
            (reverse.sender, reverse.sequence_end, reverse.acknowledged),
            (1, Some(1), Some(1))
        );
        let crossed = tracker
            .observe(&row(false, u32::MAX - 1, 101, ACK, 10))
            .unwrap();
        assert_eq!(crossed.sequence_end, Some(11));
        assert_eq!(crossed.position, SequencePosition::AtPreviousEnd);
        let ack = tracker.observe(&row(true, 101, 8, ACK, 0)).unwrap();
        assert_eq!(ack.acknowledged, Some(11));
        let later = tracker.observe(&row(false, 12, 101, ACK, 3)).unwrap();
        assert_eq!(later.position, SequencePosition::AbovePreviousEnd);
        let overlap = tracker.observe(&row(false, 2, 101, ACK | FIN, 8)).unwrap();
        assert_eq!(
            (overlap.sequence, overlap.sequence_end),
            (Some(5), Some(14))
        );
        assert_eq!(overlap.position, SequencePosition::BelowPreviousEnd);
    }

    #[test]
    fn distinguishes_endpoint_reuse_after_reset_from_a_repeated_syn() {
        let initial = row(false, 20, 0, SYN, 0);
        let (key, _) = ConversationKey::identify(initial.protocol).unwrap();
        let mut tracker = TcpTracker::new(key);
        assert_eq!(tracker.observe(&initial).unwrap().connection, 0);
        let repeated = tracker.observe(&initial).unwrap();
        assert_eq!(repeated.connection, 0);
        assert_eq!(repeated.position, SequencePosition::BelowPreviousEnd);
        tracker.observe(&row(true, 1, 21, RESET | ACK, 0)).unwrap();
        let restarted = tracker.observe(&initial).unwrap();
        assert_eq!(restarted.connection, 1);
        assert_eq!(restarted.sequence, Some(0));
        assert_eq!(restarted.position, SequencePosition::Unknown);
    }

    fn row(reverse: bool, sequence: u32, acknowledgment: u32, flags: u16, bytes: u32) -> PacketRow {
        let a = Endpoint::V4(Ipv4Addr::new(192, 0, 2, 1));
        let b = Endpoint::V4(Ipv4Addr::new(198, 51, 100, 2));
        let (source, destination, src_port, dst_port) = if reverse {
            (b, a, 443, 54321)
        } else {
            (a, b, 54321, 443)
        };
        PacketRow {
            header: PacketHeader {
                row: 1,
                interface: InterfaceId {
                    section: 0,
                    ordinal: 0,
                },
                time: None,
                link_type: 1,
                original_len: 54 + bytes,
                captured_len: 54,
            },
            protocol: ProtocolFields {
                protocol: Protocol::Tcp,
                source: Some(source),
                destination: Some(destination),
                source_port: Some(src_port),
                destination_port: Some(dst_port),
                tcp: Some(TcpSegment {
                    sequence,
                    acknowledgment,
                    flags,
                    advertised_window: 65535,
                    payload_len: Some(bytes),
                }),
                tcp_flags: Some(flags),
                ..ProtocolFields::default()
            },
        }
    }
}
