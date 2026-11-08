//! One notebook session with lazy preparation of the selected cell.
use super::super::preview_job::Task;
use super::{
    Summary, map,
    model::{self, Document, Kind},
    prepare::{self, Prepared, PreparedOutput},
    summarize,
};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    text_preview::InlineTextPreview,
    ui_appearance::WorkbenchColors,
    ui_tabs::{Tab, TabBar},
};
use egui::{Ui, Vec2, vec2};
use little_clip_core::{BackgroundJobs, palette::AppPalette};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Rendered,
    Source,
}
struct CellView {
    source: InlineTextPreview,
    markdown: Vec<little_clip_render::markdown::MarkdownLine>,
    markdown_truncated: bool,
    outputs: Vec<OutputView>,
    omitted_outputs: usize,
}
enum OutputView {
    Text {
        label: String,
        preview: InlineTextPreview,
        text: Arc<String>,
        error: bool,
    },
    Image(egui::TextureHandle),
    Message(String),
}
pub(in crate::file_manager) struct Viewer {
    id: egui::Id,
    document: Arc<Document>,
    summary: Summary,
    selected: usize,
    scroll_outline: bool,
    submitted: Option<usize>,
    pending: Option<Task<Result<Prepared, String>>>,
    cell: Option<CellView>,
    mode: Mode,
    error: Option<String>,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = model::load(path, &AtomicBool::new(false))?;
        let summary = summarize(&document);
        Ok(Self {
            id: egui::Id::new(("notebook-session", uuid::Uuid::new_v4())),
            document: Arc::new(document),
            summary,
            selected: 0,
            scroll_outline: true,
            submitted: None,
            pending: None,
            cell: None,
            mode: Mode::Rendered,
            error: None,
        })
    }
    pub fn select(&mut self, index: usize) {
        let index = index.min(self.document.cells.len().saturating_sub(1));
        if index != self.selected {
            self.selected = index;
            self.scroll_outline = true;
            self.submitted = None;
            self.cell = None;
            self.error = None;
            self.pending = None;
        }
    }
    fn poll(&mut self, ui: &Ui) {
        if self.document.cells.is_empty() {
            return;
        }
        if self.submitted != Some(self.selected) {
            self.submitted = Some(self.selected);
            let document = self.document.clone();
            let index = self.selected;
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(15),
                move |token| prepare::prepare(&document.cells[index], &document.language, token),
            ));
        }
        let Some(task) = &self.pending else { return };
        let Some(result) = task.poll() else {
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            return;
        };
        self.pending = None;
        match result {
            Err(little_clip_core::JobError::QueueFull) => {
                self.submitted = None;
                ui.ctx().request_repaint_after(Duration::from_millis(40));
            }
            result => match result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            {
                Ok(prepared) => self.cell = Some(CellView::new(prepared, ui)),
                Err(error) => self.error = Some(error),
            },
        }
    }
    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        let mut shell = StudioShell::new("Notebook", Icon::Text, palette, close);
        shell.render_header(ui, |_| {});
        let colors = WorkbenchColors::from_palette(palette);
        ui.label(format!(
            "{} · {}",
            self.summary.detail, self.document.kernel
        ));
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(colors.accent, "Code");
            ui.colored_label(colors.success, "Markdown");
            ui.colored_label(colors.warning, "Saved outputs");
            ui.colored_label(colors.danger, "Saved errors");
        });
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 42.0), egui::Sense::click());
        if let Some(index) = map(ui, rect, &self.summary, Some(self.selected), colors)
            && response.clicked()
        {
            self.select(index);
        }
        self.controls(ui);
        self.poll(ui);
        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.outline(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            self.body(ui, palette);
        });
    }
    fn controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.selected > 0, egui::Button::new("Previous cell"))
                .clicked()
            {
                self.select(self.selected - 1);
            }
            if ui
                .add_enabled(
                    self.selected + 1 < self.document.cells.len(),
                    egui::Button::new("Next cell"),
                )
                .clicked()
            {
                self.select(self.selected + 1);
            }
            if !self.document.cells.is_empty() {
                let mut index = self.selected + 1;
                ui.add(
                    egui::DragValue::new(&mut index)
                        .clamp_range(1..=self.document.cells.len())
                        .prefix("Cell "),
                );
                self.select(index - 1);
                ui.label(format!("of {}", self.summary.cells));
            }
        });
    }
    fn outline(&mut self, ui: &mut Ui) {
        ui.label("Cells");
        let mut scroll = egui::ScrollArea::vertical()
            .id_source((self.id, "outline"))
            .auto_shrink([false, false]);
        if std::mem::take(&mut self.scroll_outline) {
            scroll = scroll.vertical_scroll_offset(self.selected as f32 * 26.0);
        }
        scroll.show_rows(ui, 26.0, self.document.cells.len(), |ui, rows| {
            for index in rows {
                let cell = &self.document.cells[index];
                let caption = format!("{} · {} · {}", index + 1, cell.kind.label(), cell.title);
                let response =
                    crate::ui_ext::selectable_text_row(ui, caption, self.selected == index, 26.0)
                        .on_hover_text(&cell.title);
                if response.clicked() {
                    self.select(index);
                }
            }
        });
    }
    fn body(&mut self, ui: &mut Ui, palette: &AppPalette) {
        if let Some(error) = &self.error {
            ui.label(error);
            return;
        }
        let Some(cell) = self.document.cells.get(self.selected) else {
            ui.label("This notebook has no cells.");
            return;
        };
        ui.heading(format!("{} cell {}", cell.kind.label(), self.selected + 1));
        ui.horizontal_wrapped(|ui| {
            if cell.kind == Kind::Code {
                ui.label(cell.execution.map_or("No saved execution".into(), |count| {
                    format!("Saved execution [{count}]")
                }));
            }
            if ui.button("Copy cell source").clicked() {
                ui.output_mut(|output| output.copied_text = cell.source.as_str().to_owned());
            }
        });
        if cell.kind == Kind::Markdown {
            let tabs = [
                Tab::new(Mode::Rendered, "Rendered"),
                Tab::new(Mode::Source, "Source"),
            ];
            if let Some(mode) =
                TabBar::new((self.id, "cell-mode"), self.mode, &tabs, palette).show(ui)
            {
                self.mode = mode;
            }
        }
        let Some(view) = &mut self.cell else {
            ui.spinner();
            return;
        };
        egui::ScrollArea::vertical().id_source((self.id, "cell", self.selected)).auto_shrink([false, false]).show(ui, |ui| {
            if cell.kind == Kind::Markdown && self.mode == Mode::Rendered {
                crate::text_preview::render_markdown_lines(ui, view.markdown.iter().cloned(), palette);
                if view.markdown_truncated { ui.label("Rendered preview limited to 64 KiB / 512 lines. Source shows the full cell."); }
            } else {
                view.source.resize(vec2(ui.available_width(), view.source.preferred_height(ui, 2, 12)));
                view.source.show(ui, palette);
            }
            if !view.outputs.is_empty() { ui.separator(); ui.strong("Saved outputs"); }
            for output in &mut view.outputs { output.show(ui, palette); }
            if view.omitted_outputs > 0 { ui.label(format!("{} further saved output items are outside this cell’s 32-item preview limit.", view.omitted_outputs)); }
            metadata(ui, "Cell metadata", &cell.metadata);
            metadata(ui, "Notebook metadata", &self.document.metadata);
        });
    }
}
fn metadata(ui: &mut Ui, label: &str, text: &str) {
    egui::CollapsingHeader::new(label).show(ui, |ui| {
        if ui.button("Copy metadata").clicked() {
            ui.output_mut(|output| output.copied_text = text.into());
        }
        ui.monospace(text.chars().take(4096).collect::<String>());
        if text.chars().take(4097).count() > 4096 {
            ui.label(
                "Metadata display limited to 4096 characters; Copy metadata includes all fields.",
            );
        }
    });
}
impl CellView {
    fn new(prepared: Prepared, ui: &Ui) -> Self {
        let outputs = prepared
            .outputs
            .into_iter()
            .map(|output| match output {
                PreparedOutput::Text {
                    label,
                    source,
                    text,
                    error,
                } => OutputView::Text {
                    label,
                    preview: InlineTextPreview::new(source, Vec2::splat(1.0)),
                    text,
                    error,
                },
                PreparedOutput::Image(image) => OutputView::Image(ui.ctx().load_texture(
                    "notebook-output",
                    egui::ColorImage::from_rgba_premultiplied(
                        [image.width as usize, image.height as usize],
                        &image.premultiplied_rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                )),
                PreparedOutput::Message(message) => OutputView::Message(message),
            })
            .collect();
        Self {
            source: InlineTextPreview::new(prepared.source, Vec2::splat(1.0)),
            markdown: prepared.markdown,
            markdown_truncated: prepared.markdown_truncated,
            outputs,
            omitted_outputs: prepared.omitted_outputs,
        }
    }
}
impl OutputView {
    fn show(&mut self, ui: &mut Ui, palette: &AppPalette) {
        match self {
            Self::Text {
                label,
                preview,
                text,
                error,
            } => {
                ui.horizontal_wrapped(|ui| {
                    if *error {
                        ui.colored_label(
                            WorkbenchColors::from_palette(palette).danger,
                            label.as_str(),
                        );
                    } else {
                        ui.label(label.as_str());
                    }
                    if ui.button("Copy output").clicked() {
                        ui.output_mut(|output| output.copied_text = text.as_str().to_owned());
                    }
                });
                preview.resize(vec2(
                    ui.available_width(),
                    preview.preferred_height(ui, 1, 8),
                ));
                preview.show(ui, palette);
            }
            Self::Image(texture) => {
                ui.add(egui::Image::new(&*texture).max_width(ui.available_width()));
            }
            Self::Message(message) => {
                ui.label(message.as_str());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returning_to_a_previously_submitted_cell_reschedules_its_display() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/notebooks/sample.ipynb");
        let mut viewer = Viewer::load(&path).unwrap();
        viewer.submitted = Some(0);
        viewer.select(1);
        viewer.select(0);
        assert_eq!(viewer.selected, 0);
        assert!(viewer.submitted.is_none());
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| viewer.poll(ui));
        });
        assert_eq!(viewer.submitted, Some(0));
        assert!(viewer.pending.is_some() || viewer.cell.is_some());
        viewer.select(usize::MAX);
        assert_eq!(viewer.selected, viewer.document.cells.len() - 1);
    }
}
