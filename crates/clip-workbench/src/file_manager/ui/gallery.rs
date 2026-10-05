//! Virtualized tiles share selection, drag/drop, menus and file operations with the list.
use super::*;

impl FileManager {
    pub(in crate::file_manager) fn set_view(&mut self, view: super::super::BrowserView) {
        if view == super::super::BrowserView::Music && self.view != view {
            self.shelf.open = false;
        }
        self.view = view;
        self.music_previews.retain(&Default::default());
        self.previews.retain(&Default::default());
        self.gallery_previews.retain(&Default::default());
        if let Some(location) = self.location_mut() {
            location.manual_view = Some(view);
            location.scroll = 0.0;
        }
        self.restore_scroll = true;
    }
    pub(super) fn media_view_toolbar(&mut self, ui: &mut Ui, palette: &AppPalette) {
        use crate::ui_tabs::{Tab, TabBar};
        ui.horizontal_wrapped(|ui| {
            if !self.audio_entries.is_empty() && ui.button("Play folder music").clicked() {
                let audio = self.audio_entries.clone();
                let path = self
                    .location()
                    .and_then(|l| l.selected.as_ref())
                    .filter(|p| audio.contains(p))
                    .cloned()
                    .unwrap_or_else(|| audio[0].clone());
                self.music.play_folder(audio, &path);
            }
            if let Some(view) = TabBar::new(
                "files_media_views",
                self.view,
                &[
                    Tab::new(super::super::BrowserView::List, "List"),
                    Tab::new(super::super::BrowserView::Photos, "Photos"),
                    Tab::new(super::super::BrowserView::Music, "Music"),
                ],
                palette,
            )
            .show(ui)
            {
                self.set_view(view);
            }
            if self.view == super::super::BrowserView::Photos
                && ui
                    .add(
                        egui::Slider::new(&mut self.thumbnail_edge, 80.0..=280.0)
                            .text("Thumbnail size"),
                    )
                    .changed()
            {
                if let Some(location) = self.location_mut() {
                    location.scroll = 0.0;
                }
                self.restore_scroll = true;
            }
        });
    }

    pub(super) fn gallery_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        self.gallery_previews.poll(ui.ctx());
        let columns = ((ui.available_width() + 8.0) / (self.thumbnail_edge + 8.0))
            .floor()
            .max(1.0) as usize;
        self.gallery_columns = columns;
        let edge = ((ui.available_width() - 8.0 * (columns - 1) as f32) / columns as f32).max(1.0);
        let row_height = edge + 32.0;
        let scroll = self.gallery_scroll(ui, columns, row_height);
        let Some(directory) = &self.directory else {
            ui.label("Loading folder…");
            return;
        };
        if self.visible.is_empty() {
            ui.label("No visible items.");
        }
        let selected = self
            .index
            .and_then(|index| self.history.get(index))
            .map(|location| &location.selection);
        let can_edit = self.can_edit();
        let count = self.selection_count();
        let mut actions = ListActions::default();
        let background = actions.background(ui, can_edit, count > 0);
        let mut targets = Vec::new();
        let mut paths = std::collections::BTreeSet::new();
        let output = scroll.show_rows(
            ui,
            row_height,
            self.visible.len().div_ceil(columns),
            |ui, rows| {
                for row in rows {
                    ui.horizontal(|ui| {
                        for index in row * columns..((row + 1) * columns).min(self.visible.len()) {
                            let entry = &directory.entries[self.visible[index]];
                            let (rect, response) = ui.allocate_exact_size(
                                vec2(edge, row_height),
                                Sense::click_and_drag(),
                            );
                            row_background(
                                ui,
                                rect,
                                selected.is_some_and(|s| s.contains(&entry.path)),
                                response.hovered(),
                                colors,
                            );
                            let image_rect = Rect::from_min_size(
                                rect.min + vec2(4.0, 4.0),
                                vec2((edge - 8.0).max(1.0), (edge - 8.0).max(1.0)),
                            );
                            if !entry.directory
                                && little_clip_core::item::is_image_path(&entry.path)
                                && paths.len() < 128
                            {
                                paths.insert(entry.path.clone());
                                self.gallery_previews.show_gallery(ui, image_rect, entry);
                            } else {
                                let icon = entry_icon(entry);
                                icon.paint(
                                    ui.painter(),
                                    Rect::from_center_size(image_rect.center(), vec2(32.0, 32.0)),
                                    colors.text_secondary,
                                );
                            }
                            paint_text(
                                ui,
                                Rect::from_min_size(
                                    pos2(rect.left(), rect.top() + edge),
                                    vec2(edge, 28.0),
                                ),
                                4.0,
                                edge - 8.0,
                                &entry.name,
                                colors.text_primary,
                            );
                            response.clone().on_hover_text(&entry.name);
                            if entry.directory {
                                targets.push((rect.intersect(ui.clip_rect()), entry.path.clone()));
                            }
                            actions.row(ui, &response, entry, selected, can_edit, count);
                        }
                    });
                }
            },
        );
        self.gallery_previews.retain(&paths);
        if ui.is_enabled() {
            self.drop_targets.extend(targets);
        }
        self.finish_listing(ui, actions, output.state.offset.y, &background);
    }

    fn gallery_scroll(&mut self, ui: &Ui, columns: usize, row_height: f32) -> egui::ScrollArea {
        let mut scroll = egui::ScrollArea::vertical()
            .id_source("files_photo_grid")
            .auto_shrink([false, false]);
        if self.restore_scroll {
            scroll = scroll.vertical_scroll_offset(self.location().map_or(0.0, |l| l.scroll));
            self.restore_scroll = false;
        }
        if self.reveal_selection {
            if let Some(index) = self.visible.iter().position(|index| {
                self.directory.as_ref().is_some_and(|d| {
                    self.location().and_then(|l| l.selected.as_ref())
                        == Some(&d.entries[*index].path)
                })
            }) {
                scroll = scroll.vertical_scroll_offset(
                    (index / columns) as f32 * (row_height + ui.spacing().item_spacing.y),
                );
            }
            self.reveal_selection = false;
        }
        scroll
    }

    pub(in crate::file_manager) fn photo_collection(&self, path: &std::path::Path) -> Vec<PathBuf> {
        if self.view != super::super::BrowserView::Photos
            || !little_clip_core::item::is_image_path(path)
        {
            return Vec::new();
        }
        self.directory
            .as_ref()
            .map(|directory| {
                self.visible
                    .iter()
                    .map(|i| &directory.entries[*i])
                    .filter(|e| !e.directory && little_clip_core::item::is_image_path(&e.path))
                    .map(|e| e.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}
