//! Selection follows the same source bins as the painted minimap.
use super::Document;
use crate::file_manager::{filesystem::Entry, preview_copy, search::PreviewContext};
use egui::{Id, Rect, Response, Sense, Ui};
use std::{
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::PathBuf,
    sync::atomic::AtomicBool,
};

pub(in crate::file_manager) fn focus_id() -> Id {
    Id::unique("files_text_minimap_focus")
}

#[derive(Default)]
pub(in crate::file_manager) struct Selection {
    pub range: Option<Selected>,
    pub copy: bool,
}

#[derive(Clone)]
pub(in crate::file_manager) struct Selected {
    path: PathBuf,
    revision: u64,
    source: crate::file_source::FileSource,
    bins: Range<usize>,
    context: PreviewContext,
    bytes: Range<u64>,
    lines: (u64, u64),
}

impl Selection {
    #[expect(
        clippy::too_many_arguments,
        reason = "The strip needs its display width, source observation and parent row response"
    )]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        width: usize,
        entry: &Entry,
        document: &Document,
        row: &mut Response,
        context: &PreviewContext,
    ) -> bool {
        if document.lines == 0 || !rect.is_positive() {
            return false;
        }
        let interaction = ui.interact(
            rect,
            Id::unique(("text_minimap", &entry.path)),
            Sense::click_and_drag(),
        );
        let modified = ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift);
        let (dragging, range) = preview_copy::drag_range(ui, &interaction, focus_id(), modified);
        if let Some((start, end)) = range {
            let bins =
                document.display_range(start, width).start..document.display_range(end, width).end;
            self.range = Some(Selected::new(entry, document, bins, context));
        }
        let selected = self
            .range
            .as_ref()
            .filter(|range| range.matches(entry, document));
        let handled = selected.is_some_and(|range| {
            let (copy, menu) = range.show(ui, width, document, &interaction);
            self.copy |= copy;
            menu
        });
        if !dragging && !handled {
            *row = row.union(interaction);
        }
        dragging || handled
    }
}

impl Selected {
    fn new(
        entry: &Entry,
        document: &Document,
        bins: Range<usize>,
        context: &PreviewContext,
    ) -> Self {
        let first = &document.bins[bins.start];
        let last = &document.bins[bins.end - 1];
        Self {
            path: entry.path.clone(),
            revision: entry.revision,
            source: document.source.clone(),
            bins,
            bytes: first.bytes.start..last.bytes.end,
            lines: (first.start + 1, last.end),
            context: context.clone(),
        }
    }

    fn matches(&self, entry: &Entry, document: &Document) -> bool {
        self.path == entry.path && self.revision == entry.revision && self.source == document.source
    }

    fn show(
        &self,
        ui: &mut Ui,
        width: usize,
        document: &Document,
        interaction: &Response,
    ) -> (bool, bool) {
        let (first, last) = self.lines;
        let extent = document.display_extent(&self.bins, width);
        preview_copy::paint_range(ui, interaction.rect, extent);
        let mut copy = preview_copy::paint_range_footer(
            ui,
            interaction.rect,
            &format!("Lines {first}–{last}  ·  Ctrl+C"),
            "Copy lines",
        );
        let menu = interaction.context_menu(|ui| {
            if ui.button("Copy selected lines").clicked() {
                copy = true;
                ui.close();
            }
            ui.separator();
            let first_line = document.bins[self.bins.start]
                .excerpt
                .first()
                .into_iter()
                .flat_map(|line| &line.runs)
                .map(|(text, _)| text.as_str())
                .collect::<String>();
            let last_line = &document.bins[self.bins.end - 1].last_line;
            ui.label(format!("Start · {first}  {}", first_line.trim_end()));
            ui.label(format!("End · {last}  {last_line}"));
        });
        if copy {
            ui.memory_mut(|memory| memory.request_focus(focus_id()));
        }
        (copy, menu.is_some())
    }

    pub fn export(&self, cancel: &AtomicBool) -> Result<String, String> {
        let bytes = self.read(cancel)?;
        little_clip_core::copy_text_to_system_clipboard(&String::from_utf8_lossy(&bytes))
            .map_err(|error| error.to_string())?;
        let (first, last) = self.lines;
        Ok(format!("Copied lines {first}–{last}"))
    }

    fn read(&self, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        preview_copy::read_range(&self.context, &self.path, &self.source, cancel, |input| {
            input
                .seek(SeekFrom::Start(self.bytes.start))
                .map_err(|error| error.to_string())?;
            let length = self.bytes.end - self.bytes.start;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(length as usize)
                .map_err(|error| error.to_string())?;
            let copied = input
                .take(length)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if copied as u64 != length {
                return Err("Text source changed while copying".into());
            }
            Ok(bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_minimap_drag_keeps_both_lines_and_copies_the_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lines.txt");
        std::fs::write(&path, "one\r\n二\rthree\nfour").unwrap();
        let cancel = AtomicBool::new(false);
        let entry = crate::file_manager::filesystem::read_directory(dir.path(), &cancel)
            .unwrap()
            .entries
            .remove(0);
        let document = crate::file_manager::text_strip::load(&path, &cancel).unwrap();
        let ctx = egui::Context::default();
        let mut selection = Selection::default();
        let rect = Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(400.0, 80.0));
        let start = egui::pos2(375.0, 140.0);
        let end = egui::pos2(275.0, 140.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(start), button(start, true)],
            vec![egui::Event::PointerMoved(end)],
            vec![button(end, false)],
        ] {
            let _ = clip_ui::test_support::run_frame(
                &ctx,
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            ui.interact(rect, focus_id(), Sense::focusable_noninteractive());
                            let mut row =
                                ui.interact(rect, Id::unique("minimap_test_row"), Sense::click());
                            selection.show(
                                ui,
                                rect,
                                400,
                                &entry,
                                &document,
                                &mut row,
                                &PreviewContext::default(),
                            );
                        });
                    })
                },
            );
        }
        let selected = selection.range.unwrap();
        assert_eq!(selected.lines, (2, 3));
        assert!(ctx.memory(|memory| memory.has_focus(focus_id())));
        assert_eq!(selected.read(&cancel).unwrap(), "二\rthree\n".as_bytes());
        std::fs::write(&path, b"changed").unwrap();
        assert!(selected.read(&cancel).is_err());
    }
}
