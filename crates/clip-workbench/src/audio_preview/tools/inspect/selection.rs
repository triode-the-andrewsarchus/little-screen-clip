use super::*;

impl AudioPreviewState {
    pub(super) fn render_crossfade_label(ui: &mut egui::Ui, crossfade_sec: f32) {
        if crossfade_sec > 0.0 {
            ui.label(format!("Crossfade {:.0} ms", crossfade_sec * 1000.0));
        }
    }

    pub(super) fn render_selection_info_bar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        now: f64,
    ) {
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
                ui.add_space(crate::ui_ext::SPACE_XS);
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
                                .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
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
                                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
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

    pub(super) fn handle_crossfade_scroll(
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

    pub(super) fn reset_canvas_effect(
        &mut self,
        effect: Option<CanvasEffect>,
        crossfade: bool,
    ) -> bool {
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
}
