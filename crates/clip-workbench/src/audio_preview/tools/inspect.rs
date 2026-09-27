use super::super::*;
use little_clip_ops::hf_audio::HfSpectrogramConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanvasEffect {
    Gain,
    LowPass,
    HighPass,
}

const INSPECT_MIN_CANVAS_HEIGHT: f32 = 226.0;
const INSPECT_HELP_TEXT: &str = "Wheel / +/-: Zoom • Drag: Select • Ctrl+C: Copy • Ctrl+S: Save";

fn inspect_footer_galley(
    ui: &egui::Ui,
    palette: &AppPalette,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let font_id = egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "Navigation:",
        0.0,
        egui::TextFormat {
            font_id: font_id.clone(),
            color: hex_to_c32(&palette.colors.text_secondary),
            ..Default::default()
        },
    );
    job.append(
        INSPECT_HELP_TEXT,
        8.0,
        egui::TextFormat {
            font_id,
            color: hex_to_c32(&palette.colors.text_muted),
            ..Default::default()
        },
    );
    job.wrap.max_width = width.max(1.0);
    ui.fonts(|fonts| fonts.layout_job(job))
}

#[cfg(test)]
mod effect_tests {
    use super::*;
    use little_clip_core::config::WorkbenchAppearance;

    #[test]
    fn inspect_play_and_pause_keep_the_same_button_width() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut widths = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let width =
                    crate::ui_ext::equal_button_width(ui, &["Play [Space]", "Pause [Space]"], true)
                        + 1.0;
                for (label, icon) in [
                    ("Play [Space]", crate::icons::Icon::Play),
                    ("Pause [Space]", crate::icons::Icon::Pause),
                ] {
                    widths.push(
                        ui.add(
                            layout::AudioWorkspace::primary_button_icon(
                                Some(icon),
                                label,
                                hex_to_c32(&palette.diff_add),
                                &palette,
                            )
                            .min_size(Vec2::new(width, crate::ui_ext::CONTROL_HEIGHT)),
                        )
                        .rect
                        .size(),
                    );
                }
            });
        });
        assert_eq!(widths.len(), 2);
        assert_eq!(widths[0], widths[1]);
        assert_eq!(widths[0].y, crate::ui_ext::CONTROL_HEIGHT);
    }

    #[test]
    fn waveform_channel_labels_have_readable_contrast_in_both_appearances() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = crate::ui_appearance::palette(appearance);
            let background = crate::hex_to_c32(&palette.colors.surface_recessed);
            let foreground = crate::hex_to_c32(&palette.harmonies.base);
            let contrast = little_clip_core::palette::contrast_ratio(
                (background.r(), background.g(), background.b()),
                (foreground.r(), foreground.g(), foreground.b()),
            );
            assert!(
                contrast >= 4.5,
                "{appearance:?} waveform channel labels have only {contrast:.2}:1 contrast"
            );
        }
    }

    #[test]
    fn wheel_effects_use_waveform_and_respective_spectrum_halves() {
        let wave = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let spec = Rect::from_min_max(Pos2::new(0.0, 110.0), Pos2::new(100.0, 210.0));
        assert_eq!(
            effect_at_position(Pos2::new(50.0, 50.0), wave, spec),
            Some(CanvasEffect::Gain)
        );
        assert_eq!(
            effect_at_position(Pos2::new(50.0, 130.0), wave, spec),
            Some(CanvasEffect::LowPass)
        );
        assert_eq!(
            effect_at_position(Pos2::new(50.0, 190.0), wave, spec),
            Some(CanvasEffect::HighPass)
        );
        assert_eq!(effect_at_position(Pos2::new(50.0, 230.0), wave, spec), None);
        assert_eq!(effect_at_position(Pos2::new(110.0, 50.0), wave, spec), None);
    }

    #[test]
    fn control_wheel_uses_raw_mouse_wheel_events() {
        let events = [
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: Vec2::new(0.0, 1.0),
                modifiers: egui::Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            },
            egui::Event::Zoom(1.25),
        ];

        assert_eq!(control_wheel_delta(&events, 300.0), 50.0);
    }

    #[test]
    fn control_wheel_ignores_unmodified_wheel_events() {
        let events = [egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 20.0),
            modifiers: egui::Modifiers::default(),
        }];

        assert_eq!(control_wheel_delta(&events, 300.0), 0.0);
    }
}

fn control_wheel_delta(events: &[egui::Event], page_size: f32) -> f32 {
    events
        .iter()
        .filter_map(|event| match event {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } if modifiers.ctrl => {
                let points_per_unit = match unit {
                    egui::MouseWheelUnit::Point => 1.0,
                    egui::MouseWheelUnit::Line => 50.0,
                    egui::MouseWheelUnit::Page => page_size,
                };
                Some((delta.x + delta.y) * points_per_unit)
            }
            _ => None,
        })
        .sum()
}

fn effect_at_position(pos: Pos2, wave_rect: Rect, spec_rect: Rect) -> Option<CanvasEffect> {
    if wave_rect.contains(pos) {
        Some(CanvasEffect::Gain)
    } else if spec_rect.contains(pos) {
        Some(if pos.y < spec_rect.center().y {
            CanvasEffect::LowPass
        } else {
            CanvasEffect::HighPass
        })
    } else {
        None
    }
}

impl AudioPreviewState {
    fn render_normalize_button(&mut self, ui: &mut egui::Ui, palette: &AppPalette, now: f64) {
        let button = layout::AudioWorkspace::secondary_button_icon(
            Some(crate::icons::Icon::Volume),
            "Normalize Track",
            hex_to_c32(&palette.colors.surface_recessed),
            palette,
        );
        if ui
            .add(button)
            .on_hover_text(
                "Peak-normalize the entire track to -1 dBFS. Press [N] over a selected region to normalize only that region; elsewhere [N] normalizes the track.",
            )
            .clicked()
        {
            self.normalize_track(now);
        }
    }

    fn render_track_gain_summary(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.track_gain_db != 0.0 {
            ui.label(format!("Track gain {:+.1} dB", self.track_gain_db));
            layout::AudioWorkspace::section_divider(ui, palette);
        }
    }

    fn handle_normalize_shortcut(
        &mut self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        response: &egui::Response,
        hover_rect: Rect,
        track_min_x: f32,
        track_width: f32,
    ) {
        let requested = !ctx.wants_keyboard_input()
            && ui.input(|input| {
                input.key_pressed(egui::Key::N)
                    && !input.modifiers.ctrl
                    && !input.modifiers.command
                    && !input.modifiers.alt
            });
        if !requested {
            return;
        }
        let hovered_time = response
            .hover_pos()
            .filter(|position| hover_rect.contains(*position))
            .map(|position| {
                let ratio = (position.x - track_min_x) / track_width;
                (self.view_start_sec + ratio * self.view_duration_sec).clamp(0.0, self.duration_sec)
            });
        self.normalize_at_time(hovered_time, ctx.input(|input| input.time));
        ctx.request_repaint();
    }

    pub(crate) fn render_standard_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        _close_requested: &mut bool,
        embedded_height: Option<f32>,
    ) {
        let now = ctx.input(|i| i.time);
        let editor_start_y = ui.cursor().min.y;

        self.poll_audio_analysis(ctx, now);

        // Advance playhead if playing
        if self.is_playing {
            ctx.request_repaint();
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                let new_time = self.playback_start_offset_sec + elapsed;

                // Check if previewing a specific slice
                if self.preview_is_looping {
                    if let Some((sel_start, sel_end)) = self.active_selection_range() {
                        self.preview_stop_time = Some(sel_end);
                        self.preview_slice_range = Some((sel_start, sel_end));
                    }
                }

                if let Some(stop_t) = self.preview_stop_time {
                    if new_time >= stop_t {
                        if self.preview_is_looping {
                            let start_t =
                                self.preview_slice_range.map(|(s, _)| s).unwrap_or_else(|| {
                                    self.active_selection_range().map(|(s, _)| s).unwrap_or(0.0)
                                });
                            self.loop_seek_to(start_t);
                        } else {
                            self.pause();
                            self.current_time_sec = stop_t;
                            self.preview_stop_time = None;
                            self.preview_slice_range = None;
                            self.preview_is_looping = false;
                        }
                    } else {
                        self.current_time_sec = new_time;
                    }
                } else {
                    // Handle looping within region selection
                    let loop_range = self.active_selection_range();
                    if let Some((sel_start, sel_end)) = loop_range {
                        if self.loop_selection && new_time >= sel_end {
                            self.seek_to(sel_start);
                        } else if new_time >= self.duration_sec {
                            self.pause();
                            self.current_time_sec = self.duration_sec;
                        } else {
                            self.current_time_sec = new_time;
                        }
                    } else if new_time >= self.duration_sec {
                        self.pause();
                        self.current_time_sec = self.duration_sec;
                    } else {
                        self.current_time_sec = new_time;
                    }
                }

                // Auto-follow playhead when zoomed in
                let v_end = self.view_start_sec + self.view_duration_sec;
                if self.current_time_sec >= v_end {
                    let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                    self.view_start_sec = (self.current_time_sec - self.view_duration_sec * 0.1)
                        .clamp(0.0, max_start);
                } else if self.current_time_sec < self.view_start_sec {
                    self.view_start_sec =
                        (self.current_time_sec - self.view_duration_sec * 0.1).max(0.0);
                }
            }
        }

        // Clear focus from background palette inputs so they cannot swallow shortcuts
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });

        // Global Keybindings for Audio Preview
        let accepts_tool_shortcuts = !ctx.wants_keyboard_input();
        if accepts_tool_shortcuts && self.active_mode == AudioToolMode::Inspect {
            if ctx.input(|i| i.key_pressed(egui::Key::M)) {
                self.selection_mode = match self.selection_mode {
                    SelectionMode::Single => SelectionMode::Multi,
                    SelectionMode::Multi => SelectionMode::Single,
                };
            }
            if ctx.input(|i| i.key_pressed(egui::Key::L)) {
                self.loop_selection = !self.loop_selection;
                if self.is_playing && self.preview_stop_time.is_some() {
                    self.preview_is_looping = self.loop_selection;
                }
                let status = if self.loop_selection { "ON" } else { "OFF" };
                self.status_toast = Some((format!("🔁 Selection Looping: {}", status), now));
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_play();
        }

        // Poll asynchronous folder browsing dialog if running
        if let Some(ref rx) = self.browse_rx {
            if let Ok(res) = rx.try_recv() {
                if let Some(dir) = res {
                    self.save_destination_dir = dir;
                }
                self.browse_rx = None;
            }
        }

        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || i.raw.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.key_pressed(egui::Key::C)
                    && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd))
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::C)
        });

        if copy_requested {
            match self.selection_mode {
                SelectionMode::Single => self.copy_selected_segment(now, ChannelCopyMode::Native),
                SelectionMode::Multi => self.copy_all_multi_segments(now, ChannelCopyMode::Native),
            }
        }

        let save_requested = ctx.input(|i| {
            i.key_pressed(egui::Key::S)
                && (i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd)
        }) || ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                || i.consume_key(egui::Modifiers::CTRL, egui::Key::S)
        });

        if save_requested {
            if ctx.input(|i| i.modifiers.shift) {
                self.quick_save(now);
            } else {
                self.open_save_dialog();
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            let step = if ctx.input(|i| i.modifiers.shift) {
                5.0
            } else {
                1.0
            };
            self.seek_to(self.current_time_sec - step);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            let step = if ctx.input(|i| i.modifiers.shift) {
                5.0
            } else {
                1.0
            };
            self.seek_to(self.current_time_sec + step);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Home) || i.key_pressed(egui::Key::Num0)) {
            self.seek_to(0.0);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete)) {
            self.delete_active_slice_from_session(now);
        }

        self.handle_undo_shortcuts(ctx);

        // 'Z' key: Snap selection to zero-crossings / Toggle zero-crossing snap mode
        if ctx.input(|i| {
            i.key_pressed(egui::Key::Z)
                && !i.modifiers.ctrl
                && !i.modifiers.command
                && !i.modifiers.alt
        }) {
            self.snap_zero_crossings = !self.snap_zero_crossings;
            let status = if self.snap_zero_crossings {
                "ON"
            } else {
                "OFF"
            };
            if self.snap_zero_crossings {
                let has_sel = match self.selection_mode {
                    SelectionMode::Single => self.selection.is_some(),
                    SelectionMode::Multi => {
                        self.active_multi_index.is_some() && !self.multi_selections.is_empty()
                    }
                };
                if has_sel {
                    self.snap_active_selection_to_zero_crossings(now);
                } else {
                    self.status_toast = Some((format!("∿ Snap to Zero-Crossing: {}", status), now));
                }
            } else {
                self.status_toast = Some((format!("∿ Snap to Zero-Crossing: {}", status), now));
            }
        }

        // 'T' key: Toggle transient detection markers & snapping (when transients are ready)
        if ctx.input(|i| {
            i.key_pressed(egui::Key::T)
                && !i.modifiers.ctrl
                && !i.modifiers.command
                && !i.modifiers.alt
        }) {
            let transients_ready = self
                .analysis
                .as_ref()
                .map(|a| a.is_transients_ready)
                .unwrap_or(false);
            if transients_ready {
                self.show_transients = !self.show_transients;
                let status = if self.show_transients {
                    "ON (Snapping Enabled)"
                } else {
                    "OFF"
                };
                self.status_toast = Some((format!("⚡ Transients: {}", status), now));
            }
        }

        // Zoom keyboard shortcuts: Plus / Minus / Ctrl+0
        if ctx.input(|i| i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals)) {
            self.zoom(0.88, self.current_time_sec);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
            self.zoom(1.136, self.current_time_sec);
        }
        if ctx
            .input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::Num0))
        {
            self.fit_to_window();
        }

        // Number keys 1..9: preview corresponding slice from start to stop
        if self.selection_mode == SelectionMode::Multi && !self.multi_selections.is_empty() {
            let num_keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ];
            for (idx, &k) in num_keys.iter().enumerate() {
                if ctx.input(|i| {
                    i.key_pressed(k)
                        && !i.modifiers.ctrl
                        && !i.modifiers.command
                        && !i.modifiers.alt
                }) {
                    if idx < self.multi_selections.len() {
                        let (s, e) = self.multi_selections[idx];
                        self.active_multi_index = Some(idx);
                        self.play_slice_preview(s, e);
                        self.status_toast = Some((
                            format!(
                                "▶ Previewing slice #{} ({:02}:{:04.1} - {:02}:{:04.1})",
                                idx + 1,
                                (s / 60.0) as u32,
                                s % 60.0,
                                (e / 60.0) as u32,
                                e % 60.0
                            ),
                            now,
                        ));
                        break;
                    }
                }
            }
        }

        // 2. Transport, Zoom & Slicing Control Bar
        layout::AudioWorkspace::control_bar(ui, palette, |ui| {
            layout::AudioWorkspace::control_group(ui, palette, "Transport", |ui| {
                // Play / Pause Button
                let (play_icon, play_label) = if self.is_playing {
                    (crate::icons::Icon::Pause, "Pause [Space]")
                } else {
                    (crate::icons::Icon::Play, "Play [Space]")
                };
                let play_color = if self.is_playing {
                    hex_to_c32(&palette.diff_mod)
                } else {
                    hex_to_c32(&palette.diff_add)
                };
                // Leave one point for bold glyph shaping beyond the regular-label measurement.
                let play_width =
                    crate::ui_ext::equal_button_width(ui, &["Play [Space]", "Pause [Space]"], true)
                        + 1.0;

                let play_btn = layout::AudioWorkspace::primary_button_icon(
                    Some(play_icon),
                    play_label,
                    play_color,
                    palette,
                )
                .min_size(Vec2::new(play_width, crate::ui_ext::CONTROL_HEIGHT))
                .rounding(Rounding::same(6.0));

                if ui.add(play_btn).clicked() {
                    self.toggle_play();
                }

                ui.add_space(3.0);

                let stop_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Stop),
                    "Stop",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                );
                if ui.add(stop_btn).clicked() {
                    self.stop();
                }

                let replay_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Undo),
                    "Start",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                );
                if ui.add(replay_btn).clicked() {
                    self.seek_to(self.active_selection_range().map(|(s, _)| s).unwrap_or(0.0));
                }

                ui.add_space(3.0);

                let loop_btn = layout::AudioWorkspace::toggle_button_icon(
                    Some(crate::icons::Icon::Repeat),
                    "Loop [L]",
                    self.loop_selection,
                    palette,
                );
                if ui
                    .add(loop_btn)
                    .on_hover_text(
                        "Loop playback within the active selection or on right-click preview. Press [L] to toggle.",
                    )
                    .clicked()
                {
                    self.loop_selection = !self.loop_selection;
                    if self.is_playing && self.preview_stop_time.is_some() {
                        self.preview_is_looping = self.loop_selection;
                    }
                    let status = if self.loop_selection { "ON" } else { "OFF" };
                    self.status_toast = Some((
                        format!("🔁 Selection Looping: {}", status),
                        now,
                    ));
                }

                ui.add_space(10.0);

                layout::AudioWorkspace::time_readout(
                    ui,
                    self.current_time_sec,
                    self.duration_sec,
                    palette,
                );

                ui.add_space(12.0);

                if let Some(action) = layout::AudioWorkspace::zoom_control_bar(ui, palette) {
                    match action {
                        crate::ui_ext::ZoomAction::ZoomIn => self.zoom(0.75, self.current_time_sec),
                        crate::ui_ext::ZoomAction::ZoomOut => {
                            self.zoom(1.33, self.current_time_sec)
                        }
                        crate::ui_ext::ZoomAction::Fit => self.fit_to_window(),
                    }
                }
            });

            layout::AudioWorkspace::section_divider(ui, palette);
            layout::AudioWorkspace::control_group(ui, palette, "Selection", |ui| {
                // Mode Toggle: Single Region (default) vs Multi-Region Mode
                let is_multi = self.selection_mode == SelectionMode::Multi;
                let mode_btn = layout::AudioWorkspace::toggle_button_icon(
                    Some(crate::icons::Icon::Grid),
                    "Multi-region [M]",
                    is_multi,
                    palette,
                );
                if ui
                    .add(mode_btn)
                    .on_hover_text(
                        "Toggle between Single Slice mode and Multi-Region extraction mode",
                    )
                    .clicked()
                {
                    let next_mode = if is_multi {
                        SelectionMode::Single
                    } else {
                        SelectionMode::Multi
                    };
                    self.set_selection_mode(next_mode);
                }

                // Snap to 0-Crossing Toggle
                let snap_btn = layout::AudioWorkspace::toggle_button_icon(
                    Some(crate::icons::Icon::Waves),
                    "Snap 0-Cross [Z]",
                    self.snap_zero_crossings,
                    palette,
                );
                if ui.add(snap_btn).on_hover_text("Snap selection boundaries to zero-crossings to prevent audio clicks/pops. Press [Z] to snap now.").clicked() {
                    self.snap_zero_crossings = !self.snap_zero_crossings;
                    if self.snap_zero_crossings {
                        self.snap_active_selection_to_zero_crossings(now);
                    }
                }

                // Transient Detection & Snap Toggle
                let transients_ready = self
                    .analysis
                    .as_ref()
                    .map(|a| a.is_transients_ready)
                    .unwrap_or(false);
                let trans_btn = layout::AudioWorkspace::toggle_button_icon(
                    Some(crate::icons::Icon::Zap),
                    if transients_ready {
                        "Transients [T]"
                    } else {
                        "Transients…"
                    },
                    self.show_transients && transients_ready,
                    palette,
                );
                if transients_ready {
                    if ui.add(trans_btn).on_hover_text("Toggle transient marks and magnetic snapping for selections. Press [T] to toggle.").clicked() {
                        self.show_transients = !self.show_transients;
                        let status = if self.show_transients { "ON (Snapping Enabled)" } else { "OFF" };
                        self.status_toast = Some((
                            format!("Transients: {}", status),
                            now,
                        ));
                    }

                    // Transient sensitivity slider
                    if self.show_transients {
                        ui.add_space(4.0);
                        let mut sensitivity_position =
                            transient_sensitivity_to_slider(self.transient_sensitivity);
                        let sens_slider = egui::Slider::new(&mut sensitivity_position, 0.0..=1.0)
                            .step_by(0.01)
                            .text("Sens")
                            .show_value(false);
                        if ui
                            .add(sens_slider)
                            .on_hover_text(
                                "Adjust transient detection from stronger attacks toward softer detail",
                            )
                            .changed()
                        {
                            self.transient_sensitivity =
                                transient_sensitivity_from_slider(sensitivity_position);
                            if let Some(ref mut analysis) = self.analysis {
                                analysis.recompute_transients(self.transient_sensitivity);
                            }
                        }
                    }
                } else {
                    crate::ui_ext::on_hover_text_with_disabled_reason(
                        ui.add_enabled(false, trans_btn),
                        "Toggle transient marks and magnetic snapping for selections. Press [T] to toggle.",
                        "Calculating audio spectral flux transients...",
                    );
                }

                ui.add_space(8.0);

                self.render_normalize_button(ui, palette, now);

                // Trim Silence Button
                let trim_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Scissors),
                    "Trim Silence",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                );
                if ui
                    .add(trim_btn)
                    .on_hover_text("Trim silence (< -45 dB) from both leading and trailing sides")
                    .clicked()
                {
                    self.trim_silence(-45.0, now);
                }
            });

            layout::AudioWorkspace::section_divider(ui, palette);
            layout::AudioWorkspace::control_group(ui, palette, "Export", |ui| {
                let channels_count = self.analysis.as_ref().map(|a| a.channels).unwrap_or(2);

                match self.selection_mode {
                    SelectionMode::Single => {
                        // Copy Slice (Native) Button
                        let copy_label = if self.selection.is_some() {
                            "Copy Slice [Ctrl+C]"
                        } else {
                            "Copy Track [Ctrl+C]"
                        };

                        let copy_btn = layout::AudioWorkspace::primary_button_icon(
                            Some(crate::icons::Icon::Copy),
                            copy_label,
                            hex_to_c32(&palette.harmonies.base),
                            palette,
                        )
                        .rounding(Rounding::same(6.0));

                        if ui
                            .add(copy_btn)
                            .on_hover_text("Copy selected segment with all original channels")
                            .clicked()
                        {
                            self.copy_selected_segment(now, ChannelCopyMode::Native);
                        }

                        // Channel-Specific Copy Buttons (Left only mono, Right only mono)
                        if channels_count >= 2 {
                            ui.add_space(4.0);
                            let left_btn = layout::AudioWorkspace::secondary_button(
                                "Copy left (mono)",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            );
                            if ui
                                .add(left_btn)
                                .on_hover_text("Copy only the Left channel as a mono audio file")
                                .clicked()
                            {
                                self.copy_selected_segment(now, ChannelCopyMode::LeftOnly);
                            }

                            let right_btn = layout::AudioWorkspace::secondary_button(
                                "Copy right (mono)",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            );
                            if ui
                                .add(right_btn)
                                .on_hover_text("Copy only the Right channel as a mono audio file")
                                .clicked()
                            {
                                self.copy_selected_segment(now, ChannelCopyMode::RightOnly);
                            }
                        }

                        ui.add_space(4.0);
                        let save_label = if self.selection.is_some() {
                            "Save File... [Ctrl+S]"
                        } else {
                            "Save Track... [Ctrl+S]"
                        };
                        let save_btn = layout::AudioWorkspace::primary_button_icon(
                            Some(crate::icons::Icon::Save),
                            save_label,
                            hex_to_c32(&palette.diff_add),
                            palette,
                        )
                        .rounding(Rounding::same(6.0));
                        if ui.add(save_btn).on_hover_text("Save slice to disk formatted as original_name + timestamp + duration [Ctrl+S]").clicked() {
                                self.open_save_dialog();
                            }
                    }
                    SelectionMode::Multi => {
                        let count = self.multi_selections.len();
                        let copy_label = if count > 0 {
                            format!("Copy All ({}) [Ctrl+C]", count)
                        } else {
                            "Copy Track [Ctrl+C]".to_string()
                        };

                        let copy_btn = layout::AudioWorkspace::primary_button_icon(
                            Some(crate::icons::Icon::Copy),
                            copy_label,
                            hex_to_c32(&palette.harmonies.base),
                            palette,
                        )
                        .rounding(Rounding::same(6.0));

                        if ui
                            .add(copy_btn)
                            .on_hover_text(
                                "Export and copy all selected regions to Clipboard & Left Hand",
                            )
                            .clicked()
                        {
                            self.copy_all_multi_segments(now, ChannelCopyMode::Native);
                        }

                        if channels_count >= 2 {
                            ui.add_space(4.0);
                            let left_btn = layout::AudioWorkspace::secondary_button(
                                "Copy left (mono)",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            );
                            if ui
                                .add(left_btn)
                                .on_hover_text(
                                    "Export all regions isolating only the Left channel as mono",
                                )
                                .clicked()
                            {
                                self.copy_all_multi_segments(now, ChannelCopyMode::LeftOnly);
                            }

                            let right_btn = layout::AudioWorkspace::secondary_button(
                                "Copy right (mono)",
                                hex_to_c32(&palette.colors.surface_recessed),
                                palette,
                            );
                            if ui
                                .add(right_btn)
                                .on_hover_text(
                                    "Export all regions isolating only the Right channel as mono",
                                )
                                .clicked()
                            {
                                self.copy_all_multi_segments(now, ChannelCopyMode::RightOnly);
                            }
                        }

                        ui.add_space(4.0);
                        let save_label = if count > 0 {
                            format!("Save All ({}) [Ctrl+S]", count)
                        } else {
                            "Save Track [Ctrl+S]".to_string()
                        };
                        let save_btn = layout::AudioWorkspace::primary_button_icon(
                            Some(crate::icons::Icon::Save),
                            save_label,
                            hex_to_c32(&palette.diff_add),
                            palette,
                        )
                        .rounding(Rounding::same(6.0));
                        if ui
                            .add(save_btn)
                            .on_hover_text(
                                "Save all selected regions to individual audio files [Ctrl+S]",
                            )
                            .clicked()
                        {
                            self.open_save_dialog();
                        }
                    }
                }

                ui.add_space(6.0);

                let has_analysis = self.analysis.is_some();

                let copy_wave_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Copy),
                    "Copy Waveform Image",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                );
                let copy_wave_response = crate::ui_ext::on_hover_text_with_disabled_reason(
                    ui.add_enabled(has_analysis, copy_wave_btn),
                    "Copy rendered waveform image with axes and tick marks to clipboard",
                    "Wait for audio analysis to finish before copying the waveform image.",
                );
                if copy_wave_response.clicked() {
                    self.copy_waveform_image(palette, now);
                }

                ui.add_space(4.0);

                let copy_spec_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Copy),
                    "Copy Spectrum Image",
                    hex_to_c32(&palette.colors.surface_recessed),
                    palette,
                );
                let copy_spec_response = crate::ui_ext::on_hover_text_with_disabled_reason(
                    ui.add_enabled(has_analysis, copy_spec_btn),
                    "Copy rendered spectrum heatmap with frequency axes and timestamp ruler to clipboard",
                    "Wait for audio analysis to finish before copying the spectrum image.",
                );
                if copy_spec_response.clicked() {
                    self.copy_spectrum_image(palette, now);
                }
            });
        });

        self.render_selection_info_bar(ui, palette, now);

        ui.add_space(8.0);

        // 3. Main Synchronized 3-View Representation Canvas
        let controls_spent = ui.cursor().min.y - editor_start_y;
        let avail_w = ui.available_width() - 28.0;
        let footer_galley = inspect_footer_galley(ui, palette, ui.available_width());
        let footer_height = footer_galley.size().y + 12.0;
        let workspace_height = embedded_height
            .map(|height| (height - controls_spent).max(INSPECT_MIN_CANVAS_HEIGHT + footer_height))
            .unwrap_or_else(|| ui.available_height());

        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), workspace_height),
            egui::Layout::top_down(egui::Align::Min),
            |workspace| {
                egui::TopBottomPanel::bottom(workspace.make_persistent_id("audio_inspect_help_footer"))
                    .exact_height(footer_height)
                    .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 3.0)))
                    .show_inside(workspace, |ui| {
                        ui.add_space(6.0);
                        let (row_rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), footer_galley.size().y),
                            egui::Sense::hover(),
                        );
                        ui.painter().galley(
                            Pos2::new(
                                row_rect.center().x - footer_galley.size().x * 0.5,
                                row_rect.min.y,
                            ),
                            footer_galley.clone(),
                            Color32::PLACEHOLDER,
                        );
                        response.on_hover_text("N: Normalize hovered region / track • Shift+Wheel waveform: Gain • Shift+Wheel spectrum top/bottom: Low/High Pass • Ctrl+Wheel: Crossfade • Ctrl+Middle-Click: Clear Crossfade • Middle-Click: Reset Effect • Middle-Drag: Pan • Wheel / +/-: Zoom • Drag: Select • Ctrl+C: Copy • Ctrl+S: Save");
                    });

                let canvas_height = workspace.available_height();
                workspace.horizontal(|ui| {
                    ui.add_space(14.0);
                    self.render_waveform_canvas(
                        ui,
                        ctx,
                        palette,
                        avail_w,
                        canvas_height.max(0.0),
                    );
                });
            },
        );

        // Render Save Dialog modal if open
        self.render_save_dialog(ctx, palette, now);
    }

    fn render_crossfade_label(ui: &mut egui::Ui, crossfade_sec: f32) {
        if crossfade_sec > 0.0 {
            ui.label(format!("Crossfade {:.0} ms", crossfade_sec * 1000.0));
        }
    }

    fn render_selection_info_bar(&mut self, ui: &mut egui::Ui, palette: &AppPalette, now: f64) {
        layout::AudioWorkspace::info_bar(ui, palette, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("Selection info:")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .strong()
                        .color(hex_to_c32(&palette.colors.text_primary)),
                );
                layout::AudioWorkspace::section_divider(ui, palette);
                self.render_track_gain_summary(ui, palette);
                match self.selection_mode {
                    SelectionMode::Single => {
                        if let Some((start, end)) = self.selection {
                            let (start, end) = (start.min(end), start.max(end));
                            ui.label(
                                egui::RichText::new(format!(
                                    "Start {}   End {}   Duration {}",
                                    layout::AudioWorkspace::clock_time(start),
                                    layout::AudioWorkspace::clock_time(end),
                                    layout::AudioWorkspace::clock_time(end - start),
                                ))
                                .monospace()
                                .color(hex_to_c32(&palette.harmonies.base)),
                            );
                            if self.single_region_gain_db != 0.0 {
                                ui.label(if self.single_region_gain_db == f32::NEG_INFINITY {
                                    "Gain Muted".to_string()
                                } else {
                                    format!("Gain {:+.1} dB", self.single_region_gain_db)
                                });
                            }
                            if let Some(hz) = self.single_low_pass_hz {
                                ui.label(format!("Low pass {hz:.0} Hz"));
                            }
                            if let Some(hz) = self.single_high_pass_hz {
                                ui.label(format!("High pass {hz:.0} Hz"));
                            }
                            Self::render_crossfade_label(ui, self.active_region_crossfade());
                            if ui
                                .add(layout::AudioWorkspace::secondary_button_icon(
                                    Some(crate::icons::Icon::Close),
                                    "Clear",
                                    hex_to_c32(&palette.colors.surface_recessed),
                                    palette,
                                ))
                                .clicked()
                            {
                                self.selection = None;
                                self.clear_single_region_effects();
                            }
                        } else {
                            ui.label(
                                "No region selected. Drag across the waveform to select a slice.",
                            );
                        }
                    }
                    SelectionMode::Multi => {
                        let count = self.multi_selections.len();
                        let total: f32 = self
                            .multi_selections
                            .iter()
                            .map(|(start, end)| (end - start).abs())
                            .sum();
                        ui.label(
                            egui::RichText::new(format!(
                                "{count} regions   Total {}",
                                layout::AudioWorkspace::clock_time(total),
                            ))
                            .monospace()
                            .color(hex_to_c32(&palette.harmonies.base)),
                        );
                        if let Some((index, (start, end))) =
                            self.active_multi_index.and_then(|index| {
                                self.multi_selections.get(index).map(|range| (index, range))
                            })
                        {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Active #{}   {}–{}   Duration {}",
                                    index + 1,
                                    layout::AudioWorkspace::clock_time(*start),
                                    layout::AudioWorkspace::clock_time(*end),
                                    layout::AudioWorkspace::clock_time((end - start).abs()),
                                ))
                                .monospace(),
                            );
                            if let Some(gain) = self.region_gain_db.get(index)
                                && *gain != 0.0
                            {
                                ui.label(if *gain == f32::NEG_INFINITY {
                                    "Gain Muted".to_string()
                                } else {
                                    format!("Gain {gain:+.1} dB")
                                });
                            }
                            if let Some(hz) = self.active_filter_cutoff(SpectrumFilterKind::LowPass)
                            {
                                ui.label(format!("Low pass {hz:.0} Hz"));
                            }
                            if let Some(hz) =
                                self.active_filter_cutoff(SpectrumFilterKind::HighPass)
                            {
                                ui.label(format!("High pass {hz:.0} Hz"));
                            }
                            Self::render_crossfade_label(ui, self.active_region_crossfade());
                        } else if count == 0 {
                            ui.label("Drag across the waveform to add regions.");
                        } else {
                            ui.label("Select a region to inspect or preview it.");
                        }
                        if count > 0
                            && ui
                                .add(layout::AudioWorkspace::secondary_button_icon(
                                    Some(crate::icons::Icon::Trash),
                                    "Clear All",
                                    hex_to_c32(&palette.colors.surface_recessed),
                                    palette,
                                ))
                                .clicked()
                        {
                            self.clear_multi_regions();
                        }
                    }
                }
            });

            // Multi-region slice pill list strip when in Multi mode
            let has_multi_pills =
                self.selection_mode == SelectionMode::Multi && !self.multi_selections.is_empty();
            if has_multi_pills {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Slices:")
                            .strong()
                            .color(hex_to_c32(&palette.colors.text_secondary)),
                    );
                    let mut seek_target = None;
                    let mut preview_target = None;
                    let mut remove_slice_idx = None;
                    let slices_copy = self.multi_selections.clone();
                    let active_idx = self.active_multi_index;

                    egui::ScrollArea::horizontal()
                        .id_source("multi_slices_scroll")
                        .show(ui, |ui| {
                            for (idx, &(s, e)) in slices_copy.iter().enumerate() {
                                let is_active = active_idx == Some(idx);
                                let (bg_col, border_col, pref_col) = if is_active {
                                    (
                                        hex_to_c32_alpha(&palette.harmonies.base, 40),
                                        hex_to_c32(&palette.harmonies.base),
                                        hex_to_c32(&palette.harmonies.base),
                                    )
                                } else {
                                    (
                                        hex_to_c32(&palette.colors.surface_recessed),
                                        hex_to_c32(&palette.colors.surface_panel),
                                        hex_to_c32(&palette.colors.text_secondary),
                                    )
                                };
                                let text_col =
                                    crate::contrast_text_c32(bg_col, Some(pref_col), palette);

                                let label = format!(
                                    "#{}: {:02}:{:04.1}-{:02}:{:04.1} ({:.2}s)",
                                    idx + 1,
                                    (s / 60.0) as u32,
                                    s % 60.0,
                                    (e / 60.0) as u32,
                                    e % 60.0,
                                    e - s
                                );
                                let btn = egui::Button::new(
                                    egui::RichText::new(label).strong().color(text_col),
                                )
                                .fill(bg_col)
                                .stroke(Stroke::new(1.0_f32, border_col))
                                .rounding(Rounding::same(4.0))
                                .min_size(Vec2::new(0.0, crate::ui_ext::CONTROL_HEIGHT));

                                let btn_resp = ui.add(btn).on_hover_text(
                                    "Left-Click: focus & seek | Right-Click: preview slice (hold or toggle [L] to loop)",
                                );
                                if btn_resp.clicked() {
                                    seek_target = Some((idx, s));
                                }
                                if btn_resp.secondary_clicked() {
                                    preview_target = Some((idx, s, e));
                                }

                                let del_bg = hex_to_c32(&palette.colors.surface_recessed);
                                let del_fg = crate::contrast_text_c32(
                                    del_bg,
                                    Some(hex_to_c32(&palette.diff_del)),
                                    palette,
                                );
                                let del_btn = crate::ui_ext::icon_button(
                                    crate::icons::Icon::Close,
                                    11.0,
                                    del_bg,
                                    Some(del_fg),
                                    palette,
                                )
                                .min_size(Vec2::new(
                                    crate::ui_ext::CONTROL_HEIGHT,
                                    crate::ui_ext::CONTROL_HEIGHT,
                                ));
                                if ui
                                    .add(del_btn)
                                    .on_hover_text(format!("Remove slice #{}", idx + 1))
                                    .clicked()
                                {
                                    remove_slice_idx = Some(idx);
                                }
                                ui.add_space(6.0);
                            }
                        });

                    if let Some((idx, s, e)) = preview_target {
                        self.active_multi_index = Some(idx);
                        if self.loop_selection {
                            self.play_slice_preview_loop(s, e);
                            self.status_toast = Some((
                                format!(
                                    "🔁 Looping slice #{} ({:02}:{:04.1} - {:02}:{:04.1})",
                                    idx + 1,
                                    (s / 60.0) as u32,
                                    s % 60.0,
                                    (e / 60.0) as u32,
                                    e % 60.0
                                ),
                                now,
                            ));
                        } else {
                            self.play_slice_preview(s, e);
                            self.status_toast = Some((
                                format!(
                                    "▶ Previewing slice #{} ({:02}:{:04.1} - {:02}:{:04.1})",
                                    idx + 1,
                                    (s / 60.0) as u32,
                                    s % 60.0,
                                    (e / 60.0) as u32,
                                    e % 60.0
                                ),
                                now,
                            ));
                        }
                    } else if let Some((idx, s)) = seek_target {
                        self.active_multi_index = Some(idx);
                        self.seek_to(s);
                    }
                    if let Some(idx) = remove_slice_idx {
                        self.remove_multi_region(idx);
                    }
                });
            }
        });
    }

    fn handle_crossfade_scroll(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        control_scroll: f32,
        wave_rect: Rect,
        spec_rect: Rect,
    ) -> bool {
        if control_scroll.abs() <= 0.1 {
            return false;
        }
        let Some(position) = response
            .hover_pos()
            .filter(|position| effect_at_position(*position, wave_rect, spec_rect).is_some())
        else {
            return false;
        };
        let time = self.view_start_sec
            + ((position.x - wave_rect.min.x) / wave_rect.width()) * self.view_duration_sec;
        if !self.focus_region_at_time(time) {
            return false;
        }
        let steps = (control_scroll / 50.0).clamp(-4.0, 4.0);
        self.adjust_active_region_crossfade(steps, Instant::now());
        ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
        ui.ctx().request_repaint();
        true
    }

    fn reset_canvas_effect(&mut self, effect: Option<CanvasEffect>, crossfade: bool) -> bool {
        if crossfade {
            return self.reset_active_region_crossfade();
        }
        match effect {
            Some(CanvasEffect::Gain) => self.reset_active_region_gain(),
            Some(CanvasEffect::LowPass) => {
                self.reset_active_region_filter(SpectrumFilterKind::LowPass)
            }
            Some(CanvasEffect::HighPass) => {
                self.reset_active_region_filter(SpectrumFilterKind::HighPass)
            }
            None => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_hovered_region_crossfade(
        &self,
        painter: &egui::Painter,
        response: &egui::Response,
        fade_bar: Color32,
        ruler_rect: Rect,
        view_top_y: f32,
        view_bot_y: f32,
        track_min_x: f32,
        track_max_x: f32,
        track_width: f32,
    ) {
        let Some(position) = response
            .hover_pos()
            .filter(|position| position.y >= view_top_y && position.y <= view_bot_y)
        else {
            return;
        };
        let time = self.view_start_sec
            + ((position.x - track_min_x) / track_width) * self.view_duration_sec;
        let hovered = match self.selection_mode {
            SelectionMode::Single => self.selection.and_then(|(start, end)| {
                AudioSelectionModel::contains((start, end), time).then_some((
                    start.min(end),
                    start.max(end),
                    self.single_crossfade_sec,
                ))
            }),
            SelectionMode::Multi => self
                .multi_selections
                .iter()
                .enumerate()
                .find(|(_, range)| AudioSelectionModel::contains(**range, time))
                .map(|(index, &(start, end))| {
                    (
                        start.min(end),
                        start.max(end),
                        self.region_crossfade_sec.get(index).copied().unwrap_or(0.0),
                    )
                }),
        };
        let Some((start, end, crossfade)) = hovered else {
            return;
        };
        let crossfade = crossfade.min(Self::crossfade_limit((start, end), self.duration_sec));
        if crossfade <= 0.0 {
            return;
        }
        for time in [
            (start > 0.0).then_some(start - crossfade),
            (end < self.duration_sec).then_some(end + crossfade),
        ]
        .into_iter()
        .flatten()
        {
            let x = track_min_x
                + ((time - self.view_start_sec) / self.view_duration_sec.max(0.0001)) * track_width;
            if x >= track_min_x && x <= track_max_x {
                painter.line_segment(
                    [Pos2::new(x, ruler_rect.min.y), Pos2::new(x, view_bot_y)],
                    Stroke::new(2.0_f32, fade_bar),
                );
                painter.line_segment(
                    [
                        Pos2::new(x - 4.0, ruler_rect.min.y + 4.0),
                        Pos2::new(x + 4.0, ruler_rect.min.y + 4.0),
                    ],
                    Stroke::new(2.0_f32, fade_bar),
                );
            }
        }
    }

    fn render_waveform_canvas(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        width: f32,
        height: f32,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        let bg_frame = crate::hex_to_c32(&palette.colors.surface_recessed);
        let border_frame = crate::hex_to_c32(&palette.colors.surface_panel);
        let text_f_med = crate::hex_to_c32(&palette.colors.text_secondary);
        let text_f_low = crate::hex_to_c32(&palette.colors.text_muted);
        let accent_base = crate::hex_to_c32(&palette.harmonies.base);
        let accent_playhead = crate::hex_to_c32(&palette.harmonies.complementary);
        let transient_col = crate::hex_to_c32(&palette.diff_mod);

        // Frame border & background
        painter.rect_filled(rect, Rounding::same(8.0), bg_frame);
        painter.rect_stroke(
            rect,
            Rounding::same(8.0),
            Stroke::new(1.0_f32, border_frame),
        );

        if self.is_loading_analysis || self.analysis.is_none() {
            let elapsed = self.loading_start_time.elapsed().as_secs_f32();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!(
                    "⏳ Decoding audio track & building waveform... ({:.1}s)",
                    elapsed
                ),
                egui::FontId::proportional(15.0),
                text_f_med,
            );
            return;
        }

        // Layout Partitioning:
        let label_w = 48.0;
        let track_min_x = rect.min.x + label_w;
        let track_max_x = rect.max.x - 6.0;
        let track_width = (track_max_x - track_min_x).max(10.0);

        let track_rect = Rect::from_min_max(
            Pos2::new(track_min_x, rect.min.y),
            Pos2::new(track_max_x, rect.max.y),
        );
        let [ruler_rect, wave_rect, spec_rect, strip_rect, scroll_rect] =
            layout::AudioWorkspace::inspect_timeline_rects(track_rect);
        let wave_h = wave_rect.height();

        // Transform helpers parameterized by (start, dur) to avoid closures holding &self
        let x_to_t = move |x: f32, start: f32, dur: f32| -> f32 {
            start + ((x - track_min_x) / track_width) * dur
        };
        let t_to_x = move |t: f32, start: f32, dur: f32| -> f32 {
            track_min_x + ((t - start) / dur) * track_width
        };

        let selection_hover_rect = Rect::from_min_max(
            Pos2::new(track_min_x, ruler_rect.max.y),
            Pos2::new(track_max_x, strip_rect.max.y),
        );
        self.handle_normalize_shortcut(
            ui,
            ctx,
            &response,
            selection_hover_rect,
            track_min_x,
            track_width,
        );

        // --- INTERACTIVE MOUSE / INPUT HANDLING (MUTATES SELF BEFORE PAINTING) ---
        if response.hovered() {
            if let Some(mouse_pos) = response.hover_pos() {
                let handle_grab_radius_px = 10.0;
                let mut near_handle = false;
                match self.selection_mode {
                    SelectionMode::Single => {
                        if let Some((s, e)) = self.selection {
                            let xs = t_to_x(s, self.view_start_sec, self.view_duration_sec);
                            let xe = t_to_x(e, self.view_start_sec, self.view_duration_sec);
                            if (mouse_pos.x - xs).abs() <= handle_grab_radius_px
                                || (mouse_pos.x - xe).abs() <= handle_grab_radius_px
                            {
                                near_handle = true;
                            }
                        }
                    }
                    SelectionMode::Multi => {
                        for &(s, e) in &self.multi_selections {
                            let xs = t_to_x(s, self.view_start_sec, self.view_duration_sec);
                            let xe = t_to_x(e, self.view_start_sec, self.view_duration_sec);
                            if (mouse_pos.x - xs).abs() <= handle_grab_radius_px
                                || (mouse_pos.x - xe).abs() <= handle_grab_radius_px
                            {
                                near_handle = true;
                                break;
                            }
                        }
                    }
                }
                if near_handle {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                }
            }

            // Read raw_scroll_delta directly to avoid smooth_scroll_delta momentum lingering over multiple frames
            let raw_scroll = ui.input(|i| i.raw_scroll_delta);
            let combined_scroll = raw_scroll.x + raw_scroll.y;
            // egui-winit emits Ctrl+wheel as MouseWheel + Zoom, not Scroll, so it never
            // appears in raw_scroll_delta. Read the raw wheel event for this gesture.
            let control_scroll =
                ui.input(|i| control_wheel_delta(&i.raw.events, response.rect.height()));

            let crossfade_handled =
                self.handle_crossfade_scroll(ui, &response, control_scroll, wave_rect, spec_rect);
            if !crossfade_handled {
                if ui.input(|i| i.modifiers.shift) && combined_scroll.abs() > 0.1 {
                    // egui-winit turns Shift+wheel into horizontal Scroll events.
                    let shifted_scroll = combined_scroll;
                    let hover_pos = response.hover_pos();
                    let effect =
                        hover_pos.and_then(|pos| effect_at_position(pos, wave_rect, spec_rect));
                    let hovered_time = hover_pos
                        .filter(|_| effect.is_some())
                        .map(|pos| x_to_t(pos.x, self.view_start_sec, self.view_duration_sec));
                    let has_selected_region =
                        hovered_time.is_some_and(|time| self.focus_region_at_time(time));
                    if has_selected_region {
                        let steps = (shifted_scroll / 50.0).clamp(-4.0, 4.0);
                        let edit_time = Instant::now();
                        match effect {
                            Some(CanvasEffect::Gain) => {
                                self.adjust_active_region_gain(steps, edit_time)
                            }
                            Some(CanvasEffect::LowPass) => self.adjust_active_region_filter(
                                SpectrumFilterKind::LowPass,
                                steps,
                                edit_time,
                            ),
                            Some(CanvasEffect::HighPass) => self.adjust_active_region_filter(
                                SpectrumFilterKind::HighPass,
                                steps,
                                edit_time,
                            ),
                            None => {}
                        }
                        // The enclosing stem ScrollArea runs after this child UI.
                        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
                        ui.ctx().request_repaint();
                    } else {
                        // Shift+Wheel pans horizontally when no region is active.
                        let dt = -(shifted_scroll / 50.0) * self.view_duration_sec * 0.12;
                        self.pan(dt);
                    }
                } else if raw_scroll.y.abs() > 0.1 {
                    // Consistent, fine-grained exponential zoom curve:
                    // Standard step is ~5.5% per wheel notch (50 pt in egui-winit).
                    // Holding Ctrl or Alt enables ultra-fine zoom (~2.5% per notch).
                    let is_fine = ui.input(|i| i.modifiers.alt || i.modifiers.ctrl);
                    let step_ratio: f32 = if is_fine { 1.025 } else { 1.055 };
                    let steps = (raw_scroll.y / 50.0).clamp(-4.0, 4.0);
                    let factor = step_ratio.powf(-steps);

                    let anchor = if let Some(pos) = response.hover_pos() {
                        x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                            .clamp(0.0, self.duration_sec)
                    } else {
                        self.current_time_sec
                    };
                    self.zoom(factor, anchor);
                } else if raw_scroll.x.abs() > 0.1 {
                    // Horizontal wheel pans horizontally
                    let dt = -(raw_scroll.x / 50.0) * self.view_duration_sec * 0.12;
                    self.pan(dt);
                }
            }
        }

        let is_middle = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        let is_primary = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
        let is_secondary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
        let is_secondary_pressed =
            ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        let is_secondary_released =
            ui.input(|i| i.pointer.button_released(egui::PointerButton::Secondary));
        let shift_down = ui.input(|i| i.modifiers.shift);
        let now = ui.input(|i| i.time);

        // Right-click press down on waveform canvas (when Shift is not held)
        if is_secondary_pressed && !shift_down {
            let pointer_pos = ui
                .input(|i| i.pointer.interact_pos())
                .or_else(|| response.interact_pointer_pos())
                .or_else(|| response.hover_pos());
            if let Some(pos) = pointer_pos.filter(|p| rect.contains(*p)) {
                let is_in_ruler = pos.y <= ruler_rect.max.y;
                if !is_in_ruler {
                    let clicked_t = x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                        .clamp(0.0, self.duration_sec);
                    let clicked_slice = match self.selection_mode {
                        SelectionMode::Multi => self
                            .multi_selections
                            .iter()
                            .enumerate()
                            .find(|(_, (s, e))| clicked_t >= *s && clicked_t <= *e)
                            .map(|(idx, &(s, e))| (Some(idx), s, e))
                            .or_else(|| {
                                self.active_multi_index.and_then(|idx| {
                                    self.multi_selections
                                        .get(idx)
                                        .map(|&(s, e)| (Some(idx), s, e))
                                })
                            }),
                        SelectionMode::Single => self.selection.map(|(s, e)| {
                            let (s_min, s_max) = (s.min(e), s.max(e));
                            (None, s_min, s_max)
                        }),
                    };
                    if let Some((idx_opt, s, e)) = clicked_slice {
                        self.right_click_hold = Some(crate::audio_preview::RightClickHoldState {
                            start_instant: Instant::now(),
                            target_range: (s, e),
                            slice_index: idx_opt,
                            started_looping: false,
                        });
                    }
                }
            }
        }

        // Advance right-click hold-to-loop
        if let Some(ref mut hold) = self.right_click_hold {
            ui.ctx().request_repaint();
            if is_secondary_down {
                if !hold.started_looping
                    && hold.start_instant.elapsed()
                        >= crate::audio_preview::RIGHT_CLICK_HOLD_TIMEOUT
                {
                    hold.started_looping = true;
                    if let Some(idx) = hold.slice_index {
                        self.active_multi_index = Some(idx);
                    }
                    let (s, e) = hold.target_range;
                    self.play_slice_preview_loop(s, e);
                    self.status_toast = Some((
                        format!(
                            "🔁 Looping slice ({:02}:{:04.1} - {:02}:{:04.1}) [Hold]",
                            (s / 60.0) as u32,
                            s % 60.0,
                            (e / 60.0) as u32,
                            e % 60.0
                        ),
                        now,
                    ));
                }
            }
        }

        // Right-click release handling
        if is_secondary_released || (!is_secondary_down && self.right_click_hold.is_some()) {
            if let Some(hold) = self.right_click_hold.take() {
                if hold.started_looping {
                    self.pause();
                    self.status_toast = Some(("⏹ Stopped loop preview".to_string(), now));
                } else {
                    if let Some(idx) = hold.slice_index {
                        self.active_multi_index = Some(idx);
                    }
                    let (s, e) = hold.target_range;
                    if self.loop_selection {
                        self.play_slice_preview_loop(s, e);
                        self.status_toast = Some((
                            format!(
                                "🔁 Looping slice ({:02}:{:04.1} - {:02}:{:04.1})",
                                (s / 60.0) as u32,
                                s % 60.0,
                                (e / 60.0) as u32,
                                e % 60.0
                            ),
                            now,
                        ));
                    } else {
                        self.play_slice_preview(s, e);
                        self.status_toast = Some((
                            format!(
                                "▶ Previewing slice ({:02}:{:04.1} - {:02}:{:04.1})",
                                (s / 60.0) as u32,
                                s % 60.0,
                                (e / 60.0) as u32,
                                e % 60.0
                            ),
                            now,
                        ));
                    }
                }
            }
        }

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && self.right_click_hold.is_some() {
            if let Some(hold) = self.right_click_hold.take() {
                if hold.started_looping {
                    self.stop();
                }
            }
        }

        if response.drag_started() {
            if let Some(mouse_pos) = response.interact_pointer_pos() {
                if is_middle {
                    // Holding Middle Mouse Button pans the canvas!
                    self.active_drag = Some(DragAction::PanMiddle {
                        start_x: mouse_pos.x,
                        initial_start_sec: self.view_start_sec,
                    });
                } else if is_primary {
                    if scroll_rect.contains(mouse_pos) {
                        self.active_drag = Some(DragAction::PanScrollbar {
                            start_x: mouse_pos.x,
                            initial_start_sec: self.view_start_sec,
                        });
                    } else {
                        let mouse_t =
                            x_to_t(mouse_pos.x, self.view_start_sec, self.view_duration_sec)
                                .clamp(0.0, self.duration_sec);
                        let is_in_ruler = mouse_pos.y <= ruler_rect.max.y;

                        if is_in_ruler {
                            self.active_drag = Some(DragAction::ScrubRuler);
                            self.seek_to(mouse_t);
                        } else {
                            let handle_grab_radius_px = 10.0;
                            let mut captured = false;

                            match self.selection_mode {
                                SelectionMode::Single => {
                                    if let Some((s, e)) = self.selection {
                                        let xs =
                                            t_to_x(s, self.view_start_sec, self.view_duration_sec);
                                        let xe =
                                            t_to_x(e, self.view_start_sec, self.view_duration_sec);
                                        if (mouse_pos.x - xs).abs() <= handle_grab_radius_px {
                                            self.active_drag = Some(DragAction::DragLeftHandle {
                                                other_anchor: e,
                                            });
                                            captured = true;
                                        } else if (mouse_pos.x - xe).abs() <= handle_grab_radius_px
                                        {
                                            self.active_drag = Some(DragAction::DragRightHandle {
                                                other_anchor: s,
                                            });
                                            captured = true;
                                        }
                                    }

                                    if !captured {
                                        self.active_drag = Some(DragAction::CreateSelection {
                                            start_time: mouse_t,
                                            start_pos_x: mouse_pos.x,
                                            has_moved: false,
                                        });
                                        self.seek_to(mouse_t);
                                    }
                                }
                                SelectionMode::Multi => {
                                    // Check handles of existing multi-selections
                                    for (idx, &(s, e)) in self.multi_selections.iter().enumerate() {
                                        let xs =
                                            t_to_x(s, self.view_start_sec, self.view_duration_sec);
                                        let xe =
                                            t_to_x(e, self.view_start_sec, self.view_duration_sec);
                                        if (mouse_pos.x - xs).abs() <= handle_grab_radius_px {
                                            self.active_drag =
                                                Some(DragAction::DragMultiLeftHandle {
                                                    index: idx,
                                                    other_anchor: e,
                                                });
                                            self.active_multi_index = Some(idx);
                                            captured = true;
                                            break;
                                        } else if (mouse_pos.x - xe).abs() <= handle_grab_radius_px
                                        {
                                            self.active_drag =
                                                Some(DragAction::DragMultiRightHandle {
                                                    index: idx,
                                                    other_anchor: s,
                                                });
                                            self.active_multi_index = Some(idx);
                                            captured = true;
                                            break;
                                        }
                                    }

                                    if !captured {
                                        // Check if clicking inside an existing multi-selection slice to select it
                                        for (idx, &(s, e)) in
                                            self.multi_selections.iter().enumerate()
                                        {
                                            if mouse_t >= s && mouse_t <= e {
                                                self.active_multi_index = Some(idx);
                                                captured = true;
                                                break;
                                            }
                                        }
                                    }

                                    if !captured {
                                        let new_idx = self.multi_selections.len();
                                        self.multi_selections.push((mouse_t, mouse_t));
                                        self.region_gain_db
                                            .resize(self.multi_selections.len(), 0.0);
                                        self.region_crossfade_sec
                                            .resize(self.multi_selections.len(), 0.0);
                                        self.region_low_pass_hz
                                            .resize(self.multi_selections.len(), None);
                                        self.region_high_pass_hz
                                            .resize(self.multi_selections.len(), None);
                                        self.active_multi_index = Some(new_idx);
                                        self.active_drag = Some(DragAction::CreateMultiSelection {
                                            start_time: mouse_t,
                                            start_pos_x: mouse_pos.x,
                                            has_moved: false,
                                            index: new_idx,
                                        });
                                        self.seek_to(mouse_t);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else if response.dragged() {
            if let Some(mut drag) = self.active_drag {
                if let Some(mouse_pos) = response.interact_pointer_pos() {
                    let cur_t = x_to_t(mouse_pos.x, self.view_start_sec, self.view_duration_sec)
                        .clamp(0.0, self.duration_sec);
                    let bypass =
                        ui.input(|i| i.modifiers.shift || i.modifiers.ctrl || i.modifiers.command);
                    match drag {
                        DragAction::PanMiddle {
                            start_x,
                            initial_start_sec,
                        } => {
                            let dx = mouse_pos.x - start_x;
                            let dt = (dx / track_width) * self.view_duration_sec;
                            let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                            self.view_start_sec = (initial_start_sec - dt).clamp(0.0, max_start);
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
                        }
                        DragAction::ScrubRuler => {
                            self.seek_to(cur_t);
                        }
                        DragAction::PanScrollbar {
                            start_x,
                            initial_start_sec,
                        } => {
                            let dx = mouse_pos.x - start_x;
                            let dt = (dx / scroll_rect.width()) * self.duration_sec;
                            let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                            self.view_start_sec = (initial_start_sec + dt).clamp(0.0, max_start);
                        }
                        DragAction::DragLeftHandle { other_anchor } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            let snapped_t = self.snap_time(cur_t, bypass);
                            let t1 = snapped_t.min(other_anchor);
                            let t2 = snapped_t.max(other_anchor);
                            self.selection = Some((t1, t2));
                            self.seek_to(snapped_t);
                        }
                        DragAction::DragRightHandle { other_anchor } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            let snapped_t = self.snap_time(cur_t, bypass);
                            let t1 = snapped_t.min(other_anchor);
                            let t2 = snapped_t.max(other_anchor);
                            self.selection = Some((t1, t2));
                            self.seek_to(snapped_t);
                        }
                        DragAction::CreateSelection {
                            start_time,
                            start_pos_x,
                            ref mut has_moved,
                        } => {
                            let dx = (mouse_pos.x - start_pos_x).abs();
                            if *has_moved || dx >= 2.0 {
                                if !*has_moved {
                                    self.clear_single_region_effects();
                                }
                                *has_moved = true;
                                let s_snapped = self.snap_time(start_time, bypass);
                                let cur_snapped = self.snap_time(cur_t, bypass);
                                let t1 = s_snapped.min(cur_snapped);
                                let t2 = s_snapped.max(cur_snapped);
                                self.selection = Some((t1, t2));
                                self.seek_to(cur_snapped);
                            }
                            self.active_drag = Some(drag);
                        }
                        DragAction::DragMultiLeftHandle {
                            index,
                            other_anchor,
                        } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            if index < self.multi_selections.len() {
                                let snapped_t = self.snap_time(cur_t, bypass);
                                let t1 = snapped_t.min(other_anchor);
                                let t2 = snapped_t.max(other_anchor);
                                self.multi_selections[index] = (t1, t2);
                                self.seek_to(snapped_t);
                            }
                        }
                        DragAction::DragMultiRightHandle {
                            index,
                            other_anchor,
                        } => {
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                            if index < self.multi_selections.len() {
                                let snapped_t = self.snap_time(cur_t, bypass);
                                let t1 = snapped_t.min(other_anchor);
                                let t2 = snapped_t.max(other_anchor);
                                self.multi_selections[index] = (t1, t2);
                                self.seek_to(snapped_t);
                            }
                        }
                        DragAction::CreateMultiSelection {
                            start_time,
                            start_pos_x,
                            ref mut has_moved,
                            index,
                        } => {
                            let dx = (mouse_pos.x - start_pos_x).abs();
                            if *has_moved || dx >= 2.0 {
                                *has_moved = true;
                                let s_snapped = self.snap_time(start_time, bypass);
                                let cur_snapped = self.snap_time(cur_t, bypass);
                                let t1 = s_snapped.min(cur_snapped);
                                let t2 = s_snapped.max(cur_snapped);
                                if index < self.multi_selections.len() {
                                    self.multi_selections[index] = (t1, t2);
                                }
                                self.seek_to(cur_snapped);
                            }
                            self.active_drag = Some(drag);
                        }
                    }
                }
            }
        } else if response.drag_stopped() {
            if let Some(drag) = self.active_drag {
                match drag {
                    DragAction::CreateSelection { has_moved, .. } => {
                        if !has_moved {
                            self.selection = None;
                            self.clear_single_region_effects();
                        }
                    }
                    DragAction::CreateMultiSelection {
                        has_moved, index, ..
                    } => {
                        if !has_moved {
                            self.remove_multi_region(index);
                        } else if index < self.multi_selections.len() {
                            let (s, e) = self.multi_selections[index];
                            let (s_fix, e_fix) = (s.min(e), s.max(e));
                            self.multi_selections[index] = (s_fix, e_fix);
                            self.sort_multi_regions();
                            self.active_multi_index = self
                                .multi_selections
                                .iter()
                                .position(|r| *r == (s_fix, e_fix));
                        }
                    }
                    _ => {}
                }
            }
            self.active_drag = None;
        } else if response.middle_clicked() {
            if let Some(pos) = response
                .interact_pointer_pos()
                .filter(|pos| effect_at_position(*pos, wave_rect, spec_rect).is_some())
            {
                let time = x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                    .clamp(0.0, self.duration_sec);
                if self.focus_region_at_time(time) {
                    let changed = self.reset_canvas_effect(
                        effect_at_position(pos, wave_rect, spec_rect),
                        ui.input(|i| i.modifiers.ctrl),
                    );
                    if changed {
                        ui.ctx().request_repaint();
                    }
                }
            }
        } else if response.secondary_clicked() {
            // Shift+Right-click removes/clears selected slice
            if let Some(pos) = response.interact_pointer_pos() {
                let clicked_t = x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                    .clamp(0.0, self.duration_sec);
                let shift_down = ui.input(|i| i.modifiers.shift);

                if shift_down {
                    match self.selection_mode {
                        SelectionMode::Multi => {
                            let clicked_slice = self
                                .multi_selections
                                .iter()
                                .enumerate()
                                .find(|(_, (s, e))| clicked_t >= *s && clicked_t <= *e)
                                .map(|(idx, &(s, e))| (idx, s, e));

                            if let Some((idx, _, _)) = clicked_slice {
                                self.remove_multi_region(idx);
                                self.status_toast =
                                    Some((format!("Removed slice #{}", idx + 1), now));
                            }
                        }
                        SelectionMode::Single => {
                            if let Some((s, e)) = self.selection {
                                let (s_min, s_max) = (s.min(e), s.max(e));
                                let is_inside = clicked_t >= s_min && clicked_t <= s_max;
                                if is_inside {
                                    self.selection = None;
                                    self.clear_single_region_effects();
                                    self.status_toast =
                                        Some(("Cleared selection".to_string(), now));
                                }
                            }
                        }
                    }
                }
            }
        } else if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                if scroll_rect.contains(pos) {
                    let ratio = (pos.x - scroll_rect.min.x) / scroll_rect.width();
                    let target_start = ratio * self.duration_sec - self.view_duration_sec * 0.5;
                    let max_start = (self.duration_sec - self.view_duration_sec).max(0.0);
                    self.view_start_sec = target_start.clamp(0.0, max_start);
                } else {
                    let clicked_t = x_to_t(pos.x, self.view_start_sec, self.view_duration_sec)
                        .clamp(0.0, self.duration_sec);
                    let is_in_ruler = pos.y <= ruler_rect.max.y;
                    if !is_in_ruler {
                        match self.selection_mode {
                            SelectionMode::Single => {
                                self.selection = None;
                                self.clear_single_region_effects();
                            }
                            SelectionMode::Multi => {
                                if let Some(idx) = self
                                    .multi_selections
                                    .iter()
                                    .position(|(s, e)| clicked_t >= *s && clicked_t <= *e)
                                {
                                    self.active_multi_index = Some(idx);
                                } else {
                                    self.active_multi_index = None;
                                }
                            }
                        }
                    }
                    self.seek_to(clicked_t);
                }
            }
        } else if response.double_clicked() {
            match self.selection_mode {
                SelectionMode::Single => {
                    if self.selection.is_some() {
                        self.selection = None;
                        self.clear_single_region_effects();
                    } else {
                        self.selection = Some((0.0, self.duration_sec));
                        self.clear_single_region_effects();
                    }
                }
                SelectionMode::Multi => {
                    if self.multi_selections.is_empty() {
                        self.multi_selections.push((0.0, self.duration_sec));
                        self.region_gain_db = vec![0.0];
                        self.region_crossfade_sec = vec![0.0];
                        self.region_low_pass_hz = vec![None];
                        self.region_high_pass_hz = vec![None];
                        self.active_multi_index = Some(0);
                    } else {
                        self.clear_multi_regions();
                    }
                }
            }
        }

        // --- PAINTING PHASE: ALL MUTATIONS COMPLETED ---
        let v_start = self.view_start_sec;
        let v_dur = self.view_duration_sec.max(0.0001);
        let time_to_x =
            move |t: f32| -> f32 { track_min_x + ((t - v_start) / v_dur) * track_width };
        let x_to_time =
            move |x: f32| -> f32 { v_start + ((x - track_min_x) / track_width) * v_dur };

        let region_edits = self.region_edits();
        let gain_at = |time: f32| {
            region_edits.iter().find_map(|region| {
                let mix = region.effect_mix(time, self.duration_sec);
                (mix > 0.0).then_some((region, mix))
            })
        };
        let waveform_is_edited = self.edited_waveform.is_some();
        let source_analysis = self.analysis.as_ref().unwrap();
        let analysis = self.edited_waveform.as_ref().unwrap_or(source_analysis);

        // 1. RULER
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.min.x, rect.min.y),
                Pos2::new(rect.max.x, ruler_rect.max.y),
            ),
            Rounding {
                nw: 8.0,
                ne: 8.0,
                sw: 0.0,
                se: 0.0,
            },
            bg_frame,
        );
        painter.line_segment(
            [
                Pos2::new(rect.min.x, ruler_rect.max.y),
                Pos2::new(rect.max.x, ruler_rect.max.y),
            ],
            Stroke::new(1.0_f32, border_frame),
        );

        // Dynamic Time Ticks
        let interval_sec = time_ruler_interval(v_dur);

        let first_marker = (v_start / interval_sec).floor() * interval_sec;
        let mut t = first_marker;
        let max_t = v_start + v_dur;
        while t <= max_t + interval_sec * 0.5 {
            if t >= v_start {
                let x = time_to_x(t);
                if x >= track_min_x && x <= track_max_x {
                    // Grid line down through all views
                    painter.line_segment(
                        [
                            Pos2::new(x, ruler_rect.max.y),
                            Pos2::new(x, strip_rect.max.y),
                        ],
                        Stroke::new(
                            1.0_f32,
                            crate::hex_to_c32_alpha(&palette.colors.text_muted, 20),
                        ),
                    );
                    // Ruler tick
                    painter.line_segment(
                        [
                            Pos2::new(x, ruler_rect.min.y + 14.0),
                            Pos2::new(x, ruler_rect.max.y),
                        ],
                        Stroke::new(
                            1.0_f32,
                            crate::hex_to_c32_alpha(&palette.colors.text_secondary, 80),
                        ),
                    );
                    // Label
                    let label = format_time_label(t, interval_sec);
                    painter.text(
                        Pos2::new(x + 3.0, ruler_rect.min.y + 10.0),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(9.0),
                        text_f_med,
                    );
                }
            }
            t += interval_sec;
        }

        // 2. VIEW 1: WAVEFORM (Audacity-style Stems or Peak Bars)
        painter.rect_filled(wave_rect, Rounding::ZERO, bg_frame);
        painter.rect_stroke(
            wave_rect,
            Rounding::ZERO,
            Stroke::new(1.0_f32, border_frame),
        );

        let is_stereo = analysis.channels >= 2 && analysis.ch2_samples.is_some();
        let total_frames = analysis.total_frames;
        let sample_rate = analysis.sample_rate as f32;

        let start_frame = ((v_start * sample_rate).floor() as isize).max(0) as usize;
        let end_frame = (((v_start + v_dur) * sample_rate).ceil() as usize).min(total_frames);
        let visible_frames = end_frame.saturating_sub(start_frame);
        let px_per_frame = track_width / visible_frames.max(1) as f32;

        if is_stereo {
            let half = wave_h / 2.0;
            let y_c1 = wave_rect.min.y + half * 0.5;
            let y_c2 = wave_rect.min.y + half * 1.5;

            painter.line_segment(
                [
                    Pos2::new(track_min_x, wave_rect.min.y + half),
                    Pos2::new(track_max_x, wave_rect.min.y + half),
                ],
                Stroke::new(1.0_f32, border_frame),
            );

            painter.text(
                Pos2::new(track_min_x - 8.0, y_c1),
                egui::Align2::RIGHT_CENTER,
                "L",
                egui::FontId::proportional(11.0),
                accent_base,
            );
            painter.text(
                Pos2::new(track_min_x - 8.0, y_c2),
                egui::Align2::RIGHT_CENTER,
                "R",
                egui::FontId::proportional(11.0),
                accent_base,
            );
        } else {
            painter.text(
                Pos2::new(track_min_x - 8.0, wave_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "Mono",
                egui::FontId::proportional(11.0),
                accent_base,
            );
        }

        if px_per_frame >= 3.0 {
            // --- AUDACITY STEM MODE ---
            let ch_count = if is_stereo { 2 } else { 1 };
            for ch_idx in 0..ch_count {
                let (y_base, max_def, samples, _col) = if ch_idx == 0 {
                    let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                    let base = if is_stereo {
                        wave_rect.min.y + wave_h / 4.0
                    } else {
                        wave_rect.center().y
                    };
                    (base, half * 0.45, &analysis.ch1_samples, accent_base)
                } else {
                    let half = wave_h / 2.0;
                    let base = wave_rect.min.y + 3.0 * wave_h / 4.0;
                    let Some(ch2) = analysis.ch2_samples.as_ref() else {
                        continue;
                    };
                    (base, half * 0.45, ch2, accent_base)
                };

                painter.line_segment(
                    [
                        Pos2::new(track_min_x, y_base),
                        Pos2::new(track_max_x, y_base),
                    ],
                    Stroke::new(1.0_f32, border_frame),
                );

                let mut prev_pos: Option<Pos2> = None;
                for f in start_frame..=end_frame.min(total_frames.saturating_sub(1)) {
                    let t = f as f32 / sample_rate;
                    let x = time_to_x(t);
                    let val = if waveform_is_edited {
                        samples[f]
                    } else {
                        gain_at(t)
                            .map(|(region, mix)| {
                                let wet =
                                    apply_audio_gain(samples[f], region.gain_db, region.soft_clip);
                                samples[f] + (wet - samples[f]) * mix
                            })
                            .unwrap_or(samples[f])
                    };
                    let y = y_base - val * max_def;
                    let pos = Pos2::new(x, y);

                    let in_sel = self.is_time_selected(t);
                    let point_col = if in_sel {
                        accent_base
                    } else {
                        let c = source_analysis.query_sample_color(ch_idx, f);
                        Color32::from_rgb(c[0], c[1], c[2])
                    };

                    if let Some(p_pos) = prev_pos {
                        painter.line_segment([p_pos, pos], Stroke::new(1.5_f32, point_col));
                    }

                    painter.line_segment(
                        [Pos2::new(x, y_base), pos],
                        Stroke::new(
                            1.0_f32,
                            Color32::from_rgba_unmultiplied(
                                point_col.r(),
                                point_col.g(),
                                point_col.b(),
                                120,
                            ),
                        ),
                    );

                    painter.circle_filled(pos, 3.0, point_col);
                    painter.circle_stroke(pos, 3.0, Stroke::new(1.0_f32, border_frame));

                    prev_pos = Some(pos);
                }
            }
        } else {
            // --- OVERVIEW / PEAK ENVELOPE MODE (INSTANTANEOUS VIA PYRAMID + MESH) ---
            let num_bars = (track_width as usize).clamp(100, 1400);
            let bar_w = (track_width / num_bars as f32).max(1.0);

            let ch_count = if is_stereo { 2 } else { 1 };
            for ch_idx in 0..ch_count {
                let (y_base, max_def, _col) = if ch_idx == 0 {
                    let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                    let base = if is_stereo {
                        wave_rect.min.y + wave_h / 4.0
                    } else {
                        wave_rect.center().y
                    };
                    (base, half * 0.45, accent_base)
                } else {
                    let half = wave_h / 2.0;
                    let base = wave_rect.min.y + 3.0 * wave_h / 4.0;
                    (base, half * 0.45, accent_base)
                };

                painter.line_segment(
                    [
                        Pos2::new(track_min_x, y_base),
                        Pos2::new(track_max_x, y_base),
                    ],
                    Stroke::new(1.0_f32, border_frame),
                );

                let mut mesh = egui::Mesh::default();
                for b in 0..num_bars {
                    let bar_x = track_min_x + b as f32 * bar_w;
                    let t_start = x_to_time(bar_x);
                    let t_end = x_to_time(bar_x + bar_w);
                    let t_mid = (t_start + t_end) * 0.5;

                    let f_s = ((t_start * sample_rate) as usize).min(total_frames);
                    let f_e = (((t_end * sample_rate) as usize) + 1).min(total_frames);

                    let (min_s, max_s, block_col) = analysis.query_peaks(ch_idx, f_s, f_e);
                    let (min_s, max_s) = if waveform_is_edited {
                        (min_s, max_s)
                    } else {
                        gain_at(t_mid)
                            .map(|(region, mix)| {
                                let wet_min =
                                    apply_audio_gain(min_s, region.gain_db, region.soft_clip);
                                let wet_max =
                                    apply_audio_gain(max_s, region.gain_db, region.soft_clip);
                                (
                                    min_s + (wet_min - min_s) * mix,
                                    max_s + (wet_max - max_s) * mix,
                                )
                            })
                            .unwrap_or((min_s, max_s))
                    };

                    let in_sel = self.is_time_selected(t_mid);
                    let bar_col = if in_sel {
                        accent_base
                    } else {
                        Color32::from_rgb(block_col[0], block_col[1], block_col[2])
                    };

                    let y_top = y_base - max_s * max_def;
                    let y_bot = y_base - min_s * max_def;
                    let h = (y_bot - y_top).max(1.0);

                    mesh.add_colored_rect(
                        Rect::from_min_size(Pos2::new(bar_x, y_top), Vec2::new(bar_w.max(1.0), h)),
                        bar_col,
                    );
                }
                painter.add(egui::Shape::mesh(mesh));
            }
        }

        // Render transient marks across waveform when enabled
        if self.show_transients {
            for &t_trans in &source_analysis.transients {
                if t_trans >= v_start && t_trans <= v_start + v_dur {
                    let tx = time_to_x(t_trans);
                    // Draw vertical transient marker line
                    painter.line_segment(
                        [
                            Pos2::new(tx, wave_rect.min.y),
                            Pos2::new(tx, wave_rect.max.y),
                        ],
                        Stroke::new(1.0_f32, transient_col),
                    );
                    // Draw transient marker pip/diamond at top of waveform
                    let pip_top = Pos2::new(tx, wave_rect.min.y + 1.0);
                    let pip_left = Pos2::new(tx - 3.5, wave_rect.min.y + 5.5);
                    let pip_right = Pos2::new(tx + 3.5, wave_rect.min.y + 5.5);
                    let pip_bot = Pos2::new(tx, wave_rect.min.y + 10.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![pip_top, pip_right, pip_bot, pip_left],
                        transient_col,
                        Stroke::new(0.5_f32, border_frame),
                    ));
                }
            }
        }

        // 3. VIEW 2: SPECTROGRAM (Spectral Energy Heatmap Over Time)
        let total_dur = self.duration_sec.max(0.01);
        let fft_size = if total_dur >= 600.0 { 512 } else { 1024 };
        let nyquist = analysis.sample_rate as f32 * 0.5;
        let cfg = HfSpectrogramConfig {
            fft_size,
            gain_db: 0.0,
            range_db: 80.0,
            contrast: 1.0,
        };

        render_and_paint_spectrogram(
            &mut self.inspect_viewport_cache,
            ctx,
            &painter,
            format!("inspect_vp:{}", self.path.display()),
            spec_rect,
            track_min_x,
            track_max_x,
            track_min_x - 8.0,
            v_start,
            v_dur,
            total_dur,
            analysis.sample_rate,
            &analysis.ch1_samples,
            self.spectrogram_texture.as_ref(),
            cfg,
            bg_frame,
            text_f_med,
            Stroke::new(1.0_f32, border_frame),
            12.0,
        );
        let cutoff_painter = painter.with_clip_rect(spec_rect);
        for region in &region_edits {
            let x_start = time_to_x(region.start_sec).clamp(track_min_x, track_max_x);
            let x_end = time_to_x(region.end_sec).clamp(track_min_x, track_max_x);
            if x_end <= x_start {
                continue;
            }
            for (kind, cutoff, color) in [
                ("LP", region.low_pass_hz, hex_to_c32(&palette.diff_add)),
                ("HP", region.high_pass_hz, hex_to_c32(&palette.diff_mod)),
            ] {
                if let Some(hz) = cutoff {
                    let y = spec_rect.max.y - (hz / nyquist).clamp(0.0, 1.0) * spec_rect.height();
                    cutoff_painter.line_segment(
                        [Pos2::new(x_start, y), Pos2::new(x_end, y)],
                        Stroke::new(2.0_f32, color),
                    );
                    cutoff_painter.text(
                        Pos2::new(x_start + 4.0, y - 4.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{kind} {hz:.0} Hz"),
                        egui::FontId::monospace(10.0),
                        color,
                    );
                }
            }
        }
        painter.rect_stroke(
            spec_rect,
            Rounding::ZERO,
            Stroke::new(1.0_f32, border_frame),
        );

        // 4. VIEW 3: COLORCHORD 2-D MULTI-NOTE CHROMATIC RIBBON
        painter.text(
            Pos2::new(track_min_x - 8.0, strip_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "♫ Chord",
            egui::FontId::proportional(10.0),
            text_f_med,
        );

        if let Some(ref tex) = self.note_strip_texture {
            let total_dur = self.duration_sec.max(0.01);
            let u_start = (v_start / total_dur).clamp(0.0, 1.0);
            let u_end = ((v_start + v_dur) / total_dur).clamp(0.0, 1.0);
            let uv = Rect::from_min_max(Pos2::new(u_start, 0.0), Pos2::new(u_end, 1.0));
            // Paint theme background under texture so silent/empty note regions match active theme
            painter.rect_filled(strip_rect, Rounding::ZERO, bg_frame);
            painter.image(tex.id(), strip_rect, uv, Color32::WHITE);
        } else {
            painter.rect_filled(strip_rect, Rounding::ZERO, bg_frame);
            painter.text(
                strip_rect.center(),
                egui::Align2::CENTER_CENTER,
                "⏳ Analyzing Multi-Note Chords...",
                egui::FontId::proportional(12.0),
                text_f_low,
            );
        }
        painter.rect_stroke(
            strip_rect,
            Rounding::ZERO,
            Stroke::new(1.0_f32, border_frame),
        );

        // Hover tooltip over ColorChord note strip (multi-note chord display)
        if let Some(mouse_pos) = response.hover_pos() {
            if strip_rect.contains(mouse_pos) {
                let mouse_t = x_to_time(mouse_pos.x);
                let total_dur = self.duration_sec.max(0.01);
                let col_idx = (((mouse_t / total_dur) * (source_analysis.note_strip_width as f32))
                    .floor() as usize)
                    .min(source_analysis.note_strip_info.len().saturating_sub(1));
                if let Some(info) = source_analysis.note_strip_info.get(col_idx) {
                    let text = if !info.chord_notes.is_empty() {
                        if info.chord_notes.len() > 1 {
                            let names = info
                                .chord_notes
                                .iter()
                                .map(|n| n.note_name.as_str())
                                .collect::<Vec<_>>()
                                .join(" + ");
                            format!(
                                "🎵 Detected Chord: {} • Energy: {:.0}%",
                                names,
                                info.amplitude * 100.0
                            )
                        } else {
                            format!(
                                "🎵 Musical Note: {} ({:.1} Hz) • Energy: {:.0}%",
                                info.note_name,
                                info.frequency_hz,
                                info.amplitude * 100.0
                            )
                        }
                    } else if info.active {
                        format!(
                            "🎵 Musical Note: {} ({:.1} Hz) • Energy: {:.0}%",
                            info.note_name,
                            info.frequency_hz,
                            info.amplitude * 100.0
                        )
                    } else {
                        "🎵 Musical Note: — (Silence / Below Threshold)".to_string()
                    };
                    egui::show_tooltip(ui.ctx(), response.id.with("note_tt"), |ui| {
                        ui.label(
                            egui::RichText::new(text)
                                .strong()
                                .color(crate::hex_to_c32(&palette.colors.text_primary)),
                        );
                    });
                }
            }
        }

        // 5. HORIZONTAL VIEWPORT SCROLLBAR / PAN TRACK
        painter.rect_filled(scroll_rect, Rounding::same(3.0), bg_frame);
        painter.rect_stroke(
            scroll_rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, border_frame),
        );

        let total_dur = self.duration_sec.max(0.01);
        let thumb_x = scroll_rect.min.x + (v_start / total_dur) * scroll_rect.width();
        let thumb_w = ((v_dur / total_dur) * scroll_rect.width()).clamp(16.0, scroll_rect.width());
        let thumb_rect = Rect::from_min_size(
            Pos2::new(thumb_x, scroll_rect.min.y),
            Vec2::new(thumb_w, scroll_rect.height()),
        );

        painter.rect_filled(thumb_rect, Rounding::same(3.0), border_frame);
        painter.rect_stroke(
            thumb_rect,
            Rounding::same(3.0),
            Stroke::new(1.0_f32, text_f_med),
        );

        // 6. UNIFIED SELECTION OVERLAY ACROSS ALL 3 VIEWS
        let view_top_y = ruler_rect.max.y;
        let view_bot_y = strip_rect.max.y;

        match self.selection_mode {
            SelectionMode::Single => {
                if let Some((start, end)) = self.selection {
                    let x_start = time_to_x(start);
                    let x_end = time_to_x(end);

                    if x_end > x_start {
                        let sel_rect = Rect::from_min_max(
                            Pos2::new(x_start.max(track_min_x), view_top_y),
                            Pos2::new(x_end.min(track_max_x), view_bot_y),
                        );
                        painter.rect_filled(
                            sel_rect,
                            Rounding::ZERO,
                            crate::hex_to_c32_alpha(&palette.harmonies.base, 40),
                        );

                        if x_start >= track_min_x && x_start <= track_max_x {
                            painter.line_segment(
                                [
                                    Pos2::new(x_start, ruler_rect.min.y),
                                    Pos2::new(x_start, view_bot_y),
                                ],
                                Stroke::new(2.5_f32, accent_base),
                            );
                            painter.circle_filled(
                                Pos2::new(x_start, ruler_rect.center().y),
                                5.0,
                                accent_base,
                            );
                        }
                        if x_end >= track_min_x && x_end <= track_max_x {
                            painter.line_segment(
                                [
                                    Pos2::new(x_end, ruler_rect.min.y),
                                    Pos2::new(x_end, view_bot_y),
                                ],
                                Stroke::new(2.5_f32, accent_base),
                            );
                            painter.circle_filled(
                                Pos2::new(x_end, ruler_rect.center().y),
                                5.0,
                                accent_base,
                            );
                        }
                    }
                }
            }
            SelectionMode::Multi => {
                for (idx, &(start, end)) in self.multi_selections.iter().enumerate() {
                    let is_active = self.active_multi_index == Some(idx);
                    let x_start = time_to_x(start);
                    let x_end = time_to_x(end);

                    if x_end > x_start {
                        let sel_rect = Rect::from_min_max(
                            Pos2::new(x_start.max(track_min_x), view_top_y),
                            Pos2::new(x_end.min(track_max_x), view_bot_y),
                        );
                        let alpha = if is_active { 55 } else { 30 };
                        painter.rect_filled(
                            sel_rect,
                            Rounding::ZERO,
                            crate::hex_to_c32_alpha(&palette.harmonies.base, alpha),
                        );

                        let handle_color = accent_base;
                        let stroke_width = if is_active { 2.5_f32 } else { 1.5_f32 };

                        if x_start >= track_min_x && x_start <= track_max_x {
                            painter.line_segment(
                                [
                                    Pos2::new(x_start, ruler_rect.min.y),
                                    Pos2::new(x_start, view_bot_y),
                                ],
                                Stroke::new(stroke_width, handle_color),
                            );
                            painter.circle_filled(
                                Pos2::new(x_start, ruler_rect.center().y),
                                if is_active { 5.0 } else { 3.5 },
                                handle_color,
                            );
                        }
                        if x_end >= track_min_x && x_end <= track_max_x {
                            painter.line_segment(
                                [
                                    Pos2::new(x_end, ruler_rect.min.y),
                                    Pos2::new(x_end, view_bot_y),
                                ],
                                Stroke::new(stroke_width, handle_color),
                            );
                            painter.circle_filled(
                                Pos2::new(x_end, ruler_rect.center().y),
                                if is_active { 5.0 } else { 3.5 },
                                handle_color,
                            );
                        }

                        // Badge on ruler top showing slice index "#1", "#2"
                        if x_start <= track_max_x && x_end >= track_min_x {
                            let badge_x = (x_start + 4.0)
                                .max(track_min_x + 2.0)
                                .min(track_max_x - 30.0);
                            let badge_rect = Rect::from_min_size(
                                Pos2::new(badge_x, ruler_rect.min.y + 3.0),
                                Vec2::new(26.0, 14.0),
                            );
                            let badge_bg = if is_active { border_frame } else { bg_frame };
                            painter.rect_filled(badge_rect, Rounding::same(3.0), badge_bg);
                            painter.text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("#{}", idx + 1),
                                egui::FontId::monospace(9.0),
                                if is_active { accent_base } else { text_f_med },
                            );
                        }
                    }
                }
            }
        }

        // Show the outward crossfade extent only for the region under the pointer.
        self.paint_hovered_region_crossfade(
            &painter,
            &response,
            hex_to_c32(&palette.diff_mod),
            ruler_rect,
            view_top_y,
            view_bot_y,
            track_min_x,
            track_max_x,
            track_width,
        );

        // 7. UNIFIED PLAYHEAD CURSOR ACROSS ALL 3 VIEWS
        let playhead_x = time_to_x(self.current_time_sec);
        if playhead_x >= track_min_x && playhead_x <= track_max_x {
            painter.line_segment(
                [
                    Pos2::new(playhead_x, ruler_rect.min.y),
                    Pos2::new(playhead_x, view_bot_y),
                ],
                Stroke::new(2.0_f32, accent_playhead),
            );
            let head_top = Pos2::new(playhead_x, ruler_rect.min.y);
            let diamond = [
                Pos2::new(head_top.x - 5.0, head_top.y),
                Pos2::new(head_top.x, head_top.y + 12.0),
                Pos2::new(head_top.x + 5.0, head_top.y),
                Pos2::new(head_top.x, head_top.y - 4.0),
            ];
            painter.add(egui::Shape::convex_polygon(
                diamond.to_vec(),
                accent_playhead,
                Stroke::NONE,
            ));
        }

        // Precise Scrub Time Badge during ruler drag
        if matches!(self.active_drag, Some(DragAction::ScrubRuler)) {
            let badge_pos = Pos2::new(
                (playhead_x + 8.0).min(track_max_x - 65.0),
                ruler_rect.min.y + 26.0,
            );
            let cur_m = (self.current_time_sec / 60.0) as u32;
            let cur_s = self.current_time_sec % 60.0;
            let badge_text = format!("{:02}:{:05.2}", cur_m, cur_s);
            painter.rect_filled(
                Rect::from_min_size(badge_pos, Vec2::new(65.0, 18.0)),
                Rounding::same(4.0),
                crate::hex_to_c32_alpha(&palette.colors.surface_recessed, 220),
            );
            painter.text(
                Pos2::new(badge_pos.x + 4.0, badge_pos.y + 9.0),
                egui::Align2::LEFT_CENTER,
                badge_text,
                egui::FontId::monospace(10.0),
                crate::hex_to_c32(&palette.diff_del),
            );
        }

        // Dynamic Cursor Feedback
        let is_middle_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Middle));
        if is_middle_down || matches!(self.active_drag, Some(DragAction::PanMiddle { .. })) {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Grabbing);
        } else if let Some(hover_pos) = response.hover_pos() {
            let is_in_ruler = hover_pos.y <= ruler_rect.max.y;
            let is_in_scroll = scroll_rect.contains(hover_pos);
            if is_in_ruler || is_in_scroll {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
            } else {
                let mut near_handle = false;
                match self.selection_mode {
                    SelectionMode::Single => {
                        if let Some((s, e)) = self.selection {
                            let xs = time_to_x(s);
                            let xe = time_to_x(e);
                            if (hover_pos.x - xs).abs() <= 6.0 || (hover_pos.x - xe).abs() <= 6.0 {
                                near_handle = true;
                            }
                        }
                    }
                    SelectionMode::Multi => {
                        for &(s, e) in &self.multi_selections {
                            let xs = time_to_x(s);
                            let xe = time_to_x(e);
                            if (hover_pos.x - xs).abs() <= 6.0 || (hover_pos.x - xe).abs() <= 6.0 {
                                near_handle = true;
                                break;
                            }
                        }
                    }
                }

                if near_handle {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeHorizontal);
                } else {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
                }
            }
        }
    }
}
