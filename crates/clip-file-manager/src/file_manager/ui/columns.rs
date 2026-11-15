//! Column navigation shares the browser's listings, history and preview cache.
use super::super::filesystem::Entry;
use super::*;
use std::path::Path;

const WIDTH: f32 = 230.0;
const DIVIDER: f32 = 6.0;
mod path_bar;

// Ancestors remain real columns; horizontal scrolling changes visibility, not
// the folder chain. Archive labels resolve back to the same held snapshots.
fn parents(path: &Path) -> Vec<&Path> {
    let mut route: Vec<_> = path.ancestors().skip(1).collect();
    route.reverse();
    route
}
fn column_width(ui: &Ui, path: &Path, default: f32) -> f32 {
    ui.ctx()
        .data(|data| data.get_temp::<f32>(Id::new(("miller-column-width", path))))
        .unwrap_or(default)
}
fn divider(ui: &mut Ui, path: &Path, width: f32, height: f32, colors: WorkbenchColors) {
    let (rect, _) = ui.allocate_exact_size(vec2(DIVIDER, height), Sense::hover());
    // Directory loads and preview widgets change automatic widget IDs. Keep
    // the active resize gesture attached to its folder across those frames.
    let response = ui.interact(
        rect,
        Id::new(("miller-divider", path)),
        Sense::click_and_drag(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_text("Drag to resize column; double-click to reset width");
    let id = Id::new(("miller-column-width", path));
    if response.double_clicked() {
        ui.ctx().data_mut(|data| data.remove::<f32>(id));
    } else if response.dragged() {
        let resized = (width + response.drag_delta().x).clamp(160.0, 960.0);
        ui.ctx().data_mut(|data| data.insert_temp(id, resized));
    }
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        egui::Stroke::new(
            if response.hovered() || response.dragged() {
                2.0_f32
            } else {
                1.0_f32
            },
            if response.hovered() || response.dragged() {
                colors.accent
            } else {
                colors.text_secondary.gamma_multiply(0.3)
            },
        ),
    );
}

fn heading(ui: &mut Ui, path: &Path) {
    ui.add_sized(
        vec2(ui.available_width(), ROW_HEIGHT),
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

fn row(
    ui: &mut Ui,
    entry: &Entry,
    selected: bool,
    colors: WorkbenchColors,
    annotated: bool,
) -> egui::Response {
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
    paint_text(
        ui,
        rect,
        26.0,
        rect.width() - if annotated { 62.0 } else { 48.0 },
        &entry.name,
        colors.text_primary,
    );
    if entry.directory || super::super::container_preview::inventory::archive(&entry.path) {
        painter.text(
            rect.right_center() - vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            "›",
            FontId::proportional(18.0),
            colors.text_secondary,
        );
    }
    response.on_hover_text(display_path(&entry.path))
}

impl FileManager {
    // Both kinds of column share clipping, allocation and background drops.
    fn column_panel<R>(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        width: f32,
        height: f32,
        content: impl FnOnce(&mut Self, &mut Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let panel = ui.allocate_ui_with_layout(
            vec2(width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_size(vec2(width, height));
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                content(self, ui)
            },
        );
        if ui.is_enabled() {
            self.drop_targets.push((
                panel.response.rect.intersect(ui.clip_rect()),
                path.to_path_buf(),
            ));
        }
        panel
    }

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
        let logical = self
            .archives
            .label(&current)
            .unwrap_or_else(|| current.clone());
        let viewport = ui.available_size();
        let viewport_id = ui.id().with("miller-viewport");
        let changed = ui.ctx().data_mut(|data| {
            let old = data.get_temp::<(PathBuf, egui::Vec2)>(viewport_id);
            let next = (current.clone(), viewport);
            data.insert_temp(viewport_id, next.clone());
            old != Some(next)
        });
        // History owns its saved vertical offset. A window resize can reveal
        // the active row, but returning through history must not replace it.
        self.reveal_selection |= changed && !self.restore_scroll;
        let mut navigation = None;
        let logical_route = parents(&logical);
        let route: Vec<_> = logical_route
            .iter()
            .map(|path| self.archives.resolve(path))
            .collect();
        let widths: Vec<_> = route
            .iter()
            .map(|path| column_width(ui, path, WIDTH))
            .collect();
        let primary_width = column_width(
            ui,
            &current,
            (viewport.x * 0.6).clamp(320.0, 760.0).min(viewport.x),
        );
        let content_width =
            widths.iter().map(|width| width + DIVIDER).sum::<f32>() + primary_width + DIVIDER;
        let restore = self.restore_scroll;
        let height = (viewport.y - 30.0).max(0.0);
        let mut area = egui::ScrollArea::horizontal()
            .id_source("miller-columns")
            .max_height(height)
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
        if changed || restore {
            area = area.horizontal_scroll_offset((content_width - viewport.x).max(0.0));
        }
        area.show_viewport(ui, |ui, viewport| {
            ui.set_min_width(content_width);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal_top(|ui| {
                for (index, (parent, width)) in route.iter().zip(&widths).enumerate() {
                    ui.push_id(parent, |ui| {
                        let selected = logical_route.get(index + 1).copied().unwrap_or(&logical);
                        let selected = parent.join(selected.file_name().unwrap_or_default());
                        self.column_panel(ui, parent, *width, viewport.height(), |browser, ui| {
                            if let Some(target) = browser.column_ancestor(
                                ui,
                                parent,
                                &selected,
                                restore || changed,
                                colors,
                            ) {
                                navigation = Some(target);
                            }
                        });
                    });
                    divider(ui, parent, *width, viewport.height(), colors);
                }
                // Keep the shared rich List as the last column, rather than a
                // second preview pane with different readers and interactions.
                ui.push_id("miller-current-column", |ui| {
                    self.column_panel(
                        ui,
                        &current,
                        primary_width,
                        viewport.height(),
                        |browser, ui| {
                            if ui.clip_rect().intersects(ui.max_rect()) {
                                browser.list_ui(ui, colors, palette);
                            } else {
                                browser.previews.retain(&Default::default());
                                browser.music_previews.retain(&Default::default());
                                browser.video_previews.clear();
                                browser.containers.retire();
                            }
                        },
                    )
                });
                divider(ui, &current, primary_width, viewport.height(), colors);
            });
        });
        if let Some(path) = self.column_path_bar(ui, colors) {
            navigation = Some((path, true));
        }
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
        &mut self,
        ui: &mut Ui,
        parent: &Path,
        current: &Path,
        restore: bool,
        colors: WorkbenchColors,
    ) -> Option<(PathBuf, bool)> {
        if !ui.clip_rect().intersects(ui.max_rect()) {
            return None;
        }
        self.tree
            .branches
            .entry(parent.to_path_buf())
            .or_default()
            .expanded = true;
        heading(
            ui,
            &self
                .archives
                .label(parent)
                .unwrap_or_else(|| parent.to_path_buf()),
        );
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
                let response = row(
                    ui,
                    entry,
                    on_route,
                    colors,
                    self.organization.snapshot.color(entry).is_some(),
                );
                super::organization::paint_session(
                    ui,
                    response.rect,
                    entry,
                    &self.organization.snapshot,
                    true,
                );
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
    fn divider_drag_resizes_inside_horizontal_scroll_area() {
        let ctx = egui::Context::default();
        let path = Path::new("/parent");
        let mut center = pos2(0.0, 0.0);
        for events in [
            vec![],
            vec![
                egui::Event::PointerMoved(pos2(110.0, 50.0)),
                egui::Event::PointerButton {
                    pos: pos2(110.0, 50.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            vec![egui::Event::PointerMoved(pos2(150.0, 50.0))],
            vec![egui::Event::PointerMoved(pos2(190.0, 50.0))],
        ] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 220.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::horizontal()
                            .horizontal_scroll_offset(200.0)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.horizontal_top(|ui| {
                                    let width = column_width(ui, path, 300.0);
                                    ui.allocate_space(vec2(width, 160.0));
                                    center = ui.cursor().min + vec2(3.0, 40.0);
                                    divider(
                                        ui,
                                        path,
                                        width,
                                        160.0,
                                        WorkbenchColors::from_palette(&AppPalette::default()),
                                    );
                                    ui.allocate_space(vec2(300.0, 160.0));
                                });
                            });
                    });
                },
            );
        }
        assert!(
            ctx.data(|d| d.get_temp::<f32>(Id::new(("miller-column-width", path))))
                .is_some_and(|w| w > 340.0),
            "divider at {center:?}, dragged={:?}",
            ctx.dragged_id()
        );
    }

    #[test]
    fn parent_columns_retain_the_complete_ordered_chain() {
        assert_eq!(
            parents(Path::new("/a/b/c/d")),
            [
                Path::new("/"),
                Path::new("/a"),
                Path::new("/a/b"),
                Path::new("/a/b/c")
            ]
        );
        assert!(parents(Path::new("/")).is_empty());
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
        let (root, mut browser, target) = long_listing();
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
