use super::*;

impl ScreenRenderer {
    pub(super) fn draw_audio_waveform(
        &self,
        pixmap: &mut Pixmap,
        duration_ms: u64,
        sample_rate: u32,
        channels: u16,
        format: &str,
        waveform: Option<&little_clip_core::AudioWaveform>,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        _is_hero: bool,
    ) {
        let content_x = rx + 16.0;
        let content_w = rw - 32.0;
        let content_y = ry + 120.0;
        let content_h = (rh - 145.0).max(80.0);

        let mins = duration_ms / 60_000;
        let secs = (duration_ms % 60_000) / 1000;
        let dur_text = format!("{:02}:{:02}", mins, secs);

        let wf = match waveform {
            Some(w) if w.ch1_peaks.is_empty() => {
                self.draw_text(
                    pixmap,
                    "[WAVEFORM UNAVAILABLE]",
                    content_x,
                    content_y + 30.0,
                    20.0,
                    self.color_from_hex(&self.palette.colors.text_secondary),
                    false,
                );
                self.draw_text(
                    pixmap,
                    &format!(
                        "{:.1} kHz • {} ({})",
                        sample_rate as f64 / 1000.0,
                        if channels == 1 {
                            "Mono"
                        } else if channels == 2 {
                            "Stereo"
                        } else {
                            "Multi-ch"
                        },
                        format
                    ),
                    content_x,
                    content_y + 65.0,
                    18.0,
                    self.color_from_hex(&self.palette.colors.text_muted),
                    false,
                );
                return;
            }
            Some(w) => w,
            None => {
                // Waveform is being analyzed in background
                self.draw_text(
                    pixmap,
                    "ANALYZING AUDIO...",
                    content_x,
                    content_y + 30.0,
                    22.0,
                    self.color_from_hex(&self.palette.harmonies.base),
                    true,
                );
                self.draw_text(
                    pixmap,
                    &format!(
                        "Decoding {} at {:.1} kHz",
                        format,
                        sample_rate as f64 / 1000.0
                    ),
                    content_x,
                    content_y + 65.0,
                    18.0,
                    self.color_from_hex(&self.palette.colors.text_muted),
                    false,
                );
                if let Some(r) = Rect::from_xywh(content_x, content_y + 110.0, content_w, 3.0) {
                    let mut paint = Paint::default();
                    paint.set_color(Self::hex_to_color_alpha(&self.palette.harmonies.base, 140));
                    pixmap.fill_rect(r, &paint, Transform::identity(), None);
                }
                return;
            }
        };

        let has_dual_channel = wf.channels >= 2 && wf.ch2_peaks.is_some();
        let total_peaks = wf.ch1_peaks.len();
        if total_peaks == 0 {
            return;
        }

        let bar_spacing = 3.0f32;
        let num_bars = ((content_w / bar_spacing).floor() as usize)
            .max(10)
            .min(total_peaks);
        let bar_width = (content_w / num_bars as f32 * 0.72).clamp(1.5, 6.0);

        if has_dual_channel {
            // Dual Channel Layout (Left Channel Top, Right Channel Bottom)
            let ch_h = (content_h - 28.0) / 2.0;
            let ch1_center = content_y + ch_h / 2.0;
            let ch2_center = content_y + ch_h + 14.0 + ch_h / 2.0;

            // Channel labels
            self.draw_text(
                pixmap,
                "L",
                content_x,
                content_y + 10.0,
                14.0,
                self.color_from_hex(&self.palette.colors.text_muted),
                true,
            );
            self.draw_text(
                pixmap,
                "R",
                content_x,
                content_y + ch_h + 24.0,
                14.0,
                self.color_from_hex(&self.palette.colors.text_muted),
                true,
            );

            // Center divider line (theme-adaptive)
            let div_y = content_y + ch_h + 7.0;
            if let Some(r) = Rect::from_xywh(content_x + 16.0, div_y, content_w - 16.0, 1.0) {
                let mut paint = Paint::default();
                paint.set_color(Self::hex_to_color_alpha(
                    &self.palette.colors.surface_panel,
                    120,
                ));
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
            }

            let ch2_ref = wf.ch2_peaks.as_ref().unwrap();

            for b in 0..num_bars {
                let sample_idx = (b as f32 / num_bars as f32 * total_peaks as f32) as usize;
                let p1 = wf.ch1_peaks.get(sample_idx).copied().unwrap_or(0.04);
                let p2 = ch2_ref.get(sample_idx).copied().unwrap_or(0.04);

                let color = if let Some(ref colors) = wf.spectral_colors {
                    let [r, g, b] = colors.get(sample_idx).copied().unwrap_or([34, 197, 94]);
                    Color::from_rgba8(r, g, b, 235)
                } else {
                    // Plain waveform (Phase 1)
                    Color::from_rgba8(34, 197, 94, 220)
                };

                let bx = content_x + 18.0 + (b as f32 * ((content_w - 20.0) / num_bars as f32));

                // Channel 1 bar
                let h1 = (p1 * (ch_h * 0.46)).max(1.5);
                if let Some(rect1) = Rect::from_xywh(bx, ch1_center - h1, bar_width, h1 * 2.0) {
                    let mut paint = Paint::default();
                    paint.set_color(color);
                    pixmap.fill_rect(rect1, &paint, Transform::identity(), None);
                }

                // Channel 2 bar
                let h2 = (p2 * (ch_h * 0.46)).max(1.5);
                if let Some(rect2) = Rect::from_xywh(bx, ch2_center - h2, bar_width, h2 * 2.0) {
                    let mut paint = Paint::default();
                    paint.set_color(color);
                    pixmap.fill_rect(rect2, &paint, Transform::identity(), None);
                }
            }
        } else {
            // Single Mono Waveform (Centered)
            let center_y = content_y + content_h / 2.0 - 10.0;
            let max_h = (content_h / 2.0 - 16.0).max(10.0);

            // Subtle center axis (theme-adaptive)
            if let Some(r) = Rect::from_xywh(content_x, center_y, content_w, 1.0) {
                let mut paint = Paint::default();
                paint.set_color(Self::hex_to_color_alpha(
                    &self.palette.colors.surface_panel,
                    120,
                ));
                pixmap.fill_rect(r, &paint, Transform::identity(), None);
            }

            for b in 0..num_bars {
                let sample_idx = (b as f32 / num_bars as f32 * total_peaks as f32) as usize;
                let p = wf.ch1_peaks.get(sample_idx).copied().unwrap_or(0.04);

                let color = if let Some(ref colors) = wf.spectral_colors {
                    let [r, g, b] = colors.get(sample_idx).copied().unwrap_or([34, 197, 94]);
                    Color::from_rgba8(r, g, b, 235)
                } else {
                    // Plain waveform (Phase 1)
                    Color::from_rgba8(34, 197, 94, 220)
                };

                let bx = content_x + (b as f32 * (content_w / num_bars as f32));
                let bar_h = (p * max_h).max(1.5);
                if let Some(rect) = Rect::from_xywh(bx, center_y - bar_h, bar_width, bar_h * 2.0) {
                    let mut paint = Paint::default();
                    paint.set_color(color);
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            }
        }

        // Time footer:
        let time_y = content_y + content_h - 6.0;
        if wf.is_snippet {
            let start_mins = wf.snippet_start_ms / 60_000;
            let start_secs = (wf.snippet_start_ms % 60_000) / 1000;
            let start_str = format!("{:02}:{:02}", start_mins, start_secs);

            let end_ms = wf.snippet_start_ms + wf.snippet_duration_ms;
            let end_mins = end_ms / 60_000;
            let end_secs = (end_ms % 60_000) / 1000;
            let end_str = format!("{:02}:{:02}", end_mins, end_secs);

            let attention_col = self.color_from_hex(&self.palette.harmonies.warm_attention);

            self.draw_text(
                pixmap,
                &start_str,
                content_x,
                time_y,
                16.0,
                attention_col,
                false,
            );

            let badge_text = format!("• 30s PREVIEW OF {} •", dur_text);
            let badge_x = content_x + (content_w / 2.0) - (badge_text.len() as f32 * 3.8);
            self.draw_text(
                pixmap,
                &badge_text,
                badge_x.max(content_x + 50.0),
                time_y,
                13.0,
                attention_col,
                true,
            );

            let end_x = (content_x + content_w - 42.0).max(content_x + 50.0);
            self.draw_text(pixmap, &end_str, end_x, time_y, 16.0, attention_col, false);
        } else {
            let footer_col = self.color_from_hex(&self.palette.colors.text_muted);
            self.draw_text(pixmap, "00:00", content_x, time_y, 16.0, footer_col, false);
            let end_x = (content_x + content_w - 42.0).max(content_x + 50.0);
            self.draw_text(pixmap, &dur_text, end_x, time_y, 16.0, footer_col, false);
        }
    }
}
