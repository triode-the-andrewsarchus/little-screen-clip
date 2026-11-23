use super::activity;
use egui::{ScrollArea, Ui};
use little_clip_core::engineering::packets::{Endpoint, Interface, Packet, Resolution, Timestamp};

pub(super) fn show(
    ui: &mut Ui,
    packet: &Packet,
    interface: Option<&Interface>,
    origin: Option<i128>,
) {
    let header = packet.header;
    let fields = packet.protocol();
    ui.strong(format!("Packet {}", header.row + 1));
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{} bytes captured", header.captured_len));
        if header.captured_len != header.original_len {
            ui.weak(format!("of {} bytes", header.original_len));
        }
    });
    if let Some(time) = header.time {
        let resolution = match time.resolution {
            Resolution::Decimal(power) => format!("10^-{power} s"),
            Resolution::Binary(power) => format!("2^-{power} s"),
        };
        ui.monospace(absolute(time)).on_hover_text(format!(
            "{} ticks · {resolution} per tick · offset {} s",
            time.ticks, time.offset_seconds,
        ));
        if let Some(origin) = origin {
            ui.weak(activity::relative(time.unix_nanoseconds() - origin));
        }
    } else {
        ui.weak("No timestamp recorded");
    }
    let name = interface
        .and_then(|interface| interface.name.as_deref())
        .filter(|name| !name.is_empty());
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "Interface {}.{}",
            header.interface.section + 1,
            header.interface.ordinal
        ));
        if let Some(name) = name {
            ui.label(egui::RichText::new(name).strong());
        }
        ui.weak(link_name(header.link_type).map_or_else(
            || format!("Link type {}", header.link_type),
            ToOwned::to_owned,
        ));
    });

    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.strong(fields.protocol.label());
        if let Some(vlan) = fields.vlan {
            ui.weak(format!("VLAN {vlan}"));
        }
        if fields.fragmented {
            ui.weak("Fragment");
        }
        if let Some(flags) = fields.tcp_flags {
            ui.weak(format!("TCP flags 0x{flags:03x}"));
        }
        if let Some(kind) = fields.icmp_type {
            ui.weak(format!("ICMP type {kind}"));
        }
    });
    if fields.source.is_some() || fields.destination.is_some() {
        ui.monospace(format!(
            "From {}",
            endpoint(fields.source, fields.source_port)
        ));
        ui.monospace(format!(
            "To   {}",
            endpoint(fields.destination, fields.destination_port)
        ));
    }
    ui.separator();
    bytes(ui, packet);
}

fn absolute(time: Timestamp) -> String {
    let ns = time.unix_nanoseconds();
    let seconds = i64::try_from(ns.div_euclid(1_000_000_000));
    let nanoseconds = ns.rem_euclid(1_000_000_000) as u32;
    seconds
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, nanoseconds))
        .map_or_else(
            || format!("{ns} ns since 1970-01-01 UTC"),
            |time| time.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        )
}

fn link_name(link: u16) -> Option<&'static str> {
    match link {
        0 | 108 => Some("Loopback"),
        1 => Some("Ethernet"),
        12 | 101 => Some("Raw IP"),
        105 => Some("IEEE 802.11"),
        113 => Some("Linux cooked"),
        127 => Some("Radiotap"),
        147..=162 => Some("User-defined"),
        227 => Some("CAN SocketCAN"),
        228 => Some("Raw IPv4"),
        229 => Some("Raw IPv6"),
        276 => Some("Linux cooked v2"),
        _ => None,
    }
}

pub(super) fn endpoint(endpoint: Option<Endpoint>, port: Option<u16>) -> String {
    match (endpoint, port) {
        (Some(endpoint @ Endpoint::V6(_)), Some(port)) => format!("[{endpoint}]:{port}"),
        (Some(endpoint), Some(port)) => format!("{endpoint}:{port}"),
        (Some(endpoint), None) => endpoint.to_string(),
        (None, Some(port)) => format!(":{port}"),
        (None, None) => "—".to_owned(),
    }
}

fn bytes(ui: &mut Ui, packet: &Packet) {
    use std::fmt::Write as _;
    ui.strong("Captured bytes");
    let columns = if ui.available_width() > 490.0 { 16 } else { 8 };
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    ScrollArea::both()
        .id_salt(("packet bytes", packet.header.row))
        .max_height(ui.available_height().max(40.0))
        .show_rows(
            ui,
            row_height,
            packet.bytes.len().div_ceil(columns),
            |ui, range| {
                for line in range {
                    let start = line * columns;
                    let end = (start + columns).min(packet.bytes.len());
                    let bytes = &packet.bytes[start..end];
                    let mut rendered = format!("{start:06x}  ");
                    for &byte in bytes {
                        let _ = write!(rendered, "{byte:02x} ");
                    }
                    rendered.extend(std::iter::repeat_n(' ', 3 * (columns - bytes.len()) + 1));
                    rendered.extend(bytes.iter().map(|byte| {
                        if byte.is_ascii_graphic() || *byte == b' ' {
                            char::from(*byte)
                        } else {
                            '·'
                        }
                    }));
                    ui.add(
                        egui::Label::new(egui::RichText::new(rendered).monospace())
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                }
            },
        );
}
