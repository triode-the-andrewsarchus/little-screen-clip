use egui::{Color32, FontId, Id, Key, Modifiers, Rect, Sense, Ui, pos2, vec2};
use little_clip_core::palette::AppPalette;

use super::{FileManager, Navigation, display_path};
use crate::icons::Icon;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext;

const ROW_HEIGHT: f32 = 28.0;

fn address_id() -> Id {
    Id::new("file_manager_address")
}
fn list_id() -> Id {
    Id::new("file_manager_list_focus")
}
fn tree_id() -> Id {
    Id::new("file_manager_tree_focus")
}

fn key(ctx: &egui::Context, modifiers: Modifiers, key: Key) -> bool {
    ctx.input_mut(|input| input.consume_key(modifiers, key))
}

impl FileManager {
    pub(crate) fn handle_address_escape(&mut self, ctx: &egui::Context) {
        if self.address_editing && key(ctx, Modifiers::NONE, Key::Escape) {
            self.address = display_path(self.path());
            ctx.memory_mut(|memory| memory.request_focus(list_id()));
        }
    }

    pub(crate) fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        self.tick(super::BackgroundJobs::global());
        let colors = WorkbenchColors::from_palette(palette);
        self.shortcuts(ui.ctx());
        self.toolbar(ui, palette);
        ui.separator();
        egui::TopBottomPanel::bottom("file_manager_status")
            .frame(egui::Frame::none().inner_margin(6.0))
            .show_inside(ui, |ui| self.status(ui, colors));
        egui::SidePanel::left("file_manager_folders")
            .default_width(240.0)
            .width_range(150.0..=400.0)
            .resizable(true)
            .show_inside(ui, |ui| self.tree_ui(ui, colors));
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(8.0))
            .show_inside(ui, |ui| self.list_ui(ui, colors));
        ui.memory_mut(|memory| {
            for id in [tree_id(), list_id()] {
                memory.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            }
        });
        if self.pending.is_some()
            || self.tree.branches.values().any(|branch| {
                branch.expanded && branch.directory.is_none() && branch.error.is_none()
            })
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if key(ctx, Modifiers::CTRL, Key::L) {
            self.focus_address = true;
        }
        if key(ctx, Modifiers::ALT, Key::ArrowLeft) {
            self.history_step(false);
        }
        if key(ctx, Modifiers::ALT, Key::ArrowRight) {
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
        if ctx.memory(|memory| memory.has_focus(address_id())) {
            return;
        }
        if ctx.memory(|memory| memory.has_focus(tree_id())) {
            self.tree_shortcuts(ctx);
        } else if ctx.memory(|memory| memory.has_focus(list_id())) {
            if key(ctx, Modifiers::NONE, Key::ArrowDown) {
                self.move_selection(true);
            }
            if key(ctx, Modifiers::NONE, Key::ArrowUp) {
                self.move_selection(false);
            }
            if key(ctx, Modifiers::NONE, Key::Enter) {
                self.open_selected();
            }
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
            let output = egui::TextEdit::singleline(&mut self.address)
                .id(address_id()).hint_text("Directory path [Ctrl+L]")
                .desired_width(address_width).show(ui);
            output.response.clone().on_hover_text("Enter a directory path. Relative paths and ~/ are supported. Backslashes: \\\\; unusual bytes: \\xHH.");
            if self.focus_address {
                self.focus_address = false;
                output.response.request_focus();
                let mut state = output.state;
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0), egui::text::CCursor::new(self.address.chars().count()),
                )));
                state.store(ui.ctx(), address_id());
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

    fn status(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
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
                        self.error = Some("Opening cancelled. Enter another path or retry.".into());
                    }
                }
            } else {
                let count = self.visible.len();
                ui.add(
                    egui::Label::new(format!(
                        "{} — {count} {}",
                        display_path(self.path()),
                        if count == 1 { "item" } else { "items" }
                    ))
                    .truncate(true),
                );
            }
        });
        if let Some(error) = &self.error {
            let retry = ui
                .horizontal_wrapped(|ui| {
                    ui.add(Icon::Alert.label(egui::RichText::new(error).color(colors.danger)));
                    ui.small_button("Retry").clicked()
                })
                .inner;
            if retry {
                self.retry();
            }
        }
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

    fn list_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let width = ui.available_width();
        let name_width = (width - 245.0).max(width * 0.4);
        let (header, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
        paint_text(
            ui,
            header,
            8.0,
            name_width - 10.0,
            "Name",
            colors.text_secondary,
        );
        paint_text(ui, header, name_width, 88.0, "Size", colors.text_secondary);
        paint_text(
            ui,
            header,
            name_width + 90.0,
            width - name_width - 90.0,
            "Modified",
            colors.text_secondary,
        );
        ui.separator();
        let scroll = self.list_scroll(ui);
        let Some(directory) = &self.directory else {
            ui.label(if self.pending.is_some() {
                "Loading folder…"
            } else {
                "Choose a folder or enter a path."
            });
            return;
        };
        if self.visible.is_empty() {
            ui.label(if directory.entries.is_empty() {
                "This folder is empty."
            } else {
                "No visible items. Enable hidden files to see this folder's contents."
            });
        }
        let selected = self
            .location()
            .and_then(|location| location.selected.as_ref());
        let mut select = None;
        let mut open = None;
        let focus = ui.interact(ui.available_rect_before_wrap(), list_id(), Sense::click());
        if focus.clicked() {
            focus.request_focus();
            ui.ctx().request_repaint();
        }
        let output = scroll.show_rows(ui, ROW_HEIGHT, self.visible.len(), |ui, range| {
            for index in range {
                let entry = &directory.entries[self.visible[index]];
                let response =
                    file_row(ui, entry, Some(&entry.path) == selected, name_width, colors);
                if response.clicked() || response.double_clicked() {
                    select = Some(entry.path.clone());
                    ui.memory_mut(|memory| memory.request_focus(list_id()));
                    ui.ctx().request_repaint();
                }
                if response.double_clicked() && entry.directory {
                    open = Some(entry.path.clone());
                }
            }
        });
        if let Some(location) = self.location_mut() {
            location.scroll = output.state.offset.y;
        }
        if let Some(path) = select {
            self.select(path);
        }
        if let Some(path) = open {
            self.navigate(path, Navigation::New);
        }
    }

    fn list_scroll(&mut self, ui: &Ui) -> egui::ScrollArea {
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
                scroll = scroll.vertical_scroll_offset(
                    index as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y),
                );
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
            None => {}
        }
    }
    fn tree_row(
        &self,
        ui: &mut Ui,
        row: &super::tree::TreeRow,
        index: usize,
        colors: WorkbenchColors,
    ) -> Option<TreeAction> {
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
            response
                .clone()
                .on_hover_text(format!("{}\n{detail}", display_path(&row.path)));
            if branch.error.is_some() {
                Icon::Alert.paint(
                    ui.painter(),
                    Rect::from_min_size(rect.max - vec2(20.0, 23.0), vec2(16.0, 16.0)),
                    colors.danger,
                );
            }
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
    entry: &super::filesystem::Entry,
    selected: bool,
    name_width: f32,
    colors: WorkbenchColors,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let response = response.on_hover_text(format!(
        "{}{}",
        display_path(&entry.path),
        entry
            .warning
            .as_ref()
            .map(|warning| format!("\nMetadata unavailable: {warning}"))
            .unwrap_or_default()
    ));
    row_background(ui, rect, selected, response.hovered(), colors);
    let icon = if entry.symlink {
        Icon::Link
    } else if entry.directory {
        Icon::Folder
    } else {
        Icon::Text
    };
    icon.paint(
        ui.painter(),
        Rect::from_min_size(rect.min + vec2(6.0, 5.0), vec2(18.0, 18.0)),
        colors.text_secondary,
    );
    paint_text(
        ui,
        rect,
        32.0,
        name_width - 38.0,
        &entry.name,
        colors.text_primary,
    );
    paint_text(
        ui,
        rect,
        name_width,
        85.0,
        &entry.size,
        colors.text_secondary,
    );
    paint_text(
        ui,
        rect,
        name_width + 90.0,
        rect.width() - name_width - 90.0,
        &entry.modified,
        colors.text_secondary,
    );
    response
}

enum TreeAction {
    Toggle(std::path::PathBuf),
    Open(std::path::PathBuf),
}
