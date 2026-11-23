use super::activity;
use egui::{Align2, FontId, Rect, ScrollArea, Sense, Ui, pos2, vec2};
use little_clip_core::engineering::packets::{
    Endpoint, Interface, Layer, Packet, Resolution, Timestamp,
};
use std::ops::Range;

#[derive(Default)]
pub(super) struct Inspector {
    row: Option<u64>,
    layers: Vec<Layer>,
    selection: Option<Range<usize>>,
    reveal_byte: bool,
    reveal_field: bool,
}

impl Inspector {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        packet: &Packet,
        interface: Option<&Interface>,
        origin: Option<i128>,
    ) -> Option<String> {
        if self.row != Some(packet.header.row) {
            self.row = Some(packet.header.row);
            self.layers = packet.layers();
            self.selection = None;
        }
        metadata(ui, packet, interface, origin);
        self.diagram(ui);
        let filter = self.tree(ui);
        self.bytes(ui, packet);
        filter
    }

    fn select(&mut self, bytes: Range<usize>, from_hex: bool) {
        self.selection = Some(bytes);
        self.reveal_byte = !from_hex;
        self.reveal_field = from_hex;
    }

    fn diagram(&mut self, ui: &mut Ui) {
        if self.layers.is_empty() {
            return;
        }
        ui.separator();
        ui.strong("Packet diagram");
        let available = ui.available_width().max(32.0);
        let minimum = (available / self.layers.len() as f32 / 2.0).min(56.0);
        let used: usize = self.layers.iter().map(|layer| layer.bytes.len()).sum();
        let extra = (available - self.layers.len() as f32 * minimum).max(0.0);
        let (plot, _) = ui.allocate_exact_size(vec2(available, 28.0), Sense::hover());
        let mut left = plot.left();
        let mut target = None;
        for (index, layer) in self.layers.iter().enumerate() {
            let width = minimum + extra * layer.bytes.len() as f32 / used.max(1) as f32;
            let rect =
                Rect::from_min_size(pos2(left, plot.top()), vec2(width, plot.height())).shrink(1.0);
            let selected = self
                .selection
                .as_ref()
                .is_some_and(|bytes| overlaps(&layer.bytes, bytes));
            let response = ui.interact(
                rect,
                ui.scope_id().with(("protocol diagram", index)),
                Sense::click(),
            );
            let color = if selected || response.hovered() {
                ui.visuals().selection.bg_fill
            } else {
                super::table::layer_color(layer.name, ui.visuals().dark_mode)
            };
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 3.0, color);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                layer.name,
                FontId::proportional(11.0),
                if selected || response.hovered() {
                    ui.visuals().selection.stroke.color
                } else {
                    ui.visuals().text_color()
                },
            );
            if response.clicked() {
                target = Some(layer.bytes.clone());
            }
            response.on_hover_text(format!(
                "{} · {} bytes · offset {}",
                layer.name,
                layer.bytes.len(),
                layer.bytes.start
            ));
            left += width;
        }
        if let Some(target) = target {
            self.select(target, false);
        }
    }

    fn tree(&mut self, ui: &mut Ui) -> Option<String> {
        let mut filter = None;
        let mut selection = None;
        let height = (ui.available_height() * 0.53).max(52.0);
        ScrollArea::vertical()
            .id_salt("captured protocol tree")
            .max_height(height)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (index, layer) in self.layers.iter().enumerate() {
                    if layer.fields.is_empty() {
                        if ui
                            .selectable_label(
                                self.selection.as_ref() == Some(&layer.bytes),
                                format!("{} · {} bytes", layer.name, layer.bytes.len()),
                            )
                            .clicked()
                        {
                            selection = Some(layer.bytes.clone());
                        }
                        continue;
                    }
                    let expanded = self
                        .selection
                        .as_ref()
                        .is_some_and(|bytes| overlaps(bytes, &layer.bytes));
                    let mut header = egui::CollapsingHeader::new(format!(
                        "{} · {} bytes",
                        layer.name,
                        layer.bytes.len()
                    ))
                    .id_salt(("protocol", index, layer.name))
                    .default_open(index < 3);
                    if expanded && self.reveal_field {
                        header = header.open(Some(true));
                    }
                    header.show(ui, |ui| {
                        for field in &layer.fields {
                            let selected = self
                                .selection
                                .as_ref()
                                .is_some_and(|bytes| overlaps(bytes, &field.bytes));
                            let response = ui
                                .selectable_label(
                                    selected,
                                    format!("{}   {}", field.label, field.value),
                                )
                                .on_hover_text(format!(
                                    "Captured bytes {}–{}",
                                    field.bytes.start,
                                    field.bytes.end - 1
                                ));
                            if selected && self.reveal_field {
                                response.scroll_to_me(None);
                                self.reveal_field = false;
                            }
                            if response.clicked() {
                                selection = Some(field.bytes.clone());
                            }
                            if let Some(expression) = &field.filter {
                                response.context_menu(|ui| {
                                    if ui.button("Apply as display filter").clicked() {
                                        filter = Some(expression.clone());
                                        ui.close();
                                    }
                                    if ui.button("Copy display filter").clicked() {
                                        ui.ctx().copy_text(expression.clone());
                                        ui.close();
                                    }
                                });
                            }
                        }
                    });
                }
            });
        if let Some(selection) = selection {
            self.select(selection, false);
        }
        filter
    }

    fn bytes(&mut self, ui: &mut Ui, packet: &Packet) {
        ui.separator();
        ui.strong("Captured bytes");
        let columns = if ui.available_width() > 545.0 { 16 } else { 8 };
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        ui.spacing_mut().item_spacing = vec2(2.0, 1.0);
        let mut area = ScrollArea::both()
            .id_salt(("packet bytes", packet.header.row))
            .auto_shrink([false, false])
            .max_height(ui.available_height().max(36.0));
        if std::mem::take(&mut self.reveal_byte)
            && let Some(selection) = &self.selection
        {
            let row = selection.start / columns;
            area = area.vertical_scroll_offset(row.saturating_sub(1) as f32 * (row_height + 1.0));
        }
        let mut selected = None;
        area.show_rows(
            ui,
            row_height,
            packet.bytes.len().div_ceil(columns),
            |ui, range| {
                for line in range {
                    let start = line * columns;
                    let end = (start + columns).min(packet.bytes.len());
                    ui.horizontal(|ui| {
                        ui.weak(egui::RichText::new(format!("{start:04x}")).monospace());
                        for offset in start..end {
                            let active = self
                                .selection
                                .as_ref()
                                .is_some_and(|range| range.contains(&offset));
                            let text = egui::RichText::new(format!("{:02x}", packet.bytes[offset]))
                                .monospace();
                            if ui
                                .selectable_label(active, text)
                                .on_hover_text(format!("Byte {offset}"))
                                .clicked()
                            {
                                selected = Some(offset..offset + 1);
                            }
                        }
                        let ascii = packet.bytes[start..end]
                            .iter()
                            .map(|&byte| {
                                if byte.is_ascii_graphic() || byte == b' ' {
                                    char::from(byte)
                                } else {
                                    '·'
                                }
                            })
                            .collect::<String>();
                        ui.add_space(7.0);
                        ui.monospace(ascii);
                    });
                }
            },
        );
        if let Some(selected) = selected {
            self.select(selected, true);
        }
    }
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn metadata(ui: &mut Ui, packet: &Packet, interface: Option<&Interface>, origin: Option<i128>) {
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

    if fields.fragmented {
        ui.weak("Fragment");
    }
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
