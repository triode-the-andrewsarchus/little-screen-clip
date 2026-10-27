use super::{
    answer::{Answer, Applicability, State},
    columns::Columns,
};
use crate::{
    file_manager::{
        FileManager,
        filesystem::Entry,
        ui::{ListColumns, paint_text},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Id, Rect, Sense, Ui};

enum Action {
    Sort(u64),
    Remove(u64),
    Retry(u64),
    Refresh(u64),
}
impl FileManager {
    pub(in crate::file_manager) fn answer_columns_ui(&mut self, ui: &mut Ui) {
        let mut retry = None;
        if let Some(folder) = self
            .search
            .answers
            .current_mut()
            .filter(|folder| !folder.columns.is_empty())
        {
            ui.horizontal_wrapped(|ui| {
                ui.small("Session answers");
                for column in &folder.columns {
                    if ui
                        .selectable_label(
                            folder.focused == column.id,
                            format!("{} / {}", column.finished, column.total),
                        )
                        .on_hover_text(&column.title)
                        .clicked()
                    {
                        folder.focused = column.id;
                    }
                }
                if folder.paused {
                    ui.label("Answer model unavailable. Install it from Index, then retry.");
                    if ui.button("Retry").clicked() {
                        retry = Some(folder.focused);
                    }
                }
            });
            if folder
                .columns
                .iter()
                .any(|column| column.finished < column.total)
            {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if let Some(id) = retry {
            self.search.answers.retry(id, false);
            self.rebuild_visible();
        }
    }
    pub(super) fn answer_headers(
        &mut self,
        ui: &mut Ui,
        row: Rect,
        columns: ListColumns,
        offset: usize,
        colors: WorkbenchColors,
    ) {
        let mut action = None;
        if let Some(folder) = self.search.answers.current_mut() {
            for (index, column) in folder.columns.iter_mut().enumerate() {
                let Some(cell) = columns.extra_cell_at(row, index + offset) else {
                    continue;
                };
                let selected = folder.sort.filter(|sort| sort.0 == column.id);
                paint_text(
                    ui,
                    cell,
                    5.0,
                    cell.width() - 25.0,
                    &column.title,
                    if selected.is_some() {
                        colors.accent
                    } else {
                        colors.text_secondary
                    },
                );
                if let Some((_, descending)) = selected {
                    let icon = if descending {
                        crate::icons::Icon::ArrowDown
                    } else {
                        crate::icons::Icon::ArrowUp
                    };
                    icon.paint(
                        ui.painter(),
                        Rect::from_center_size(
                            egui::pos2(cell.right() - 12.0, cell.center().y),
                            egui::vec2(14.0, 14.0),
                        ),
                        colors.accent,
                    );
                }
                let response = ui.interact(cell, Id::new(("answer_header", column.id)), Sense::click()).on_hover_text(format!("{}\n{:?} · {} / {} rows complete\nClick to sort; right-click for column actions. Dates group by precision; numbers group by exact currency/unit.", column.title, column.query.answer_type, column.finished, column.total));
                if response.clicked() {
                    action = Some(Action::Sort(column.id));
                }
                response.context_menu(|ui| {
                    ui.label("Column title");
                    ui.text_edit_singleline(&mut column.title);
                    if ui.button("Retry missing/failed answers").clicked() {
                        action = Some(Action::Retry(column.id));
                        ui.close_menu();
                    }
                    if ui.button("Refresh all answers").clicked() {
                        action = Some(Action::Refresh(column.id));
                        ui.close_menu();
                    }
                    if ui.button("Remove column").clicked() {
                        action = Some(Action::Remove(column.id));
                        ui.close_menu();
                    }
                });
            }
        }
        match action {
            Some(Action::Sort(id)) => {
                if let Some(folder) = self.search.answers.current_mut() {
                    folder.sort = Some((id, folder.sort.is_some_and(|old| old.0 == id && !old.1)));
                    folder.focused = id;
                }
            }
            Some(Action::Remove(id)) => self.search.answers.remove(id),
            Some(Action::Retry(id)) => self.search.answers.retry(id, false),
            Some(Action::Refresh(id)) => self.search.answers.retry(id, true),
            None => return,
        }
        self.rebuild_visible();
    }
}
impl Columns {
    pub fn paint_cells(
        &self,
        ui: &mut Ui,
        columns: ListColumns,
        row: Rect,
        entry: &Entry,
        offset: usize,
        colors: WorkbenchColors,
    ) {
        let Some(folder) = self.current() else { return };
        for (index, column) in folder.columns.iter().enumerate() {
            let Some(cell) = columns.extra_cell_at(row, index + offset) else {
                continue;
            };
            let skipped = column.skips(entry).map(|reason| {
                Answer::status(
                    if reason.starts_with("Excluded") {
                        State::Excluded
                    } else {
                        State::Unsupported
                    },
                    reason,
                )
            });
            let answer = column.answer(entry).or(skipped.as_ref());
            paint_text(
                ui,
                cell,
                5.0,
                cell.width() - 10.0,
                answer.map_or("Pending…", Answer::label),
                colors.text_secondary,
            );
            ui.interact(
                cell,
                Id::new(("answer_cell", column.id, &entry.path)),
                Sense::hover(),
            )
            .on_hover_ui(|ui| {
                ui.set_max_width(460.0);
                if let Some(answer) = answer {
                    ui.small(match answer.applicability {
                        Applicability::Applicable => "Applies to this question",
                        Applicability::NotApplicable => "Not applicable to this question",
                        Applicability::Undetermined => "Applicability undetermined",
                    });
                    ui.label(&answer.reason);
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for (location, quote) in &answer.evidence {
                                ui.separator();
                                ui.strong(location);
                                ui.label(quote);
                            }
                        });
                } else {
                    ui.label("Waiting for bounded source extraction and local answering.");
                }
            });
        }
    }
}
