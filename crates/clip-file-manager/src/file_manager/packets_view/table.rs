use super::{activity, inspect};
use crate::{
    file_manager::capture_browse::table::{Columns, Page, Response},
    ui_appearance::WorkbenchColors,
};
use egui::{Rect, Ui};
use little_clip_core::engineering::packets::PacketRow;

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
    row: impl Fn(u64) -> Option<&'a PacketRow>,
) -> Response {
    COLUMNS.show(
        ui,
        page,
        colors,
        row,
        |packet| packet.header.row,
        |ui, rect, packet| {
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
