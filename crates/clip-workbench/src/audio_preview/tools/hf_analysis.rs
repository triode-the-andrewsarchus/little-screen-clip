//! High-Frequency & Ultrasonic Audio Analysis studio tool.
//!
//! Provides a dedicated full-height spectrogram view for high-sample-rate recordings
//! (such as ultrasonic bat echolocation calls up to Nyquist), multiple specialized
//! playback transformations (Normal, Time Expansion, and real-time Pitch Shift),
//! an ultrasonic high-pass filter toggle, and an ultrasonic Region of Interest (ROI)
//! energy mini-map for navigation.

use super::super::*;
use crate::icons::Icon;
use little_clip_ops::hf_audio::{
    HfSpectrogramConfig, apply_ultrasonic_highpass, pitch_shift_realtime,
};
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Instant;

/// Playback speed and pitch mode for high-frequency analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HfPlaybackMode {
    /// Standard 1.0x playback at original recording speed.
    #[default]
    Normal,
    /// Time-expanded playback (slowed down by 1/ratio, lowering pitch and extending duration).
    TimeExpansion,
    /// Real-time pitch-shifted playback (frequencies shifted down by ratio, original duration preserved).
    PitchShift,
}

impl HfPlaybackMode {
    /// Returns the user-facing display label for this playback mode.
    ///
    /// # Returns
    ///
    /// A static string slice representing the button or header label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal (1.0x)",
            Self::TimeExpansion => "Time Expansion",
            Self::PitchShift => "Pitch Shift",
        }
    }
}

/// State machine for the High-Frequency and Ultrasonic Analysis studio tool.
#[derive(Clone)]
pub struct HfAnalysisState {
    /// Active playback transformation mode.
    pub mode: HfPlaybackMode,
    /// Pitch transposition or time expansion ratio.
    pub ratio: f64,
    /// Whether the 20 kHz ultrasonic high-pass filter is engaged during playback.
    pub ultrasonic_bandpass: bool,
    /// FFT window size (256, 512, 1024, 2048, 4096) for high-resolution analysis.
    pub fft_size: usize,
    /// Gain boost in decibels (-20.0 .. +40.0 dB).
    pub gain_db: f32,
    /// Dynamic display range in decibels (20.0 .. 120.0 dB).
    pub range_db: f32,
    /// Contrast exponent (0.2 .. 3.0).
    pub contrast: f32,
    /// Cached dynamic STFT viewport spectrogram texture.
    pub viewport_cache: ViewportSpectrogramCache,
}

impl std::fmt::Debug for HfAnalysisState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HfAnalysisState")
            .field("mode", &self.mode)
            .field("ratio", &self.ratio)
            .field("ultrasonic_bandpass", &self.ultrasonic_bandpass)
            .field("fft_size", &self.fft_size)
            .field("gain_db", &self.gain_db)
            .field("range_db", &self.range_db)
            .field("contrast", &self.contrast)
            .field("viewport_cache", &self.viewport_cache)
            .finish()
    }
}

impl Default for HfAnalysisState {
    fn default() -> Self {
        Self {
            mode: HfPlaybackMode::Normal,
            ratio: 10.0,
            ultrasonic_bandpass: false,
            fft_size: 1024,
            gain_db: 0.0,
            range_db: 80.0,
            contrast: 1.0,
            viewport_cache: ViewportSpectrogramCache::default(),
        }
    }
}

impl HfAnalysisState {
    /// Available ratios for files with sample rates greater than 196 kHz.
    pub const HIGH_SR_RATIOS: &'static [f64] = &[2.0, 4.0, 8.0, 10.0, 16.0, 32.0, 64.0, 128.0];

    /// Available ratios for files with sample rates up to 196 kHz.
    pub const STANDARD_SR_RATIOS: &'static [f64] = &[1.25, 1.5, 2.0, 2.5, 5.0, 10.0];

    /// Available FFT window sizes.
    pub const FFT_SIZES: &'static [usize] = &[256, 512, 1024, 2048, 4096];

    /// Returns the default FFT size for the given sample rate.
    pub fn default_fft_size(sample_rate: u32) -> usize {
        if sample_rate > 196_000 { 1024 } else { 512 }
    }

    /// Sanitizes the FFT size to ensure it is in the supported list.
    pub fn sanitize_fft_size(&mut self, sample_rate: u32) {
        if !Self::FFT_SIZES.contains(&self.fft_size) {
            self.fft_size = Self::default_fft_size(sample_rate);
        }
    }

    /// Returns the available playback ratios for the given sample rate.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Audio sample rate in Hz.
    ///
    /// # Returns
    ///
    /// A static slice of ratio numbers.
    pub fn available_ratios(sample_rate: u32) -> &'static [f64] {
        if sample_rate > 196_000 {
            Self::HIGH_SR_RATIOS
        } else {
            Self::STANDARD_SR_RATIOS
        }
    }

    /// Returns the default ratio for the given sample rate.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Audio sample rate in Hz.
    ///
    /// # Returns
    ///
    /// Default ratio value as an `f64`.
    pub fn default_ratio(sample_rate: u32) -> f64 {
        if sample_rate > 196_000 { 10.0 } else { 2.0 }
    }

    /// Sanitizes the currently selected ratio to ensure it is valid for the file's sample rate.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Audio sample rate in Hz.
    pub fn sanitize_ratio(&mut self, sample_rate: u32) {
        let list = Self::available_ratios(sample_rate);
        if !list.contains(&self.ratio) {
            self.ratio = Self::default_ratio(sample_rate);
        }
    }
}

#[allow(unused_imports)]
pub use super::spectrogram_view::frequency_ticks;

impl AudioPreviewState {
    /// Plays or restarts high-frequency audio from the specified timestamp using the active HF mode.
    ///
    /// # Arguments
    ///
    /// * `start_sec` - Audio start position in seconds.
    pub fn play_hf_audio_at(&mut self, start_sec: f32) {
        if let Some(ref p) = self.player {
            p.clear();
            let sr = self
                .analysis
                .as_ref()
                .map(|a| a.sample_rate)
                .unwrap_or(384_000);
            let mode = self.hf_analysis.mode;
            let ratio = self.hf_analysis.ratio.max(1.0);
            let bandpass = self.hf_analysis.ultrasonic_bandpass;

            if let Some(ref analysis) = self.analysis {
                let start_frame =
                    ((start_sec * sr as f32) as usize).min(analysis.ch1_samples.len());
                let raw_samples = &analysis.ch1_samples[start_frame..];

                let processed_samples = match mode {
                    HfPlaybackMode::Normal => {
                        if bandpass {
                            apply_ultrasonic_highpass(raw_samples, sr as f32, 20_000.0_f32)
                        } else {
                            raw_samples.to_vec()
                        }
                    }
                    HfPlaybackMode::TimeExpansion => {
                        if bandpass {
                            apply_ultrasonic_highpass(raw_samples, sr as f32, 20_000.0_f32)
                        } else {
                            raw_samples.to_vec()
                        }
                    }
                    HfPlaybackMode::PitchShift => {
                        let shifted = pitch_shift_realtime(raw_samples, ratio);
                        if bandpass {
                            apply_ultrasonic_highpass(&shifted, sr as f32, 20_000.0_f32)
                        } else {
                            shifted
                        }
                    }
                };

                let ch_nz = NonZeroU16::new(1).unwrap();
                let sr_nz = NonZeroU32::new(sr.max(1)).unwrap();

                if mode == HfPlaybackMode::TimeExpansion {
                    let speed = (1.0 / ratio) as f32;
                    let source = rodio::buffer::SamplesBuffer::new(ch_nz, sr_nz, processed_samples)
                        .speed(speed);
                    p.append(source);
                } else {
                    let source = rodio::buffer::SamplesBuffer::new(ch_nz, sr_nz, processed_samples);
                    p.append(source);
                }
                p.play();
                self.is_playing = true;
                self.playback_start_instant = Some(Instant::now());
                self.playback_start_offset_sec = start_sec;
            } else if let Ok(file) = File::open(&self.path) {
                if let Ok(decoder) = Decoder::new(BufReader::new(file)) {
                    if mode == HfPlaybackMode::TimeExpansion {
                        let speed = (1.0 / ratio) as f32;
                        p.append(decoder.speed(speed));
                    } else {
                        p.append(decoder);
                    }
                    let target_pos = Duration::from_secs_f32(start_sec);
                    let _ = p.try_seek(target_pos);
                    p.play();
                    self.is_playing = true;
                    self.playback_start_instant = Some(Instant::now());
                    self.playback_start_offset_sec = start_sec;
                }
            }
        }
    }

    /// Renders the specialized High-Frequency and Ultrasonic Analysis user interface.
    ///
    /// # Arguments
    ///
    /// * `ui` - Egui user interface context.
    /// * `ctx` - Egui global application context.
    /// * `palette` - Application visual theme palette.
    /// * `close_requested` - Out-parameter flag set to `true` when user requests closure.
    pub fn render_hf_analysis_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        _close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);
        self.poll_audio_analysis(ctx, now);

        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate)
            .or_else(|| self.metadata.as_ref().map(|m| m.sample_rate))
            .unwrap_or(44_100);
        let nyquist = sample_rate as f32 * 0.5_f32;
        self.hf_analysis.sanitize_ratio(sample_rate);
        self.hf_analysis.sanitize_fft_size(sample_rate);

        // Advance playhead during playback
        if self.is_playing {
            ctx.request_repaint();
            if let Some(start_inst) = self.playback_start_instant {
                let elapsed = start_inst.elapsed().as_secs_f32();
                let speed = if self.hf_analysis.mode == HfPlaybackMode::TimeExpansion {
                    (1.0 / self.hf_analysis.ratio).max(0.0001) as f32
                } else {
                    1.0_f32
                };
                let new_time = self.playback_start_offset_sec + elapsed * speed;
                if new_time >= self.duration_sec {
                    self.pause();
                    self.current_time_sec = self.duration_sec;
                } else {
                    self.current_time_sec = new_time;
                }
            }
        }

        // Global spacebar shortcut for play/pause in HF mode
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            if self.is_playing {
                self.pause();
            } else {
                self.play();
            }
        }

        let total_dur = self.duration_sec.max(0.01_f32);

        self.render_hf_toolbar(ui, palette, sample_rate, nyquist, total_dur);
        ui.add_space(crate::ui_ext::SPACE_XS);
        self.render_hf_roi_bar(ui, palette, total_dur);
        ui.add_space(crate::ui_ext::SPACE_XS);
        self.render_hf_spectrogram_canvas(ui, ctx, palette, sample_rate, nyquist, total_dur);
    }

    fn render_hf_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        sample_rate: u32,
        nyquist: f32,
        total_dur: f32,
    ) {
        let text_f_med = hex_to_c32(&palette.colors.text_secondary);
        let text_f_high = hex_to_c32(&palette.colors.text_primary);
        let accent_base = hex_to_c32(&palette.harmonies.base);
        let accent_cyan = Color32::from_rgb(0, 210, 255);

        // --- TOP TOOLBAR STRIP ---
        ui.horizontal_wrapped(|ui| {
            // Transport controls
            let (play_label, play_icon) = if self.is_playing {
                ("Pause (Space)", Icon::Pause)
            } else {
                ("Play (Space)", Icon::Play)
            };
            let transport_width =
                crate::ui_ext::equal_button_width(ui, &["Play (Space)", "Pause (Space)"], true);
            if ui
                .add(
                    layout::AudioWorkspace::primary_button_icon(
                        Some(play_icon),
                        play_label,
                        accent_base,
                        palette,
                    )
                    .min_size(egui::Vec2::new(
                        transport_width,
                        crate::ui_ext::CONTROL_HEIGHT,
                    )),
                )
                .clicked()
            {
                if self.is_playing {
                    self.pause();
                } else {
                    self.play();
                }
            }

            ui.separator();

            // Playback Mode selection
            ui.horizontal_wrapped(|ui| {
                layout::AudioWorkspace::inline_label(ui, "Mode:", text_f_med);
                let tabs: Vec<_> = [
                    HfPlaybackMode::Normal,
                    HfPlaybackMode::TimeExpansion,
                    HfPlaybackMode::PitchShift,
                ]
                .iter()
                .map(|&mode| crate::ui_tabs::Tab::new(mode, mode.label()))
                .collect();
                if let Some(mode) = crate::ui_tabs::TabBar::new(
                    "hf_playback_modes",
                    self.hf_analysis.mode,
                    &tabs,
                    palette,
                )
                .show(ui)
                {
                    self.hf_analysis.mode = mode;
                    if self.is_playing {
                        self.play_hf_audio_at(self.current_time_sec);
                    }
                }
            });

            // Ratio selection for Time Expansion or Pitch Shift
            if self.hf_analysis.mode != HfPlaybackMode::Normal {
                ui.separator();
                ui.horizontal(|ui| {
                    layout::AudioWorkspace::inline_label(ui, "Ratio:", text_f_med);
                    let available = HfAnalysisState::available_ratios(sample_rate);
                    for &r in available {
                        let is_sel = (self.hf_analysis.ratio - r).abs() < 1e-4;
                        let label = if (r - r.round()).abs() < 1e-4 {
                            format!("{:.0}x", r)
                        } else {
                            format!("{:.2}x", r)
                        };
                        if ui
                            .add(layout::AudioWorkspace::toggle_button_icon(
                                None, label, is_sel, palette,
                            ))
                            .clicked()
                        {
                            self.hf_analysis.ratio = r;
                            if self.is_playing {
                                self.play_hf_audio_at(self.current_time_sec);
                            }
                        }
                    }
                });
            }

            ui.separator();

            // Ultrasonic High-Pass toggle (> 20 kHz)
            let bandpass_active = self.hf_analysis.ultrasonic_bandpass;
            let hp_label = if bandpass_active {
                ">20 kHz High-Pass ON"
            } else {
                ">20 kHz High-Pass OFF"
            };
            let highpass_width = crate::ui_ext::equal_button_width(
                ui,
                &[">20 kHz High-Pass ON", ">20 kHz High-Pass OFF"],
                true,
            );
            if ui
                .add(
                    layout::AudioWorkspace::toggle_button_icon(
                        Some(Icon::Volume),
                        hp_label,
                        bandpass_active,
                        palette,
                    )
                    .min_size(egui::Vec2::new(
                        highpass_width,
                        crate::ui_ext::CONTROL_HEIGHT,
                    )),
                )
                .clicked()
            {
                self.hf_analysis.ultrasonic_bandpass = !bandpass_active;
                if self.is_playing {
                    self.play_hf_audio_at(self.current_time_sec);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "SR: {:.1} kHz | Nyquist: {:.1} kHz",
                        sample_rate as f32 / 1000.0_f32,
                        nyquist / 1000.0_f32
                    ))
                    .monospace()
                    .color(text_f_high),
                );
            });
        });

        ui.add_space(3.0_f32);

        // --- OVERSAMPLE FFT & DISPLAY CONTROLS STRIP ---
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("FFT Window:")
                    .color(text_f_med)
                    .strong(),
            );
            for &sz in HfAnalysisState::FFT_SIZES {
                let is_sel = self.hf_analysis.fft_size == sz;
                let (btn_bg, btn_fg) = if is_sel {
                    (accent_base, Color32::WHITE)
                } else {
                    (hex_to_c32(&palette.colors.surface_recessed), text_f_med)
                };
                let freq_res = sample_rate as f32 / sz as f32;
                let time_res_ms = (sz as f32 / sample_rate as f32) * 1000.0_f32;
                let tooltip = format!("{sz} pts (Δf: {freq_res:.1} Hz, Δt: {time_res_ms:.2} ms)");
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(format!("{sz}"))
                                .monospace()
                                .strong()
                                .color(btn_fg),
                        )
                        .fill(btn_bg)
                        .rounding(4.0_f32),
                    )
                    .on_hover_text(tooltip)
                    .clicked()
                {
                    self.hf_analysis.fft_size = sz;
                }
            }

            ui.separator();

            ui.label(egui::RichText::new("Gain:").color(text_f_med));
            ui.add(
                egui::DragValue::new(&mut self.hf_analysis.gain_db)
                    .speed(0.5_f32)
                    .clamp_range(-20.0_f32..=40.0_f32)
                    .suffix(" dB"),
            )
            .on_hover_text("Spectrogram intensity boost (-20 to +40 dB)");

            ui.label(egui::RichText::new("Range:").color(text_f_med));
            ui.add(
                egui::DragValue::new(&mut self.hf_analysis.range_db)
                    .speed(1.0_f32)
                    .clamp_range(20.0_f32..=120.0_f32)
                    .suffix(" dB"),
            )
            .on_hover_text("Dynamic display range floor (20 to 120 dB)");

            ui.label(egui::RichText::new("Contrast:").color(text_f_med));
            ui.add(
                egui::DragValue::new(&mut self.hf_analysis.contrast)
                    .speed(0.05_f32)
                    .clamp_range(0.2_f32..=3.0_f32),
            )
            .on_hover_text("Display contrast curve exponent (0.2 to 3.0)");

            if ui
                .add(
                    egui::Button::new(egui::RichText::new("↺ Reset").color(text_f_med))
                        .fill(hex_to_c32(&palette.colors.surface_recessed))
                        .rounding(4.0_f32),
                )
                .on_hover_text("Reset FFT and intensity settings to defaults")
                .clicked()
            {
                self.hf_analysis.gain_db = 0.0_f32;
                self.hf_analysis.range_db = 80.0_f32;
                self.hf_analysis.contrast = 1.0_f32;
                self.hf_analysis.fft_size = HfAnalysisState::default_fft_size(sample_rate);
            }

            // Quick Peak Navigation if ultrasonic peaks were detected
            let (has_peaks, n_peaks) = self
                .analysis
                .as_ref()
                .map(|a| {
                    (
                        !a.ultrasonic_peak_times.is_empty(),
                        a.ultrasonic_peak_times.len(),
                    )
                })
                .unwrap_or((false, 0));

            if has_peaks {
                ui.separator();
                ui.label(egui::RichText::new("Chirps:").color(text_f_med).strong());

                let mut jump_target = None;
                if ui
                    .add(layout::AudioWorkspace::secondary_button_icon(
                        Some(Icon::ArrowLeft),
                        "Prev",
                        hex_to_c32(&palette.colors.surface_recessed),
                        palette,
                    ))
                    .on_hover_text("Jump to previous ultrasonic chirp")
                    .clicked()
                {
                    if let Some(ref analysis) = self.analysis {
                        let cur_t = self.current_time_sec;
                        if let Some(&prev_t) = analysis
                            .ultrasonic_peak_times
                            .iter()
                            .rev()
                            .find(|&&t| t < cur_t - 0.05_f32)
                        {
                            jump_target = Some(prev_t);
                        } else if let Some(&last_t) = analysis.ultrasonic_peak_times.last() {
                            jump_target = Some(last_t);
                        }
                    }
                }

                if ui
                    .add(layout::AudioWorkspace::secondary_button_icon(
                        Some(Icon::ArrowRight),
                        "Next",
                        hex_to_c32(&palette.colors.surface_recessed),
                        palette,
                    ))
                    .on_hover_text("Jump to next ultrasonic chirp")
                    .clicked()
                {
                    if let Some(ref analysis) = self.analysis {
                        let cur_t = self.current_time_sec;
                        if let Some(&next_t) = analysis
                            .ultrasonic_peak_times
                            .iter()
                            .find(|&&t| t > cur_t + 0.05_f32)
                        {
                            jump_target = Some(next_t);
                        } else if let Some(&first_t) = analysis.ultrasonic_peak_times.first() {
                            jump_target = Some(first_t);
                        }
                    }
                }

                if let Some(target_t) = jump_target {
                    self.seek_to(target_t);
                    self.view_start_sec = (target_t - self.view_duration_sec * 0.5_f32)
                        .clamp(0.0_f32, (total_dur - self.view_duration_sec).max(0.0_f32));
                }

                ui.label(
                    egui::RichText::new(format!("({n_peaks} detected)"))
                        .monospace()
                        .color(accent_cyan),
                );
            }
        });
    }

    fn render_hf_roi_bar(&mut self, ui: &mut egui::Ui, palette: &AppPalette, total_dur: f32) {
        let border_frame = hex_to_c32(&palette.colors.surface_panel);
        let bg_frame = hex_to_c32(&palette.colors.surface_recessed);
        let accent_cyan = Color32::from_rgb(0, 210, 255);
        let accent_amber = Color32::from_rgb(255, 185, 0);

        // --- REGION OF INTEREST (ROI) MINI-MAP BAR ---
        let roi_height = 28.0_f32;
        let (roi_rect, roi_response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), roi_height),
            egui::Sense::click_and_drag(),
        );

        let v_start = self.view_start_sec;
        let v_dur = self.view_duration_sec.max(0.001_f32);
        let time_to_roi_x = |t: f32| -> f32 {
            roi_rect.min.x + (t / total_dur).clamp(0.0_f32, 1.0_f32) * roi_rect.width()
        };
        let roi_x_to_time = |x: f32| -> f32 {
            ((x - roi_rect.min.x) / roi_rect.width()).clamp(0.0_f32, 1.0_f32) * total_dur
        };

        {
            let painter = ui.painter();
            painter.rect_filled(roi_rect, Rounding::same(4.0_f32), bg_frame);
            painter.rect_stroke(
                roi_rect,
                Rounding::same(4.0_f32),
                Stroke::new(1.0_f32, border_frame),
            );

            if let Some(ref analysis) = self.analysis {
                // Draw energy bars if ultrasonic ROI is computed
                let roi = &analysis.ultrasonic_roi;
                if !roi.is_empty() {
                    let n_cols = roi.len();
                    let col_w = (roi_rect.width() / n_cols as f32).max(1.0_f32);
                    for (col_idx, &val) in roi.iter().enumerate() {
                        if val <= 0.01_f32 {
                            continue;
                        }
                        let x0 =
                            roi_rect.min.x + (col_idx as f32 / n_cols as f32) * roi_rect.width();
                        let bar_h = (val * (roi_rect.height() - 4.0_f32)).max(1.0_f32);
                        let y0 = roi_rect.max.y - 2.0_f32 - bar_h;
                        let rect_bar =
                            Rect::from_min_size(Pos2::new(x0, y0), egui::vec2(col_w, bar_h));
                        let col = if val > 0.6_f32 {
                            accent_amber
                        } else {
                            accent_cyan
                        };
                        painter.rect_filled(rect_bar, Rounding::ZERO, col);
                    }
                }

                // Draw peak burst markers
                for &peak_t in &analysis.ultrasonic_peak_times {
                    let px = time_to_roi_x(peak_t);
                    painter.line_segment(
                        [
                            Pos2::new(px, roi_rect.min.y + 2.0_f32),
                            Pos2::new(px, roi_rect.max.y - 2.0_f32),
                        ],
                        Stroke::new(1.5_f32, Color32::from_rgb(255, 60, 120)),
                    );
                }
            }

            // Viewport window highlight on ROI bar
            let vx0 = time_to_roi_x(v_start);
            let vx1 = time_to_roi_x(v_start + v_dur);
            let v_rect = Rect::from_min_max(
                Pos2::new(vx0, roi_rect.min.y + 1.0_f32),
                Pos2::new(vx1, roi_rect.max.y - 1.0_f32),
            );
            painter.rect_filled(
                v_rect,
                Rounding::ZERO,
                Color32::from_rgba_unmultiplied(255, 255, 255, 30),
            );
            painter.rect_stroke(
                v_rect,
                Rounding::ZERO,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 120)),
            );

            // Playhead indicator on ROI bar
            let play_x = time_to_roi_x(self.current_time_sec);
            painter.line_segment(
                [
                    Pos2::new(play_x, roi_rect.min.y),
                    Pos2::new(play_x, roi_rect.max.y),
                ],
                Stroke::new(2.0_f32, Color32::WHITE),
            );
        }

        // ROI bar interaction (click or drag seeks and centers view)
        if roi_response.clicked() || roi_response.dragged() {
            if let Some(mpos) = roi_response.interact_pointer_pos() {
                let target_t = roi_x_to_time(mpos.x);
                self.seek_to(target_t);
                self.view_start_sec = (target_t - self.view_duration_sec * 0.5_f32)
                    .clamp(0.0_f32, (total_dur - self.view_duration_sec).max(0.0_f32));
            }
        }

        if roi_response.hovered() {
            if let Some(mpos) = roi_response.hover_pos() {
                let hover_t = roi_x_to_time(mpos.x);
                let roi_val = self
                    .analysis
                    .as_ref()
                    .and_then(|a| {
                        if a.ultrasonic_roi.is_empty() {
                            None
                        } else {
                            let idx =
                                ((hover_t / total_dur) * a.ultrasonic_roi.len() as f32) as usize;
                            a.ultrasonic_roi.get(idx).copied()
                        }
                    })
                    .unwrap_or(0.0_f32);
                roi_response.on_hover_ui(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Time: {:.3}s | Ultrasonic Energy: {:.0}%",
                            hover_t,
                            roi_val * 100.0_f32
                        ))
                        .monospace(),
                    );
                });
            }
        }
    }

    fn render_hf_spectrogram_canvas(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        sample_rate: u32,
        nyquist: f32,
        total_dur: f32,
    ) {
        let border_frame = hex_to_c32(&palette.colors.surface_panel);
        let bg_frame = hex_to_c32(&palette.colors.surface_recessed);
        let text_f_med = hex_to_c32(&palette.colors.text_secondary);

        let v_start = self.view_start_sec;
        let v_dur = self.view_duration_sec.max(0.001_f32);

        // --- FULL-HEIGHT SPECTROGRAM DISPLAY ---
        let available_size = ui.available_size();
        let gutter_width = 46.0_f32;
        let ruler_height = 20.0_f32;
        let spec_height = (available_size.y - ruler_height).max(120.0_f32);

        let (canvas_rect, canvas_response) =
            ui.allocate_exact_size(available_size, egui::Sense::click_and_drag());

        let track_min_x = canvas_rect.min.x + gutter_width;
        let track_max_x = canvas_rect.max.x;
        let track_width = (track_max_x - track_min_x).max(1.0_f32);

        let spec_rect = Rect::from_min_size(
            Pos2::new(track_min_x, canvas_rect.min.y),
            egui::vec2(track_width, spec_height),
        );
        let ruler_rect = Rect::from_min_size(
            Pos2::new(track_min_x, spec_rect.max.y),
            egui::vec2(track_width, ruler_height),
        );

        let time_to_spec_x =
            |t: f32| -> f32 { track_min_x + ((t - v_start) / v_dur) * track_width };
        let spec_x_to_time =
            |x: f32| -> f32 { v_start + ((x - track_min_x) / track_width) * v_dur };

        // Mouse pan & zoom interactions on spectrogram
        if canvas_response.hovered() {
            let scroll_delta = ctx.input(|i| i.raw_scroll_delta);
            if scroll_delta.y != 0.0_f32 {
                let zoom_anchor = ctx
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| spec_x_to_time(p.x))
                    .unwrap_or(self.current_time_sec);
                let zoom_factor = if scroll_delta.y > 0.0_f32 {
                    0.85_f32
                } else {
                    1.176_f32
                };
                self.zoom(zoom_factor, zoom_anchor);
            }
            if scroll_delta.x != 0.0_f32 {
                let pan_sec = -(scroll_delta.x / track_width) * v_dur;
                self.pan(pan_sec);
            }
        }

        if canvas_response.clicked() || canvas_response.dragged() {
            if let Some(mpos) = canvas_response.interact_pointer_pos() {
                if mpos.x >= track_min_x && mpos.x <= track_max_x {
                    let clicked_t = spec_x_to_time(mpos.x);
                    self.seek_to(clicked_t);
                }
            }
        }

        let painter = ui.painter();

        // Draw frequency axis gutter on the left
        let gutter_rect = Rect::from_min_size(
            Pos2::new(canvas_rect.min.x, canvas_rect.min.y),
            egui::vec2(gutter_width, spec_height),
        );
        painter.rect_filled(gutter_rect, Rounding::ZERO, bg_frame);
        painter.line_segment(
            [
                Pos2::new(track_min_x, canvas_rect.min.y),
                Pos2::new(track_min_x, canvas_rect.max.y),
            ],
            Stroke::new(1.0_f32, border_frame),
        );

        // --- SPECTROGRAM SURFACE & FREQUENCY GRID (SHARED) ---
        let cfg = HfSpectrogramConfig {
            fft_size: self.hf_analysis.fft_size,
            gain_db: self.hf_analysis.gain_db,
            range_db: self.hf_analysis.range_db,
            contrast: self.hf_analysis.contrast,
        };

        if let Some(ref analysis) = self.analysis {
            render_and_paint_spectrogram(
                &mut self.hf_analysis.viewport_cache,
                ctx,
                painter,
                format!("hf_vp:{}", self.path.display()),
                spec_rect,
                track_min_x,
                track_max_x,
                track_min_x - 6.0_f32,
                v_start,
                v_dur,
                total_dur,
                sample_rate,
                &analysis.ch1_samples,
                self.spectrogram_texture.as_ref(),
                cfg,
                bg_frame,
                text_f_med,
                Stroke::new(1.0_f32, hex_to_c32_alpha(&palette.colors.surface_panel, 60)),
                11.0_f32,
            );
        } else {
            paint_spectrogram_surface(
                painter,
                spec_rect,
                None,
                self.spectrogram_texture.as_ref(),
                v_start,
                v_dur,
                total_dur,
                bg_frame,
                text_f_med,
            );
        }

        // Draw Playhead line
        let playhead_x = time_to_spec_x(self.current_time_sec);
        if playhead_x >= track_min_x && playhead_x <= track_max_x {
            painter.line_segment(
                [
                    Pos2::new(playhead_x, spec_rect.min.y),
                    Pos2::new(playhead_x, ruler_rect.max.y),
                ],
                Stroke::new(2.0_f32, Color32::WHITE),
            );
            let pip = Pos2::new(playhead_x, spec_rect.min.y);
            painter.circle_filled(pip, 4.0_f32, Color32::WHITE);
        }

        // Draw Bottom Time Ruler
        painter.rect_filled(ruler_rect, Rounding::ZERO, bg_frame);
        painter.line_segment(
            [
                Pos2::new(track_min_x, ruler_rect.min.y),
                Pos2::new(track_max_x, ruler_rect.min.y),
            ],
            Stroke::new(1.0_f32, border_frame),
        );

        let time_step = time_ruler_interval(v_dur);
        let start_marker = (v_start / time_step).floor() * time_step;
        let mut marker_t = start_marker;
        while marker_t <= v_start + v_dur + time_step * 0.5_f32 {
            if marker_t >= v_start {
                let mx = time_to_spec_x(marker_t);
                if mx >= track_min_x && mx <= track_max_x {
                    painter.line_segment(
                        [
                            Pos2::new(mx, ruler_rect.min.y),
                            Pos2::new(mx, ruler_rect.min.y + 6.0_f32),
                        ],
                        Stroke::new(1.0_f32, text_f_med),
                    );
                    let label = format_time_label(marker_t, time_step);
                    painter.text(
                        Pos2::new(mx, ruler_rect.min.y + 8.0_f32),
                        egui::Align2::CENTER_TOP,
                        label,
                        egui::FontId::monospace(9.0_f32),
                        text_f_med,
                    );
                }
            }
            marker_t += time_step;
        }

        // Hover crosshairs and frequency/time readout badge
        if let Some(hpos) = canvas_response.hover_pos() {
            if spec_rect.contains(hpos) {
                let hover_t = spec_x_to_time(hpos.x);
                let frac_y =
                    ((spec_rect.max.y - hpos.y) / spec_rect.height()).clamp(0.0_f32, 1.0_f32);
                let hover_freq = frac_y * nyquist;

                // Subtle crosshair lines
                painter.line_segment(
                    [
                        Pos2::new(spec_rect.min.x, hpos.y),
                        Pos2::new(spec_rect.max.x, hpos.y),
                    ],
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 45)),
                );
                painter.line_segment(
                    [
                        Pos2::new(hpos.x, spec_rect.min.y),
                        Pos2::new(hpos.x, spec_rect.max.y),
                    ],
                    Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 45)),
                );

                // Cursor readout badge in top-right corner of spectrogram
                let badge_text = format!("{:.3} s  |  {:.1} kHz", hover_t, hover_freq / 1000.0_f32);
                let badge_pos = Pos2::new(spec_rect.max.x - 8.0_f32, spec_rect.min.y + 8.0_f32);
                let font = egui::FontId::monospace(11.0_f32);
                let galley = painter.layout_no_wrap(badge_text, font, Color32::WHITE);
                let badge_size = galley.size() + egui::vec2(12.0_f32, 6.0_f32);
                let badge_rect = Rect::from_min_size(
                    Pos2::new(badge_pos.x - badge_size.x, badge_pos.y),
                    badge_size,
                );
                painter.rect_filled(
                    badge_rect,
                    Rounding::same(3.0_f32),
                    Color32::from_black_alpha(180),
                );
                painter.rect_stroke(
                    badge_rect,
                    Rounding::same(3.0_f32),
                    Stroke::new(1.0_f32, border_frame),
                );
                painter.galley(
                    Pos2::new(badge_rect.min.x + 6.0_f32, badge_rect.min.y + 3.0_f32),
                    galley,
                    Color32::WHITE,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_ops::hf_audio::render_hf_viewport_spectrogram;

    #[test]
    fn frequency_ticks_generates_full_nyquist_for_ultrasonic() {
        let ticks = frequency_ticks(384_000);
        assert!(!ticks.is_empty());
        assert_eq!(ticks[0].1, "180k");
        assert_eq!(ticks[1].1, "160k");
        let labels: Vec<&str> = ticks.iter().map(|(_, l)| l.as_str()).collect();
        assert!(labels.contains(&"180k"));
        assert!(labels.contains(&"100k"));
        assert!(labels.contains(&"20k"));
        assert!(labels.contains(&"10k"));
        assert!(labels.contains(&"1k"));
        assert!(labels.contains(&"100"));
    }

    #[test]
    fn frequency_ticks_caps_at_20k_for_standard_sample_rates() {
        let ticks_44k = frequency_ticks(44_100);
        assert_eq!(ticks_44k[0].1, "20k");
        let ticks_192k = frequency_ticks(192_000);
        assert_eq!(ticks_192k[0].1, "20k");
        let ticks_196k = frequency_ticks(196_000);
        assert_eq!(ticks_196k[0].1, "20k");
    }

    #[test]
    fn available_ratios_match_spec_thresholds() {
        let high_ratios = HfAnalysisState::available_ratios(384_000);
        assert_eq!(high_ratios, &[2.0, 4.0, 8.0, 10.0, 16.0, 32.0, 64.0, 128.0]);

        let standard_ratios = HfAnalysisState::available_ratios(192_000);
        assert_eq!(standard_ratios, &[1.25, 1.5, 2.0, 2.5, 5.0, 10.0]);

        let standard_ratios_196k = HfAnalysisState::available_ratios(196_000);
        assert_eq!(standard_ratios_196k, &[1.25, 1.5, 2.0, 2.5, 5.0, 10.0]);
    }

    fn generate_synthetic_ultrasonic_wav(
        sample_rate: u32,
        duration_sec: f32,
    ) -> std::path::PathBuf {
        use std::io::Write;
        let temp_path = std::env::temp_dir().join(format!("test_bat_{}.wav", uuid::Uuid::new_v4()));
        let mut file = std::fs::File::create(&temp_path).expect("failed to create temp wav");
        let num_samples = (sample_rate as f32 * duration_sec) as usize;
        let num_channels = 1u16;
        let bits_per_sample = 16u16;
        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample as u32 / 8);
        let block_align = num_channels * (bits_per_sample / 8);
        let data_len = (num_samples * num_channels as usize * 2) as u32;
        let riff_len = 36 + data_len;

        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_len.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&num_channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&block_align.to_le_bytes()).unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_len.to_le_bytes()).unwrap();

        // 30 kHz chirp between 0.5s and 0.6s
        let chirp_start = (sample_rate as f32 * 0.5) as usize;
        let chirp_end = (sample_rate as f32 * 0.6) as usize;
        for i in 0..num_samples {
            let s = if i >= chirp_start && i < chirp_end {
                0.8 * (2.0 * std::f32::consts::PI * 30_000.0 * (i - chirp_start) as f32
                    / sample_rate as f32)
                    .sin()
            } else {
                0.0
            };
            let val = (s * 30000.0) as i16;
            file.write_all(&val.to_le_bytes()).unwrap();
        }
        file.flush().unwrap();
        temp_path
    }

    #[test]
    fn test_bat_file_analysis_and_hf_features() {
        let bat_path = generate_synthetic_ultrasonic_wav(384_000, 1.2);

        let res = little_clip_core::audio::compute_audio_analysis_progressive(
            &bat_path,
            600,
            |progress| {
                if let little_clip_core::audio::AudioAnalysisProgress::Complete = progress {
                    // Done
                }
            },
        );
        let analysis = res.expect("synthetic ultrasonic audio should decode and analyze");
        assert_eq!(analysis.sample_rate, 384_000);
        assert!(
            !analysis.ultrasonic_roi.is_empty(),
            "ROI envelope should be computed"
        );
        assert!(
            !analysis.ultrasonic_peak_times.is_empty(),
            "Should detect chirp peaks"
        );

        // Chirp is placed at ~0.55s
        let has_peak_near_chirp = analysis
            .ultrasonic_peak_times
            .iter()
            .any(|&t| (t - 0.55).abs() < 0.2);
        assert!(
            has_peak_near_chirp,
            "Detected peaks: {:?}",
            analysis.ultrasonic_peak_times
        );

        // Verify frequency ticks up to Nyquist
        let ticks = frequency_ticks(analysis.sample_rate);
        assert_eq!(ticks[0].1, "180k");
        assert_eq!(ticks[1].1, "160k");

        // Verify pitch shifting a 0.5s snippet
        let sr = analysis.sample_rate as usize;
        let snippet_start = (0.4 * sr as f64) as usize;
        let snippet_end = snippet_start + (0.5 * sr as f64) as usize;
        if snippet_end <= analysis.ch1_samples.len() {
            let snippet = &analysis.ch1_samples[snippet_start..snippet_end];
            let shifted = pitch_shift_realtime(snippet, 10.0);
            assert_eq!(shifted.len(), snippet.len());
            let filtered = apply_ultrasonic_highpass(snippet, 384_000.0, 20_000.0);
            assert_eq!(filtered.len(), snippet.len());
        }

        // Verify high-resolution viewport spectrogram rendering on chirp snippet
        let cfg = HfSpectrogramConfig {
            fft_size: 1024,
            gain_db: 0.0,
            range_db: 80.0,
            contrast: 1.0,
        };
        let (tex_w, tex_h, vp_rgba) = render_hf_viewport_spectrogram(
            &analysis.ch1_samples,
            analysis.sample_rate,
            0.4,
            0.5,
            600,
            cfg,
        );
        assert_eq!(tex_w, 600);
        assert_eq!(tex_h, 512);
        assert_eq!(vp_rgba.len(), 600 * 512 * 4);

        let _ = std::fs::remove_file(&bat_path);
    }

    #[test]
    fn test_fft_size_selection_and_sanitization() {
        let mut state = HfAnalysisState::default();
        assert_eq!(HfAnalysisState::default_fft_size(384_000), 1024);
        assert_eq!(HfAnalysisState::default_fft_size(44_100), 512);
        assert!(HfAnalysisState::FFT_SIZES.contains(&4096));

        state.fft_size = 4096;
        state.sanitize_fft_size(384_000);
        assert_eq!(state.fft_size, 4096);

        state.fft_size = 999;
        state.sanitize_fft_size(384_000);
        assert_eq!(state.fft_size, 1024);

        state.fft_size = 999;
        state.sanitize_fft_size(44_100);
        assert_eq!(state.fft_size, 512);
    }
}
