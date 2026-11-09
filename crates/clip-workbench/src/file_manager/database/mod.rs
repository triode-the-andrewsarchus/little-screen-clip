//! Table selection, bounded row browsing and declared schema relationships.
mod model;
use super::{filesystem::Entry, media::PreviewData, preview_job::Task, shelf::preview::Ready};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    spreadsheet_view::InlineSpreadsheetPreview,
    ui_appearance::WorkbenchColors,
    ui_tabs::{Tab, TabBar},
};
use little_clip_core::{BackgroundJobs, palette::AppPalette};
use little_clip_ops::spreadsheet::{SpreadsheetData, SpreadsheetSheet};
use model::{Document, PAGE_ROWS, Page, Table, clipped};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) fn known(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "sqlite" | "sqlite3" | "db" | "db3"
        )
    })
}
pub(super) struct Summary {
    tables: Vec<(String, String, usize)>,
    total: usize,
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "SQLite".into(), Icon::Text);
    if token.load(Ordering::Relaxed) {
        return data;
    }
    match Document::read(path) {
        Ok(document) if !token.load(Ordering::Relaxed) => {
            data.detail = format!(
                "{} objects{} · checkpointed main file",
                document.tables.len(),
                if document.truncated { "+" } else { "" }
            );
            data.database = Some(Arc::new(Summary {
                total: document.tables.len(),
                tables: document
                    .tables
                    .iter()
                    .take(16)
                    .map(|table| {
                        (
                            clipped(&table.name),
                            table.kind.clone(),
                            table.columns.len(),
                        )
                    })
                    .collect(),
            }));
        }
        Err(error) => data.detail = error,
        _ => (),
    }
    data
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Rows,
    Schema,
}
pub(super) struct Viewer {
    document: Arc<Document>,
    selected: usize,
    mode: Mode,
    page: Option<Page>,
    grid: Option<InlineSpreadsheetPreview>,
    task: Option<Task<Result<Page, String>>>,
    error: Option<String>,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = Arc::new(Document::read(path)?);
        let mut viewer = Self {
            document,
            selected: 0,
            mode: Mode::Rows,
            page: None,
            grid: None,
            task: None,
            error: None,
        };
        viewer.select(0);
        Ok(viewer)
    }
    pub fn select(&mut self, index: usize) {
        if index >= self.document.tables.len() {
            return;
        }
        self.selected = index;
        self.request(0);
    }
    fn request(&mut self, offset: usize) {
        self.task = None;
        self.page = None;
        self.grid = None;
        self.error = None;
        let document = self.document.clone();
        let index = self.selected;
        self.task = Some(Task::start(
            BackgroundJobs::global(),
            Duration::from_secs(10),
            move |token| {
                if token.load(Ordering::Relaxed) {
                    return Err("Table loading cancelled".into());
                }
                document.page(index, offset)
            },
        ));
    }
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.task else {
            return;
        };
        let Some(result) = task.poll() else {
            ctx.request_repaint_after(Duration::from_millis(25));
            return;
        };
        self.task = None;
        match result.map_err(|e| e.to_string()).and_then(|page| page) {
            Ok(page) => {
                let mut rows = Vec::with_capacity(page.rows.len() + 1);
                rows.push(page.columns.clone());
                rows.extend(page.rows.iter().cloned());
                let data = SpreadsheetData {
                    title: String::new(),
                    sheets: vec![SpreadsheetSheet::new(
                        self.document.tables[self.selected].name.clone(),
                        rows,
                    )],
                };
                self.grid = Some(InlineSpreadsheetPreview::new(
                    Arc::new(data),
                    egui::vec2(800.0, 500.0),
                ));
                self.page = Some(page);
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        self.poll(ui.ctx());
        StudioShell::new("Database", Icon::Text, palette, close).render_header(ui, |_| {});
        let tabs = [
            Tab::new(Mode::Rows, "Rows"),
            Tab::new(Mode::Schema, "Schema"),
        ];
        if let Some(mode) = TabBar::new("database-mode", self.mode, &tabs, palette).show(ui) {
            self.mode = mode;
        }
        ui.label(if self.document.wal {
            "Checkpointed main file only · WAL changes are not included · read only"
        } else {
            "Main file snapshot · external journals are not read · read only"
        });
        ui.label(format!(
            "User version {} · application ID {}",
            self.document.user_version, self.document.application_id
        ));
        if self.document.truncated {
            ui.label("Object list is limited to the first 512 names.");
        }
        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.tables(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            let Some(table) = self.document.tables.get(self.selected) else {
                ui.label("No table or view objects");
                return;
            };
            if self.mode == Mode::Schema {
                if let Some(name) = schema(ui, table) {
                    if let Some(index) = self
                        .document
                        .tables
                        .iter()
                        .position(|table| table.name == name)
                    {
                        self.select(index);
                    }
                }
                return;
            }
            self.rows(ui, palette);
        });
    }
    fn rows(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.heading(clipped(&self.document.tables[self.selected].name));
        let mut offset = None;
        if let Some(page) = &self.page {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(page.offset > 0, egui::Button::new("Previous rows"))
                    .clicked()
                {
                    offset = Some(page.offset.saturating_sub(PAGE_ROWS));
                }
                ui.label(if page.rows.is_empty() {
                    "No rows".into()
                } else {
                    format!(
                        "Rows {}–{}{}",
                        page.offset + 1,
                        page.offset + page.rows.len(),
                        if page.more { " · more available" } else { "" }
                    )
                });
                if ui
                    .add_enabled(page.more, egui::Button::new("Next rows"))
                    .clicked()
                {
                    offset = Some(page.offset + PAGE_ROWS);
                }
            });
            if page.limited_columns {
                ui.label(
                    "First 32 ordinary columns shown; see Schema for the complete column list.",
                );
            }
            ui.label("Long text is clipped; BLOBs show byte counts. Row order is the snapshot's scan order.");
        }
        if let Some(offset) = offset {
            self.request(offset);
        }
        if self.task.is_some() {
            ui.label("Loading rows…");
        }
        if let Some(error) = &self.error {
            ui.colored_label(WorkbenchColors::from_palette(palette).warning, error);
        }
        if let Some(grid) = &mut self.grid {
            grid.resize(ui.available_size().max(egui::vec2(1.0, 1.0)));
            grid.show(ui, palette);
        }
    }
    fn tables(&mut self, ui: &mut egui::Ui) {
        ui.heading("Objects");
        let height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        let mut selected = None;
        egui::ScrollArea::vertical()
            .id_source("database-tables")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show_rows(ui, height, self.document.tables.len(), |ui, rows| {
                for index in rows {
                    let table = &self.document.tables[index];
                    if crate::ui_ext::selectable_text_row(
                        ui,
                        clipped(&table.name),
                        index == self.selected,
                        height,
                    )
                    .on_hover_text(format!("{} · {} columns", table.kind, table.columns.len()))
                    .clicked()
                        && index != self.selected
                    {
                        selected = Some(index);
                    }
                }
            });
        if let Some(index) = selected {
            self.select(index);
        }
    }
}
fn schema(ui: &mut egui::Ui, table: &Table) -> Option<String> {
    let mut chosen = None;
    egui::ScrollArea::vertical()
        .id_source("database-schema")
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .show(ui, |ui| {
            ui.heading(clipped(&table.name));
            ui.label(&table.kind);
            if !table.columns.is_empty() {
                egui::Grid::new("database-columns")
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Column");
                        ui.strong("Declared type");
                        ui.strong("Constraints");
                        ui.end_row();
                        for column in &table.columns {
                            ui.label(clipped(&column.name));
                            ui.label(clipped(&column.kind));
                            ui.label(format!(
                                "{}{}{}",
                                if column.primary { "Primary key " } else { "" },
                                if column.required { "Not null " } else { "" },
                                if column.hidden {
                                    "Generated/hidden"
                                } else {
                                    ""
                                }
                            ));
                            ui.end_row();
                        }
                    });
            }
            if !table.relationships.is_empty() {
                ui.separator();
                ui.strong("Declared foreign keys");
                for (source, target, column) in &table.relationships {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{} →", clipped(source)));
                        if ui
                            .button(clipped(target))
                            .on_hover_text("Inspect the referenced table")
                            .clicked()
                        {
                            chosen = Some(target.clone());
                        }
                        ui.label(format!(". {}", clipped(column)));
                    });
                }
            }
            ui.separator();
            ui.strong("Stored definition");
            ui.add(egui::Label::new(egui::RichText::new(&table.sql).monospace()).wrap(true));
        });
    chosen
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let strip = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("SQLite schema", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(strip, 2.0, colors.recessed);
    let summary = ready.and_then(|r| r.data.database.as_ref())?;
    let mut x = strip.left() + 4.0;
    let mut hovered = None;
    for (index, (name, kind, count)) in summary.tables.iter().enumerate() {
        let width = 180.0;
        if x + width > strip.right() {
            break;
        }
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, strip.top() + 4.0),
            egui::vec2(width, strip.height() - 8.0),
        );
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 3.0, colors.accent.gamma_multiply(0.18));
        painter.text(
            rect.left_top() + egui::vec2(6.0, 4.0),
            egui::Align2::LEFT_TOP,
            name,
            egui::TextStyle::Body.resolve(ui.style()),
            colors.text_primary,
        );
        painter.text(
            rect.left_top() + egui::vec2(6.0, 28.0),
            egui::Align2::LEFT_TOP,
            if kind == "table" {
                format!("{kind} · {count} columns")
            } else {
                format!("{kind} · metadata only")
            },
            egui::TextStyle::Small.resolve(ui.style()),
            colors.text_secondary,
        );
        if super::ui::preview_hovered(ui, rect) {
            hovered = Some(index);
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("database-object", index)),
                |ui| {
                    ui.label(name);
                    ui.label(format!("{} schema objects total", summary.total));
                },
            );
        }
        x += width + 4.0;
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_summary_and_selected_table_keep_the_same_schema_order() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/database/harbor.sqlite");
        let data = load(&path, &AtomicBool::new(false));
        let summary = data.database.unwrap();
        assert_eq!(summary.total, 3);
        assert_eq!(summary.tables[1].0, "sailings");
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.select(1);
        assert_eq!(viewer.document.tables[viewer.selected].name, "sailings");
        viewer.select(usize::MAX);
        assert_eq!(viewer.selected, 1);
        assert!(known(Path::new("SAVED.SQLITE3")));
        assert!(!known(Path::new("saved.parquet")));
        assert!(load(&path, &AtomicBool::new(true)).database.is_none());
    }
}
