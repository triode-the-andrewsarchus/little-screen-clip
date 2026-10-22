use std::path::PathBuf;

use egui::{Color32, FontId, Id, Key, Modifiers, Rect, Sense, Ui, pos2, vec2};
use little_clip_core::palette::AppPalette;

use super::{FileManager, Navigation, display_path};
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext;

mod bookmarks;
mod gallery;
mod list;
mod mounts;
pub(super) mod music;
mod video;

const ROW_HEIGHT: f32 = 28.0;

fn address_id() -> Id {
    Id::new("file_manager_address")
}
pub(super) fn list_id() -> Id {
    Id::new("file_manager_list_focus")
}
fn tree_id() -> Id {
    Id::new("file_manager_tree_focus")
}

pub(super) fn key(ctx: &egui::Context, modifiers: Modifiers, key: Key) -> bool {
    ctx.input_mut(|input| input.consume_key(modifiers, key))
}

impl FileManager {
    pub(crate) fn handle_address_escape(&mut self, ctx: &egui::Context) {
        if self.edits.dialog.is_none()
            && !self.transfers_modal()
            && self.address_editing
            && key(ctx, Modifiers::NONE, Key::Escape)
        {
            self.address = display_path(self.path());
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
        }
    }

    pub(crate) fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        self.drop_targets.clear();
        self.bookmark_drop_rect = None;
        self.tick(super::BackgroundJobs::global());
        let colors = WorkbenchColors::from_palette(palette);
        self.sync_modal_focus(ui.ctx());
        self.shortcuts(ui.ctx());
        ui.set_enabled(self.edits.dialog.is_none() && !self.transfers_modal());
        self.toolbar(ui, palette);
        self.media_view_toolbar(ui, palette);
        ui.separator();
        self.docked_panels(ui, palette);
        ui.memory_mut(|memory| {
            for id in [
                tree_id(),
                list_id(),
                music::waveform_id(),
                super::midi_strip::focus_id(),
            ] {
                memory.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        escape: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            }
        });
        self.naming_ui(ui.ctx(), colors);
        self.transfer_windows(ui.ctx(), colors);
        let file_clipboard = ui.memory(|memory| {
            [
                list_id(),
                music::waveform_id(),
                super::midi_strip::focus_id(),
            ]
            .into_iter()
            .any(|id| memory.has_focus(id))
        }) && self.edits.dialog.is_none()
            && !self.transfers_modal();
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                egui::Id::new(egui_winit::CLIPBOARD_SHORTCUTS_AS_KEYS),
                file_clipboard,
            );
        });
        self.schedule_repaints(ui.ctx());
    }

    fn schedule_repaints(&self, ctx: &egui::Context) {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        if let Some(last) = &self.last_status {
            let elapsed = last.timestamp.elapsed();
            if elapsed < super::STATUS_MIN_DURATION {
                ctx.request_repaint_after(super::STATUS_MIN_DURATION - elapsed);
            }
        }
        ctx.request_repaint_after(if self.shelf.open {
            std::time::Duration::from_millis(500)
        } else {
            std::time::Duration::from_secs(3)
        });
        if self.pending.is_some()
            || self.bookmarks.busy()
            || self.mounts.busy()
            || self.edit_busy()
            || self.fuse_mounts.busy()
            || self.transfers.needs_repaint()
            || self.shelf.busy()
            || self.watch.is_some()
            || self.tree.branches.values().any(|branch| {
                branch.expanded && branch.directory.is_none() && branch.error.is_none()
            })
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn copy_strip_selection(&mut self, ctx: &egui::Context) {
        if ctx.memory(|memory| memory.has_focus(super::midi_strip::focus_id())) {
            self.midi.copy_selection(ctx.input(|input| input.time));
        } else {
            self.copy_music_range();
        }
    }
    fn docked_panels(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        if self.shelf.open {
            let height = (ui.available_height() * 0.38).clamp(144.0, 224.0);
            egui::TopBottomPanel::bottom("file_manager_shelf")
                .exact_height(height)
                .frame(egui::Frame::none().fill(colors.recessed).inner_margin(8.0))
                .show_inside(ui, |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    self.shelf_ui(ui, palette);
                });
        }
        let last_active = self
            .last_status
            .as_ref()
            .is_some_and(|status| status.is_active(super::STATUS_MIN_DURATION));
        if self.filesystem_busy()
            || self.fuse_mounts.busy()
            || self.mounts.busy()
            || self.error.is_some()
            || self.transfers.has_status()
            || self.edits.report.is_some()
            || last_active
            || self
                .directory
                .as_ref()
                .is_some_and(|directory| directory.skipped > 0)
        {
            egui::TopBottomPanel::bottom("file_manager_status")
                .default_height(56.0)
                .max_height(150.0)
                .frame(egui::Frame::none().inner_margin(6.0))
                .show_inside(ui, |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(0.0)
                        .max_height(ui.available_height())
                        .show(ui, |ui| self.status(ui, colors));
                });
        }
        egui::SidePanel::left("file_manager_folders")
            .default_width(240.0)
            .width_range(150.0..=400.0)
            .resizable(true)
            .show_inside(ui, |ui| self.sidebar_ui(ui, colors));
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(8.0))
            .show_inside(ui, |ui| {
                if ui.is_enabled() {
                    self.drop_targets.push((
                        ui.available_rect_before_wrap().intersect(ui.clip_rect()),
                        self.path().to_path_buf(),
                    ));
                }
                if self.view == super::BrowserView::Photos {
                    self.gallery_ui(ui, colors, palette);
                } else if self.view == super::BrowserView::Music {
                    self.music_ui(ui, colors);
                } else if self.view == super::BrowserView::Videos {
                    self.video_ui(ui, colors);
                } else {
                    self.list_ui(ui, colors, palette);
                }
            });
    }

    fn sync_modal_focus(&mut self, ctx: &egui::Context) {
        let modal = self.edits.dialog.is_some() || self.transfers_modal();
        if self.was_modal && !modal {
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
        }
        self.was_modal = modal;
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.edits.dialog.is_some() || self.transfers_modal() {
            self.type_navigation.clear();
            return;
        }
        self.navigation_shortcuts(ctx);
        self.type_navigation_keys(ctx);
        if ctx.memory(|memory| memory.has_focus(address_id())) {
            return;
        }
        self.selection_shortcuts(ctx);
        if key(
            ctx,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            Key::N,
        ) {
            self.begin_naming(super::edits::NamingAction::Folder);
        }
        if ctx.memory(|memory| memory.has_focus(tree_id())) {
            self.tree_shortcuts(ctx);
        } else if ctx.memory(|memory| {
            [super::midi_strip::focus_id(), music::waveform_id()]
                .into_iter()
                .any(|id| memory.has_focus(id))
        }) {
            if key(ctx, Modifiers::CTRL, Key::C) {
                self.copy_strip_selection(ctx);
            }
            if key(ctx, Modifiers::CTRL, Key::V) {
                self.paste_files();
            }
        } else if ctx.memory(|memory| memory.has_focus(list_id())) {
            self.list_shortcuts(ctx);
            if self.edits.dialog.is_some() {
                return;
            }
            if key(ctx, Modifiers::NONE, Key::ArrowDown) {
                self.move_selection(true, false);
            }
            if key(ctx, Modifiers::NONE, Key::ArrowUp) {
                self.move_selection(false, false);
            }
            if key(ctx, Modifiers::NONE, Key::Enter) {
                self.open_selected();
            }
        }
    }

    fn navigation_shortcuts(&mut self, ctx: &egui::Context) {
        if key(ctx, Modifiers::CTRL, Key::L) {
            self.focus_address = true;
        }
        if key(ctx, Modifiers::ALT, Key::ArrowLeft)
            || (!popup_open(ctx)
                && ctx.input(|input| input.pointer.button_pressed(egui::PointerButton::Extra1)))
        {
            self.history_step(false);
        }
        if key(ctx, Modifiers::ALT, Key::ArrowRight)
            || (!popup_open(ctx)
                && ctx.input(|input| input.pointer.button_pressed(egui::PointerButton::Extra2)))
        {
            self.history_step(true);
        }
        if key(ctx, Modifiers::ALT, Key::ArrowUp) {
            self.parent();
        }
        if key(ctx, Modifiers::NONE, Key::F5) {
            self.refresh();
        }
        if key(ctx, Modifiers::CTRL, Key::H) {
            self.toggle_hidden();
        }
    }

    fn list_shortcuts(&mut self, ctx: &egui::Context) {
        if self.view == super::BrowserView::Photos {
            for (key_code, delta) in [(Key::ArrowLeft, -1), (Key::ArrowRight, 1)] {
                if key(ctx, Modifiers::NONE, key_code) {
                    self.move_selection_by(delta, false);
                } else if key(ctx, Modifiers::SHIFT, key_code) {
                    self.move_selection_by(delta, true);
                }
            }
        }
        self.transfer_shortcuts(ctx);
        if key(ctx, Modifiers::CTRL, Key::A) {
            self.select_all();
        }
        if key(ctx, Modifiers::NONE, Key::Escape) {
            self.clear_selection();
        }
        if key(ctx, Modifiers::NONE, Key::F2) {
            self.begin_rename();
        }
        if key(ctx, Modifiers::SHIFT, Key::ArrowDown) {
            self.move_selection(true, true);
        }
        if key(ctx, Modifiers::SHIFT, Key::ArrowUp) {
            self.move_selection(false, true);
        }
    }

    fn transfer_shortcuts(&mut self, ctx: &egui::Context) {
        if key(ctx, Modifiers::CTRL, Key::C) {
            self.copy_selection(false);
        }
        if key(ctx, Modifiers::CTRL, Key::X) {
            self.copy_selection(true);
        }
        if key(ctx, Modifiers::CTRL, Key::V) {
            self.paste_files();
        }
        if key(ctx, Modifiers::SHIFT, Key::Delete) {
            self.confirm_delete(true);
        }
        if key(ctx, Modifiers::NONE, Key::Delete) {
            self.confirm_delete(false);
        }
    }

    fn tree_shortcuts(&mut self, ctx: &egui::Context) {
        let rows = self.tree.rows(self.show_hidden);
        if rows.is_empty() {
            return;
        }
        let index = rows
            .iter()
            .position(|row| Some(&row.path) == self.tree.selected.as_ref())
            .unwrap_or(0);
        let target = if key(ctx, Modifiers::NONE, Key::ArrowDown) {
            Some((index + 1).min(rows.len() - 1))
        } else if key(ctx, Modifiers::NONE, Key::ArrowUp) {
            Some(index.saturating_sub(1))
        } else {
            None
        };
        if let Some(target) = target {
            self.tree.selected = Some(rows[target].path.clone());
            self.tree.reveal = true;
        }
        let path = self
            .tree
            .selected
            .clone()
            .unwrap_or_else(|| rows[0].path.clone());
        if key(ctx, Modifiers::NONE, Key::Enter) {
            self.navigate(path.clone(), Navigation::New);
        }
        let expanded = self
            .tree
            .branches
            .get(&path)
            .is_some_and(|branch| branch.expanded);
        if (key(ctx, Modifiers::NONE, Key::ArrowRight) && !expanded)
            || (key(ctx, Modifiers::NONE, Key::ArrowLeft) && expanded)
        {
            self.tree.toggle(path);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        ui.horizontal(|ui| {
            if nav_button(ui, Icon::ArrowLeft, "Back [Alt+Left]", self.index.is_some_and(|i| i > 0), palette) { self.history_step(false); }
            if nav_button(ui, Icon::ArrowRight, "Forward [Alt+Right]", self.index.is_some_and(|i| i + 1 < self.history.len()), palette) { self.history_step(true); }
            if nav_button(ui, Icon::ArrowUp, "Parent folder [Alt+Up]", self.path().parent().is_some(), palette) { self.parent(); }
            let address_width = (ui.available_width() - 180.0).max(80.0);
            let mut output = egui::TextEdit::singleline(&mut self.address)
                .id(address_id()).hint_text("Directory path [Ctrl+L]")
                .desired_width(address_width).show(ui);
            output.response.clone().on_hover_text("Enter a directory path. Relative paths and ~/ are supported. Backslashes: \\\\; unusual bytes: \\xHH.");
            if self.focus_address {
                self.focus_address = false;
                focus_text(ui.ctx(), &mut output, self.address.chars().count());
            }
            if output.response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                self.submit_address();
                ui.memory_mut(|memory| memory.request_focus(list_id()));
            } else if output.response.lost_focus() {
                self.address = display_path(self.path());
            }
            self.address_editing = ui.memory(|memory| memory.has_focus(address_id()));
            if nav_button(ui, Icon::Repeat, "Refresh [F5]", true, palette) { self.refresh(); }
            if ui.add(ui_ext::secondary_button(
                if self.show_hidden { "Hidden: on" } else { "Hidden: off" }, colors.panel, palette,
            )).on_hover_text("Show hidden files [Ctrl+H]").clicked() { self.toggle_hidden(); }
        });
    }

    pub(super) fn selection_summary(&self) -> String {
        let selection = self.location().map(|location| &location.selection);
        let selected = selection.is_some_and(|paths| !paths.is_empty());
        let entries = self
            .directory
            .iter()
            .flat_map(|directory| self.visible.iter().map(|&index| &directory.entries[index]))
            .filter(|entry| {
                !selected || selection.is_some_and(|paths| paths.contains(&entry.path))
            });
        let mut folders = 0;
        let mut files = 0;
        let mut bytes = 0_u64;
        for entry in entries {
            if entry.directory {
                folders += 1;
            } else {
                files += 1;
                bytes = bytes.saturating_add(entry.size_bytes);
            }
        }
        let mut parts = Vec::new();
        if folders > 0 {
            parts.push(format!(
                "{folders} {}",
                if folders == 1 { "folder" } else { "folders" }
            ));
        }
        if files > 0 {
            parts.push(format!(
                "{files} {} ({})",
                if files == 1 { "file" } else { "files" },
                super::filesystem::format_size(bytes)
            ));
        }
        if parts.is_empty() {
            return "Empty folder".into();
        }
        if selected && files == 0 {
            return format!(
                "{folders} {}",
                if folders == 1 { "Folder" } else { "Folders" }
            );
        }
        format!(
            "{}{}",
            if selected { "Selection: " } else { "" },
            parts.join(" | ")
        )
    }

    fn has_active_status(&self) -> bool {
        self.error.is_some()
            || self.edits.report.is_some()
            || self.pending.is_some()
            || self.fuse_mounts.status_text().is_some()
            || self.mounts.status_text().is_some()
            || self.edit_busy()
            || self.transfers.has_status()
    }

    fn pending_status_ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(request) = &self.pending {
                ui.spinner();
                ui.add(
                    egui::Label::new(format!("Opening {}…", display_path(&request.path)))
                        .truncate(true),
                );
                if ui.small_button("Cancel").clicked() {
                    self.pending = None;
                    self.address = display_path(self.path());
                    if self.directory.is_none() {
                        self.set_error("Opening cancelled. Enter another path or retry.");
                    }
                }
            }
            let status = self
                .fuse_mounts
                .status_text()
                .or_else(|| self.mounts.status_text().map(str::to_owned));
            if let Some(mount_status) = status {
                ui.spinner();
                ui.add(egui::Label::new(mount_status).truncate(true));
            }
        });
    }

    fn error_status_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let Some(error) = &self.error else {
            return;
        };
        if self.last_status.as_ref().map(|s| &s.text) != Some(error) {
            self.last_status = Some(super::LastStatus::new(Icon::Alert, error.clone(), true));
        }
        let (retry, dismiss) = ui
            .horizontal_wrapped(|ui| {
                ui.add(Icon::Alert.label(egui::RichText::new(error).color(colors.danger)));
                let retry = ui.small_button("Retry").clicked();
                let dismiss = ui.small_button("Dismiss").clicked();
                (retry, dismiss)
            })
            .inner;
        if retry {
            self.retry();
        }
        if dismiss {
            self.clear_status();
        }
    }

    fn last_status_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        if self.has_active_status() {
            return;
        }
        let Some(last) = &self.last_status else {
            return;
        };
        if !last.is_active(super::STATUS_MIN_DURATION) {
            return;
        }
        let color = if last.is_error {
            colors.danger
        } else {
            colors.text_secondary
        };
        let dismiss = ui
            .horizontal_wrapped(|ui| {
                ui.add(
                    last.icon
                        .label(egui::RichText::new(&last.text).color(color)),
                );
                ui.small_button("Dismiss").clicked()
            })
            .inner;
        if dismiss {
            self.clear_status();
        }
    }

    fn status(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        self.pending_status_ui(ui);
        self.error_status_ui(ui, colors);
        self.edit_status(ui, colors);
        self.transfer_status(ui, colors);
        self.last_status_ui(ui, colors);
        if let Some(directory) = &self.directory
            && directory.skipped > 0
        {
            ui.colored_label(
                colors.warning,
                format!(
                    "{} entries could not be read; refresh to retry",
                    directory.skipped
                ),
            );
        }
    }

    fn finish_listing(
        &mut self,
        ui: &Ui,
        mut actions: ListActions,
        scroll: f32,
        background: &egui::Response,
    ) {
        if let Some(location) = self.location_mut() {
            location.scroll = scroll;
        }
        if actions.transfer.is_none() {
            actions.transfer = drop_files(ui, background, self.path());
        }
        self.apply_list_actions(actions);
    }

    fn apply_list_actions(&mut self, actions: ListActions) {
        if actions.selection.is_some() || actions.clear {
            self.type_navigation.clear();
        }
        if let Some((paths, destination, moving)) = actions.transfer {
            self.queue_transfer(paths, destination, moving);
        }
        if let Some((path, additive, range)) = actions.selection {
            self.select_with(path, additive, range);
        } else if actions.clear {
            self.clear_selection();
        }
        if actions.open_trash {
            self.open_trash();
        }
        if actions.select_all {
            self.select_all();
        }
        if let Some(action) = actions.files {
            self.selection_action(action);
        }
        if let Some(action) = actions.naming {
            self.begin_naming(action);
        }
        if let Some(path) = actions.inspect {
            self.inspect_file = Some(path.clone());
            self.open_file = Some(path);
        }
        if let Some(path) = actions.open {
            self.open_path(path);
        }
        if let Some(folder) = actions.navigate_folder {
            self.navigate(folder, Navigation::New);
        }
        if let Some((binary_path, writable)) = actions.mount_binary {
            let label = binary_path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            self.set_status(
                Icon::Hourglass,
                format!("Mounting '{label}' via fuse-binwalk…"),
                false,
            );
            self.fuse_mounts
                .request_mount(binary_path, writable, super::BackgroundJobs::global());
        }
        if let Some(mount_point) = actions.unmount_binary {
            self.set_status(Icon::Hourglass, "Unmounting binary contents…", false);
            self.fuse_mounts
                .request_unmount(mount_point, super::BackgroundJobs::global());
        }
        if let Some(path) = actions.search_similar {
            self.set_view(super::BrowserView::Photos);
            self.photo_mode = super::gallery::similarity::PhotoMode::Similarity;
            if let Some(directory) = &self.directory {
                self.similarity_canvas.sync(
                    directory,
                    &self.visible,
                    self.photo_recurse,
                    self.show_hidden,
                );
            }
            self.similarity_canvas.search_from_photo(&path);
        }
    }

    fn list_scroll(&mut self, row_height: f32) -> egui::ScrollArea {
        let mut scroll = egui::ScrollArea::vertical()
            .id_source("file_manager_listing")
            .auto_shrink([false, false]);
        if self.restore_scroll {
            scroll = scroll
                .vertical_scroll_offset(self.location().map_or(0.0, |location| location.scroll));
            self.restore_scroll = false;
        }
        if self.reveal_selection {
            if let Some(index) = self.visible.iter().position(|i| {
                self.location()
                    .and_then(|location| location.selected.as_ref())
                    == self
                        .directory
                        .as_ref()
                        .map(|directory| &directory.entries[*i].path)
            }) {
                scroll = scroll.vertical_scroll_offset(if self.view == super::BrowserView::Music {
                    self.music_offsets[index]
                } else if self.view == super::BrowserView::Videos {
                    self.video_offsets[index]
                } else {
                    self.list_offsets
                        .get(index)
                        .copied()
                        .unwrap_or(index as f32 * row_height)
                });
            }
            self.reveal_selection = false;
        }
        scroll
    }

    fn tree_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        ui.heading("Folders");
        ui.separator();
        let rows = self.tree.rows(self.show_hidden);
        let mut scroll = egui::ScrollArea::both()
            .min_scrolled_height(0.0)
            .id_source("file_manager_tree")
            .auto_shrink([false, false]);
        if self.tree.reveal
            && let Some(index) = rows
                .iter()
                .position(|row| Some(&row.path) == self.tree.selected.as_ref())
        {
            scroll = scroll
                .vertical_scroll_offset(index as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y));
            self.tree.reveal = false;
        }
        let mut action = None;
        let focus = ui.interact(ui.available_rect_before_wrap(), tree_id(), Sense::click());
        if focus.clicked() {
            focus.request_focus();
            ui.ctx().request_repaint();
        }
        scroll.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
            for index in range {
                if let Some(row_action) = self.tree_row(ui, &rows[index], index, colors) {
                    action = Some(row_action);
                }
            }
        });
        match action {
            Some(TreeAction::Toggle(path)) => self.tree.toggle(path),
            Some(TreeAction::Open(path)) => {
                self.tree.selected = Some(path.clone());
                self.navigate(path, Navigation::New);
            }
            Some(TreeAction::Transfer(paths, destination, moving)) => {
                self.queue_transfer(paths, destination, moving)
            }
            None => {}
        }
    }
    fn tree_row(
        &mut self,
        ui: &mut Ui,
        row: &super::tree::TreeRow,
        index: usize,
        colors: WorkbenchColors,
    ) -> Option<TreeAction> {
        if ui.is_enabled() {
            self.drop_targets.push((
                Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), ROW_HEIGHT))
                    .intersect(ui.clip_rect()),
                row.path.clone(),
            ));
        }
        let branch = self.tree.branches.get(&row.path);
        let expanded = branch.is_some_and(|branch| branch.expanded);
        let indent = row.depth as f32 * 16.0;
        let (rect, _) = ui.allocate_exact_size(
            vec2(ui.available_width().max(indent + 180.0), ROW_HEIGHT),
            Sense::hover(),
        );
        let arrow_rect = Rect::from_min_size(rect.min + vec2(indent, 0.0), vec2(26.0, ROW_HEIGHT));
        let arrow = ui.interact(
            arrow_rect,
            Id::new(("file_tree_arrow", index)),
            Sense::click(),
        );
        let body = Rect::from_min_max(pos2(arrow_rect.max.x, rect.min.y), rect.max);
        let response = ui.interact(body, Id::new(("file_tree_row", index)), Sense::click());
        row_background(
            ui,
            rect,
            row.path == self.path(),
            response.hovered(),
            colors,
        );
        if self.tree.selected.as_ref() == Some(&row.path)
            && ui.memory(|memory| memory.has_focus(tree_id()))
        {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                3.0,
                egui::Stroke::new(1.0_f32, colors.focus),
            );
        }
        (if expanded {
            Icon::ArrowDown
        } else {
            Icon::ArrowRight
        })
        .paint(
            ui.painter(),
            arrow_rect.shrink2(vec2(5.0, 6.0)),
            colors.text_secondary,
        );
        Icon::Folder.paint(
            ui.painter(),
            Rect::from_min_size(body.min + vec2(0.0, 5.0), vec2(18.0, 18.0)),
            colors.text_secondary,
        );
        let label = if row.home {
            "Home".into()
        } else if row.path.parent().is_none() {
            "Filesystem /".into()
        } else {
            display_path(std::path::Path::new(
                row.path.file_name().unwrap_or_default(),
            ))
        };
        paint_text(
            ui,
            body,
            24.0,
            body.width() - 24.0,
            &label,
            colors.text_primary,
        );
        if let Some(branch) = branch {
            let detail =
                branch
                    .error
                    .as_deref()
                    .unwrap_or(if expanded && branch.directory.is_none() {
                        "Loading folder…"
                    } else {
                        ""
                    });
            if !popup_open(ui.ctx()) {
                response
                    .clone()
                    .on_hover_text(format!("{}\n{detail}", display_path(&row.path)));
            }
            if branch.error.is_some() {
                Icon::Alert.paint(
                    ui.painter(),
                    Rect::from_min_size(rect.max - vec2(20.0, 23.0), vec2(16.0, 16.0)),
                    colors.danger,
                );
            }
        }
        if let Some((paths, destination, moving)) = drop_files(ui, &response, &row.path) {
            return Some(TreeAction::Transfer(paths, destination, moving));
        }
        if arrow.clicked() {
            return Some(TreeAction::Toggle(row.path.clone()));
        }
        if response.clicked() || response.double_clicked() {
            ui.memory_mut(|memory| memory.request_focus(tree_id()));
            ui.ctx().request_repaint();
            return Some(TreeAction::Open(row.path.clone()));
        }
        None
    }
}

fn nav_button(ui: &mut Ui, icon: Icon, tooltip: &str, enabled: bool, palette: &AppPalette) -> bool {
    ui.add_enabled(
        enabled,
        ui_ext::icon_button(
            icon,
            ui_ext::CONTROL_ICON_SIZE,
            WorkbenchColors::from_palette(palette).panel,
            None,
            palette,
        ),
    )
    .on_hover_text(tooltip)
    .clicked()
}

fn entry_icon(entry: &super::filesystem::Entry) -> Icon {
    if entry.directory {
        Icon::Folder
    } else if little_clip_core::audio::is_audio_path(&entry.path) {
        Icon::Audio
    } else if entry.is_video() {
        Icon::Film
    } else {
        Icon::Text
    }
}

fn current_selection(
    history: &[super::Location],
    index: Option<usize>,
) -> Option<&std::collections::BTreeSet<PathBuf>> {
    index
        .and_then(|index| history.get(index))
        .map(|location| &location.selection)
}

fn media_row(ui: &mut Ui, rect: Rect, selected: bool, colors: WorkbenchColors) -> egui::Response {
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    row_background(ui, rect, selected, response.hovered(), colors);
    response
}

fn row_background(ui: &Ui, rect: Rect, selected: bool, hovered: bool, colors: WorkbenchColors) {
    if selected || hovered {
        ui.painter().rect_filled(
            rect,
            3.0,
            if selected {
                colors.accent.gamma_multiply(0.2)
            } else {
                colors.raised
            },
        );
    }
}

/// Fits a scene thumbnail into its box, with the shared model icon as fallback.
pub(super) fn model_thumbnail(
    ui: &Ui,
    rect: Rect,
    texture: Option<&egui::TextureHandle>,
    colors: WorkbenchColors,
) {
    ui.painter().rect_filled(rect, 3.0, colors.recessed);
    if let Some(texture) = texture {
        let size = texture.size_vec2();
        let scale = (rect.width() / size.x).min(rect.height() / size.y);
        ui.painter().image(
            texture.id(),
            Rect::from_center_size(rect.center(), size * scale),
            Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        Icon::Model3d.paint(
            ui.painter(),
            Rect::from_center_size(rect.center(), vec2(32.0, 32.0)),
            colors.text_secondary,
        );
    }
}

/// Returns whether any popup or context menu is currently open or opening.
pub(super) fn popup_open(ctx: &egui::Context) -> bool {
    let reload_error = Id::new("appearance_reload_error");
    ctx.input(|input| input.pointer.secondary_clicked() || input.pointer.secondary_down())
        || ctx.memory(|memory| {
            memory.any_popup_open()
                || memory
                    .areas()
                    .visible_layer_ids()
                    .into_iter()
                    .any(|layer| layer.order == egui::Order::Foreground && layer.id != reload_error)
        })
}

/// Restricts passive preview hover to the enabled, visible portion of its row.
pub(super) fn preview_hovered(ui: &Ui, rect: Rect) -> bool {
    ui.is_enabled()
        && !popup_open(ui.ctx())
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|point| rect.intersect(ui.clip_rect()).contains(point))
}

/// Paints a file-type icon and reserves the shared strip area beside it.
pub(super) fn strip_body(ui: &Ui, row: Rect, icon: Icon, colors: WorkbenchColors) -> Rect {
    let thumbnail = Rect::from_min_size(row.min + vec2(6.0, 32.0), vec2(64.0, 66.0));
    icon.paint(ui.painter(), thumbnail.shrink(18.0), colors.text_secondary);
    Rect::from_min_max(
        pos2(thumbnail.right() + 8.0, thumbnail.top()),
        row.max - vec2(6.0, 10.0),
    )
}

/// Lays out numbered strip items and paints omitted middle items consistently.
pub(super) fn strip_cells(
    ui: &Ui,
    strip: Rect,
    slots: Vec<Option<usize>>,
    width: f32,
    colors: WorkbenchColors,
) -> Vec<(usize, Rect)> {
    slots
        .into_iter()
        .enumerate()
        .filter_map(|(position, ordinal)| {
            let cell = Rect::from_min_size(
                strip.min + vec2(position as f32 * width, 0.0),
                vec2(width, strip.height()),
            );
            match ordinal {
                Some(ordinal) => Some((ordinal, cell)),
                None => {
                    ui.painter().text(
                        cell.center(),
                        egui::Align2::CENTER_CENTER,
                        "…",
                        FontId::proportional(20.0),
                        colors.text_secondary,
                    );
                    None
                }
            }
        })
        .collect()
}

/// Absolute table positions shared by the header and every mixed List row.
#[derive(Clone, Copy)]
pub(super) struct ListColumns {
    size: f32,
    modified: f32,
    right: f32,
}

impl ListColumns {
    fn new(header: Rect) -> Self {
        let name_width = (header.width() - 245.0).max(header.width() * 0.4);
        Self {
            size: header.left() + name_width,
            modified: header.left() + name_width + 90.0,
            right: header.right(),
        }
    }

    fn paint(&self, ui: &Ui, row: Rect, size: &str, modified: &str, color: Color32) {
        paint_text(ui, row, self.size - row.left(), 85.0, size, color);
        paint_text(
            ui,
            row,
            self.modified - row.left(),
            self.right - self.modified,
            modified,
            color,
        );
    }

    fn sort_header(
        &self,
        ui: &mut Ui,
        header: Rect,
        sort: super::sorting::Sort,
        colors: WorkbenchColors,
    ) -> Option<super::sorting::Column> {
        use super::sorting::Column;
        let mut clicked = None;
        for (column, label, left, right) in [
            (
                Column::Type,
                "Type",
                header.left() + 8.0,
                header.left() + 64.0,
            ),
            (Column::Name, "Name", header.left() + 64.0, self.size),
            (Column::Size, "Size", self.size, self.modified),
            (Column::Modified, "Modified", self.modified, self.right),
        ] {
            let cell = Rect::from_min_max(pos2(left, header.top()), pos2(right, header.bottom()));
            let response = ui
                .interact(cell, Id::new(("files_sort", column)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("Sort by {label}; click again to reverse"));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, label));
            if response.hovered() {
                ui.painter().rect_filled(cell, 2.0, colors.recessed);
            }
            let active = sort.column == column;
            let color = if active {
                colors.accent
            } else {
                colors.text_secondary
            };
            paint_text(ui, cell, 0.0, cell.width() - 18.0, label, color);
            if active {
                let width = ui
                    .painter()
                    .layout_no_wrap(label.into(), FontId::proportional(14.0), color)
                    .size()
                    .x;
                let arrow = Rect::from_center_size(
                    pos2(cell.left() + width + 10.0, cell.center().y),
                    vec2(14.0, 14.0),
                );
                (if sort.descending {
                    Icon::ArrowDown
                } else {
                    Icon::ArrowUp
                })
                .paint(
                    &ui.painter().with_clip_rect(cell.intersect(ui.clip_rect())),
                    arrow,
                    color,
                );
            }
            if response.clicked() {
                clicked = Some(column);
            }
        }
        clicked
    }
}

pub(super) fn media_header(
    ui: &Ui,
    mut header: Rect,
    entry: &super::filesystem::Entry,
    details: Option<String>,
    columns: Option<ListColumns>,
    colors: WorkbenchColors,
) {
    if let Some(columns) = columns {
        columns.paint(
            ui,
            header,
            &entry.size,
            &entry.modified,
            colors.text_secondary,
        );
        header.max.x = (columns.size - 8.0).max(header.left());
    }
    let details_width = details.as_ref().map_or(0.0, |details| {
        ui.painter()
            .layout_no_wrap(
                details.clone(),
                FontId::proportional(14.0),
                colors.text_secondary,
            )
            .size()
            .x
            .min(header.width() * 0.7)
    });
    let name_width = (header.width() - details_width - 12.0).max(0.0);
    paint_text(
        ui,
        header,
        0.0,
        name_width,
        &entry.name,
        colors.text_primary,
    );
    if let Some(details) = &details {
        paint_text(
            ui,
            header,
            header.width() - details_width,
            details_width,
            details,
            colors.text_secondary,
        );
    }
    if !popup_open(ui.ctx()) {
        ui.interact(
            header,
            Id::new(("media_metadata", &entry.path)),
            Sense::hover(),
        )
        .on_hover_text(details.map_or_else(
            || entry.name.clone(),
            |details| format!("{}\n{}", entry.name, details),
        ));
    }
}

fn paint_text(ui: &Ui, rect: Rect, x: f32, width: f32, text: &str, color: Color32) {
    let clip = Rect::from_min_max(
        rect.min + vec2(x, 0.0),
        pos2(rect.min.x + x + width.max(0.0), rect.max.y),
    );
    ui.painter()
        .with_clip_rect(clip.intersect(ui.clip_rect()))
        .text(
            pos2(clip.min.x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            FontId::proportional(14.0),
            color,
        );
}

fn file_row(
    ui: &mut Ui,
    rect: Rect,
    entry: &super::filesystem::Entry,
    selected: bool,
    columns: ListColumns,
    count: Option<String>,
    colors: WorkbenchColors,
) -> egui::Response {
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let response = if rect.height() <= ROW_HEIGHT && !popup_open(ui.ctx()) {
        response.on_hover_text(format!(
            "{}{}",
            display_path(&entry.path),
            entry
                .warning
                .as_ref()
                .map(|warning| format!("\nMetadata unavailable: {warning}"))
                .unwrap_or_default()
        ))
    } else {
        response
    };
    row_background(ui, rect, selected, response.hovered(), colors);
    let rect = Rect::from_min_size(rect.min, vec2(rect.width(), ROW_HEIGHT));
    let name_width = columns.size - rect.left();
    let icon = if entry.symlink {
        Icon::Link
    } else if entry.directory {
        Icon::Folder
    } else {
        Icon::Text
    };
    if entry.directory {
        icon.paint(
            ui.painter(),
            Rect::from_min_size(rect.min + vec2(6.0, 5.0), vec2(18.0, 18.0)),
            colors.text_secondary,
        );
    }
    paint_text(
        ui,
        rect,
        32.0,
        name_width - 38.0 - if count.is_some() { 108.0 } else { 0.0 },
        &entry.name,
        colors.text_primary,
    );
    if let Some(count) = count {
        paint_text(
            ui,
            rect,
            name_width - 112.0,
            104.0,
            &count,
            colors.text_secondary,
        );
    }
    columns.paint(
        ui,
        rect,
        &entry.size,
        &entry.modified,
        colors.text_secondary,
    );
    response
}

enum TreeAction {
    Toggle(std::path::PathBuf),
    Open(std::path::PathBuf),
    Transfer(Vec<PathBuf>, PathBuf, bool),
}

// Collect row events before mutating browser state, so virtual rendering borrows
// the directory only once and all selection decisions use the same snapshot.
#[derive(Default)]
struct ListActions {
    additive: bool,
    selection: Option<(PathBuf, bool, bool)>,
    open: Option<PathBuf>,
    navigate_folder: Option<PathBuf>,
    inspect: Option<PathBuf>,
    naming: Option<super::edits::NamingAction>,
    files: Option<super::transfers::ui::SelectionAction>,
    select_all: bool,
    open_trash: bool,
    clear: bool,
    transfer: Option<(Vec<PathBuf>, PathBuf, bool)>,
    mount_binary: Option<(PathBuf, bool)>,
    unmount_binary: Option<PathBuf>,
    search_similar: Option<PathBuf>,
}

impl ListActions {
    fn background(&mut self, ui: &mut Ui, can_edit: bool, selected: bool) -> egui::Response {
        let focus = ui.interact(ui.available_rect_before_wrap(), list_id(), Sense::click());
        if focus.clicked() {
            focus.request_focus();
            ui.ctx().request_repaint();
            self.clear = true;
        }
        focus.context_menu(|ui| {
            self.naming = super::edit_ui::naming_buttons(ui, can_edit, None);
            self.files = super::transfers::ui::selection_buttons(ui, selected, can_edit, can_edit);
            self.select_all = ui.button("Select all").clicked();
            self.open_trash = ui.button("Open Trash…").clicked();
            if self.naming.is_some() || self.files.is_some() || self.select_all || self.open_trash {
                ui.close_menu();
            }
        });
        focus
    }

    fn drag(
        &mut self,
        response: &egui::Response,
        path: &PathBuf,
        selected: Option<&std::collections::BTreeSet<PathBuf>>,
    ) {
        if !response.drag_started() {
            return;
        }
        let marked = selected.is_some_and(|paths| paths.contains(path));
        let paths = if marked {
            selected
                .map(|paths| paths.iter().cloned().collect())
                .unwrap_or_default()
        } else {
            vec![path.clone()]
        };
        response.dnd_set_drag_payload(super::transfers::FileDrag {
            paths,
            copy_only: false,
        });
        if !marked {
            self.selection = Some((path.clone(), false, false));
        }
    }

    // Selection and focus share the same input policy across lists and photo tiles.
    fn row_selection(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        entry: &super::filesystem::Entry,
        selected: Option<&std::collections::BTreeSet<PathBuf>>,
    ) -> bool {
        let marked = selected.is_some_and(|paths| paths.contains(&entry.path));
        self.drag(response, &entry.path, selected);
        if response.clicked() || response.double_clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            self.selection = Some((
                entry.path.clone(),
                self.additive || modifiers.ctrl || modifiers.command,
                modifiers.shift,
            ));
            ui.memory_mut(|memory| memory.request_focus(list_id()));
            ui.ctx().request_repaint();
        }
        if response.secondary_clicked() && !marked {
            self.selection = Some((entry.path.clone(), false, false));
            ui.memory_mut(|memory| memory.request_focus(list_id()));
        }
        marked
    }

    fn row(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        entry: &super::filesystem::Entry,
        selected: Option<&std::collections::BTreeSet<PathBuf>>,
        can_edit: bool,
        selection_count: usize,
    ) {
        let marked = self.row_selection(ui, response, entry, selected);
        if entry.directory {
            self.transfer = drop_files(ui, response, &entry.path);
        }
        self.row_context_menu(response, entry, marked, can_edit, selection_count);
        self.row_activation(ui, response, entry);
    }

    fn row_context_menu(
        &mut self,
        response: &egui::Response,
        entry: &super::filesystem::Entry,
        marked: bool,
        can_edit: bool,
        selection_count: usize,
    ) {
        response.context_menu(|ui| {
            if let Some(path) = inspection_button(ui, entry) {
                self.inspect = Some(path);
                ui.close_menu();
            }

            if ui
                .button(if entry.directory {
                    "Open folder"
                } else {
                    "Open in Files"
                })
                .clicked()
            {
                self.open = Some(entry.path.clone());
                ui.close_menu();
            }
            if !entry.directory && entry.system.is_none() && ui.button("Open externally").clicked()
            {
                let _ = super::viewers::external(entry.path.clone());
                ui.close_menu();
            }

            if entry.is_photo() && ui.button("Find similar photos in canvas").clicked() {
                self.search_similar = Some(entry.path.clone());
                ui.close_menu();
            }

            let mounts = super::fuse_binwalk::cached_mount_table();
            if super::fuse_binwalk::is_mountable_binary(entry) {
                let mount_point = super::fuse_binwalk::mount_point_for_binary(&entry.path);
                let shift = ui.input(|i| i.modifiers.shift);
                if let Some(mode) = mounts.status(&mount_point) {
                    if ui.button("Open mounted binary contents").clicked() {
                        self.navigate_folder = Some(mount_point.clone());
                        ui.close_menu();
                    }
                    if shift && mode == super::fuse_binwalk::MountMode::ReadOnly {
                        if ui
                            .button(super::fuse_binwalk::remount_writable_menu_label())
                            .clicked()
                        {
                            self.mount_binary = Some((entry.path.clone(), true));
                            ui.close_menu();
                        }
                    }
                    if ui.button("Unmount binary contents").clicked() {
                        self.unmount_binary = Some(mount_point);
                        ui.close_menu();
                    }
                } else {
                    let label = super::fuse_binwalk::mount_menu_label(shift);
                    if ui.button(label).clicked() {
                        self.mount_binary = Some((entry.path.clone(), shift));
                        ui.close_menu();
                    }
                }
            }
            if let Some(mount_root) = mounts.enclosing_mount_point(&entry.path) {
                if ui.button("Unmount binary contents").clicked() {
                    self.unmount_binary = Some(mount_root);
                    ui.close_menu();
                }
            }
            ui.separator();
            let rename = (!marked || selection_count == 1).then_some(entry.path.as_path());
            self.naming =
                super::edit_ui::naming_buttons(ui, can_edit && entry.system.is_none(), rename);
            self.files = super::transfers::ui::selection_buttons(
                ui,
                true,
                can_edit,
                can_edit && entry.system.is_none(),
            );
            if self.naming.is_some() || self.files.is_some() {
                ui.close_menu();
            }
        });
    }

    fn row_activation(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        entry: &super::filesystem::Entry,
    ) {
        let modifiers = ui.input(|input| input.modifiers);
        if self.additive || modifiers.ctrl || modifiers.command || modifiers.shift {
            return;
        }
        let icon = Rect::from_min_size(
            response.rect.min + vec2(6.0, 32.0),
            vec2(response.rect.height() - 42.0, response.rect.height() - 42.0),
        );
        let inspect_icon = entry.system.is_some()
            && ui
                .ctx()
                .pointer_interact_pos()
                .is_some_and(|point| icon.intersect(ui.clip_rect()).contains(point));
        if (response.clicked() || response.double_clicked()) && inspect_icon {
            self.inspect = Some(entry.path.clone());
        } else if response.double_clicked() {
            self.open = Some(entry.path.clone());
        }
    }
}

/// Offers the appropriate explicit metadata inspection without changing normal opening.
fn inspection_button(ui: &mut Ui, entry: &super::filesystem::Entry) -> Option<PathBuf> {
    let label = match (entry.system.is_some(), entry.regular_file) {
        (true, _) => "Inspect system object",
        (false, true) => "Inspect loop associations",
        _ => return None,
    };
    ui.add(egui::Button::new(label).wrap(false))
        .clicked()
        .then(|| entry.path.clone())
}

pub(super) fn focus_text(
    ctx: &egui::Context,
    output: &mut egui::text_edit::TextEditOutput,
    chars: usize,
) {
    output.response.request_focus();
    output
        .state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(chars),
        )));
    output.state.clone().store(ctx, output.response.id);
}

fn drop_files(
    ui: &Ui,
    response: &egui::Response,
    destination: &std::path::Path,
) -> Option<(Vec<PathBuf>, PathBuf, bool)> {
    if !ui.is_enabled() {
        return None;
    }
    response
        .dnd_release_payload::<super::transfers::FileDrag>()
        .map(|drag| {
            (
                drag.paths.clone(),
                destination.into(),
                !drag.copy_only && !ui.input(|i| i.modifiers.ctrl),
            )
        })
}

// File, music, and video rows share variable-height virtualization.
fn visible_media_rows(offsets: &[f32], offset: f32, height: f32) -> std::ops::Range<usize> {
    let first = offsets.partition_point(|y| *y <= offset).saturating_sub(1);
    let end = offsets
        .partition_point(|y| *y <= offset + height)
        .min(offsets.len().saturating_sub(1));
    first..end
}
fn media_rows(
    ui: &mut Ui,
    scroll: egui::ScrollArea,
    offsets: &[f32],
    mut row: impl FnMut(&mut Ui, usize, Rect),
) -> egui::scroll_area::ScrollAreaOutput<()> {
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_height(offsets.last().copied().unwrap_or(0.0));
        let origin = ui.min_rect().min;
        for index in visible_media_rows(offsets, viewport.min.y, viewport.height()) {
            let rect = Rect::from_min_size(
                origin + vec2(0.0, offsets[index]),
                vec2(ui.available_width(), offsets[index + 1] - offsets[index]),
            );
            row(ui, index, rect);
        }
    })
}

/// Paints a passive image below a row, with optional overlay, keeping file-list input available.
pub(super) fn preview_popup_with_overlay(
    ui: &Ui,
    id: Id,
    anchor: Rect,
    size: egui::Vec2,
    texture: Option<&egui::TextureHandle>,
    overlay: Option<(&egui::TextureHandle, Color32)>,
    message: Option<&str>,
) {
    if popup_open(ui.ctx()) {
        return;
    }
    let screen = ui.ctx().screen_rect().shrink(8.0);
    let left = anchor
        .left()
        .clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .fixed_pos(pos2(left, anchor.bottom() + 6.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            if let Some(texture) = texture {
                let (rect, _response) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().image(
                    texture.id(),
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                if let Some((overlay, tint)) = overlay {
                    ui.painter().image(
                        overlay.id(),
                        rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        tint,
                    );
                }
            }
            if let Some(message) = message {
                ui.label(message);
            }
        });
}

/// Paints a passive image below a row, keeping file-list input available.
pub(super) fn preview_popup(
    ui: &Ui,
    id: Id,
    anchor: Rect,
    size: egui::Vec2,
    texture: Option<&egui::TextureHandle>,
    message: Option<&str>,
) {
    preview_popup_with_overlay(ui, id, anchor, size, texture, None, message);
}

/// Common header and body geometry for passive information strips.
pub(super) fn information_strip(
    ui: &mut Ui,
    row: Rect,
    entry: &super::filesystem::Entry,
    detail: &str,
    columns: Option<ListColumns>,
    colors: WorkbenchColors,
) -> Rect {
    media_header(
        ui,
        Rect::from_min_size(row.min + vec2(6.0, 0.0), vec2(row.width() - 12.0, 24.0)),
        entry,
        Some(detail.into()),
        columns,
        colors,
    );
    Rect::from_min_max(row.min + vec2(6.0, 32.0), row.max - vec2(6.0, 10.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_inspection_preserves_request_mode_for_ordinary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("image.img");
        let mut browser = FileManager::new(temp.path().into());
        browser.apply_list_actions(ListActions {
            inspect: Some(path.clone()),
            ..Default::default()
        });
        assert_eq!(browser.open_file.take(), Some(path.clone()));
        assert_eq!(browser.inspect_file.take(), Some(path.clone()));
        browser.apply_list_actions(ListActions {
            open: Some(path.clone()),
            ..Default::default()
        });
        assert_eq!(browser.open_file.take(), Some(path));
        assert!(browser.inspect_file.is_none());
    }

    #[test]
    fn mouse_back_and_forward_buttons_step_navigation_history() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();

        let mut browser = FileManager::new(temp.path().into());
        browser.history = vec![
            super::super::Location {
                path: first.clone(),
                selected: None,
                selection: Default::default(),
                anchor: None,
                selection_undo: None,
                scroll: 0.0,
                manual_view: None,
            },
            super::super::Location {
                path: second.clone(),
                selected: None,
                selection: Default::default(),
                anchor: None,
                selection_undo: None,
                scroll: 0.0,
                manual_view: None,
            },
        ];
        browser.index = Some(1);

        let ctx = egui::Context::default();
        let test_nav = |browser: &mut FileManager, button: egui::PointerButton| {
            let mut raw = egui::RawInput::default();
            raw.events.push(egui::Event::PointerButton {
                pos: pos2(20.0, 20.0),
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            });
            let _ = ctx.run(raw, |ctx| {
                browser.navigation_shortcuts(ctx);
            });
        };

        test_nav(&mut browser, egui::PointerButton::Extra1);
        assert_eq!(
            browser.pending.as_ref().map(|req| &req.path),
            Some(&first),
            "mouse Back (Extra1) should step back in history"
        );
        browser.index = Some(0);
        browser.pending = None;

        test_nav(&mut browser, egui::PointerButton::Extra2);
        assert_eq!(
            browser.pending.as_ref().map(|req| &req.path),
            Some(&second),
            "mouse Forward (Extra2) should step forward in history"
        );
    }

    #[test]
    fn context_menu_suppresses_preview_hover_and_popup() {
        let ctx = egui::Context::default();
        let target_rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 60.0));
        let hover_pos = pos2(40.0, 30.0);

        // Frame 1: ordinary hover without context menu.
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::PointerMoved(hover_pos));
        let mut hovered_without_menu = false;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_rect(target_rect, Sense::click());
                hovered_without_menu = preview_hovered(ui, target_rect);
                response.context_menu(|ui| {
                    let _ = ui.button("Open in Files");
                });
            });
        });
        assert!(hovered_without_menu);

        // Frame 2: right-click opens context menu; hover is suppressed immediately.
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::PointerMoved(hover_pos));
        raw.events.push(egui::Event::PointerButton {
            pos: hover_pos,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        raw.events.push(egui::Event::PointerButton {
            pos: hover_pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        let mut hovered_on_open = true;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_rect(target_rect, Sense::click());
                hovered_on_open = preview_hovered(ui, target_rect);
                response.context_menu(|ui| {
                    let _ = ui.button("Open in Files");
                });
            });
        });
        assert!(!hovered_on_open);

        // Frame 3: while context menu stays open, hover remains suppressed even away from menu box.
        let mut raw = egui::RawInput::default();
        raw.events
            .push(egui::Event::PointerMoved(pos2(180.0, 50.0)));
        let mut hovered_while_open = true;
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_rect(target_rect, Sense::click());
                hovered_while_open = preview_hovered(ui, target_rect);
                response.context_menu(|ui| {
                    let _ = ui.button("Open in Files");
                });
            });
        });
        assert!(!hovered_while_open);
    }
}
