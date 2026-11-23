use super::{Endpoint, PacketRow, Protocol, ProtocolFields};
use std::{
    cmp::Reverse,
    collections::{HashMap, hash_map::Entry},
    net::{IpAddr, SocketAddr},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Transport {
    Tcp,
    Udp,
}

/// An unordered transport endpoint pair within one observed VLAN context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConversationKey {
    pub peers: [SocketAddr; 2],
    transport: Transport,
    pub vlan: Option<u16>,
}

impl ConversationKey {
    /// Returns the pair and the index of the frame's source endpoint.
    pub fn identify(fields: ProtocolFields) -> Option<(Self, usize)> {
        let transport = match fields.protocol {
            Protocol::Tcp => Transport::Tcp,
            Protocol::Dns if fields.tcp_flags.is_some() => Transport::Tcp,
            Protocol::Udp | Protocol::Dns => Transport::Udp,
            _ => return None,
        };
        let source = socket(fields.source?, fields.source_port?)?;
        let destination = socket(fields.destination?, fields.destination_port?)?;
        let direction = usize::from(source > destination);
        let peers = if direction == 0 {
            [source, destination]
        } else {
            [destination, source]
        };
        let key = Self {
            peers,
            transport,
            vlan: fields.vlan,
        };
        Some((key, direction))
    }

    /// Returns the transport protocol shared by both directions.
    pub fn protocol(self) -> Protocol {
        match self.transport {
            Transport::Tcp => Protocol::Tcp,
            Transport::Udp => Protocol::Udp,
        }
    }
}

fn socket(endpoint: Endpoint, port: u16) -> Option<SocketAddr> {
    let address = match endpoint {
        Endpoint::V4(address) => IpAddr::V4(address),
        Endpoint::V6(address) => IpAddr::V6(address),
        Endpoint::Ethernet(_) => return None,
    };
    Some(SocketAddr::new(address, port))
}

/// Observed counts for one direction of a conversation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Direction {
    pub packets: u64,
    pub captured_bytes: u64,
}

/// Observations for one endpoint pair, in the order of its canonical peers.
#[derive(Clone, Debug)]
pub struct Conversation {
    pub key: ConversationKey,
    pub directions: [Direction; 2],
    pub time_ns: Option<[i128; 2]>,
}

impl Conversation {
    /// Returns the count in both directions.
    pub fn packet_count(&self) -> u64 {
        self.directions[0].packets + self.directions[1].packets
    }
}

/// A bounded summary of the transport endpoint pairs in the supplied rows.
pub struct Conversations {
    groups: HashMap<ConversationKey, Conversation>,
    cap: usize,
    pub omitted_packets: u64,
    pub unclassified_packets: u64,
}

impl Conversations {
    /// Limits the number of distinct pairs retained.
    pub fn new(cap: usize) -> Self {
        Self {
            groups: HashMap::new(),
            cap,
            omitted_packets: 0,
            unclassified_packets: 0,
        }
    }

    /// Includes one row, preserving captured size and observed timestamps.
    pub fn observe(&mut self, row: &PacketRow) {
        let Some((key, direction)) = ConversationKey::identify(row.protocol) else {
            self.unclassified_packets += 1;
            return;
        };
        let at_capacity = self.groups.len() >= self.cap;
        let group = match self.groups.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(_) if at_capacity => {
                self.omitted_packets += 1;
                return;
            }
            Entry::Vacant(entry) => entry.insert(Conversation {
                key,
                directions: [Direction::default(); 2],
                time_ns: None,
            }),
        };
        group.directions[direction].packets += 1;
        group.directions[direction].captured_bytes += u64::from(row.header.captured_len);
        if let Some(time) = row.header.time {
            let ns = time.unix_nanoseconds();
            let [first, last] = group.time_ns.get_or_insert([ns; 2]);
            *first = (*first).min(ns);
            *last = (*last).max(ns);
        }
    }

    /// Returns pairs ordered by decreasing packet count.
    pub fn ranked(self) -> Vec<Conversation> {
        let mut rows: Vec<_> = self.groups.into_values().collect();
        rows.sort_unstable_by_key(|group| (Reverse(group.packet_count()), group.key));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::packets::{InterfaceId, PacketHeader, Resolution, Timestamp};

    fn packet(reverse: bool, protocol: Protocol, time: u64) -> PacketRow {
        let endpoints = [
            (Endpoint::V4("192.0.2.1".parse().unwrap()), 12000),
            (Endpoint::V4("198.51.100.2".parse().unwrap()), 53),
        ];
        let (source, source_port) = endpoints[usize::from(reverse)];
        let (destination, destination_port) = endpoints[usize::from(!reverse)];
        PacketRow {
            header: PacketHeader {
                row: 0,
                interface: InterfaceId {
                    section: 0,
                    ordinal: 0,
                },
                link_type: 1,
                time: Some(Timestamp {
                    ticks: time,
                    resolution: Resolution::Decimal(9),
                    offset_seconds: 0,
                }),
                captured_len: 42,
                original_len: 100,
            },
            protocol: ProtocolFields {
                protocol,
                source: Some(source),
                destination: Some(destination),
                source_port: Some(source_port),
                destination_port: Some(destination_port),
                vlan: Some(1),
                ..Default::default()
            },
        }
    }

    #[test]
    fn counts_bidirectional_dns_and_udp_together_with_timestamp_extrema() {
        let mut groups = Conversations::new(10);
        groups.observe(&packet(false, Protocol::Udp, 200));
        groups.observe(&packet(true, Protocol::Dns, 100));
        let ranked = groups.ranked();
        assert_eq!(ranked.len(), 1);
        let group = &ranked[0];
        assert_eq!(group.directions.map(|direction| direction.packets), [1, 1]);
        assert_eq!(
            group.directions.map(|direction| direction.captured_bytes),
            [42, 42]
        );
        assert_eq!(group.time_ns, Some([100, 200]));
    }

    #[test]
    fn separates_vlan_and_transport_and_keeps_counting_at_capacity() {
        let udp = packet(false, Protocol::Udp, 0);
        let mut tcp = packet(true, Protocol::Dns, 10);
        tcp.protocol.tcp_flags = Some(0x18);
        let mut vlan = udp.clone();
        vlan.protocol.vlan = Some(2);
        let mut groups = Conversations::new(2);
        for row in [&udp, &tcp, &vlan, &udp] {
            groups.observe(row);
        }
        assert_eq!(groups.omitted_packets, 1);
        let rows = groups.ranked();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].packet_count(), 2);
        assert_eq!(rows[0].key.protocol(), Protocol::Udp);
        assert_eq!(rows[1].key.protocol(), Protocol::Tcp);
    }
}
