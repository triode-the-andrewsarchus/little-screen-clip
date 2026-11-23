use super::{Packet, ProtocolFields, protocol};
use std::{fmt::Display, ops::Range};

/// Decoded field and its exact source byte range relative to the captured frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub filter: Option<String>,
    pub bytes: Range<usize>,
}

/// Captured protocol header or payload and its immediately decoded children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub name: &'static str,
    pub bytes: Range<usize>,
    pub fields: Vec<Field>,
}

pub(super) fn layers(packet: &Packet) -> Vec<Layer> {
    let bytes = &packet.bytes;
    if bytes.is_empty() {
        return Vec::new();
    }
    let (summary, locations) = protocol::describe(packet.header.link_type, bytes);
    let mut layers = Vec::new();
    let network_start = locations.network.as_ref().map_or(0, |(_, begin)| *begin);
    if packet.header.link_type == 1 && bytes.len() >= 14 {
        ethernet(&mut layers, bytes, network_start.max(14));
    } else if network_start > 0 {
        layers.push(Layer::new("Link header", 0..network_start));
    }
    if let Some((kind, start)) = locations.network {
        let end = locations
            .transport
            .as_ref()
            .map_or(bytes.len(), |(_, range)| range.start);
        network(&mut layers, bytes, kind, start..end, &summary);
    }
    if let Some((kind, range)) = locations.transport {
        transport(&mut layers, bytes, kind, range, &summary);
    }
    let consumed = layers.last().map_or(0, |layer| layer.bytes.end);
    if consumed < bytes.len() {
        layers.push(Layer::new(
            if consumed == 0 {
                "Captured data"
            } else {
                "Remaining data"
            },
            consumed..bytes.len(),
        ));
    }
    layers
}

impl Layer {
    fn new(name: &'static str, bytes: Range<usize>) -> Self {
        Self {
            name,
            bytes,
            fields: Vec::new(),
        }
    }

    fn add(
        &mut self,
        label: &'static str,
        offset: usize,
        len: usize,
        value: impl Display,
        key: &str,
    ) {
        let first = self.bytes.start + offset;
        let end = first + len;
        if end <= self.bytes.end {
            let value = value.to_string();
            let filter = (!key.is_empty()).then(|| format!("{key} == {value}"));
            self.fields.push(Field {
                label,
                value,
                filter,
                bytes: first..end,
            });
        }
    }

    fn word(&mut self, bytes: &[u8], name: &'static str, offset: usize, key: &str) {
        if let Some(value) = read::<2>(bytes, self.bytes.start + offset) {
            self.add(name, offset, 2, u16::from_be_bytes(value), key);
        }
    }

    fn byte(&mut self, bytes: &[u8], name: &'static str, offset: usize, key: &str) {
        if let Some(&value) = bytes.get(self.bytes.start + offset) {
            self.add(name, offset, 1, value, key);
        }
    }
}

fn read<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
    bytes.get(offset..)?.first_chunk().copied()
}

fn ethernet(layers: &mut Vec<Layer>, bytes: &[u8], end: usize) {
    use super::Endpoint;
    let mut layer = Layer::new("Ethernet II", 0..14);
    if let (Some(dst), Some(src)) = (read(bytes, 0), read(bytes, 6)) {
        layer.add("Destination", 0, 6, Endpoint::Ethernet(dst), "eth.dst");
        layer.add("Source", 6, 6, Endpoint::Ethernet(src), "eth.src");
    }
    layer.word(bytes, "EtherType", 12, "eth.type");
    layers.push(layer);
    for start in (14..end).step_by(4) {
        if let Some(tag) = read::<2>(bytes, start).map(u16::from_be_bytes) {
            let mut vlan = Layer::new("802.1Q VLAN", start..(start + 4).min(bytes.len()));
            vlan.add("VLAN ID", 0, 2, tag & 0x0fff, "vlan.id");
            vlan.add("Priority", 0, 2, tag >> 13, "vlan.priority");
            vlan.word(bytes, "Encapsulated EtherType", 2, "vlan.etype");
            layers.push(vlan);
        }
    }
}

fn network(
    layers: &mut Vec<Layer>,
    bytes: &[u8],
    kind: u16,
    mut range: Range<usize>,
    data: &ProtocolFields,
) {
    let start = range.start;
    match kind {
        0x0800 if range.len() >= 20 => {
            let Some(&first) = bytes
                .get(start)
                .filter(|&&first| first >> 4 == 4 && first & 0x0f >= 5)
            else {
                return;
            };
            range.end = range.end.min(start + usize::from(first & 0x0f) * 4);
            let mut ip = Layer::new("IPv4", range);
            ip.add("Version", 0, 1, 4, "ip.version");
            ip.add(
                "Header length",
                0,
                1,
                usize::from(first & 0x0f) * 4,
                "ip.hdr_len",
            );
            ip.word(bytes, "Total length", 2, "ip.len");
            ip.word(bytes, "Identification", 4, "ip.id");
            ip.word(bytes, "Flags and fragment offset", 6, "");
            ip.byte(bytes, "Time to live", 8, "ip.ttl");
            ip.byte(bytes, "Protocol", 9, "ip.proto");
            ip.word(bytes, "Header checksum", 10, "ip.checksum");
            addresses(&mut ip, data, 12, 4, ["ip.src", "ip.dst"]);
            layers.push(ip);
        }
        0x86dd if range.len() >= 40 && bytes[start] >> 4 == 6 => {
            let extension_end = range.end;
            let mut ip = Layer::new("IPv6", start..start + 40);
            ip.add("Version", 0, 1, 6, "ipv6.version");
            ip.word(bytes, "Payload length", 4, "ipv6.plen");
            ip.byte(bytes, "Next header", 6, "ipv6.nxt");
            ip.byte(bytes, "Hop limit", 7, "ipv6.hlim");
            addresses(&mut ip, data, 8, 16, ["ipv6.src", "ipv6.dst"]);
            layers.push(ip);
            if extension_end > start + 40
                && matches!(bytes[start + 6], 0 | 43 | 44 | 51 | 60)
                && extension_end - start <= 2048
            {
                layers.push(Layer::new("IPv6 extension data", start + 40..extension_end));
            }
        }
        0x0806 if range.len() >= 8 => {
            let mut arp = Layer::new("ARP", start..range.end.min(start + 28));
            arp.word(bytes, "Hardware type", 0, "arp.hw.type");
            arp.word(bytes, "Protocol type", 2, "arp.proto.type");
            arp.byte(bytes, "Hardware address length", 4, "arp.hw.size");
            arp.byte(bytes, "Protocol address length", 5, "arp.proto.size");
            arp.word(bytes, "Operation", 6, "arp.opcode");
            if let Some(value) = data.source {
                arp.add("Sender IP", 14, 4, value, "arp.src.proto_ipv4");
            }
            if let Some(value) = data.destination {
                arp.add("Target IP", 24, 4, value, "arp.dst.proto_ipv4");
            }
            layers.push(arp);
        }
        _ => {}
    }
}

fn addresses(
    layer: &mut Layer,
    data: &ProtocolFields,
    offset: usize,
    count: usize,
    keys: [&str; 2],
) {
    for (index, (value, label)) in [(data.source, "Source"), (data.destination, "Destination")]
        .into_iter()
        .enumerate()
    {
        if let Some(value) = value {
            layer.add(label, offset + index * count, count, value, keys[index]);
        }
    }
}

fn transport(
    layers: &mut Vec<Layer>,
    bytes: &[u8],
    kind: u8,
    range: Range<usize>,
    data: &ProtocolFields,
) {
    let start = range.start;
    let length = range.len();
    let head = match kind {
        6 => bytes
            .get(start + 12)
            .map_or(length.min(20), |byte| usize::from(byte >> 4).max(5) * 4)
            .min(length),
        17 => length.min(8),
        1 | 58 => length.min(8),
        _ => 0,
    };
    if head == 0 {
        return;
    }
    let mut layer = Layer::new(
        match kind {
            6 => "TCP",
            17 => "UDP",
            1 => "ICMP",
            _ => "ICMPv6",
        },
        start..start + head,
    );
    match kind {
        6 => tcp_fields(&mut layer, bytes, data),
        17 => {
            for (label, offset, key) in [
                ("Source port", 0, "udp.srcport"),
                ("Destination port", 2, "udp.dstport"),
                ("Length", 4, "udp.length"),
                ("Checksum", 6, "udp.checksum"),
            ] {
                layer.word(bytes, label, offset, key);
            }
        }
        1 | 58 => {
            layer.byte(
                bytes,
                "Type",
                0,
                if kind == 1 {
                    "icmp.type"
                } else {
                    "icmpv6.type"
                },
            );
            layer.byte(
                bytes,
                "Code",
                1,
                if kind == 1 {
                    "icmp.code"
                } else {
                    "icmpv6.code"
                },
            );
            layer.word(bytes, "Checksum", 2, "");
        }
        _ => {}
    }
    layers.push(layer);
    let payload = start + head;
    let mut end = range.end;
    if kind == 17
        && let Some(len) = read::<2>(bytes, start + 4)
            .map(u16::from_be_bytes)
            .filter(|&len| len >= 8)
    {
        end = end.min(start + usize::from(len));
    }
    let dns = payload + if kind == 6 { 2 } else { 0 };
    if data.protocol == super::Protocol::Dns && dns + 12 <= end {
        let mut layer = Layer::new("DNS", payload..end);
        for (label, offset, key) in [
            ("Transaction ID", 0, "dns.id"),
            ("Flags", 2, "dns.flags"),
            ("Questions", 4, "dns.count.queries"),
            ("Answers", 6, "dns.count.answers"),
            ("Authority records", 8, "dns.count.auth_rr"),
            ("Additional records", 10, "dns.count.add_rr"),
        ] {
            layer.word(bytes, label, dns - payload + offset, key);
        }
        layers.push(layer);
    } else if payload < end {
        layers.push(Layer::new("Payload", payload..end));
    }
}

fn tcp_fields(layer: &mut Layer, bytes: &[u8], data: &ProtocolFields) {
    for (label, offset, key) in [
        ("Source port", 0, "tcp.srcport"),
        ("Destination port", 2, "tcp.dstport"),
        ("Window", 14, "tcp.window_size_value"),
        ("Checksum", 16, "tcp.checksum"),
        ("Urgent pointer", 18, "tcp.urgent_pointer"),
    ] {
        layer.word(bytes, label, offset, key);
    }
    for (label, offset, key) in [
        ("Sequence", 4, "tcp.seq_raw"),
        ("Acknowledgment", 8, "tcp.ack_raw"),
    ] {
        if let Some(value) = read::<4>(bytes, layer.bytes.start + offset) {
            layer.add(label, offset, 4, u32::from_be_bytes(value), key);
        }
    }
    if let Some(flags) = data.tcp_flags {
        layer.add("Flags", 12, 2, flags, "tcp.flags");
    }
    if layer.bytes.len() > 20 {
        layer.add(
            "Options",
            20,
            layer.bytes.len() - 20,
            format!("{} bytes", layer.bytes.len() - 20),
            "",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::packets::{InterfaceId, PacketHeader};

    #[test]
    fn tagged_ipv4_options_udp_and_payload_point_to_the_captured_bytes() {
        let mut bytes = vec![0; 55];
        bytes[12..14].copy_from_slice(&0x8100_u16.to_be_bytes());
        bytes[14..16].copy_from_slice(&7_u16.to_be_bytes());
        bytes[16..18].copy_from_slice(&0x0800_u16.to_be_bytes());
        bytes[18] = 0x46;
        bytes[20..22].copy_from_slice(&37_u16.to_be_bytes());
        bytes[27] = 17;
        bytes[30..34].copy_from_slice(&[192, 0, 2, 7]);
        bytes[34..38].copy_from_slice(&[198, 51, 100, 2]);
        bytes[42..44].copy_from_slice(&5100_u16.to_be_bytes());
        bytes[44..46].copy_from_slice(&8080_u16.to_be_bytes());
        bytes[46..48].copy_from_slice(&13_u16.to_be_bytes());
        let packet = Packet {
            header: PacketHeader {
                row: 0,
                interface: InterfaceId {
                    section: 0,
                    ordinal: 0,
                },
                link_type: 1,
                time: None,
                captured_len: 55,
                original_len: 55,
            },
            bytes,
        };
        let layers = packet.layers();
        assert_eq!(
            layers
                .iter()
                .map(|layer| layer.bytes.clone())
                .collect::<Vec<_>>(),
            [0..14, 14..18, 18..42, 42..50, 50..55]
        );
        let source = layers[2]
            .fields
            .iter()
            .find(|field| field.label == "Source")
            .unwrap();
        assert_eq!(source.bytes, 30..34);
        assert_eq!(source.filter.as_deref(), Some("ip.src == 192.0.2.7"));
        let port = layers[3]
            .fields
            .iter()
            .find(|field| field.label == "Destination port")
            .unwrap();
        assert_eq!(port.bytes, 44..46);
        assert_eq!(port.filter.as_deref(), Some("udp.dstport == 8080"));
    }
}
