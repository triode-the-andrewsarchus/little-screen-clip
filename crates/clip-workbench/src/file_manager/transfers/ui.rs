use egui::{Key, Modifiers, Ui};
use std::sync::atomic::Ordering;

use super::super::{FileManager, display_path};
use super::{Decision, Kind, Transfers};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};

impl FileManager {
    pub(crate) fn transfers_modal(&self) -> bool {
        self.transfers.confirmation.is_some()
            || self.transfers.trash.open
            || self.transfers.has_conflict()
    }

    pub(crate) fn transfer_toolbar(&mut self, ui: &mut Ui) {
        let selected = self.selection_count() > 0;
        let action = ui
            .horizontal_wrapped(|ui| {
                let action = selection_buttons(ui, selected, self.directory.is_some());
                if ui.button("Trash…").clicked() {
                    self.transfers.trash.show();
                }
                if ui.button("Clear held files").clicked() {
                    self.transfers.clipboard = None;
                }
                if ui.button(format!("Shelf ({})", self.shelf.len())).clicked() {
                    self.shelf.open = !self.shelf.open;
                }
                action
            })
            .inner;
        if let Some(action) = action {
            self.selection_action(action);
        }
    }

    pub(in crate::file_manager) fn selection_action(&mut self, action: SelectionAction) {
        match action {
            SelectionAction::Copy => self.copy_selection(false),
            SelectionAction::Cut => self.copy_selection(true),
            SelectionAction::Paste => self.paste_files(),
            SelectionAction::Trash => self.confirm_delete(false),
            SelectionAction::Delete => self.confirm_delete(true),
        }
    }

    pub(crate) fn transfer_status(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        if !self.filesystem_busy()
            && let Some(directory) = &self.directory
        {
            let artifacts = directory
                .entries
                .iter()
                .filter(|entry| {
                    let name = &entry.name;
                    name.starts_with(".little-clip-copy-")
                        || name.starts_with(".little-clip-replaced-")
                        || name.starts_with(".little-clip-moving-")
                })
                .count();
            if artifacts > 0 {
                ui.colored_label(colors.warning, format!("{artifacts} transfer artifacts remain. Enable hidden files to inspect staged copies or preserved originals."));
            }
        }
        self.transfers.status(ui, colors, !self.shelf.open);
    }

    pub(crate) fn transfer_windows(&mut self, ctx: &egui::Context, colors: WorkbenchColors) {
        self.transfers.conflict_ui(ctx, colors);
        self.transfers.confirmation_ui(ctx, colors);
        self.transfers.trash_ui(ctx, colors);
    }
}

impl Transfers {
    fn has_conflict(&self) -> bool {
        self.running.as_ref().is_some_and(|running| {
            running
                .control
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .conflict
                .is_some()
        })
    }

    fn status(&mut self, ui: &mut Ui, colors: WorkbenchColors, show_held: bool) {
        if let Some(running) = &self.running {
            let progress = running
                .control
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ui.horizontal(|ui| {
                ui.spinner();
                ui.add(
                    egui::Label::new(format!(
                        "{}: {}/{} · {} bytes · {}",
                        progress.title,
                        progress.done,
                        progress.total,
                        progress.bytes,
                        display_path(&progress.current)
                    ))
                    .truncate(true),
                );
                if ui.button("Cancel operation").clicked() {
                    running.control.cancelled.store(true, Ordering::Relaxed);
                    running.control.decision_ready.notify_all();
                }
            });
        }
        if !self.queue.is_empty() {
            ui.horizontal(|ui| {
                ui.label(format!("{} batches waiting", self.queue.len()));
                if ui.button("Cancel waiting").clicked() {
                    self.cancel_waiting();
                }
            });
        }
        if self.import.is_some() {
            ui.label("Reading desktop clipboard…");
        }
        if let Some(clipboard) = &self.clipboard
            && show_held
        {
            ui.label(format!(
                "{} files held for {}",
                clipboard.paths.len(),
                if clipboard.cut { "moving" } else { "copying" }
            ));
        }
        if let Some(notice) = &self.notice {
            ui.colored_label(colors.warning, notice);
        }
        for (index, report) in self.reports.iter().enumerate() {
            egui::CollapsingHeader::new(format!(
                "{}: {} completed, {} skipped, {} failed{}",
                report.title,
                report.completed.len(),
                report.skipped.len(),
                report.failed.len(),
                if report.cancelled { " (cancelled)" } else { "" }
            ))
            .id_source(("file_operation_report", index))
            .default_open(!report.failed.is_empty())
            .show(ui, |ui| {
                ui.label(format!("{} items unprocessed", report.unprocessed));
                for (path, error) in report.failed.iter().take(50) {
                    ui.add(
                        Icon::Alert.label(
                            egui::RichText::new(format!("{}: {error}", display_path(path)))
                                .color(colors.danger),
                        ),
                    );
                }
                for path in report.skipped.iter().take(50) {
                    ui.label(format!("Skipped {}", display_path(path)));
                }
            });
        }
    }

    fn conflict_ui(&mut self, ctx: &egui::Context, colors: WorkbenchColors) {
        let Some(running) = &self.running else {
            return;
        };
        let conflict = running
            .control
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .conflict
            .clone();
        let Some(conflict) = conflict else {
            return;
        };
        let mut decision = None;
        let mut cancel = false;
        egui::Window::new("Destination already exists").id(egui::Id::new("file_transfer_conflict"))
            .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Source: {}", display_path(&conflict.source)));
                ui.label(format!("Destination: {}", display_path(&conflict.destination)));
                ui.colored_label(colors.warning, "Replace discards the previous destination, including its contents if it is a folder. It cannot be undone here.");
                ui.checkbox(&mut self.apply_all, "Apply Replace or Skip to remaining conflicts in this batch");
                ui.horizontal(|ui| {
                    if ui.button("Replace").clicked() { decision = Some(Decision::Replace); }
                    if ui.button("Skip").clicked() { decision = Some(Decision::Skip); }
                    cancel = ui.button("Cancel batch").clicked();
                });
                ui.horizontal(|ui| {
                    ui.label("Another name:");
                    ui.text_edit_singleline(&mut self.conflict_name);
                    if ui.button("Use name").clicked() {
                        match super::super::edits::parse_name(&self.conflict_name) {
                            Ok(name) => decision = Some(Decision::Name(name)),
                            Err(error) => self.conflict_error = Some(error),
                        }
                    }
                });
                if let Some(error) = &self.conflict_error { ui.colored_label(colors.danger, error); }
            });
        cancel |= ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        if cancel {
            running.control.cancelled.store(true, Ordering::Relaxed);
        }
        let decided = decision.is_some();
        if let Some(decision) = decision {
            running
                .control
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .decision = Some((decision, self.apply_all));
            self.conflict_name.clear();
            self.conflict_error = None;
        }
        if cancel || decided {
            running.control.decision_ready.notify_all();
        }
    }

    fn confirmation_ui(&mut self, ctx: &egui::Context, colors: WorkbenchColors) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        let title = if confirmation.permanent {
            "Delete permanently?"
        } else {
            "Move to Trash?"
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("{} selected items", confirmation.paths.len()));
                if confirmation.permanent {
                    ui.colored_label(
                        colors.danger,
                        "This removes these files and folders permanently. There is no Undo.",
                    );
                }
                ui.label("Enter confirms; Escape cancels.");
                for path in confirmation.paths.iter().take(10) {
                    ui.label(display_path(path));
                }
                ui.horizontal(|ui| {
                    accept = ui
                        .button(if confirmation.permanent {
                            "Delete permanently"
                        } else {
                            "Move to Trash"
                        })
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        accept |= ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
        cancel |= ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        if accept {
            let confirmation = self.confirmation.take().expect("confirmed deletion");
            if self.enqueue(
                if confirmation.permanent {
                    Kind::Delete
                } else {
                    Kind::Trash
                },
                confirmation.paths,
            ) {
                self.queue
                    .back_mut()
                    .expect("confirmed batch was enqueued")
                    .expected = Some(confirmation.expected);
            }
        } else if cancel {
            self.confirmation = None;
        }
    }

    fn trash_ui(&mut self, ctx: &egui::Context, colors: WorkbenchColors) {
        if !self.trash.open {
            return;
        }
        let mut close = false;
        let mut restore = false;
        egui::Window::new("Trash").default_size([640.0, 420.0]).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() { self.trash.invalidate(); }
                    restore = ui.add_enabled(!self.trash.selected.is_empty(), egui::Button::new("Restore selected")).clicked();
                    close = ui.button("Close").clicked();
                });
                ui.label("Restore returns files to their original folders and refuses to overwrite existing entries.");
                if self.trash.pending.is_some() { ui.spinner(); }
                if let Some(error) = &self.trash.error { ui.colored_label(colors.danger, error); }
                egui::ScrollArea::vertical().max_height(320.0).show_rows(ui, 28.0, self.trash.items.len(), |ui, range| {
                    for index in range {
                        let item = &self.trash.items[index];
                        let mut selected = self.trash.selected.contains(&item.id);
                        if ui.checkbox(&mut selected, display_path(&item.original_path())).changed() {
                            if selected { self.trash.selected.insert(item.id.clone()); } else { self.trash.selected.remove(&item.id); }
                        }
                    }
                });
                if self.trash.items.is_empty() && self.trash.pending.is_none() { ui.label("Trash is empty."); }
            });
        close |= ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        if restore {
            let items: Vec<_> = self
                .trash
                .items
                .iter()
                .filter(|item| self.trash.selected.contains(&item.id))
                .cloned()
                .collect();
            let ids = items
                .iter()
                .map(|item| std::path::PathBuf::from(&item.id))
                .collect();
            self.enqueue(Kind::Restore(items), ids);
            self.trash.open = false;
        }
        if close {
            self.trash.open = false;
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::file_manager) enum SelectionAction {
    Copy,
    Cut,
    Paste,
    Trash,
    Delete,
}

pub(in crate::file_manager) fn selection_buttons(
    ui: &mut Ui,
    selected: bool,
    can_paste: bool,
) -> Option<SelectionAction> {
    let mut action = None;
    for (label, enabled, candidate) in [
        ("Copy", selected, SelectionAction::Copy),
        ("Cut", selected, SelectionAction::Cut),
        ("Paste", can_paste, SelectionAction::Paste),
        ("Move to Trash", selected, SelectionAction::Trash),
        ("Delete permanently…", selected, SelectionAction::Delete),
    ] {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            action = Some(candidate);
        }
    }
    action
}
