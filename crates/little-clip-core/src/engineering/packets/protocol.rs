use std::{
    fmt,
    net::{Ipv4Addr, Ipv6Addr},
    ops::Range,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Ethernet([u8; 6]),
    V4(Ipv4Addr),
    V6(Ipv6Addr),
}

impl fmt::Display for Endpoint {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ethernet([a, b, c, d, e, f]) => {
                write!(output, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
            }
            Self::V4(address) => address.fmt(output),
            Self::V6(address) => address.fmt(output),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    Unknown,
    Ethernet,
    Arp,
    Ipv4,
    Ipv6,
    Icmp,
    Icmpv6,
    Tcp,
    Udp,
    Dns,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Ethernet => "Ethernet",
            Self::Arp => "ARP",
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
            Self::Icmp => "ICMP",
            Self::Icmpv6 => "ICMPv6",
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
            Self::Dns => "DNS",
        }
    }
}

/// Passive fields decoded from the captured packet prefix. Unknown or
/// truncated encapsulations retain the fields already observed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtocolFields {
    pub protocol: Protocol,
    pub source: Option<Endpoint>,
    pub destination: Option<Endpoint>,
    pub source_port: Option<u16>,
    pub destination_port: Option<u16>,
    pub vlan: Option<u16>,
    pub tcp_flags: Option<u16>,
    pub icmp_type: Option<u8>,
    pub fragmented: bool,
}

pub(super) fn inspect(link: u16, bytes: &[u8]) -> ProtocolFields {
    describe(link, bytes).0
}

#[derive(Default)]
pub(super) struct Locations {
    pub network: Option<(u16, usize)>,
    pub transport: Option<(u8, Range<usize>)>,
}

pub(super) fn describe(link: u16, bytes: &[u8]) -> (ProtocolFields, Locations) {
    let mut data = ProtocolFields::default();
    let mut locations = Locations::default();
    if let Some((kind, network)) = link_header(link, bytes, &mut data) {
        let offset = bytes.len() - network.len();
        locations.network = Some((kind, offset));
        let transport = match kind {
            0x0800 => ipv4(network, &mut data),
            0x86dd => ipv6(network, &mut data),
            0x0806 => {
                arp(network, &mut data);
                None
            }
            _ => None,
        };
        locations.transport =
            transport.map(|(kind, range)| (kind, range.start + offset..range.end + offset));
    }
    (data, locations)
}

fn extract<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..)?.first_chunk::<N>().copied()
}

fn word(bytes: &[u8], offset: usize) -> Option<u16> {
    extract(bytes, offset).map(u16::from_be_bytes)
}

fn link_header<'a>(
    link: u16,
    bytes: &'a [u8],
    data: &mut ProtocolFields,
) -> Option<(u16, &'a [u8])> {
    match link {
        1 => ethernet(bytes, data),
        113 => Some((word(bytes, 14)?, bytes.get(16..)?)),
        276 => Some((word(bytes, 0)?, bytes.get(20..)?)),
        12 | 101 => raw_ip(bytes),
        228 => Some((0x0800, bytes)),
        229 => Some((0x86dd, bytes)),
        0 | 108 => {
            let family: [u8; 4] = extract(bytes, 0)?;
            let network = bytes.get(4..)?;
            match (u32::from_le_bytes(family), u32::from_be_bytes(family)) {
                (2, _) | (_, 2) => Some((0x0800, network)),
                (10 | 24 | 28 | 30, _) | (_, 10 | 24 | 28 | 30) => Some((0x86dd, network)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn raw_ip(bytes: &[u8]) -> Option<(u16, &[u8])> {
    match bytes.first()? >> 4 {
        4 => Some((0x0800, bytes)),
        6 => Some((0x86dd, bytes)),
        _ => None,
    }
}

fn ethernet<'a>(bytes: &'a [u8], data: &mut ProtocolFields) -> Option<(u16, &'a [u8])> {
    data.protocol = Protocol::Ethernet;
    data.destination = extract(bytes, 0).map(Endpoint::Ethernet);
    data.source = extract(bytes, 6).map(Endpoint::Ethernet);
    let mut kind = word(bytes, 12)?;
    let mut offset = 14;
    for _ in 0..4 {
        if !matches!(kind, 0x8100 | 0x88a8 | 0x9100) {
            break;
        }
        let vlan = word(bytes, offset)? & 0x0fff;
        data.vlan.get_or_insert(vlan);
        kind = word(bytes, offset + 2)?;
        offset += 4;
    }
    Some((kind, bytes.get(offset..)?))
}

fn arp(bytes: &[u8], data: &mut ProtocolFields) {
    data.protocol = Protocol::Arp;
    if bytes.get(2..6) == Some(&[0x08, 0x00, 6, 4]) {
        data.source = extract::<4>(bytes, 14).map(|address| Endpoint::V4(address.into()));
        data.destination = extract::<4>(bytes, 24).map(|address| Endpoint::V4(address.into()));
    }
}

fn ipv4(bytes: &[u8], data: &mut ProtocolFields) -> Option<(u8, Range<usize>)> {
    let Some(head) = bytes.first_chunk::<20>() else {
        return None;
    };
    let header = usize::from(head[0] & 0x0f) * 4;
    if head[0] >> 4 != 4 || header < 20 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([head[2], head[3]]));
    if length < header {
        return None;
    }
    data.protocol = Protocol::Ipv4;
    data.source = extract::<4>(head, 12).map(|address| Endpoint::V4(address.into()));
    data.destination = extract::<4>(head, 16).map(|address| Endpoint::V4(address.into()));
    let fragment = u16::from_be_bytes([head[6], head[7]]);
    data.fragmented = fragment & 0x3fff != 0;
    if fragment & 0x1fff == 0
        && let Some(layer) = bytes.get(header..length.min(bytes.len()))
    {
        transport(head[9], layer, data);
        return Some((head[9], header..header + layer.len()));
    }
    None
}

fn ipv6(bytes: &[u8], data: &mut ProtocolFields) -> Option<(u8, Range<usize>)> {
    let Some(head) = bytes.first_chunk::<40>() else {
        return None;
    };
    if head[0] >> 4 != 6 {
        return None;
    }
    data.protocol = Protocol::Ipv6;
    data.source = extract::<16>(head, 8).map(|address| Endpoint::V6(address.into()));
    data.destination = extract::<16>(head, 24).map(|address| Endpoint::V6(address.into()));
    let length = usize::from(u16::from_be_bytes([head[4], head[5]]));
    // A zero payload length can indicate an IPv6 jumbogram; obey the captured extent.
    let end = if length == 0 {
        bytes.len()
    } else {
        bytes.len().min(40 + length)
    };
    let mut rest = &bytes[40..end];
    let mut kind = head[6];
    for _ in 0..8 {
        let length = match kind {
            0 | 43 | 60 => rest.get(1).map(|length| (usize::from(*length) + 1) * 8),
            51 => rest.get(1).map(|length| (usize::from(*length) + 2) * 4),
            44 => {
                data.fragmented = true;
                if word(rest, 2).is_none_or(|fragment| fragment & 0xfff8 != 0) {
                    return None;
                }
                Some(8)
            }
            _ => {
                transport(kind, rest, data);
                return Some((kind, end - rest.len()..end));
            }
        };
        let Some((next, tail)) = rest
            .first()
            .copied()
            .zip(length.and_then(|length| rest.get(length..)))
        else {
            return None;
        };
        kind = next;
        rest = tail;
    }
    None
}

fn transport(kind: u8, layer: &[u8], data: &mut ProtocolFields) {
    match kind {
        1 | 58 => {
            data.protocol = if kind == 1 {
                Protocol::Icmp
            } else {
                Protocol::Icmpv6
            };
            data.icmp_type = layer.first().copied();
        }
        6 | 17 => {
            data.protocol = if kind == 6 {
                Protocol::Tcp
            } else {
                Protocol::Udp
            };
            data.source_port = word(layer, 0);
            data.destination_port = word(layer, 2);
            let header = if kind == 6 {
                if let Some(flags) = word(layer, 12) {
                    data.tcp_flags = Some(flags & 0x01ff);
                    usize::from(flags >> 12) * 4 + 2
                } else {
                    return;
                }
            } else {
                8
            };
            if (data.source_port == Some(53) || data.destination_port == Some(53))
                && header >= if kind == 6 { 22 } else { 8 }
                && layer.len() >= header + 12
            {
                data.protocol = Protocol::Dns;
            }
        }
        _ => {}
    }
}
