//! Column navigation shares the browser's listings, history and preview cache.
use super::super::filesystem::Entry;
use super::*;
use std::collections::BTreeSet;
use std::path::Path;

const WIDTH: f32 = 230.0;
const PREVIEW: f32 = 300.0;

// Nearby parents reuse the sidebar's bounded directory jobs. Never enumerate
// siblings recursively or retain a second set of directory snapshots.
fn parents(path: &Path, width: f32) -> Vec<&Path> {
    let count = if width >= WIDTH * 3.0 + PREVIEW + 64.0 {
        2
    } else {
        1
    };
    let mut result: Vec<_> = path.ancestors().skip(1).take(count).collect();
    result.reverse();
    result
}

fn heading(ui: &mut Ui, path: &Path) {
    ui.add(
        egui::Label::new(
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy(),
        )
        .truncate(true),
    )
    .on_hover_text(display_path(path));
    ui.separator();
}

fn row(ui: &mut Ui, entry: &Entry, selected: bool, colors: WorkbenchColors) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), ROW_HEIGHT),
        Sense::click_and_drag(),
    );
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if selected || response.hovered() {
        painter.rect_filled(
            rect,
            3.0,
            if selected {
                colors.accent.gamma_multiply(0.22)
            } else {
                colors.recessed
            },
        );
    }
    let icon = entry_icon(entry);
    icon.paint(
        &painter,
        Rect::from_min_size(rect.min + vec2(4.0, 6.0), vec2(16.0, 16.0)),
        colors.text_secondary,
    );
    painter.text(
        rect.min + vec2(26.0, 6.0),
        egui::Align2::LEFT_TOP,
        &entry.name,
        FontId::proportional(13.0),
        colors.text_primary,
    );
    response.on_hover_text(display_path(&entry.path))
}

fn panel<R>(
    ui: &mut Ui,
    width: f32,
    height: f32,
    content: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.allocate_ui_with_layout(
        vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
            content(ui)
        },
    )
}

impl FileManager {
    pub(super) fn column_shortcuts(&mut self, ctx: &egui::Context) {
        if self.view == super::super::BrowserView::Columns {
            if key(ctx, Modifiers::NONE, Key::ArrowLeft) {
                let child = self.path().to_path_buf();
                self.parent();
                if self.pending.is_some() {
                    self.reveal_target = Some(child);
                }
            }
            if key(ctx, Modifiers::NONE, Key::ArrowRight) {
                let folder = self
                    .location()
                    .and_then(|location| location.selected.as_ref())
                    .is_some_and(|path| {
                        self.directory.as_ref().is_some_and(|directory| {
                            directory
                                .entries
                                .iter()
                                .any(|entry| &entry.path == path && entry.directory)
                        })
                    });
                if folder {
                    self.open_selected();
                }
            }
        }
    }

    pub(super) fn columns_ui(
        &mut self,
        ui: &mut Ui,
        colors: WorkbenchColors,
        palette: &AppPalette,
    ) {
        self.previews.poll(ui.ctx());
        let current = self.path().to_path_buf();
        let mut actions = ListActions::for_browser(self);
        let mut ancestor_navigation = None;
        let mut scroll = self.location().map_or(0.0, |location| location.scroll);
        let restore = std::mem::take(&mut self.restore_scroll);
        let reveal = std::mem::take(&mut self.reveal_selection);
        let height = ui.available_height().max(0.0);
        let route = parents(&current, ui.available_width());
        egui::ScrollArea::horizontal()
            .id_source("miller-columns")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for parent in route {
                        ui.push_id(parent, |ui| {
                            let column = panel(ui, WIDTH, height, |ui| {
                                if let Some(target) =
                                    self.column_ancestor(ui, parent, &current, restore, colors)
                                {
                                    ancestor_navigation = Some(target);
                                }
                            });
                            if ui.is_enabled() {
                                self.drop_targets.push((
                                    column.response.rect.intersect(ui.clip_rect()),
                                    parent.to_path_buf(),
                                ));
                            }
                        });
                        ui.separator();
                    }
                    let current_panel = panel(ui, WIDTH, height, |ui| {
                        scroll = self.column_current(ui, &mut actions, restore, reveal, colors);
                    });
                    if ui.is_enabled() {
                        self.drop_targets.push((
                            current_panel.response.rect.intersect(ui.clip_rect()),
                            current.clone(),
                        ));
                    }
                    if restore {
                        current_panel
                            .response
                            .scroll_to_me(Some(egui::Align::Center));
                    }
                    ui.separator();
                    panel(ui, PREVIEW, height, |ui| {
                        self.column_preview(ui, &mut actions, palette)
                    });
                });
            });
        if let Some(location) = self.location_mut() {
            location.scroll = scroll;
        }
        self.apply_list_actions(actions);
        if let Some((path, directory)) = ancestor_navigation {
            let target = if directory {
                path.clone()
            } else {
                path.parent().unwrap_or(&current).to_path_buf()
            };
            self.navigate(target, Navigation::New);
            if !directory {
                self.reveal_target = Some(path);
            }
        }
    }

    fn column_ancestor(
        &self,
        ui: &mut Ui,
        parent: &Path,
        current: &Path,
        restore: bool,
        colors: WorkbenchColors,
    ) -> Option<(PathBuf, bool)> {
        heading(ui, parent);
        let branch = self.tree.branches.get(parent);
        let Some(directory) = branch.and_then(|branch| branch.directory.as_ref()) else {
            ui.label(
                branch
                    .and_then(|branch| branch.error.as_deref())
                    .unwrap_or("Loading folder…"),
            );
            return None;
        };
        let entries: Vec<_> = directory
            .entries
            .iter()
            .filter(|entry| !entry.hidden || self.show_hidden)
            .collect();
        let mut target = None;
        let route_id = ui.id().with("ancestor-route");
        let changed_route = ui.ctx().data_mut(|data| {
            let changed = data.get_temp::<PathBuf>(route_id).as_deref() != Some(current);
            data.insert_temp(route_id, current.to_path_buf());
            changed
        });
        let mut area = egui::ScrollArea::vertical()
            .id_source("ancestor")
            .auto_shrink([false, false]);
        if (restore || changed_route)
            && let Some(index) = entries
                .iter()
                .position(|entry| current.starts_with(&entry.path))
        {
            area = area.vertical_scroll_offset(index as f32 * ROW_HEIGHT);
        }
        area.show_rows(ui, ROW_HEIGHT, entries.len(), |ui, range| {
            for entry in &entries[range] {
                let on_route = current.starts_with(&entry.path);
                let response = row(ui, entry, on_route, colors);
                if restore && on_route {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                if response.clicked() {
                    target = Some((entry.path.clone(), entry.directory));
                }
            }
        });
        target
    }

    fn column_current(
        &mut self,
        ui: &mut Ui,
        actions: &mut ListActions,
        restore: bool,
        reveal: bool,
        colors: WorkbenchColors,
    ) -> f32 {
        heading(ui, self.path());
        let can_edit = self.can_edit();
        let count = self.selection_count();
        let background = actions.background(ui, can_edit, count > 0);
        let selected = self.location().map(|location| &location.selection);
        let active = self
            .location()
            .and_then(|location| location.selected.as_ref());
        let mut area = egui::ScrollArea::vertical()
            .id_source(("miller-current", self.path()))
            .auto_shrink([false, false]);
        if restore {
            area = area
                .vertical_scroll_offset(self.location().map_or(0.0, |location| location.scroll));
        }
        if reveal
            && let Some(index) = self.visible.iter().position(|index| {
                self.directory
                    .as_ref()
                    .is_some_and(|directory| Some(&directory.entries[*index].path) == active)
            })
        {
            area = area.vertical_scroll_offset(index as f32 * ROW_HEIGHT);
        }
        let output = area.show_rows(ui, ROW_HEIGHT, self.visible.len(), |ui, range| {
            let Some(directory) = self.directory.as_ref() else {
                return;
            };
            for index in range {
                let entry = &directory.entries[self.visible[index]];
                let marked = selected.is_some_and(|paths| paths.contains(&entry.path));
                let response = row(ui, entry, marked, colors);
                actions.row(ui, &response, entry, selected, can_edit, count);
                if reveal && Some(&entry.path) == active {
                    response.scroll_to_me(None);
                }
                if response.clicked() && entry.directory && !ui.input(|input| input.modifiers.any())
                {
                    actions.open = Some(entry.path.clone());
                }
            }
        });
        if self.visible.is_empty() {
            ui.label("No visible files.");
        }
        if actions.transfer.is_none() {
            actions.transfer = drop_files(ui, &background, self.path());
        }
        output.state.offset.y
    }

    fn column_preview(&mut self, ui: &mut Ui, actions: &mut ListActions, palette: &AppPalette) {
        ui.label("Preview");
        ui.separator();
        let active = self
            .location()
            .and_then(|location| location.selected.as_ref());
        let preview = self
            .directory
            .as_ref()
            .and_then(|directory| {
                directory
                    .entries
                    .iter()
                    .find(|entry| Some(&entry.path) == active)
            })
            .cloned();
        let mut keep = BTreeSet::new();
        if let Some(entry) = preview {
            ui.add(egui::Label::new(&entry.name).truncate(true));
            ui.small(&entry.size);
            if ui.button("Open").clicked() {
                actions.open = Some(entry.path.clone());
            }
            if entry.directory {
                ui.label("Open this folder to browse its contents.");
            } else {
                keep.insert(entry.path.clone());
                self.column_content(ui, &entry, palette);
            }
        } else {
            ui.label("Select a file to preview.");
        }
        self.previews.retain(&keep);
    }

    fn column_content(&mut self, ui: &mut Ui, entry: &Entry, palette: &AppPalette) {
        let Some(ready) = self.previews.listing(entry) else {
            ui.spinner();
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
            return;
        };
        if let Some(document) = &ready.data.document {
            egui::ScrollArea::vertical()
                .id_source("column-text-preview")
                .show(ui, |ui| document.excerpt_ui(ui, 0.0, 1, palette));
        } else if ready.texture.is_some() {
            let rect = ui.available_rect_before_wrap();
            self.previews.show(ui, rect, entry);
        } else {
            ui.label(&ready.data.detail);
            ui.label(&ready.data.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn column_route_is_bounded_ordered_and_includes_immediate_parent() {
        assert_eq!(
            parents(Path::new("/a/b/c/d"), 1200.0),
            [Path::new("/a/b"), Path::new("/a/b/c")]
        );
        assert_eq!(parents(Path::new("/a/b/c/d"), 800.0), [Path::new("/a/b/c")]);
        assert_eq!(parents(Path::new("/a"), 800.0), [Path::new("/")]);
        assert!(parents(Path::new("/"), 800.0).is_empty());
    }
}
