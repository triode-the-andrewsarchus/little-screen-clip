use std::path::PathBuf;

use super::{
    pdf_strip::focus_id as pdf_id, text_strip::selection::focus_id as minimap_id,
    video_timeline::selection::focus_id as timeline_id,
};
use egui::{Color32, FontId, Id, Key, Modifiers, Rect, Sense, Ui, pos2, vec2};
use little_clip_core::palette::AppPalette;

use super::{FileManager, Navigation, display_path};
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext;

mod bookmarks;
mod columns;
pub(crate) mod gallery;
mod list;
mod mounts;
pub(super) mod music;
pub(super) mod video;

const ROW_HEIGHT: f32 = 28.0;
pub(super) const GROUP_HEADER_HEIGHT: f32 = 52.0;

pub(super) fn address_id() -> Id {
    Id::unique("file_manager_address")
}
pub(super) fn list_id() -> Id {
    Id::unique("file_manager_list_focus")
}
fn tree_id() -> Id {
    Id::unique("file_manager_tree_focus")
}

fn strip_focus_ids() -> [Id; 6] {
    [
        music::waveform_id(),
        super::midi_strip::focus_id(),
        minimap_id(),
        timeline_id(),
        pdf_id(),
        super::scope_view::strip::focus_id(),
    ]
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
            self.address = self.location_label();
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
        }
    }

    pub(crate) fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        self.drop_targets.clear();
        self.bookmark_drop_rect = None;
        self.sync_preview_context();
        let settle_focus = self.sync_browser_focus(ui.ctx());
        let focus_handoff = self.focus_list;
        self.shortcuts(ui.ctx());
        if self.browser_modal() {
            ui.disable();
        }
        self.toolbar(ui, palette);
        self.search_ui(ui, palette);
        self.answer_columns_ui(ui, palette);
        self.sync_preview_context();
        self.media_view_toolbar(ui, palette);
        ui.separator();
        self.docked_panels(ui, palette);
        // A coalesced header click can also clear focus during the later list
        // background interaction. Restore it only after that interaction ends.
        if std::mem::take(&mut self.focus_list) && !self.browser_modal() && !popup_open(ui.ctx()) {
            let text_focus = ui
                .memory(|memory| memory.focused())
                .and_then(|id| egui::TextEdit::load_state(ui.ctx(), id))
                .is_some();
            if !text_focus {
                ui.memory_mut(|memory| memory.request_focus(list_id()));
                self.focus_list = settle_focus || !focus_handoff;
                if self.focus_list {
                    ui.ctx().request_repaint();
                }
            }
        }
        ui.memory_mut(|memory| {
            for id in [tree_id(), list_id()].into_iter().chain(strip_focus_ids()) {
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
        self.naming_ui(ui.ctx(), palette);
        self.transfer_windows(ui.ctx(), palette);
        self.search_settings_ui(ui.ctx(), palette);
        self.large_video_copy_ui(ui.ctx(), palette);
        let file_clipboard = !self.browser_modal()
            && ui.memory(|memory| {
                memory.has_focus(list_id())
                    || memory
                        .focused()
                        .is_some_and(|id| strip_focus_ids().contains(&id))
            });
        let text_focus_shortcuts = if ui.memory(|memory| memory.has_focus(list_id())) {
            vec![egui::KeyboardShortcut::new(Modifiers::CTRL, Key::F)]
        } else {
            Vec::new()
        };
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                egui::Id::unique(egui_winit::CLIPBOARD_SHORTCUTS_AS_KEYS),
                file_clipboard,
            );
            data.insert_temp(
                egui::Id::unique(egui_winit::CLIPBOARD_TEXT_FOCUS_SHORTCUTS),
                text_focus_shortcuts,
            );
        });
    }

    pub(super) fn schedule_repaints(&self, ctx: &egui::Context) {
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
            || self.constellation.busy()
            || self.mounts.busy()
            || self.edit_busy()
            || self.fuse_mounts.busy()
            || self.transfers.needs_repaint()
            || self.shelf.busy()
            || self.archives.busy()
            || self.watch.is_some()
            || self.tree.branches.values().any(|branch| {
                branch.expanded && branch.directory.is_none() && branch.error.is_none()
            })
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn copy_strip_selection(&mut self, ctx: &egui::Context) {
        if ctx.memory(|memory| memory.has_focus(super::scope_view::strip::focus_id())) {
            if let Some(selection) = self.previews.scope_selection.clone() {
                let context = self.search_preview_context();
                self.preview_copies
                    .start(ctx, move |cancel| selection.export(&context, cancel));
            }
        } else if ctx.memory(|memory| memory.has_focus(super::midi_strip::focus_id())) {
            self.midi.copy_selection(ctx.input(|input| input.time));
        } else if ctx.memory(|memory| memory.has_focus(minimap_id())) {
            self.copy_text_range(ctx);
        } else if ctx.memory(|memory| memory.has_focus(pdf_id())) {
            self.copy_pdf_pages(ctx);
        } else if ctx.memory(|memory| memory.has_focus(timeline_id())) {
            self.copy_video_range(ctx, super::video_timeline::selection::CopyKind::Video);
        } else {
            self.copy_music_range();
        }
    }
    fn copy_text_range(&mut self, ctx: &egui::Context) {
        if let Some(selection) = self.previews.text_selection.range.clone() {
            self.preview_copies
                .start(ctx, move |cancel| selection.export(cancel));
        }
    }

    fn copy_pdf_pages(&mut self, ctx: &egui::Context) {
        let selection = &mut self.previews.pdf_pages.selection;
        if let Some(pages) = selection
            .requested
            .take()
            .or_else(|| selection.selected.clone())
        {
            let context = self.search_preview_context();
            self.preview_copies
                .start(ctx, move |cancel| pages.export(&context, cancel));
        }
    }

    fn docked_panels(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        if self.shelf.open {
            let height = (ui.available_height() * 0.38).clamp(144.0, 224.0);
            egui::Panel::bottom("file_manager_shelf")
                .exact_size(height)
                .frame(egui::Frame::NONE.fill(colors.recessed).inner_margin(8.0))
                .show(ui, |ui| {
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
            egui::Panel::bottom("file_manager_status")
                .default_size(80.0)
                .min_size(if self.error.is_some() { 80.0 } else { 56.0 })
                .max_size(150.0)
                .frame(egui::Frame::NONE.inner_margin(6.0))
                .show(ui, |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    egui::ScrollArea::vertical()
                        .min_scrolled_height(0.0)
                        .max_height(ui.available_height())
                        .show(ui, |ui| self.status(ui, colors, palette));
                });
        }
        egui::Panel::left("file_manager_folders")
            .default_size(240.0)
            .size_range(150.0..=400.0)
            .resizable(true)
            .show(ui, |ui| self.sidebar_ui(ui, colors));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(
                if self.view == super::BrowserView::Constellation {
                    0.0
                } else {
                    8.0
                },
            ))
            .show(ui, |ui| {
                if ui.is_enabled()
                    && !matches!(
                        self.view,
                        super::BrowserView::Constellation | super::BrowserView::Columns
                    )
                {
                    self.drop_targets.push((
                        ui.available_rect_before_wrap().intersect(ui.clip_rect()),
                        self.path().to_path_buf(),
                    ));
                }
                if self.view == super::BrowserView::Columns {
                    self.columns_ui(ui, colors, palette);
                } else if self.view == super::BrowserView::Photos {
                    self.gallery_ui(ui, colors, palette);
                } else if self.view == super::BrowserView::Constellation {
                    self.constellation_ui(ui, colors, palette);
                } else {
                    self.list_ui(ui, colors, palette);
                }
            });
    }

    pub(super) fn constellation_ui(
        &mut self,
        ui: &mut Ui,
        _colors: WorkbenchColors,
        palette: &AppPalette,
    ) {
        let current_path = self.path().to_path_buf();
        self.constellation.check_and_load_folder(&current_path);

        let mut sample_to_play: Option<PathBuf> = None;
        let mut sample_to_select: Option<PathBuf> = None;

        self.constellation.render_ui(
            ui,
            &current_path,
            palette,
            &mut |path| {
                sample_to_play = Some(path.to_path_buf());
            },
            &mut |path| {
                sample_to_select = Some(path.to_path_buf());
            },
        );

        self.retire_constellation_audition();
        if let Some(p) = sample_to_play {
            let guard = self.search_source_guard(&p);
            self.music.play(&p, guard);
        }
        if let Some(p) = sample_to_select {
            self.select(p);
        }
    }

    fn browser_modal(&self) -> bool {
        self.edits.dialog.is_some()
            || self.transfers_modal()
            || self.search.preferences.open
            || self.large_video_copy.is_some()
    }

    fn sync_browser_focus(&mut self, ctx: &egui::Context) -> bool {
        let modal = self.browser_modal();
        let focused = ctx.memory(|memory| memory.focused());
        let default_list = matches!(
            self.view,
            super::BrowserView::List | super::BrowserView::Columns
        ) && focused.is_none();
        let requested = self.was_modal || self.focus_list || default_list;
        self.was_modal = modal;
        let owns_arrows =
            focused.is_some_and(|id| id == tree_id() || strip_focus_ids().contains(&id));
        // Explicit sidebar/strip focus, clicks and Tab choose their own target.
        if owns_arrows
            || ctx.input(|input| {
                input.pointer.any_down()
                    || input.pointer.any_released()
                    || input.key_pressed(Key::Tab)
            })
        {
            self.focus_list = false;
            return false;
        }
        if !modal && !popup_open(ctx) && requested {
            self.focus_list = true;
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
            // egui installs a focus lock only after the target owned the
            // previous frame. Repaint until that handoff settles, retaining
            // arrows even if egui's pending directional focus runs meanwhile.
            return focused != Some(list_id())
                || ctx.input(|input| {
                    [
                        Key::ArrowUp,
                        Key::ArrowDown,
                        Key::ArrowLeft,
                        Key::ArrowRight,
                    ]
                    .into_iter()
                    .any(|key| input.key_pressed(key))
                });
        }
        false
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.browser_modal() {
            self.type_navigation.clear();
            return;
        }
        if self.view == super::BrowserView::Constellation {
            self.constellation_shortcuts(ctx);
            return;
        }
        self.navigation_shortcuts(ctx);
        if self.search_navigation_keys(ctx) {
            return;
        }
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
        } else if let Some(id) = ctx
            .memory(|memory| memory.focused())
            .filter(|id| strip_focus_ids().contains(id))
        {
            self.strip_shortcuts(ctx, id);
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

    fn strip_shortcuts(&mut self, ctx: &egui::Context, id: Id) {
        if [minimap_id(), timeline_id(), pdf_id()].contains(&id)
            && !popup_open(ctx)
            && key(ctx, Modifiers::NONE, Key::Escape)
        {
            self.previews.text_selection.range = None;
            self.video_previews.selection = None;
            self.previews.pdf_pages.selection.selected = None;
            self.focus_list = true;
        }
        if key(ctx, Modifiers::CTRL, Key::C) {
            self.copy_strip_selection(ctx);
        }
        if key(ctx, Modifiers::CTRL, Key::V) {
            self.paste_files();
        }
    }

    fn constellation_shortcuts(&mut self, ctx: &egui::Context) {
        self.type_navigation.clear();
        if key(ctx, Modifiers::CTRL, Key::F)
            || (!ctx.egui_wants_keyboard_input()
                && !self.constellation.uses_pad_key('/')
                && key(ctx, Modifiers::NONE, Key::Slash))
        {
            self.constellation.focus_search = true;
        }
        self.navigation_shortcuts(ctx);
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
        self.column_shortcuts(ctx);
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
        ui.horizontal(|ui| {
            if nav_button(
                ui,
                Icon::ArrowLeft,
                "Back [Alt+Left]",
                self.index.is_some_and(|i| i > 0),
                palette,
            ) {
                self.history_step(false);
            }
            if nav_button(
                ui,
                Icon::ArrowRight,
                "Forward [Alt+Right]",
                self.index.is_some_and(|i| i + 1 < self.history.len()),
                palette,
            ) {
                self.history_step(true);
            }
            if nav_button(
                ui,
                Icon::ArrowUp,
                "Parent folder [Alt+Up]",
                self.path().parent().is_some(),
                palette,
            ) {
                self.parent();
            }
            let address_width = (ui.available_width() - 235.0).max(80.0);
            self.address_ui(ui, address_width);
            self.index_ui(ui);
            if nav_button(ui, Icon::Repeat, "Refresh [F5]", true, palette) {
                self.refresh();
            }
            if ui
                .add(clip_ui::buttons::ButtonRole::Secondary.button(
                    if self.show_hidden {
                        "Hidden: on"
                    } else {
                        "Hidden: off"
                    },
                    palette,
                ))
                .on_hover_text("Show hidden files [Ctrl+H]")
                .clicked()
            {
                self.toggle_hidden();
            }
        });
    }

    pub(super) fn selection_summary(&self) -> String {
        if self.view == super::BrowserView::Constellation {
            return self.constellation.sample_count_label();
        }
        let selection = self.location().map(|location| &location.selection);
        let selected = selection.is_some_and(|paths| !paths.is_empty());
        let entries = self.displayed_entries().into_iter().filter(|entry| {
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
            return if self.search.applied.is_some() {
                "No matching files"
            } else {
                "Empty folder"
            }
            .into();
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

    fn pending_status_ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        ui.horizontal(|ui| {
            if let Some(request) = &self.pending {
                ui.spinner();
                ui.add(
                    egui::Label::new(format!("Opening {}…", display_path(&request.path)))
                        .truncate(),
                );
                if ui
                    .add(
                        clip_ui::buttons::ButtonRole::Secondary
                            .button("Cancel", palette)
                            .small(),
                    )
                    .clicked()
                {
                    self.pending = None;
                    self.address = self.location_label();
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
                ui.add(egui::Label::new(mount_status).truncate());
            }
        });
    }

    fn error_status_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
        let Some(error) = &self.error else {
            return;
        };
        if self.last_status.as_ref().map(|s| &s.text) != Some(error) {
            self.last_status = Some(super::LastStatus::new(Icon::Alert, error.clone(), true));
        }
        let (retry, dismiss) = ui
            .horizontal_wrapped(|ui| {
                ui.add(Icon::Alert.label(egui::RichText::new(error).color(colors.danger)));
                let retry = ui
                    .add(
                        clip_ui::buttons::ButtonRole::Secondary
                            .button("Retry", palette)
                            .small(),
                    )
                    .clicked();
                let dismiss = ui
                    .add(
                        clip_ui::buttons::ButtonRole::Secondary
                            .button("Dismiss", palette)
                            .small(),
                    )
                    .clicked();
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

    fn last_status_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
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
                ui.add(
                    clip_ui::buttons::ButtonRole::Secondary
                        .button("Dismiss", palette)
                        .small(),
                )
                .clicked()
            })
            .inner;
        if dismiss {
            self.clear_status();
        }
    }

    fn status(&mut self, ui: &mut Ui, colors: WorkbenchColors, palette: &AppPalette) {
        self.pending_status_ui(ui, palette);
        self.error_status_ui(ui, colors, palette);
        self.edit_status(ui, colors, palette);
        self.transfer_status(ui, colors, palette);
        self.last_status_ui(ui, colors, palette);
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
        self.compare_selection |= actions.compare_selection;
        if actions.selection.is_some() || actions.clear {
            self.type_navigation.clear();
        }
        if let Some((paths, destination, moving)) = actions.transfer {
            self.queue_drag_transfer(paths, destination, moving);
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
            self.document_start = actions
                .open_position
                .map(|position| (path.clone(), position));
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
                    self.photo_recursive_limit,
                    self.photo_recursive_depth,
                    self.photo_recursive_timeout_seconds,
                );
            }
            self.similarity_canvas.search_from_photo(&path);
        }

        self.external_file = actions.external;
        if let Some(path) = actions.reveal {
            self.reveal_search_result(path, false);
        }
    }

    fn list_scroll(&mut self, ui: &Ui, row_height: f32) -> egui::ScrollArea {
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("file_manager_listing")
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
                    .zip(
                        self.directory
                            .as_ref()
                            .map(|directory| &directory.entries[*i]),
                    )
                    .is_some_and(|(selected, entry)| {
                        selected == &entry.path
                            || (entry.directory && selected.starts_with(&entry.path))
                    })
            }) {
                scroll = scroll.vertical_scroll_offset({
                    let base = self
                        .list_offsets
                        .get(index)
                        .copied()
                        .unwrap_or(index as f32 * row_height);
                    let nested = self
                        .location()
                        .and_then(|location| location.selected.as_ref())
                        .zip(
                            self.directory
                                .as_ref()
                                .map(|directory| &directory.entries[self.visible[index]]),
                        )
                        .map_or(0.0, |(selected, entry)| {
                            self.containers
                                .selection_offset(entry, selected, self.show_hidden)
                        });
                    let heading = self
                        .list_sort()
                        .and_then(|sort| {
                            sort.group_at(self.directory.as_ref()?, &self.visible, index)
                        })
                        .map_or(0.0, |_| GROUP_HEADER_HEIGHT);
                    let top = base + heading + nested;
                    let pinned = if self.grouped_by_type() {
                        GROUP_HEADER_HEIGHT
                    } else {
                        0.0
                    };
                    let height = (ui.available_height() - pinned).max(1.0);
                    let bottom = self
                        .list_offsets
                        .get(index + 1)
                        .copied()
                        .unwrap_or(top + row_height);
                    // Reveal only the part outside the viewport. A selection
                    // change inside it must not evict and reload visible strips.
                    // Oversized rows align their top instead of oscillating.
                    let lower = (bottom.min(top + height) - height - pinned).max(0.0);
                    self.location()
                        .map_or(0.0, |location| location.scroll)
                        .clamp(lower, (top - pinned).max(lower))
                });
            }
            self.reveal_selection = false;
        }
        scroll
    }

    pub(super) fn list_anchor(&self) -> Option<(PathBuf, f32)> {
        let scroll = self.location()?.scroll;
        let row = self
            .list_offsets
            .partition_point(|offset| *offset <= scroll)
            .saturating_sub(1);
        let entry = self
            .directory
            .as_ref()?
            .entries
            .get(*self.visible.get(row)?)?;
        Some((entry.path.clone(), scroll - self.list_offsets[row]))
    }
    pub(super) fn restore_list_anchor(&mut self, anchor: Option<(PathBuf, f32)>) {
        let Some((path, within)) = anchor else { return };
        let Some(row) = self.visible.iter().position(|index| {
            self.directory
                .as_ref()
                .is_some_and(|directory| directory.entries[*index].path == path)
        }) else {
            return;
        };
        let offset = self.list_offsets[row]
            + within.min((self.list_offsets[row + 1] - self.list_offsets[row]).max(0.0));
        if let Some(location) = self.location_mut() {
            location.scroll = offset;
        }
        self.restore_scroll = true;
    }
    fn tree_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        ui.heading("Folders");
        ui.separator();
        let rows = self.tree.rows(self.show_hidden);
        let mut scroll = egui::ScrollArea::both()
            .min_scrolled_height(0.0)
            .id_salt("file_manager_tree")
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
                self.queue_drag_transfer(paths, destination, moving)
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
            Id::unique(("file_tree_arrow", index)),
            Sense::click(),
        );
        let body = Rect::from_min_max(pos2(arrow_rect.max.x, rect.min.y), rect.max);
        let response = ui.interact(body, Id::unique(("file_tree_row", index)), Sense::click());
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
                egui::StrokeKind::Middle,
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

pub(super) fn entry_icon(entry: &super::filesystem::Entry) -> Icon {
    if entry.directory {
        Icon::Folder
    } else if entry.is_brush() {
        Icon::Brush
    } else if entry.is_spreadsheet() {
        Icon::Spreadsheet
    } else if super::container_preview::inventory::archive(&entry.path) {
        Icon::FolderZip
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
    ctx.input(|input| input.pointer.secondary_clicked() || input.pointer.secondary_down())
        || egui::Popup::is_any_open(ctx)
        || ctx.any_popup_open()
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
    left: f32,
    size: f32,
    modified: f32,
    right: f32,
    extra: Option<f32>,
    extra_count: usize,
    metadata: Option<(f32, f32, super::sorting::Category)>,
}

impl ListColumns {
    pub(super) fn new(header: Rect) -> Self {
        let name_width = (header.width() - 245.0).max(header.width() * 0.4);
        Self {
            left: header.left(),
            size: header.left() + name_width,
            modified: header.left() + name_width + 90.0,
            right: header.right(),
            extra: None,
            extra_count: 0,
            metadata: None,
        }
    }
    pub(super) fn with_extra(mut self) -> Self {
        self.extra = Some(self.size - 145.0);
        self.extra_count = 1;
        self
    }
    pub(super) fn with_extras(mut self, count: usize) -> Self {
        if count > 0 {
            self.extra = Some(self.size - count as f32 * 145.0);
        }
        self.extra_count = count;
        self
    }
    pub(super) fn for_family(mut self, family: Option<super::sorting::Category>) -> Self {
        self.metadata = None;
        if let Some(family) =
            family.filter(|family| !super::metadata::Field::for_family(*family).is_empty())
        {
            let end = self.extra.unwrap_or(self.size);
            let available = (end - self.left).max(0.0);
            let name_share = if family == super::sorting::Category::Geography {
                0.25
            } else {
                0.4
            };
            let left = self.left + (available * name_share).max(120.0).min(available);
            self.metadata = Some((left, end, family));
        }
        self
    }
    pub(super) fn metadata_fields(self) -> &'static [super::metadata::Field] {
        self.metadata.map_or(&[], |(_, _, family)| {
            super::metadata::Field::for_family(family)
        })
    }
    pub(super) fn metadata_cell(self, row: Rect, index: usize) -> Option<Rect> {
        let (left, right, family) = self.metadata?;
        let fields = self.metadata_fields();
        if index >= fields.len() {
            return None;
        }
        let (count, start, end) = if family == super::sorting::Category::Geography {
            // The date columns need more room than counts or coordinates.
            const EDGES: [f32; 7] = [0.0, 2.0, 4.0, 5.0, 6.0, 7.0, 8.0];
            (EDGES[fields.len()], EDGES[index], EDGES[index + 1])
        } else {
            (fields.len() as f32, index as f32, (index + 1) as f32)
        };
        let width = (right - left) / count;
        Some(Rect::from_min_max(
            pos2(left + start * width, row.top()),
            pos2(left + end * width, row.top() + ROW_HEIGHT),
        ))
    }
    fn name_end(self) -> f32 {
        self.metadata
            .map_or_else(|| self.extra.unwrap_or(self.size), |range| range.0)
    }
    pub(super) fn extra_cell(self, row: Rect) -> Option<Rect> {
        self.extra_cell_at(row, 0)
    }
    pub(super) fn extra_cell_at(self, row: Rect, index: usize) -> Option<Rect> {
        if index >= self.extra_count {
            return None;
        }
        let left = self.extra? + index as f32 * 145.0;
        Some(Rect::from_min_max(
            pos2(left, row.top()),
            pos2(left + 145.0, row.top() + ROW_HEIGHT),
        ))
    }

    fn paint(&self, ui: &Ui, row: Rect, size: &str, modified: &str, color: Color32) {
        paint_text(ui, row, self.size - row.left() + 5.0, 80.0, size, color);
        paint_text(
            ui,
            row,
            self.modified - row.left() + 5.0,
            self.right - self.modified - 10.0,
            modified,
            color,
        );
    }

    fn sort_header(
        &self,
        ui: &mut Ui,
        header: Rect,
        sort: Option<super::sorting::Sort>,
        colors: WorkbenchColors,
        show_type: bool,
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
            (
                Column::Name,
                "Name",
                header.left() + if show_type { 64.0 } else { 8.0 },
                self.name_end(),
            ),
            (Column::Size, "Size", self.size, self.modified),
            (Column::Modified, "Modified", self.modified, self.right),
        ]
        .into_iter()
        .filter(|(column, ..)| show_type || *column != Column::Type)
        {
            let cell = Rect::from_min_max(pos2(left, header.top()), pos2(right, header.bottom()));
            let active = sort
                .filter(|sort| sort.column == column)
                .map(|sort| sort.descending);
            let response = sort_heading(
                ui,
                cell,
                ui.make_persistent_id(("files_sort", column)),
                label,
                active,
                colors,
            )
            .on_hover_text(format!("Sort by {label}; click again to reverse"));
            if response.clicked() {
                clicked = Some(column);
            }
        }
        clicked
    }
}

/// Shared fixed-rectangle sort header; text clipping never grows the table layout.
pub(super) fn sort_heading(
    ui: &mut Ui,
    cell: Rect,
    id: Id,
    label: &str,
    active: Option<bool>,
    colors: WorkbenchColors,
) -> egui::Response {
    let response = ui
        // Own the gesture so a pinned header click cannot start dragging the
        // surrounding scroll area when the pointer moved during that frame.
        .interact(cell, id, Sense::click_and_drag())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), label));
    if response.hovered() {
        ui.painter().rect_filled(cell, 2.0, colors.recessed);
    }
    let color = if active.is_some() {
        colors.accent
    } else {
        colors.text_secondary
    };
    paint_text(ui, cell, 5.0, cell.width() - 23.0, label, color);
    if let Some(descending) = active {
        let arrow =
            Rect::from_center_size(pos2(cell.right() - 9.0, cell.center().y), vec2(14.0, 14.0));
        (if descending {
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
    response
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
        header.max.x = (columns.name_end() - 8.0).max(header.left());
    }
    // Optional summaries yield to the filename, including in narrow typed
    // columns. Match paint_text's font and retain a small glyph-edge margin.
    let filename_width = ui
        .painter()
        .layout_no_wrap(
            entry.name.clone(),
            FontId::proportional(14.0),
            colors.text_primary,
        )
        .size()
        .x
        .ceil()
        + 2.0;
    let details_budget = (header.width() - filename_width - 12.0).max(0.0);
    let details_width = details.as_ref().map_or(0.0, |details| {
        ui.painter()
            .layout_no_wrap(
                details.clone(),
                FontId::proportional(14.0),
                colors.text_secondary,
            )
            .size()
            .x
            .min(details_budget)
    });
    let gap = if details_width > 0.0 { 12.0 } else { 0.0 };
    let name_width = (header.width() - details_width - gap).max(0.0);
    paint_text(
        ui,
        header,
        0.0,
        name_width,
        &entry.name,
        colors.text_primary,
    );
    if let Some(details) = details.as_ref().filter(|_| details_width > 0.0) {
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
            Id::unique(("media_metadata", &entry.path)),
            Sense::hover(),
        )
        .on_hover_text(details.map_or_else(
            || entry.name.clone(),
            |details| format!("{}\n{}", entry.name, details),
        ));
    }
}

pub(super) fn paint_text(ui: &Ui, rect: Rect, x: f32, width: f32, text: &str, color: Color32) {
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
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::ListItem,
            response.enabled(),
            selected,
            &entry.name,
        )
    });
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
    let name_width = columns.name_end() - rect.left();
    let count = count.filter(|_| name_width >= 260.0);
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
    Transfer(super::transfers::FileDrag, PathBuf, bool),
}

// Collect row events before mutating browser state, so virtual rendering borrows
// the directory only once and all selection decisions use the same snapshot.
#[derive(Default)]
struct ListActions {
    additive: bool,
    selection: Option<(PathBuf, bool, bool)>,
    open: Option<PathBuf>,
    open_position: Option<super::DocumentPosition>,
    navigate_folder: Option<PathBuf>,
    reveal: Option<PathBuf>,
    external: Option<PathBuf>,
    search_results: bool,
    context: super::search::PreviewContext,
    metadata_only: bool,
    copy_only: bool,
    inspect: Option<PathBuf>,
    naming: Option<super::edits::NamingAction>,
    files: Option<super::transfers::ui::SelectionAction>,
    select_all: bool,
    open_trash: bool,
    clear: bool,
    transfer: Option<(super::transfers::FileDrag, PathBuf, bool)>,
    mount_binary: Option<(PathBuf, bool)>,
    unmount_binary: Option<PathBuf>,
    search_similar: Option<PathBuf>,
    compare_selection: bool,
}

impl ListActions {
    fn for_browser(browser: &FileManager) -> Self {
        Self {
            search_results: browser.search.has_result_sources(),
            metadata_only: browser.search.has_offline_results(),
            context: browser.search_preview_context(),
            copy_only: browser.archive_context().is_some(),
            ..Self::default()
        }
    }

    fn background(&mut self, ui: &mut Ui, can_edit: bool, selected: bool) -> egui::Response {
        let focus = ui.interact(ui.available_rect_before_wrap(), list_id(), Sense::click());
        // Enter returning focus from a text field is not a background click.
        if focus.clicked_by(egui::PointerButton::Primary) {
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
                ui.close();
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
        if self.metadata_only || !response.drag_started_by(egui::PointerButton::Primary) {
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
            copy_only: self.copy_only,
            context: self.context.clone(),
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

    fn unavailable_row(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        entry: &super::filesystem::Entry,
        selected: Option<&std::collections::BTreeSet<PathBuf>>,
    ) {
        self.row_selection(ui, response, entry, selected);
        response.context_menu(|ui| {
            ui.label(
                "Unavailable last-known result. Reconnect and refresh before using this file.",
            );
        });
        if response.double_clicked() {
            self.open = Some(entry.path.clone());
        }
    }

    fn video_audio_menu(&mut self, ui: &mut Ui, entry: &super::filesystem::Entry) {
        if !entry.directory
            && entry.system.is_none()
            && little_clip_core::animation::is_video_path(&entry.path)
            && ui.button("Edit Audio Track").clicked()
        {
            self.open = Some(entry.path.clone());
            self.open_position = Some(super::DocumentPosition::VideoAudioTrack);
            ui.close();
        }
    }

    fn raw_iq_menu(&mut self, ui: &mut Ui, entry: &super::filesystem::Entry) {
        let candidate = entry.path.extension().is_some_and(|extension| {
            ["cs8", "cu8", "iq", "bin"]
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        });
        if entry.regular_file && candidate && ui.button("Open as raw I/Q").clicked() {
            self.open = Some(entry.path.clone());
            self.open_position = Some(super::DocumentPosition::RawIq {
                revision: entry.revision,
            });
            ui.close();
        }
    }

    fn comparison_menu(
        &mut self,
        ui: &mut Ui,
        entry: &super::filesystem::Entry,
        marked: bool,
        count: usize,
    ) {
        if marked
            && entry.regular_file
            && !self.search_results
            && !self.copy_only
            && (2..=super::engineering_compare::MAX_SOURCES).contains(&count)
            && ui.button("Compare selected files").clicked()
        {
            self.compare_selection = true;
            ui.close();
        }
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
                ui.close();
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
                ui.close();
            }
            if self.search_results && ui.button("Reveal in folder").clicked() {
                self.reveal = Some(entry.path.clone());
                ui.close();
            }
            if !entry.directory && entry.system.is_none() && ui.button("Open externally").clicked()
            {
                self.external = Some(entry.path.clone());
                ui.close();
            }

            self.video_audio_menu(ui, entry);
            self.raw_iq_menu(ui, entry);

            self.comparison_menu(ui, entry, marked, selection_count);

            if entry.is_photo() && ui.button("Find similar photos in canvas").clicked() {
                self.search_similar = Some(entry.path.clone());
                ui.close();
            }

            let mounts = super::fuse_binwalk::cached_mount_table();
            if super::fuse_binwalk::is_mountable_binary(entry) {
                let mount_point = super::fuse_binwalk::mount_point_for_binary(&entry.path);
                let shift = ui.input(|i| i.modifiers.shift);
                if let Some(mode) = mounts.status(&mount_point) {
                    if ui.button("Open mounted binary contents").clicked() {
                        self.navigate_folder = Some(mount_point.clone());
                        ui.close();
                    }
                    if shift
                        && mode == super::fuse_binwalk::MountMode::ReadOnly
                        && ui
                            .button(super::fuse_binwalk::remount_writable_menu_label())
                            .clicked()
                    {
                        self.mount_binary = Some((entry.path.clone(), true));
                        ui.close();
                    }
                    if ui.button("Unmount binary contents").clicked() {
                        self.unmount_binary = Some(mount_point);
                        ui.close();
                    }
                } else {
                    let label = super::fuse_binwalk::mount_menu_label(shift);
                    if ui.button(label).clicked() {
                        self.mount_binary = Some((entry.path.clone(), shift));
                        ui.close();
                    }
                }
            }
            if let Some(mount_root) = mounts.enclosing_mount_point(&entry.path)
                && ui.button("Unmount binary contents").clicked()
            {
                self.unmount_binary = Some(mount_root);
                ui.close();
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
                ui.close();
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
    ui.add(egui::Button::new(label).wrap_mode(egui::TextWrapMode::Extend))
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
) -> Option<(super::transfers::FileDrag, PathBuf, bool)> {
    if !ui.is_enabled() {
        return None;
    }
    response
        .dnd_release_payload::<super::transfers::FileDrag>()
        .map(|drag| {
            (
                drag.as_ref().clone(),
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
fn media_rows<R>(
    ui: &mut Ui,
    scroll: egui::ScrollArea,
    offsets: &[f32],
    groups: &[(usize, super::sorting::Category)],
    mut header: impl FnMut(&mut Ui, Rect, super::sorting::Category) -> Option<R>,
    mut row: impl FnMut(&mut Ui, usize, Rect) -> Option<R>,
) -> egui::scroll_area::ScrollAreaOutput<Option<R>> {
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_height(offsets.last().copied().unwrap_or(0.0));
        let origin = ui.min_rect().min;
        let mut clicked = None;
        let group = groups
            .partition_point(|(index, _)| offsets[*index] <= viewport.min.y)
            .saturating_sub(1);
        if let Some(&(_, family)) = groups.get(group) {
            let next = groups
                .get(group + 1)
                .map_or(f32::INFINITY, |(index, _)| offsets[*index]);
            let push = (next - viewport.min.y - GROUP_HEADER_HEIGHT).min(0.0);
            let pinned = Rect::from_min_size(
                origin + vec2(0.0, viewport.min.y + push),
                vec2(ui.available_width(), GROUP_HEADER_HEIGHT),
            );
            clicked = header(ui, pinned, family);
            // Clip both painting and hit testing below the pinned header, so
            // preview clicks, selection and drop targets cannot pass through it.
            let mut clip = ui.clip_rect();
            clip.min.y = clip.min.y.max(pinned.bottom());
            ui.set_clip_rect(clip);
        }
        for index in visible_media_rows(offsets, viewport.min.y, viewport.height()) {
            let rect = Rect::from_min_size(
                origin + vec2(0.0, offsets[index]),
                vec2(ui.available_width(), offsets[index + 1] - offsets[index]),
            );
            clicked = row(ui, index, rect).or(clicked);
        }
        clicked
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
    let screen = ui.ctx().content_rect().shrink(8.0);
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
    fn long_sort_labels_keep_fixed_bounds_in_narrow_cells() {
        let ctx = egui::Context::default();
        let colors = WorkbenchColors::from_palette(&AppPalette::default());
        for width in [28.0, 60.0, 160.0] {
            let _ = clip_ui::test_support::run_frame(&ctx, egui::RawInput::default(), |ctx| {
                ctx.root_ui(|ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let (cell, _) =
                            ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
                        let before = ui.min_rect();
                        let response = sort_heading(
                            ui,
                            cell,
                            Id::unique("bounded_header"),
                            "Megapixels",
                            Some(false),
                            colors,
                        );
                        assert_eq!(response.rect, cell);
                        assert_eq!(ui.min_rect(), before);
                    })
                });
            });
        }
    }

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
            let _ = clip_ui::test_support::run_frame(&ctx, raw, |ctx| {
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
        let _ = clip_ui::test_support::run_frame(&ctx, raw, |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let response = ui.allocate_rect(target_rect, Sense::click());
                    hovered_without_menu = preview_hovered(ui, target_rect);
                    response.context_menu(|ui| {
                        let _ = ui.button("Open in Files");
                    });
                })
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
        let _ = clip_ui::test_support::run_frame(&ctx, raw, |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let response = ui.allocate_rect(target_rect, Sense::click());
                    hovered_on_open = preview_hovered(ui, target_rect);
                    response.context_menu(|ui| {
                        let _ = ui.button("Open in Files");
                    });
                })
            });
        });
        assert!(!hovered_on_open);

        // Frame 3: while context menu stays open, hover remains suppressed even away from menu box.
        let mut raw = egui::RawInput::default();
        raw.events
            .push(egui::Event::PointerMoved(pos2(180.0, 50.0)));
        let mut hovered_while_open = true;
        let _ = clip_ui::test_support::run_frame(&ctx, raw, |ctx| {
            ctx.root_ui(|ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let response = ui.allocate_rect(target_rect, Sense::click());
                    hovered_while_open = preview_hovered(ui, target_rect);
                    response.context_menu(|ui| {
                        let _ = ui.button("Open in Files");
                    });
                })
            });
        });
        assert!(!hovered_while_open);
    }
}
