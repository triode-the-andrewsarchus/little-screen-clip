//! Column navigation shares the browser's listings, history and preview cache.
use super::super::filesystem::Entry;
use super::*;
use std::path::Path;

const WIDTH: f32 = 230.0;
const RAIL: f32 = 116.0;

// Every ancestor remains in the compact path rail. The nearest folder also
// exposes siblings when the viewport can afford a full navigation column.
fn parents(path: &Path, width: f32) -> Vec<&Path> {
    if width >= 760.0 {
        path.parent().into_iter().collect()
    } else {
        Vec::new()
    }
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
                let child = self
                    .archive_context()
                    .filter(|snapshot| snapshot.root == self.path())
                    .map_or_else(
                        || self.path().to_path_buf(),
                        |snapshot| snapshot.source.path().to_path_buf(),
                    );
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
                            directory.entries.iter().any(|entry| {
                                &entry.path == path
                                    && (entry.directory
                                        || super::super::container_preview::inventory::archive(
                                            &entry.path,
                                        ))
                            })
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
        let current = self.path().to_path_buf();
        let viewport = ui.available_size();
        let viewport_id = ui.id().with("miller-viewport");
        let resized = ui.ctx().data_mut(|data| {
            let changed = data
                .get_temp::<egui::Vec2>(viewport_id)
                .is_some_and(|old| old != viewport);
            data.insert_temp(viewport_id, viewport);
            changed
        });
        self.reveal_selection |= resized;
        let mut navigation = None;
        let logical = self
            .archives
            .label(&current)
            .unwrap_or_else(|| current.clone());
        let branch = self
            .archive_context()
            .filter(|snapshot| snapshot.root == current)
            .map_or_else(
                || current.clone(),
                |snapshot| snapshot.source.path().to_path_buf(),
            );
        let route = parents(&branch, viewport.x);
        for parent in &route {
            self.tree
                .branches
                .entry(parent.to_path_buf())
                .or_default()
                .expanded = true;
        }
        let restore = self.restore_scroll;
        ui.horizontal_top(|ui| {
            panel(ui, RAIL, viewport.y, |ui| {
                ui.strong("Path");
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("column-path")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut ancestors: Vec<_> = logical.ancestors().collect();
                        ancestors.reverse();
                        for path in ancestors {
                            let selected = path == logical;
                            let label = path
                                .file_name()
                                .unwrap_or(path.as_os_str())
                                .to_string_lossy();
                            let response = ui
                                .add_sized(
                                    [ui.available_width(), 32.0],
                                    egui::SelectableLabel::new(
                                        selected,
                                        egui::RichText::new(label).size(12.0),
                                    ),
                                )
                                .on_hover_text(display_path(path));
                            if response.clicked() && !selected {
                                navigation = Some((path.to_path_buf(), true));
                            }
                        }
                    });
            });
            ui.separator();
            for parent in route {
                ui.push_id(parent, |ui| {
                    let column = panel(ui, WIDTH, viewport.y, |ui| {
                        if let Some(target) =
                            self.column_ancestor(ui, parent, &branch, restore, colors)
                        {
                            navigation = Some(target);
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
            // The final column is the ordinary rich List, including its bounded
            // preview jobs, selection, scrolling, context actions and exports.
            ui.push_id("miller-current-column", |ui| {
                panel(ui, ui.available_width().max(0.0), viewport.y, |ui| {
                    self.list_ui(ui, colors, palette);
                });
            });
        });
        if let Some((path, directory)) = navigation {
            if !directory && super::super::container_preview::inventory::archive(&path) {
                self.open_archive(path);
                return;
            }
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
            area = area
                .vertical_scroll_offset(index as f32 * (ROW_HEIGHT + ui.spacing().item_spacing.y));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearby_column_is_optional_but_path_rail_can_represent_every_ancestor() {
        assert_eq!(
            parents(Path::new("/a/b/c/d"), 1200.0),
            [Path::new("/a/b/c")]
        );
        assert!(parents(Path::new("/a/b/c/d"), 700.0).is_empty());
        assert!(parents(Path::new("/"), 1200.0).is_empty());
    }

    fn long_listing() -> (tempfile::TempDir, FileManager, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        for index in 0..100 {
            std::fs::create_dir(root.path().join(format!("row{index:03}"))).unwrap();
        }
        let target = root.path().join("row099");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut browser = FileManager {
            pending: None,
            ..FileManager::default()
        };
        browser.commit(
            super::super::super::filesystem::read_directory(root.path(), &cancel).unwrap(),
            Navigation::New,
        );
        browser.set_view(super::super::super::BrowserView::Columns);
        browser.select(target.clone());
        browser
            .tree
            .branches
            .entry(root.path().into())
            .or_default()
            .directory =
            Some(super::super::super::filesystem::read_directory(root.path(), &cancel).unwrap());
        (root, browser, target)
    }

    #[test]
    fn column_selection_and_parent_route_reveal_include_virtual_row_spacing() {
        let (root, browser, target) = long_listing();
        for spacing in [0.0, 6.0, 12.0] {
            {
                let ctx = egui::Context::default();
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 220.0))),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.spacing_mut().item_spacing.y = spacing;
                            let colors = WorkbenchColors::from_palette(&AppPalette::default());
                            browser.column_ancestor(ui, root.path(), &target, true, colors);
                        });
                    },
                );
                assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "row099")), "Selected row was outside the painted virtual range: spacing={spacing}");
            }
        }
    }

    #[test]
    fn resizing_columns_keeps_active_row_in_current_viewport() {
        let (_root, mut browser, _target) = long_listing();
        browser.reveal_selection = true;
        let ctx = egui::Context::default();
        for (width, height) in [
            (1600.0, 600.0),
            (1600.0, 600.0),
            (700.0, 320.0),
            (700.0, 320.0),
        ] {
            browser.drop_targets.clear();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, height))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        browser.columns_ui(
                            ui,
                            WorkbenchColors::from_palette(&AppPalette::default()),
                            &AppPalette::default(),
                        )
                    });
                },
            );
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "row099" && shape.clip_rect.contains_rect(Rect::from_min_size(text.pos, text.galley.size())))), "Selected current-column row disappeared at {width}x{height}");
        }
    }
}
