use super::{activity, inspect};
use crate::{
    file_manager::capture_browse::table::{Columns, Page, Response, Sort, TableOptions},
    ui_appearance::WorkbenchColors,
};
use egui::{Color32, Rect, Ui, vec2};
use little_clip_core::engineering::packets::{Endpoint, PacketRow, Protocol};
use std::cmp::Ordering;

const COLUMNS: Columns<6> = Columns {
    id: "packet rows",
    headings: ["No.", "Time", "Protocol", "Source", "Destination", "Bytes"],
    fractions: [0.09, 0.17, 0.12, 0.255, 0.255, 0.11],
    min_height: 50.0,
    selected_fill: 0.48,
    hover_fill: 0.20,
};

pub(super) fn show<'a>(
    ui: &mut Ui,
    view: (Page<'_>, TableOptions),
    origin: Option<i128>,
    colors: WorkbenchColors,
    colorize: bool,
    row: impl Fn(u64) -> Option<&'a PacketRow>,
) -> Response {
    COLUMNS.show_with_options(
        ui,
        view,
        colors,
        row,
        |packet| packet.header.row,
        |ui, rect, packet| {
            if colorize {
                let color = protocol_color(packet.protocol.protocol);
                let painter = ui.painter();
                painter.rect_filled(rect, 0.0, color.gamma_multiply(0.13));
                painter.rect_filled(
                    Rect::from_min_size(rect.min, vec2(2.0, rect.height())),
                    0.0,
                    color,
                );
            }
            row_contents(ui, rect, packet, origin, colors);
        },
    )
}

pub(super) fn compare(sort: Sort, left: &PacketRow, right: &PacketRow) -> Ordering {
    match sort.column {
        0 => sort.compare(left.header.row, right.header.row),
        1 => sort.known_first(
            left.header.time.map(|time| time.unix_nanoseconds()),
            right.header.time.map(|time| time.unix_nanoseconds()),
        ),
        2 => sort.compare(
            left.protocol.protocol.label(),
            right.protocol.protocol.label(),
        ),
        3 => sort.known_first(
            endpoint_key(left.protocol.source, left.protocol.source_port),
            endpoint_key(right.protocol.source, right.protocol.source_port),
        ),
        4 => sort.known_first(
            endpoint_key(left.protocol.destination, left.protocol.destination_port),
            endpoint_key(right.protocol.destination, right.protocol.destination_port),
        ),
        5 => sort.compare(left.header.captured_len, right.header.captured_len),
        _ => Ordering::Equal,
    }
}

fn endpoint_key(endpoint: Option<Endpoint>, port: Option<u16>) -> Option<(u8, u128, Option<u16>)> {
    let (family, value) = match endpoint? {
        Endpoint::Ethernet(bytes) => {
            let [a, b, c, d, e, f] = bytes;
            (0, u64::from_be_bytes([0, 0, a, b, c, d, e, f]).into())
        }
        Endpoint::V4(address) => (1, u32::from(address).into()),
        Endpoint::V6(address) => (2, u128::from(address)),
    };
    Some((family, value, port))
}

fn row_contents(
    ui: &Ui,
    rect: Rect,
    packet: &PacketRow,
    origin: Option<i128>,
    colors: WorkbenchColors,
) {
    let data = packet.protocol;
    let header = packet.header;
    let ordinal = (header.row + 1).to_string();
    let time = header.time.zip(origin).map_or_else(
        || "—".to_owned(),
        |(time, origin)| activity::relative(time.unix_nanoseconds() - origin),
    );
    let source = inspect::endpoint(data.source, data.source_port);
    let destination = inspect::endpoint(data.destination, data.destination_port);
    let bytes = header.captured_len.to_string();
    COLUMNS.cells(
        ui,
        rect,
        [
            &ordinal,
            &time,
            data.protocol.label(),
            &source,
            &destination,
            &bytes,
        ],
        colors.text_primary,
    );
}

pub(super) fn protocol_color(protocol: Protocol) -> Color32 {
    let [r, g, b] = match protocol {
        Protocol::Tcp => [125, 117, 245],
        Protocol::Udp => [69, 160, 247],
        Protocol::Dns => [54, 196, 181],
        Protocol::Icmp | Protocol::Icmpv6 => [229, 155, 73],
        Protocol::Arp => [226, 193, 73],
        Protocol::Ipv4 | Protocol::Ipv6 => [98, 175, 121],
        _ => [136, 137, 160],
    };
    Color32::from_rgb(r, g, b)
}

pub(super) fn layer_color(name: &str, dark: bool) -> Color32 {
    let protocol = match name {
        "TCP" => Protocol::Tcp,
        "UDP" => Protocol::Udp,
        "DNS" => Protocol::Dns,
        "ARP" => Protocol::Arp,
        "IPv4" => Protocol::Ipv4,
        "IPv6" => Protocol::Ipv6,
        "ICMP" | "ICMPv6" => Protocol::Icmp,
        _ => Protocol::Unknown,
    };
    let backdrop = if dark {
        Color32::from_gray(33)
    } else {
        Color32::from_gray(247)
    };
    backdrop.lerp_to_gamma(protocol_color(protocol), if dark { 0.27 } else { 0.30 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::capture_browse::table::RowOrder;
    use little_clip_core::engineering::packets::{
        InterfaceId, PacketHeader, Resolution, Timestamp,
    };

    #[test]
    fn packet_order_uses_numeric_keys_and_source_ties_across_pages() {
        let count = super::super::PAGE_ROWS as usize + 2;
        let mut rows = (0..count)
            .map(|row| PacketRow {
                header: PacketHeader {
                    row: row as u64,
                    interface: InterfaceId {
                        section: 0,
                        ordinal: 0,
                    },
                    link_type: 1,
                    time: (row != 0).then_some(Timestamp {
                        ticks: 900,
                        resolution: Resolution::Decimal(6),
                        offset_seconds: 0,
                    }),
                    captured_len: 9,
                    original_len: 9,
                },
                protocol: Default::default(),
            })
            .collect::<Vec<_>>();
        rows[count - 1].header.time.as_mut().unwrap().ticks = 1_100;
        rows[count - 1].header.captured_len = 100;

        let mut order = RowOrder::default();
        order.sort = Some(Sort {
            column: 1,
            descending: true,
        });
        order.rebuild(&rows, compare);
        assert_eq!(order.source_index(0), count - 1);
        assert_eq!(order.source_index(1), 1);
        assert_eq!(order.display_index(0), count - 1);
        order.sort.as_mut().unwrap().column = 5;
        order.rebuild(&rows, compare);
        assert_eq!(order.source_index(0), count - 1);
        assert_eq!(order.source_index(1), 0);

        order.sort = None;
        order.rebuild(&rows, compare);
        assert_eq!(order.source_index(count - 1), count - 1);
    }
}
