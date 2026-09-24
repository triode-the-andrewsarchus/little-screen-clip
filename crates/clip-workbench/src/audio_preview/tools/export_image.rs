//! Audio inspection image export tool for rendering zoomed waveforms and spectrograms.
//!
//! Generates publication-ready PNG images containing time and frequency/amplitude axes,
//! tick marks, and color-accurate audio visualizations for the active zoom viewport.

use fontdue::Font;
use little_clip_core::audio::types::FullAudioAnalysis;
use little_clip_core::audio::types::apply_audio_gain;
use little_clip_core::palette::AppPalette;
use little_clip_ops::hf_audio::{HfSpectrogramConfig, render_hf_viewport_spectrogram};
use little_clip_render::tiny_skia::{
    Color, IntSize, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

use super::spectrogram_view::{format_time_label, frequency_ticks, time_ruler_interval};

pub const EXPORT_IMAGE_WIDTH: u32 = 1200;
pub const RULER_HEIGHT: f32 = 28.0;
pub const GUTTER_WIDTH: f32 = 56.0;
pub const MARGIN: f32 = 10.0;

#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

/// Converts a hex string into a tiny-skia [`Color`].
pub fn skia_color(hex: &str) -> Color {
    let (r, g, b, a) = little_clip_core::color::parse_hex_color_rgba_or_white(hex);
    Color::from_rgba8(r, g, b, a)
}

/// Converts a hex string with custom alpha into a tiny-skia [`Color`].
pub fn skia_color_alpha(hex: &str, alpha: u8) -> Color {
    let (r, g, b, _) = little_clip_core::color::parse_hex_color_rgba_or_white(hex);
    Color::from_rgba8(r, g, b, alpha)
}

/// Fills a rectangle with the specified color.
pub fn fill_rect(pixmap: &mut Pixmap, rect: Rect, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// Strokes a rectangle with the specified color and stroke width.
pub fn stroke_rect(pixmap: &mut Pixmap, rect: Rect, color: Color, stroke_width: f32) {
    let mut paint = Paint::default();
    paint.set_color(color);
    let mut stroke = Stroke::default();
    stroke.width = stroke_width;
    let path = PathBuilder::from_rect(rect);
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Draws a line segment from `(x1, y1)` to `(x2, y2)`.
pub fn draw_line(
    pixmap: &mut Pixmap,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: Color,
    stroke_width: f32,
) {
    let mut paint = Paint::default();
    paint.set_color(color);
    let mut stroke = Stroke::default();
    stroke.width = stroke_width;
    let mut pb = PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// Draws text onto a tiny-skia pixmap using fontdue.
pub fn draw_text_skia(
    pixmap: &mut Pixmap,
    font: &Font,
    text: &str,
    mut start_x: f32,
    top_y: f32,
    font_size: f32,
    color: [u8; 4],
    align: TextAlign,
) {
    if text.is_empty() {
        return;
    }
    match align {
        TextAlign::Left => {}
        TextAlign::Center => {
            let total_w = measure_text_skia(font, text, font_size);
            start_x -= total_w * 0.5;
        }
        TextAlign::Right => {
            let total_w = measure_text_skia(font, text, font_size);
            start_x -= total_w;
        }
    }

    let mut cur_x = start_x;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, font_size);
        let gx = cur_x + metrics.xmin as f32;
        let gy = top_y + font_size - metrics.ymin as f32 - metrics.height as f32;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let alpha = bitmap[row * metrics.width + col];
                if alpha > 0 {
                    let px = (gx + col as f32).round() as i32;
                    let py = (gy + row as f32).round() as i32;
                    if px >= 0
                        && px < pixmap.width() as i32
                        && py >= 0
                        && py < pixmap.height() as i32
                    {
                        let a = (alpha as u32 * color[3] as u32) / 255;
                        let r = (color[0] as u32 * a) / 255;
                        let g = (color[1] as u32 * a) / 255;
                        let b = (color[2] as u32 * a) / 255;
                        let idx = py as usize * pixmap.width() as usize + px as usize;
                        let dst_pix = &mut pixmap.pixels_mut()[idx];
                        let cur_a = dst_pix.alpha() as u32;
                        let inv_a = 255 - a;
                        let out_r = (r + (dst_pix.red() as u32 * inv_a) / 255).min(255) as u8;
                        let out_g = (g + (dst_pix.green() as u32 * inv_a) / 255).min(255) as u8;
                        let out_b = (b + (dst_pix.blue() as u32 * inv_a) / 255).min(255) as u8;
                        let out_a = (a + (cur_a * inv_a) / 255).min(255) as u8;
                        let c = Color::from_rgba8(out_r, out_g, out_b, out_a)
                            .to_color_u8()
                            .premultiply();
                        *dst_pix = c;
                    }
                }
            }
        }
        cur_x += metrics.advance_width;
    }
}

/// Measures the rendered width of text in points using fontdue.
pub fn measure_text_skia(font: &Font, text: &str, font_size: f32) -> f32 {
    let mut total_w = 0.0;
    for ch in text.chars() {
        let metrics = font.metrics(ch, font_size);
        total_w += metrics.advance_width;
    }
    total_w
}

/// Helper to encode a tiny-skia Pixmap to PNG bytes.
pub fn encode_pixmap_png(pixmap: &Pixmap) -> Result<Vec<u8>, String> {
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Renders a waveform image to PNG bytes for the active zoomed region.
///
/// Includes:
/// - Top timeline ruler with dynamic timestamp labels and tick marks.
/// - Vertical time grid lines through the waveform.
/// - Left gutter with channel labels ("L", "R" or "Mono") and amplitude ticks (+1.0, 0.0, -1.0).
/// - Audacity stem mode when zoomed in, or peak envelope mode when zoomed out.
/// - Respects current non-destructive edits and sample colors.
/// - Excludes playhead, selections, and transient markers.
pub fn render_waveform_image(
    analysis: &FullAudioAnalysis,
    source_analysis: &FullAudioAnalysis,
    region_edits: &[super::super::AudioRegionEdit],
    v_start: f32,
    v_dur: f32,
    duration_sec: f32,
    palette: &AppPalette,
    font_mono: Option<&Font>,
    font_reg: Option<&Font>,
) -> Result<Vec<u8>, String> {
    let is_stereo = analysis.channels >= 2 && analysis.ch2_samples.is_some();
    let total_h: u32 = if is_stereo { 340 } else { 240 };
    let width = EXPORT_IMAGE_WIDTH;

    let mut pixmap = Pixmap::new(width, total_h)
        .ok_or_else(|| "Failed to allocate pixmap buffer for waveform export".to_string())?;

    let bg_canvas = skia_color(&palette.colors.canvas);
    let bg_frame = skia_color(&palette.colors.surface_recessed);
    let border_frame = skia_color(&palette.colors.surface_panel);

    let (f_med_r, f_med_g, f_med_b, _) =
        little_clip_core::color::parse_hex_color_rgba_or_white(&palette.colors.text_secondary);
    let (accent_r, accent_g, accent_b, _) =
        little_clip_core::color::parse_hex_color_rgba_or_white(&palette.harmonies.base);
    let (f_low_r, f_low_g, f_low_b, _) =
        little_clip_core::color::parse_hex_color_rgba_or_white(&palette.colors.text_muted);

    // 1. Canvas background
    pixmap.fill(bg_canvas);

    // 2. Geometry layout
    let track_min_x = MARGIN + GUTTER_WIDTH;
    let track_max_x = width as f32 - MARGIN;
    let track_width = (track_max_x - track_min_x).max(10.0);

    let ruler_y_min = MARGIN;
    let ruler_y_max = ruler_y_min + RULER_HEIGHT;

    let wave_y_min = ruler_y_max + 4.0;
    let wave_y_max = total_h as f32 - MARGIN;
    let wave_h = (wave_y_max - wave_y_min).max(40.0);

    let v_dur = v_dur.max(0.0001);
    let time_to_x = |t: f32| -> f32 { track_min_x + ((t - v_start) / v_dur) * track_width };
    let x_to_time = |x: f32| -> f32 { v_start + ((x - track_min_x) / track_width) * v_dur };

    // 3. Ruler Background & Border
    let ruler_rect = Rect::from_xywh(track_min_x, ruler_y_min, track_width, RULER_HEIGHT).unwrap();
    fill_rect(&mut pixmap, ruler_rect, bg_frame);
    stroke_rect(&mut pixmap, ruler_rect, border_frame, 1.0);

    // 4. Waveform Box Background & Border
    let wave_rect = Rect::from_xywh(track_min_x, wave_y_min, track_width, wave_h).unwrap();
    fill_rect(&mut pixmap, wave_rect, bg_frame);
    stroke_rect(&mut pixmap, wave_rect, border_frame, 1.0);

    // 5. Timeline Ruler Ticks & Time Grid Lines
    let interval_sec = time_ruler_interval(v_dur);
    let first_marker = (v_start / interval_sec).floor() * interval_sec;
    let mut t = first_marker;
    let max_t = v_start + v_dur;
    let grid_color = skia_color_alpha(&palette.colors.text_muted, 35);
    let tick_color = skia_color_alpha(&palette.colors.text_secondary, 140);

    while t <= max_t + interval_sec * 0.5 {
        if t >= v_start {
            let x = time_to_x(t);
            if x >= track_min_x && x <= track_max_x {
                // Vertical grid line through the entire waveform
                draw_line(&mut pixmap, x, wave_y_min, x, wave_y_max, grid_color, 1.0);
                // Ruler tick line
                draw_line(
                    &mut pixmap,
                    x,
                    ruler_y_max - 10.0,
                    x,
                    ruler_y_max,
                    tick_color,
                    1.0,
                );
                // Ruler time label
                if let Some(font) = font_mono.or(font_reg) {
                    let label = format_time_label(t, interval_sec);
                    draw_text_skia(
                        &mut pixmap,
                        font,
                        &label,
                        x + 3.0,
                        ruler_y_min + 6.0,
                        9.0,
                        [f_med_r, f_med_g, f_med_b, 255],
                        TextAlign::Left,
                    );
                }
            }
        }
        t += interval_sec;
    }

    // 6. Gutter Axis Labels & Amplitude Center Lines
    let font_for_labels = font_reg.or(font_mono);
    if is_stereo {
        let half = wave_h / 2.0;
        let y_mid = wave_y_min + half;
        let y_c1 = wave_y_min + half * 0.5;
        let y_c2 = wave_y_min + half * 1.5;

        // Divider between L and R
        draw_line(
            &mut pixmap,
            track_min_x,
            y_mid,
            track_max_x,
            y_mid,
            border_frame,
            1.0,
        );
        // Center lines for L and R
        draw_line(
            &mut pixmap,
            track_min_x,
            y_c1,
            track_max_x,
            y_c1,
            skia_color_alpha(&palette.colors.surface_panel, 90),
            1.0,
        );
        draw_line(
            &mut pixmap,
            track_min_x,
            y_c2,
            track_max_x,
            y_c2,
            skia_color_alpha(&palette.colors.surface_panel, 90),
            1.0,
        );

        if let Some(font) = font_for_labels {
            draw_text_skia(
                &mut pixmap,
                font,
                "L",
                track_min_x - 8.0,
                y_c1 - 6.0,
                11.0,
                [accent_r, accent_g, accent_b, 255],
                TextAlign::Right,
            );
            draw_text_skia(
                &mut pixmap,
                font,
                "R",
                track_min_x - 8.0,
                y_c2 - 6.0,
                11.0,
                [accent_r, accent_g, accent_b, 255],
                TextAlign::Right,
            );
        }
    } else {
        let y_c = wave_y_min + wave_h * 0.5;
        draw_line(
            &mut pixmap,
            track_min_x,
            y_c,
            track_max_x,
            y_c,
            skia_color_alpha(&palette.colors.surface_panel, 90),
            1.0,
        );

        if let Some(font) = font_for_labels {
            draw_text_skia(
                &mut pixmap,
                font,
                "Mono",
                track_min_x - 8.0,
                y_c - 6.0,
                11.0,
                [accent_r, accent_g, accent_b, 255],
                TextAlign::Right,
            );
        }
    }

    // Gutter amplitude ticks (+1.0, 0.0, -1.0)
    if let Some(font) = font_mono.or(font_reg) {
        let text_col = [f_low_r, f_low_g, f_low_b, 200];
        if is_stereo {
            let half = wave_h / 2.0;
            draw_text_skia(
                &mut pixmap,
                font,
                "+1",
                track_min_x - 22.0,
                wave_y_min + 2.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
            draw_text_skia(
                &mut pixmap,
                font,
                "-1",
                track_min_x - 22.0,
                wave_y_min + half - 10.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
            draw_text_skia(
                &mut pixmap,
                font,
                "+1",
                track_min_x - 22.0,
                wave_y_min + half + 2.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
            draw_text_skia(
                &mut pixmap,
                font,
                "-1",
                track_min_x - 22.0,
                wave_y_max - 10.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
        } else {
            draw_text_skia(
                &mut pixmap,
                font,
                "+1.0",
                track_min_x - 14.0,
                wave_y_min + 2.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
            draw_text_skia(
                &mut pixmap,
                font,
                " 0.0",
                track_min_x - 14.0,
                wave_y_min + wave_h * 0.5 - 4.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
            draw_text_skia(
                &mut pixmap,
                font,
                "-1.0",
                track_min_x - 14.0,
                wave_y_max - 10.0,
                8.0,
                text_col,
                TextAlign::Right,
            );
        }
    }

    // 7. Waveform Samples / Peaks
    let total_frames = analysis.total_frames;
    let sample_rate = analysis.sample_rate as f32;

    let start_frame = ((v_start * sample_rate).floor() as isize).max(0) as usize;
    let end_frame = (((v_start + v_dur) * sample_rate).ceil() as usize).min(total_frames);
    let visible_frames = end_frame.saturating_sub(start_frame);
    let px_per_frame = track_width / visible_frames.max(1) as f32;

    let gain_at = |time: f32| {
        region_edits.iter().find_map(|region| {
            let mix = region.effect_mix(time, duration_sec);
            (mix > 0.0).then_some((region, mix))
        })
    };

    let ch_count = if is_stereo { 2 } else { 1 };

    if px_per_frame >= 3.0 {
        // --- STEM MODE (ZOOMED IN TO SAMPLE LEVEL) ---
        for ch_idx in 0..ch_count {
            let (y_base, max_def, samples) = if ch_idx == 0 {
                let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                let base = if is_stereo {
                    wave_y_min + wave_h / 4.0
                } else {
                    wave_y_min + wave_h / 2.0
                };
                (base, half * 0.45, &analysis.ch1_samples)
            } else {
                let half = wave_h / 2.0;
                let base = wave_y_min + 3.0 * wave_h / 4.0;
                let Some(ch2) = analysis.ch2_samples.as_ref() else {
                    continue;
                };
                (base, half * 0.45, ch2)
            };

            let mut prev_pos: Option<(f32, f32)> = None;
            for f in start_frame..=end_frame.min(total_frames.saturating_sub(1)) {
                let t = f as f32 / sample_rate;
                let x = time_to_x(t);
                let val = gain_at(t)
                    .map(|(region, mix)| {
                        let wet = apply_audio_gain(samples[f], region.gain_db, region.soft_clip);
                        samples[f] + (wet - samples[f]) * mix
                    })
                    .unwrap_or(samples[f]);

                let y = y_base - val * max_def;
                let c = source_analysis.query_sample_color(ch_idx, f);
                let point_color = Color::from_rgba8(c[0], c[1], c[2], 255);
                let stem_color = Color::from_rgba8(c[0], c[1], c[2], 120);

                if let Some((px, py)) = prev_pos {
                    draw_line(&mut pixmap, px, py, x, y, point_color, 1.5);
                }

                // Vertical stem line to baseline
                draw_line(&mut pixmap, x, y_base, x, y, stem_color, 1.0);

                // Sample dot
                let dot_rect = Rect::from_xywh(x - 2.5, y - 2.5, 5.0, 5.0).unwrap_or(wave_rect);
                fill_rect(&mut pixmap, dot_rect, point_color);
                stroke_rect(&mut pixmap, dot_rect, border_frame, 0.5);

                prev_pos = Some((x, y));
            }
        }
    } else {
        // --- OVERVIEW / PEAK ENVELOPE MODE ---
        let num_bars = (track_width as usize).clamp(100, track_width as usize);
        let bar_w = (track_width / num_bars as f32).max(1.0);

        for ch_idx in 0..ch_count {
            let (y_base, max_def) = if ch_idx == 0 {
                let half = if is_stereo { wave_h / 2.0 } else { wave_h };
                let base = if is_stereo {
                    wave_y_min + wave_h / 4.0
                } else {
                    wave_y_min + wave_h / 2.0
                };
                (base, half * 0.45)
            } else {
                let half = wave_h / 2.0;
                let base = wave_y_min + 3.0 * wave_h / 4.0;
                (base, half * 0.45)
            };

            for b in 0..num_bars {
                let bar_x = track_min_x + b as f32 * bar_w;
                let t_start = x_to_time(bar_x);
                let t_end = x_to_time(bar_x + bar_w);
                let t_mid = (t_start + t_end) * 0.5;

                let f_s = ((t_start * sample_rate) as usize).min(total_frames);
                let f_e = (((t_end * sample_rate) as usize) + 1).min(total_frames);

                let (min_s, max_s, block_col) = analysis.query_peaks(ch_idx, f_s, f_e);
                let (min_s, max_s) = gain_at(t_mid)
                    .map(|(region, mix)| {
                        let wet_min = apply_audio_gain(min_s, region.gain_db, region.soft_clip);
                        let wet_max = apply_audio_gain(max_s, region.gain_db, region.soft_clip);
                        (
                            min_s + (wet_min - min_s) * mix,
                            max_s + (wet_max - max_s) * mix,
                        )
                    })
                    .unwrap_or((min_s, max_s));

                let bar_color = Color::from_rgba8(block_col[0], block_col[1], block_col[2], 255);
                let y_top = y_base - max_s * max_def;
                let y_bot = y_base - min_s * max_def;
                let h = (y_bot - y_top).max(1.0);

                if let Some(bar_rect) = Rect::from_xywh(bar_x, y_top, bar_w.max(1.0), h) {
                    fill_rect(&mut pixmap, bar_rect, bar_color);
                }
            }
        }
    }

    encode_pixmap_png(&pixmap)
}

/// Renders a high-resolution spectrogram image to PNG bytes for the active zoomed region.
///
/// Includes:
/// - Top timeline ruler with dynamic timestamp labels and tick marks.
/// - Vertical time grid lines through the spectrogram.
/// - Left gutter with Nyquist-aware frequency tick labels (e.g. 20k, 10k, 5k, 1k, 100 Hz).
/// - Horizontal frequency grid lines across the heatmap.
/// - Frequency energy heatmap (Inferno palette) computed via STFT for the visible zoom view.
/// - Excludes playhead, selections, and transient markers.
pub fn render_spectrum_image(
    analysis: &FullAudioAnalysis,
    v_start: f32,
    v_dur: f32,
    duration_sec: f32,
    palette: &AppPalette,
    font_mono: Option<&Font>,
    font_reg: Option<&Font>,
) -> Result<Vec<u8>, String> {
    let width = EXPORT_IMAGE_WIDTH;
    let total_h: u32 = 440;

    let mut pixmap = Pixmap::new(width, total_h)
        .ok_or_else(|| "Failed to allocate pixmap buffer for spectrogram export".to_string())?;

    let bg_canvas = skia_color(&palette.colors.canvas);
    let bg_frame = skia_color(&palette.colors.surface_recessed);
    let border_frame = skia_color(&palette.colors.surface_panel);

    let (f_med_r, f_med_g, f_med_b, _) =
        little_clip_core::color::parse_hex_color_rgba_or_white(&palette.colors.text_secondary);

    // 1. Canvas background
    pixmap.fill(bg_canvas);

    // 2. Geometry layout
    let track_min_x = MARGIN + GUTTER_WIDTH;
    let track_max_x = width as f32 - MARGIN;
    let track_width = (track_max_x - track_min_x).max(10.0);

    let ruler_y_min = MARGIN;
    let ruler_y_max = ruler_y_min + RULER_HEIGHT;

    let spec_y_min = ruler_y_max + 4.0;
    let spec_y_max = total_h as f32 - MARGIN;
    let spec_h = (spec_y_max - spec_y_min).max(40.0);

    let v_dur = v_dur.max(0.0001);
    let time_to_x = |t: f32| -> f32 { track_min_x + ((t - v_start) / v_dur) * track_width };

    // 3. Ruler Background & Border
    let ruler_rect = Rect::from_xywh(track_min_x, ruler_y_min, track_width, RULER_HEIGHT).unwrap();
    fill_rect(&mut pixmap, ruler_rect, bg_frame);
    stroke_rect(&mut pixmap, ruler_rect, border_frame, 1.0);

    // 4. Compute and Render Spectrogram Heatmap Texture
    let total_dur = duration_sec.max(0.01);
    let fft_size = if total_dur >= 600.0 { 512 } else { 1024 };
    let cfg = HfSpectrogramConfig {
        fft_size,
        gain_db: 0.0,
        range_db: 80.0,
        contrast: 1.0,
    };

    // If stereo, mix channels so signal on both L and R appears in the spectrum
    let samples_storage: Vec<f32>;
    let samples: &[f32] = if let Some(ref ch2) = analysis.ch2_samples {
        samples_storage = analysis
            .ch1_samples
            .iter()
            .zip(ch2.iter())
            .map(|(a, b)| (a + b) * 0.5)
            .collect();
        &samples_storage
    } else {
        &analysis.ch1_samples
    };

    let (tex_w, tex_h, rgba) = render_hf_viewport_spectrogram(
        samples,
        analysis.sample_rate,
        v_start,
        v_dur,
        track_width as usize,
        cfg,
    );

    // Draw spectrogram heatmap into `[track_min_x, spec_y_min, track_width, spec_h]`
    if let Some(img_size) = IntSize::from_wh(tex_w as u32, tex_h as u32) {
        if let Some(img_pixmap) = Pixmap::from_vec(rgba, img_size) {
            let scale_x = track_width / tex_w as f32;
            let scale_y = spec_h / tex_h as f32;
            let transform =
                Transform::from_translate(track_min_x, spec_y_min).pre_scale(scale_x, scale_y);
            pixmap.draw_pixmap(
                0,
                0,
                img_pixmap.as_ref(),
                &PixmapPaint::default(),
                transform,
                None,
            );
        }
    }

    // 5. Frequency Axis Ticks & Horizontal Grid Lines
    let nyquist = analysis.sample_rate as f32 * 0.5;
    let ticks = frequency_ticks(analysis.sample_rate);
    let mut last_y = f32::NEG_INFINITY;
    let grid_stroke_color = skia_color_alpha(&palette.colors.surface_panel, 120);

    for (freq_hz, lbl) in ticks {
        if freq_hz >= nyquist {
            continue;
        }
        let y = spec_y_max - (freq_hz / nyquist) * spec_h;
        if y - last_y < 14.0 {
            continue;
        }
        last_y = y;

        // Horizontal frequency grid line across heatmap
        draw_line(
            &mut pixmap,
            track_min_x,
            y,
            track_max_x,
            y,
            grid_stroke_color,
            1.0,
        );

        // Frequency tick label in left gutter
        if let Some(font) = font_mono.or(font_reg) {
            draw_text_skia(
                &mut pixmap,
                font,
                &lbl,
                track_min_x - 6.0,
                y - 5.0,
                9.0,
                [f_med_r, f_med_g, f_med_b, 255],
                TextAlign::Right,
            );
        }
    }

    // 6. Timeline Ruler Ticks & Vertical Time Grid Lines
    let interval_sec = time_ruler_interval(v_dur);
    let first_marker = (v_start / interval_sec).floor() * interval_sec;
    let mut t = first_marker;
    let max_t = v_start + v_dur;
    let time_grid_color = skia_color_alpha(&palette.colors.text_muted, 35);
    let tick_color = skia_color_alpha(&palette.colors.text_secondary, 140);

    while t <= max_t + interval_sec * 0.5 {
        if t >= v_start {
            let x = time_to_x(t);
            if x >= track_min_x && x <= track_max_x {
                // Vertical grid line through heatmap
                draw_line(
                    &mut pixmap,
                    x,
                    spec_y_min,
                    x,
                    spec_y_max,
                    time_grid_color,
                    1.0,
                );
                // Ruler tick line
                draw_line(
                    &mut pixmap,
                    x,
                    ruler_y_max - 10.0,
                    x,
                    ruler_y_max,
                    tick_color,
                    1.0,
                );
                // Ruler time label
                if let Some(font) = font_mono.or(font_reg) {
                    let label = format_time_label(t, interval_sec);
                    draw_text_skia(
                        &mut pixmap,
                        font,
                        &label,
                        x + 3.0,
                        ruler_y_min + 6.0,
                        9.0,
                        [f_med_r, f_med_g, f_med_b, 255],
                        TextAlign::Left,
                    );
                }
            }
        }
        t += interval_sec;
    }

    // 7. Spectrum border
    let spec_rect = Rect::from_xywh(track_min_x, spec_y_min, track_width, spec_h).unwrap();
    stroke_rect(&mut pixmap, spec_rect, border_frame, 1.0);

    encode_pixmap_png(&pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::audio::types::PeakBlock;
    use little_clip_core::palette::AppPalette;

    fn make_mock_analysis(stereo: bool) -> FullAudioAnalysis {
        let sample_rate = 44_100;
        let total_frames = 44_100;
        let mut ch1 = Vec::with_capacity(total_frames);
        let mut ch2 = Vec::with_capacity(total_frames);
        for i in 0..total_frames {
            let t = i as f32 / sample_rate as f32;
            ch1.push((2.0 * std::f32::consts::PI * 440.0 * t).sin());
            ch2.push((2.0 * std::f32::consts::PI * 880.0 * t).sin());
        }

        let peaks_lvl0 = vec![
            PeakBlock {
                min: -1.0,
                max: 1.0,
                rgb: [200, 100, 50],
            };
            total_frames / 64
        ];

        FullAudioAnalysis {
            channels: if stereo { 2 } else { 1 },
            sample_rate,
            duration_ms: 1000,
            total_frames,
            ch1_samples: ch1,
            ch2_samples: if stereo { Some(ch2) } else { None },
            overview_peaks1: vec![1.0; 200],
            overview_peaks2: if stereo { Some(vec![1.0; 200]) } else { None },
            peaks_lvl0_ch1: peaks_lvl0.clone(),
            peaks_lvl0_ch2: if stereo {
                Some(peaks_lvl0.clone())
            } else {
                None
            },
            peaks_lvl1_ch1: Vec::new(),
            peaks_lvl1_ch2: None,
            peaks_lvl2_ch1: Vec::new(),
            peaks_lvl2_ch2: None,
            spectrogram_width: 100,
            spectrogram_height: 50,
            spectrogram_rgba: vec![0u8; 100 * 50 * 4],
            note_strip_width: 100,
            note_strip_height: 20,
            note_strip_rgba: vec![0u8; 100 * 20 * 4],
            note_strip_info: Vec::new(),
            spectral_colors: vec![[100, 100, 100]; 100],
            transients: vec![0.1, 0.5],
            is_transients_ready: true,
            is_spectrogram_ready: true,
            ultrasonic_roi: Vec::new(),
            ultrasonic_peak_times: Vec::new(),
            is_truncated: false,
            truncation_warning: None,
        }
    }

    #[test]
    fn test_render_waveform_image_stereo_png() {
        let analysis = make_mock_analysis(true);
        let palette = AppPalette::default();
        let (font_reg, _, _, font_mono) = little_clip_render::ScreenRenderer::load_fonts();

        let png = render_waveform_image(
            &analysis,
            &analysis,
            &[],
            0.0,
            1.0,
            1.0,
            &palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        )
        .expect("Stereo waveform render should succeed");

        assert!(!png.is_empty());
        // Verify PNG magic header: 0x89 'P' 'N' 'G' '\r' '\n' 0x1A '\n'
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn test_render_waveform_image_mono_png() {
        let analysis = make_mock_analysis(false);
        let palette = AppPalette::default();
        let (font_reg, _, _, font_mono) = little_clip_render::ScreenRenderer::load_fonts();

        let png = render_waveform_image(
            &analysis,
            &analysis,
            &[],
            0.0,
            1.0,
            1.0,
            &palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        )
        .expect("Mono waveform render should succeed");

        assert!(!png.is_empty());
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn test_render_spectrum_image_png() {
        let analysis = make_mock_analysis(true);
        let palette = AppPalette::default();
        let (font_reg, _, _, font_mono) = little_clip_render::ScreenRenderer::load_fonts();

        let png = render_spectrum_image(
            &analysis,
            0.2,
            0.5,
            1.0,
            &palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        )
        .expect("Spectrogram render should succeed");

        assert!(!png.is_empty());
        assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn test_zoomed_region_variation() {
        let analysis = make_mock_analysis(true);
        let palette = AppPalette::default();
        let (font_reg, _, _, font_mono) = little_clip_render::ScreenRenderer::load_fonts();

        let png1 = render_spectrum_image(
            &analysis,
            0.0,
            0.5,
            1.0,
            &palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        )
        .unwrap();

        let png2 = render_spectrum_image(
            &analysis,
            0.5,
            0.5,
            1.0,
            &palette,
            font_mono.as_ref(),
            font_reg.as_ref(),
        )
        .unwrap();

        assert_ne!(
            png1, png2,
            "Different zoom regions must generate distinct renders"
        );
    }
}
