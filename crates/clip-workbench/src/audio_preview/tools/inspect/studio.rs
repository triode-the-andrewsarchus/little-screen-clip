use super::*;

impl AudioPreviewState {
    pub(super) fn render_normalize_button(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        now: f64,
    ) {
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

    pub(super) fn render_track_gain_summary(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.track_gain_db != 0.0 {
            ui.label(format!("Track gain {:+.1} dB", self.track_gain_db));
            layout::AudioWorkspace::section_divider(ui, palette);
        }
    }

    pub(super) fn handle_normalize_shortcut(
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
            match rx.try_recv() {
                Ok(res) => {
                    if let Some(dir) = res {
                        self.save_destination_dir = dir;
                    }
                    self.browse_rx = None;
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
                Err(TryRecvError::Disconnected) => self.browse_rx = None,
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
            {
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
                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS));

                if ui.add(play_btn).clicked() {
                    self.toggle_play();
                }

                ui.add_space(3.0);

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

                ui.add_space(crate::ui_ext::SPACE_MD);

                if ui
                    .add(layout::AudioWorkspace::secondary_button_icon(
                        Some(crate::icons::Icon::ZoomFit),
                        "Fit",
                        hex_to_c32(&palette.colors.surface_recessed),
                        palette,
                    ))
                    .on_hover_text("Fit Track / Window (Ctrl+0)")
                    .clicked()
                {
                    self.fit_to_window();
                }
            }

            layout::AudioWorkspace::section_divider(ui, palette);
            {
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
                        ui.add_space(crate::ui_ext::SPACE_XS);
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
                            self.set_transient_sensitivity(
                                transient_sensitivity_from_slider(sensitivity_position),
                            );
                        }
                    }
                } else {
                    crate::ui_ext::on_hover_text_with_disabled_reason(
                        ui.add_enabled(false, trans_btn),
                        "Toggle transient marks and magnetic snapping for selections. Press [T] to toggle.",
                        "Calculating audio spectral flux transients...",
                    );
                }

                ui.add_space(crate::ui_ext::SPACE_SM);

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
            }

            layout::AudioWorkspace::section_divider(ui, palette);
            {
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
                        .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS));

                        if ui
                            .add(copy_btn)
                            .on_hover_text("Copy selected segment with all original channels")
                            .clicked()
                        {
                            self.copy_selected_segment(now, ChannelCopyMode::Native);
                        }

                        // Channel-Specific Copy Buttons (Left only mono, Right only mono)
                        if channels_count >= 2 {
                            ui.add_space(crate::ui_ext::SPACE_XS);
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

                        ui.add_space(crate::ui_ext::SPACE_XS);
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
                        .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS));
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
                        .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS));

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
                            ui.add_space(crate::ui_ext::SPACE_XS);
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

                        ui.add_space(crate::ui_ext::SPACE_XS);
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
                        .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS));
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

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                let has_analysis = self.analysis.is_some();

                let copy_wave_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Copy),
                    "Copy Wave .png",
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

                ui.add_space(crate::ui_ext::SPACE_XS);

                let copy_spec_btn = layout::AudioWorkspace::secondary_button_icon(
                    Some(crate::icons::Icon::Copy),
                    "Copy FFT .png",
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
            }
        });

        self.render_selection_info_bar(ui, palette, now);

        ui.add_space(crate::ui_ext::SPACE_SM);

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
                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
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
}
