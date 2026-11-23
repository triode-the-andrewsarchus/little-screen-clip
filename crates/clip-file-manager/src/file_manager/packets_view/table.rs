use super::{activity, inspect};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Rect, ScrollArea, Sense, Ui, pos2, vec2};
use little_clip_core::engineering::packets::PacketRow;
use std::ops::Range;

const HEIGHT: f32 = 21.0;
const COLUMN_FRACTIONS: [f32; 6] = [0.10, 0.17, 0.13, 0.22, 0.23, 0.15];
const NAMES: [&str; 6] = ["No.", "Time", "Protocol", "Source", "Destination", "Bytes"];

pub(super) struct Request {
    pub page: u64,
    pub count: usize,
    pub origin: Option<i128>,
    pub selected: Option<u64>,
    pub range: Option<Range<u64>>,
    pub scroll: Option<usize>,
    pub generation: u64,
    pub height: f32,
}

pub(super) struct Response {
    pub visible: Range<u64>,
    pub clicked: Option<(u64, bool)>,
}

pub(super) fn show<'a>(
    ui: &mut Ui,
    request: Request,
    colors: WorkbenchColors,
    row: impl Fn(u64) -> Option<&'a PacketRow>,
) -> Response {
    let header = ui
        .allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover())
        .0;
    properties(ui, header, NAMES, colors.text_muted);
    let mut area = ScrollArea::vertical()
        .id_salt(("packet rows", request.page, request.generation))
        .auto_shrink([false, false])
        .max_height((request.height - HEIGHT).max(50.0));
    if let Some(row) = request.scroll {
        area = area.vertical_scroll_offset(row as f32 * (HEIGHT + ui.spacing().item_spacing.y));
    }
    let mut visible = request.page..request.page;
    let mut clicked = None;
    area.show_rows(ui, HEIGHT, request.count, |ui, range| {
        visible = request.page + range.start as u64..request.page + range.end as u64;
        for index in visible.clone() {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::click());
            let Some(packet) = row(index) else {
                let id = format!("{}", index + 1);
                properties(ui, rect, [&id, "…", "", "", "", ""], colors.text_muted);
                continue;
            };
            let ordinal = packet.header.row;
            let active = request.selected == Some(ordinal);
            let ranged = request
                .range
                .as_ref()
                .is_some_and(|range| range.contains(&ordinal));
            if active || ranged || response.hovered() {
                let factor = if active { 0.48 } else { 0.20 };
                ui.painter()
                    .rect_filled(rect, 2.0, colors.focus.gamma_multiply(factor));
            }
            if response.clicked() {
                clicked = Some((ordinal, ui.input(|input| input.modifiers.shift)));
            }
            row_contents(ui, rect, packet, request.origin, colors);
        }
    });
    Response { visible, clicked }
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
    properties(
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

fn properties(ui: &Ui, rect: Rect, values: [&str; 6], color: egui::Color32) {
    let mut left = rect.left();
    for (value, fraction) in values.into_iter().zip(COLUMN_FRACTIONS) {
        let next = left + fraction * rect.width();
        let cell = Rect::from_min_max(pos2(left, rect.top()), pos2(next, rect.bottom()));
        ui.painter().with_clip_rect(cell).text(
            pos2(left + 3.0, rect.center().y),
            Align2::LEFT_CENTER,
            value,
            FontId::monospace(10.5),
            color,
        );
        left = next;
    }
}
