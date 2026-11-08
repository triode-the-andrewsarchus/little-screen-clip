//! A compact cell map shared by notebook rows and the interactive viewer.
mod model;
mod prepare;
pub(super) mod viewer;
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Rect, Ui, pos2, vec2};
use model::{Document, Kind};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use unicode_width::UnicodeWidthStr;

pub(super) struct Summary {
    pub detail: String,
    pub cells: usize,
    spans: Vec<Span>,
}
struct Span {
    first: usize,
    end: usize,
    kind: Kind,
    counts: [usize; 4],
    errors: usize,
    outputs: usize,
    lines: usize,
    excerpt: String,
    strokes: Vec<(usize, usize)>,
}
pub(super) fn known(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Notebook".into(), Icon::Text);
    match model::load(path, token) {
        Ok(document) => {
            let summary = Arc::new(summarize(&document));
            data.detail = summary.detail.clone();
            data.notebook = Some(summary);
        }
        Err(error) => data.detail = error,
    }
    data
}
fn summarize(document: &Document) -> Summary {
    let cells = document.cells.len();
    let code = document
        .cells
        .iter()
        .filter(|cell| cell.kind == Kind::Code)
        .count();
    let errors = document
        .cells
        .iter()
        .filter(|cell| cell.has_error())
        .count();
    let detail = format!(
        "{cells} cells · {code} code · {errors} with saved errors · {}",
        document.language
    );
    let step = cells.div_ceil(256).max(1);
    let spans = document
        .cells
        .chunks(step)
        .enumerate()
        .map(|(group, cells)| {
            let first = group * step;
            Span {
                first,
                end: first + cells.len(),
                kind: cells[0].kind,
                counts: std::array::from_fn(|index| {
                    cells
                        .iter()
                        .filter(|cell| cell.kind as usize == index)
                        .count()
                }),
                errors: cells.iter().filter(|cell| cell.has_error()).count(),
                outputs: cells.iter().map(|cell| cell.output_count).sum(),
                lines: cells.iter().map(|cell| cell.source.lines().count()).sum(),
                excerpt: cells[0].source.chars().take(160).collect(),
                strokes: cells[0]
                    .source
                    .lines()
                    .take(8)
                    .map(|line| {
                        let prefix = super::text_strip::expand_tabs(
                            &line.chars().take(80).collect::<String>(),
                        );
                        let end = prefix.width();
                        let start = end - prefix.trim_start().width();
                        (start.min(80), end.min(80))
                    })
                    .collect(),
            }
        })
        .collect();
    Summary {
        detail,
        cells,
        spans,
    }
}
fn kind_color(kind: Kind, colors: WorkbenchColors) -> egui::Color32 {
    match kind {
        Kind::Code => colors.accent,
        Kind::Markdown => colors.success,
        _ => colors.text_secondary,
    }
}
/// Paint semantic cells without installing row-level widgets or consuming scrolling.
fn map(
    ui: &mut Ui,
    rect: Rect,
    summary: &Summary,
    selected: Option<usize>,
    colors: WorkbenchColors,
) -> Option<usize> {
    if summary.spans.is_empty() {
        return None;
    }
    let width = rect.width() / summary.spans.len() as f32;
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|input| input.pointer.hover_pos()))
        .flatten();
    let mut hovered = None;
    for (index, span) in summary.spans.iter().enumerate() {
        let cell = Rect::from_min_max(
            pos2(rect.left() + index as f32 * width, rect.top()),
            pos2(
                rect.left() + (index + 1) as f32 * width - width.min(1.0) * 0.5,
                rect.bottom(),
            ),
        );
        let color = kind_color(span.kind, colors);
        ui.painter()
            .rect_filled(cell, 2.0, color.gamma_multiply(0.22));
        let mut x = cell.left();
        for (index, count) in span.counts.iter().enumerate() {
            let part = cell.width() * *count as f32 / (span.end - span.first) as f32;
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(x, cell.top()), vec2(part, 3.0)),
                0.0,
                kind_color(Kind::ALL[index], colors),
            );
            x += part;
        }
        for (line, (start, end)) in span
            .strokes
            .iter()
            .enumerate()
            .filter(|(_, (start, end))| cell.width() >= 8.0 && start < end)
        {
            let y = cell.top() + 8.0 + line as f32 * (cell.height() - 16.0).max(8.0) / 8.0;
            let scale = (cell.width() - 6.0).max(0.0) / 80.0;
            let start = cell.left() + 3.0 + *start as f32 * scale;
            let end = cell.left() + 3.0 + *end as f32 * scale;
            ui.painter().line_segment(
                [pos2(start, y), pos2(end, y)],
                egui::Stroke::new(1.0_f32, color),
            );
        }
        if span.outputs > 0 {
            let mark = Rect::from_min_size(
                pos2(cell.left(), cell.bottom() - 5.0),
                vec2(cell.width(), 5.0),
            );
            ui.painter().rect_filled(
                mark,
                0.0,
                if span.errors > 0 {
                    colors.danger
                } else {
                    colors.warning
                },
            );
        }
        if selected.is_some_and(|selected| (span.first..span.end).contains(&selected)) {
            ui.painter().rect_stroke(
                cell.shrink(1.0),
                2.0,
                egui::Stroke::new(2.0_f32, colors.focus),
            );
        }
        if pointer.is_some_and(|point| cell.contains(point)) {
            hovered = Some(index);
        }
    }
    if let Some(index) = hovered {
        let span = &summary.spans[index];
        egui::show_tooltip_at_pointer(
            ui.ctx(),
            egui::Id::new(("notebook-map", rect.min.x.to_bits(), rect.min.y.to_bits())),
            |ui| {
                ui.label(format!(
                    "Cells {}–{} · first: {} · {} lines · {} outputs",
                    span.first + 1,
                    span.end,
                    span.kind.label(),
                    span.lines,
                    span.outputs
                ));
                ui.label(&span.excerpt);
            },
        );
        return Some(span.first);
    }
    None
}
pub(super) fn show_list(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let strip = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Notebook cell map", |ready| &ready.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(strip, 3.0, colors.recessed);
    let Some(summary) = ready.and_then(|ready| ready.data.notebook.as_ref()) else {
        ui.painter().text(
            strip.left_center(),
            egui::Align2::LEFT_CENTER,
            ready.map_or("Loading notebook…", |ready| &ready.data.detail),
            egui::FontId::proportional(13.0),
            colors.text_secondary,
        );
        return None;
    };
    map(ui, strip, summary, None, colors)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cell_map_preserves_indentation_tabs_and_empty_lines() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), br#"{"nbformat":4,"cells":[{"cell_type":"code","source":"if ready:\n\tprint(1)\n\n    pass\n"}]}"#).unwrap();
        let document = model::load(file.path(), &AtomicBool::new(false)).unwrap();
        let summary = summarize(&document);
        assert_eq!(summary.spans[0].strokes[1], (4, 12));
        assert_eq!(summary.spans[0].strokes[2], (0, 0));
        assert_eq!(summary.spans[0].strokes[3], (4, 8));
    }

    #[test]
    fn large_row_summaries_keep_bounded_excerpts_and_cell_group_ranges() {
        let path = tempfile::NamedTempFile::new().unwrap();
        let value = serde_json::json!({"nbformat":4,"cells":(0..2000).map(|_| serde_json::json!({"cell_type":"code","source":"x".repeat(4096)})).collect::<Vec<_>>()});
        std::fs::write(path.path(), serde_json::to_vec(&value).unwrap()).unwrap();
        let document = model::load(path.path(), &AtomicBool::new(false)).unwrap();
        let summary = summarize(&document);
        assert_eq!(summary.cells, 2000);
        assert!(summary.spans.len() <= 256);
        assert_eq!(summary.spans.last().unwrap().end, 2000);
        assert!(
            summary
                .spans
                .iter()
                .all(|span| span.excerpt.len() <= 160 && span.strokes.len() <= 8)
        );
    }
}
