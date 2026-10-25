//! Grid actions and canvas overlays, separate from tile rendering and file operations.
use super::*;
use crate::file_manager::gallery::similarity::canvas::SimilarityCanvas;

#[derive(Clone, Copy)]
pub(super) enum HeaderAction {
    Solo(usize),
    Toggle(usize),
    SelectAll(usize),
    Allowance(usize, f32),
    Remove(usize),
}

impl FileManager {
    pub(super) fn apply_similarity_header_action(&mut self, action: HeaderAction) {
        match action {
            HeaderAction::Solo(id) => {
                for grid in &mut self.similarity_canvas.grids {
                    grid.selected_for_search = grid.id == id;
                }
            }
            HeaderAction::Toggle(id) => {
                if let Some(grid) = self.similarity_canvas.grids.iter_mut().find(|g| g.id == id) {
                    grid.selected_for_search = !grid.selected_for_search;
                }
            }
            HeaderAction::Remove(id) => self.similarity_canvas.remove_grid(id),
            HeaderAction::Allowance(id, delta) => {
                if self.similarity_canvas.adjust_grid_allowance(id, delta) {
                    self.similarity_canvas.recompute_layout();
                }
            }
            HeaderAction::SelectAll(id) => {
                if let Some(grid) = self.similarity_canvas.grids.iter().find(|g| g.id == id) {
                    let paths: Vec<_> = grid
                        .indices
                        .iter()
                        .filter_map(|&index| {
                            self.similarity_canvas
                                .entries
                                .get(index)
                                .map(|entry| entry.path.clone())
                        })
                        .collect();
                    self.select_all_paths(paths);
                }
            }
        }
    }

    pub(super) fn similarity_search_hud(
        &mut self,
        ui: &mut Ui,
        canvas_rect: Rect,
        colors: WorkbenchColors,
    ) {
        // Floating search HUD pill
        let search_id = Id::new("similarity_canvas_search");
        let search_pos = pos2(
            canvas_rect.left() + HUD_INSET_X,
            canvas_rect.top() + SEARCH_HUD_INSET_Y,
        );
        egui::Area::new(search_id)
            .order(egui::Order::Middle)
            .fixed_pos(search_pos)
            .show(ui.ctx(), |ui| {
                hud_frame(ui, colors).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        crate::icons::Icon::Search.ui(ui, 14.0, colors.text_secondary);
                        let edit_res = ui.add(
                            egui::TextEdit::singleline(&mut self.similarity_canvas.search_query)
                                .hint_text("Search concept (e.g. cat, !dog, indoors)…")
                                .desired_width(200.0),
                        );
                        if self.similarity_canvas.search_query == "+"
                            || self.similarity_canvas.search_query == "="
                            || self.similarity_canvas.search_query == "-"
                        {
                            self.similarity_canvas.search_query.clear();
                        }
                        if (edit_res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
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
    }
    pub(super) fn similarity_navigation_hud(
        &mut self,
        ui: &mut Ui,
        canvas_rect: Rect,
        colors: WorkbenchColors,
    ) {
        // Floating Grid Jump Navigator
        let nav_id = Id::new("similarity_grid_nav");
        let nav_pos = pos2(
            canvas_rect.left() + HUD_INSET_X,
            canvas_rect.bottom() - BOTTOM_HUD_HEIGHT,
        );
        egui::Area::new(nav_id)
            .order(egui::Order::Middle)
            .fixed_pos(nav_pos)
            .show(ui.ctx(), |ui| {
                hud_frame(ui, colors).show(ui, |ui| {
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
    }
    pub(super) fn similarity_status_hud(
        &mut self,
        ui: &mut Ui,
        canvas_rect: Rect,
        colors: WorkbenchColors,
    ) {
        // Floating HUD pill
        let hud_id = Id::new("similarity_canvas_hud");
        let hud_pos = pos2(canvas_rect.right() - 250.0, canvas_rect.bottom() - 44.0);
        egui::Area::new(hud_id)
            .order(egui::Order::Middle)
            .fixed_pos(hud_pos)
            .show(ui.ctx(), |ui| {
                hud_frame(ui, colors).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.small_button("Fit").clicked() {
                            self.similarity_canvas.fit_to_view(canvas_rect);
                        }
                        ui.label(format!("{:.0}%", self.similarity_canvas.zoom * 100.0));
                        ui.separator();
                        if let Some(error) = self.similarity_canvas.error.clone() {
                            ui.label(egui::RichText::new(error).color(colors.text_secondary));
                            if ui.small_button("Retry").clicked() {
                                self.similarity_canvas.invalidate();
                            }
                        }
                        if self.similarity_canvas.is_scanning() {
                            ui.label("Scanning subfolders…");
                        } else if self.similarity_canvas.is_hashing() {
                            ui.label(format!(
                                "Clustering {}/{}…",
                                self.similarity_canvas.hashing_progress.0,
                                self.similarity_canvas.hashing_progress.1
                            ));
                        } else {
                            ui.label(format!("{} photos", self.similarity_canvas.entries.len()));
                        }
                    });
                });
            });
    }
}

pub(super) fn scroll_badge(
    ui: &Ui,
    entry: Option<&crate::file_manager::filesystem::Entry>,
    sort_column: crate::file_manager::sorting::Column,
    last_scroll_time: f64,
    colors: WorkbenchColors,
) {
    let is_alpha = sort_column == crate::file_manager::sorting::Column::Name;
    let badge_content = entry.and_then(|entry| {
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
        let time_since_scroll = ui.input(|i| i.time) - last_scroll_time;
        if time_since_scroll < BADGE_DISPLAY_DURATION {
            let alpha = ((BADGE_DISPLAY_DURATION - time_since_scroll) / BADGE_FADE_DURATION)
                .clamp(0.0, 1.0) as f32;
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
}

pub(super) fn grid_headers(
    ui: &mut Ui,
    canvas: &SimilarityCanvas,
    canvas_rect: Rect,
    colors: WorkbenchColors,
) -> Vec<HeaderAction> {
    let mut actions = Vec::new();
    for (index, grid) in canvas.grids.iter().enumerate() {
        paint_grid_divider(ui, canvas, canvas_rect, grid, index, colors);
        let rect = canvas.grid_header_rect(canvas_rect.min, grid);
        if !rect.intersects(ui.clip_rect()) {
            continue;
        }
        if let Some(action) = grid_header(ui, canvas, grid, rect, colors) {
            actions.push(action);
        }
        if grid.id > 0 && grid.indices.is_empty() {
            let origin = canvas.tile_screen_rect(canvas_rect.min, grid.col_offset, grid.row_offset);
            ui.painter().text(
                origin.min + vec2(12.0, 8.0),
                egui::Align2::LEFT_TOP,
                "No matching photos found in target grid.",
                FontId::proportional(13.0),
                colors.text_secondary,
            );
        }
    }
    actions
}

fn grid_header(
    ui: &mut Ui,
    canvas: &SimilarityCanvas,
    grid: &CanvasGrid,
    rect: Rect,
    colors: WorkbenchColors,
) -> Option<HeaderAction> {
    let title = format!("{} ({})", grid.title, grid.indices.len());
    let mut action = None;
    let clip = rect.intersect(ui.clip_rect());
    let stroke = egui::Stroke::new(
        1.0,
        if grid.selected_for_search {
            colors.accent
        } else {
            colors.border
        },
    );
    let painter = ui.painter().with_clip_rect(clip);
    painter.rect(rect, 4.0, colors.panel, stroke);
    // At overview zoom, keep the label legible without growing the hit area into another grid.
    if rect.height() < 26.0 || rect.width() < 260.0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &title,
            FontId::proportional(12.0),
            colors.text_primary,
        );
        return ui
            .interact(
                rect,
                ui.id().with(("compact_grid", grid.id)),
                Sense::click(),
            )
            .on_hover_text("Click to search this grid; zoom in for controls")
            .clicked()
            .then_some(HeaderAction::Solo(grid.id));
    }
    ui.allocate_ui_at_rect(rect.shrink2(vec2(4.0, 2.0)), |ui| {
        ui.set_clip_rect(clip);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().button_padding = vec2(3.0, 1.0);
        ui.spacing_mut().interact_size.y = 20.0;
        ui.horizontal(|ui| {
            let mut included = grid.selected_for_search;
            if ui
                .checkbox(&mut included, "")
                .on_hover_text("Include this grid in the next search")
                .changed()
            {
                action = Some(HeaderAction::Toggle(grid.id));
            }
            // Reserve controls at the right, so long search terms cannot push them outside the header.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if grid.id > 0 {
                    if header_button(ui, Icon::Close, true, "Remove this search grid", colors) {
                        action = Some(HeaderAction::Remove(grid.id));
                    }
                    if header_button(
                        ui,
                        Icon::Plus,
                        canvas.can_increase_grid_allowance(grid.id),
                        "Increase match allowance (include more photos)",
                        colors,
                    ) {
                        action = Some(HeaderAction::Allowance(grid.id, CanvasGrid::ALLOWANCE_STEP));
                    }
                    ui.label(format!("{:.0}%", grid.allowance * 100.0))
                        .on_hover_text("Match allowance: higher includes more photos");
                    if header_button(
                        ui,
                        Icon::Minus,
                        canvas.can_decrease_grid_allowance(grid.id),
                        "Decrease match allowance (fewer photos)",
                        colors,
                    ) {
                        action = Some(HeaderAction::Allowance(
                            grid.id,
                            -CanvasGrid::ALLOWANCE_STEP,
                        ));
                    }
                }
                if header_button(
                    ui,
                    Icon::Grid,
                    true,
                    "Select all photos in this grid",
                    colors,
                ) {
                    action = Some(HeaderAction::SelectAll(grid.id));
                }
                if ui
                    .add_sized(
                        vec2(ui.available_width(), 20.0),
                        egui::Label::new(egui::RichText::new(&title).strong())
                            .truncate(true)
                            .sense(Sense::click()),
                    )
                    .on_hover_text(format!("Search only this grid: {title}"))
                    .clicked()
                {
                    action = Some(HeaderAction::Solo(grid.id));
                }
            });
        });
    });
    action
}

fn header_button(
    ui: &mut Ui,
    icon: Icon,
    enabled: bool,
    hint: &str,
    colors: WorkbenchColors,
) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::image(icon.image_tinted(12.0, colors.text_secondary))
            .min_size(vec2(20.0, 20.0)),
    )
    .on_hover_text(hint)
    .clicked()
}

fn paint_grid_divider(
    ui: &Ui,
    canvas: &SimilarityCanvas,
    canvas_rect: Rect,
    grid: &CanvasGrid,
    index: usize,
    colors: WorkbenchColors,
) {
    let origin = canvas
        .tile_screen_rect(canvas_rect.min, grid.col_offset, grid.row_offset)
        .min;
    let step = (canvas.tile_size + canvas.spacing) * canvas.zoom;
    let segment = if grid.row_offset > 0 {
        let y = origin.y - step * 0.5;
        [
            pos2(origin.x, y),
            pos2(origin.x + grid.grid_cols.max(1) as f32 * step, y),
        ]
    } else if index > 0 {
        let x = origin.x - step * 0.5;
        [
            pos2(x, canvas_rect.min.y + canvas.pan.y - 48.0),
            pos2(
                x,
                canvas_rect.min.y + canvas.pan.y + canvas.grid_rows.max(1) as f32 * step,
            ),
        ]
    } else {
        return;
    };
    // The painter already clips segments to the canvas; no separate bounds branches are needed.
    ui.painter()
        .line_segment(segment, egui::Stroke::new(1.0, colors.border));
}

fn hud_frame(ui: &Ui, colors: WorkbenchColors) -> egui::Frame {
    egui::Frame::popup(ui.style())
        .fill(colors.panel)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .rounding(6.0)
        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
}
