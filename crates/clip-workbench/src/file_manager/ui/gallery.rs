//! Virtualized tiles share selection, drag/drop, menus and file operations with the list.
use super::*;
use crate::file_manager::gallery::similarity::canvas::CanvasGrid;

mod controls;
mod thumbnails;

const MIN_THUMBNAIL_EDGE: f32 = 80.0;
const MAX_THUMBNAIL_EDGE: f32 = 280.0;
const CTRL_ZOOM_SPEED: f32 = 0.4;
const SCROLL_SETTLE_SECONDS: f64 = 0.2;
const BADGE_DISPLAY_DURATION: f64 = 1.2;
const BADGE_FADE_DURATION: f64 = 0.3;
const CANVAS_EDGE_FADE_SIZE: f32 = 28.0;
const HUD_INSET_X: f32 = 20.0;
const SEARCH_HUD_INSET_Y: f32 = 16.0;
const BOTTOM_HUD_HEIGHT: f32 = 44.0;

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
            {
                let is_time = self.sort.column == super::super::sorting::Column::Modified;
                let sort_label = match (self.sort.column, self.sort.descending) {
                    (super::super::sorting::Column::Modified, true) => "Sort: Date ↓",
                    (super::super::sorting::Column::Modified, false) => "Sort: Date ↑",
                    (super::super::sorting::Column::Name, false) => "Sort: Name (A-Z)",
                    (super::super::sorting::Column::Name, true) => "Sort: Name (Z-A)",
                    _ => "Sort: Date ↓",
                };
                let sort_resp = ui.button(sort_label).on_hover_text(
                    "Switch photo sorting between Date and Name. Right-click for options.",
                );
                if sort_resp.clicked() {
                    let next_col = if is_time {
                        super::super::sorting::Column::Name
                    } else {
                        super::super::sorting::Column::Modified
                    };
                    self.sort_by(next_col);
                }
                sort_resp.context_menu(|ui| {
                    use super::super::sorting::{Column, Sort};
                    for (column, descending, label) in [
                        (Column::Modified, true, "Date (Newest first)"),
                        (Column::Modified, false, "Date (Oldest first)"),
                        (Column::Name, false, "Name (A to Z)"),
                        (Column::Name, true, "Name (Z to A)"),
                    ] {
                        let active =
                            self.sort.column == column && self.sort.descending == descending;
                        if ui.selectable_label(active, label).clicked() {
                            self.sort = Sort { column, descending };
                            self.gallery_layout.invalidate_order();
                            self.rebuild_visible();
                            ui.close_menu();
                        }
                    }
                });

                if ui
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
        self.gallery_hover.poll(ui.ctx());
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);

        let pointer_inside = ui.rect_contains_pointer(ui.available_rect_before_wrap());
        if pointer_inside {
            let ctrl_zoom = ui.input(|i| {
                if (i.modifiers.ctrl || i.modifiers.command) && i.raw_scroll_delta.y.abs() > 0.01 {
                    Some(i.raw_scroll_delta.y)
                } else {
                    None
                }
            });
            if let Some(delta) = ctrl_zoom {
                let old_edge = self.thumbnail_edge;
                let new_edge = (old_edge + delta * CTRL_ZOOM_SPEED)
                    .clamp(MIN_THUMBNAIL_EDGE, MAX_THUMBNAIL_EDGE);
                if (new_edge - old_edge).abs() > 0.1 {
                    self.thumbnail_edge = new_edge;
                }
                ui.input_mut(|i| {
                    i.raw_scroll_delta = egui::Vec2::ZERO;
                });
                ui.ctx().request_repaint();
            }
        }

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
        let mut retain_paths = std::collections::BTreeSet::new();
        let mut hovered_path = None;
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

            // 10 rows buffer above and below (nominal row height is 2.0 units)
            const BUFFER_ROWS_UNITS: f32 = 20.0;
            let retain_min = (min - BUFFER_ROWS_UNITS).max(0.0);
            let retain_max = max + BUFFER_ROWS_UNITS;
            let retain_tiles = self.gallery_layout.placements(retain_min, retain_max);
            retain_paths = retain_tiles
                .iter()
                .map(|tile| directory.entries[self.visible[tile.index]].path.clone())
                .collect();

            // Discard items more than 10 rows out of view and cancel in-flight fetches
            self.gallery_previews.retain(&retain_paths);

            // Track scroll settling (~0.2s without scrolling)
            let current_scroll_y = viewport.min.y;
            let now = ui.input(|i| i.time);
            if (current_scroll_y - self.gallery_last_scroll_y).abs() > 1.0 {
                self.gallery_last_scroll_y = current_scroll_y;
                self.gallery_last_scroll_time = now;
            }
            let dpi = ui.ctx().pixels_per_point();
            let requests = priority
                .iter()
                .filter_map(|tile| {
                    thumbnails::Request::from_tile(
                        tile,
                        &directory.entries[self.visible[tile.index]],
                        unit,
                        dpi,
                    )
                })
                .collect::<Vec<_>>();
            let mut neighbors = retain_tiles
                .iter()
                .filter(|tile| !paths.contains(&directory.entries[self.visible[tile.index]].path))
                .collect::<Vec<_>>();
            neighbors.sort_by(|a, b| {
                let distance = |tile: &&super::super::gallery::Placement| {
                    (tile.y * unit - viewport.center().y).abs()
                };
                distance(a).total_cmp(&distance(b))
            });
            let neighbors = neighbors
                .into_iter()
                .filter_map(|tile| {
                    thumbnails::Request::from_tile(
                        tile,
                        &directory.entries[self.visible[tile.index]],
                        unit,
                        dpi,
                    )
                })
                .collect::<Vec<_>>();
            thumbnails::schedule(
                &mut self.gallery_previews,
                ui.ctx(),
                &requests,
                &neighbors,
                now - self.gallery_last_scroll_time,
            );

            for tile in &tiles {
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
                let preview = if photo && retain_paths.contains(&entry.path) {
                    Some(&mut self.gallery_previews)
                } else {
                    None
                };
                paint_gallery_tile(
                    ui,
                    &response,
                    entry,
                    marked,
                    colors,
                    check_color,
                    preview,
                    Some(&mut self.gallery_hover),
                    &mut hovered_path,
                );
                if entry.directory {
                    targets.push((rect.intersect(ui.clip_rect()), entry.path.clone()));
                }
                actions.row(ui, &response, entry, selected, can_edit, count);
            }

            controls::scroll_badge(
                ui,
                tiles
                    .iter()
                    .min_by(|a, b| a.y.total_cmp(&b.y))
                    .map(|tile| &directory.entries[self.visible[tile.index]]),
                self.sort.column,
                self.gallery_last_scroll_time,
                colors,
            );
        });
        self.gallery_previews.retain(&retain_paths);
        self.gallery_hover.retain(hovered_path.as_deref());
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
        self.gallery_hover.poll(ui.ctx());

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

        let prev_clip = ui.clip_rect();
        let clip_rect = canvas_rect.intersect(prev_clip);
        ui.set_clip_rect(clip_rect);

        self.similarity_canvas
            .handle_input(ui, canvas_rect, &response);

        // Fill background
        ui.painter().rect_filled(canvas_rect, 0.0, colors.canvas);
        let dpi = ui.ctx().pixels_per_point();
        let target_edge = (self.similarity_canvas.tile_size * self.similarity_canvas.zoom * dpi)
            .ceil()
            .clamp(64.0, 2048.0) as u32;

        let mut visible_paths = std::collections::BTreeSet::new();
        let mut hovered_path = None;
        let middle_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));

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
                Id::new(("sim_tile", &entry.path, placement.col, placement.row)),
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
                Some(&mut self.gallery_hover),
                &mut hovered_path,
            );

            let middle_active = middle_down
                || self.similarity_canvas.is_middle_panning
                || tile_response.clicked_by(egui::PointerButton::Middle)
                || tile_response.dragged_by(egui::PointerButton::Middle);

            if !middle_active {
                actions.row(ui, &tile_response, entry, selected, can_edit, count);
            }
        }

        self.gallery_previews.retain(&visible_paths);
        self.gallery_hover.retain(hovered_path.as_deref());

        let header_actions =
            controls::grid_headers(ui, &self.similarity_canvas, canvas_rect, colors);
        for action in header_actions {
            self.apply_similarity_header_action(action);
        }

        // Paint edge fade vignette on top of canvas tiles, dividers and headers
        ui_ext::paint_canvas_edge_fade(
            ui.painter(),
            clip_rect,
            colors.canvas,
            CANVAS_EDGE_FADE_SIZE,
        );

        self.similarity_search_hud(ui, canvas_rect, colors);
        self.similarity_navigation_hud(ui, canvas_rect, colors);
        self.similarity_status_hud(ui, canvas_rect, colors);

        self.finish_listing(ui, actions, 0.0, &response);
        ui.set_clip_rect(prev_clip);
    }

    pub(in crate::file_manager) fn photo_collection(&self, path: &std::path::Path) -> Vec<PathBuf> {
        if !little_clip_core::item::is_image_path(path) {
            return Vec::new();
        }
        if self.view == super::super::BrowserView::Photos
            && self.photo_mode == super::super::gallery::similarity::PhotoMode::Similarity
            && self.similarity_canvas.is_current_directory(self.path())
        {
            let collection = self.similarity_canvas.photo_collection();
            if collection.iter().any(|candidate| candidate == path) {
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
    mut preview: Option<&mut super::super::previews::EntryPreviews>,
    mut hover: Option<&mut super::super::gallery::GalleryHover>,
    hovered_path: &mut Option<std::path::PathBuf>,
) {
    if response.hovered() {
        *hovered_path = Some(entry.path.clone());
    }

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
    let (detail, texture_info, metadata_summary) = if let Some(ref mut preview) = preview {
        preview.show_gallery(ui, image_rect, entry);
        if let Some(ready) = preview.listing(entry) {
            let detail = if !ready.data.detail.is_empty() {
                Some(ready.data.detail.clone())
            } else {
                None
            };
            let tex = ready.texture.as_ref().map(|t| (t.id(), t.size_vec2()));
            let meta = ready
                .data
                .photo_metadata
                .clone()
                .or_else(|| ready.data.photo.as_ref().map(|p| p.metadata.clone()));
            (detail, tex, meta)
        } else {
            (None, None, None)
        }
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
        (None, None, None)
    };

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

    if response.hovered() && !super::popup_open(ui.ctx()) {
        // Dimensions arrive with the worker preview; hovering never opens the file.
        let orig_dims = detail
            .as_deref()
            .and_then(|d| {
                let parts: Vec<&str> = d.split('×').map(|s| s.trim()).collect();
                if parts.len() == 2 {
                    let w = parts[0].parse::<f32>().ok()?;
                    let h = parts[1].parse::<f32>().ok()?;
                    Some((w, h))
                } else {
                    None
                }
            })
            .or_else(|| texture_info.map(|(_, size)| (size.x, size.y)));

        let screen = ui.ctx().screen_rect().shrink(16.0);
        let max_image_w = (screen.width() - 32.0).max(100.0);
        let max_image_h = (screen.height() - 120.0).max(100.0);

        let (mut orig_w, mut orig_h) = orig_dims.unwrap_or((320.0, 240.0));
        if let Some((_, tex_size)) = texture_info {
            if (tex_size.x > tex_size.y) != (orig_w > orig_h) {
                std::mem::swap(&mut orig_w, &mut orig_h);
            }
        }

        let scale = (max_image_w / orig_w.max(1.0))
            .min(max_image_h / orig_h.max(1.0))
            .min(1.0);
        let display_size = vec2(orig_w * scale, orig_h * scale);

        let needed_edge = (orig_w.max(orig_h) as u32).clamp(
            crate::file_manager::gallery::hover::MIN_HOVER_EDGE,
            crate::file_manager::gallery::hover::MAX_HOVER_EDGE,
        );
        let needed_edge = needed_edge
            .div_ceil(crate::file_manager::gallery::hover::HOVER_EDGE_STEP)
            * crate::file_manager::gallery::hover::HOVER_EDGE_STEP;

        let active_texture = if let Some(ref mut hover) = hover {
            hover.texture_for(&entry.path, entry.revision, needed_edge, 1, texture_info)
        } else {
            texture_info
        };

        let dims = Some((orig_w, orig_h));
        paint_tile_hover_preview(
            ui,
            response,
            entry,
            dims,
            metadata_summary,
            active_texture,
            display_size,
        );
    }
}

fn paint_tile_hover_preview(
    _ui: &Ui,
    response: &egui::Response,
    entry: &super::super::filesystem::Entry,
    dims: Option<(f32, f32)>,
    meta: Option<little_clip_ops::metadata::ImageMetadataSummary>,
    texture_info: Option<(egui::TextureId, egui::Vec2)>,
    display_size: egui::Vec2,
) {
    response.clone().on_hover_ui(|ui| {
        ui.set_max_width(display_size.x.max(340.0));
        if let Some((tex_id, _size)) = texture_info {
            ui.image((tex_id, display_size));
            ui.add_space(4.0);
        }

        ui.horizontal(|ui| {
            let ext = entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_uppercase())
                .unwrap_or_else(|| "IMG".to_string());
            ui.label(
                egui::RichText::new(format!(" {ext} "))
                    .size(10.0)
                    .strong()
                    .background_color(ui.visuals().extreme_bg_color),
            );
            ui.strong(&entry.name);
        });

        ui.label(
            egui::RichText::new(entry.path.to_string_lossy().as_ref())
                .weak()
                .size(11.0),
        );

        ui.horizontal(|ui| {
            if !entry.size.is_empty() {
                ui.label(egui::RichText::new(&entry.size).strong().size(12.0));
            }
            if let Some((w, h)) = dims {
                let mp = (w * h) / 1_000_000.0;
                ui.label(
                    egui::RichText::new(format!("· {:.0} × {:.0} ({:.1} MP)", w, h, mp))
                        .weak()
                        .size(12.0),
                );
            }
            if !entry.modified.is_empty() {
                ui.label(
                    egui::RichText::new(format!("· {}", entry.modified))
                        .weak()
                        .size(12.0),
                );
            }
        });

        if let Some(ref m) = meta {
            let mut camera_parts = Vec::new();
            if let Some(ref model) = m.model {
                camera_parts.push(model.clone());
            } else if let Some(ref make) = m.make {
                camera_parts.push(make.clone());
            }

            let mut exp_parts = Vec::new();
            if let Some(fl) = m.focal_length {
                exp_parts.push(format!("{:.0}mm", fl));
            }
            if let Some(fn_val) = m.f_number {
                exp_parts.push(format!("ƒ/{:.1}", fn_val));
            }
            if let Some(ref et) = m.exposure_time {
                exp_parts.push(et.clone());
            }
            if let Some(iso) = m.iso {
                exp_parts.push(format!("ISO {}", iso));
            }

            if !camera_parts.is_empty() || !exp_parts.is_empty() {
                ui.horizontal(|ui| {
                    if !camera_parts.is_empty() {
                        ui.add(Icon::Camera.image_tinted(12.0, ui.visuals().text_color()));
                        ui.label(
                            egui::RichText::new(camera_parts.join(" "))
                                .size(11.0)
                                .color(ui.visuals().text_color()),
                        );
                    }
                    if !exp_parts.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("· {}", exp_parts.join(" · ")))
                                .weak()
                                .size(11.0),
                        );
                    }
                });
            }
        }
    });
}
