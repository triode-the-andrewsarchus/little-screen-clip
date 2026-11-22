//! Exact page selections over the visible thumbnails and their omitted span.
use crate::file_manager::{filesystem, preview_copy, search::PreviewContext};
use egui::{Id, Pos2, Rect, Response, Sense, Ui};
use std::{fs::File, ops::RangeInclusive, path::PathBuf, sync::atomic::AtomicBool};

pub(in crate::file_manager) fn focus_id() -> Id {
    Id::unique("files_pdf_page_strip_focus")
}

pub(super) struct Layout<'a> {
    pub rect: Rect,
    pub slots: &'a [Option<usize>],
    pub count: usize,
}

impl Layout<'_> {
    pub fn width(&self) -> f32 {
        56.0_f32.min(self.rect.width() / self.slots.len().max(1) as f32)
    }

    fn track(&self) -> Rect {
        let mut rect = self.rect;
        rect.max.x = rect.left() + self.width() * self.slots.len() as f32;
        rect
    }

    fn page_at(&self, fraction: f32) -> Option<usize> {
        let index = (fraction * self.slots.len() as f32) as usize;
        self.slots
            .get(index.min(self.slots.len().checked_sub(1)?))
            .copied()
            .flatten()
    }

    fn gap(&self, at: Pos2) -> Option<(usize, usize)> {
        let index = ((at.x - self.rect.left()) / self.width()).floor() as usize;
        if !self.track().contains(at) || self.slots.get(index)?.is_some() {
            return None;
        }
        let before = self.slots.get(index.checked_sub(1)?)?.as_ref()?;
        let after = self.slots.get(index + 1)?.as_ref()?;
        Some((before + 1, after - 1))
    }

    pub fn highlight(&self, pages: &RangeInclusive<usize>, body: Rect) -> Rect {
        let slot = |page| {
            self.slots
                .iter()
                .position(|item| *item == Some(page))
                .or_else(|| self.slots.iter().position(Option::is_none))
                .unwrap_or_default()
        };
        let mut rect = body;
        rect.min.x = body.left() + slot(*pages.start()) as f32 * self.width();
        rect.max.x = body.left() + (slot(*pages.end()) + 1) as f32 * self.width();
        rect
    }
}

#[derive(Default)]
pub(in crate::file_manager) struct Selection {
    pub selected: Option<Pages>,
    pub requested: Option<Pages>,
}

#[derive(Clone)]
pub(in crate::file_manager) struct Pages {
    path: PathBuf,
    revision: u64,
    pub(super) range: RangeInclusive<usize>,
}

impl Pages {
    fn new(entry: &filesystem::Entry, (first, last): (usize, usize)) -> Self {
        Self {
            path: entry.path.clone(),
            revision: entry.revision,
            range: first.min(last)..=first.max(last),
        }
    }

    pub(super) fn matches(&self, entry: &filesystem::Entry) -> bool {
        self.path == entry.path && self.revision == entry.revision
    }

    pub(super) fn label(&self) -> String {
        let (first, last) = (self.range.start(), self.range.end());
        if first == last {
            format!("Page {first}")
        } else {
            format!("Pages {first}–{last}")
        }
    }

    pub fn export(self, context: &PreviewContext, cancel: &AtomicBool) -> Result<String, String> {
        let name = format!("pages-{}-{}", self.range.start(), self.range.end());
        preview_copy::copy_file(&self.path, &name, "pdf", |output| {
            self.write(context, output, cancel)
        })?;
        Ok(format!("Copied {} as PDF", self.label().to_lowercase()))
    }

    fn write(
        &self,
        context: &PreviewContext,
        output: &File,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        filesystem::check_revision(&self.path, self.revision)?;
        let source = crate::file_source::FileSource::capture(&self.path)?;
        preview_copy::read_range(context, &self.path, &source, cancel, |input| {
            little_clip_core::pdf::extract_pdf_pages(input, self.range.clone(), output, cancel)?;
            filesystem::check_revision(&self.path, self.revision)
        })
    }
}

impl Selection {
    pub(super) fn interact(
        &mut self,
        ui: &mut Ui,
        layout: &Layout<'_>,
        entry: &filesystem::Entry,
        row: &mut Response,
    ) -> bool {
        let interaction = ui.interact(
            layout.track(),
            Id::unique(("pdf_page_strip", &entry.path)),
            Sense::click_and_drag(),
        );
        let modified = ui.input(|input| {
            input.modifiers.ctrl || input.modifiers.command || input.modifiers.shift
        });
        let (dragging, range) = preview_copy::drag_range(ui, &interaction, focus_id(), modified);
        if let Some((a, b)) = range
            && let (Some(first), Some(last)) = (layout.page_at(a), layout.page_at(b))
        {
            self.selected = Some(Pages::new(entry, (first, last)));
        }
        let menu = self.menu(ui, layout, entry, &interaction, modified);
        if let Some((first, last)) = interaction.hover_pos().and_then(|point| layout.gap(point)) {
            interaction
                .clone()
                .on_hover_text(format!("Pages {first}–{last} · Click to enter a range"));
        }
        if !dragging && !menu {
            *row = row.union(interaction);
        }
        dragging || menu
    }

    fn menu(
        &mut self,
        ui: &mut Ui,
        layout: &Layout<'_>,
        entry: &filesystem::Entry,
        response: &Response,
        modified: bool,
    ) -> bool {
        let id = response.id.with("page_range_draft");
        let draft = response.interact_pointer_pos().and_then(|point| {
            let gap = layout.gap(point);
            let page = layout.page_at((point.x - response.rect.left()) / response.rect.width());
            if response.secondary_clicked() || (response.clicked() && !modified && gap.is_some()) {
                Some((page, gap.or_else(|| page.map(|page| (page, page)))?))
            } else {
                None
            }
        });
        if let Some(draft) = draft {
            ui.data_mut(|data| data.insert_temp(id, draft));
        }
        egui::Popup::menu(response)
            .open_memory(draft.map(|_| egui::SetOpenCommand::Bool(true)))
            .at_pointer_fixed()
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                let Some((page, mut endpoints)) =
                    ui.data(|data| data.get_temp::<(Option<usize>, (usize, usize))>(id))
                else {
                    return;
                };
                if let Some(page) = page {
                    if ui.button(format!("Copy page {page} as PDF")).clicked() {
                        let single = Pages::new(entry, (page, page));
                        self.requested = Some(single.clone());
                        self.selected = Some(single);
                        ui.close();
                    }
                    ui.separator();
                }
                range_fields(ui, layout.count, &mut endpoints);
                if ui.button("Select range").clicked() {
                    self.selected = Some(Pages::new(entry, endpoints));
                    ui.memory_mut(|memory| memory.request_focus(focus_id()));
                    ui.close();
                }
                ui.data_mut(|data| data.insert_temp(id, (page, endpoints)));
            });
        draft.is_some()
    }
}

fn range_fields(ui: &mut Ui, limit: usize, (first, last): &mut (usize, usize)) {
    ui.horizontal(|ui| {
        let from = ui.label("First page");
        ui.add(egui::DragValue::new(first).range(1..=limit))
            .labelled_by(from.id);
        let to = ui.label("Last page");
        ui.add(egui::DragValue::new(last).range(1..=limit))
            .labelled_by(to.id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};
    use egui_kittest::{Harness, kittest::Queryable};

    fn harness() -> Harness<'static, Selection> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pages.pdf"), b"identity fixture").unwrap();
        let entry = filesystem::read_directory(dir.path(), &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        Harness::builder()
            .with_size(vec2(700.0, 420.0))
            .build_ui_state(
                move |ui, selection: &mut Selection| {
                    let rect = Rect::from_min_size(pos2(80.0, 100.0), vec2(280.0, 66.0));
                    let layout = Layout {
                        rect,
                        slots: &[Some(1), Some(2), None, Some(14), Some(15)],
                        count: 15,
                    };
                    ui.interact(rect, focus_id(), Sense::focusable_noninteractive());
                    let mut row =
                        ui.interact(rect, Id::unique("test_pdf_row"), Sense::click_and_drag());
                    selection.interact(ui, &layout, &entry, &mut row);
                },
                Selection::default(),
            )
    }

    fn release(harness: &Harness<'_, Selection>, pos: Pos2) {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
    }

    #[test]
    fn reverse_drag_across_the_omitted_span_selects_every_page_and_keeps_focus() {
        let mut harness = harness();
        let start = pos2(325.0, 125.0);
        let end = pos2(160.0, 125.0);
        harness.hover_at(start);
        harness.step();
        harness.drag_at(start);
        harness.step();
        harness.hover_at(end);
        harness.step();
        release(&harness, end);
        harness.run();

        let selected = harness.state().selected.as_ref().unwrap();
        assert_eq!(selected.range, 2..=15);
        assert!(harness.ctx.memory(|memory| memory.has_focus(focus_id())));
    }

    #[test]
    fn clicking_the_ellipsis_opens_an_exact_range_for_the_omitted_pages() {
        let mut harness = harness();
        let gap = pos2(220.0, 125.0);
        harness.hover_at(gap);
        harness.step();
        harness.drag_at(gap);
        harness.step();
        release(&harness, gap);
        harness.run();
        harness.get_by_label("Select range").click();
        harness.run();
        assert_eq!(harness.state().selected.as_ref().unwrap().range, 3..=13);
    }
}
