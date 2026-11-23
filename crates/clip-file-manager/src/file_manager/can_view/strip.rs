use super::{activity, capture::Capture, hex};
use crate::{
    file_manager::{
        DocumentPosition, display_path,
        filesystem::Entry,
        shelf::preview::{Data, Ready},
        ui::{ListColumns, information_strip},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Id, Rect, Sense, Ui, vec2};
use little_clip_core::engineering::can::Summary;
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    pub(in crate::file_manager) summary: Summary,
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "CAN capture".into(),
        crate::icons::Icon::Crosshair,
    );
    match Capture::open(path, cancel) {
        Ok((_, summary)) => {
            let span = activity::domain(&summary);
            let count = if summary.complete {
                "frames"
            } else {
                "initial frames"
            };
            data.detail = format!(
                "{} · {} {count} · {} buses · {}",
                summary.format.label(),
                summary.frames,
                summary.channels.len(),
                activity::time_label(span.end - span.start - 1),
            );
            data.can_capture = Some(Arc::new(Preview { summary }));
        }
        Err(error) => data.detail = error,
    }
    data
}

pub(in crate::file_manager) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: ListColumns,
    colors: WorkbenchColors,
    ready: Option<&Ready>,
    row_response: &mut egui::Response,
) -> (Option<DocumentPosition>, bool) {
    let detail = ready.map_or("Reading CAN activity…", |ready| {
        ready.data.detail.as_str()
    });
    let strip = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(preview) = ready.and_then(|ready| ready.data.can_capture.as_deref()) else {
        return (None, false);
    };
    let summary = &preview.summary;
    let span = activity::domain(summary);
    let painter = ui.painter_at(strip);
    activity::draw(&painter, strip, &span, &summary.activity, colors);

    let response = ui.interact(
        strip,
        Id::unique(("can-trace-strip", &entry.path, entry.revision)),
        Sense::click(),
    );
    *row_response = row_response.clone().union(response.clone());
    let hovered = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
        .map(|point| activity::at(strip, &span, point.x));
    let bin = hovered.and_then(|time| activity::nearest(&summary.activity, time));
    if let Some(bin) = bin {
        activity::highlight(&painter, strip, &span, &bin.time_ns, colors);
    }
    response.on_hover_ui(|ui| preview.expanded(ui, colors, hovered));
    (
        bin.map(|bin| DocumentPosition::CanFrame {
            ordinal: bin.first_row,
            revision: entry.revision,
        }),
        false,
    )
}

impl Preview {
    fn expanded(&self, ui: &mut Ui, colors: WorkbenchColors, hovered: Option<i128>) {
        let summary = &self.summary;
        let span = activity::domain(summary);
        ui.set_width(340.0);
        ui.strong(summary.format.label());
        if !summary.complete {
            ui.weak(format!("First {} frames", summary.frames));
        }
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let painter = ui.painter_at(rect);
        activity::draw(&painter, rect, &span, &summary.activity, colors);
        if let Some(bin) = hovered.and_then(|time| activity::nearest(&summary.activity, time)) {
            activity::highlight(&painter, rect, &span, &bin.time_ns, colors);
            ui.label(format!(
                "{} · {} frames · {} errors",
                activity::time_label(bin.time_ns.start.max(span.start) - span.start),
                bin.frames,
                bin.error_frames,
            ));
            if let Some(frame) = summary.sample.get(bin.first_row as usize)
                && let Some(id) = frame.id
            {
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(format!("{}  {}", frame.channel, id));
                    ui.monospace(hex(&frame.data[..frame.data.len().min(16)]));
                });
            }
        }
        for item in summary.channels.iter().take(4) {
            ui.horizontal(|ui| {
                ui.strong(item.channel.to_string());
                ui.weak(format!("{} frames", item.frames));
                if item.error_frames > 0 {
                    ui.colored_label(colors.warning, format!("{} errors", item.error_frames));
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            for item in summary.identifiers.iter().take(4) {
                ui.monospace(format!("{}  ×{}", item.id, item.frames));
            }
        });
    }
}
