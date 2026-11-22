//! Guarded sheet-corner loading and passive List row painting.
use super::{
    display_path,
    filesystem::{Entry, check_revision},
    media::PreviewData,
    shelf::preview::Ready,
};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use little_clip_core::{palette::AppPalette, spreadsheet};
use std::{path::Path, sync::atomic::AtomicBool};

pub(super) fn load(path: &Path, revision: u64, cancelled: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(display_path(path), "Spreadsheet".into(), Icon::Spreadsheet);
    let loaded = (|| {
        check_revision(path, revision)?;
        let workbook = spreadsheet::load_spreadsheet_preview(path, 8, 32, cancelled)?;
        check_revision(path, revision)?;
        workbook
            .sheets
            .into_iter()
            .next()
            .ok_or_else(|| "No sheet data".to_owned())
    })();
    match loaded {
        Ok(sheet) => {
            data.detail = format!(
                "{} · {} rows × {} columns",
                sheet.name, sheet.total_rows, sheet.total_cols
            );
            data.spreadsheet = Some(sheet);
        }
        Err(error) => data.text = error,
    }
    data
}

pub(super) fn show(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    colors: WorkbenchColors,
    palette: &AppPalette,
    ready: Option<&Ready>,
) -> Option<usize> {
    let detail = ready.map_or("Spreadsheet", |ready| ready.data.detail.as_str());
    let rect = super::ui::information_strip(ui, row, entry, detail, Some(columns), colors);
    if !rect.is_positive() || !ui.is_rect_visible(rect) {
        return None;
    }
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 3.0, colors.recessed);
    if let Some(sheet) = ready.and_then(|ready| ready.data.spreadsheet.as_ref()) {
        if sheet.is_empty() {
            paint_spreadsheet_status(&painter, rect, "Empty sheet", colors);
        } else {
            clip_studios::spreadsheet_view::paint_sheet_corner(ui, rect, sheet, palette);
        }
        return Some(sheet.workbook_index);
    }
    let status = if ready.is_none() {
        "Loading sheet…"
    } else {
        "Sheet preview unavailable"
    };
    paint_spreadsheet_status(&painter, rect, status, colors);
    None
}

fn paint_spreadsheet_status(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    colors: WorkbenchColors,
) {
    painter.text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(12.0),
        colors.text_secondary,
    );
}
