//! Common display-filter expressions evaluated from capture rows, without a decoder subprocess.
use little_clip_core::engineering::packets::{Endpoint, PacketRow, Protocol, TcpSegment};
use std::{cmp::Ordering, net::IpAddr, str::FromStr};

pub(super) struct DisplayFilter(Expr);

enum Expr {
    Test(Field, Option<(Compare, Literal)>),
    In(Field, Vec<Member>),
    Not(Box<Expr>),
    Both(Box<Expr>, Box<Expr>),
    Either(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
}

enum Member {
    Value(Literal),
    Range(Literal, Literal),
}

impl Member {
    fn contains(&self, value: Literal) -> bool {
        match *self {
            Self::Value(target) => value.compare(target).is_some_and(Ordering::is_eq),
            Self::Range(first, last) => {
                value.compare(first).is_some_and(|order| !order.is_lt())
                    && value.compare(last).is_some_and(|order| !order.is_gt())
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Compare {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy)]
enum Literal {
    Integer(u64),
    Address(IpAddr, u8),
}

impl Literal {
    fn parse(text: &str) -> Option<Self> {
        if let Some(value) = match text {
            "true" => Some(1),
            "false" => Some(0),
            value if value.starts_with("0x") => u64::from_str_radix(&value[2..], 16).ok(),
            value => value.parse().ok(),
        } {
            return Some(Self::Integer(value));
        }
        let (address, length) = text
            .split_once('/')
            .map_or((text, None), |(address, length)| (address, Some(length)));
        let address = IpAddr::from_str(address).ok()?;
        let max = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match length {
            Some(value) => value.parse::<u8>().ok().filter(|value| *value <= max)?,
            None => max,
        };
        Some(Self::Address(address, prefix))
    }

    fn exact(self) -> bool {
        matches!(
            self,
            Self::Integer(_) | Self::Address(IpAddr::V4(_), 32) | Self::Address(IpAddr::V6(_), 128)
        )
    }

    fn compare(self, rhs: Self) -> Option<Ordering> {
        match (self, rhs) {
            (Self::Integer(left), Self::Integer(right)) => Some(left.cmp(&right)),
            (Self::Address(IpAddr::V4(left), _), Self::Address(IpAddr::V4(right), prefix)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
                Some((u32::from(left) & mask).cmp(&(u32::from(right) & mask)))
            }
            (Self::Address(IpAddr::V6(left), _), Self::Address(IpAddr::V6(right), prefix)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
                Some((u128::from(left) & mask).cmp(&(u128::from(right) & mask)))
            }
            _ => None,
        }
    }
}

impl Compare {
    fn matches(self, ordering: Ordering) -> bool {
        match self {
            Self::Eq => ordering.is_eq(),
            Self::Ne => !ordering.is_eq(),
            Self::Lt => ordering.is_lt(),
            Self::Le => !ordering.is_gt(),
            Self::Gt => ordering.is_gt(),
            Self::Ge => !ordering.is_lt(),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "==" | "eq" => Self::Eq,
            "!=" | "ne" => Self::Ne,
            "<" | "lt" => Self::Lt,
            "<=" | "le" => Self::Le,
            ">" | "gt" => Self::Gt,
            ">=" | "ge" => Self::Ge,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
enum Side {
    Source,
    Destination,
    Either,
}

#[derive(Clone, Copy)]
enum TcpHeader {
    Sequence,
    Acknowledgment,
    PayloadLength,
    Window,
}

impl TcpHeader {
    fn value(self, segment: TcpSegment) -> Option<u64> {
        let number = match self {
            Self::Sequence => segment.sequence,
            Self::Acknowledgment => segment.acknowledgment,
            Self::PayloadLength => segment.payload_len?,
            Self::Window => u32::from(segment.advertised_window),
        };
        Some(u64::from(number))
    }
}

#[derive(Clone, Copy)]
enum Field {
    Protocol(Protocol),
    Ip(bool, Side),
    Port(Protocol, Side),
    FrameNumber,
    WireLength,
    CapturedLength,
    Vlan,
    TcpFlags(Option<u16>),
    TcpHeader(TcpHeader),
    IcmpKind(Protocol),
}

impl Field {
    fn parse(text: &str) -> Option<Self> {
        let value = match text {
            "frame" => Self::FrameNumber,
            "frame.number" => Self::FrameNumber,
            "frame.len" => Self::WireLength,
            "frame.cap_len" => Self::CapturedLength,
            "eth" => Self::Protocol(Protocol::Ethernet),
            "arp" => Self::Protocol(Protocol::Arp),
            "ip" => Self::Protocol(Protocol::Ipv4),
            "ipv6" => Self::Protocol(Protocol::Ipv6),
            "tcp" => Self::Protocol(Protocol::Tcp),
            "udp" => Self::Protocol(Protocol::Udp),
            "dns" => Self::Protocol(Protocol::Dns),
            "icmp" => Self::Protocol(Protocol::Icmp),
            "icmpv6" => Self::Protocol(Protocol::Icmpv6),
            "icmp.type" => Self::IcmpKind(Protocol::Icmp),
            "icmpv6.type" => Self::IcmpKind(Protocol::Icmpv6),
            "vlan" | "vlan.id" => Self::Vlan,
            "tcp.flags" => Self::TcpFlags(None),
            "tcp.seq_raw" => Self::TcpHeader(TcpHeader::Sequence),
            "tcp.ack_raw" => Self::TcpHeader(TcpHeader::Acknowledgment),
            "tcp.len" => Self::TcpHeader(TcpHeader::PayloadLength),
            "tcp.window_size_value" => Self::TcpHeader(TcpHeader::Window),
            _ => return Self::parse_member(text),
        };
        Some(value)
    }

    fn parse_member(text: &str) -> Option<Self> {
        let (layer, field) = text.split_once('.')?;
        let side = match field {
            "src" | "srcport" => Side::Source,
            "dst" | "dstport" => Side::Destination,
            "addr" | "port" => Side::Either,
            _ => {
                return match (layer, field) {
                    ("tcp", "flags.fin") => Some(Self::TcpFlags(Some(1))),
                    ("tcp", "flags.syn") => Some(Self::TcpFlags(Some(2))),
                    ("tcp", "flags.reset") => Some(Self::TcpFlags(Some(4))),
                    ("tcp", "flags.push") => Some(Self::TcpFlags(Some(8))),
                    ("tcp", "flags.ack") => Some(Self::TcpFlags(Some(16))),
                    ("tcp", "flags.urg") => Some(Self::TcpFlags(Some(32))),
                    _ => None,
                };
            }
        };
        match layer {
            "ip" | "ipv6" if matches!(field, "src" | "dst" | "addr") => {
                Some(Self::Ip(layer == "ipv6", side))
            }
            "tcp" | "udp" if matches!(field, "srcport" | "dstport" | "port") => Some(Self::Port(
                if layer == "tcp" {
                    Protocol::Tcp
                } else {
                    Protocol::Udp
                },
                side,
            )),
            _ => None,
        }
    }

    fn literal(self, text: &str) -> Option<Literal> {
        if matches!(self, Self::Protocol(_)) {
            return None;
        }
        let value = Literal::parse(text)?;
        (matches!(self, Self::Ip(..)) == matches!(value, Literal::Address(..))).then_some(value)
    }

    fn values(self, row: &PacketRow) -> [Option<Literal>; 2] {
        let data = row.protocol;
        let number = |value| [Some(Literal::Integer(value)), None];
        match self {
            Self::Protocol(protocol) => {
                if has_protocol(row, protocol) {
                    number(1)
                } else {
                    [None; 2]
                }
            }
            Self::Ip(ipv6, side) if data.protocol != Protocol::Arp => side
                .values(data.source, data.destination)
                .map(|value| match value {
                    Some(Endpoint::V4(address)) if !ipv6 => {
                        Some(Literal::Address(address.into(), 32))
                    }
                    Some(Endpoint::V6(address)) if ipv6 => {
                        Some(Literal::Address(address.into(), 128))
                    }
                    _ => None,
                }),
            Self::Port(protocol, side) if has_protocol(row, protocol) => side
                .values(data.source_port, data.destination_port)
                .map(|value| value.map(|port| Literal::Integer(u64::from(port)))),
            Self::FrameNumber => number(row.header.row + 1),
            Self::WireLength => number(u64::from(row.header.original_len)),
            Self::CapturedLength => number(u64::from(row.header.captured_len)),
            Self::Vlan => data
                .vlan
                .map_or([None; 2], |value| number(u64::from(value))),
            Self::TcpFlags(mask) => data.tcp_flags.map_or([None; 2], |flags| {
                number(mask.map_or(u64::from(flags), |mask| u64::from(flags & mask != 0)))
            }),
            Self::TcpHeader(field) => data
                .tcp
                .and_then(|segment| field.value(segment))
                .map_or([None; 2], number),
            Self::IcmpKind(protocol) if protocol == data.protocol => data
                .icmp_type
                .map_or([None; 2], |value| number(u64::from(value))),
            _ => [None; 2],
        }
    }
}

impl Side {
    fn values<T>(self, source: Option<T>, destination: Option<T>) -> [Option<T>; 2] {
        match self {
            Self::Source => [source, None],
            Self::Destination => [destination, None],
            Self::Either => [source, destination],
        }
    }
}

pub(super) fn has_protocol(row: &PacketRow, protocol: Protocol) -> bool {
    let data = row.protocol;
    if data.protocol == protocol {
        return true;
    }
    match protocol {
        Protocol::Ethernet => row.header.link_type == 1 && row.header.captured_len >= 14,
        Protocol::Ipv4 => {
            data.protocol != Protocol::Arp && matches!(data.source, Some(Endpoint::V4(_)))
        }
        Protocol::Ipv6 => matches!(data.source, Some(Endpoint::V6(_))),
        Protocol::Tcp => data.protocol == Protocol::Dns && data.tcp_flags.is_some(),
        Protocol::Udp => data.protocol == Protocol::Dns && data.tcp_flags.is_none(),
        _ => false,
    }
}

impl DisplayFilter {
    /// Returns None for terms outside the native display-filter subset.
    pub fn compile(text: &str) -> Option<Self> {
        let tokens = lex(text)?;
        let mut parser = Parser {
            tokens: &tokens,
            cursor: 0,
        };
        let expr = parser.expression(0, 0)?;
        (parser.cursor == tokens.len()).then_some(Self(expr))
    }

    pub fn matches(&self, row: &PacketRow) -> bool {
        self.0.matches(row)
    }
}

impl Expr {
    fn matches(&self, row: &PacketRow) -> bool {
        match self {
            Self::Not(expr) => !expr.matches(row),
            Self::Both(left, right) => left.matches(row) && right.matches(row),
            Self::Either(left, right) => left.matches(row) || right.matches(row),
            Self::Xor(left, right) => left.matches(row) != right.matches(row),
            Self::In(field, set) => field
                .values(row)
                .into_iter()
                .flatten()
                .any(|value| set.iter().any(|item| item.contains(value))),
            Self::Test(field, test) => {
                let mut values = field.values(row).into_iter().flatten();
                let Some((operation, target)) = test else {
                    return values.next().is_some();
                };
                let compare = |value: Literal| {
                    value
                        .compare(*target)
                        .is_some_and(|ordering| operation.matches(ordering))
                };
                if matches!(operation, Compare::Ne) {
                    let Some(first) = values.next() else {
                        return false;
                    };
                    compare(first) && values.all(compare)
                } else {
                    values.any(compare)
                }
            }
        }
    }
}

struct Parser<'a> {
    tokens: &'a [&'a str],
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn take(&mut self) -> Option<&'a str> {
        let item = self.tokens.get(self.cursor).copied()?;
        self.cursor += 1;
        Some(item)
    }

    fn expression(&mut self, min_precedence: u8, depth: u8) -> Option<Expr> {
        if depth >= 28 {
            return None;
        }
        let mut expr = self.term(depth + 1)?;
        loop {
            let op = self.tokens.get(self.cursor).copied().unwrap_or_default();
            let (precedence, combine): (u8, fn(Box<Expr>, Box<Expr>) -> Expr) = match op {
                "and" | "&&" => (3, Expr::Both),
                "xor" | "^^" => (2, Expr::Xor),
                "or" | "||" => (1, Expr::Either),
                _ => break,
            };
            if precedence < min_precedence {
                break;
            }
            self.cursor += 1;
            let other = self.expression(precedence + 1, depth + 1)?;
            expr = combine(Box::new(expr), Box::new(other));
        }
        Some(expr)
    }

    fn term(&mut self, depth: u8) -> Option<Expr> {
        if depth >= 28 {
            return None;
        }
        let token = self.take()?;
        match token {
            "not" | "!" => Some(Expr::Not(Box::new(self.term(depth + 1)?))),
            "(" => {
                let expr = self.expression(0, depth + 1)?;
                (self.take() == Some(")")).then_some(expr)
            }
            _ => {
                let field = Field::parse(token)?;
                if self.consume("in") {
                    return Some(Expr::In(field, self.members(field)?));
                }
                let test = match self
                    .tokens
                    .get(self.cursor)
                    .and_then(|token| Compare::parse(token))
                {
                    Some(op) => {
                        self.cursor += 1;
                        Some((op, field.literal(self.take()?)?))
                    }
                    None => None,
                };
                Some(Expr::Test(field, test))
            }
        }
    }
    fn consume(&mut self, expected: &str) -> bool {
        if self.tokens.get(self.cursor) != Some(&expected) {
            return false;
        }
        self.cursor += 1;
        true
    }

    fn members(&mut self, field: Field) -> Option<Vec<Member>> {
        self.consume("{").then_some(())?;
        let mut set = Vec::new();
        loop {
            let first = field.literal(self.take()?)?;
            let item = if self.consume("..") {
                let last = field.literal(self.take()?)?;
                if !first.exact() || !last.exact() || first.compare(last)?.is_gt() {
                    return None;
                }
                Member::Range(first, last)
            } else {
                Member::Value(first)
            };
            set.push(item);
            if self.consume("}") {
                return Some(set);
            }
            self.consume(",").then_some(())?;
        }
    }
}

fn lex(text: &str) -> Option<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && tokens.len() < 192 {
        let first = rest.as_bytes()[0];
        let size = match first {
            b'(' | b')' | b'{' | b'}' | b',' => 1,
            b'.' if rest.starts_with("..") => 2,
            b'!' | b'<' | b'>' | b'=' => 1 + usize::from(rest.as_bytes().get(1) == Some(&b'=')),
            b'&' | b'|' | b'^' if rest.as_bytes().get(1) == Some(&first) => 2,
            _ => {
                let word = rest
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || b"._:/".contains(c))
                    .count();
                rest[..word].find("..").unwrap_or(word)
            }
        };
        if size == 0 {
            return None;
        }
        tokens.push(&rest[..size]);
        rest = rest[size..].trim_start();
    }
    rest.is_empty().then_some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::packets::{InterfaceId, PacketHeader, ProtocolFields};

    #[test]
    fn conditions_preserve_protocol_presence_port_pairs_and_operator_precedence() {
        let row = PacketRow {
            header: PacketHeader {
                row: 6,
                interface: InterfaceId {
                    section: 0,
                    ordinal: 0,
                },
                link_type: 1,
                time: None,
                captured_len: 60,
                original_len: 60,
            },
            protocol: ProtocolFields {
                protocol: Protocol::Tcp,
                source: Some(Endpoint::V4("192.168.1.2".parse().unwrap())),
                destination: Some(Endpoint::V4("192.168.1.4".parse().unwrap())),
                source_port: Some(3456),
                destination_port: Some(443),
                tcp_flags: Some(2),
                tcp: Some(TcpSegment {
                    sequence: u32::MAX - 1,
                    acknowledgment: 0,
                    flags: 2,
                    advertised_window: 4096,
                    payload_len: Some(8),
                }),
                ..Default::default()
            },
        };
        let matches = |text| DisplayFilter::compile(text).unwrap().matches(&row);
        assert!(matches(
            "ip.addr==192.168.1.0/24 and tcp.flags.syn == true && !tcp.flags.ack == 1"
        ));
        assert!(matches(
            "eth and (udp or tcp.port==443) and frame.number ge 7"
        ));
        assert!(!matches("tcp.port != 443 or udp.port != 443"));
        assert!(matches("not udp.port == 443"));
        assert!(matches(
            "tcp.seq_raw == 4294967294 and tcp.ack_raw == 0 and tcp.len > 0 and tcp.window_size_value == 4096"
        ));
        assert!(!matches("tcp.len == 0 or tcp.seq_raw < 123"));
        assert!(matches(
            "tcp.port in {80, 443, 4000..4500} && frame.len in { 50 .. 70 }"
        ));
        assert!(matches("ip.addr in {192.168.1.3..192.168.1.5, 127.0.0.1}"));
        assert!(matches("ip.src in {192.168.1.0/24}"));
        assert!(!matches("tcp.port in { 500 .. 3000 }")); // A single endpoint must satisfy both bounds.
        assert!(!matches("udp.port in { 1..65535 }"));
        assert!(DisplayFilter::compile("tcp.port in {70..60}").is_none());
        assert!(DisplayFilter::compile("tcp.port in {}").is_none());
        assert!(DisplayFilter::compile("http.host contains \"example\"").is_none());
    }
}
