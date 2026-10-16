use egui::{Id, Key, Modifiers, Ui};
use std::path::Path;

use super::edits::NamingAction;
use super::ui::{focus_text, key, list_id};
use super::{FileManager, display_path};
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;

pub(super) fn naming_buttons(
    ui: &mut Ui,
    enabled: bool,
    rename: Option<&Path>,
) -> Option<NamingAction> {
    let mut action = None;
    if ui
        .add_enabled(enabled, egui::Button::new("New folder"))
        .on_hover_text("Create a folder [Ctrl+Shift+N]")
        .clicked()
    {
        action = Some(NamingAction::Folder);
    }
    if ui
        .add_enabled(enabled, egui::Button::new("New file"))
        .on_hover_text("Create an empty file")
        .clicked()
    {
        action = Some(NamingAction::File);
    }
    if ui
        .add_enabled(enabled && rename.is_some(), egui::Button::new("Rename"))
        .on_hover_text("Rename one selected item [F2]")
        .clicked()
    {
        action = rename.map(|path| NamingAction::Rename(path.into()));
    }
    action
}

impl FileManager {
    pub(super) fn naming_ui(&mut self, ctx: &egui::Context, colors: WorkbenchColors) {
        let Some(dialog) = &mut self.edits.dialog else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(dialog.action.title())
            .id(Id::new("file_manager_naming"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("In {}", display_path(&dialog.parent)));
                if let NamingAction::Content(content) = &dialog.action {
                    ui.label(format!("Captured {} · {} bytes", content.mime, content.bytes.len()));
                    ui.label("Creates a new file. Existing files are kept; choose another name on conflict.");
                }
                ui.label("Name:");
                let mut output = egui::TextEdit::singleline(&mut dialog.name)
                    .id(Id::new("file_manager_name"))
                    .desired_width(360.0)
                    .show(ui);
                if dialog.focus {
                    dialog.focus = false;
                    focus_text(ctx, &mut output, dialog.name.chars().count());
                }
                if output.response.has_focus() || output.response.lost_focus() {
                    submit = key(ctx, Modifiers::NONE, Key::Enter);
                }
                ui.label(
                    "One name; spaces are allowed. Literal backslash: \\\\; unusual bytes: \\xHH.",
                );
                if let Some(error) = &dialog.error {
                    ui.add(Icon::Alert.label(egui::RichText::new(error).color(colors.danger)));
                }
                ui.horizontal(|ui| {
                    submit |= ui
                        .button(if matches!(dialog.action, NamingAction::Rename(_)) {
                            "Rename"
                        } else {
                            "Create"
                        })
                        .clicked();
                    cancel |= ui.button("Cancel").clicked();
                });
            });
        cancel |= key(ctx, Modifiers::NONE, Key::Escape);
        if cancel {
            self.edits.dialog = None;
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
        } else if submit {
            self.submit_naming();
            if self.edits.dialog.is_none() {
                ctx.memory_mut(|memory| memory.request_focus(list_id()));
            }
        }
    }

    pub(super) fn edit_status(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        if self.edit_busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Applying file operation…");
            });
        }
        if let Some(report) = &self.edits.report {
            let dismiss = ui
                .horizontal_wrapped(|ui| {
                    let color = if report.failed {
                        colors.danger
                    } else {
                        colors.text_secondary
                    };
                    ui.add(
                        (if report.failed {
                            Icon::Alert
                        } else {
                            Icon::Folder
                        })
                        .label(egui::RichText::new(&report.text).color(color)),
                    );
                    ui.small_button("Dismiss").clicked()
                })
                .inner;
            if dismiss {
                self.edits.report = None;
            }
        }
    }
}
