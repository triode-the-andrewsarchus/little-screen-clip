use super::{activity, direction, hex, kind};
use crate::{
    file_manager::capture_browse::table::{Columns, Page, Response},
    ui_appearance::WorkbenchColors,
};
use egui::{Rect, ScrollArea, Ui};
use little_clip_core::engineering::can::{Frame, FrameKind};

const COLUMNS: Columns<7> = Columns {
    id: "can-frames",
    headings: ["Frame", "Time", "Bus", "ID", "Type", "Bytes", "Data"],
    fractions: [0.09, 0.15, 0.13, 0.14, 0.15, 0.07, 0.27],
    min_height: 48.0,
    selected_fill: 0.45,
    hover_fill: 0.19,
};

pub(super) fn show<'a>(
    ui: &mut Ui,
    page: Page<'_>,
    origin: i128,
    colors: WorkbenchColors,
    frame: impl Fn(u64) -> Option<&'a Frame>,
) -> Response {
    COLUMNS.show(
        ui,
        page,
        colors,
        frame,
        |frame| frame.row,
        |ui, rect, frame| {
            contents(ui, rect, frame, origin, colors);
        },
    )
}

fn contents(ui: &Ui, rect: Rect, frame: &Frame, origin: i128, colors: WorkbenchColors) {
    let row = (frame.row + 1).to_string();
    let time = activity::time_label(frame.timestamp_ns - origin);
    let bus = frame.channel.to_string();
    let id = frame.id.map_or_else(|| "—".into(), |id| id.to_string());
    let mut variation = kind(frame.kind).to_owned();
    let direction = direction(frame.direction);
    if !direction.is_empty() {
        variation.push(' ');
        variation.push_str(direction);
    }
    let bytes = frame.data_len.to_string();
    let shown = frame.data.len().min(12);
    let mut data = hex(&frame.data[..shown]);
    if frame.data.len() > shown {
        data.push_str(" …");
    }
    let color = if frame.kind == FrameKind::Error {
        colors.warning
    } else {
        colors.text_primary
    };
    COLUMNS.cells(
        ui,
        rect,
        [&row, &time, &bus, &id, &variation, &bytes, &data],
        color,
    );
}

pub(super) fn details(ui: &mut Ui, frame: &Frame, origin: i128, height: f32) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("Frame {}", frame.row + 1));
        ui.monospace(activity::time_label(frame.timestamp_ns - origin))
            .on_hover_text(format!("Source timestamp {} ns", frame.timestamp_ns));
        ui.label(frame.channel.to_string());
        if let Some(id) = frame.id {
            ui.monospace(id.to_string()).on_hover_text(if id.extended {
                "29-bit ID"
            } else {
                "11-bit ID"
            });
        }
        ui.label(kind(frame.kind));
        if !direction(frame.direction).is_empty() {
            ui.label(direction(frame.direction));
        }
        if let Some(dlc) = frame.dlc {
            ui.weak(format!("DLC {dlc}"));
        }
        if !frame.data.is_empty() && ui.button("Copy payload").clicked() {
            ui.ctx().copy_text(hex(&frame.data));
        }
    });
    if let FrameKind::Flexible {
        bitrate_switch,
        error_state,
    } = frame.kind
    {
        let flag = |value| if value { "on" } else { "off" };
        ui.weak(format!(
            "BRS {} · ESI {}",
            flag(bitrate_switch),
            flag(error_state)
        ));
    }
    if frame.data.len() != usize::from(frame.data_len) {
        ui.weak(format!(
            "{} captured of {} declared bytes",
            frame.data.len(),
            frame.data_len
        ));
    }
    ScrollArea::vertical()
        .id_salt(("can-frame-payload", frame.row))
        .max_height(height.max(24.0))
        .show(ui, |ui| {
            if frame.data.is_empty() {
                ui.weak("No payload bytes");
            }
            for (row, bytes) in frame.data.chunks(16).enumerate() {
                let bytes_hex = hex(bytes);
                let ascii: String = bytes
                    .iter()
                    .map(|byte| {
                        if byte.is_ascii_graphic() {
                            char::from(*byte)
                        } else {
                            '·'
                        }
                    })
                    .collect();
                ui.monospace(format!("{:04X}  {:47}  {ascii}", row * 16, bytes_hex));
            }
        });
}
