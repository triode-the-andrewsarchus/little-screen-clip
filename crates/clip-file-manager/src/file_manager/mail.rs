//! Saved-message reading and attachment summaries, without mail-client actions.
use super::{filesystem::Entry, media::PreviewData, shelf::preview::Ready};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    text_preview::{InlineTextPreview, TextPreviewData},
    ui_appearance::WorkbenchColors,
    ui_tabs::{Tab, TabBar},
};
use little_clip_core::{
    email::{Attachment, Document, MAX_BYTES},
    html_preview::Block,
    palette::AppPalette,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn read(path: &Path, token: &AtomicBool) -> Result<Document, String> {
    let bytes = crate::file_source::read_file_snapshot(path, MAX_BYTES)?;
    if token.load(Ordering::Relaxed) {
        return Err("Email opening cancelled".into());
    }
    Document::parse(&bytes)
}
pub(super) struct Summary {
    subject: String,
    excerpt: String,
    attachments: Vec<Attachment>,
    count: usize,
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Email".into(), Icon::Text);
    match read(path, token) {
        Ok(document) => {
            let sender = document
                .headers
                .iter()
                .find(|(name, _)| name == "From")
                .map(|(_, value)| value.as_str())
                .unwrap_or("Unknown sender");
            data.detail = format!("{} · {} attachments", sender, document.attachments.len());
            data.mail = Some(Arc::new(Summary {
                subject: document.subject.clone(),
                excerpt: document.excerpt(),
                count: document.attachments.len(),
                attachments: document.attachments.into_iter().take(8).collect(),
            }));
        }
        Err(error) => data.detail = error,
    }
    data
}
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Message,
    Source,
}
pub(super) struct Viewer {
    document: Document,
    source: InlineTextPreview,
    mode: Mode,
    selected: usize,
}
impl Viewer {
    pub fn load(path: &Path) -> Result<Self, String> {
        let document = read(path, &AtomicBool::new(false))?;
        let source = InlineTextPreview::new(
            TextPreviewData::from_source(Arc::new(document.source.clone()), Some("txt")),
            egui::vec2(800.0, 500.0),
        );
        Ok(Self {
            document,
            source,
            mode: Mode::Message,
            selected: 0,
        })
    }
    pub fn select(&mut self, index: usize) {
        if index < self.document.attachments.len() {
            self.selected = index;
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        let mut shell = StudioShell::new("Message", Icon::Text, palette, close);
        shell.render_header(ui, |_| {});
        let tabs = [
            Tab::new(Mode::Message, "Message"),
            Tab::new(Mode::Source, "Source"),
        ];
        if let Some(mode) = TabBar::new("mail-mode", self.mode, &tabs, palette).show(ui) {
            self.mode = mode;
        }
        if self.mode == Mode::Source {
            if self.document.source_truncated {
                ui.label("Source preview is truncated after 524,288 characters.");
            }
            if self.document.source_lossy {
                ui.label("Non-UTF-8 source bytes are shown as replacement characters.");
            }
            self.source
                .resize(ui.available_size().max(egui::vec2(1.0, 1.0)));
            self.source.show(ui, palette);
            return;
        }
        let layout = if self.document.attachments.is_empty() {
            StudioPanelLayout::center_only()
        } else {
            StudioPanelLayout::default().with_right()
        }
        .allocate(ui);
        if let Some(right) = layout.right {
            ui.allocate_ui_at_rect(right, |ui| {
                ui.set_clip_rect(right.intersect(ui.clip_rect()));
                self.attachments(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            self.message(ui, palette);
        });
    }
    fn message(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        egui::ScrollArea::vertical()
            .id_source("mail-body")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                ui.heading(&self.document.subject);
                egui::Grid::new("mail-headers")
                    .num_columns(2)
                    .min_row_height(0.0)
                    .spacing(egui::vec2(12.0, 4.0))
                    .show(ui, |ui| {
                        for (name, value) in &self.document.headers {
                            ui.strong(name);
                            ui.add(egui::Label::new(value).wrap(true));
                            ui.end_row();
                        }
                    });
                ui.separator();
                let colors = WorkbenchColors::from_palette(palette);
                for problem in &self.document.problems {
                    ui.colored_label(colors.warning, problem);
                }
                if self.document.body.is_empty() {
                    ui.label("This message has no displayable text body.");
                }
                for block in &self.document.body {
                    match block {
                        Block::Heading(_, text) => {
                            ui.add_space(8.0);
                            ui.heading(text);
                        }
                        Block::Text(text) => {
                            ui.add(egui::Label::new(text).wrap(true));
                            ui.add_space(8.0);
                        }
                        Block::Image(_, _) => (),
                    }
                }
            });
    }
    fn attachments(&mut self, ui: &mut egui::Ui) {
        ui.heading(format!("Attachments ({})", self.document.attachments.len()));
        if let Some(attachment) = self.document.attachments.get(self.selected) {
            ui.label("Content type");
            ui.monospace(&attachment.content_type);
            ui.label(format!("{} decoded bytes", attachment.bytes));
            if attachment.inline {
                ui.label("Inline part");
            }
        }
        ui.separator();
        let row_height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        egui::ScrollArea::vertical()
            .id_source("mail-attachments")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show_rows(
                ui,
                row_height,
                self.document.attachments.len(),
                |ui, rows| {
                    for index in rows {
                        let attachment = &self.document.attachments[index];
                        if crate::ui_ext::selectable_text_row(
                            ui,
                            &attachment.name,
                            self.selected == index,
                            row_height,
                        )
                        .clicked()
                        {
                            self.selected = index;
                        }
                    }
                },
            );
    }
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
        ready.map_or("Email message", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    ui.painter().rect_filled(strip, 2.0, colors.recessed);
    let summary = ready.and_then(|r| r.data.mail.as_ref())?;
    let painter = ui.painter().with_clip_rect(strip.shrink(3.0));
    painter.text(
        strip.left_top() + egui::vec2(5.0, 2.0),
        egui::Align2::LEFT_TOP,
        &summary.subject,
        egui::FontId::proportional(14.0),
        colors.text_primary,
    );
    painter.text(
        strip.left_top() + egui::vec2(5.0, 21.0),
        egui::Align2::LEFT_TOP,
        &summary.excerpt,
        egui::FontId::proportional(12.0),
        colors.text_secondary,
    );
    let mut x = strip.left() + 5.0;
    let mut hovered = None;
    for (index, attachment) in summary.attachments.iter().enumerate() {
        let name: String = attachment.name.chars().take(28).collect();
        let galley = ui.painter().layout_no_wrap(
            name,
            egui::FontId::proportional(12.0),
            colors.text_primary,
        );
        let width = (galley.size().x + 14.0).min(220.0);
        if x + width > strip.right() {
            break;
        }
        let cell = egui::Rect::from_min_size(
            egui::pos2(x, strip.bottom() - 21.0),
            egui::vec2(width, 19.0),
        );
        painter.rect_filled(cell, 3.0, colors.accent.gamma_multiply(0.2));
        painter.galley(
            cell.left_top() + egui::vec2(7.0, 2.0),
            galley,
            colors.text_primary,
        );
        if super::ui::preview_hovered(ui, cell) {
            hovered = Some(index);
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("mail-attachment", index)),
                |ui| {
                    ui.label(format!(
                        "{} · {} · {} bytes",
                        attachment.name, attachment.content_type, attachment.bytes
                    ));
                },
            );
        }
        x += width + 5.0;
    }
    if summary.count > summary.attachments.len() {
        painter.text(
            egui::pos2(x, strip.bottom() - 11.0),
            egui::Align2::LEFT_CENTER,
            "More attachments…",
            egui::FontId::proportional(12.0),
            colors.text_secondary,
        );
    }
    hovered
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/email/harbor.eml")
    }
    #[test]
    fn folder_chips_and_message_details_share_attachment_order() {
        let preview = load(&fixture(), &AtomicBool::new(false));
        let summary = preview.mail.unwrap();
        assert_eq!(summary.count, 2);
        assert_eq!(
            summary
                .attachments
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["route.txt", "tide-data.bin"]
        );
        assert!(summary.excerpt.contains("blue pier"));
        let mut viewer = Viewer::load(&fixture()).unwrap();
        viewer.select(1);
        assert_eq!(
            viewer.document.attachments[viewer.selected].content_type,
            "application/octet-stream"
        );
        viewer.select(usize::MAX);
        assert_eq!(viewer.selected, 1);
        assert!(
            (little_clip_core::file_format::Format::for_path(Path::new("SAVED.EML"))
                == Some(little_clip_core::file_format::Format::Mail))
        );
        assert!(
            !(little_clip_core::file_format::Format::for_path(Path::new("mail.mbox"))
                == Some(little_clip_core::file_format::Format::Mail))
        );
        assert!(read(&fixture(), &AtomicBool::new(true)).is_err());
    }
}
