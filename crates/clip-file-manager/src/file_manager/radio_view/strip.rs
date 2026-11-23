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
use little_clip_core::engineering::radio::{self, BANDS, Summary};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) type Preview = Summary;

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "Radio programming".into(),
        crate::icons::Icon::Waves,
    );
    match radio::preview(path, cancel) {
        Ok(plan) => {
            let model = plan.model.as_deref().unwrap_or(plan.format.title());
            data.detail = if plan.count > 0 {
                format!("{model} · {} channels", plan.count)
            } else {
                format!("{model} · {} clone records", plan.records)
            };
            data.radio = Some(Arc::new(plan));
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
    response: &mut egui::Response,
) -> (Option<DocumentPosition>, bool) {
    let detail = ready.map_or("Reading radio memories…", |ready| {
        ready.data.detail.as_str()
    });
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(plan) = ready.and_then(|ready| ready.data.radio.as_ref()) else {
        return (None, false);
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, colors.recessed);
    let count = plan.channels.len().min(4);
    let width = rect.width() / count.max(1) as f32;
    for (index, channel) in plan.channels.iter().take(count).enumerate() {
        let bounds = Rect::from_min_size(
            rect.min + vec2(index as f32 * width, 0.0),
            vec2(width, rect.height()),
        );
        let painter = painter.with_clip_rect(bounds);
        let label = format!(
            "{}  {}",
            channel.name,
            radio::frequency_label(channel.receive_hz)
        );
        painter.text(
            bounds.left_center() + vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.0),
            ui.visuals().text_color(),
        );
    }
    if count == 0
        && let Some([start, end]) = plan.address_span
    {
        painter.text(
            rect.left_center() + vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("0x{start:X} – 0x{end:X}"),
            egui::FontId::monospace(11.0),
            ui.visuals().text_color(),
        );
    }
    let hit = ui.interact(
        rect,
        Id::unique(("radio-strip", &entry.path, entry.revision)),
        Sense::click(),
    );
    *response = response.clone().union(hit.clone());
    let ordinal = hit
        .hover_pos()
        .map(|pos| (((pos.x - rect.left()) / width) as usize).min(count.saturating_sub(1)));
    hit.on_hover_ui(|ui| {
        ui.strong(plan.model.as_deref().unwrap_or(plan.format.title()));
        ui.horizontal_wrapped(|ui| {
            for (name, count) in BANDS
                .into_iter()
                .zip(plan.band_counts)
                .filter(|(_, count)| *count > 0)
            {
                ui.label(format!("{name} {count}"));
            }
        });
        if let Some(channel) = ordinal.and_then(|index| plan.channels.get(index)) {
            ui.label(format!(
                "{}  {} · {} · {}",
                channel.number,
                channel.name,
                radio::frequency_label(channel.receive_hz),
                channel.mode
            ));
        }
        ui.weak("Double-click to inspect");
    });
    (
        ordinal.map(|ordinal| DocumentPosition::RadioChannel {
            ordinal,
            revision: entry.revision,
        }),
        false,
    )
}
