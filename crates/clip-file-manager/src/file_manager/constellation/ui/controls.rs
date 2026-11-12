use super::*;
use clip_ui::buttons::ButtonRole;
use little_clip_core::palette::AppPalette;

impl ConstellationState {
    pub(super) fn render_indexing_progress(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
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
                if ui
                    .add(ButtonRole::Secondary.button("Cancel", palette))
                    .clicked()
                {
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
                if ui
                    .add(ButtonRole::Secondary.button("Dismiss", palette).small())
                    .clicked()
                {
                    self.indexing_status = None;
                }
            });
        }
    }

    pub(super) fn render_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        _current_folder: &std::path::Path,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
        palette: &AppPalette,
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
                .add_enabled(!self.samples.is_empty(), ButtonRole::Secondary.button("Random", palette))
                .clicked()
            {
                let query = self.search_query.to_lowercase();
                let eligible: Vec<_> = self
                    .samples
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| self.matches_filters(*index, &query))
                    .map(|(_, sample)| sample)
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
                .add_enabled(self.selected_path.is_some(), ButtonRole::Secondary.button("Next unplayed", palette))
                .on_hover_text("Audition the nearest unplayed sample (Shift+Space); click a sample to start a new exploration")
                .clicked()
            {
                self.play_nearest_neighbor(on_play_sample);
            }
        });
        self.render_set_navigation(ui, palette);
    }

    pub(super) fn render_set_navigation(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.horizontal_wrapped(|ui| {
            if self.expanded_set.is_some() {
                if ui
                    .add(
                        ButtonRole::Secondary
                            .button("Back to Map", palette)
                            .min_size(egui::Vec2::ZERO),
                    )
                    .clicked()
                {
                    self.leave_set();
                }
                return;
            }
            if let Some(set) = self.selected_set()
                && ui
                    .add(
                        ButtonRole::Secondary
                            .button("Explore set", palette)
                            .min_size(egui::Vec2::ZERO),
                    )
                    .on_hover_text(
                        "Open pitch rings; cross rings to audition from lowest to highest",
                    )
                    .clicked()
            {
                self.explore_set(set);
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
        });
    }

    pub(super) fn render_camera_controls(&mut self, ui: &mut egui::Ui) {
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
        let active = (0..self.samples.len())
            .filter(|index| self.matches_filters(*index, &query))
            .count();
        let mut label = format!("{active} / {} samples", self.samples.len());
        if let Some(set) = self.selected_set() {
            let set = &self.sample_sets.sets[set];
            label.push_str(&format!(" · {} · {} notes", set.label, set.members.len()));
        }
        label
    }

    /// Two duration endpoints use the observed library bounds; zero retains "all" semantics.
    pub(super) fn render_duration_range(&mut self, ui: &mut egui::Ui) {
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
        let response = range_handles(ui, &mut fractions, self.expanded_set.is_none());
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

    pub(super) fn render_pitch_range(&mut self, ui: &mut egui::Ui) {
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
        let response = range_handles(ui, &mut fractions, self.expanded_set.is_none());
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

    pub(super) fn render_audio_filters(&mut self, ui: &mut egui::Ui) {
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
        if !self.sample_sets.sets.is_empty()
            && outlined_toggle(
                ui,
                "Multisamples only",
                self.multisamples_only,
                ui.visuals().hyperlink_color,
            )
            .clicked()
        {
            self.multisamples_only = !self.multisamples_only;
        }
    }

    pub(super) fn render_empty_state(
        &mut self,
        ui: &mut egui::Ui,
        current_folder: &std::path::Path,
        palette: &AppPalette,
    ) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(10.0));
            ui.heading("Sample constellation");
            ui.add_space(8.0);
            ui.label("Explore samples by sound. Similar timbres sit together.");
            ui.label("Choose a sample, then assign it to a drum pad.");
            ui.add_space(16.0);
            if ui
                .add(ButtonRole::Primary.button("Index folder", palette))
                .on_hover_text("Analyze audio in this folder and its subfolders")
                .clicked()
            {
                self.start_indexing(current_folder);
            }
        });
    }

    pub(super) fn render_sample_hud(
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
        if cached_preview.is_none() && self.pending_hover_path.as_ref() != Some(&sample.file_path) {
            self.pending_hover_path = Some(sample.file_path.clone());
            self.hover_cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.hover_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let request = super::super::HoverPreviewRequest {
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
            rel_str.to_string()
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
            specs.push(super::super::audio_probe::format_sample_rate(s));
        }
        if let Some(c) = ch {
            specs.push(super::super::audio_probe::format_channels(c));
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

    pub(super) fn play_nearest_neighbor(
        &mut self,
        on_play_sample: &mut dyn FnMut(&std::path::Path),
    ) {
        let Some(selected) = &self.selected_path else {
            return;
        };

        let Some(curr) = self.samples.iter().find(|s| &s.file_path == selected) else {
            return;
        };

        let mut closest_sample: Option<ConstellationSample> = None;
        let mut min_dist = f32::MAX;

        let query = self.search_query.to_lowercase();
        for (index, s) in self.samples.iter().enumerate() {
            if &s.file_path == selected
                || self.visited_samples.contains(&s.file_path)
                || !self.matches_filters(index, &query)
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

    pub(super) fn render_playback_trail(
        &self,
        painter: &egui::Painter,
        positions: &[Option<Pos2>],
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
        let active_slice: Vec<(&super::super::HistoryTrailPoint, Pos2)> = self
            .history_trail
            .iter()
            .skip(start_idx)
            .filter_map(|point| {
                let index = self
                    .sample_sets
                    .index_for_path(&point.file_path)
                    .or_else(|| {
                        self.samples
                            .iter()
                            .position(|sample| sample.file_path == point.file_path)
                    })?;
                let key = self
                    .sample_sets
                    .set_for(index)
                    .filter(|set| Some(*set) != self.expanded_set)
                    .map_or(index, |set| self.samples.len() + set);
                positions
                    .get(key)
                    .copied()
                    .flatten()
                    .map(|pos| (point, pos))
            })
            .collect();
        let n = active_slice.len();
        if n < 2 {
            return;
        }

        // Draw fading trajectory segments
        for i in 1..n {
            let p_prev = active_slice[i - 1].1;
            let p_curr = active_slice[i].1;

            // How many steps ago was this segment from the newest point?
            let step_from_new = (n - 1) - i;
            let t = 1.0 - (step_from_new as f32 / (n - 1) as f32); // 1.0 (newest) down to ~0.0 (oldest)

            let fade = t.powf(1.25);
            let alpha = (fade * 210.0).clamp(12.0, 210.0) as u8;
            let width = 1.2 + fade * 1.5;

            let c1 = active_slice[i - 1].0.category.color();
            let c2 = active_slice[i].0.category.color();

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
        if let Some(&(last, last_pos)) = active_slice.last() {
            let last_color = last.category.color();
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
pub(in crate::file_manager::constellation) fn short_label(value: &str, limit: usize) -> String {
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
pub(super) fn range_handles(
    ui: &mut egui::Ui,
    fractions: &mut [f32; 2],
    interactive: bool,
) -> egui::Response {
    let sense = if interactive {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(160.0, 24.0), sense);
    let range_color = if interactive {
        ui.visuals().selection.bg_fill
    } else {
        ui.visuals().weak_text_color()
    };
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
        Stroke::new(3.0_f32, range_color),
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

pub(super) fn note_to_hz(note: f32) -> f32 {
    440.0 * 2.0_f32.powf((note - 69.0) / 12.0)
}

/// A 1 ms offset keeps zero-length bounds finite while distributing decades evenly.
pub(super) fn duration_fraction(time: f32, low: f32, high: f32) -> f32 {
    let span = ((high + 0.001) / (low + 0.001)).ln();
    if span <= f32::EPSILON {
        return 0.0;
    }
    (((time + 0.001) / (low + 0.001)).ln() / span).clamp(0.0, 1.0)
}

pub(super) fn duration_at(fraction: f32, low: f32, high: f32) -> f32 {
    if fraction <= 0.0 {
        return low;
    }
    if fraction >= 1.0 {
        return high;
    }
    ((low + 0.001) * ((high + 0.001) / (low + 0.001)).powf(fraction) - 0.001).clamp(low, high)
}

pub(super) fn sample_pitch_label(sample: &ConstellationSample) -> String {
    if let Some((_, note)) = super::super::multisample::filename_note(&sample.file_path) {
        format!(" · {} (filename)", midi_note_str(note as f32))
    } else if let Some(note) = super::super::multisample::acoustic_note(sample) {
        format!(" · {}", midi_note_str(note))
    } else {
        " · pitch unknown".into()
    }
}

pub(in crate::file_manager::constellation) fn freq_to_note_str(freq: f32) -> String {
    midi_note_str(69.0 + 12.0 * (freq / 440.0).log2())
}

pub(super) fn midi_note_str(midi: f32) -> String {
    let note_names = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let rounded = midi.round() as i32;
    let note_idx = rounded.rem_euclid(12) as usize;
    let octave = rounded.div_euclid(12) - 1;
    format!("{}{}", note_names[note_idx], octave)
}
