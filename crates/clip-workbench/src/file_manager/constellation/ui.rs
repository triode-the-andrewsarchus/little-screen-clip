//! Constellation 2D sample map visualizer and interactive space.
//! Inspired by 2D audio sample mapping tools.

use super::ConstellationState;
use super::db::ConstellationSample;
use super::generated_model::SampleCategory;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};

/// Keeps category and search dimming consistent across both map projection paths.
fn filtered_star_color(base: Color32, category_dimmed: bool, filter_dimmed: bool) -> Color32 {
    let alpha = if category_dimmed {
        28
    } else if filter_dimmed {
        25
    } else {
        255
    };
    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha)
}

impl ConstellationState {
    fn matches_filters(&self, sample: &ConstellationSample, lowercase_query: &str) -> bool {
        self.category_filter
            .is_none_or(|category| sample.category == category)
            && (lowercase_query.is_empty()
                || sample.filename.to_lowercase().contains(lowercase_query))
            && sample.duration_seconds >= self.min_duration
            && (self.max_duration == 0.0 || sample.duration_seconds <= self.max_duration)
            && match self.channel_filter {
                1 => sample.channels == 1,
                2 => sample.channels == 2,
                3 => sample.channels > 2,
                _ => true,
            }
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
        self.render_toolbar(ui, current_folder, on_play_sample);
        self.render_indexing_progress(ui);

        self.map_viewport = Some(ui.available_rect_before_wrap().shrink(8.0));
        if self.pack_shelf_open {
            egui::SidePanel::right("constellation_pack_shelf_panel")
                .resizable(true)
                .default_width(360.0)
                .width_range(280.0..=420.0)
                .frame(egui::Frame::none().inner_margin(8.0))
                .show_inside(ui, |ui| {
                    // Keep content requests from changing the user's divider width.
                    // egui 0.27 otherwise stores overflowing child bounds as panel size.
                    let bounds = ui.available_rect_before_wrap();
                    ui.allocate_rect(bounds, Sense::hover());
                    let mut content = ui.child_ui(bounds, egui::Layout::top_down(egui::Align::Min));
                    let ui = &mut content;

                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    use crate::ui_tabs::{Tab, TabBar};
                    let selected_tab = ui
                        .horizontal(|ui| {
                            let tabs_width = (ui.available_width() - 32.0).max(1.0);
                            let selected = ui
                                .allocate_ui(Vec2::new(tabs_width, 28.0), |ui| {
                                    TabBar::new(
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
                                    .show(ui)
                                })
                                .inner;
                            if ui
                                .add(
                                    egui::Button::image(
                                        crate::icons::Icon::Close
                                            .image_tinted(12.0, ui.visuals().text_color()),
                                    )
                                    .small(),
                                )
                                .on_hover_text("Collapse rack and editor")
                                .clicked()
                            {
                                self.pack_shelf_open = false;
                            }
                            selected
                        })
                        .inner;
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
                        egui::ScrollArea::vertical()
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
                    let response = ui.allocate_rect(rect, Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(
                            rect,
                            0.0,
                            ui.visuals().widgets.hovered.weak_bg_fill,
                        );
                    }
                    let icon =
                        crate::icons::Icon::Plus.image_tinted(12.0, ui.visuals().text_color());
                    icon.paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(12.0)));
                    if response.on_hover_text("Show rack and editor").clicked() {
                        self.pack_shelf_open = true;
                    }
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().inner_margin(8.0))
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
                        .default_height(340.0)
                        .height_range(240.0..=420.0)
                        .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 6.0)))
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

    fn render_indexing_progress(&mut self, ui: &mut egui::Ui) {
        let Some(status) = self.indexing_status.clone() else {
            return;
        };
        if self.active_job.is_some() {
            let elapsed = self
                .indexing_started
                .map_or(0.0, |started| started.elapsed().as_secs_f32());
            let text = self.indexing_progress.map_or_else(
                || format!("Discovering samples · {elapsed:.1}s"),
                |progress| format!("{:.0}% · {elapsed:.1}s", progress * 100.0),
            );
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 85.0).max(20.0);
                ui.add(
                    egui::ProgressBar::new(self.indexing_progress.unwrap_or(0.0))
                        .fill(ui.visuals().widgets.active.fg_stroke.color)
                        .animate(true)
                        .desired_width(width)
                        .text(text),
                );
                if ui.button("Cancel").clicked() {
                    self.cancel_indexing();
                }
            });
            ui.label(
                egui::RichText::new(status)
                    .small()
                    .monospace()
                    .color(ui.visuals().weak_text_color()),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label(status);
                if ui.small_button("Dismiss").clicked() {
                    self.indexing_status = None;
                }
            });
        }
        ui.separator();
    }

    fn render_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        _current_folder: &std::path::Path,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
    ) {
        ui.horizontal_wrapped(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .id_source("constellation-search")
                    .hint_text("Filter samples · Ctrl+F")
                    .desired_width(170.0),
            );
            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }
            if (response.has_focus() || response.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Escape))
            {
                self.search_query.clear();
                response.surrender_focus();
            }
            if ui.add(egui::Button::new("Hover audition").selected(self.hover_to_play).stroke(ui.visuals().widgets.noninteractive.bg_stroke)).on_hover_text("Play samples as the pointer passes over them").clicked() {
                self.hover_to_play = !self.hover_to_play;
            }
            self.render_audio_filters(ui);
            if ui
                .add_enabled(!self.samples.is_empty(), egui::Button::new("Random"))
                .clicked()
            {
                let query = self.search_query.to_lowercase();
                let eligible: Vec<_> = self
                    .samples
                    .iter()
                    .filter(|sample| self.matches_filters(sample, &query))
                    .collect();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos() as usize;
                if let Some(sample) = eligible
                    .get(nanos % eligible.len().max(1))
                    .map(|sample| (*sample).clone())
                {
                    self.selected_path = Some(sample.file_path.clone());
                    self.record_played_sample(&sample);
                    on_play_sample(&sample.file_path);
                }
            }
            if ui
                .add_enabled(self.selected_path.is_some(), egui::Button::new("Next unplayed"))
                .on_hover_text("Audition the nearest unplayed sample (Shift+Space); click a sample to start a new exploration")
                .clicked()
            {
                self.play_nearest_neighbor(on_play_sample);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(
                    egui::Button::new(format!("All ({})", self.samples.len()))
                        .selected(self.category_filter.is_none())
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke),
                )
                .clicked()
            {
                self.category_filter = None;
            }
            let mut counts = [0usize; 12];
            for sample in &self.samples {
                counts[sample.category as usize] += 1;
            }
            for category in SampleCategory::ALL {
                let count = counts[category as usize];
                if count == 0 && self.category_filter != Some(category) {
                    continue;
                }
                let active = self.category_filter == Some(category);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(format!("{} ({count})", category.display_name()))
                                .color(category.color()),
                        )
                        .selected(active)
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke),
                    )
                    .clicked()
                {
                    self.category_filter = if active { None } else { Some(category) };
                }
            }
        });
        ui.separator();
    }

    pub(crate) fn sample_count_label(&self) -> String {
        let query = self.search_query.to_lowercase();
        let active = self
            .samples
            .iter()
            .filter(|sample| self.matches_filters(sample, &query))
            .count();
        format!("{active} / {} samples", self.samples.len())
    }

    /// Two duration endpoints use the observed library bounds; zero retains "all" semantics.
    fn render_duration_range(&mut self, ui: &mut egui::Ui) {
        let shortest = self
            .samples
            .iter()
            .map(|sample| sample.duration_seconds)
            .filter(|duration| duration.is_finite())
            .reduce(f32::min)
            .unwrap_or(0.0);
        let longest = self
            .samples
            .iter()
            .map(|sample| sample.duration_seconds)
            .filter(|duration| duration.is_finite())
            .reduce(f32::max)
            .unwrap_or(shortest);
        let mut low = self.min_duration.max(shortest).min(longest);
        let mut high = if self.max_duration == 0.0 {
            longest
        } else {
            self.max_duration.clamp(low, longest)
        };
        ui.label("Duration");
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(160.0, 24.0), Sense::click_and_drag());
        let track = rect.shrink2(Vec2::new(8.0, 0.0));
        let span = (longest - shortest).max(f32::EPSILON);
        let x_at = |time: f32| track.left() + (time - shortest) / span * track.width();
        let low_x = x_at(low);
        let high_x = x_at(high);
        let handle_id = response.id.with("duration_endpoint");
        if response.drag_started() || response.clicked() {
            let x = ui
                .input(|input| input.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos())
                .map_or(low_x, |pos| pos.x);
            ui.data_mut(|data| {
                data.insert_temp(handle_id, (x - low_x).abs() <= (x - high_x).abs())
            });
        }
        if (response.dragged() || response.clicked())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let time =
                shortest + ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0) * span;
            if ui
                .data(|data| data.get_temp::<bool>(handle_id))
                .unwrap_or(true)
            {
                low = time.min(high);
            } else {
                high = time.max(low);
            }
            self.min_duration = if low <= shortest { 0.0 } else { low };
            self.max_duration = if high >= longest { 0.0 } else { high };
        }
        if response.double_clicked() {
            self.min_duration = 0.0;
            self.max_duration = 0.0;
        }
        let y = track.center().y;
        let painter = ui.painter_at(rect);
        painter.line_segment(
            [egui::pos2(track.left(), y), egui::pos2(track.right(), y)],
            Stroke::new(3.0_f32, ui.visuals().widgets.inactive.bg_fill),
        );
        painter.line_segment(
            [egui::pos2(x_at(low), y), egui::pos2(x_at(high), y)],
            Stroke::new(3.0_f32, ui.visuals().selection.bg_fill),
        );
        for time in [low, high] {
            painter.circle(
                egui::pos2(x_at(time), y),
                6.0,
                ui.visuals().widgets.inactive.bg_fill,
                ui.visuals().widgets.inactive.fg_stroke,
            );
        }
        response.on_hover_text(format!(
            "{low:.3}–{high:.3} s · drag either end · double-click to reset"
        ));
        ui.weak(format!("{low:.2}–{high:.2} s"));
    }

    fn render_audio_filters(&mut self, ui: &mut egui::Ui) {
        self.render_duration_range(ui);
        if ui
            .add(egui::Button::new("Mono samples only").selected(self.channel_filter == 1))
            .clicked()
        {
            self.channel_filter = if self.channel_filter == 1 { 0 } else { 1 };
        }
    }

    fn render_empty_state(&mut self, ui: &mut egui::Ui, current_folder: &std::path::Path) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(10.0));
            ui.heading("Sample constellation");
            ui.add_space(8.0);
            ui.label("Explore samples by sound. Similar timbres sit together.");
            ui.label("Choose a sample, then assign it to a drum pad.");
            ui.add_space(16.0);
            if ui
                .button("Index folder")
                .on_hover_text("Analyze audio in this folder and its subfolders")
                .clicked()
            {
                self.start_indexing(current_folder);
            }
        });
    }

    /// Distinguishes a sample-reference drag from empty-canvas panning.
    fn handle_map_drag(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        center: Pos2,
        view_scale: f32,
        relaxed_map: Option<&std::collections::HashMap<&std::path::Path, Pos2>>,
        query_lower: &str,
    ) -> Option<std::path::PathBuf> {
        if response.drag_started() {
            if let Some(origin) = ui.input(|input| input.pointer.press_origin()) {
                let nearest = self
                    .samples
                    .iter()
                    .filter(|sample| self.matches_filters(sample, query_lower))
                    .filter_map(|sample| {
                        let pos = relaxed_map
                            .and_then(|map| map.get(sample.file_path.as_path()))
                            .copied()
                            .unwrap_or(center + Vec2::new(sample.x, -sample.y) * view_scale);
                        let distance = pos.distance_sq(origin);
                        (distance <= 144.0).then_some((sample, distance))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((sample, _)) = nearest {
                    response.dnd_set_drag_payload(crate::file_manager::transfers::FileDrag {
                        paths: vec![sample.file_path.clone()],
                        copy_only: true,
                    });
                    return Some(sample.file_path.clone());
                }
            }
        }
        None
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

        // Zoom handling
        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.1 && response.hovered() {
            let pointer_pos = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
            let factor = if scroll_delta > 0.0 { 1.15 } else { 1.0 / 1.15 };
            let old_zoom = self.zoom;
            self.zoom = (self.zoom * factor).clamp(0.2, 50.0);

            // Zoom toward pointer
            let mouse_rel = pointer_pos - viewport.center() - self.pan;
            self.pan -= mouse_rel * (self.zoom / old_zoom - 1.0);
        }

        // The map inherits the file browser canvas; only samples carry category color.
        let center = viewport.center() + self.pan;

        // 2. High-Performance GPU Batched Star Rendering
        // Organic category coordinates can extend beyond ±1. Fit their actual
        // extent, leaving a margin, while preserving every relative position.
        let extent = self.samples.iter().fold(1.0_f32, |extent, sample| {
            extent.max(sample.x.abs()).max(sample.y.abs())
        });
        let view_scale =
            (viewport.width().min(viewport.height()) * 0.45).max(1.0) / extent * self.zoom;
        let query_lower = self.search_query.to_lowercase();

        let mut hovered_sample: Option<ConstellationSample> = None;
        let mut hovered_pos = Pos2::ZERO;

        let mouse_pos = response.hover_pos().filter(|_| !response.dragged());

        // Allocate a single batched GPU mesh
        let mut star_mesh = egui::Mesh::default();
        let base_star_radius = (4.0_f32 * self.zoom.clamp(0.6, 2.0)).clamp(2.0, 8.0);
        // Count projected neighbors without suppressing any point. The full viewport
        // keeps radii stable when docks obscure part of the map.
        let density = PointDensity::new(
            self.samples
                .iter()
                .map(|sample| {
                    Pos2::new(
                        center.x + sample.x * view_scale,
                        center.y - sample.y * view_scale,
                    )
                })
                .filter(|pos| viewport.expand(32.0).contains(*pos)),
        );

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
            let cull_min_x = viewport.left() - margin;
            let cull_max_x = viewport.right() + margin;
            let cull_min_y = viewport.top() - margin;
            let cull_max_y = viewport.bottom() + margin;

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
                let mut positions: Vec<_> = vis_stars.iter().map(|star| star.pos).collect();
                relax_star_positions(&mut positions, min_dist);
                let disp: Vec<_> = positions
                    .iter()
                    .zip(&vis_stars)
                    .map(|(position, star)| *position - star.pos)
                    .collect();

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

                    let is_filter_dimmed = !self.matches_filters(star.sample, &query_lower);

                    let color = filtered_star_color(base_color, star.is_dimmed, is_filter_dimmed);

                    add_star_diamond(
                        &mut star_mesh,
                        star.pos,
                        map_point_radius(&density, star.pos, base_star_radius, self.zoom),
                        color,
                    );
                }

                relaxed_map = Some(map);
            }
        } else {
            // High-level zoom path: direct projection with sub-pixel density capping for maximum FPS
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

                let is_filter_dimmed = !self.matches_filters(sample, &query_lower);
                let color = filtered_star_color(base_color, is_category_dimmed, is_filter_dimmed);

                add_star_diamond(
                    &mut star_mesh,
                    Pos2::new(sx, sy),
                    map_point_radius(&density, Pos2::new(sx, sy), base_star_radius, self.zoom),
                    color,
                );
            }
        }

        let dragged_path = self.handle_map_drag(
            ui,
            &response,
            center,
            view_scale,
            relaxed_map.as_ref(),
            &query_lower,
        );

        if let Some(path) = dragged_path {
            self.selected_path = Some(path);
        }
        if response.dragged()
            && !egui::DragAndDrop::has_payload_of_type::<crate::file_manager::transfers::FileDrag>(
                ui.ctx(),
            )
        {
            self.pan += response.drag_delta();
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
                self.visited_samples.clear();
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
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );

        // Glass card background
        painter.rect_filled(hud_rect, 6.0, ui.visuals().window_fill());
        painter.rect_stroke(
            hud_rect,
            6.0,
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
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
            ui.visuals().text_color(),
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
            ui.visuals().weak_text_color(),
        );

        // 3. Colored Waveform Strip
        let wave_w = hud_w - 20.0;
        let wave_h = 32.0;
        let wave_rect = Rect::from_min_size(
            hud_rect.min + Vec2::new(10.0, 44.0),
            Vec2::new(wave_w, wave_h),
        );

        painter.rect_filled(wave_rect, 3.0, ui.visuals().extreme_bg_color);
        painter.rect_stroke(
            wave_rect,
            3.0,
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );

        painter.line_segment(
            [
                Pos2::new(wave_rect.left() + 2.0, wave_rect.center().y),
                Pos2::new(wave_rect.right() - 2.0, wave_rect.center().y),
            ],
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
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
                    ui.visuals().weak_text_color(),
                );
            }
        } else {
            painter.text(
                wave_rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loading waveform...",
                egui::FontId::proportional(10.0),
                ui.visuals().weak_text_color(),
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
            ui.visuals().text_color(),
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
            ui.visuals().weak_text_color(),
        );
    }

    /// Retriggers the selection, or advances without revisiting an auditioned sample.
    ///
    /// Exhausting filtered neighbors leaves the current selection unchanged.
    pub(crate) fn space_audition(&mut self, advance: bool) -> Option<std::path::PathBuf> {
        if self.browser_open {
            return None;
        }
        self.playback.lock().unwrap().show_midi_notes = false;
        if !advance && self.pack_shelf_open && self.pad_editor_open && !self.output_editor_open {
            let layers = &self.pack_bank.slots[self.selected_slot].samples;
            if let Some(sample) = layers
                .get(self.selected_layers[self.selected_slot].min(layers.len().saturating_sub(1)))
                .cloned()
            {
                self.stop_map_audition = true;
                self.audition_layers(&[sample]);
                return None;
            }
        }
        if advance {
            let mut next = None;
            self.play_nearest_neighbor(&mut |path| next = Some(path.to_path_buf()));
            next
        } else {
            let sample = self
                .samples
                .iter()
                .find(|sample| Some(&sample.file_path) == self.selected_path.as_ref())?
                .clone();
            self.record_played_sample(&sample);
            Some(sample.file_path)
        }
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
            if &s.file_path == selected
                || self.visited_samples.contains(&s.file_path)
                || !self.matches_filters(s, &query)
            {
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

pub(super) fn freq_to_note_str(freq: f32) -> String {
    let note_names = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let midi = 69.0 + 12.0 * (freq / 440.0).log2();
    let rounded = midi.round() as i32;
    let note_idx = rounded.rem_euclid(12) as usize;
    let octave = (rounded / 12) - 1;
    format!("{}{}", note_names[note_idx], octave)
}

/// Fully uniform radii for the final four wheel steps, blended in beforehand.
fn map_point_radius(density: &PointDensity, point: Pos2, maximum: f32, zoom: f32) -> f32 {
    let uniform_at = 50.0 / 1.15_f32.powi(4);
    let blend = ((zoom - uniform_at / 1.3) / (uniform_at - uniform_at / 1.3)).clamp(0.0, 1.0);
    let compact = density.radius(point, maximum);
    compact + (maximum - compact) * blend
}

/// Deterministic local spacing; bounded neighbor work and displacement retain the map shape.
fn relax_star_positions(points: &mut [Pos2], spacing: f32) {
    if spacing <= 0.0 {
        return;
    }
    let original = points.to_vec();
    let cell = |pos: Pos2| {
        (
            (pos.x / spacing).floor() as i32,
            (pos.y / spacing).floor() as i32,
        )
    };
    let iterations = if points.len() <= 2048 { 16 } else { 4 };
    for _ in 0..iterations {
        let mut grid: std::collections::HashMap<_, Vec<usize>> = std::collections::HashMap::new();
        for (index, point) in points.iter().enumerate() {
            grid.entry(cell(*point)).or_default().push(index);
        }
        let mut next = points.to_vec();
        for (index, point) in points.iter().enumerate() {
            let (x, y) = cell(*point);
            let mut displacement = Vec2::ZERO;
            for cy in y - 1..=y + 1 {
                for cx in x - 1..=x + 1 {
                    for other in grid.get(&(cx, cy)).into_iter().flatten().take(48).copied() {
                        if index == other {
                            continue;
                        }
                        let delta = *point - points[other];
                        let distance = delta.length();
                        if distance >= spacing {
                            continue;
                        }
                        let direction = if distance > 0.001 {
                            delta / distance
                        } else {
                            let seed = index.min(other).wrapping_mul(73856093)
                                ^ index.max(other).wrapping_mul(19349663);
                            let angle = (seed % 360) as f32 * std::f32::consts::PI / 180.0;
                            Vec2::angled(angle) * if index < other { 1.0 } else { -1.0 }
                        };
                        displacement += direction * (spacing - distance) * 0.5;
                    }
                }
            }
            displacement *= spacing / displacement.length().max(spacing);
            let mut offset = *point + displacement - original[index];
            offset *= 64.0 / offset.length().max(64.0);
            next[index] = original[index] + offset;
        }
        points.copy_from_slice(&next);
    }
}

/// Local screen density controls size, never whether a sample is drawn.
struct PointDensity(std::collections::HashMap<(i32, i32), usize>);
impl PointDensity {
    fn cell(pos: Pos2) -> (i32, i32) {
        ((pos.x / 32.0).floor() as i32, (pos.y / 32.0).floor() as i32)
    }
    fn new(points: impl Iterator<Item = Pos2>) -> Self {
        let mut cells = std::collections::HashMap::new();
        for point in points {
            *cells.entry(Self::cell(point)).or_insert(0) += 1;
        }
        Self(cells)
    }
    fn radius(&self, point: Pos2, maximum: f32) -> f32 {
        let count = self.0.get(&Self::cell(point)).copied().unwrap_or(1) as f32;
        (32.0 * 32.0 * 0.12 / (2.0 * count))
            .sqrt()
            .clamp(0.35, maximum)
    }
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

    fn sample_fixture(index: usize) -> ConstellationSample {
        ConstellationSample {
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
            source_revision: None,
        }
    }

    #[test]
    fn duration_handles_keep_the_grabbed_endpoint_and_reset_on_double_click() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::new();
        state.samples = (0..2).map(sample_fixture).collect();
        state.samples[1].duration_seconds = 2.0;
        let render = |state: &mut ConstellationState, events, time| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 300.0))),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| state.render_duration_range(ui));
                },
            )
        };
        let output = render(&mut state, vec![], 0.0);
        let handles: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Circle(circle) if circle.radius == 6.0 => Some(circle.center),
                _ => None,
            })
            .collect();
        assert_eq!(handles.len(), 2);
        for (round, (start, fraction, expected_low, expected_high)) in [
            (handles[0], 0.75, 1.625, 0.0),
            (handles[1], 0.9, 1.625, 1.85),
        ]
        .into_iter()
        .enumerate()
        {
            let target = handles[0].lerp(handles[1], fraction);
            for phase in 0..3 {
                let pos = if phase == 0 { start } else { target };
                let mut events = vec![egui::Event::PointerMoved(pos)];
                if phase != 1 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: phase == 0,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                render(&mut state, events, 0.1 + round as f64 + phase as f64 * 0.1);
            }
            assert!((state.min_duration - expected_low).abs() < 0.001);
            assert!((state.max_duration - expected_high).abs() < 0.001);
        }
        let pos = handles[0].lerp(handles[1], 0.5);
        for (index, pressed) in [true, false, true, false].into_iter().enumerate() {
            render(
                &mut state,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                3.0 + index as f64 * 0.05,
            );
        }
        assert_eq!((state.min_duration, state.max_duration), (0.0, 0.0));
    }

    #[test]
    fn n_advances_only_when_unbound_and_outside_text_entry() {
        for (assigned, editing) in [(false, false), (true, false), (false, true)] {
            let ctx = egui::Context::default();
            let mut state = ConstellationState::new();
            state.samples = (0..2).map(sample_fixture).collect();
            state.samples[1].x = 0.1;
            state.selected_path = Some(state.samples[0].file_path.clone());
            if assigned {
                state.pad_keys[0] = 'n';
            }
            state.focus_search = editing;
            state.playback.lock().unwrap().show_midi_notes = true;
            let mut auditions = Vec::new();
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::N,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(
                            ui,
                            std::path::Path::new("/samples"),
                            &Default::default(),
                            &mut |path| auditions.push(path.to_owned()),
                            &mut |_| {},
                        )
                    });
                },
            );
            assert_eq!(auditions.len(), usize::from(!assigned && !editing));
            assert_eq!(state.playback.lock().unwrap().show_midi_notes, editing);
        }
        assert_eq!(
            freq_to_note_str(440.0 * 2.0_f32.powf((36.0 - 69.0) / 12.0)),
            "C2"
        );
        assert_eq!(
            freq_to_note_str(440.0 * 2.0_f32.powf((51.0 - 69.0) / 12.0)),
            "D#3"
        );
    }

    #[test]
    fn space_plays_only_the_active_editor_layer_through_the_bus() {
        use super::super::sample_pack::{CachedAudio, SlotSample};
        use std::sync::Arc;
        let mut state = ConstellationState::new();
        let map = sample_fixture(0);
        state.selected_path = Some(map.file_path.clone());
        state.samples.push(map);
        state.pack_bank.compressor_bypass = true;
        for value in [0.2, 0.6] {
            let mut layer = SlotSample::new("layer.wav".into(), "layer.wav".into(), None, 1.0);
            let pcm = Arc::new(vec![value; 48000]);
            layer.cached_audio = Some(Arc::new(CachedAudio {
                channels: 1,
                sample_rate: 48000,
                samples: pcm.clone(),
                mono_samples: pcm,
                waveform_peaks: vec![],
                waveform_colors: vec![],
            }));
            state.pack_bank.slots[0].samples.push(layer);
        }
        state.selected_layers[0] = 1;
        let (player, mut rendered) = rodio::Player::new();
        state.playback.lock().unwrap().output =
            Some(super::super::output::PadOutput::with_player(player, true));
        assert_eq!(state.space_audition(false), None);
        for _ in 0..4000 {
            rendered.next();
        }
        assert!((rendered.next().unwrap() - 0.6 * 0.7943282).abs() < 0.00001);
        assert!(state.stop_map_audition);
        state.pack_shelf_open = false;
        assert_eq!(
            state.space_audition(false),
            Some(state.samples[0].file_path.clone())
        );
    }

    #[test]
    fn status_count_includes_search_category_duration_and_channel_filters() {
        let mut state = ConstellationState::new();
        state.samples = (0..4).map(sample_fixture).collect();
        state.samples[1].duration_seconds = 2.0;
        state.samples[2].channels = 2;
        state.samples[3].category = SampleCategory::Snare;
        assert_eq!(state.sample_count_label(), "4 / 4 samples");
        state.channel_filter = 1;
        state.max_duration = 1.0;
        state.category_filter = Some(SampleCategory::Kick);
        assert_eq!(state.sample_count_label(), "1 / 4 samples");
        state.search_query = "absent".into();
        assert_eq!(state.sample_count_label(), "0 / 4 samples");
    }

    #[test]
    fn final_zoom_steps_use_equal_radii_and_separate_coincident_points() {
        let point = Pos2::new(100.0, 100.0);
        let density = PointDensity::new(std::iter::repeat_n(point, 1000));
        for step in 0..=4 {
            let zoom = 50.0 / 1.15_f32.powi(step);
            assert!((map_point_radius(&density, point, 8.0, zoom) - 8.0).abs() < 0.00001);
        }
        let mut points = vec![point; 4];
        relax_star_positions(&mut points, 18.0);
        for (index, first) in points.iter().enumerate() {
            assert!((*first - point).length() <= 64.001);
            for other in points.iter().skip(index + 1) {
                assert!((*first - *other).length() > 17.5);
            }
        }
        let mut repeat = vec![point; 4];
        relax_star_positions(&mut repeat, 18.0);
        assert_eq!(points, repeat);
    }

    #[test]
    fn space_retriggers_and_neighbors_never_revisit_until_click_reset() {
        let mut state = ConstellationState::new();
        state.samples = (0..4)
            .map(|index| {
                let mut sample = sample_fixture(index);
                sample.x = index as f32 * 0.1;
                sample
            })
            .collect();
        state.selected_path = Some(state.samples[0].file_path.clone());
        let first = state.space_audition(false).unwrap();
        assert_eq!(state.space_audition(false), Some(first.clone()));
        for index in 1..4 {
            assert_eq!(
                state.space_audition(true),
                Some(state.samples[index].file_path.clone())
            );
        }
        assert_eq!(state.space_audition(true), None);
        state.visited_samples.clear(); // Explicit click starts a new exploration.
        state.selected_path = Some(first.clone());
        let sample = state.samples[0].clone();
        state.record_played_sample(&sample);
        state.channel_filter = 2;
        assert_eq!(state.space_audition(true), None, "respect active filters");
        state.channel_filter = 0;
        assert_eq!(
            state.space_audition(true),
            Some(state.samples[1].file_path.clone())
        );
        state.browser_open = true;
        assert_eq!(state.space_audition(false), None);
    }

    #[test]
    fn density_preserves_sparse_targets_and_reduces_dense_marker_size() {
        let point = Pos2::new(100.0, 100.0);
        let sparse = PointDensity::new(std::iter::once(point));
        let dense = PointDensity::new(std::iter::repeat_n(point, 1000));
        assert_eq!(sparse.radius(point, 4.0), 4.0);
        assert!(dense.radius(point, 4.0) < 0.5);
    }

    #[test]
    fn dense_coarse_mesh_keeps_every_point_and_all_filter_matches() {
        let count = 68_055;
        let mut state = ConstellationState::new();
        state.samples = (0..count).map(sample_fixture).collect();
        assert_eq!(
            coarse_mesh(&mut state).len(),
            count * 4,
            "Every point is rendered, including coincident samples"
        );
        state.category_filter = Some(SampleCategory::Snare);
        let solo = coarse_mesh(&mut state);
        assert_eq!(
            solo.len(),
            count * 4,
            "filtering dims points without omitting them"
        );
        assert!(
            solo.iter()
                .any(|vertex| vertex.color == SampleCategory::Snare.color())
        );
        state.category_filter = None;
        state.search_query = "target".into();
        let search = coarse_mesh(&mut state);
        assert_eq!(search.len(), count * 4);
        assert!(
            search
                .iter()
                .any(|vertex| vertex.color == SampleCategory::Snare.color())
        );
        let sample = state.samples[999].clone();
        state.min_duration = 0.6;
        assert!(!state.matches_filters(&sample, "target"));
        state.min_duration = 0.0;
        state.max_duration = 0.25;
        assert!(!state.matches_filters(&sample, "target"));
        state.max_duration = 0.0;
        state.channel_filter = 2;
        assert!(!state.matches_filters(&sample, "target"));
        state.channel_filter = 1;
        assert!(state.matches_filters(&sample, "target"));
        state.channel_filter = 0;
        state.samples.truncate(1);
        state.samples[0].x = -2.3;
        state.samples[0].y = 1.5;
        state.search_query.clear();
        let fitted = coarse_mesh(&mut state);
        assert_eq!(
            fitted.len(),
            4,
            "a sample beyond the unit square stays visible"
        );
        assert!(
            fitted
                .iter()
                .all(|vertex| (8.0..792.0).contains(&vertex.pos.x)
                    && (8.0..592.0).contains(&vertex.pos.y))
        );
    }

    fn render_frame(
        ctx: &egui::Context,
        state: &mut ConstellationState,
        palette: &little_clip_core::palette::AppPalette,
        size: Vec2,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(
                        ui,
                        std::path::Path::new("/samples"),
                        palette,
                        &mut |_| {},
                        &mut |_| {},
                    );
                });
            },
        )
    }

    #[test]
    fn docks_do_not_move_visible_stars_at_coarse_or_relaxed_zoom() {
        for zoom in [1.0, 2.5] {
            let ctx = egui::Context::default();
            let palette = little_clip_core::palette::AppPalette::default();
            let mut state = ConstellationState::new();
            state.zoom = zoom;
            let mut sample = sample_fixture(0);
            // Stay in the top-left exposed portion even when both docks are open.
            sample.x = -0.4 / zoom;
            sample.y = 0.4 / zoom;
            state.samples.push(sample);
            state.pack_bank.slots[0]
                .samples
                .push(super::super::sample_pack::SlotSample::new(
                    "/samples/0.wav".into(),
                    "0.wav".into(),
                    None,
                    0.5,
                ));
            let mut expected = None;
            for rack in [true, false, true] {
                state.pack_shelf_open = rack;
                for editor in [false, true, false] {
                    state.pad_editor_open = editor;
                    let output = render_frame(&ctx, &mut state, &palette, Vec2::new(1000.0, 800.0));
                    let points: Vec<_> = output
                        .shapes
                        .into_iter()
                        .filter_map(|shape| match shape.shape {
                            egui::Shape::Mesh(mesh) => Some(
                                mesh.vertices
                                    .iter()
                                    .filter(|vertex| vertex.color == SampleCategory::Kick.color())
                                    .map(|vertex| vertex.pos)
                                    .collect::<Vec<_>>(),
                            ),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    assert_eq!(points.len(), 4, "fixture star must remain visible");
                    if let Some(ref expected) = expected {
                        assert_eq!(&points, expected);
                    }
                    expected = Some(points);
                }
            }
        }
    }

    #[test]
    fn rack_layout_does_not_grow_across_frames_or_open_a_pack_window() {
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette = little_clip_core::palette::AppPalette::for_appearance(appearance);
            crate::ui_appearance::apply_visuals(&ctx, &palette);
            let mut state = ConstellationState::new();
            for size in [Vec2::new(840.0, 550.0), Vec2::new(1200.0, 1000.0)] {
                for frame in 0..120 {
                    render_frame(&ctx, &mut state, &palette, size);
                    assert_eq!(state.pad_drop_targets.len(), 16);
                    assert!(
                        state
                            .pad_drop_targets
                            .iter()
                            .all(|(rect, _)| rect.height() >= 27.9),
                        "every pad must remain visible at the compact window size"
                    );
                    if size.y >= 1000.0 {
                        for (rect, _) in &state.pad_drop_targets {
                            assert!(
                                (rect.width() - rect.height()).abs() < 1.0 && rect.height() > 44.0,
                                "pad bounds {rect:?}, frame {frame}, size {size:?}"
                            );
                        }
                    }
                    let panel = egui::containers::panel::PanelState::load(
                        &ctx,
                        egui::Id::new("constellation_pack_shelf_panel"),
                    )
                    .unwrap();
                    assert!(
                        panel.rect.width() <= 361.0,
                        "rack expanded on frame {frame}: {:?}",
                        panel.rect
                    );
                }
            }
            let id = egui::Id::new("constellation_pack_shelf_panel");
            let mut user_panel = egui::containers::panel::PanelState::load(&ctx, id).unwrap();
            user_panel.rect.min.x = user_panel.rect.max.x - 312.0;
            ctx.data_mut(|data| data.insert_persisted(id, user_panel));
            for _ in 0..30 {
                render_frame(&ctx, &mut state, &palette, Vec2::new(1200.0, 1000.0));
                assert!(
                    (egui::containers::panel::PanelState::load(&ctx, id)
                        .unwrap()
                        .rect
                        .width()
                        - 312.0)
                        .abs()
                        < 1.0
                );
            }
            state.pack_shelf_open = false;
            render_frame(&ctx, &mut state, &palette, Vec2::new(1200.0, 1000.0));
            state.pack_shelf_open = true;
            render_frame(&ctx, &mut state, &palette, Vec2::new(1200.0, 1000.0));
            assert!(
                (egui::containers::panel::PanelState::load(&ctx, id)
                    .unwrap()
                    .rect
                    .width()
                    - 312.0)
                    .abs()
                    < 1.0
            );
            state.browser_open = true;
            render_frame(&ctx, &mut state, &palette, Vec2::new(840.0, 550.0));
            state.browser_open = false;
            state.output_editor_open = true;
            render_frame(&ctx, &mut state, &palette, Vec2::new(840.0, 550.0));
            let panel = egui::containers::panel::PanelState::load(
                &ctx,
                egui::Id::new("constellation_pack_shelf_panel"),
            )
            .unwrap();
            assert!(panel.rect.width() <= 361.0 && panel.rect.bottom() <= 550.0);
            assert!(!ctx.memory(|memory| {
                memory
                    .areas()
                    .visible_layer_ids()
                    .iter()
                    .any(|layer| layer.order == egui::Order::Middle)
            }));
        }
    }

    #[test]
    fn pad_assignment_uses_configured_keys_and_only_text_focus_blocks_it() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::new();
        let sample = ConstellationSample {
            file_path: "/samples/kick.wav".into(),
            folder_path: "/samples".into(),
            filename: "kick.wav".into(),
            category: SampleCategory::Kick,
            confidence: 1.0,
            duration_seconds: 0.5,
            pitch_hz: 60.0,
            pitch_clarity: 0.0,
            brightness: 0.0,
            x: 0.0,
            y: 0.0,
            file_size: 100,
            mtime: 0,
            sample_rate: 44100,
            channels: 1,
            bit_depth: 16,
            source_revision: None,
        };
        state.selected_path = Some(sample.file_path.clone());
        state.samples.push(sample);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.button("Canvas control").request_focus();
            });
        });
        assert!(ctx.wants_keyboard_input());
        let input = || egui::RawInput {
            events: vec![
                egui::Event::Key {
                    // Some layouts map Shift+digit to a different supported logical key.
                    key: egui::Key::Plus,
                    physical_key: Some(egui::Key::Num1),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                },
                egui::Event::Key {
                    // Virtual keyboards can assign digits to arbitrary physical codes.
                    key: egui::Key::Num2,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| {
            state.handle_shelf_keyboard_shortcuts(ctx);
            assert!(ctx.input(|input| input.events.is_empty()));
        });
        assert_eq!(state.pack_bank.slots[12].samples.len(), 1);
        assert_eq!(state.pack_bank.slots[13].samples.len(), 1);
        let mut text = String::new();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.text_edit_singleline(&mut text).request_focus();
            });
        });
        let _ = ctx.run(input(), |ctx| {
            state.handle_shelf_keyboard_shortcuts(ctx);
            assert!(!ctx.input(|input| input.events.is_empty()));
        });
        assert_eq!(state.pack_bank.slots[12].samples.len(), 1);
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
