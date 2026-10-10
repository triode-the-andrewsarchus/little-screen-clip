use super::{Document, Preview, transport::Range};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Id, Rect, Sense, Ui, pos2, vec2};
use std::sync::Arc;
pub(in crate::file_manager) fn focus_id() -> Id {
    Id::new("files_midi_strip_focus")
}

pub(in crate::file_manager) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &super::super::filesystem::Entry,
    previews: &mut super::super::previews::EntryPreviews,
    preview: &mut Preview,
    response: &mut egui::Response,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) -> bool {
    let icon = Rect::from_min_size(row.min + vec2(6.0, 32.0), vec2(64.0, row.height() - 42.0));
    let icon_response = ui.interact(
        icon,
        Id::new(("midi_strip_editor", &entry.path)),
        Sense::click(),
    );
    Icon::Midi.paint(ui.painter(), icon.shrink(16.0), colors.text_secondary);
    icon_response
        .clone()
        .on_hover_text("Double-click to open the MIDI editor");
    *response = response.clone().union(icon_response);
    let strip = Rect::from_min_max(
        pos2(icon.right() + 8.0, icon.top()),
        pos2(row.right() - 6.0, row.bottom() - 10.0),
    );
    ui.painter().rect_filled(strip, 3.0, colors.recessed);
    let document = previews.show_midi(ui, strip, entry, palette);
    let details = document.as_ref().map(|document| document.details());
    super::super::ui::media_header(
        ui,
        Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
        entry,
        details,
        colors,
    );
    let Some(document) = document else {
        let detail = previews
            .listing(entry)
            .map_or("Loading MIDI preview…", |ready| {
                ready.data.detail.as_str()
            });
        ui.painter().text(
            strip.left_center(),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(14.0),
            colors.text_secondary,
        );
        return false;
    };
    paint_selection(ui, strip, entry, preview, &document, colors);
    let interaction = ui.interact(
        strip,
        Id::new(("midi_strip", &entry.path)),
        Sense::click_and_drag(),
    );
    if ui.input(|input| input.modifiers.ctrl || input.modifiers.command || input.modifiers.shift) {
        *response = response.clone().union(interaction);
        return false;
    }
    let time_at = |point: egui::Pos2| {
        ((point.x - strip.left()) / strip.width()).clamp(0.0, 1.0) * document.data.duration_sec
    };
    if let Some(point) = interaction.interact_pointer_pos() {
        let time = time_at(point);
        if interaction.clicked() {
            preview.queued = Some((entry.path.clone(), entry.revision, document.clone(), time));
        }
        if interaction.drag_started() {
            let start = ui
                .input(|input| input.pointer.press_origin())
                .map_or(time, time_at);
            preview.selection = Some(Range {
                path: entry.path.clone(),
                revision: entry.revision,
                document: document.clone(),
                start,
                end: time,
            });
        }
        if interaction.dragged()
            && let Some(range) = &mut preview.selection
        {
            range.end = time;
        }
    }
    let engaged =
        interaction.clicked() || interaction.dragged() || interaction.is_pointer_button_down_on();
    if engaged {
        ui.memory_mut(|memory| memory.request_focus(focus_id()));
    }
    interaction.on_hover_text("Click to play from here. Drag to select MIDI; Ctrl+C copies the region, Space plays it once. Ctrl/Shift+click selects files.");
    engaged
}
fn paint_selection(
    ui: &Ui,
    strip: Rect,
    entry: &super::super::filesystem::Entry,
    preview: &Preview,
    document: &Arc<Document>,
    colors: WorkbenchColors,
) {
    let duration = document.data.duration_sec.max(0.001);
    if let Some(range) = preview
        .selection
        .as_ref()
        .filter(|range| range.path == entry.path && range.revision == entry.revision)
    {
        let x = |time: f32| strip.left() + time.clamp(0.0, duration) / duration * strip.width();
        let selected = Rect::from_min_max(
            pos2(x(range.start.min(range.end)), strip.top()),
            pos2(x(range.start.max(range.end)), strip.bottom()),
        );
        ui.painter()
            .rect_filled(selected, 0.0, colors.accent.gamma_multiply(0.25));
        ui.painter()
            .rect_stroke(selected, 0.0, egui::Stroke::new(1.0_f32, colors.accent));
    }
    if let Some(time) = preview.position(&entry.path, entry.revision) {
        let x = strip.left() + time.clamp(0.0, duration) / duration * strip.width();
        ui.painter().line_segment(
            [pos2(x, strip.top()), pos2(x, strip.bottom())],
            egui::Stroke::new(2.0_f32, colors.accent),
        );
    }
}
