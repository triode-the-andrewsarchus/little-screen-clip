use super::{activity, direction, hex, kind};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, FontId, Rect, ScrollArea, Sense, Ui, pos2, vec2};
use little_clip_core::engineering::can::{Frame, FrameKind};
use std::ops::Range;

const ROW_HEIGHT: f32 = 21.0;
const HEADINGS: [&str; 7] = ["Frame", "Time", "Bus", "ID", "Type", "Bytes", "Data"];
const FRACTIONS: [f32; 7] = [0.09, 0.15, 0.13, 0.14, 0.15, 0.07, 0.27];

pub(super) struct Page<'a> {
    pub offset: u64,
    pub count: usize,
    pub origin: i128,
    pub selected: Option<u64>,
    pub selection: Option<&'a Range<u64>>,
    pub scroll_to: Option<usize>,
    pub generation: u64,
    pub height: f32,
}

pub(super) struct Response {
    pub visible: Range<u64>,
    pub clicked: Option<(u64, bool)>,
}

pub(super) fn show<'a>(
    ui: &mut Ui,
    page: Page<'_>,
    colors: WorkbenchColors,
    frame: impl Fn(u64) -> Option<&'a Frame>,
) -> Response {
    let header = ui
        .allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover())
        .0;
    cells(ui, header, HEADINGS, colors.text_muted);
    let mut area = ScrollArea::vertical()
        .id_salt(("can-frames", page.offset, page.generation))
        .auto_shrink([false, false])
        .max_height((page.height - ROW_HEIGHT).max(48.0));
    if let Some(row) = page.scroll_to {
        area = area.vertical_scroll_offset(row as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y));
    }
    let mut output = Response {
        visible: page.offset..page.offset,
        clicked: None,
    };
    area.show_rows(ui, ROW_HEIGHT, page.count, |ui, rows| {
        output.visible = page.offset + rows.start as u64..page.offset + rows.end as u64;
        for number in output.visible.clone() {
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let Some(frame) = frame(number) else {
                let row = format!("{}", number + 1);
                cells(ui, rect, [&row, "…", "", "", "", "", ""], colors.text_muted);
                continue;
            };
            let selected = page.selected == Some(frame.row);
            let ranged = page
                .selection
                .is_some_and(|range| range.contains(&frame.row));
            if selected || ranged || response.hovered() {
                let factor = if selected { 0.45 } else { 0.19 };
                ui.painter()
                    .rect_filled(rect, 2.0, colors.focus.gamma_multiply(factor));
            }
            if response.clicked() {
                output.clicked = Some((frame.row, ui.input(|input| input.modifiers.shift)));
            }
            contents(ui, rect, frame, page.origin, colors);
        }
    });
    output
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
    cells(
        ui,
        rect,
        [&row, &time, &bus, &id, &variation, &bytes, &data],
        color,
    );
}

fn cells(ui: &Ui, rect: Rect, values: [&str; 7], color: egui::Color32) {
    let mut left = rect.left();
    for (value, fraction) in values.into_iter().zip(FRACTIONS) {
        let right = left + fraction * rect.width();
        let cell = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
        ui.painter().with_clip_rect(cell).text(
            pos2(left + 3.0, rect.center().y),
            Align2::LEFT_CENTER,
            value,
            FontId::monospace(10.5),
            color,
        );
        left = right;
    }
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
