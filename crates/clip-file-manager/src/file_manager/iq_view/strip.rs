//! Capture power and range selection in a file-list strip.
use super::plot;
use crate::{
    file_manager::{
        DocumentPosition, display_path,
        filesystem::Entry,
        preview_copy,
        shelf::preview::{Data, Ready},
        ui::{ListColumns, information_strip},
    },
    file_source::ReadCheck,
    ui_appearance::WorkbenchColors,
};
use egui::{Id, Rect, Sense, Ui, vec2};
use little_clip_core::engineering::{
    iq::{PowerOverview, Recording},
    waveforms::display_value,
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    pub power: PowerOverview,
    pub rate: Option<f64>,
    pub center_hz: Option<f64>,
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "I/Q recording".into(),
        crate::icons::Icon::Waves,
    );
    let result =
        Recording::open_sigmf(path, None, ReadCheck::default(), cancel).and_then(|recording| {
            recording.checked(cancel, |capture| {
                let metadata = capture.metadata();
                let power = capture.overview(0..capture.sample_count(), 160, cancel)?;
                let rate = metadata.sample_rate_hz;
                let center_hz = capture.tuning_at(0).and_then(|tuning| tuning.center_hz);
                Ok((
                    Preview {
                        power,
                        rate,
                        center_hz,
                    },
                    metadata.encoding.name(),
                ))
            })
        });
    match result {
        Ok((capture, encoding)) => {
            let span = capture.power.samples.clone();
            let duration = plot::sample_label(span.end - span.start, capture.rate);
            let sampled = if capture.power.sampled() {
                " · sampled"
            } else {
                ""
            };
            data.detail = format!("{encoding} · {duration}{sampled}");
            data.iq = Some(Arc::new(capture));
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
    let detail = ready.map_or("Reading I/Q recording…", |ready| {
        ready.data.detail.as_str()
    });
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(capture) = ready.and_then(|ready| ready.data.iq.as_ref()) else {
        return (None, false);
    };
    let span = &capture.power.samples;
    let scale = plot::power_scale(&capture.power);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, colors.recessed);
    plot::draw_activity(
        &painter,
        rect.shrink2(vec2(3.0, 1.0)),
        span,
        &capture.power,
        &scale,
        None,
    );
    let id = Id::unique(("iq-strip", &entry.path, entry.revision));
    let (response, engaged, selection) = preview_copy::strip_range(ui, rect, id, row_response);
    let hovered = response
        .hover_pos()
        .map(|position| plot::sample_at(rect, span, position.x));
    if !response.dragged() {
        response.on_hover_ui(|ui| {
            if let Some(rate) = capture.rate {
                ui.strong(display_value(rate, "Sa/s"));
            }
            if let Some(center) = capture.center_hz {
                ui.weak(format!("Center {}", display_value(center, "Hz")));
            }
            let (rect, _) = ui.allocate_exact_size(vec2(320.0, 76.0), Sense::hover());
            let paint = ui.painter_at(rect);
            paint.rect_filled(rect, 2.0, colors.recessed);
            plot::draw_activity(&paint, rect, span, &capture.power, &scale, None);
            if let Some(hovered) = hovered {
                paint.vline(
                    plot::x_at(rect, span, hovered),
                    rect.y_range(),
                    egui::Stroke::new(0.75, egui::Color32::WHITE),
                );
            }
            ui.weak("Drag to select · Double-click to view");
        });
    }
    let position = selection
        .map(|(first, end)| plot::sample_fraction(span, first)..plot::sample_fraction(span, end))
        .or_else(|| {
            hovered.map(|center| {
                let half = ((span.end - span.start) / 20).max(1);
                center.saturating_sub(half).max(span.start)
                    ..center.saturating_add(half).min(span.end)
            })
        })
        .filter(|range| range.start < range.end)
        .map(|range| DocumentPosition::IqSamples {
            first: range.start,
            end: range.end,
            revision: entry.revision,
            pair_revision: entry.sigmf_pair_revision(),
        });
    (position, engaged)
}
