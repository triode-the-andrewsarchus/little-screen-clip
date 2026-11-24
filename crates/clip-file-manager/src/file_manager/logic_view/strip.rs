use super::{plot, timing};
use crate::{
    file_manager::{
        DocumentPosition, display_path,
        filesystem::Entry,
        preview_copy,
        shelf::preview::{Data, Ready},
        ui::{ListColumns, information_strip},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Id, Rect, Sense, Ui, vec2};
use little_clip_core::engineering::{
    logic_preview::{Overview, Recording},
    waveforms::display_value,
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) type Preview = Overview;

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "Logic capture".into(),
        crate::icons::Icon::Waves,
    );
    match Recording::open(path, cancel).and_then(|recording| recording.overview(cancel)) {
        Ok(capture) => {
            let samples = if capture.sampled() { " · sampled" } else { "" };
            data.detail = format!(
                "{} · {} ch · {}{samples}",
                capture.label,
                capture.lanes.len(),
                display_value(capture.span.end - capture.span.start, "s"),
            );
            data.logic = Some(Arc::new(capture));
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
    let detail = ready.map_or("Reading logic capture…", |ready| {
        ready.data.detail.as_str()
    });
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(capture) = ready.and_then(|ready| ready.data.logic.as_ref()) else {
        return (None, false);
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, colors.recessed);
    let dark = ui.visuals().dark_mode;
    let rect = plot::draw_overview(&painter, rect.shrink2(vec2(3.0, 1.0)), capture, dark);
    let id = Id::unique(("logic-strip", &entry.path, entry.revision));
    let (response, engaged, selected) = preview_copy::strip_range(ui, rect, id, row_response);
    let hovered = response
        .hover_pos()
        .map(|pos| timing::at(rect, &capture.span, pos.x));
    if !response.dragged() {
        response.on_hover_ui(|ui| {
            ui.horizontal(|ui| {
                ui.strong(capture.label);
                if let Some(time) = hovered {
                    ui.monospace(display_value(time, "s"));
                }
            });
            let channels = capture.lanes_previewed.len();
            for index in 0..channels {
                ui.horizontal(|ui| {
                    let name = &capture.lanes[index].name;
                    let color = plot::preview_color(index, dark);
                    ui.add_sized(
                        [114.0, 20.0],
                        egui::Label::new(
                            egui::RichText::new(name.rsplit('.').next().unwrap_or(name))
                                .color(color),
                        )
                        .truncate(),
                    )
                    .on_hover_text(name);
                    let (rect, _) = ui.allocate_exact_size(vec2(280.0, 18.0), Sense::hover());
                    plot::draw_envelope(
                        &ui.painter_at(rect),
                        rect,
                        &capture.span,
                        &capture.lanes_previewed[index],
                        color,
                    );
                    if let Some(time) = hovered {
                        let x = super::timing::x_at(rect, &capture.span, time);
                        ui.painter_at(rect).vline(
                            x,
                            rect.y_range(),
                            egui::Stroke::new(1.0, colors.text_secondary),
                        );
                    }
                });
            }
            if capture.lanes.len() > channels {
                ui.weak(format!(
                    "Showing {channels} of {} channels",
                    capture.lanes.len()
                ));
            }
            if let Some(note) = &capture.note {
                ui.weak(note);
            }
            ui.weak("Drag to select · Double-click to open the region");
        });
    }
    let position = if let Some((start, end)) = selected {
        let width = capture.span.end - capture.span.start;
        Some((
            capture.span.start + f64::from(start) * width,
            capture.span.start + f64::from(end) * width,
        ))
    } else {
        hovered.map(|time| {
            let half = (capture.span.end - capture.span.start) * 0.1;
            (
                (time - half).max(capture.span.start),
                (time + half).min(capture.span.end),
            )
        })
    }
    .map(|(start, end)| DocumentPosition::LogicRegion {
        start_bits: start.to_bits(),
        end_bits: end.to_bits(),
        revision: entry.revision,
    });
    (position, engaged)
}
