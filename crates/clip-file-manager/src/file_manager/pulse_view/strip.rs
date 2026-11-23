use super::{draw_pulses, duration};
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
use little_clip_core::engineering::{pulses::Capture, waveforms::display_value};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) type Preview = Capture;

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "Signal capture".into(),
        crate::icons::Icon::Zap,
    );
    match Capture::preview(path, cancel) {
        Ok(capture) => {
            let first = &capture.signals[0];
            let details = if first.pulses.is_empty() {
                first.protocol.as_deref().unwrap_or("Saved signal").into()
            } else {
                format!(
                    "{} pulses · {}",
                    first.pulses.len(),
                    display_value(duration(first), "s")
                )
            };
            let count = capture.signals.len();
            let records = if count > 1 {
                format!(" · {count} signals")
            } else {
                String::new()
            };
            let coverage = if capture.complete { "" } else { " · prefix" };
            data.detail = format!("{} · {details}{records}{coverage}", capture.format.title());
            data.pulses = Some(Arc::new(capture));
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
    let detail = ready.map_or("Reading signals…", |ready| ready.data.detail.as_str());
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(capture) = ready.and_then(|ready| ready.data.pulses.as_ref()) else {
        return (None, false);
    };
    let shown = capture.signals.len().min(5);
    let width = rect.width() / shown as f32;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, colors.recessed);
    for (index, signal) in capture.signals.iter().take(shown).enumerate() {
        let cell = Rect::from_min_size(
            rect.min + vec2(index as f32 * width, 0.0),
            vec2(width, rect.height()),
        )
        .shrink2(vec2(2.0, 2.0));
        let painter = painter.with_clip_rect(cell);
        if signal.pulses.is_empty() {
            painter.text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                signal.label().as_ref(),
                egui::FontId::proportional(11.0),
                ui.visuals().text_color(),
            );
        } else {
            draw_pulses(&painter, cell, signal, &(0.0..duration(signal)));
        }
    }
    let hit = ui.interact(
        rect,
        Id::unique(("pulse-strip", &entry.path, entry.revision)),
        Sense::click(),
    );
    *response = response.clone().union(hit.clone());
    let ordinal = hit
        .hover_pos()
        .map(|pos| (((pos.x - rect.left()) / width) as usize).min(shown - 1));
    if let Some(index) = ordinal {
        let signal = &capture.signals[index];
        hit.on_hover_ui(|ui| {
            ui.strong(signal.label().as_ref());
            if let Some(protocol) = &signal.protocol {
                ui.label(protocol);
            }
            if let Some(carrier) = signal.carrier_hz {
                ui.label(display_value(carrier as f64, "Hz"));
            }
            if !signal.pulses.is_empty() {
                let (rect, _) = ui.allocate_exact_size(vec2(330.0, 60.0), Sense::hover());
                draw_pulses(&ui.painter_at(rect), rect, signal, &(0.0..duration(signal)));
            }
            ui.weak("Double-click to inspect");
        });
    }
    (
        ordinal.map(|ordinal| DocumentPosition::PulseSignal {
            ordinal,
            revision: entry.revision,
        }),
        false,
    )
}
