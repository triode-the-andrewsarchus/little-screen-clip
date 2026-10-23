//! Constellation 2D sample map visualizer and interactive space.
//! Inspired by 2D audio sample mapping tools.

use super::ConstellationState;
use super::db::ConstellationSample;
use super::generated_model::SampleCategory;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};

impl ConstellationState {
    fn matches_filters(&self, sample: &ConstellationSample, lowercase_query: &str) -> bool {
        self.category_filter
            .is_none_or(|category| sample.category == category)
            && sample.filename.to_lowercase().contains(lowercase_query)
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        current_folder: &std::path::Path,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
        on_select_sample: &mut dyn FnMut(&std::path::Path),
    ) {
        if self.browser_open
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.close_pack_browser();
        }
        // Handle keyboard shortcuts (keys 1-0 for slot trigger, Shift+1-0 for assigning)
        self.handle_shelf_keyboard_shortcuts(ui.ctx());

        // Top Toolbar
        self.render_toolbar(ui, current_folder, on_play_sample);

        // Render Sample Pack Browser modal if open
        self.render_pack_browser(ui.ctx(), current_folder);

        // If indexing is currently active, show progress overlay
        if let Some(progress) = self.indexing_status.clone() {
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.spinner();
                if let Some(folder) = self.indexing_folder() {
                    let folder_name = folder
                        .file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default();
                    ui.label(format!("[{}] {}", folder_name, progress));
                } else {
                    ui.label(progress);
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if cancel {
                self.cancel_indexing();
            }
            ui.separator();
        }

        // Layout: Central starfield canvas + Right creation shelf
        if self.pack_shelf_open {
            egui::SidePanel::right("constellation_pack_shelf_panel")
                .resizable(true)
                .default_width(320.0)
                .width_range(280.0..=480.0)
                .show_inside(ui, |ui| {
                    self.render_pack_shelf(ui, current_folder);
                });
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.samples.is_empty() && self.active_job.is_none() {
                self.render_empty_state(ui, current_folder);
                return;
            }
            self.render_starfield(ui, current_folder, on_play_sample, on_select_sample);
        });
    }

    fn render_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        current_folder: &std::path::Path,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
    ) {
        ui.horizontal_wrapped(|ui| {
            // Category filter pills
            let all_selected = self.category_filter.is_none();
            if ui
                .selectable_label(all_selected, format!("All ({})", self.samples.len()))
                .clicked()
            {
                self.category_filter = None;
            }

            // Count per category
            let mut counts = [0usize; 12];
            for s in &self.samples {
                counts[s.category as usize] += 1;
            }

            for cat in SampleCategory::ALL {
                let cnt = counts[cat as usize];
                if cnt == 0 && self.category_filter != Some(cat) {
                    continue;
                }
                let is_active = self.category_filter == Some(cat);
                let label = format!("{} ({})", cat.display_name(), cnt);
                let btn = egui::Button::new(egui::RichText::new(label).color(if is_active {
                    Color32::WHITE
                } else {
                    cat.color()
                }))
                .fill(if is_active {
                    cat.color()
                } else {
                    Color32::from_black_alpha(80)
                });

                if ui.add(btn).clicked() {
                    if self.category_filter == Some(cat) {
                        self.category_filter = None;
                    } else {
                        self.category_filter = Some(cat);
                    }
                }
            }

            ui.separator();

            let search_id = ui.make_persistent_id("constellation-search");
            let search_response = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .id(search_id)
                    .hint_text("Filter by name (Ctrl+F)...")
                    .desired_width(130.0),
            );
            if self.focus_search {
                search_response.request_focus();
                self.focus_search = false;
            }
            // egui clears keyboard focus at frame start for Escape, so include
            // the response's previous focus when handling search dismissal.
            if (search_response.has_focus() || search_response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Escape))
            {
                self.search_query.clear();
                search_response.surrender_focus();
                ui.ctx().request_repaint();
            }

            // Hover to play toggle
            ui.checkbox(&mut self.hover_to_play, "Hover to audition");

            // Creative features
            if !self.samples.is_empty() {
                if ui
                    .button("Random")
                    .on_hover_text("Play random sample")
                    .clicked()
                {
                    let nanos = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as usize)
                        .unwrap_or(0);
                    let query = self.search_query.to_lowercase();
                    let eligible: Vec<_> = self
                        .samples
                        .iter()
                        .filter(|sample| self.matches_filters(sample, &query))
                        .collect();
                    let Some(s) = eligible
                        .get(nanos % eligible.len().max(1))
                        .map(|sample| (*sample).clone())
                    else {
                        return;
                    };
                    self.selected_path = Some(s.file_path.clone());
                    self.record_played_sample(&s);
                    on_play_sample(&s.file_path);
                }

                if ui
                    .button("Nearest Neighbor")
                    .on_hover_text("Find acoustically closest sample")
                    .clicked()
                {
                    self.play_nearest_neighbor(on_play_sample);
                }
            }

            ui.separator();

            // Sample Pack Creation Shelf Toggle & Browser
            let shelf_active = self.pack_shelf_open;
            let shelf_label = if shelf_active {
                "Creation Shelf (On)"
            } else {
                "Creation Shelf"
            };
            let shelf_btn =
                egui::Button::new(egui::RichText::new(shelf_label).color(if shelf_active {
                    Color32::from_rgb(120, 220, 255)
                } else {
                    Color32::WHITE
                }))
                .fill(if shelf_active {
                    Color32::from_rgba_premultiplied(35, 65, 110, 220)
                } else {
                    Color32::from_black_alpha(80)
                });

            if ui
                .add(shelf_btn)
                .on_hover_text("Toggle sample pack creation shelf (10 slots for keys 1-0)")
                .clicked()
            {
                self.pack_shelf_open = !self.pack_shelf_open;
            }

            if ui
                .button("Browse Packs")
                .on_hover_text("Open Sample Pack Browser")
                .clicked()
            {
                self.refresh_browser_packs(current_folder);
                self.browser_open = true;
            }

            // Quick Assign to Slot if a sample is selected on map
            if let Some(ref path) = self.selected_path.clone() {
                if let Some(sample) = self.samples.iter().find(|s| &s.file_path == path).cloned() {
                    ui.separator();
                    ui.menu_button("Assign Selected to Slot ▾", |ui| {
                        ui.label(
                            egui::RichText::new(format!("Sample: {}", sample.filename)).strong(),
                        );
                        ui.separator();
                        for slot_idx in 0..super::sample_pack::NUM_SLOTS {
                            let key_char = super::sample_pack::SLOT_KEY_CHARS[slot_idx];
                            let slot_title = self.pack_bank.slots[slot_idx].display_title();
                            let text = format!("[{key_char}] {slot_title}");
                            if ui.button(text).clicked() {
                                self.add_sample_to_slot(slot_idx, &sample);
                                self.slot_flash[slot_idx] = Some(std::time::Instant::now());
                                ui.close_menu();
                            }
                        }
                    });
                }
            }
        });
        ui.separator();
    }

    fn render_empty_state(&mut self, ui: &mut egui::Ui, current_folder: &std::path::Path) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.heading(
                egui::RichText::new("Sample Constellation Map")
                    .size(24.0)
                    .color(Color32::from_rgb(100, 200, 255)),
            );
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(
                    "Transform your audio folder into an interactive, color-coded 2D galaxy of sounds.\n\
                     Kicks, snares, hats, melodic synths, and basses are grouped by their physical timbral characteristics.",
                )
                .size(13.0)
                .color(Color32::from_gray(180)),
            );
            ui.add_space(20.0);

            let btn = egui::Button::new(
                egui::RichText::new("Index folder into sample constellation (recursive)")
                    .size(15.0)
                    .color(Color32::WHITE),
            )
            .fill(Color32::from_rgb(60, 100, 200))
            .min_size(Vec2::new(320.0, 42.0));

            if ui.add(btn).clicked() {
                self.start_indexing(current_folder);
            }

            ui.add_space(40.0);
        });
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

        // Pan handling
        if response.dragged() {
            self.pan += response.drag_delta();
        }

        // Zoom handling
        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.1 && response.hovered() {
            let pointer_pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
            let factor = if scroll_delta > 0.0 { 1.15 } else { 1.0 / 1.15 };
            let old_zoom = self.zoom;
            self.zoom = (self.zoom * factor).clamp(0.2, 50.0);

            // Zoom toward pointer
            let mouse_rel = pointer_pos - rect.center() - self.pan;
            self.pan -= mouse_rel * (self.zoom / old_zoom - 1.0);
        }

        // 1. Sleek Matte Slate Cosmic Void
        painter.rect_filled(rect, 0.0, Color32::from_rgb(26, 29, 34));
        let center = rect.center() + self.pan;

        // 2. High-Performance GPU Batched Star Rendering
        let view_scale = (rect.width().min(rect.height()) * 0.50).max(10.0) * self.zoom;
        let query_lower = self.search_query.to_lowercase();
        let has_query = !query_lower.is_empty();

        let mut hovered_sample: Option<ConstellationSample> = None;
        let mut hovered_pos = Pos2::ZERO;

        let mouse_pos = response.hover_pos().filter(|_| !response.dragged());

        // Allocate a single batched GPU mesh
        let mut star_mesh = egui::Mesh::default();
        let base_star_radius = (2.0_f32 * self.zoom.clamp(0.6, 3.5)).clamp(1.4, 6.0);
        let mut selected_sample_overlay: Option<(Pos2, Color32)> = None;

        // When hover-to-play is OFF: the highlighted sample is ALWAYS the closest one on canvas,
        // making click targets effortless and forgiving even if the cursor is far from any point.
        // When hover-to-play is ON: apply a sane screen cutoff based on zoom so traversing the empty void
        // between sample clumps does not continuously trigger auditory playback.
        let mut closest_hover_dist_sq = if self.hover_to_play {
            let screen_cutoff = (base_star_radius * 2.8 + 8.0).clamp(14.0, 26.0);
            screen_cutoff * screen_cutoff
        } else {
            f32::MAX
        };

        // Dynamic spatial anti-overlap push when zoomed in
        let push_factor = ((self.zoom - 1.8) / 0.8).clamp(0.0, 1.0);
        let mut relaxed_map: Option<std::collections::HashMap<&std::path::Path, Pos2>> = None;

        if push_factor > 0.0 {
            // Zoomed-in path: collect visible candidates and apply spatial repulsion
            let margin = 50.0_f32;
            let cull_min_x = rect.left() - margin;
            let cull_max_x = rect.right() + margin;
            let cull_min_y = rect.top() - margin;
            let cull_max_y = rect.bottom() + margin;

            struct VisStar<'a> {
                sample: &'a ConstellationSample,
                pos: Pos2,
                is_dimmed: bool,
            }

            let mut vis_stars: Vec<VisStar> = Vec::with_capacity(2048);

            for sample in &self.samples {
                let is_dimmed = if let Some(cat) = self.category_filter {
                    sample.category != cat
                } else {
                    false
                };

                let sx = center.x + sample.x * view_scale;
                let sy = center.y - sample.y * view_scale;

                if sx >= cull_min_x && sx <= cull_max_x && sy >= cull_min_y && sy <= cull_max_y {
                    vis_stars.push(VisStar {
                        sample,
                        pos: Pos2::new(sx, sy),
                        is_dimmed,
                    });
                }
            }

            // Spatial relaxation on visible stars
            let n_vis = vis_stars.len();
            if n_vis > 0 {
                let min_dist = (base_star_radius * 2.2).max(7.5) * push_factor;
                let min_dist_sq = min_dist * min_dist;
                let cell_size = (min_dist * 1.5).max(12.0);

                let origin_x = cull_min_x;
                let origin_y = cull_min_y;
                let grid_w = (((cull_max_x - origin_x) / cell_size).ceil() as usize + 2).max(1);
                let grid_h = (((cull_max_y - origin_y) / cell_size).ceil() as usize + 2).max(1);
                let total_cells = grid_w * grid_h;

                let mut head = vec![usize::MAX; total_cells];
                let mut next = vec![usize::MAX; n_vis];

                for (idx, star) in vis_stars.iter().enumerate() {
                    let gx = ((star.pos.x - origin_x) / cell_size) as usize;
                    let gy = ((star.pos.y - origin_y) / cell_size) as usize;
                    if gx < grid_w && gy < grid_h {
                        let cell = gy * grid_w + gx;
                        next[idx] = head[cell];
                        head[cell] = idx;
                    }
                }

                let mut disp = vec![Vec2::ZERO; n_vis];

                for (idx, star) in vis_stars.iter().enumerate() {
                    let gx = ((star.pos.x - origin_x) / cell_size) as usize;
                    let gy = ((star.pos.y - origin_y) / cell_size) as usize;
                    if gx >= grid_w || gy >= grid_h {
                        continue;
                    }

                    let mut dx_acc = 0.0f32;
                    let mut dy_acc = 0.0f32;

                    let gx_min = gx.saturating_sub(1);
                    let gx_max = (gx + 1).min(grid_w - 1);
                    let gy_min = gy.saturating_sub(1);
                    let gy_max = (gy + 1).min(grid_h - 1);

                    for cy in gy_min..=gy_max {
                        for cx in gx_min..=gx_max {
                            let cell = cy * grid_w + cx;
                            let mut cur = head[cell];
                            let mut checked = 0;
                            while cur != usize::MAX && checked < 12 {
                                checked += 1;
                                if cur != idx {
                                    let other_pos = vis_stars[cur].pos;
                                    let dx = star.pos.x - other_pos.x;
                                    let dy = star.pos.y - other_pos.y;
                                    let d2 = dx * dx + dy * dy;
                                    if d2 < min_dist_sq {
                                        let d = d2.sqrt();
                                        if d < 0.001 {
                                            let angle = ((idx * 37 + cur * 17) % 360) as f32
                                                * std::f32::consts::PI
                                                / 180.0;
                                            dx_acc += angle.cos() * (min_dist * 0.5);
                                            dy_acc += angle.sin() * (min_dist * 0.5);
                                        } else {
                                            let push = (min_dist - d) * 0.5;
                                            let inv = push / d;
                                            dx_acc += dx * inv;
                                            dy_acc += dy * inv;
                                        }
                                    }
                                }
                                cur = next[cur];
                            }
                        }
                    }
                    disp[idx] = Vec2::new(dx_acc * 0.75, dy_acc * 0.75);
                }

                let mut map = std::collections::HashMap::with_capacity(n_vis);

                // Apply displacements to visible stars and render
                for (idx, star) in vis_stars.iter_mut().enumerate() {
                    star.pos += disp[idx];
                    map.insert(star.sample.file_path.as_path(), star.pos);

                    let is_selected = self.selected_path.as_ref() == Some(&star.sample.file_path);
                    let base_color = star.sample.category.color();

                    if is_selected {
                        selected_sample_overlay = Some((star.pos, base_color));
                    }

                    // Check hover directly against screen position (only non-dimmed samples participate in hover)
                    if self.matches_filters(star.sample, &query_lower) {
                        if let Some(mp) = mouse_pos {
                            let dx = star.pos.x - mp.x;
                            let dy = star.pos.y - mp.y;
                            let d2 = dx * dx + dy * dy;
                            if d2 < closest_hover_dist_sq {
                                closest_hover_dist_sq = d2;
                                hovered_sample = Some(star.sample.clone());
                                hovered_pos = star.pos;
                            }
                        }
                    }

                    if is_selected {
                        continue;
                    }

                    // Frustum check for actual render
                    if !rect.contains(star.pos) {
                        continue;
                    }

                    let is_query_dimmed =
                        has_query && !star.sample.filename.to_lowercase().contains(&query_lower);

                    let color = if star.is_dimmed {
                        Color32::from_rgba_unmultiplied(
                            base_color.r(),
                            base_color.g(),
                            base_color.b(),
                            28,
                        )
                    } else if is_query_dimmed {
                        Color32::from_rgba_unmultiplied(
                            base_color.r(),
                            base_color.g(),
                            base_color.b(),
                            25,
                        )
                    } else {
                        base_color
                    };

                    add_star_diamond(&mut star_mesh, star.pos, base_star_radius, color);
                }

                relaxed_map = Some(map);
            }
        } else {
            // High-level zoom path: direct projection with sub-pixel density capping for maximum FPS
            let cell_size = 3.0_f32;
            let grid_cols = ((rect.width() / cell_size).ceil() as usize).max(1);
            let grid_rows = ((rect.height() / cell_size).ceil() as usize).max(1);
            let total_cells = grid_cols * grid_rows;
            let mut occupancy = vec![[0_u8; 2]; total_cells];

            let rect_min_x = rect.left() - base_star_radius;
            let rect_max_x = rect.right() + base_star_radius;
            let rect_min_y = rect.top() - base_star_radius;
            let rect_max_y = rect.bottom() + base_star_radius;

            for sample in &self.samples {
                let is_category_dimmed = if let Some(cat) = self.category_filter {
                    sample.category != cat
                } else {
                    false
                };

                let sx = center.x + sample.x * view_scale;
                let sy = center.y - sample.y * view_scale;

                // Frustum cull
                if sx < rect_min_x || sx > rect_max_x || sy < rect_min_y || sy > rect_max_y {
                    continue;
                }

                let is_selected = self.selected_path.as_ref() == Some(&sample.file_path);
                let base_color = sample.category.color();

                if is_selected {
                    selected_sample_overlay = Some((Pos2::new(sx, sy), base_color));
                }

                // Check hover directly against screen position (only non-dimmed samples participate in hover)
                if self.matches_filters(sample, &query_lower) {
                    if let Some(mp) = mouse_pos {
                        let dx = sx - mp.x;
                        let dy = sy - mp.y;
                        let d2 = dx * dx + dy * dy;
                        if d2 < closest_hover_dist_sq {
                            closest_hover_dist_sq = d2;
                            hovered_sample = Some(sample.clone());
                            hovered_pos = Pos2::new(sx, sy);
                        }
                    }
                }

                if is_selected {
                    continue;
                }

                let is_query_dimmed =
                    has_query && !sample.filename.to_lowercase().contains(&query_lower);
                // Cap foreground and background independently: all-sample views
                // remain bounded, while a dimmed star cannot hide a filter match.
                // Hit testing above still considers every sample, including stars
                // omitted from this coarse rendering mesh.
                let col = ((sx - rect.left()) / cell_size) as usize;
                let row = ((sy - rect.top()) / cell_size) as usize;
                if col < grid_cols && row < grid_rows {
                    let layer = usize::from(is_category_dimmed || is_query_dimmed);
                    let count = &mut occupancy[row * grid_cols + col][layer];
                    if *count >= 2 {
                        continue;
                    }
                    *count += 1;
                }

                let color = if is_category_dimmed {
                    Color32::from_rgba_unmultiplied(
                        base_color.r(),
                        base_color.g(),
                        base_color.b(),
                        28,
                    )
                } else if is_query_dimmed {
                    Color32::from_rgba_unmultiplied(
                        base_color.r(),
                        base_color.g(),
                        base_color.b(),
                        25,
                    )
                } else {
                    base_color
                };

                add_star_diamond(&mut star_mesh, Pos2::new(sx, sy), base_star_radius, color);
            }
        }

        // Render playback history trail (fades over steps and distance)
        self.render_playback_trail(&painter, center, view_scale, relaxed_map.as_ref());

        // Upload and render all stars to the GPU in a single draw call!
        if !star_mesh.is_empty() {
            painter.add(egui::Shape::mesh(star_mesh));
        }

        // Render selected sample prominent overlay
        if let Some((pos, color)) = selected_sample_overlay {
            let r = 7.0 * self.zoom.clamp(0.8, 2.5);
            painter.circle_stroke(pos, r + 4.0, Stroke::new(2.0_f32, Color32::WHITE));
            painter.circle_filled(
                pos,
                r + 8.0,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 80),
            );
            painter.circle_filled(pos, r, color);
            painter.circle_filled(pos, r * 0.45, Color32::WHITE);
        }

        // Render hovered sample prominent overlay
        if let Some(ref hovered) = hovered_sample {
            let r = 6.0 * self.zoom.clamp(0.8, 2.5);
            let color = hovered.category.color();
            painter.circle_stroke(hovered_pos, r + 3.0, Stroke::new(1.5_f32, color));
            painter.circle_filled(
                hovered_pos,
                r + 7.0,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 60),
            );
            painter.circle_filled(hovered_pos, r, color);
            painter.circle_filled(hovered_pos, r * 0.45, Color32::WHITE);
        }

        // 3. Hover and Click Interactions
        if let Some(hovered) = hovered_sample {
            // Click to select & play
            if response.clicked() {
                self.selected_path = Some(hovered.file_path.clone());
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
            self.render_sample_hud(ui, &painter, rect, hovered_pos, &hovered, current_folder);
        } else {
            self.last_hovered_path = None;
        }
    }

    fn render_sample_hud(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        screen_rect: Rect,
        star_pos: Pos2,
        sample: &ConstellationSample,
        current_folder: &std::path::Path,
    ) {
        // Poll any completed hover previews from background worker
        self.poll_hover_previews();

        let cached_preview = self.hover_previews.get(&sample.file_path).cloned();
        if cached_preview.is_none() {
            if self.pending_hover_path.as_ref() != Some(&sample.file_path) {
                self.pending_hover_path = Some(sample.file_path.clone());
                self.hover_cancelled
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.hover_cancelled =
                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let request = super::HoverPreviewRequest {
                    generation: self.folder_generation,
                    cancelled: self.hover_cancelled.clone(),
                    file_path: sample.file_path.clone(),
                    duration: sample.duration_seconds,
                    sample_rate: (sample.sample_rate > 0).then_some(sample.sample_rate),
                    channels: (sample.channels > 0).then_some(sample.channels),
                    bit_depth: (sample.bit_depth > 0).then_some(sample.bit_depth),
                };
                if self
                    .ensure_hover_worker()
                    .is_none_or(|sender| sender.try_send(request).is_err())
                {
                    self.pending_hover_path = None;
                }
                ui.ctx().request_repaint();
            }
        }

        let (sr, ch, bits) = if let Some(ref prev) = cached_preview {
            (prev.sample_rate, prev.channels, prev.bit_depth)
        } else {
            let sr = (sample.sample_rate > 0).then_some(sample.sample_rate);
            let ch = (sample.channels > 0).then_some(sample.channels);
            let bits = (sample.bit_depth > 0).then_some(sample.bit_depth);
            (sr, ch, bits)
        };

        let hud_w = 270.0;
        let hud_h = 126.0;

        let hud_x = if star_pos.x + 20.0 + hud_w > screen_rect.right() {
            star_pos.x - 20.0 - hud_w
        } else {
            star_pos.x + 20.0
        };
        let hud_y = if star_pos.y - 40.0 + hud_h > screen_rect.bottom() {
            screen_rect.bottom() - hud_h - 10.0
        } else {
            (star_pos.y - 30.0).max(screen_rect.top() + 10.0)
        };

        let hud_rect = Rect::from_min_size(Pos2::new(hud_x, hud_y), Vec2::new(hud_w, hud_h));

        // Connective beam from star to HUD card
        painter.line_segment(
            [
                star_pos,
                Pos2::new(
                    if hud_x < star_pos.x {
                        hud_rect.right()
                    } else {
                        hud_rect.left()
                    },
                    hud_rect.center().y,
                ),
            ],
            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(100, 150, 255, 80)),
        );

        // Glass card background
        painter.rect_filled(
            hud_rect,
            6.0,
            Color32::from_rgba_premultiplied(16, 20, 34, 242),
        );
        painter.rect_stroke(
            hud_rect,
            6.0,
            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(70, 100, 180, 140)),
        );

        // 1. Top Row: Filename (left) + Category Pill (right)
        let name_rect = Rect::from_min_size(
            hud_rect.min + Vec2::new(10.0, 8.0),
            Vec2::new(hud_w - 95.0, 16.0),
        );
        let short_name = short_label(&sample.filename, 22);
        painter.text(
            name_rect.min,
            egui::Align2::LEFT_TOP,
            short_name,
            egui::FontId::proportional(12.5),
            Color32::WHITE,
        );

        // Category Tag Pill
        let cat_color = sample.category.color();
        let pill_rect = Rect::from_min_size(
            hud_rect.min + Vec2::new(hud_w - 78.0, 7.0),
            Vec2::new(68.0, 17.0),
        );
        painter.rect_filled(
            pill_rect,
            4.0,
            Color32::from_rgba_premultiplied(
                cat_color.r() / 3,
                cat_color.g() / 3,
                cat_color.b() / 3,
                200,
            ),
        );
        painter.rect_stroke(pill_rect, 4.0, Stroke::new(1.0_f32, cat_color));
        painter.text(
            pill_rect.center(),
            egui::Align2::CENTER_CENTER,
            sample.category.display_name(),
            egui::FontId::proportional(10.0),
            Color32::WHITE,
        );

        // 2. Relative Path from Root
        let root = self
            .current_indexed_folder
            .as_deref()
            .unwrap_or(current_folder);
        let rel_str = match sample.file_path.strip_prefix(root) {
            Ok(rel) if rel.as_os_str().is_empty() => sample.filename.clone(),
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => {
                if let Some(parent) = sample.file_path.parent().and_then(|p| p.file_name()) {
                    format!("{}/{}", parent.to_string_lossy(), sample.filename)
                } else {
                    sample.filename.clone()
                }
            }
        };

        let short_rel = if rel_str.len() > 36 {
            let mut truncated = String::from("");
            truncated.extend(rel_str.chars().take(12));
            truncated.push_str(".../");
            truncated.push_str(&sample.filename);
            if truncated.len() > 38 {
                format!(".../{}", sample.filename)
            } else {
                truncated
            }
        } else {
            format!("{}", rel_str)
        };

        painter.text(
            hud_rect.min + Vec2::new(10.0, 27.0),
            egui::Align2::LEFT_TOP,
            short_rel,
            egui::FontId::proportional(10.0),
            Color32::from_rgb(145, 185, 235),
        );

        // 3. Colored Waveform Strip
        let wave_w = hud_w - 20.0;
        let wave_h = 32.0;
        let wave_rect = Rect::from_min_size(
            hud_rect.min + Vec2::new(10.0, 44.0),
            Vec2::new(wave_w, wave_h),
        );

        painter.rect_filled(
            wave_rect,
            3.0,
            Color32::from_rgba_premultiplied(10, 14, 24, 230),
        );
        painter.rect_stroke(
            wave_rect,
            3.0,
            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(60, 80, 130, 80)),
        );

        painter.line_segment(
            [
                Pos2::new(wave_rect.left() + 2.0, wave_rect.center().y),
                Pos2::new(wave_rect.right() - 2.0, wave_rect.center().y),
            ],
            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(70, 90, 140, 60)),
        );

        if let Some(ref prev) = cached_preview {
            if !prev.waveform.is_empty() {
                let count = prev.waveform.len().max(1);
                for (index, amplitude) in prev.waveform.iter().enumerate() {
                    let x =
                        wave_rect.left() + (index as f32 + 0.5) / count as f32 * wave_rect.width();
                    let height = (amplitude * wave_rect.height() * 0.48).max(1.0);
                    let color = prev
                        .waveform_colors
                        .get(index)
                        .map_or(Color32::from_rgb(100, 200, 255), |rgb| {
                            Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                        });
                    painter.line_segment(
                        [
                            Pos2::new(x, wave_rect.center().y - height),
                            Pos2::new(x, wave_rect.center().y + height),
                        ],
                        Stroke::new(1.5_f32, color),
                    );
                }
            } else {
                painter.text(
                    wave_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No audio waveform",
                    egui::FontId::proportional(10.0),
                    Color32::from_gray(120),
                );
            }
        } else {
            painter.text(
                wave_rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loading waveform...",
                egui::FontId::proportional(10.0),
                Color32::from_gray(140),
            );
        }

        // 4. Format Specs: Bit depth, Sample rate, Channels
        let mut specs = Vec::new();
        if let Some(b) = bits {
            specs.push(format!("{b}-bit"));
        }
        if let Some(s) = sr {
            specs.push(super::audio_probe::format_sample_rate(s));
        }
        if let Some(c) = ch {
            specs.push(super::audio_probe::format_channels(c));
        }
        let format_text = if specs.is_empty() {
            "Audio".to_string()
        } else {
            specs.join(" · ")
        };

        painter.text(
            hud_rect.min + Vec2::new(10.0, 83.0),
            egui::Align2::LEFT_TOP,
            format_text,
            egui::FontId::proportional(10.5),
            Color32::from_rgb(140, 215, 255),
        );

        // 5. Acoustic Stats: Duration, Frequency, Note
        let dur_str = if sample.duration_seconds < 1.0 {
            format!("{:.0} ms", sample.duration_seconds * 1000.0)
        } else {
            format!("{:.2} s", sample.duration_seconds)
        };

        let pitch_str = if sample.pitch_clarity > 0.4 && sample.pitch_hz > 30.0 {
            let note = freq_to_note_str(sample.pitch_hz);
            format!(" · {}", note)
        } else {
            String::new()
        };

        let stats_text = format!("{} · {:.0} Hz{}", dur_str, sample.brightness, pitch_str);
        painter.text(
            hud_rect.min + Vec2::new(10.0, 101.0),
            egui::Align2::LEFT_TOP,
            stats_text,
            egui::FontId::proportional(10.5),
            Color32::from_gray(175),
        );
    }

    fn play_nearest_neighbor(&mut self, on_play_sample: &mut dyn FnMut(&std::path::Path)) {
        let Some(selected) = &self.selected_path else {
            return;
        };

        let Some(curr) = self.samples.iter().find(|s| &s.file_path == selected) else {
            return;
        };

        let mut closest_sample: Option<ConstellationSample> = None;
        let mut min_dist = f32::MAX;

        let query = self.search_query.to_lowercase();
        for s in &self.samples {
            if &s.file_path == selected || !self.matches_filters(s, &query) {
                continue;
            }
            let dx = s.x - curr.x;
            let dy = s.y - curr.y;
            let dist = dx * dx + dy * dy;
            if dist < min_dist {
                min_dist = dist;
                closest_sample = Some(s.clone());
            }
        }

        if let Some(next_sample) = closest_sample {
            self.selected_path = Some(next_sample.file_path.clone());
            self.record_played_sample(&next_sample);
            on_play_sample(&next_sample.file_path);
        }
    }

    fn render_playback_trail(
        &self,
        painter: &egui::Painter,
        center: Pos2,
        view_scale: f32,
        relaxed_positions: Option<&std::collections::HashMap<&std::path::Path, Pos2>>,
    ) {
        let trail_len = self.history_trail.len();
        if trail_len < 2 {
            return;
        }

        let display_count = self.compute_trail_display_count();
        if display_count < 2 {
            return;
        }

        // Take the most recent `display_count` points in chronological order:
        // older points first, newest point last
        let start_idx = trail_len.saturating_sub(display_count);
        let active_slice: Vec<&super::HistoryTrailPoint> =
            self.history_trail.iter().skip(start_idx).collect();
        let n = active_slice.len();
        if n < 2 {
            return;
        }

        // Project each waypoint to screen coordinates (using relaxed position when available)
        let screen_positions: Vec<Pos2> = active_slice
            .iter()
            .map(|pt| {
                if let Some(map) = relaxed_positions {
                    if let Some(&p) = map.get(pt.file_path.as_path()) {
                        return p;
                    }
                }
                Pos2::new(center.x + pt.x * view_scale, center.y - pt.y * view_scale)
            })
            .collect();

        // Draw fading trajectory segments
        for i in 1..n {
            let p_prev = screen_positions[i - 1];
            let p_curr = screen_positions[i];

            // How many steps ago was this segment from the newest point?
            let step_from_new = (n - 1) - i;
            let t = 1.0 - (step_from_new as f32 / (n - 1) as f32); // 1.0 (newest) down to ~0.0 (oldest)

            let fade = t.powf(1.25);
            let alpha = (fade * 210.0).clamp(12.0, 210.0) as u8;
            let width = 1.2 + fade * 1.5;

            let c1 = active_slice[i - 1].category.color();
            let c2 = active_slice[i].category.color();

            // Blend colors with alpha
            let blended_r = (((c1.r() as u16 + c2.r() as u16) / 2) * alpha as u16 / 255) as u8;
            let blended_g = (((c1.g() as u16 + c2.g() as u16) / 2) * alpha as u16 / 255) as u8;
            let blended_b = (((c1.b() as u16 + c2.b() as u16) / 2) * alpha as u16 / 255) as u8;
            let line_color =
                Color32::from_rgba_premultiplied(blended_r, blended_g, blended_b, alpha);

            // Subtle outer glow for recent segments
            if fade > 0.40 {
                let glow_alpha = ((fade - 0.40) * 60.0) as u8;
                let glow_r =
                    (((c1.r() as u16 + c2.r() as u16) / 2) * glow_alpha as u16 / 255) as u8;
                let glow_g =
                    (((c1.g() as u16 + c2.g() as u16) / 2) * glow_alpha as u16 / 255) as u8;
                let glow_b =
                    (((c1.b() as u16 + c2.b() as u16) / 2) * glow_alpha as u16 / 255) as u8;
                let glow_color =
                    Color32::from_rgba_premultiplied(glow_r, glow_g, glow_b, glow_alpha);
                painter.line_segment([p_prev, p_curr], Stroke::new(width + 2.5, glow_color));
            }

            // Core trail line
            painter.line_segment([p_prev, p_curr], Stroke::new(width, line_color));

            // Soft glowing node at previous waypoint
            let node_r = 1.6 + fade * 1.8;
            painter.circle_filled(p_prev, node_r, line_color);
        }

        // Draw prominent glowing node at newest waypoint
        if let Some(&last_pos) = screen_positions.last() {
            let last_color = active_slice.last().unwrap().category.color();
            let glow = Color32::from_rgba_unmultiplied(
                last_color.r(),
                last_color.g(),
                last_color.b(),
                180,
            );
            painter.circle_filled(last_pos, 4.0, glow);
            painter.circle_filled(last_pos, 2.0, Color32::WHITE);
        }
    }
}

/// Shortens a label at Unicode scalar boundaries, including room for an ASCII ellipsis.
pub(super) fn short_label(value: &str, limit: usize) -> String {
    if value.chars().nth(limit).is_some() {
        format!(
            "{}...",
            value
                .chars()
                .take(limit.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        value.to_owned()
    }
}

fn freq_to_note_str(freq: f32) -> String {
    let note_names = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let rounded = midi.round() as i32;
    let note_idx = rounded.rem_euclid(12) as usize;
    let octave = (rounded / 12) - 1;
    format!("{}{}", note_names[note_idx], octave)
}

#[inline]
fn add_star_diamond(mesh: &mut egui::Mesh, pos: Pos2, r: f32, color: Color32) {
    let v_start = mesh.vertices.len() as u32;
    mesh.vertices.push(egui::epaint::Vertex {
        pos: Pos2::new(pos.x, pos.y - r),
        uv: egui::epaint::WHITE_UV,
        color,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: Pos2::new(pos.x + r, pos.y),
        uv: egui::epaint::WHITE_UV,
        color,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: Pos2::new(pos.x, pos.y + r),
        uv: egui::epaint::WHITE_UV,
        color,
    });
    mesh.vertices.push(egui::epaint::Vertex {
        pos: Pos2::new(pos.x - r, pos.y),
        uv: egui::epaint::WHITE_UV,
        color,
    });
    mesh.indices.extend_from_slice(&[
        v_start,
        v_start + 1,
        v_start + 2,
        v_start,
        v_start + 2,
        v_start + 3,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coarse_mesh(state: &mut ConstellationState) -> Vec<egui::epaint::Vertex> {
        let context = egui::Context::default();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    state.render_starfield(
                        ui,
                        std::path::Path::new("/samples"),
                        &mut |_| {},
                        &mut |_| {},
                    );
                });
            },
        );
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.vertices.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn dense_coarse_mesh_is_bounded_without_hiding_category_or_search_matches() {
        let mut state = ConstellationState::new();
        state.samples = (0..1000)
            .map(|index| ConstellationSample {
                file_path: std::path::PathBuf::from(format!("/samples/{index}.wav")),
                folder_path: "/samples".into(),
                filename: if index == 999 {
                    "target.wav".into()
                } else {
                    "background.wav".into()
                },
                category: if index == 999 {
                    SampleCategory::Snare
                } else {
                    SampleCategory::Kick
                },
                confidence: 1.0,
                duration_seconds: 0.5,
                pitch_hz: 0.0,
                pitch_clarity: 0.0,
                brightness: 0.0,
                x: 0.0,
                y: 0.0,
                file_size: 100,
                mtime: 0,
                sample_rate: 44100,
                channels: 1,
                bit_depth: 16,
            })
            .collect();
        assert_eq!(
            coarse_mesh(&mut state).len(),
            8,
            "All view paints at most two stars in a cell"
        );
        state.category_filter = Some(SampleCategory::Snare);
        let solo = coarse_mesh(&mut state);
        assert_eq!(solo.len(), 12, "two background stars and one matching star");
        assert!(
            solo.iter()
                .any(|vertex| vertex.color == SampleCategory::Snare.color())
        );
        state.category_filter = None;
        state.search_query = "target".into();
        let search = coarse_mesh(&mut state);
        assert_eq!(search.len(), 12);
        assert!(
            search
                .iter()
                .any(|vertex| vertex.color == SampleCategory::Snare.color())
        );
    }

    #[test]
    fn escape_clears_search_before_the_text_edit_surrenders_focus() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::new();
        state.search_query = "kick".into();
        state.focus_search = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_toolbar(ui, std::path::Path::new("/samples"), &mut |_| {});
            });
        });
        assert!(ctx.wants_keyboard_input());
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_toolbar(ui, std::path::Path::new("/samples"), &mut |_| {});
            });
        });
        assert!(state.search_query.is_empty());
    }

    #[test]
    fn unicode_sample_names_are_shortened_without_splitting_utf8() {
        let name = "打楽器のサンプル🎵_café_very_long_sample.wav";
        let short = short_label(name, 22);
        assert_eq!(short.chars().count(), 22);
        assert!(short.ends_with("..."));
        assert_eq!(short_label("短い.wav", 22), "短い.wav");
    }
}
