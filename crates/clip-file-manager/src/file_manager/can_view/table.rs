use super::{activity, direction, hex, kind};
use crate::{
    file_manager::capture_browse::table::{Columns, Page, Response, Sort, TableOptions},
    ui_appearance::WorkbenchColors,
};
use egui::{Rect, ScrollArea, Ui};
use little_clip_core::engineering::can::{Channel, Frame, FrameKind};
use std::cmp::Ordering;

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
    view: (Page<'_>, TableOptions),
    origin: i128,
    colors: WorkbenchColors,
    frame: impl Fn(u64) -> Option<&'a Frame>,
) -> Response {
    COLUMNS.show_with_options(
        ui,
        view,
        colors,
        frame,
        |frame| frame.row,
        |ui, rect, frame| {
            contents(ui, rect, frame, origin, colors);
        },
    )
}

pub(super) fn compare(sort: Sort, left: &Frame, right: &Frame) -> Ordering {
    fn channel(frame: &Frame) -> Option<&Channel> {
        (frame.channel != Channel::Unknown).then_some(&frame.channel)
    }
    match sort.column {
        0 => sort.compare(left.row, right.row),
        1 => sort.compare(left.timestamp_ns, right.timestamp_ns),
        2 => sort.known_first(channel(left), channel(right)),
        3 => sort.known_first(left.id, right.id),
        4 => sort.compare(
            (kind(left.kind), direction(left.direction)),
            (kind(right.kind), direction(right.direction)),
        ),
        5 => sort.compare(left.data_len, right.data_len),
        _ => Ordering::Equal,
    }
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

pub(super) fn details(
    ui: &mut Ui,
    frame: &Frame,
    origin: i128,
    height: f32,
    field: Option<&little_clip_core::engineering::can::dbc::Field>,
) {
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
                if let Some(field) = field {
                    highlight_payload(ui, row * 16, bytes, &ascii, field);
                } else {
                    ui.monospace(format!("{:04X}  {:47}  {ascii}", row * 16, hex(bytes)));
                }
            }
        });
}

fn highlight_payload(
    ui: &mut Ui,
    start: usize,
    bytes: &[u8],
    ascii: &str,
    field: &little_clip_core::engineering::can::dbc::Field,
) {
    use egui::text::{LayoutJob, TextFormat};
    let mut layout = LayoutJob::default();
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let normal = TextFormat::simple(font_id, ui.visuals().text_color());
    let mut marked = normal.clone();
    marked.background = ui.visuals().selection.bg_fill.gamma_multiply(0.55);
    let mut active = [0u8; 16];
    for &bit in field.bits() {
        let byte = usize::from(bit / 8);
        if (start..start + 16).contains(&byte) {
            active[byte - start] |= 1 << (bit % 8);
        }
    }
    layout.append(&format!("{start:04X}  "), 0.0, normal.clone());
    for (&byte, &mask) in bytes.iter().zip(&active) {
        let style = if mask > 0 { &marked } else { &normal };
        layout.append(&format!("{byte:02X}"), 0.0, style.clone());
        layout.append(" ", 0.0, normal.clone());
    }
    let padding = " ".repeat((16 - bytes.len()) * 3 + 1);
    layout.append(&format!("{padding}{ascii}"), 0.0, normal);
    let mask = active[..bytes.len()]
        .iter()
        .map(|mask| format!("{mask:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    ui.label(layout)
        .on_hover_text(format!("{} · bit mask {mask}", field.name));
}
