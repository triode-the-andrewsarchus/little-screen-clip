use super::*;

impl AudioPreviewState {
    pub(in crate::audio_preview) fn paint_channel_canvas(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        palette: &AppPalette,
        track: Rect,
    ) {
        let [ruler, _, _, _, scroll] = layout::AudioWorkspace::inspect_timeline_rects(track);
        self.poll_derived_signals(ctx);
        if self.analysis.is_none() {
            return;
        }
        if let (Some(source), Some(derived)) = (
            self.edited_waveform.as_ref().or(self.analysis.as_ref()),
            self.channel_tools.derived.as_ref(),
        ) && let (Some(right), Some(diff)) =
            (source.ch2_samples.as_ref(), derived.ch2_samples.as_ref())
        {
            self.channel_tools.spectra.update(
                ctx,
                [
                    source.ch1_samples.clone(),
                    right.clone(),
                    derived.ch1_samples.clone(),
                    diff.clone(),
                ],
                source.sample_rate,
                self.view_start_sec,
                self.view_duration_sec,
                track.width(),
            );
        }
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        let start = self.view_start_sec;
        let duration = self.view_duration_sec.max(0.0001);
        let time_x = |time: f32| track.min.x + (time - start) / duration * track.width();
        let interval = time_ruler_interval(duration);
        let mut time = (start / interval).ceil() * interval;
        while time <= start + duration {
            let x = time_x(time);
            painter.text(
                Pos2::new(x, ruler.center().y),
                egui::Align2::CENTER_CENTER,
                format_time_label(time, interval),
                egui::FontId::monospace(11.0),
                colors.text_secondary,
            );
            time += interval;
        }
        let plots = Rect::from_min_max(
            Pos2::new(track.min.x, ruler.max.y + 4.0),
            Pos2::new(track.max.x, scroll.min.y - 4.0),
        );
        let labels = [
            "L",
            "R",
            "L+R",
            if self.channel_tools.reverse_difference {
                "R−L"
            } else {
                "L−R"
            },
        ];
        for (index, label) in labels.into_iter().enumerate() {
            self.paint_channel_row(painter, palette, plots, index, label);
        }
        self.paint_channel_overlay(painter, palette, plots, ruler);
        painter.rect_filled(scroll, Rounding::same(3.0), colors.recessed);
        let total = self.duration_sec.max(0.01);
        let thumb = Rect::from_min_size(
            Pos2::new(scroll.min.x + start / total * scroll.width(), scroll.min.y),
            Vec2::new(
                (duration / total * scroll.width()).clamp(1.0, scroll.width()),
                scroll.height(),
            ),
        );
        painter.rect_filled(thumb, Rounding::same(3.0), colors.panel);
    }
    fn paint_channel_row(
        &mut self,
        painter: &egui::Painter,
        palette: &AppPalette,
        plots: Rect,
        index: usize,
        label: &str,
    ) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        let start = self.view_start_sec;
        let duration = self.view_duration_sec.max(0.0001);
        let track = plots;
        let row_height = plots.height() / 4.0;
        let header = channel_header_rect(plots, index);
        let top = header.max.y + 2.0;
        let wave = Rect::from_min_max(
            Pos2::new(track.min.x, top),
            Pos2::new(
                track.max.x,
                top + (row_height - header.height() - 2.0) * 0.45,
            ),
        );
        let spectrum = Rect::from_min_max(
            Pos2::new(track.min.x, wave.max.y + 2.0),
            Pos2::new(
                track.max.x,
                plots.min.y + (index + 1) as f32 * row_height - 4.0,
            ),
        );
        painter.rect_filled(wave, Rounding::ZERO, colors.recessed);
        painter.text(
            Pos2::new(track.min.x - 8.0, wave.center().y),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::monospace(12.0),
            colors.text_primary,
        );
        let analysis = if index < 2 {
            Some(
                self.edited_waveform
                    .as_ref()
                    .or(self.analysis.as_ref())
                    .unwrap()
                    .as_ref(),
            )
        } else {
            self.channel_tools.derived.as_ref()
        };
        if let Some(analysis) = analysis {
            let channel = index % 2;
            let sign = if index == 3 && self.channel_tools.reverse_difference {
                -1.0
            } else {
                1.0
            };
            let wave_painter = painter.with_clip_rect(wave);
            let columns = wave.width().ceil().max(1.0) as usize;
            for column in 0..columns {
                let first = ((start + column as f32 / columns as f32 * duration).max(0.0)
                    * analysis.sample_rate as f32) as usize;
                let last = ((start + (column + 1) as f32 / columns as f32 * duration).max(0.0)
                    * analysis.sample_rate as f32) as usize;
                let (min, max, rgb) = analysis.query_peaks(channel, first, last.max(first + 1));
                let x = wave.min.x + column as f32;
                // Derived signals have a ±2 range; original channels use ±1.
                let scale = wave.height() * if index < 2 { 0.45 } else { 0.225 };
                wave_painter.line_segment(
                    [
                        Pos2::new(x, wave.center().y - min * sign * scale),
                        Pos2::new(x, wave.center().y - max * sign * scale),
                    ],
                    Stroke::new(1.0_f32, Color32::from_rgb(rgb[0], rgb[1], rgb[2])),
                );
            }
            paint_spectrogram_surface(
                painter,
                spectrum,
                self.channel_tools.spectra.textures[index].as_ref(),
                None,
                start,
                duration,
                self.duration_sec,
                colors.recessed,
                colors.text_secondary,
            );
            paint_frequency_axis_and_grid(
                painter,
                spectrum,
                track.min.x,
                track.max.x,
                track.min.x - 8.0,
                analysis.sample_rate,
                Stroke::new(1.0_f32, colors.panel),
                colors.text_secondary,
                10.0,
            );
        } else {
            painter.text(
                wave.center(),
                egui::Align2::CENTER_CENTER,
                "Building signal…",
                egui::FontId::proportional(12.0),
                colors.text_secondary,
            );
        }
        painter.line_segment(
            [wave.left_center(), wave.right_center()],
            Stroke::new(0.5_f32, colors.panel),
        );
        if channel_row_dimmed(self.channel_tools.solo, index) {
            let body = Rect::from_min_max(
                Pos2::new(plots.min.x - 44.0, top),
                Pos2::new(plots.max.x, spectrum.max.y),
            );
            painter.rect_filled(body, Rounding::ZERO, colors.canvas.linear_multiply(0.72));
        }
    }
    fn paint_channel_overlay(
        &self,
        painter: &egui::Painter,
        palette: &AppPalette,
        plots: Rect,
        ruler: Rect,
    ) {
        let accent = hex_to_c32(&palette.harmonies.base);
        let start = self.view_start_sec;
        let duration = self.view_duration_sec.max(0.0001);
        let time_x = |time: f32| plots.min.x + (time - start) / duration * plots.width();
        let track = plots;
        let overlay = painter.with_clip_rect(plots);
        let ranges: Vec<_> = match self.selection_mode {
            SelectionMode::Single => self.selection.into_iter().collect(),
            SelectionMode::Multi => self.multi_selections.clone(),
        };
        for (start, end) in ranges {
            let (start, end) = AudioSelectionModel::normalize((start, end));
            let left = time_x(start);
            let right = time_x(end);
            overlay.rect_filled(
                Rect::from_min_max(Pos2::new(left, plots.min.y), Pos2::new(right, plots.max.y)),
                Rounding::ZERO,
                hex_to_c32_alpha(&palette.harmonies.base, 40),
            );
            for x in [left, right] {
                painter.line_segment(
                    [Pos2::new(x, ruler.min.y), Pos2::new(x, plots.max.y)],
                    Stroke::new(2.0_f32, accent),
                );
            }
        }
        if self.show_transients {
            for &time in self.current_transient_markers() {
                if time >= start && time <= start + duration {
                    let x = time_x(time);
                    overlay.line_segment(
                        [Pos2::new(x, plots.min.y), Pos2::new(x, plots.max.y)],
                        Stroke::new(0.5_f32, hex_to_c32(&palette.diff_mod)),
                    );
                }
            }
        }
        let x = time_x(self.current_time_sec);
        if x >= track.min.x && x <= track.max.x {
            painter.line_segment(
                [Pos2::new(x, ruler.min.y), Pos2::new(x, plots.max.y)],
                Stroke::new(2.0_f32, hex_to_c32(&palette.harmonies.complementary)),
            );
        }
    }
}

fn channel_header_rect(plots: Rect, index: usize) -> Rect {
    let row_height = plots.height() / 4.0;
    let header_height = crate::ui_ext::CONTROL_HEIGHT.min(row_height * 0.5);
    Rect::from_min_size(
        Pos2::new(plots.min.x, plots.min.y + index as f32 * row_height),
        Vec2::new(plots.width(), header_height),
    )
}

pub(super) fn channel_row_dimmed(solo: u64, index: usize) -> bool {
    match solo {
        1 => index != 2,
        2 => index != 3,
        _ => false,
    }
}

impl AudioPreviewState {
    pub(in crate::audio_preview) fn render_channel_headers(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        track: Rect,
    ) -> bool {
        let [ruler, _, _, _, scroll] = layout::AudioWorkspace::inspect_timeline_rects(track);
        let plots = Rect::from_min_max(
            Pos2::new(track.min.x, ruler.max.y + 4.0),
            Pos2::new(track.max.x, scroll.min.y - 4.0),
        );
        let mut hovered = false;
        for (index, label, mode) in [
            (0, "Stereo [0]", 0),
            (2, "Solo sum [7]", 1),
            (3, "Solo diff [8]", 2),
        ] {
            let header = channel_header_rect(plots, index);
            hovered |= ui.input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| header.contains(pos))
            });
            ui.allocate_ui_at_rect(header, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            layout::AudioWorkspace::toggle_button(
                                label,
                                self.channel_tools.solo == mode,
                                palette,
                            )
                            .min_size(Vec2::new(0.0, header.height())),
                        )
                        .clicked()
                    {
                        self.channel_tools.solo = if self.channel_tools.solo == mode {
                            0
                        } else {
                            mode
                        };
                    }
                    if index == 3
                        && ui
                            .add(
                                layout::AudioWorkspace::toggle_button(
                                    if self.channel_tools.reverse_difference {
                                        "R−L [9]"
                                    } else {
                                        "L−R [9]"
                                    },
                                    self.channel_tools.reverse_difference,
                                    palette,
                                )
                                .min_size(Vec2::new(0.0, header.height())),
                            )
                            .clicked()
                    {
                        self.channel_tools.reverse_difference =
                            !self.channel_tools.reverse_difference;
                    }
                });
            });
        }
        self.sync_channel_monitor();
        hovered
    }
}
