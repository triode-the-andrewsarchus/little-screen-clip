//! Shared spectrogram viewport caching, rendering, and frequency/time grid helpers.
//!
//! Provides reusable components shared across the Inspect & Slice studio view,
//! High-Frequency (HF) Analysis view, and other audio preview tools:
//!
//! - [`ViewportSpectrogramCache`]: Manages on-the-fly STFT dynamic viewport textures.
//! - [`paint_spectrogram_surface`]: Paints the heatmap texture with UV mapping or fallback.
//! - [`paint_frequency_axis_and_grid`]: Paints the frequency ticks gutter and horizontal grid lines.
//! - [`render_and_paint_spectrogram`]: High-level orchestrator combining viewport updates,
//!   surface painting, and frequency grid overlay.
//! - [`frequency_ticks`]: Computes Nyquist-aware frequency ticks (supporting ultrasonic >196 kHz).
//! - [`time_ruler_interval`] & [`format_time_label`]: Standardized timeline ruler helpers.

use eframe::egui::{self, Color32, Pos2, Rect, Rounding, Stroke};
use little_clip_ops::hf_audio::{HfSpectrogramConfig, render_hf_viewport_spectrogram};

/// Returns frequency label ticks and values in descending order for the spectrogram axis.
///
/// For sample rates greater than 196 kHz, returns ticks at 20 kHz intervals down from
/// the highest multiple of 20 kHz below Nyquist, followed by standard audible markers.
/// For sample rates up to 196 kHz, returns standard audible tick marks capped at 20 kHz.
///
/// # Arguments
///
/// * `sample_rate` - The audio sample rate in Hz.
///
/// # Returns
///
/// A [`Vec`] of `(frequency_hz, label)` pairs in descending order of frequency.
pub fn frequency_ticks(sample_rate: u32) -> Vec<(f32, String)> {
    let nyquist = sample_rate as f32 * 0.5_f32;
    let mut ticks = Vec::new();

    if sample_rate > 196_000 {
        let top_multiple = ((nyquist / 20_000.0_f32).floor() as u32) * 20_000;
        let mut freq = top_multiple;
        while freq >= 20_000 {
            if (freq as f32) < nyquist {
                ticks.push((freq as f32, format!("{}k", freq / 1000)));
            }
            if freq < 20_000 {
                break;
            }
            freq = freq.saturating_sub(20_000);
        }
    } else if 20_000.0_f32 < nyquist {
        ticks.push((20_000.0_f32, "20k".to_string()));
    }

    for (f, lbl) in [
        (10_000.0_f32, "10k"),
        (5_000.0_f32, "5k"),
        (1_000.0_f32, "1k"),
        (100.0_f32, "100"),
    ] {
        if f < nyquist {
            ticks.push((f, lbl.to_string()));
        }
    }

    ticks
}

/// Dynamic time step interval in seconds for the timeline ruler.
///
/// # Arguments
///
/// * `v_dur` - Currently visible viewport duration in seconds.
///
/// # Returns
///
/// Recommended grid interval step in seconds.
pub fn time_ruler_interval(v_dur: f32) -> f32 {
    if v_dur > 120.0 {
        30.0
    } else if v_dur > 30.0 {
        10.0
    } else if v_dur > 10.0 {
        2.0
    } else if v_dur > 2.0 {
        0.5
    } else if v_dur > 0.5 {
        0.1
    } else if v_dur > 0.1 {
        0.02
    } else if v_dur > 0.02 {
        0.005
    } else {
        0.001
    }
}

/// Formats a time marker timestamp into a human-readable ruler string.
///
/// # Arguments
///
/// * `t` - Timestamp in seconds.
/// * `interval_sec` - Ruler step interval in seconds.
///
/// # Returns
///
/// A formatted string such as `"01:23"` or `"00:04.25"`.
pub fn format_time_label(t: f32, interval_sec: f32) -> String {
    if interval_sec >= 1.0 {
        let m = (t / 60.0) as u32;
        let s = (t % 60.0) as u32;
        format!("{m:02}:{s:02}")
    } else if interval_sec >= 0.01 {
        let m = (t / 60.0) as u32;
        let s = t % 60.0;
        format!("{m:02}:{s:05.2}")
    } else {
        let s = t % 60.0;
        format!("{s:06.3}s")
    }
}

/// Cache for dynamic high-resolution STFT viewport spectrogram textures.
#[derive(Default, Clone)]
pub struct ViewportSpectrogramCache {
    /// Loaded egui texture handle for the active visible viewport.
    pub texture: Option<egui::TextureHandle>,
    /// Cache key tracking: `(start_ms, dur_ms, target_width_px, fft_size, gain_x10, range_x10, contrast_x100)`.
    pub last_key: Option<(u32, u32, usize, usize, i32, i32, i32)>,
}

impl std::fmt::Debug for ViewportSpectrogramCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportSpectrogramCache")
            .field("has_texture", &self.texture.is_some())
            .field("last_key", &self.last_key)
            .finish()
    }
}

impl ViewportSpectrogramCache {
    /// Invalidate and clear the cached viewport texture and key.
    pub fn clear(&mut self) {
        self.texture = None;
        self.last_key = None;
    }

    /// Updates or retrieves the cached viewport spectrogram texture.
    ///
    /// If the visible viewport parameters or spectrogram configuration changed since the last
    /// invocation, recomputes the STFT across the visible duration and uploads a fresh
    /// texture to the egui context.
    #[allow(clippy::too_many_arguments)]
    pub fn get_or_update(
        &mut self,
        ctx: &egui::Context,
        texture_id: impl Into<String>,
        samples: &[f32],
        sample_rate: u32,
        v_start: f32,
        v_dur: f32,
        track_width_px: f32,
        config: HfSpectrogramConfig,
    ) -> Option<&egui::TextureHandle> {
        if samples.is_empty() {
            return self.texture.as_ref();
        }

        let target_w = (track_width_px as usize).clamp(200, 2048);
        let start_ms = (v_start * 1000.0).max(0.0) as u32;
        let dur_ms = (v_dur * 1000.0).max(1.0) as u32;
        let gain_x10 = (config.gain_db * 10.0) as i32;
        let range_x10 = (config.range_db * 10.0) as i32;
        let contrast_x100 = (config.contrast * 100.0) as i32;
        let current_key = (
            start_ms,
            dur_ms,
            target_w,
            config.fft_size,
            gain_x10,
            range_x10,
            contrast_x100,
        );

        if self.last_key != Some(current_key) || self.texture.is_none() {
            let (tex_w, tex_h, rgba) = render_hf_viewport_spectrogram(
                samples,
                sample_rate,
                v_start,
                v_dur,
                target_w,
                config,
            );
            let color_img = egui::ColorImage::from_rgba_unmultiplied([tex_w, tex_h], &rgba);
            self.texture =
                Some(ctx.load_texture(texture_id, color_img, egui::TextureOptions::LINEAR));
            self.last_key = Some(current_key);
        }

        self.texture.as_ref()
    }
}

/// Paints the spectrogram surface onto `spec_rect`.
///
/// Priority:
/// 1. Dynamic viewport texture (if available, drawn with UV `[0..1, 0..1]`).
/// 2. Overview static texture (if available, drawn with UV `(v_start/dur)..(v_end/dur)`).
/// 3. Placeholder solid background with loading message.
#[allow(clippy::too_many_arguments)]
pub fn paint_spectrogram_surface(
    painter: &egui::Painter,
    spec_rect: Rect,
    vp_texture: Option<&egui::TextureHandle>,
    overview_texture: Option<&egui::TextureHandle>,
    v_start: f32,
    v_dur: f32,
    total_dur: f32,
    bg_frame: Color32,
    placeholder_text_color: Color32,
) {
    if let Some(vp_tex) = vp_texture {
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        painter.image(vp_tex.id(), spec_rect, uv, Color32::WHITE);
    } else if let Some(tex) = overview_texture {
        let total_dur = total_dur.max(0.01);
        let u_start = (v_start / total_dur).clamp(0.0, 1.0);
        let u_end = ((v_start + v_dur) / total_dur).clamp(0.0, 1.0);
        let uv = Rect::from_min_max(Pos2::new(u_start, 0.0), Pos2::new(u_end, 1.0));
        painter.image(tex.id(), spec_rect, uv, Color32::WHITE);
    } else {
        painter.rect_filled(spec_rect, Rounding::ZERO, bg_frame);
        crate::icons::Icon::Hourglass.paint_text(
            painter,
            spec_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Generating Spectrogram Heatmap...",
            egui::FontId::proportional(12.0),
            placeholder_text_color,
        );
    }
}

/// Paints horizontal frequency grid lines across the track and frequency labels in the gutter.
pub fn paint_frequency_axis_and_grid(
    painter: &egui::Painter,
    spec_rect: Rect,
    track_min_x: f32,
    track_max_x: f32,
    label_x: f32,
    sample_rate: u32,
    grid_stroke: Stroke,
    text_color: Color32,
    min_label_spacing: f32,
) {
    let nyquist = sample_rate as f32 * 0.5;
    let ticks = frequency_ticks(sample_rate);
    let mut last_y = f32::NEG_INFINITY;
    for (freq_hz, lbl) in ticks {
        if freq_hz >= nyquist {
            continue;
        }
        let y = spec_rect.max.y - (freq_hz / nyquist) * spec_rect.height();
        if y - last_y < min_label_spacing {
            continue;
        }
        last_y = y;

        painter.text(
            Pos2::new(label_x, y),
            egui::Align2::RIGHT_CENTER,
            lbl,
            egui::FontId::monospace(9.0),
            text_color,
        );
        painter.line_segment(
            [Pos2::new(track_min_x, y), Pos2::new(track_max_x, y)],
            grid_stroke,
        );
    }
}

/// High-level helper that updates dynamic viewport spectrogram texture, paints the heatmap
/// surface, and overlays the frequency grid and axis tick labels.
#[allow(clippy::too_many_arguments)]
pub fn render_and_paint_spectrogram(
    cache: &mut ViewportSpectrogramCache,
    ctx: &egui::Context,
    painter: &egui::Painter,
    texture_id: impl Into<String>,
    spec_rect: Rect,
    track_min_x: f32,
    track_max_x: f32,
    label_x: f32,
    v_start: f32,
    v_dur: f32,
    total_dur: f32,
    sample_rate: u32,
    samples: &[f32],
    overview_texture: Option<&egui::TextureHandle>,
    config: HfSpectrogramConfig,
    bg_color: Color32,
    text_color: Color32,
    grid_stroke: Stroke,
    min_label_spacing: f32,
) {
    let track_width = (track_max_x - track_min_x).max(1.0);
    let vp_tex = cache.get_or_update(
        ctx,
        texture_id,
        samples,
        sample_rate,
        v_start,
        v_dur,
        track_width,
        config,
    );

    paint_spectrogram_surface(
        painter,
        spec_rect,
        vp_tex,
        overview_texture,
        v_start,
        v_dur,
        total_dur,
        bg_color,
        text_color,
    );

    paint_frequency_axis_and_grid(
        painter,
        spec_rect,
        track_min_x,
        track_max_x,
        label_x,
        sample_rate,
        grid_stroke,
        text_color,
        min_label_spacing,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frequency_ticks_high_sample_rate() {
        let ticks = frequency_ticks(384_000);
        assert!(!ticks.is_empty());
        assert_eq!(ticks[0].1, "180k");
        assert_eq!(ticks[1].1, "160k");
        assert!(ticks.iter().any(|(_, l)| l == "20k"));
        assert!(ticks.iter().any(|(_, l)| l == "10k"));
    }

    #[test]
    fn test_frequency_ticks_standard_sample_rate() {
        let ticks = frequency_ticks(44_100);
        assert!(!ticks.is_empty());
        assert_eq!(ticks[0].1, "20k");
        assert_eq!(ticks[1].1, "10k");
    }

    #[test]
    fn test_time_ruler_interval_scaling() {
        assert_eq!(time_ruler_interval(200.0), 30.0);
        assert_eq!(time_ruler_interval(50.0), 10.0);
        assert_eq!(time_ruler_interval(15.0), 2.0);
        assert_eq!(time_ruler_interval(5.0), 0.5);
        assert_eq!(time_ruler_interval(1.0), 0.1);
        assert_eq!(time_ruler_interval(0.2), 0.02);
        assert_eq!(time_ruler_interval(0.05), 0.005);
        assert_eq!(time_ruler_interval(0.005), 0.001);
    }

    #[test]
    fn test_format_time_label() {
        assert_eq!(format_time_label(65.0, 2.0), "01:05");
        assert_eq!(format_time_label(1.25, 0.05), "00:01.25");
        assert_eq!(format_time_label(0.1234, 0.001), "00.123s");
    }

    #[test]
    fn test_viewport_cache_clear() {
        let mut cache = ViewportSpectrogramCache {
            texture: None,
            last_key: Some((0, 100, 500, 1024, 0, 800, 100)),
        };
        cache.clear();
        assert!(cache.texture.is_none());
        assert!(cache.last_key.is_none());
    }
}
