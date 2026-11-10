//! Full-duration subtitle coverage without retaining caption documents in list rows.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use little_clip_core::subtitles::{Document, Format, MAX_BYTES, Timeline};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) struct Summary {
    timeline: Timeline,
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Subtitle cue timeline".into(), Icon::Text);
    match read(path, token) {
        Ok(document) => {
            data.detail = crate::subtitle_view::details(document.cues.len(), &document.timeline);
            data.subtitles = Some(Arc::new(Summary {
                timeline: document.timeline,
            }));
        }
        Err(error) => data.detail = error,
    }
    data
}
fn read(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BYTES)?;
    let source = std::str::from_utf8(&bytes).map_err(|_| "Subtitle preview requires UTF-8 text")?;
    Document::parse(
        source,
        Format::from_path(path).ok_or("Unknown subtitle format")?,
        token,
    )
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let rect = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Subtitle cue timeline", |ready| &ready.data.detail),
        Some(columns),
        colors,
    );
    let Some(summary) = ready.and_then(|ready| ready.data.subtitles.as_ref()) else {
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            ready.map_or("Loading cues…", |ready| &ready.data.detail),
            egui::FontId::proportional(13.0),
            colors.text_secondary,
        );
        return None;
    };
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|input| input.pointer.hover_pos()))
        .flatten();
    crate::subtitle_view::paint(ui, rect, &summary.timeline, None, pointer, colors)
}
