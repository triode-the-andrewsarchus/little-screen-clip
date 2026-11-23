use super::{
    activity,
    capture::{self, Capture},
    inspect,
};
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
use little_clip_core::engineering::packets::{ActivityBin, Summary};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    summary: Summary,
    activity: Vec<ActivityBin>,
}

impl Preview {
    pub(in crate::file_manager) fn summary(&self) -> &Summary {
        &self.summary
    }
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "Packet capture".into(),
        crate::icons::Icon::Crosshair,
    );
    let summary = match Capture::open(path, cancel) {
        Ok((_, summary)) => summary,
        Err(error) => {
            data.detail = error;
            return data;
        }
    };
    data.detail = format!(
        "{} · {}{} packets · {} interfaces",
        capture::name(summary.format),
        summary.packets.len(),
        if summary.complete { "" } else { "+" },
        summary.interfaces.len(),
    );
    let activity = summary.activity();
    data.packets = Some(Arc::new(Preview { summary, activity }));
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
    let detail = ready.map_or("Reading packet activity…", |ready| {
        ready.data.detail.as_str()
    });
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(preview) = ready.and_then(|ready| ready.data.packets.as_ref()) else {
        return (None, false);
    };
    let response = ui.interact(
        rect,
        Id::unique(("capture activity strip", &entry.path, entry.revision)),
        Sense::click(),
    );
    *row_response = row_response.clone().union(response.clone());
    let time = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
        .zip(activity::bounds(&preview.activity))
        .map(|(pos, bounds)| activity::at(rect, bounds, pos.x));
    let packet = time.and_then(|time| activity::nearest(&preview.activity, time));
    activity::with_mark(
        &ui.painter_at(rect),
        rect,
        &preview.activity,
        packet.map(|bin| bin.start_ns),
        colors,
    );
    response.on_hover_ui(|ui| preview.tooltip(ui, colors, packet));
    let position = packet.map(|bin| DocumentPosition::CapturedPacket {
        ordinal: bin.first_row,
        revision: entry.revision,
    });
    (position, false)
}

impl Preview {
    fn tooltip(&self, ui: &mut Ui, colors: WorkbenchColors, bin: Option<ActivityBin>) {
        ui.set_width(388.0);
        ui.horizontal_wrapped(|ui| {
            ui.strong(capture::name(self.summary.format));
            ui.weak(format!("{} interfaces", self.summary.interfaces.len()));
            if !self.summary.complete {
                ui.weak("Start of capture");
            }
        });
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
        activity::with_mark(
            &ui.painter_at(rect),
            rect,
            &self.activity,
            bin.map(|bin| bin.start_ns),
            colors,
        );
        let first = bin.map_or(0, |bin| bin.first_row as usize);
        for row in self.summary.packets.iter().skip(first).take(4) {
            let fields = row.protocol;
            ui.horizontal_wrapped(|ui| {
                ui.monospace(format!("#{}", row.header.row + 1));
                ui.strong(fields.protocol.label());
                ui.monospace(format!(
                    "{} → {}",
                    inspect::endpoint(fields.source, fields.source_port),
                    inspect::endpoint(fields.destination, fields.destination_port)
                ));
            });
        }
        ui.weak("Double-click to inspect the packet");
    }
}
