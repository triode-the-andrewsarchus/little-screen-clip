use super::{activity, inspect};
use crate::{
    file_manager::capture_browse::table::{Columns, Page, Response},
    ui_appearance::WorkbenchColors,
};
use egui::{Color32, Rect, Ui, vec2};
use little_clip_core::engineering::packets::{PacketRow, Protocol};

const COLUMNS: Columns<6> = Columns {
    id: "packet rows",
    headings: ["No.", "Time", "Protocol", "Source", "Destination", "Bytes"],
    fractions: [0.10, 0.17, 0.13, 0.22, 0.23, 0.15],
    min_height: 50.0,
    selected_fill: 0.48,
    hover_fill: 0.20,
};

pub(super) fn show<'a>(
    ui: &mut Ui,
    page: Page<'_>,
    origin: Option<i128>,
    colors: WorkbenchColors,
    colorize: bool,
    row: impl Fn(u64) -> Option<&'a PacketRow>,
) -> Response {
    COLUMNS.show(
        ui,
        page,
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
