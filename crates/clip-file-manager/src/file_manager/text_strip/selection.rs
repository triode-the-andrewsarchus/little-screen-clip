//! Selection follows the same source bins as the painted minimap.
use super::Document;
use crate::file_manager::{filesystem::Entry, search::PreviewContext};
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
        let Some(source) = document.source.as_ref().filter(|_| document.lines > 0) else {
            return false;
        };
        let interaction = ui.interact(
            rect,
            Id::unique(("text_minimap", &entry.path)),
            Sense::click_and_drag(),
        );
        let modified = ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift);
        let dragging = interaction.dragged_by(egui::PointerButton::Primary)
            || interaction.drag_stopped_by(egui::PointerButton::Primary);
        if !modified && dragging {
            ui.memory_mut(|memory| memory.request_focus(focus_id()));
            if let Some((start, end)) =
                crate::file_manager::preview_copy::drag_fractions(ui, &interaction)
            {
                let bins = document.display_range(start, width).start
                    ..document.display_range(end, width).end;
                let first = &document.bins[bins.start];
                let last = &document.bins[bins.end - 1];
                self.range = Some(Selected {
                    path: entry.path.clone(),
                    revision: entry.revision,
                    source: source.clone(),
                    bins,
                    bytes: first.bytes.start..last.bytes.end,
                    lines: (first.start + 1, last.end),
                    context: context.clone(),
                });
            }
        }
        if let Some(range) = self.range.as_ref().filter(|range| {
            range.path == entry.path
                && range.revision == entry.revision
                && range.source == *source
                && range.bins.end <= document.bins.len()
        }) {
            let first = &document.bins[range.bins.start];
            let last = &document.bins[range.bins.end - 1];
            let extent = document.display_extent(&range.bins, width);
            let label = crate::file_manager::preview_copy::paint_range(
                ui,
                rect,
                extent,
                &format!("Lines {}–{}  ·  Ctrl+C", first.start + 1, last.end),
            );
            if label.width() >= 290.0
                && ui
                    .put(
                        Rect::from_min_max(
                            egui::pos2(label.right() - 74.0, label.top() + 1.0),
                            label.right_bottom(),
                        ),
                        egui::Button::new("Copy lines").small(),
                    )
                    .clicked()
            {
                self.copy = true;
            }
            interaction.context_menu(|ui| {
                if ui.button("Copy selected lines").clicked() {
                    self.copy = true;
                    ui.close();
                }
                ui.separator();
                let first_text = first
                    .excerpt
                    .first()
                    .into_iter()
                    .flat_map(|line| &line.runs)
                    .map(|(text, _)| text.as_str())
                    .collect::<String>();
                ui.label(format!(
                    "Start · {}  {}",
                    first.start + 1,
                    first_text.trim_end()
                ));
                ui.label(format!("End · {}  {}", last.end, last.last_line));
            });
            if interaction.secondary_clicked() {
                return true;
            }
        }
        if !dragging || modified {
            *row = row.union(interaction);
        }
        !modified && dragging
    }
}

impl Selected {
    pub fn export(&self, cancel: &AtomicBool) -> Result<String, String> {
        let (first, last) = self.lines;
        let label = format!("lines-{first}-{last}");
        crate::file_manager::preview_copy::copy_file(&self.path, &label, None, |output| {
            self.write(output, cancel)
        })?;
        Ok(format!("Copied lines {first}–{last} as a file"))
    }
    fn write(&self, output: &mut impl std::io::Write, cancel: &AtomicBool) -> Result<(), String> {
        self.context.read(&self.path, cancel, || {
            let (mut input, _) = crate::file_source::ReadCheck::default().open(
                &self.path,
                Some(&self.source),
                cancel,
            )?;
            input
                .seek(SeekFrom::Start(self.bytes.start))
                .map_err(|e| e.to_string())?;
            let length = self.bytes.end - self.bytes.start;
            let copied = std::io::copy(&mut (&mut input).take(length), output)
                .map_err(|error| error.to_string())?;
            if copied != length {
                return Err("Text source changed while copying".into());
            }
            self.source.check_file(&input)?;
            if crate::file_source::FileSource::capture(&self.path)? != self.source {
                return Err("Text source changed while copying".into());
            }
            Ok(())
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
        std::fs::write(&path, b"one\r\ntwo\rthree\nfour").unwrap();
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
        let mut bytes = Vec::new();
        selected.write(&mut bytes, &cancel).unwrap();
        assert_eq!(bytes, b"two\rthree\n");
        std::fs::write(&path, b"changed").unwrap();
        assert!(selected.write(&mut Vec::new(), &cancel).is_err());
    }
}
