//! Virtualized tiles share selection, drag/drop, menus and file operations with the list.
use super::*;

impl FileManager {
    pub(in crate::file_manager) fn set_view(&mut self, view: super::super::BrowserView) {
        if view == self.view {
            return;
        }
        self.type_navigation.clear();
        self.retire_media_previews();
        if self.view == super::super::BrowserView::Constellation
            || matches!(
                view,
                super::super::BrowserView::List
                    | super::super::BrowserView::Videos
                    | super::super::BrowserView::Constellation
            )
        {
            self.stop_previews();
        }
        if view == super::super::BrowserView::Music && self.view != view {
            self.shelf.open = false;
        }
        if view == super::super::BrowserView::Constellation {
            let path = self.path().to_path_buf();
            self.constellation.check_and_load_folder(&path);
        }
        self.view = view;
        if let Some(location) = self.location_mut() {
            location.manual_view = Some(view);
            location.scroll = 0.0;
        }
        self.restore_scroll = true;
    }
    pub(in crate::file_manager) fn retire_media_previews(&mut self) {
        self.constellation.stop_playback();
        self.midi.stop();
        self.gallery_layout.retire();
        self.similarity_canvas.retire();
        self.video_previews.clear();
        self.previews.retain(&Default::default());
        self.gallery_previews.retain(&Default::default());
        self.music_previews.retain(&Default::default());
    }
    pub(super) fn media_view_toolbar(&mut self, ui: &mut Ui, palette: &AppPalette) {
        use crate::ui_tabs::{Tab, TabBar};
        ui.horizontal_wrapped(|ui| {
            if let Some(view) = TabBar::new(
                "files_media_views",
                self.view,
                &[
                    Tab::new(super::super::BrowserView::List, "List"),
                    Tab::new(super::super::BrowserView::Photos, "Photos"),
                    Tab::new(super::super::BrowserView::Music, "Music"),
                    Tab::new(super::super::BrowserView::Videos, "Videos"),
                    Tab::new(super::super::BrowserView::Constellation, "Constellation"),
                ],
                palette,
            )
            .show(ui)
            {
                self.set_view(view);
            }
            self.transfer_toolbar(ui);
            if self.view == super::super::BrowserView::Photos {
                if ui
                    .button(if self.gallery_selecting {
                        "Done"
                    } else {
                        "Select"
                    })
                    .clicked()
                {
                    self.gallery_selecting = !self.gallery_selecting;
                }
                ui.label("Select multiple: Select mode, Ctrl+click, or Shift+click.");
            }
            if self.view == super::super::BrowserView::Photos
                && self.photo_mode == super::super::gallery::similarity::PhotoMode::Masonry
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
            if self.view == super::super::BrowserView::Photos
                && self.photo_mode == super::super::gallery::similarity::PhotoMode::Similarity
            {
                if ui.button("Reset View").clicked() {
                    self.similarity_canvas.reset_view();
                }
            }
        });
    }

    pub(super) fn gallery_ui(
        &mut self,
        ui: &mut Ui,
        colors: WorkbenchColors,
        palette: &AppPalette,
    ) {
        if self.photo_mode == super::super::gallery::similarity::PhotoMode::Similarity {
            self.similarity_gallery_ui(ui, colors, palette);
            return;
        }

        self.gallery_previews.poll(ui.ctx());
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(1.0);
        let columns = ((width + 2.0) / (self.thumbnail_edge + 2.0))
            .floor()
            .clamp(1.0, 62.0) as usize;
        self.gallery_columns = columns;
        let unit = ((width + 2.0) / (columns * 2) as f32).max(1.0);
        if let Some(directory) = &self.directory {
            self.gallery_layout
                .sync(directory, &self.visible, self.show_hidden, columns * 2);
        }
        let offset = self.location().map_or(0.0, |location| location.scroll) / unit;
        let correction =
            self.gallery_layout
                .prepare(ui.ctx(), offset, ui.available_height() / unit);
        let mut scroll = self.gallery_scroll(ui, unit);
        if let Some(offset) = correction {
            scroll = scroll.vertical_scroll_offset(offset * unit);
        }
        let Some(directory) = &self.directory else {
            ui.label("Loading folder…");
            return;
        };
        if self.visible.is_empty() {
            ui.label("No visible items.");
        }
        let selected = current_selection(&self.history, self.index);
        let can_edit = self.can_edit();
        let count = self.selection_count();
        let mut actions = ListActions {
            additive: self.gallery_selecting,
            ..Default::default()
        };
        let check_color = crate::ui_ext::contrast_text_c32(colors.accent, None, palette);
        let background = actions.background(ui, can_edit, count > 0);
        let mut targets = Vec::new();
        let mut paths = std::collections::BTreeSet::new();
        let output = scroll.show_viewport(ui, |ui, viewport| {
            ui.set_min_height((self.gallery_layout.height() * unit - 2.0).max(0.0));
            let origin = ui.min_rect().min;
            let min = (viewport.min.y / unit - 2.0).max(0.0);
            let max = viewport.max.y / unit + 2.0;
            self.gallery_layout
                .set_viewport(viewport.min.y / unit, viewport.max.y / unit);
            let tiles = self.gallery_layout.placements(min, max);
            let mut priority: Vec<_> = tiles.iter().collect();
            priority.sort_by(|a, b| {
                let distance = |tile: &&super::super::gallery::Placement| {
                    (tile.y * unit - viewport.center().y).abs()
                };
                distance(a).total_cmp(&distance(b))
            });
            paths = priority
                .iter()
                .map(|tile| directory.entries[self.visible[tile.index]].path.clone())
                .collect();
            self.gallery_previews.retain(&paths);
            for tile in priority.iter() {
                let entry = &directory.entries[self.visible[tile.index]];
                if tile.ready && entry.is_photo() {
                    let edge = tile.thumbnail_edge(unit, ui.ctx().pixels_per_point());
                    self.gallery_previews
                        .request_gallery(entry, edge, tile.orientation);
                }
            }
            for tile in tiles {
                let index = tile.index;
                let entry = &directory.entries[self.visible[index]];
                let rect = Rect::from_min_size(
                    origin + vec2(tile.x as f32 * unit, tile.y * unit),
                    vec2(
                        (tile.width as f32 * unit - 2.0).max(1.0),
                        (tile.height as f32 * unit - 2.0).max(1.0),
                    ),
                );
                let response = ui.interact(
                    rect,
                    Id::new(("gallery_tile", &entry.path)),
                    Sense::click_and_drag(),
                );
                let marked = selected.is_some_and(|s| s.contains(&entry.path));
                let photo = entry.is_photo();
                let preview = if photo && paths.contains(&entry.path) {
                    Some(&mut self.gallery_previews)
                } else {
                    None
                };
                paint_gallery_tile(ui, &response, entry, marked, colors, check_color, preview);
                if !super::popup_open(ui.ctx()) {
                    response.clone().on_hover_text(&entry.name);
                }
                if entry.directory {
                    targets.push((rect.intersect(ui.clip_rect()), entry.path.clone()));
                }
                actions.row(ui, &response, entry, selected, can_edit, count);
            }
        });
        self.gallery_previews.retain(&paths);
        if ui.is_enabled() {
            self.drop_targets.extend(targets);
        }
        self.finish_listing(ui, actions, output.state.offset.y, &background);
    }

    fn gallery_scroll(&mut self, _ui: &Ui, unit: f32) -> egui::ScrollArea {
        let mut scroll = egui::ScrollArea::vertical()
            .id_source("files_photo_grid")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
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
                scroll =
                    scroll.vertical_scroll_offset(self.gallery_layout.item_offset(index) * unit);
            }
            self.reveal_selection = false;
        }
        scroll
    }

    pub(super) fn similarity_gallery_ui(
        &mut self,
        ui: &mut Ui,
        colors: WorkbenchColors,
        palette: &AppPalette,
    ) {
        self.similarity_canvas.poll(ui.ctx());
        self.gallery_previews.poll(ui.ctx());

        if let Some(directory) = &self.directory {
            self.similarity_canvas.sync(
                directory,
                &self.visible,
                self.photo_recurse,
                self.show_hidden,
            );
        }

        let selected = current_selection(&self.history, self.index);
        let can_edit = self.can_edit();
        let count = self.selection_count();
        let mut actions = ListActions {
            additive: self.gallery_selecting,
            ..Default::default()
        };
        let check_color = crate::ui_ext::contrast_text_c32(colors.accent, None, palette);

        let (canvas_rect, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

        self.similarity_canvas
            .handle_input(ui, canvas_rect, &response);

        // Fill background
        ui.painter().rect_filled(canvas_rect, 0.0, colors.canvas);

        let clip_rect = canvas_rect.intersect(ui.clip_rect());
        let dpi = ui.ctx().pixels_per_point();
        let target_edge = (self.similarity_canvas.tile_size * self.similarity_canvas.zoom * dpi)
            .ceil()
            .clamp(64.0, 2048.0) as u32;

        let mut visible_paths = std::collections::BTreeSet::new();

        for placement in &self.similarity_canvas.placements {
            let Some(entry) = self.similarity_canvas.entries.get(placement.index) else {
                continue;
            };

            let tile_rect = self.similarity_canvas.tile_screen_rect(
                canvas_rect.min,
                placement.col,
                placement.row,
            );

            if !tile_rect.intersects(clip_rect) {
                continue;
            }

            visible_paths.insert(entry.path.clone());
            self.gallery_previews.request_gallery(entry, target_edge, 1);

            let tile_response = ui.interact(
                tile_rect,
                Id::new(("sim_tile", &entry.path)),
                Sense::click_and_drag(),
            );

            let marked = selected.is_some_and(|s| s.contains(&entry.path));
            paint_gallery_tile(
                ui,
                &tile_response,
                entry,
                marked,
                colors,
                check_color,
                Some(&mut self.gallery_previews),
            );

            if !super::popup_open(ui.ctx()) {
                tile_response.clone().on_hover_text(&entry.name);
            }

            actions.row(ui, &tile_response, entry, selected, can_edit, count);
        }

        self.gallery_previews.retain(&visible_paths);

        // Floating HUD pill
        let hud_id = Id::new("similarity_canvas_hud");
        let hud_pos = pos2(canvas_rect.right() - 250.0, canvas_rect.bottom() - 44.0);
        egui::Area::new(hud_id)
            .order(egui::Order::Foreground)
            .fixed_pos(hud_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(colors.panel)
                    .stroke(egui::Stroke::new(1.0, colors.border))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.small_button("Fit").clicked() {
                                self.similarity_canvas.fit_to_view(canvas_rect);
                            }
                            ui.label(format!("{:.0}%", self.similarity_canvas.zoom * 100.0));
                            ui.separator();
                            if self.similarity_canvas.is_scanning() {
                                ui.label("Scanning subfolders…");
                            } else if self.similarity_canvas.is_hashing() {
                                ui.label(format!(
                                    "Clustering {}/{}…",
                                    self.similarity_canvas.hashing_progress.0,
                                    self.similarity_canvas.hashing_progress.1
                                ));
                            } else {
                                ui.label(format!(
                                    "{} photos",
                                    self.similarity_canvas.entries.len()
                                ));
                            }
                        });
                    });
            });

        self.finish_listing(ui, actions, 0.0, &response);
    }

    pub(in crate::file_manager) fn photo_collection(&self, path: &std::path::Path) -> Vec<PathBuf> {
        if !little_clip_core::item::is_image_path(path) {
            return Vec::new();
        }
        if self.photo_mode == super::super::gallery::similarity::PhotoMode::Similarity {
            let collection = self.similarity_canvas.photo_collection();
            if !collection.is_empty() {
                return collection;
            }
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

/// Paints a center crop and its selection feedback within the tile boundary.
fn paint_gallery_tile(
    ui: &mut Ui,
    response: &egui::Response,
    entry: &super::super::filesystem::Entry,
    marked: bool,
    colors: WorkbenchColors,
    check_color: Color32,
    preview: Option<&mut super::super::previews::EntryPreviews>,
) {
    let rect = response.rect;
    ui.painter().rect_filled(
        rect,
        0.0,
        if marked {
            colors.accent
        } else {
            colors.recessed
        },
    );
    let image_rect = rect.shrink(if marked { 7.0 } else { 0.0 });
    if let Some(preview) = preview {
        preview.show_gallery(ui, image_rect, entry);
    } else {
        entry_icon(entry).paint(
            ui.painter(),
            Rect::from_center_size(image_rect.center(), vec2(32.0, 32.0)),
            colors.text_secondary,
        );
        paint_text(
            ui,
            Rect::from_min_size(
                pos2(image_rect.left(), image_rect.bottom() - 28.0),
                vec2(image_rect.width(), 28.0),
            ),
            4.0,
            image_rect.width() - 8.0,
            &entry.name,
            colors.text_primary,
        );
    }
    if marked {
        let badge = pos2(rect.right() - 18.0, rect.bottom() - 18.0);
        ui.painter().circle_filled(badge, 12.0, colors.accent);
        Icon::Check.paint(
            ui.painter(),
            Rect::from_center_size(badge, vec2(16.0, 16.0)),
            check_color,
        );
    } else if response.hovered() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            0.0,
            egui::Stroke::new(2.0_f32, colors.focus),
        );
    }
}
