//! Pitch-height sample map with duration-sized stars and folder-local note families.

use super::ConstellationState;
use super::db::ConstellationSample;
use super::generated_model::SampleCategory;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// Pan-independent placement; only sample coordinates, zoom/scale or spacing invalidate it.
#[derive(Default)]
pub(super) struct OverlapLayout {
    original: Vec<Pos2>,
    placed: Vec<Pos2>,
    spacing: f32,
}

impl OverlapLayout {
    pub(super) fn positions(&mut self, original: Vec<Pos2>, spacing: f32) -> &[Pos2] {
        if self.spacing != spacing || self.original != original {
            self.spacing = spacing;
            self.placed.clone_from(&original);
            self.original = original;
            relax_star_positions(&mut self.placed, spacing);
        }
        &self.placed
    }
}

/// Keep selection and audition highlights in their dot's depth layer.
fn append_point_highlight(
    mesh: &mut egui::Mesh,
    offset: usize,
    bounds: Rect,
    pos: Pos2,
    color: Color32,
    radius: f32,
    selected: bool,
) {
    let r = radius + if selected { 2.0 } else { 1.0 };
    let halo = if selected { 8.0 } else { 7.0 };
    let opacity = circle_edge_opacity(bounds, pos, r + halo);
    let start = mesh.indices.len();
    for (radius, color) in [
        (
            r + halo,
            Color32::from_rgba_unmultiplied(
                color.r(),
                color.g(),
                color.b(),
                if selected { 80 } else { 60 },
            ),
        ),
        (r, color),
        (r * 0.45, Color32::WHITE),
    ] {
        mesh.add_rect_with_uv(
            Rect::from_center_size(pos, Vec2::splat(radius * 2.0)),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 1.0)),
            color.gamma_multiply(opacity),
        );
    }
    let indices: Vec<_> = mesh.indices.drain(start..).collect();
    mesh.indices.splice(offset..offset, indices);
}

/// Fades a complete circle uniformly before its extent reaches a visible canvas edge.
fn circle_edge_opacity(bounds: Rect, position: Pos2, radius: f32) -> f32 {
    let clearance = (position.x - bounds.left())
        .min(bounds.right() - position.x)
        .min(position.y - bounds.top())
        .min(bounds.bottom() - position.y)
        - radius;
    let t = (clearance / 6.0).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Keeps filtered samples as faint spatial context without competing with matches.
fn filtered_star_color(base: Color32, dimmed: bool) -> Color32 {
    // The Glow renderer blends in gamma space; scale RGB with alpha so overlapping
    // context splats cannot add up to a bright halo, especially on light canvases.
    base.gamma_multiply(if dimmed { 3.0 / 255.0 } else { 1.0 })
}

/// Marks active filters with an outline while retaining normal text contrast and corners.
pub(super) fn outlined_toggle(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    active: bool,
    accent: Color32,
) -> egui::Response {
    let border = if active {
        Stroke::new(1.5_f32, accent)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    ui.add(
        egui::Button::new(label)
            .fill(ui.visuals().widgets.inactive.weak_bg_fill)
            .rounding(crate::ui_ext::CONTROL_CORNER_RADIUS)
            .stroke(border),
    )
}

/// The same edge target collapses and expands the rack without changing its width.
fn render_rack_rail(ui: &mut egui::Ui, rect: Rect, expanded: bool) -> bool {
    let response = ui.allocate_rect(rect, Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon = if expanded {
        crate::icons::Icon::Minus
    } else {
        crate::icons::Icon::Plus
    };
    icon.image_tinted(12.0, ui.visuals().text_color())
        .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(12.0)));
    response
        .on_hover_text(if expanded {
            "Collapse rack and editor"
        } else {
            "Show rack and editor"
        })
        .clicked()
}

impl ConstellationState {
    pub(super) fn matches_filters(&self, index: usize, lowercase_query: &str) -> bool {
        let sample = &self.samples[index];
        if let Some(set) = self.expanded_set {
            // Explore is a complete family snapshot, independent of overview filters.
            return self.sample_sets.set_for(index) == Some(set);
        }
        (!self.multisamples_only || self.sample_sets.set_for(index).is_some())
            && self.matches_ranges(index)
            && self
                .category_filter
                .is_none_or(|category| sample.category == category)
            && (lowercase_query.is_empty()
                || sample.filename.to_lowercase().contains(lowercase_query))
            && match self.channel_filter {
                1 => sample.channels == 1,
                2 => sample.channels == 2,
                3 => sample.channels > 2,
                _ => true,
            }
    }

    pub(super) fn matches_ranges(&self, index: usize) -> bool {
        let sample = &self.samples[index];
        sample.duration_seconds >= self.min_duration
            && (self.max_duration == 0.0 || sample.duration_seconds <= self.max_duration)
            && self.pitch_range.is_none_or(|(low, high)| {
                self.sample_sets
                    .note(index)
                    .is_some_and(|note| note >= low - 0.5 && note <= high + 0.5)
            })
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        current_folder: &std::path::Path,
        palette: &little_clip_core::palette::AppPalette,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
        on_select_sample: &mut dyn FnMut(&std::path::Path),
    ) {
        self.pad_drop_targets.clear();
        if self.browser_open
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_pack_browser();
        }
        if ui.is_enabled() {
            self.handle_explore_escape(ui.ctx());
        }
        self.handle_shelf_keyboard_shortcuts(ui.ctx());
        let text_focused = ui
            .ctx()
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
        if !text_focused
            && !self.focus_search
            && !self.browser_open
            && !self.pad_keys.contains(&'n')
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::N))
        {
            self.playback.lock().unwrap().show_midi_notes = false;
            self.play_nearest_neighbor(on_play_sample);
        }
        #[cfg(not(test))]
        if self.midi.is_none() {
            match super::midi::Controller::new(ui.ctx().clone(), self.playback.clone()) {
                Ok(controller) => self.midi = Some(controller),
                Err(error) => self.browser_status = Some((error, std::time::Instant::now())),
            }
        }
        let notes = self
            .midi
            .as_ref()
            .map(|controller| controller.poll(!self.browser_open))
            .unwrap_or_default();
        for (pad, _) in notes {
            self.slot_flash[pad] = Some(std::time::Instant::now());
        }
        egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(8.0, 0.0))
            .show(ui, |ui| {
                self.render_toolbar(ui, current_folder, on_play_sample);
                self.render_indexing_progress(ui);
            });

        // Paint the divider at the canvas boundary instead of reserving a
        // separator row whose lower padding leaves an extra empty top border.
        let canvas_bounds = ui.available_rect_before_wrap();
        ui.painter().hline(
            (canvas_bounds.left() + 8.0)..=(canvas_bounds.right() - 8.0),
            canvas_bounds.top(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        self.map_viewport = Some(canvas_bounds.shrink(8.0));
        if self.pack_shelf_open {
            egui::SidePanel::right("constellation_pack_shelf_panel")
                .resizable(true)
                .default_width(360.0)
                .width_range(280.0..=420.0)
                .frame(egui::Frame::none().inner_margin(egui::Margin {
                    left: 8.0,
                    right: 0.0,
                    top: 0.0,
                    bottom: 0.0,
                }))
                .show_inside(ui, |ui| {
                    // Keep content requests from changing the user's divider width.
                    // egui 0.27 otherwise stores overflowing child bounds as panel size.
                    let bounds = ui.available_rect_before_wrap();
                    ui.allocate_rect(bounds, Sense::hover());
                    let rail = Rect::from_min_max(
                        Pos2::new(bounds.right() - 20.0, bounds.top()),
                        bounds.max,
                    );
                    if render_rack_rail(ui, rail, true) {
                        self.pack_shelf_open = false;
                    }
                    let content_bounds = Rect::from_min_max(
                        bounds.min + Vec2::new(0.0, 8.0),
                        bounds.max - Vec2::new(28.0, 8.0),
                    );
                    let mut content =
                        ui.child_ui(content_bounds, egui::Layout::top_down(egui::Align::Min));
                    let ui = &mut content;

                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    use crate::ui_tabs::{Tab, TabBar};
                    let selected_tab = TabBar::new(
                        "constellation_rack_tabs",
                        if self.browser_open {
                            1
                        } else if self.output_editor_open {
                            2
                        } else {
                            0
                        },
                        &[
                            Tab::new(0, "Rack"),
                            Tab::new(1, "Packs"),
                            Tab::new(2, "Output"),
                        ],
                        palette,
                    )
                    .equal_widths()
                    .show(ui);
                    if let Some(tab) = selected_tab {
                        if self.browser_open || tab == 1 {
                            self.stop_playback();
                        }
                        self.output_editor_open = tab == 2;
                        if tab == 1 {
                            self.browser_open = true;
                            self.refresh_browser_packs(current_folder);
                        } else {
                            self.close_pack_browser();
                        }
                    }
                    ui.add_space(8.0);
                    if self.output_editor_open {
                        ui.style_mut().spacing.scroll.floating = false;
                        egui::ScrollArea::vertical()
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                            )
                            .max_height(ui.available_height())
                            .min_scrolled_height(0.0)
                            .show(ui, |ui| self.render_compressor(ui));
                    } else if self.browser_open {
                        self.render_pack_browser(ui, current_folder);
                    } else {
                        self.render_pack_shelf(ui, current_folder);
                    }
                });
        }
        if !self.pack_shelf_open {
            egui::SidePanel::right("constellation_rack_reopen")
                .resizable(false)
                .exact_width(20.0)
                .frame(egui::Frame::none())
                .show_inside(ui, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    if render_rack_rail(ui, rect, false) {
                        self.pack_shelf_open = true;
                    }
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show_inside(ui, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                if self.samples.is_empty() && self.active_job.is_none() {
                    self.render_empty_state(ui, current_folder);
                    return;
                }
                if self.pack_shelf_open
                    && !self.browser_open
                    && !self.output_editor_open
                    && self.pad_editor_open
                    && !self.pack_bank.slots[self.selected_slot].is_empty()
                {
                    egui::TopBottomPanel::bottom("constellation_selected_pad")
                        .resizable(true)
                        .default_height(290.0)
                        .height_range(220.0..=420.0)
                        .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(8.0, 6.0)))
                        .show_inside(ui, |ui| {
                            ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                            ui.separator();
                            // Overflow gets a reserved gutter, never an overlay on trim handles.
                            ui.style_mut().spacing.scroll.floating = false;
                            egui::ScrollArea::vertical()
                                .id_source("constellation_layer_editor")
                                .min_scrolled_height(0.0)
                                .show(ui, |ui| self.render_selected_pad(ui, palette));
                        });
                }
                self.render_starfield(ui, current_folder, on_play_sample, on_select_sample);
            });
        let text_focused = ui
            .ctx()
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
        self.checkpoint_bank(ui.input(|input| input.pointer.any_down()) || text_focused);
        self.playback.lock().unwrap().sync(&self.pack_bank);
    }

    /// Distinguishes a sample-reference drag from empty-canvas panning.
    fn handle_map_drag(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        center: Pos2,
        query_lower: &str,
        points: &[super::space::MapPoint],
    ) -> Option<std::path::PathBuf> {
        if !self.orbit_mode && response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(origin) = ui.input(|input| input.pointer.press_origin()) {
                let nearest = points
                    .iter()
                    .filter_map(|point| {
                        let sample = &self.samples[point.index];
                        let pos = point.position + center.to_vec2();
                        let distance = pos.distance_sq(origin);
                        (self.matches_filters(point.index, query_lower)
                            && distance <= (point.radius + 7.0).max(12.0).powi(2))
                        .then_some((sample, distance))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((sample, _)) = nearest {
                    response.dnd_set_drag_payload(crate::file_manager::transfers::FileDrag {
                        paths: vec![sample.file_path.clone()],
                        copy_only: true,
                        context: Default::default(),
                    });
                    return Some(sample.file_path.clone());
                }
            }
        }
        None
    }

    /// Owns camera gestures and prevents camera movement from triggering auditions.
    fn handle_camera_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        viewport: Rect,
    ) -> Option<Pos2> {
        // Zoom handling
        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.1 && response.hovered() {
            let pointer_pos = ui
                .input(|i| i.pointer.hover_pos())
                .unwrap_or(response.rect.center());
            let factor = if scroll_delta > 0.0 { 1.15 } else { 1.0 / 1.15 };
            let old_zoom = self.zoom;
            let max_zoom = if self.selected_set().is_some() {
                super::space::SET_MAX_ZOOM
            } else {
                50.0
            };
            self.zoom = (self.zoom * factor).clamp(0.2, max_zoom);

            // Zoom toward pointer
            let mouse_rel = pointer_pos - viewport.center() - self.pan;
            self.pan -= mouse_rel * (self.zoom / old_zoom - 1.0);
        }

        // Panning never auditions a point on press/release. Resume hover only
        // after a fresh pointer movement, not because the map moved underneath it.
        use egui::PointerButton::{Middle, Primary, Secondary};
        let camera_buttons: &[egui::PointerButton] = if self.orbit_mode {
            &[Middle, Secondary, Primary]
        } else {
            &[Middle, Secondary]
        };
        let (camera_down, camera_released, moved) = ui.input(|input| {
            (
                camera_buttons
                    .iter()
                    .any(|button| input.pointer.button_down(*button)),
                camera_buttons
                    .iter()
                    .any(|button| input.pointer.button_released(*button)),
                input.pointer.delta() != Vec2::ZERO,
            )
        });
        if camera_down {
            self.map_hover_suspended = true;
        } else if moved && !camera_released {
            self.map_hover_suspended = false;
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || (self.orbit_mode && response.dragged_by(egui::PointerButton::Primary))
        {
            self.camera.orbit(response.drag_delta());
        } else if response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary)
                && !self.orbit_mode
                && !egui::DragAndDrop::has_payload_of_type::<
                    crate::file_manager::transfers::FileDrag,
                >(ui.ctx())
                && !response.drag_started_by(egui::PointerButton::Primary))
        {
            self.pan += response.drag_delta();
        }

        response
            .hover_pos()
            .filter(|_| !response.dragged() && !self.map_hover_suspended)
    }

    /// Pick the nearest visible dot, with frontmost surfaces winning in 3D.
    fn pick_map_point<'a>(
        &self,
        points: &'a [super::space::MapPoint],
        pointer: Pos2,
        center: Pos2,
        bounds: Rect,
        query: &str,
        mut cutoff: f32,
    ) -> Option<&'a super::space::MapPoint> {
        let mut picked = None;
        let mut over_surface = false;
        for point in points {
            let position = point.position + center.to_vec2();
            if !self.matches_filters(point.index, query)
                || circle_edge_opacity(bounds, position, point.radius) <= 0.0
            {
                continue;
            }
            let distance = position.distance_sq(pointer);
            let inside = self.pitch_height_enabled && distance <= point.radius * point.radius;
            if inside || (!over_surface && distance <= cutoff) {
                picked = Some(point);
                cutoff = distance;
                over_surface = inside;
            }
        }
        picked
    }

    fn render_starfield(
        &mut self,
        ui: &mut egui::Ui,
        current_folder: &std::path::Path,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
        on_select_sample: &mut dyn FnMut(&std::path::Path),
    ) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let viewport = self.map_viewport.unwrap_or(rect);

        // The map inherits the file browser canvas; only samples carry category color.
        // 2. High-Performance GPU Batched Star Rendering
        // Organic category coordinates can extend beyond ±1. Fit their actual
        // extent, leaving a margin, while preserving every relative position.
        let query_lower = self.search_query.to_lowercase();

        let mouse_pos = self.handle_camera_pointer(ui, &response, viewport);

        // Allocate a single batched GPU mesh
        let dot = dot_texture(ui.ctx());
        let mut star_mesh = egui::Mesh::with_texture(dot.id());
        let mut context_mesh = egui::Mesh::with_texture(dot.id());
        let stem = super::space::stem_texture(ui.ctx());
        let mut stem_mesh = egui::Mesh::with_texture(stem.id());
        let base_star_radius = map_point_radius(viewport, self.samples.len(), self.zoom);
        let (view_scale, center, points) =
            self.layout_map_points(viewport, rect, base_star_radius, &query_lower);
        // Only the bounded audition trail needs a position lookup. Dense integer
        // slots avoid hashing every sample's full path on every animation frame.
        let mut trail_positions = if self.history_trail.len() >= 2 {
            vec![None; self.samples.len() + self.sample_sets.sets.len()]
        } else {
            Vec::new()
        };
        let collected: std::collections::HashSet<_> = self
            .pack_bank
            .slots
            .iter()
            .flat_map(|slot| slot.samples.iter().map(|sample| sample.file_path.as_path()))
            .collect();
        self.paint_reference_plane(&painter, center, view_scale);
        self.paint_pitch_rings(&painter, center, view_scale);
        painter.text(
            rect.left_top() + Vec2::splat(12.0),
            egui::Align2::LEFT_TOP,
            if self.expanded_set.is_some() {
                "Pitch rings: inner to outer, low to high · cross rings to audition · Esc: Back to Map"
            } else if self.pitch_height_enabled {
                "Height: pitch · C4 reference (unknown stays on plane) / Size: duration · Right-drag: orbit"
            } else {
                "Flat map   /   Size: duration"
            },
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
        let edge_bounds = rect.intersect(ui.clip_rect());

        // Choose once from the same projected geometry that will be painted.
        let cutoff = if self.hover_to_play {
            (base_star_radius * 2.8 + 8.0).clamp(14.0, 26.0).powi(2)
        } else {
            f32::MAX
        };
        let mut hovered_point = mouse_pos.and_then(|pointer| {
            self.pick_map_point(&points, pointer, center, edge_bounds, &query_lower, cutoff)
        });
        let over_dot = hovered_point.is_some_and(|point| {
            mouse_pos.is_some_and(|pointer| {
                pointer.distance_sq(point.position + center.to_vec2())
                    <= (point.radius + 4.0).powi(2)
            })
        });
        if !over_dot
            && let Some(index) =
                mouse_pos.and_then(|pointer| self.ring_sample_at(pointer, center, view_scale))
        {
            hovered_point = points
                .iter()
                .find(|point| point.index == index && self.matches_filters(index, &query_lower));
        }
        let hovered_sample = hovered_point.map(|point| self.samples[point.index].clone());
        let hovered_pos =
            hovered_point.map_or(Pos2::ZERO, |point| point.position + center.to_vec2());
        let hovered_radius = hovered_point.map_or(0.0, |point| point.radius);
        let hovered_set = hovered_point.and_then(|point| point.collapsed_set);
        let mut highlights = Vec::with_capacity(2);
        let mut hovered_star_offset = None;

        // One rendering and hit-testing path at every zoom level.
        let rect_min_x = rect.left() - 12.0;
        let rect_max_x = rect.right() + 12.0;
        let rect_min_y = rect.top() - 12.0;
        let rect_max_y = rect.bottom() + 12.0;

        let selected_set = self.selected_set();
        for point in &points {
            let sample = &self.samples[point.index];
            let pos = point.position + center.to_vec2();
            let base = point.base + center.to_vec2();
            let (sx, sy) = (pos.x, pos.y);
            let radius = point.radius;
            let matches = self.matches_filters(point.index, &query_lower);
            let base_color = sample.category.color();
            if matches && !trail_positions.is_empty() {
                let key = point
                    .collapsed_set
                    .map_or(point.index, |set| self.samples.len() + set);
                trail_positions[key] = Some(pos);
            }
            // Draw stems only for visible/matching stars, keeping filtered context quiet.
            if matches && rect.contains(pos) {
                super::space::append_stem(
                    &mut stem_mesh,
                    rect,
                    base,
                    pos,
                    base_color.gamma_multiply(0.28),
                );
            }
            // Frustum cull
            if sx < rect_min_x || sx > rect_max_x || sy < rect_min_y || sy > rect_max_y {
                continue;
            }

            // Exclusions are a low-opacity density field, with no hard disk edge.
            let radius = if matches {
                radius
            } else {
                (radius * 1.6 + 1.5).clamp(2.5, 8.0)
            };
            let opacity = circle_edge_opacity(edge_bounds, pos, radius);
            let is_selected = self.selected_path.as_ref() == Some(&sample.file_path)
                || point
                    .collapsed_set
                    .is_some_and(|set| Some(set) == selected_set);

            if is_selected && matches {
                highlights.push((star_mesh.indices.len() + 6, pos, base_color, radius, true));
            }

            if hovered_point.is_some_and(|hovered| hovered.index == point.index) {
                hovered_star_offset = Some(star_mesh.indices.len() + 6);
            }
            let color = filtered_star_color(base_color, !matches).gamma_multiply(opacity);
            let (mesh, uv) = if matches {
                self.paint_point_decoration(&painter, point, pos, color, &collected);
                (
                    &mut star_mesh,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(0.5, 1.0)),
                )
            } else {
                (
                    &mut context_mesh,
                    Rect::from_min_max(Pos2::new(0.5, 0.0), Pos2::new(1.0, 1.0)),
                )
            };
            mesh.add_rect_with_uv(
                Rect::from_center_size(pos, Vec2::splat(radius * 2.0)),
                uv,
                color,
            );
        }

        let dragged_path = self.handle_map_drag(ui, &response, center, &query_lower, &points);

        if let Some(path) = dragged_path {
            self.selected_path = Some(path);
        }

        if !context_mesh.is_empty() {
            painter.add(egui::Shape::mesh(context_mesh));
        }

        if !stem_mesh.is_empty() {
            painter.add(egui::Shape::mesh(stem_mesh));
        }

        self.render_playback_trail(&painter, &trail_positions);

        if let Some(ref hovered) = hovered_sample
            && let Some(offset) = hovered_star_offset
        {
            highlights.push((
                offset,
                hovered_pos,
                hovered.category.color(),
                hovered_radius,
                false,
            ));
        }
        // Insert highlights directly after their dot's triangles, so nearer dots
        // occlude them too. Vertex storage can stay appended; only indices move.
        highlights.sort_by_key(|highlight| highlight.0);
        for (offset, pos, color, radius, selected) in highlights.into_iter().rev() {
            append_point_highlight(
                &mut star_mesh,
                offset,
                edge_bounds,
                pos,
                color,
                radius,
                selected,
            );
        }
        if !star_mesh.is_empty() {
            painter.add(egui::Shape::mesh(star_mesh));
        }

        // 3. Hover and Click Interactions
        if let Some(hovered) = hovered_sample {
            // Click to select & play
            if response.clicked() {
                self.visited_samples.clear();
                self.selected_path = Some(hovered.file_path.clone());
                self.explore_selection = self.expanded_set.map(|_| hovered.file_path.clone());
                self.record_played_sample(&hovered);
                on_select_sample(&hovered.file_path);
                on_play_sample(&hovered.file_path);
            }

            // Hover-to-play audition
            if self.hover_to_play && self.last_hovered_path.as_ref() != Some(&hovered.file_path) {
                self.last_hovered_path = Some(hovered.file_path.clone());
                self.record_played_sample(&hovered);
                on_play_sample(&hovered.file_path);
            }

            // Draw floating HUD Card for hovered sample
            if let Some(set) = hovered_set {
                let set = &self.sample_sets.sets[set];
                painter.text(
                    Pos2::new(rect.left() + 12.0, rect.bottom() - 12.0),
                    egui::Align2::LEFT_BOTTOM,
                    format!(
                        "{} · {} recordings · middle note audition · click, then Explore set",
                        set.label,
                        set.members.len()
                    ),
                    egui::FontId::proportional(12.0),
                    ui.visuals().text_color(),
                );
            }
            self.render_sample_hud(ui, &painter, rect, hovered_pos, &hovered, current_folder);
        } else {
            self.last_hovered_path = None;
        }
    }

    fn paint_point_decoration(
        &self,
        painter: &egui::Painter,
        point: &super::space::MapPoint,
        pos: Pos2,
        color: Color32,
        collected: &std::collections::HashSet<&std::path::Path>,
    ) {
        let radius = point.radius;
        let base_color = self.samples[point.index].category.color();
        let opacity = circle_edge_opacity(painter.clip_rect(), pos, radius);
        let is_collected = collected.contains(self.samples[point.index].file_path.as_path())
            || point.collapsed_set.is_some_and(|set| {
                self.sample_sets.sets[set]
                    .members
                    .iter()
                    .any(|index| collected.contains(self.samples[*index].file_path.as_path()))
            });
        if is_collected {
            for (extra, alpha) in [(9.0, 0.08), (5.0, 0.16), (2.5, 0.35)] {
                painter.circle_filled(
                    pos,
                    radius + extra,
                    base_color.gamma_multiply(alpha * opacity),
                );
            }
        }
        if point.collapsed_set.is_some() {
            painter.circle_stroke(
                pos,
                radius + 3.0,
                Stroke::new(1.0_f32, color.gamma_multiply(0.7)),
            );
            painter.circle_stroke(
                pos,
                radius + 6.0,
                Stroke::new(0.8_f32, color.gamma_multiply(0.4)),
            );
        } else if self.sample_sets.set_for(point.index).is_some() {
            if let Some(note) = self.sample_sets.note(point.index) {
                painter.text(
                    pos + Vec2::new(radius + 12.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    midi_note_str(note),
                    egui::FontId::monospace(13.0),
                    color,
                );
            }
        }
    }
}

/// One radius for the whole library, independent of local density and dock visibility.
/// Budget disk area at fit zoom, then grow naturally with the camera zoom.
fn map_point_radius(viewport: Rect, count: usize, zoom: f32) -> f32 {
    let fitted_side = viewport.width().min(viewport.height()) * 0.9;
    let fit_radius = (fitted_side * fitted_side * 0.12
        / (std::f32::consts::PI * count.max(1) as f32))
        .sqrt()
        .min(4.0);
    (fit_radius * zoom).clamp(0.35, 8.0)
}

/// Leave the original projection untouched until the final four wheel steps.
pub(super) fn spacing_strength(zoom: f32) -> f32 {
    let start = 50.0 / 1.15_f32.powi(4);
    ((zoom - start) / (50.0 - start)).clamp(0.0, 1.0)
}

/// Place colliding points in nearby unoccupied cells with at most 48 probes each.
/// Earlier points never move again: there is no iterative force propagation that
/// can hollow out clusters or make dense/coincident inputs quadratic. Saturated
/// neighborhoods retain overlaps rather than moving samples far from their origin.
fn relax_star_positions(points: &mut [Pos2], spacing: f32) {
    if spacing <= 0.0 {
        return;
    }
    let mut occupied = std::collections::HashSet::with_capacity(points.len());
    for point in points {
        let cell = (
            (point.x / spacing).floor() as i32,
            (point.y / spacing).floor() as i32,
        );
        if occupied.insert(cell) {
            continue;
        }
        'search: for ring in 1_i32..=3 {
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs().max(dy.abs()) != ring {
                        continue;
                    }
                    let offset = Vec2::new(dx as f32, dy as f32) * spacing;
                    if offset.length_sq() <= 64.0 * 64.0
                        && occupied.insert((cell.0 + dx, cell.1 + dy))
                    {
                        *point += offset;
                        break 'search;
                    }
                }
            }
        }
    }
}

/// Atlas of a sharp disk (left) and a Gaussian density splat (right). Both use four vertices.
fn dot_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("constellation_dot_texture");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let mut image = egui::ColorImage::new([64, 32], Color32::TRANSPARENT);
    for y in 0..32 {
        for x in 0..32 {
            let distance = Vec2::new(x as f32 - 15.5, y as f32 - 15.5).length();
            let alpha = ((15.5 - distance).clamp(0.0, 1.0) * 255.0) as u8;
            image[(x, y)] = Color32::from_white_alpha(alpha);
            let normalized = distance / 15.5;
            let edge = (-4.5_f32).exp();
            let gaussian = ((-4.5 * normalized * normalized).exp() - edge) / (1.0 - edge);
            let soft_alpha = (gaussian.clamp(0.0, 1.0) * 255.0) as u8;
            image[(x + 32, y)] =
                Color32::from_rgba_premultiplied(soft_alpha, soft_alpha, soft_alpha, soft_alpha);
        }
    }
    let texture = ctx.load_texture("constellation dot", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

#[cfg(test)]
#[path = "frame_benchmark.rs"]
mod frame_benchmark;

mod controls;
pub(super) use controls::*;

#[cfg(test)]
mod tests;
