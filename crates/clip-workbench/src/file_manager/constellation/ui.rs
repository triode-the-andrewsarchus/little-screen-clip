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
    pub(super) fn matches_filters(
        &self,
        sample: &ConstellationSample,
        lowercase_query: &str,
    ) -> bool {
        let index = self.sample_sets.index_for_path(&sample.file_path);
        self.expanded_set
            .is_none_or(|set| index.and_then(|index| self.sample_sets.set_for(index)) == Some(set))
            && (!self.multisamples_only
                || index
                    .and_then(|index| self.sample_sets.set_for(index))
                    .is_some())
            && self.pitch_range.is_none_or(|(low, high)| {
                index
                    .and_then(|index| self.sample_sets.note(index))
                    .is_some_and(|note| note >= low - 0.5 && note <= high + 0.5)
            })
            && self
                .category_filter
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

    fn render_indexing_progress(&mut self, ui: &mut egui::Ui) {
        let Some(status) = self.indexing_status.clone() else {
            return;
        };
        ui.separator();
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
            if outlined_toggle(ui, "Hover audition", self.hover_to_play, ui.visuals().hyperlink_color).on_hover_text("Play samples as the pointer passes over them").clicked() {
                self.hover_to_play = !self.hover_to_play;
            }
            if outlined_toggle(ui, "Pitch height", self.pitch_height_enabled, ui.visuals().hyperlink_color)
                .on_hover_text("Height: pitch relative to C4. Dotted stems meet the base plane. Unknown pitch stays on the plane. Point size: duration.")
                .clicked() {
                self.pitch_height_enabled = !self.pitch_height_enabled;
            }
            self.render_camera_controls(ui);
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
        self.render_set_navigation(ui);
    }

    fn render_set_navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.expanded_set.is_some() {
                if ui.button("Back to Map").clicked() {
                    self.leave_set();
                }
                return;
            }
            if outlined_toggle(
                ui,
                format!("All ({})", self.samples.len()),
                self.category_filter.is_none(),
                ui.visuals().hyperlink_color,
            )
            .clicked()
            {
                self.category_filter = None;
            }
            ui.toggle_value(&mut self.multisamples_only, "Multisamples only");
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
                if outlined_toggle(
                    ui,
                    egui::RichText::new(format!("{} ({count})", category.display_name()))
                        .color(category.color()),
                    active,
                    category.color(),
                )
                .clicked()
                {
                    self.category_filter = if active { None } else { Some(category) };
                }
            }
            if let Some(set) = self.selected_set() {
                if ui
                    .button("Explore set")
                    .on_hover_text(
                        "Open pitch rings; cross rings to audition from lowest to highest",
                    )
                    .clicked()
                {
                    self.explore_set(set);
                }
            }
        });
    }

    fn render_camera_controls(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Camera", |ui| {
            ui.label("Right-drag: orbit · Middle-drag: pan · Wheel: zoom");
            if outlined_toggle(
                ui,
                "Left-drag to orbit",
                self.orbit_mode,
                ui.visuals().hyperlink_color,
            )
            .clicked()
            {
                self.orbit_mode = !self.orbit_mode;
            }
            ui.add(egui::Slider::new(&mut self.camera.azimuth, 0.0..=360.0).text("Azimuth"));
            ui.add(egui::Slider::new(&mut self.camera.elevation, 5.0..=90.0).text("Elevation"));
            ui.horizontal(|ui| {
                if ui.button("Top view").clicked() {
                    self.camera.elevation = 90.0;
                }
                if ui.button("Oblique view").clicked() {
                    self.camera = Default::default();
                }
                if ui.button("Reset view").clicked() {
                    self.camera = Default::default();
                    self.pan = Vec2::ZERO;
                    self.zoom = 1.0;
                    self.pending_set_focus = self.expanded_set;
                }
            });
        });
    }

    pub(crate) fn sample_count_label(&self) -> String {
        let query = self.search_query.to_lowercase();
        let active = self
            .samples
            .iter()
            .filter(|sample| self.matches_filters(sample, &query))
            .count();
        let mut label = format!("{active} / {} samples", self.samples.len());
        if let Some(set) = self.selected_set() {
            let set = &self.sample_sets.sets[set];
            label.push_str(&format!(" · {} · {} notes", set.label, set.members.len()));
        }
        label
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
        // Keep the selection representable when a different folder changes the bounds.
        self.min_duration = if low <= shortest { 0.0 } else { low };
        self.max_duration = if high >= longest { 0.0 } else { high };
        ui.label("Duration");
        let mut fractions = [
            duration_fraction(low, shortest, longest),
            duration_fraction(high, shortest, longest),
        ];
        let response = range_handles(ui, &mut fractions);
        if response.changed() {
            low = duration_at(fractions[0], shortest, longest);
            high = duration_at(fractions[1], shortest, longest);
            self.min_duration = if low <= shortest { 0.0 } else { low };
            self.max_duration = if high >= longest { 0.0 } else { high };
        }
        if response.double_clicked() {
            self.min_duration = 0.0;
            self.max_duration = 0.0;
        }
        response.on_hover_text(format!(
            "{low:.3}–{high:.3} s · exponential scale · drag either end · double-click to reset"
        ));
        ui.weak(format!("{low:.3}–{high:.3} s"));
    }

    fn render_pitch_range(&mut self, ui: &mut egui::Ui) {
        let (lowest, highest) = self
            .sample_sets
            .notes
            .iter()
            .flatten()
            .fold((127.0_f32, 0.0_f32), |(low, high), note| {
                (low.min(*note), high.max(*note))
            });
        let (lowest, highest) = if lowest <= highest {
            (lowest, highest)
        } else {
            (0.0, 127.0)
        };
        let (mut low, mut high) = self.pitch_range.unwrap_or((lowest, highest));
        low = low.clamp(lowest, highest);
        high = high.clamp(low, highest);
        let span = (highest - lowest).max(1.0);
        let mut fractions = [(low - lowest) / span, (high - lowest) / span];
        ui.label("Pitch");
        let response = range_handles(ui, &mut fractions);
        if response.changed() || response.clicked() {
            low = (lowest + fractions[0] * span)
                .round()
                .clamp(lowest, highest);
            high = (lowest + fractions[1] * span).round().clamp(low, highest);
            self.pitch_range = Some((low, high));
        }
        if response.double_clicked() {
            self.pitch_range = None;
        }
        response.on_hover_text(format!("{}–{} · {:.0}–{:.0} Hz · drag either end to show measured/named pitches only · double-click for all, including unknown pitch", midi_note_str(low), midi_note_str(high), note_to_hz(low), note_to_hz(high)));
        ui.weak(if self.pitch_range.is_none() {
            "All pitches".to_owned()
        } else {
            format!("{}–{}", midi_note_str(low), midi_note_str(high))
        });
    }

    fn render_audio_filters(&mut self, ui: &mut egui::Ui) {
        self.render_duration_range(ui);
        self.render_pitch_range(ui);
        if outlined_toggle(
            ui,
            "Mono samples only",
            self.channel_filter == 1,
            ui.visuals().hyperlink_color,
        )
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
                        (self.matches_filters(sample, query_lower)
                            && distance <= (point.radius + 7.0).max(12.0).powi(2))
                        .then_some((sample, distance))
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
        let pan_id = response.id.with("suppress_hover_after_pan");
        let mut suppress_hover = ui
            .data(|data| data.get_temp::<bool>(pan_id))
            .unwrap_or(false);
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
            suppress_hover = true;
        } else if moved && !camera_released {
            suppress_hover = false;
        }
        ui.data_mut(|data| data.insert_temp(pan_id, suppress_hover));
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
            .filter(|_| !response.dragged() && !suppress_hover)
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

        let mut hovered_sample: Option<ConstellationSample> = None;
        let mut hovered_pos = Pos2::ZERO;
        let mut hovered_radius = 0.0;

        let mouse_pos = self.handle_camera_pointer(ui, &response, viewport);

        // Allocate a single batched GPU mesh
        let dot = dot_texture(ui.ctx());
        let mut star_mesh = egui::Mesh::with_texture(dot.id());
        let mut context_mesh = egui::Mesh::with_texture(dot.id());
        let mut stem_mesh = egui::Mesh::with_texture(dot.id());
        let base_star_radius = map_point_radius(viewport, self.samples.len(), self.zoom);
        let (view_scale, center, points) =
            self.layout_map_points(viewport, rect, base_star_radius, &query_lower);
        // Every interaction uses the exact same projected positions as painting.
        let mut position_map = std::collections::HashMap::with_capacity(self.samples.len());
        for point in &points {
            if !self.matches_filters(&self.samples[point.index], &query_lower) {
                continue;
            }
            let pos = point.position + center.to_vec2();
            position_map.insert(self.samples[point.index].file_path.as_path(), pos);
            if let Some(set) = point.collapsed_set {
                for &index in &self.sample_sets.sets[set].members {
                    position_map.insert(self.samples[index].file_path.as_path(), pos);
                }
            }
        }
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
                "Pitch rings: lowest at center · cross rings to audition · Right-drag: orbit"
            } else if self.pitch_height_enabled {
                "Height: pitch · C4 reference (unknown stays on plane) / Size: duration · Right-drag: orbit"
            } else {
                "Flat map   /   Size: duration"
            },
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
        let edge_bounds = rect.intersect(ui.clip_rect());

        let mut selected_sample_overlay: Option<(Pos2, Color32, f32)> = None;

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

        // One rendering and hit-testing path at every zoom level.
        let rect_min_x = rect.left() - 12.0;
        let rect_max_x = rect.right() + 12.0;
        let rect_min_y = rect.top() - 12.0;
        let rect_max_y = rect.bottom() + 12.0;

        let selected_set = self.selected_set();
        let mut hovered_set = None;
        for point in &points {
            let sample = &self.samples[point.index];
            let pos = point.position + center.to_vec2();
            let base = point.base + center.to_vec2();
            let (sx, sy) = (pos.x, pos.y);
            let radius = point.radius;
            let matches = self.matches_filters(sample, &query_lower);
            let base_color = sample.category.color();
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
                selected_sample_overlay = Some((Pos2::new(sx, sy), base_color, radius));
            }

            // Check hover directly against screen position (only non-dimmed samples participate in hover)
            if opacity > 0.0 && self.matches_filters(sample, &query_lower) {
                if let Some(mp) = mouse_pos {
                    let dx = sx - mp.x;
                    let dy = sy - mp.y;
                    let d2 = dx * dx + dy * dy;
                    if d2 < closest_hover_dist_sq {
                        closest_hover_dist_sq = d2;
                        hovered_sample = Some(sample.clone());
                        hovered_pos = Pos2::new(sx, sy);
                        hovered_set = point.collapsed_set;
                        hovered_radius = radius;
                    }
                }
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

        if let Some(index) =
            mouse_pos.and_then(|pointer| self.ring_sample_at(pointer, center, view_scale))
        {
            let sample = &self.samples[index];
            if self.matches_filters(sample, &query_lower) {
                hovered_sample = Some(sample.clone());
                hovered_set = None;
                if let Some(point) = points.iter().find(|point| point.index == index) {
                    hovered_pos = point.position + center.to_vec2();
                    hovered_radius = point.radius;
                }
            } else {
                hovered_sample = None;
            }
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

        // Render playback history trail (fades over steps and distance)
        self.render_playback_trail(&painter, center, view_scale, Some(&position_map));

        // Upload and render all stars to the GPU in a single draw call!
        if !star_mesh.is_empty() {
            painter.add(egui::Shape::mesh(star_mesh));
        }

        // Render selected sample prominent overlay
        if let Some((pos, color, radius)) = selected_sample_overlay {
            let r = radius + 2.0;
            let opacity = circle_edge_opacity(edge_bounds, pos, r + 8.0);
            painter.circle_filled(
                pos,
                r + 8.0,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 80)
                    .gamma_multiply(opacity),
            );
            painter.circle_filled(pos, r, color.gamma_multiply(opacity));
            painter.circle_filled(pos, r * 0.45, Color32::WHITE.gamma_multiply(opacity));
        }

        // Render hovered sample prominent overlay
        if let Some(ref hovered) = hovered_sample {
            let r = hovered_radius + 1.0;
            let color = hovered.category.color();
            let opacity = circle_edge_opacity(edge_bounds, hovered_pos, r + 7.0);
            painter.circle_filled(
                hovered_pos,
                r + 7.0,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 60)
                    .gamma_multiply(opacity),
            );
            painter.circle_filled(hovered_pos, r, color.gamma_multiply(opacity));
            painter.circle_filled(
                hovered_pos,
                r * 0.45,
                Color32::WHITE.gamma_multiply(opacity),
            );
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

        let pitch_str = sample_pitch_label(sample);

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
        let active_slice: Vec<&super::HistoryTrailPoint> = self
            .history_trail
            .iter()
            .skip(start_idx)
            .filter(|point| {
                relaxed_positions.is_none_or(|map| map.contains_key(point.file_path.as_path()))
            })
            .collect();
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

/// Two normalized endpoints shared by the duration and semitone pitch ranges.
fn range_handles(ui: &mut egui::Ui, fractions: &mut [f32; 2]) -> egui::Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(160.0, 24.0), Sense::click_and_drag());
    let track = rect.shrink2(Vec2::new(8.0, 0.0));
    let x_at = |fraction: f32| track.left() + fraction * track.width();
    let handle_id = response.id.with("range_endpoint");
    if response.drag_started() || response.clicked() {
        let x = ui
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos())
            .map_or(x_at(fractions[0]), |pos| pos.x);
        ui.data_mut(|data| {
            data.insert_temp(
                handle_id,
                (x - x_at(fractions[0])).abs() <= (x - x_at(fractions[1])).abs(),
            )
        });
    }
    if (response.dragged() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let value = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
        if ui
            .data(|data| data.get_temp::<bool>(handle_id))
            .unwrap_or(true)
        {
            fractions[0] = value.min(fractions[1]);
        } else {
            fractions[1] = value.max(fractions[0]);
        }
        response.mark_changed();
    }
    let y = track.center().y;
    let painter = ui.painter_at(rect);
    painter.line_segment(
        [egui::pos2(track.left(), y), egui::pos2(track.right(), y)],
        Stroke::new(3.0_f32, ui.visuals().widgets.inactive.bg_fill),
    );
    painter.line_segment(
        [
            egui::pos2(x_at(fractions[0]), y),
            egui::pos2(x_at(fractions[1]), y),
        ],
        Stroke::new(3.0_f32, ui.visuals().selection.bg_fill),
    );
    for fraction in *fractions {
        painter.circle(
            egui::pos2(x_at(fraction), y),
            6.0,
            ui.visuals().widgets.inactive.bg_fill,
            ui.visuals().widgets.inactive.fg_stroke,
        );
    }
    response
}

fn note_to_hz(note: f32) -> f32 {
    440.0 * 2.0_f32.powf((note - 69.0) / 12.0)
}

/// A 1 ms offset keeps zero-length bounds finite while distributing decades evenly.
fn duration_fraction(time: f32, low: f32, high: f32) -> f32 {
    let span = ((high + 0.001) / (low + 0.001)).ln();
    if span <= f32::EPSILON {
        return 0.0;
    }
    (((time + 0.001) / (low + 0.001)).ln() / span).clamp(0.0, 1.0)
}

fn duration_at(fraction: f32, low: f32, high: f32) -> f32 {
    if fraction <= 0.0 {
        return low;
    }
    if fraction >= 1.0 {
        return high;
    }
    ((low + 0.001) * ((high + 0.001) / (low + 0.001)).powf(fraction) - 0.001).clamp(low, high)
}

fn sample_pitch_label(sample: &ConstellationSample) -> String {
    if let Some((_, note)) = super::multisample::filename_note(&sample.file_path) {
        format!(" · {} (filename)", midi_note_str(note as f32))
    } else if let Some(note) = super::multisample::acoustic_note(sample) {
        format!(" · {}", midi_note_str(note))
    } else {
        " · pitch unknown".into()
    }
}

pub(super) fn freq_to_note_str(freq: f32) -> String {
    midi_note_str(69.0 + 12.0 * (freq / 440.0).log2())
}

fn midi_note_str(midi: f32) -> String {
    let note_names = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let rounded = midi.round() as i32;
    let note_idx = rounded.rem_euclid(12) as usize;
    let octave = rounded.div_euclid(12) - 1;
    format!("{}{}", note_names[note_idx], octave)
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
mod tests {
    use super::*;

    fn render_map_frame(
        ctx: &egui::Context,
        state: &mut ConstellationState,
        events: Vec<egui::Event>,
        play: &mut dyn FnMut(&std::path::Path),
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_starfield(ui, std::path::Path::new("/samples"), play, &mut |_| {});
                });
            },
        )
    }

    fn coarse_mesh(state: &mut ConstellationState) -> Vec<egui::epaint::Vertex> {
        let context = egui::Context::default();
        let output = render_map_frame(&context, state, vec![], &mut |_| {});
        output
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                // The shared vignette is an untextured overlay; count the dot batch only.
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.vertices.clone())
                }
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

    fn note_sample(path: &str) -> ConstellationSample {
        let mut sample = sample_fixture(0);
        sample.file_path = path.into();
        sample.filename = sample
            .file_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // The key must use the actual parent, even when an index covers a recursive root.
        sample.folder_path = "/recursive-root".into();
        sample
    }

    #[test]
    fn multisample_families_stay_in_actual_folders_and_preserve_variants() {
        let samples: Vec<_> = [
            "/a/old_piano_G4.wav",
            "/a/old_piano_C4.wav",
            "/a/old_piano_D4.wav",
            "/b/old_piano_C4.wav",
            "/b/old_piano_D4.wav",
            "/a/old_piano_C4_vel80.wav",
            "/a/old_piano_D4_vel90.wav",
            "/a/old_piano_01.wav",
            "/a/old_piano_02.wav",
            "/a/only_C4.wav",
            "/a/only_C4.flac",
        ]
        .into_iter()
        .map(note_sample)
        .collect();
        let sets = super::super::multisample::SampleSets::build(&samples);
        assert_eq!(sets.sets.len(), 2);
        let group = &sets.sets[sets.set_for(0).unwrap()];
        assert_eq!(group.members, vec![1, 2, 0]);
        assert_eq!(
            group.representative, 2,
            "audition the middle recorded pitch"
        );
        assert_ne!(sets.set_for(0), sets.set_for(3));
        for index in 5..samples.len() {
            assert_eq!(sets.set_for(index), None);
        }
        assert_eq!(sets.note(1), Some(60.0));
        assert_eq!(sets.note(7), None, "sequence numbers do not become pitches");
    }

    #[test]
    fn named_pitch_families_filter_peers_and_keep_unrelated_singletons_separate() {
        let mut state = ConstellationState::default();
        state.samples = [
            "/a/bar_D2.wav",
            "/a/foo_C3.wav",
            "/a/C4_piano_vel80.wav",
            "/a/piano_D4_vel80.wav",
            "/a/piano_E4_vel90.wav",
            "/a/DoubleSawsC3.wav",
            "/a/DoubleSawsF#4.wav",
            "/bare/C3.wav",
            "/bare/D3.wav",
            "/a/solo_F2.wav",
            "/a/no_octave_C.wav",
        ]
        .into_iter()
        .map(note_sample)
        .collect();
        for sample in &mut state.samples {
            sample.pitch_hz = 440.0;
            sample.pitch_clarity = 1.0;
        }
        state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
        assert_eq!(state.sample_sets.sets.len(), 3);
        assert_eq!(state.sample_sets.note(0), Some(38.0));
        assert_eq!(state.sample_sets.note(9), Some(41.0));
        assert_eq!(
            state.sample_sets.note(10),
            Some(69.0),
            "no octave keeps measured pitch"
        );
        state.multisamples_only = true;
        let active: Vec<_> = state
            .samples
            .iter()
            .enumerate()
            .filter_map(|(index, sample)| state.matches_filters(sample, "").then_some(index))
            .collect();
        assert_eq!(active, [2, 3, 5, 6, 7, 8]);
        state.selected_path = Some(state.samples[9].file_path.clone());
        assert!(state.selected_set().is_none());
        assert_eq!(state.sample_count_label(), "6 / 11 samples");
        state.selected_path = Some(state.samples[2].file_path.clone());
        assert_eq!(
            state.sample_count_label(),
            "6 / 11 samples · piano_vel80 · 2 notes"
        );
    }

    #[test]
    fn pitch_projection_duration_and_ring_detail_share_stable_geometry() {
        let mut state = ConstellationState::default();
        state.samples = [
            "/a/piano_C3.wav",
            "/a/piano_C4.wav",
            "/a/piano_C5.wav",
            "/a/noise.wav",
        ]
        .into_iter()
        .map(note_sample)
        .collect();
        state.samples[0].duration_seconds = 0.001;
        state.samples[2].duration_seconds = 200.0;
        state.samples[3].pitch_hz = 440.0;
        state.samples[3].pitch_clarity = 0.2;
        state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
        let overview = state.map_points(100.0, 4.0, "");
        assert_eq!(overview.len(), 2);
        let group = overview
            .iter()
            .find(|point| point.collapsed_set.is_some())
            .unwrap();
        assert_eq!(group.index, 1);
        assert_eq!(group.base, group.position, "C4 is the plane");
        let noise = overview.iter().find(|point| point.index == 3).unwrap();
        assert_eq!(noise.base, noise.position, "uncertain pitch stays on plane");
        state.selected_path = Some(state.samples[1].file_path.clone());
        state.expanded_set = state.selected_set();
        state.zoom = 12.0;
        let detail = state.map_points(100.0, 4.0, "");
        assert_eq!(detail.len(), 4);
        let low = detail.iter().find(|point| point.index == 0).unwrap();
        let high = detail.iter().find(|point| point.index == 2).unwrap();
        assert_eq!(
            low.position, low.base,
            "detail rings use the lowest note's local plane"
        );
        assert_eq!(high.position, high.base);
        assert!(low.radius < high.radius);
        assert!(detail.iter().all(|point| point.collapsed_set.is_none()));
        let visible = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 240.0));
        state.pending_set_focus = Some(state.selected_set().unwrap());
        let (_, center, focused) = state.layout_map_points(visible, visible, 8.0, "");
        assert!(
            focused
                .iter()
                .filter(|point| state.sample_sets.set_for(point.index).is_some())
                .all(|point| visible
                    .shrink(20.0)
                    .contains(point.position + center.to_vec2())),
            "Explore must fit every recorded pitch into a short canvas"
        );
        state.pitch_height_enabled = false;
        assert!(
            state
                .map_points(100.0, 4.0, "")
                .iter()
                .all(|point| point.base == point.position)
        );
        state.zoom = 1.0;
        state.expanded_set = None;
        let filtered = state.map_points(100.0, 4.0, "c5");
        assert_eq!(
            filtered
                .iter()
                .find(|point| point.collapsed_set.is_some())
                .unwrap()
                .index,
            2,
            "an explicitly filtered note remains available"
        );
    }

    #[test]
    fn collapsed_hover_auditions_middle_note_and_expanded_hover_picks_individuals() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.samples = ["/a/piano_C3.wav", "/a/piano_C4.wav", "/a/piano_C5.wav"]
            .into_iter()
            .map(note_sample)
            .collect();
        state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
        let mut auditions = Vec::new();
        let mut frame = |state: &mut ConstellationState, pointer: Option<Pos2>| {
            render_map_frame(
                &ctx,
                state,
                pointer.map(egui::Event::PointerMoved).into_iter().collect(),
                &mut |path| auditions.push(path.to_owned()),
            )
        };
        let first = frame(&mut state, None);
        let star = first
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.calc_bounds().center())
                }
                _ => None,
            })
            .unwrap();
        let _ = frame(&mut state, Some(star));
        assert_eq!(auditions, vec![state.samples[1].file_path.clone()]);
        state.selected_path = Some(state.samples[1].file_path.clone());
        state.expanded_set = state.selected_set();
        state.zoom = 12.0;
        state.pending_map_reveal = Some(state.samples[2].file_path.clone());
        state.last_hovered_path = None;
        let _ = render_map_frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(Pos2::new(400.0, 300.0))],
            &mut |path| auditions.push(path.to_owned()),
        );
        assert_eq!(auditions.last(), Some(&state.samples[2].file_path));
    }

    #[test]
    fn explore_centers_lowest_sample_and_crossing_each_ring_auditions_its_recording() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.samples = [
            "/a/piano_C5.wav",
            "/a/piano_C3.wav",
            "/a/piano_C4.wav",
            "/a/piano_C4.flac",
        ]
        .into_iter()
        .map(note_sample)
        .collect();
        state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
        let set = state.sample_sets.set_for(0).unwrap();
        let members = state.sample_sets.sets[set].members.clone();
        state.selected_path = Some(state.samples[members[1]].file_path.clone());
        state.camera.orbit(Vec2::new(75.0, 20.0));
        state.pan = Vec2::new(130.0, -42.0);
        state.zoom = 3.7;
        let before = (
            state.camera,
            state.pan,
            state.zoom,
            state.selected_path.clone(),
        );
        state.explore_set(set);
        assert_eq!(
            state.selected_path.as_ref(),
            Some(&state.samples[members[0]].file_path)
        );
        let initial = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
        let rings = initial
            .shapes
            .iter()
            .filter(
                |shape| matches!(&shape.shape, egui::Shape::Path(path) if path.points.len() == 97),
            )
            .count();
        assert_eq!(
            rings,
            members.len(),
            "duplicate-pitch recordings need separate rings too"
        );
        let viewport = Rect::from_min_max(Pos2::new(8.0, 8.0), Pos2::new(792.0, 592.0));
        let (scale, center, points) = state.layout_map_points(viewport, viewport, 4.0, "");
        let origin = points
            .iter()
            .find(|point| point.index == members[0])
            .unwrap()
            .position
            + center.to_vec2();
        assert!(
            (origin - viewport.center()).length() < 0.01,
            "Explore must center the lowest note"
        );
        let mut played = Vec::new();
        for (ordinal, index) in members.iter().enumerate() {
            let radius = if ordinal == 0 {
                0.0
            } else {
                (ordinal + 1) as f32 * 0.025
            };
            // Pick a circumference far from the dot, to test ring rather than dot hit testing.
            let pointer = origin + state.map_base(Pos2::new(0.0, 1.0)) * radius * scale;
            assert_eq!(state.ring_sample_at(pointer, center, scale), Some(*index));
            let _ = render_map_frame(
                &ctx,
                &mut state,
                vec![egui::Event::PointerMoved(pointer)],
                &mut |path| played.push(path.to_owned()),
            );
        }
        assert_eq!(
            played,
            members
                .iter()
                .map(|index| state.samples[*index].file_path.clone())
                .collect::<Vec<_>>()
        );
        state.selected_path = Some(state.samples[members[2]].file_path.clone());
        state.camera.orbit(Vec2::new(-40.0, -20.0));
        state.leave_set();
        assert!(state.expanded_set.is_none());
        assert!(state.pending_set_focus.is_none());
        assert_eq!(
            (
                state.camera,
                state.pan,
                state.zoom,
                state.selected_path.clone()
            ),
            before
        );
    }

    #[test]
    fn pitch_range_filters_measured_notes_and_keeps_unknowns_out_of_active_ranges() {
        let mut state = ConstellationState::default();
        state.samples = [
            "/a/piano_C3.wav",
            "/a/piano_C4.wav",
            "/a/piano_C5.wav",
            "/a/noise.wav",
            "/a/boundary.wav",
            "/a/new-tone.wav",
        ]
        .into_iter()
        .map(note_sample)
        .collect();
        for index in [4, 5] {
            state.samples[index].pitch_hz = 1000.0;
            state.samples[index].pitch_clarity = 0.99;
        }
        state.samples[5].source_revision =
            Some(super::super::classifier::PITCH_ANALYSIS_REVISION.to_vec());
        state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
        assert_eq!(
            state.sample_sets.note(4),
            None,
            "ignore old search-boundary artifacts immediately"
        );
        assert!(
            state.sample_sets.note(5).is_some(),
            "a newly measured real 1 kHz tone is valid"
        );
        state.pitch_range = Some((60.0, 72.0));
        let matches: Vec<_> = state
            .samples
            .iter()
            .map(|sample| state.matches_filters(sample, ""))
            .collect();
        assert_eq!(matches, vec![false, true, true, false, false, false]);
        state.pitch_range = None;
        assert!(
            state
                .samples
                .iter()
                .all(|sample| state.matches_filters(sample, ""))
        );
    }

    #[test]
    fn assigned_rack_samples_gain_a_map_halo_and_lose_it_on_removal() {
        use super::super::sample_pack::SlotSample;
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.pack_shelf_open = false;
        state.samples.push(sample_fixture(0));
        let circles = |output: egui::FullOutput| {
            output
                .shapes
                .into_iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::Circle(_)))
                .count()
        };
        let before = circles(render_frame(
            &ctx,
            &mut state,
            &Default::default(),
            Vec2::new(800.0, 600.0),
        ));
        state.pack_bank.slots[15].samples.push(SlotSample::new(
            state.samples[0].file_path.clone(),
            "hit.wav".into(),
            None,
            0.5,
        ));
        let assigned = circles(render_frame(
            &ctx,
            &mut state,
            &Default::default(),
            Vec2::new(800.0, 600.0),
        ));
        assert_eq!(assigned, before + 3);
        state.pack_bank.slots[15].samples.clear();
        assert_eq!(
            circles(render_frame(
                &ctx,
                &mut state,
                &Default::default(),
                Vec2::new(800.0, 600.0)
            )),
            before
        );
    }

    #[test]
    fn pad_clicks_cycle_layers_and_reveal_them_without_opening_studio() {
        let mut state = ConstellationState::default();
        state.samples = (0..3).map(sample_fixture).collect();
        state.pack_bank.slots[0].samples = state.samples[..2]
            .iter()
            .map(|sample| {
                super::super::sample_pack::SlotSample::new(
                    sample.file_path.clone(),
                    sample.filename.clone(),
                    None,
                    1.0,
                )
            })
            .collect();
        state.pack_bank.slots[1].samples = vec![state.pack_bank.slots[0].samples[0].clone()];
        state.zoom = 4.0;
        for (pad, layer) in [(0, 0), (0, 1), (1, 0), (0, 0)] {
            state.reveal_pad_layer(pad);
            assert_eq!(state.selected_layers[pad], layer);
            assert_eq!(
                state.selected_path,
                Some(state.samples[layer].file_path.clone())
            );
            coarse_mesh(&mut state);
            assert!(state.pending_map_reveal.is_none());
            assert_eq!(state.zoom, 4.0);
            assert!(state.studio_request.is_none());
        }
        state.pack_bank.slots[0].samples.clear();
        state.reveal_pad_layer(0); // Empty pads cannot divide by zero.
    }

    #[test]
    fn middle_drag_on_a_sample_pans_without_creating_a_file_drag_or_audition() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::new();
        state.samples.push(sample_fixture(0));
        let mut auditions = 0;
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_starfield(
                            ui,
                            std::path::Path::new("/samples"),
                            &mut |_| auditions += 1,
                            &mut |_| {},
                        )
                    });
                },
            )
        };
        frame(vec![]);
        let start = Pos2::new(400.0, 300.0);
        let end = start + Vec2::new(50.0, 40.0);
        frame(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        frame(vec![egui::Event::PointerMoved(end)]);
        frame(vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Middle,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert_eq!(state.pan, Vec2::new(50.0, 40.0));
        assert_eq!(auditions, 0);
        assert!(!egui::DragAndDrop::has_payload_of_type::<
            crate::file_manager::transfers::FileDrag,
        >(&ctx));
    }

    #[test]
    fn exponential_duration_range_gives_short_samples_room_and_preserves_endpoints() {
        let (low, high) = (0.001, 600.0);
        let fraction = duration_fraction(0.1, low, high);
        assert!(
            fraction > 0.3,
            "100 ms should have substantial track space in a ten-minute library"
        );
        assert!(duration_at(0.5, low, high) < 2.0);
        assert_eq!(duration_at(0.0, low, high), low);
        assert_eq!(duration_at(1.0, low, high), high);
        for time in [0.001, 0.01, 0.1, 1.0, 60.0, 600.0] {
            assert!(
                (duration_at(duration_fraction(time, low, high), low, high) - time).abs() < 0.001
            );
        }
        assert_eq!(duration_at(0.5, 0.0, 0.0), 0.0);
        assert_eq!(duration_fraction(0.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn selecting_a_folded_set_keeps_its_dot_and_canvas_fixed_at_high_zoom() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.pack_shelf_open = false;
        state.samples = ["/a/piano_C3.wav", "/a/piano_C4.wav", "/a/piano_C5.wav"]
            .into_iter()
            .map(note_sample)
            .collect();
        for sample in &mut state.samples {
            sample.x = 0.1;
            sample.y = 0.1;
        }
        state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
        state.zoom = 3.0;
        let mut selections = 0;
        let mut frame = |state: &mut ConstellationState, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(
                            ui,
                            std::path::Path::new("/a"),
                            &Default::default(),
                            &mut |_| {},
                            &mut |_| selections += 1,
                        );
                    });
                },
            )
        };
        let center_of = |output: egui::FullOutput| {
            output
                .shapes
                .into_iter()
                .find_map(|shape| match shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                        Some(mesh.calc_bounds().center())
                    }
                    _ => None,
                })
                .unwrap()
        };
        let _ = frame(&mut state, vec![]);
        let before = center_of(frame(&mut state, vec![]));
        for _ in 0..2 {
            for pressed in [true, false] {
                let _ = frame(
                    &mut state,
                    vec![
                        egui::Event::PointerMoved(before),
                        egui::Event::PointerButton {
                            pos: before,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            let after = center_of(frame(&mut state, vec![]));
            assert!(
                (after - before).length() < 0.01,
                "selection must not move the set or resize the map"
            );
            assert!(
                state.expanded_set.is_none(),
                "only Explore set should expand rings"
            );
        }
        assert_eq!(selections, 2);
    }

    #[test]
    fn right_drag_orbits_without_panning_auditioning_or_starting_a_file_drag() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.samples.push(sample_fixture(0));
        let original = state.camera;
        let start = Pos2::new(400.0, 300.0);
        let end = start + Vec2::new(80.0, 40.0);
        let mut auditions = 0;
        let _ = render_map_frame(&ctx, &mut state, vec![], &mut |_| auditions += 1);
        for (pos, pressed) in [(start, Some(true)), (end, None), (end, Some(false))] {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let _ = render_map_frame(&ctx, &mut state, events, &mut |_| auditions += 1);
        }
        assert_ne!(state.camera, original);
        assert_eq!(state.pan, Vec2::ZERO);
        assert_eq!(auditions, 0);
        assert!(!egui::DragAndDrop::has_payload_of_type::<
            crate::file_manager::transfers::FileDrag,
        >(&ctx));
    }

    #[test]
    fn ordinary_selection_and_collection_have_no_set_marker_rings() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.pack_shelf_open = false;
        state.samples.push(sample_fixture(0));
        state.selected_path = Some(state.samples[0].file_path.clone());
        let output = render_map_frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(Pos2::new(400.0, 300.0))],
            &mut |_| {},
        );
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Circle(circle) if circle.stroke.width > 0.0)));
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
            (handles[0], 0.75, 1.414, 0.0),
            (handles[1], 0.9, 1.414, 1.741),
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
    fn rendered_edge_dots_keep_one_alpha_at_all_four_vertices() {
        let ctx = egui::Context::default();
        let palette = little_clip_core::palette::AppPalette::default();
        let mut state = ConstellationState::new();
        state.pack_shelf_open = false;
        state.samples.push(sample_fixture(0));
        let texture = dot_texture(&ctx).id();
        let inspect = |output: egui::FullOutput| {
            output
                .shapes
                .into_iter()
                .find_map(|shape| match shape.shape {
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some((
                        shape.clip_rect,
                        mesh.calc_bounds(),
                        mesh.vertices.iter().map(|v| v.color).collect::<Vec<_>>(),
                    )),
                    _ => None,
                })
                .unwrap()
        };
        let size = Vec2::new(800.0, 600.0);
        let output = render_frame(&ctx, &mut state, &palette, size);
        let dividers: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match shape.shape {
                egui::Shape::LineSegment { points, .. }
                    if points[0].y == points[1].y && points[1].x - points[0].x > 500.0 =>
                {
                    Some(points[0].y)
                }
                _ => None,
            })
            .collect();
        let (clip, dot, _) = inspect(output);
        assert!(
            dividers.iter().any(|y| (y - clip.top()).abs() < 0.5),
            "The header divider must meet the dot canvas without an extra top gap"
        );
        let target = Pos2::new(clip.left() + dot.width() * 0.5 + 3.0, clip.center().y);
        state.pan += target - dot.center();
        let (_, _, colors) = inspect(render_frame(&ctx, &mut state, &palette, size));
        assert_eq!(colors.len(), 4);
        assert!(colors.iter().all(|color| *color == colors[0]));
        assert!(colors[0].a() > 0 && colors[0].a() < 240);
        state.pan.x -= 3.0;
        state.selected_path = Some(state.samples[0].file_path.clone());
        let (_, _, colors) = inspect(render_frame(&ctx, &mut state, &palette, size));
        assert_eq!(
            colors.len(),
            4,
            "Selection must retain its ordinary dot beneath the highlight"
        );
        assert!(colors.iter().all(|color| color.a() == 0));
    }

    #[test]
    fn circle_edges_fade_uniformly_and_disappear_before_being_cut_off() {
        let bounds = Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0));
        for radius in [0.35, 4.0, 8.0, 25.5] {
            for (clearance, expected) in
                [(-1.0, 0.0), (0.0, 0.0), (3.0, 0.5), (6.0, 1.0), (30.0, 1.0)]
            {
                let distance = radius + clearance;
                for point in [
                    Pos2::new(distance, 100.0),
                    Pos2::new(200.0 - distance, 100.0),
                    Pos2::new(100.0, distance),
                    Pos2::new(100.0, 200.0 - distance),
                ] {
                    assert!((circle_edge_opacity(bounds, point, radius) - expected).abs() < 0.0001);
                }
            }
        }
        assert_eq!(circle_edge_opacity(bounds, Pos2::new(12.0, 4.0), 4.0), 0.0);
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
    fn overlap_cache_invalidates_on_coordinate_or_spacing_changes() {
        let mut layout = OverlapLayout::default();
        let points = vec![Pos2::ZERO; 2];
        let first = layout.positions(points.clone(), 10.0).to_vec();
        assert_eq!(layout.positions(points.clone(), 10.0), first);
        assert_ne!(layout.positions(points, 20.0), first);
        let changed = vec![Pos2::new(100.0, 100.0), Pos2::new(200.0, 200.0)];
        assert_eq!(layout.positions(changed.clone(), 20.0), changed);
    }

    #[test]
    fn high_zoom_overlap_placement_moves_only_by_the_pan_delta() {
        let mut state = ConstellationState::new();
        state.zoom = 50.0;
        state.samples = (0..30)
            .map(|index| {
                let mut sample = sample_fixture(index);
                sample.x = (index % 5) as f32 * 0.0008;
                sample.y = (index / 5) as f32 * 0.0008;
                sample
            })
            .collect();
        let original = coarse_mesh(&mut state);
        assert_eq!(original.len(), 120);
        for pan in [Vec2::new(3.25, 5.75), Vec2::new(-16.0, 10.0), Vec2::ZERO] {
            state.pan = pan;
            let translated = coarse_mesh(&mut state);
            assert_eq!(translated.len(), original.len());
            for (before, after) in original.iter().zip(translated) {
                assert!(
                    (after.pos - before.pos - pan).length() < 0.01,
                    "Panning must not recompute a different overlap offset"
                );
            }
        }
    }

    #[test]
    fn spacing_only_runs_at_extreme_zoom_and_is_bounded_for_coincident_points() {
        for zoom in [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 28.0] {
            assert_eq!(spacing_strength(zoom), 0.0);
        }
        assert_eq!(spacing_strength(50.0), 1.0);
        let point = Pos2::new(100.0, 100.0);
        let mut points = vec![point; 68_055];
        relax_star_positions(&mut points, 18.0);
        assert_eq!(points[0], point);
        for first in &points {
            assert!((*first - point).length() <= 64.001);
        }
        for other in &points[1..4] {
            assert!((*other - point).length() >= 18.0);
        }
        let mut repeat = vec![point; points.len()];
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
    fn all_points_share_one_radius_at_every_zoom_in_sparse_and_dense_libraries() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        assert_eq!(map_point_radius(viewport, 100, 1.0), 4.0);
        assert!(map_point_radius(viewport, 68_055, 1.0) < 1.0);
        for count in [100, 68_055] {
            let mut state = ConstellationState::new();
            state.samples = (0..count).map(sample_fixture).collect();
            // A sparse outlier must have precisely the same size as the dense cluster.
            state.samples[0].x = 0.001;
            for zoom in [0.5, 1.0, 2.0, 5.0, 20.0, 50.0] {
                state.zoom = zoom;
                let mesh = coarse_mesh(&mut state);
                assert_eq!(mesh.len(), count * 4);
                let radii: Vec<_> = mesh
                    .chunks_exact(4)
                    .map(|vertices| vertices[1].pos.x - vertices[0].pos.x)
                    .collect();
                assert!(radii[0] > 0.0);
                assert!(
                    radii
                        .iter()
                        .all(|radius| (*radius - radii[0]).abs() < 0.0001)
                );
            }
        }
    }

    #[test]
    fn category_and_duration_exclusions_render_only_faint_soft_density() {
        use super::super::sample_pack::SlotSample;
        let ctx = egui::Context::default();
        let mut state = ConstellationState::default();
        state.samples = (0..3).map(sample_fixture).collect();
        for (index, sample) in state.samples.iter_mut().enumerate() {
            sample.x = (index as f32 - 1.0) * 0.4;
        }
        state.samples[1].category = SampleCategory::Snare;
        state.samples[1].duration_seconds = 2.0;
        state.samples[2].category = SampleCategory::Snare;
        state.category_filter = Some(SampleCategory::Snare);
        state.max_duration = 0.5;
        state.selected_path = Some(state.samples[0].file_path.clone());
        state.pack_bank.slots[0].samples.push(SlotSample::new(
            state.samples[0].file_path.clone(),
            "excluded.wav".into(),
            None,
            0.5,
        ));
        for sample in state.samples[..2].to_vec() {
            state.record_played_sample(&sample);
        }
        let output = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
        let meshes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh)
                }
                _ => None,
            })
            .collect();
        let soft = meshes
            .iter()
            .find(|mesh| mesh.vertices.iter().all(|vertex| vertex.uv.x >= 0.5))
            .unwrap();
        assert_eq!(
            soft.vertices.len(),
            8,
            "both category and duration exclusions use the soft texture"
        );
        assert!(soft.vertices.iter().all(|vertex| vertex.color.a() <= 3));
        assert!(
            soft.vertices.iter().all(|vertex| {
                let color = vertex.color;
                color.r().max(color.g()).max(color.b()) <= color.a()
            }),
            "context tint must remain premultiplied for gamma-space blending"
        );
        let sharp = meshes
            .iter()
            .find(|mesh| mesh.vertices.iter().any(|vertex| vertex.uv.x < 0.5))
            .unwrap();
        assert_eq!(sharp.vertices.len(), 4);
        assert!(sharp.vertices.iter().all(|vertex| vertex.color.a() == 255));
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::Shape::Circle(_))),
            "excluded selection, collection and history must not produce bright markers"
        );
        let atlas = output
            .textures_delta
            .set
            .iter()
            .find_map(|(_, delta)| match &delta.image {
                egui::ImageData::Color(image) if image.size == [64, 32] => Some(image),
                _ => None,
            })
            .unwrap();
        assert!(atlas[(21, 15)].a() > 240);
        assert!(
            atlas[(53, 15)].a() < 180,
            "the context profile must fall off well inside the disk"
        );
        assert!(atlas[(60, 15)].a() < 20);
        assert_eq!(atlas[(63, 15)].a(), 0);
        assert!(
            (32..64).all(|x| (0..32).all(|y| atlas[(x, y)].r() == atlas[(x, y)].a())),
            "soft texture must not brighten overlapping samples on a light canvas"
        );
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
    fn mixer_controls_and_filename_tabs_remain_visible_at_every_editor_width() {
        use super::super::sample_pack::SlotSample;
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            ctx.style_mut(|style| style.animation_time = 0.0);
            let palette = little_clip_core::palette::AppPalette::for_appearance(appearance);
            crate::ui_appearance::apply_visuals(&ctx, &palette);
            let mut state = ConstellationState::new();
            state.samples.push(sample_fixture(0));
            state.pad_editor_open = true;
            state.pack_bank.slots[0].choke_group = 1;
            state.pack_bank.slots[1].choke_group = 1;
            state.pack_bank.slots[0].choke_priority = 2;
            state.pack_bank.slots[0].samples.push(SlotSample::new(
                "/samples/kick.wav".into(),
                "kick.wav".into(),
                None,
                1.0,
            ));
            for size in [
                Vec2::new(840.0, 650.0),
                Vec2::new(1400.0, 1000.0),
                Vec2::new(2200.0, 1300.0),
                Vec2::new(3200.0, 1800.0),
            ] {
                render_frame(&ctx, &mut state, &palette, size);
                let output = render_frame(&ctx, &mut state, &palette, size);

                let editor = egui::containers::panel::PanelState::load(
                    &ctx,
                    egui::Id::new("constellation_selected_pad"),
                )
                .unwrap()
                .rect;
                assert!(
                    !output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Rect(rect) if rect.rect.width() <= 12.0
                        && rect.rect.height() > 40.0
                        && rect.rect.right() > editor.right() - 24.0
                        && editor.contains_rect(rect.rect))),
                    "The default editor should not need a vertical scrollbar at {size:?}"
                );
                for wanted in [
                    "kick.wav",
                    "Gain",
                    "Pan",
                    "Pitch",
                    "Speed",
                    "Retrigger",
                    "+1.00×",
                    "+0 oct",
                    "A",
                    "R",
                    "Edit in Audio Studio",
                ] {
                    assert!(
                        output.shapes.iter().any(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == wanted => {
                                shape.clip_rect.contains_rect(
                                    Rect::from_min_size(text.pos, text.galley.size()).shrink(0.5),
                                )
                            }
                            _ => false,
                        }),
                        "{wanted} is clipped at {size:?} in {appearance:?}"
                    );
                }
                let center = |label: &str| {
                    output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == label => {
                                Some(text.pos + text.galley.size() * 0.5)
                            }
                            _ => None,
                        })
                        .unwrap()
                };
                for label in ["+1.00×", "+0 oct"] {
                    let point = center(label);
                    let readout = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if rect.rect.contains(point) && rect.rect.width() < 100.0 =>
                            {
                                Some((shape.clip_rect, rect))
                            }
                            _ => None,
                        })
                        .unwrap();
                    assert!(
                        readout
                            .0
                            .contains_rect(readout.1.rect.expand(readout.1.stroke.width * 0.5)),
                        "{label} border clipped at {size:?}: {readout:?}"
                    );
                }
                let pitch_readout = center("+0 oct");
                let studio = center("Edit in Audio Studio");
                assert!(
                    studio.y > pitch_readout.y + 10.0 || studio.x > pitch_readout.x + 95.0,
                    "Studio must follow the complete pitch readout without overlap: {studio:?}, {pitch_readout:?}"
                );
                assert!(center("Pan").y + 2.0 >= center("Retrigger").y);
                assert!(center("Pan").y > center("Gain").y);
                assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Circle(circle) if circle.radius == 13.0)));
                assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text() == "P" || text.galley.text() == "T")));
                assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text().contains("drag IN / OUT")
                        || text.galley.text() == "Start" || text.galley.text() == "End")));
            }
        }
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
