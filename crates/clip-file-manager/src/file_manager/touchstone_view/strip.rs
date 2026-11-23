use super::{
    capture::{Band, Capture, Trace, trace_points},
    plot::{self, Projection},
};
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
    touchstone::{Metadata, Parameter, TracePoint},
    waveforms::display_value,
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    metadata: Metadata,
    traces: Vec<(String, Vec<TracePoint>)>,
    band: Band,
    complete: bool,
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "RF measurements".into(),
        crate::icons::Icon::Crosshair,
    );
    let summary = match Capture::open(path, 512, cancel) {
        Ok((_, summary)) => summary,
        Err(error) => {
            data.detail = error;
            return data;
        }
    };
    let mut traces = vec![named_trace(&summary.metadata, &summary.points, [0, 0])];
    if summary.metadata.ports >= 2 {
        traces.push(named_trace(&summary.metadata, &summary.points, [1, 0]));
    }
    let band = plot::domain(&traces[0].1);
    data.detail = format!(
        "{} ports · {} – {}{}",
        summary.metadata.ports,
        display_value(band[0], "Hz"),
        display_value(band[1], "Hz"),
        if summary.complete { "" } else { " · prefix" },
    );
    data.touchstone = Some(Arc::new(Preview {
        metadata: summary.metadata,
        traces,
        band,
        complete: summary.complete,
    }));
    data
}

fn named_trace(
    metadata: &Metadata,
    points: &[little_clip_core::engineering::touchstone::Point],
    [response, stimulus]: Trace,
) -> (String, Vec<TracePoint>) {
    (
        metadata
            .parameter_label(response, stimulus)
            .unwrap_or_default(),
        trace_points(points, [response, stimulus]),
    )
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
    let detail = ready.map_or("Reading RF measurements…", |ready| {
        ready.data.detail.as_str()
    });
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(preview) = ready.and_then(|ready| ready.data.touchstone.as_ref()) else {
        return (None, false);
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, colors.recessed);
    preview.draw(&painter, rect.shrink2(vec2(1.0, 2.0)), colors, false);

    let id = Id::unique(("touchstone-strip", &entry.path, entry.revision));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    *row_response = row_response.clone().union(response.clone());
    let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
    let (engaged, drag) = preview_copy::drag_range(ui, &response, id, modified);
    let selection_id = id.with("band");
    let selection = ui.data_mut(|data| {
        if let Some(range) = drag {
            data.insert_temp(selection_id, range);
        } else if response.clicked_by(egui::PointerButton::Secondary) {
            data.remove::<(f32, f32)>(selection_id);
        }
        data.get_temp::<(f32, f32)>(selection_id)
    });
    if let Some(selection) = selection {
        preview_copy::paint_range(ui, rect, selection);
    }

    let hovered = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
        .map(|pos| plot::hz_at(rect, preview.band, pos.x));
    if !response.dragged() {
        response.on_hover_ui(|ui| preview.tooltip(ui, colors, hovered));
    }
    let band = if let Some((start, end)) = selection {
        [start, end].map(|fraction| plot::hz_at_fraction(preview.band, fraction))
    } else {
        let Some(hz) = hovered else {
            return (None, engaged);
        };
        let half = (preview.band[1] - preview.band[0]) * 0.12;
        plot::fit_band([hz - half, hz + half], preview.band)
    };
    (
        Some(DocumentPosition::TouchstoneBand {
            start_hz_bits: band[0].to_bits(),
            end_hz_bits: band[1].to_bits(),
            revision: entry.revision,
        }),
        engaged,
    )
}

impl Preview {
    fn draw(&self, painter: &egui::Painter, rect: Rect, colors: WorkbenchColors, expanded: bool) {
        let projection = if self.metadata.parameter == Parameter::S {
            Projection::Decibels
        } else {
            Projection::Magnitude
        };
        let draw_count = if expanded { self.traces.len() } else { 1 };
        for (index, (_, points)) in self.traces.iter().take(draw_count).enumerate() {
            let rect = Rect::from_min_size(
                rect.min + vec2(0.0, index as f32 * rect.height() / draw_count as f32),
                vec2(rect.width(), rect.height() / draw_count as f32),
            );
            plot::trace(
                painter,
                rect.shrink2(vec2(0.0, 2.0)),
                self.band,
                points,
                projection,
                [colors.accent, colors.warning][index],
                expanded,
            );
        }
    }

    fn tooltip(&self, ui: &mut Ui, colors: WorkbenchColors, frequency: Option<f64>) {
        ui.set_width(384.0);
        ui.horizontal(|ui| {
            ui.strong(format!(
                "Touchstone {} · {} ports",
                self.metadata.version.label(),
                self.metadata.ports
            ));
            if !self.complete {
                ui.weak("Start of sweep");
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (index, (name, _)) in self.traces.iter().enumerate() {
                ui.colored_label([colors.accent, colors.warning][index], name);
            }
            if let Some(hz) = frequency {
                ui.monospace(display_value(hz, "Hz"));
            }
        });
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 140.0), Sense::hover());
        let rect = Rect::from_min_max(outer.min + vec2(58.0, 4.0), outer.max - vec2(6.0, 18.0));
        let painter = ui.painter_at(outer);
        painter.rect_filled(rect, 2.0, colors.recessed);
        plot::axes(&painter, rect, self.band, colors.text_muted);
        self.draw(&painter, rect, colors, true);
        if let Some(hz) = frequency {
            plot::marker(&painter, rect, self.band, hz, "", colors.focus);
        }
        ui.weak("Drag a band · Double-click to inspect");
    }
}
