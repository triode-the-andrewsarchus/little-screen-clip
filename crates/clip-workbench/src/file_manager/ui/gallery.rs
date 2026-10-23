//! Virtualized tiles share selection, drag/drop, menus and file operations with the list.
use super::*;

impl FileManager {
    pub(in crate::file_manager) fn set_view(&mut self, view: super::super::BrowserView) {
        if view == self.view {
            return;
        }
        self.type_navigation.clear();
        self.retire_media_previews();
        if view != self.view
            && matches!(
                view,
                super::super::BrowserView::List | super::super::BrowserView::Videos
            )
        {
            self.stop_previews();
        }
        if view == super::super::BrowserView::Music && self.view != view {
            self.shelf.open = false;
        }
        self.view = view;
        if let Some(location) = self.location_mut() {
            location.manual_view = Some(view);
            location.scroll = 0.0;
        }
        self.restore_scroll = true;
    }
    pub(in crate::file_manager) fn retire_media_previews(&mut self) {
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
                    if ui
                        .selectable_label(
                            self.sort.column == super::super::sorting::Column::Modified
                                && self.sort.descending,
                            "Date (Newest first)",
                        )
                        .clicked()
                    {
                        self.sort = super::super::sorting::Sort {
                            column: super::super::sorting::Column::Modified,
                            descending: true,
                        };
                        self.gallery_layout.invalidate_order();
                        self.rebuild_visible();
                        ui.close_menu();
                    }
                    if ui
                        .selectable_label(
                            self.sort.column == super::super::sorting::Column::Modified
                                && !self.sort.descending,
                            "Date (Oldest first)",
                        )
                        .clicked()
                    {
                        self.sort = super::super::sorting::Sort {
                            column: super::super::sorting::Column::Modified,
                            descending: false,
                        };
                        self.gallery_layout.invalidate_order();
                        self.rebuild_visible();
                        ui.close_menu();
                    }
                    if ui
                        .selectable_label(
                            self.sort.column == super::super::sorting::Column::Name
                                && !self.sort.descending,
                            "Name (A to Z)",
                        )
                        .clicked()
                    {
                        self.sort = super::super::sorting::Sort {
                            column: super::super::sorting::Column::Name,
                            descending: false,
                        };
                        self.gallery_layout.invalidate_order();
                        self.rebuild_visible();
                        ui.close_menu();
                    }
                    if ui
                        .selectable_label(
                            self.sort.column == super::super::sorting::Column::Name
                                && self.sort.descending,
                            "Name (Z to A)",
                        )
                        .clicked()
                    {
                        self.sort = super::super::sorting::Sort {
                            column: super::super::sorting::Column::Name,
                            descending: true,
                        };
                        self.gallery_layout.invalidate_order();
                        self.rebuild_visible();
                        ui.close_menu();
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
                let new_edge = (old_edge + delta * 0.4).clamp(80.0, 280.0);
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
            let is_scroll_settled = (now - self.gallery_last_scroll_time) >= 0.2;

            // 1. Request base thumbnails for visible tiles
            for tile in priority.iter() {
                let entry = &directory.entries[self.visible[tile.index]];
                if tile.ready && entry.is_photo() {
                    let edge = tile.thumbnail_edge(unit, ui.ctx().pixels_per_point());
                    self.gallery_previews
                        .request_gallery(entry, edge, tile.orientation);
                }
            }

            // Check if all visible thumbnails in view have loaded
            let all_visible_loaded = priority.iter().all(|tile| {
                let entry = &directory.entries[self.visible[tile.index]];
                if !tile.ready || !entry.is_photo() {
                    return true;
                }
                self.gallery_previews.is_ready(entry)
            });

            if all_visible_loaded {
                // 2. Pre-cache buffer tiles (prior and next 10 rows) ordered by closeness to viewport
                let mut buffer_tiles: Vec<_> = retain_tiles
                    .iter()
                    .filter(|t| !priority.iter().any(|p| p.index == t.index))
                    .collect();
                buffer_tiles.sort_by(|a, b| {
                    let dist_a = (a.y * unit - viewport.center().y).abs();
                    let dist_b = (b.y * unit - viewport.center().y).abs();
                    dist_a.total_cmp(&dist_b)
                });
                for tile in buffer_tiles {
                    let entry = &directory.entries[self.visible[tile.index]];
                    if tile.ready && entry.is_photo() {
                        let edge = tile.thumbnail_edge(unit, ui.ctx().pixels_per_point());
                        self.gallery_previews
                            .request_gallery(entry, edge, tile.orientation);
                    }
                }

                // 3. Once settled for ~0.2s, fetch higher quality previews of only what's currently visible
                if is_scroll_settled {
                    let mut any_pending_upgrade = false;
                    for tile in priority.iter() {
                        let entry = &directory.entries[self.visible[tile.index]];
                        if tile.ready && entry.is_photo() {
                            let hq_edge = 1024;
                            self.gallery_previews
                                .request_gallery(entry, hq_edge, tile.orientation);
                            if self.gallery_previews.gallery_size(&entry.path) < hq_edge {
                                any_pending_upgrade = true;
                            }
                        }
                    }
                    if any_pending_upgrade {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(16));
                    }
                } else {
                    let elapsed = now - self.gallery_last_scroll_time;
                    if elapsed < 0.2 {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_secs_f64(
                                (0.2 - elapsed).max(0.001),
                            ));
                    }
                }
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16));
            }

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

            let is_alpha = self.sort.column == super::super::sorting::Column::Name;
            let top_tile = tiles.iter().min_by(|a, b| a.y.total_cmp(&b.y));
            let badge_content = top_tile.and_then(|t| {
                let entry = &directory.entries[self.visible[t.index]];
                if is_alpha {
                    entry
                        .name
                        .chars()
                        .find(|c| c.is_alphanumeric())
                        .map(|c| c.to_uppercase().to_string())
                        .or_else(|| {
                            entry
                                .name
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                        })
                } else if let Some(time) = entry.modified_time {
                    Some(
                        chrono::DateTime::<chrono::Local>::from(time)
                            .format("%B %Y")
                            .to_string(),
                    )
                } else if entry.modified.len() >= 7 {
                    Some(entry.modified[0..7].to_string())
                } else {
                    None
                }
            });

            if let Some(text) = badge_content {
                let time_since_scroll = now - self.gallery_last_scroll_time;
                if time_since_scroll < 1.2 {
                    let alpha = ((1.2 - time_since_scroll) / 0.3).clamp(0.0, 1.0) as f32;
                    let clip = ui.clip_rect();
                    let (badge_rect, font_size) = if is_alpha {
                        (
                            Rect::from_min_size(
                                pos2(clip.right() - 56.0, clip.top() + 16.0),
                                vec2(40.0, 40.0),
                            ),
                            20.0,
                        )
                    } else {
                        (
                            Rect::from_min_size(
                                pos2(clip.right() - 170.0, clip.top() + 16.0),
                                vec2(150.0, 30.0),
                            ),
                            13.0,
                        )
                    };
                    let fill = colors.panel.gamma_multiply(0.92 * alpha);
                    let text_color = colors.text_primary.gamma_multiply(alpha);
                    let stroke_color = colors.border.gamma_multiply(alpha);

                    ui.painter()
                        .rect(badge_rect, 6.0, fill, egui::Stroke::new(1.0, stroke_color));
                    ui.painter().text(
                        badge_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(font_size),
                        text_color,
                    );
                    ui.ctx().request_repaint();
                }
            }
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

        // Grid header bars and dividers
        let mut solo_grid: Option<usize> = None;
        let mut toggle_grid: Option<usize> = None;
        let mut remove_grid: Option<usize> = None;
        let mut select_all_grid: Option<usize> = None;
        let mut adjust_grid_allowance: Option<(usize, f32)> = None;

        let total_selected = self
            .similarity_canvas
            .grids
            .iter()
            .filter(|g| g.selected_for_search)
            .count();

        for (grid_idx, grid) in self.similarity_canvas.grids.iter().enumerate() {
            let top_left = self.similarity_canvas.tile_screen_rect(
                canvas_rect.min,
                grid.col_offset,
                grid.row_offset,
            );

            let step = (self.similarity_canvas.tile_size + self.similarity_canvas.spacing)
                * self.similarity_canvas.zoom;

            // Dividers: vertical divider between columns, horizontal divider between stacked grids
            if grid_idx > 0 && grid.row_offset == 0 {
                let div_x = top_left.min.x - step * 0.5;
                let total_h = (self.similarity_canvas.grid_rows.max(1) as f32) * step;
                ui.painter().line_segment(
                    [
                        pos2(
                            div_x,
                            canvas_rect.min.y + self.similarity_canvas.pan.y - 48.0,
                        ),
                        pos2(
                            div_x,
                            canvas_rect.min.y + self.similarity_canvas.pan.y + total_h,
                        ),
                    ],
                    egui::Stroke::new(1.0, colors.border),
                );
            } else if grid.row_offset > 0 {
                let div_y = top_left.min.y - step * 0.5;
                let total_w = (grid.grid_cols.max(1) as f32) * step;
                ui.painter().line_segment(
                    [
                        pos2(top_left.min.x, div_y),
                        pos2(top_left.min.x + total_w, div_y),
                    ],
                    egui::Stroke::new(1.0, colors.border),
                );
            }

            if top_left.min.x > canvas_rect.max.x + 100.0
                || top_left.min.x + (grid.grid_cols as f32) * step < canvas_rect.min.x - 100.0
                || top_left.min.y > canvas_rect.max.y + 100.0
                || top_left.min.y + (grid.grid_rows as f32) * step < canvas_rect.min.y - 100.0
            {
                continue;
            }

            let is_solo = grid.selected_for_search && total_selected == 1;
            let is_multi = grid.selected_for_search && total_selected > 1;

            let bar_rect = Rect::from_min_size(
                pos2(top_left.min.x, top_left.min.y - 44.0),
                vec2(720.0, 28.0),
            );

            ui.allocate_ui_at_rect(bar_rect, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(colors.panel.gamma_multiply(0.95))
                    .stroke(egui::Stroke::new(
                        1.0,
                        if grid.selected_for_search {
                            colors.accent
                        } else {
                            colors.border
                        },
                    ))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(6.0, 3.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let solo_resp = ui
                                .selectable_label(is_solo, "Search this grid")
                                .on_hover_text(
                                    "Select ONLY this grid as the search set for the next query",
                                );
                            if solo_resp.clicked() {
                                solo_grid = Some(grid.id);
                            }

                            let add_resp = ui
                                .selectable_label(is_multi, "+ Add grid to search")
                                .on_hover_text(if grid.selected_for_search {
                                    "This grid is currently in the search set. Click to remove."
                                } else {
                                    "Add this grid to the active search set alongside other grids."
                                });
                            if add_resp.clicked() {
                                toggle_grid = Some(grid.id);
                            }

                            ui.separator();

                            let title_text = if grid.id == 0 {
                                format!("All photos ({})", grid.indices.len())
                            } else {
                                format!("\"{}\" ({})", grid.title, grid.indices.len())
                            };
                            ui.label(egui::RichText::new(title_text).strong());

                            ui.separator();
                            let sel_all_resp = ui.small_button("Select all").on_hover_text(
                                "Select all photos in this grid in the file manager",
                            );
                            if sel_all_resp.clicked() {
                                select_all_grid = Some(grid.id);
                            }

                            if grid.id > 0 {
                                ui.separator();
                                let can_decrease =
                                    self.similarity_canvas.can_decrease_grid_allowance(grid.id);
                                let can_increase =
                                    self.similarity_canvas.can_increase_grid_allowance(grid.id);

                                let minus_resp = ui.add_enabled(
                                    can_decrease,
                                    egui::Button::image(
                                        crate::icons::Icon::Minus.image_tinted(
                                            10.0,
                                            if can_decrease {
                                                colors.text_secondary
                                            } else {
                                                colors.text_secondary.gamma_multiply(0.3)
                                            },
                                        ),
                                    )
                                    .frame(true)
                                    .min_size(vec2(16.0, 16.0)),
                                );
                                if can_decrease {
                                    if minus_resp
                                        .on_hover_text("Decrease match allowance (fewer photos)")
                                        .clicked()
                                    {
                                        adjust_grid_allowance = Some((grid.id, -0.05));
                                    }
                                } else {
                                    minus_resp.on_hover_text(
                                        "Cannot decrease: further reduction would yield no matching photos",
                                    );
                                }

                                let allowance_pct = (grid.allowance * 100.0).round() as i32;
                                ui.label(format!("{allowance_pct}%"))
                                    .on_hover_text(format!(
                                        "Match allowance: {allowance_pct}% (higher includes more photos)"
                                    ));

                                let plus_resp = ui.add_enabled(
                                    can_increase,
                                    egui::Button::image(
                                        crate::icons::Icon::Plus.image_tinted(
                                            10.0,
                                            if can_increase {
                                                colors.text_secondary
                                            } else {
                                                colors.text_secondary.gamma_multiply(0.3)
                                            },
                                        ),
                                    )
                                    .frame(true)
                                    .min_size(vec2(16.0, 16.0)),
                                );
                                if can_increase {
                                    if plus_resp
                                        .on_hover_text(
                                            "Increase match allowance (include more photos)",
                                        )
                                        .clicked()
                                    {
                                        adjust_grid_allowance = Some((grid.id, 0.05));
                                    }
                                } else {
                                    let reason = if !grid.candidate_indices.is_empty()
                                        && grid.indices.len() >= grid.candidate_indices.len()
                                    {
                                        "Match allowance includes all photos from the search"
                                    } else {
                                        "No additional matching photos found at higher allowance"
                                    };
                                    plus_resp.on_hover_text(reason);
                                }

                                ui.separator();
                                let close_resp = ui.add(
                                    egui::Button::image(
                                        crate::icons::Icon::Close
                                            .image_tinted(10.0, colors.text_secondary),
                                    )
                                    .frame(true)
                                    .min_size(vec2(16.0, 16.0)),
                                );
                                if close_resp
                                    .on_hover_text("Remove this search grid")
                                    .clicked()
                                {
                                    remove_grid = Some(grid.id);
                                }
                            }
                        });
                    });
            });

            if grid.indices.is_empty() && grid.id > 0 {
                let msg_pos = pos2(top_left.min.x + 12.0, top_left.min.y + 8.0);
                ui.painter().text(
                    msg_pos,
                    egui::Align2::LEFT_TOP,
                    "No matching photos found in target grid.",
                    egui::FontId::proportional(13.0),
                    colors.text_secondary,
                );
            }
        }

        if let Some(id) = solo_grid {
            for g in &mut self.similarity_canvas.grids {
                g.selected_for_search = g.id == id;
            }
        }

        if let Some(g) =
            toggle_grid.and_then(|id| self.similarity_canvas.grids.iter_mut().find(|g| g.id == id))
        {
            g.selected_for_search = !g.selected_for_search;
        }

        if let Some(id) = remove_grid {
            self.similarity_canvas.remove_grid(id);
        }

        if let Some((grid_id, delta)) = adjust_grid_allowance {
            if self.similarity_canvas.adjust_grid_allowance(grid_id, delta) {
                self.similarity_canvas.recompute_layout();
            }
        }

        if let Some(id) = select_all_grid {
            if let Some(grid) = self.similarity_canvas.grids.iter().find(|g| g.id == id) {
                let paths: Vec<PathBuf> = grid
                    .indices
                    .iter()
                    .filter_map(|&idx| {
                        self.similarity_canvas
                            .entries
                            .get(idx)
                            .map(|e| e.path.clone())
                    })
                    .collect();
                self.select_all_paths(paths);
            }
        }

        // Floating search HUD pill
        let search_id = Id::new("similarity_canvas_search");
        let search_pos = pos2(canvas_rect.left() + 20.0, canvas_rect.top() + 16.0);
        egui::Area::new(search_id)
            .order(egui::Order::Middle)
            .fixed_pos(search_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(colors.panel)
                    .stroke(egui::Stroke::new(1.0, colors.border))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            crate::icons::Icon::Search.ui(ui, 14.0, colors.text_secondary);
                            let edit_res = ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.similarity_canvas.search_query,
                                )
                                .hint_text("Search concept (e.g. cat, !dog, indoors)…")
                                .desired_width(200.0),
                            );
                            if self.similarity_canvas.search_query == "+"
                                || self.similarity_canvas.search_query == "="
                                || self.similarity_canvas.search_query == "-"
                            {
                                self.similarity_canvas.search_query.clear();
                            }
                            if (edit_res.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                || ui.small_button("Search").clicked()
                            {
                                self.similarity_canvas.trigger_search();
                            }
                            let has_searches = self.similarity_canvas.grids.len() > 1;
                            if !self.similarity_canvas.search_query.is_empty() || has_searches {
                                let clear_resp = ui.add(
                                    egui::Button::image(
                                        crate::icons::Icon::Close
                                            .image_tinted(10.0, colors.text_secondary),
                                    )
                                    .frame(true)
                                    .min_size(vec2(16.0, 16.0)),
                                );
                                if clear_resp
                                    .on_hover_text("Clear searches and reset to all photos")
                                    .clicked()
                                {
                                    self.similarity_canvas.clear_all_searches();
                                }
                            }
                            if self.similarity_canvas.is_searching() {
                                ui.spinner();
                                ui.label("Searching…");
                            } else if has_searches {
                                ui.separator();
                                let target_grids = self
                                    .similarity_canvas
                                    .grids
                                    .iter()
                                    .filter(|g| g.selected_for_search)
                                    .count();
                                let target_photos: usize = self
                                    .similarity_canvas
                                    .grids
                                    .iter()
                                    .filter(|g| g.selected_for_search)
                                    .map(|g| g.indices.len())
                                    .sum();
                                ui.label(format!(
                                    "{target_grids} target grid(s) ({target_photos} photos)"
                                ));
                            }
                        });
                    });
            });

        // Floating Grid Jump Navigator
        let nav_id = Id::new("similarity_grid_nav");
        let nav_pos = pos2(canvas_rect.left() + 20.0, canvas_rect.bottom() - 44.0);
        egui::Area::new(nav_id)
            .order(egui::Order::Middle)
            .fixed_pos(nav_pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style())
                    .fill(colors.panel)
                    .stroke(egui::Stroke::new(1.0, colors.border))
                    .rounding(6.0)
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Grids:").size(11.0).weak());
                            let mut jump_to_grid = None;
                            for grid in &self.similarity_canvas.grids {
                                let label = if grid.id == 0 {
                                    format!("All ({})", grid.indices.len())
                                } else {
                                    let truncated = if grid.title.chars().count() > 14 {
                                        let s: String = grid.title.chars().take(12).collect();
                                        format!("{s}… ({})", grid.indices.len())
                                    } else {
                                        format!("{} ({})", grid.title, grid.indices.len())
                                    };
                                    truncated
                                };
                                let is_active = grid.selected_for_search;
                                let btn = ui
                                    .selectable_label(is_active, label)
                                    .on_hover_text(format!("Jump to grid: {}", grid.title));
                                if btn.clicked() {
                                    jump_to_grid = Some(grid.id);
                                }
                            }
                            if let Some(id) = jump_to_grid {
                                self.similarity_canvas.pending_pan_to_grid = Some(id);
                            }
                        });
                    });
            });

        // Floating HUD pill
        let hud_id = Id::new("similarity_canvas_hud");
        let hud_pos = pos2(canvas_rect.right() - 250.0, canvas_rect.bottom() - 44.0);
        egui::Area::new(hud_id)
            .order(egui::Order::Middle)
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
            let meta = ready.data.photo.as_ref().map(|p| p.metadata.clone());
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
        let orig_dims = imagesize::size(&entry.path)
            .ok()
            .map(|dim| (dim.width as f32, dim.height as f32))
            .or_else(|| {
                detail.as_deref().and_then(|d| {
                    let parts: Vec<&str> = d.split('×').map(|s| s.trim()).collect();
                    if parts.len() == 2 {
                        let w = parts[0].parse::<f32>().ok()?;
                        let h = parts[1].parse::<f32>().ok()?;
                        Some((w, h))
                    } else {
                        None
                    }
                })
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

        let needed_edge = (orig_w.max(orig_h) as u32).clamp(128, 4096);
        let needed_edge = needed_edge.div_ceil(128) * 128;

        let active_texture = if let Some(ref mut hover) = hover {
            hover.texture_for(&entry.path, needed_edge, 1, texture_info)
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
                        ui.label(
                            egui::RichText::new(format!("📷 {}", camera_parts.join(" ")))
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
