//! Drawing primitives for text measurement, clipped text rendering, badges, and layout frames.

use crate::{FontStyle, ScreenRenderer, is_zero_width_or_bom};
use tiny_skia::*;

/// Structured text split into a prefix, elision glyph (e.g. `▸`), and optional suffix (e.g. file extension).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElidedText {
    /// Leading text before elision.
    pub prefix: String,
    /// Elision glyph or marker string.
    pub elision: Option<String>,
    /// Trailing suffix preserved after elision (e.g. `.png`).
    pub suffix: Option<String>,
}

impl ElidedText {
    /// Reconstitutes the prefix, elision glyph, and suffix into a single string.
    pub fn full_text(&self) -> String {
        format!(
            "{}{}{}",
            self.prefix,
            self.elision.as_deref().unwrap_or(""),
            self.suffix.as_deref().unwrap_or("")
        )
    }
}

impl ScreenRenderer {
    /// Draws a subtle theme-aware checkerboard pattern for transparent image previews.
    pub fn draw_checkerboard_pattern(
        &self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        cell_size: f32,
    ) {
        let c1 = self.color_from_hex(&self.palette.colors.surface_recessed);
        let c2 = self.color_from_hex(&self.palette.colors.surface_panel);
        let mut p1 = Paint::default();
        p1.set_color(c1);
        let mut p2 = Paint::default();
        p2.set_color(c2);

        let cols = (w / cell_size).ceil() as usize;
        let rows = (h / cell_size).ceil() as usize;

        for r in 0..rows {
            let cy = y + r as f32 * cell_size;
            let ch = (h - (r as f32 * cell_size)).min(cell_size);
            if ch <= 0.0 {
                break;
            }
            for c in 0..cols {
                let cx = x + c as f32 * cell_size;
                let cw = (w - (c as f32 * cell_size)).min(cell_size);
                if cw <= 0.0 {
                    break;
                }
                if let Some(rect) = Rect::from_xywh(cx, cy, cw, ch) {
                    let paint = if (r + c) % 2 == 0 { &p1 } else { &p2 };
                    pixmap.fill_rect(rect, paint, Transform::identity(), None);
                }
            }
        }
    }

    pub fn measure_text_width_styled(&self, text: &str, size: f32, style: FontStyle) -> f32 {
        let font = match style {
            FontStyle::Regular => self.font_regular.as_ref().or(self.font_bold.as_ref()),
            FontStyle::Bold => self.font_bold.as_ref().or(self.font_regular.as_ref()),
            FontStyle::Italic => self.font_italic.as_ref().or(self.font_regular.as_ref()),
            FontStyle::BoldItalic => self
                .font_bold
                .as_ref()
                .or(self.font_italic.as_ref())
                .or(self.font_regular.as_ref()),
            FontStyle::Mono => self.font_mono.as_ref().or(self.font_regular.as_ref()),
        };

        if let Some(font) = font {
            text.chars()
                .filter(|&ch| !is_zero_width_or_bom(ch))
                .map(|ch| font.metrics(ch, size).advance_width)
                .sum()
        } else {
            text.chars().filter(|&ch| !is_zero_width_or_bom(ch)).count() as f32 * size * 0.6
        }
    }

    /// Ultra-fast TrueType text rasterizer with direct buffer blending.
    pub fn measure_text_width(&self, text: &str, size: f32, bold: bool) -> f32 {
        self.measure_text_width_styled(
            text,
            size,
            if bold {
                FontStyle::Bold
            } else {
                FontStyle::Regular
            },
        )
    }

    /// Truncates a filename or text string with a configurable single-character elision glyph (default '▸'),
    /// preserving the file extension when present and fitting precisely within max_width_px.
    pub fn truncate_filename_elided(
        &self,
        name: &str,
        max_width_px: f32,
        size: f32,
        bold: bool,
    ) -> ElidedText {
        if self.measure_text_width(name, size, bold) <= max_width_px {
            return ElidedText {
                prefix: name.to_string(),
                elision: None,
                suffix: None,
            };
        }

        let elision_glyph = if self.config.display.elision_char.is_empty() {
            "▸"
        } else {
            &self.config.display.elision_char
        };
        let elision_w = self.measure_text_width(elision_glyph, size, bold);

        // Check if there is a recognizable file extension (e.g. .jpg, .png, .gif, .mp4, .rs)
        let dot_pos = name.rfind('.');
        if let Some(dot_idx) = dot_pos {
            let ext = &name[dot_idx..];
            if ext.len() >= 2 && ext.len() <= 6 && !ext.contains(' ') && dot_idx > 0 {
                let stem = &name[..dot_idx];
                let ext_w = self.measure_text_width(ext, size, bold);

                if ext_w + elision_w < max_width_px {
                    let avail_stem_w = max_width_px - ext_w - elision_w;
                    let mut stem_prefix = String::new();
                    let mut curr_w = 0.0;

                    for ch in stem.chars() {
                        let ch_w = self.measure_text_width(&ch.to_string(), size, bold);
                        if curr_w + ch_w > avail_stem_w {
                            break;
                        }
                        stem_prefix.push(ch);
                        curr_w += ch_w;
                    }

                    if !stem_prefix.is_empty() {
                        return ElidedText {
                            prefix: stem_prefix,
                            elision: Some(elision_glyph.to_string()),
                            suffix: Some(ext.to_string()),
                        };
                    }
                }
            }
        }

        // Fallback: general tail elision
        let mut prefix = String::new();
        let mut curr_w = 0.0;
        let avail_w = (max_width_px - elision_w).max(0.0);

        for ch in name.chars() {
            let ch_w = self.measure_text_width(&ch.to_string(), size, bold);
            if curr_w + ch_w > avail_w {
                break;
            }
            prefix.push(ch);
            curr_w += ch_w;
        }

        if prefix.is_empty() {
            ElidedText {
                prefix: name
                    .chars()
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                elision: Some(elision_glyph.to_string()),
                suffix: None,
            }
        } else {
            ElidedText {
                prefix,
                elision: Some(elision_glyph.to_string()),
                suffix: None,
            }
        }
    }

    /// Draws TrueType text onto the pixmap using the specified style, size, and right clipping boundary.
    pub fn draw_text_styled_clipped(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        style: FontStyle,
        clip_right: f32,
    ) -> f32 {
        let font = match style {
            FontStyle::Regular => self.font_regular.as_ref().or(self.font_bold.as_ref()),
            FontStyle::Bold => self.font_bold.as_ref().or(self.font_regular.as_ref()),
            FontStyle::Italic => self.font_italic.as_ref().or(self.font_regular.as_ref()),
            FontStyle::BoldItalic => self
                .font_bold
                .as_ref()
                .or(self.font_italic.as_ref())
                .or(self.font_regular.as_ref()),
            FontStyle::Mono => self.font_mono.as_ref().or(self.font_regular.as_ref()),
        };

        if let Some(font) = font {
            let mut curr_x = x;
            let cr = (color.red() * 255.0) as u32;
            let cg = (color.green() * 255.0) as u32;
            let cb = (color.blue() * 255.0) as u32;
            let base_alpha = color.alpha() * 255.0;

            let pix_w = pixmap.width() as i32;
            let pix_h = pixmap.height() as i32;
            let data = pixmap.data_mut();

            for ch in text.chars() {
                if is_zero_width_or_bom(ch) {
                    continue;
                }
                if curr_x >= clip_right {
                    break;
                }

                let (metrics, bitmap) = font.rasterize(ch, size);
                if metrics.width > 0 && metrics.height > 0 {
                    let gx = curr_x + metrics.xmin as f32;
                    let gy = y + size - metrics.height as f32 - metrics.ymin as f32;

                    for row in 0..metrics.height {
                        let py = (gy + row as f32) as i32;
                        if py < 0 || py >= pix_h {
                            continue;
                        }
                        let row_offset = (py as usize * pix_w as usize) * 4;

                        for col in 0..metrics.width {
                            let alpha = bitmap[row * metrics.width + col];
                            if alpha == 0 {
                                continue;
                            }
                            let px = (gx + col as f32) as i32;
                            if px < 0 || px >= pix_w || px >= clip_right as i32 {
                                continue;
                            }

                            let idx = row_offset + px as usize * 4;
                            let a_norm = ((base_alpha * (alpha as f32 / 255.0)) as u32).min(255);

                            if a_norm == 255 {
                                data[idx] = cr as u8;
                                data[idx + 1] = cg as u8;
                                data[idx + 2] = cb as u8;
                                data[idx + 3] = 255;
                            } else if a_norm > 0 {
                                let inv_a = 255 - a_norm;
                                data[idx] = ((cr * a_norm + data[idx] as u32 * inv_a) / 255) as u8;
                                data[idx + 1] =
                                    ((cg * a_norm + data[idx + 1] as u32 * inv_a) / 255) as u8;
                                data[idx + 2] =
                                    ((cb * a_norm + data[idx + 2] as u32 * inv_a) / 255) as u8;
                                data[idx + 3] = 255;
                            }
                        }
                    }
                }
                curr_x += metrics.advance_width;
            }
            curr_x
        } else {
            x
        }
    }

    pub(crate) fn draw_text_clipped(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        bold: bool,
        clip_right: f32,
    ) -> f32 {
        let style = if bold {
            FontStyle::Bold
        } else {
            FontStyle::Regular
        };
        self.draw_text_styled_clipped(pixmap, text, x, y, size, color, style, clip_right)
    }

    pub(crate) fn draw_elided_text_clipped(
        &self,
        pixmap: &mut Pixmap,
        elided: &ElidedText,
        x: f32,
        y: f32,
        size: f32,
        base_color: Color,
        elision_color: Color,
        bold: bool,
        clip_right: f32,
    ) -> f32 {
        let mut cx = x;
        cx = self.draw_text_clipped(
            pixmap,
            &elided.prefix,
            cx,
            y,
            size,
            base_color,
            bold,
            clip_right,
        );
        if let Some(ref elision_str) = elided.elision {
            cx = self.draw_text_clipped(
                pixmap,
                elision_str,
                cx,
                y,
                size,
                elision_color,
                bold,
                clip_right,
            );
        }
        if let Some(ref suffix_str) = elided.suffix {
            cx = self.draw_text_clipped(
                pixmap, suffix_str, cx, y, size, base_color, bold, clip_right,
            );
        }
        cx
    }

    pub(crate) fn draw_footer_badge(
        &self,
        pixmap: &mut Pixmap,
        badge_text: &str,
        x: f32,
        y: f32,
        size: f32,
        clip_right: f32,
    ) {
        let badge_w = self.measure_text_width(badge_text, size - 2.0, true) + 16.0;
        let badge_h = size + 4.0;
        let bg_color = self.color_from_hex(&self.palette.colors.surface_recessed);
        let border_color = self.color_from_hex(&self.palette.colors.surface_panel);
        let text_color = self.contrast_color(
            bg_color,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        if let Some(rect) = Rect::from_xywh(x, y - 2.0, badge_w.min(clip_right - x), badge_h) {
            let mut paint = Paint::default();
            paint.set_color(bg_color);
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);

            let mut stroke = Stroke::default();
            stroke.width = 1.0;
            let mut stroke_paint = Paint::default();
            stroke_paint.set_color(border_color);
            let path = PathBuilder::from_rect(rect);
            pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
        }

        self.draw_text_clipped(
            pixmap,
            badge_text,
            x + 8.0,
            y,
            size - 2.0,
            text_color,
            true,
            clip_right,
        );
    }

    pub(crate) fn draw_pill_badge(
        &self,
        pixmap: &mut Pixmap,
        badge_text: &str,
        x: f32,
        y: f32,
        size: f32,
        text_color: Color,
        bg_color: Color,
        clip_right: f32,
    ) {
        // Strip emojis that lack font glyphs to prevent missing glyph open boxes (▯)
        let clean_text = badge_text
            .trim_start_matches(|c: char| {
                !c.is_ascii_alphanumeric() && c != '[' && c != '(' && c != '#'
            })
            .trim();
        let font_size = size;
        let pad_x = 8.0;
        let pad_y = 3.0;
        let text_w = self.measure_text_width(clean_text, font_size, true);
        let badge_w = text_w + pad_x * 2.0;
        let badge_h = font_size + pad_y * 2.0 + 2.0;

        // Ensure high contrast between text and pill background
        let text_color = self.contrast_color(bg_color, Some(text_color));

        if let Some(rect) = Rect::from_xywh(x, y, badge_w.min(clip_right - x), badge_h) {
            let mut paint = Paint::default();
            paint.set_color(bg_color);
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }

        self.draw_text_clipped(
            pixmap,
            clean_text,
            x + pad_x,
            y + pad_y - 1.0,
            font_size,
            text_color,
            true,
            clip_right,
        );
    }

    pub(crate) fn draw_card_viewport_frame(
        &self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> Option<Rect> {
        let rect = Rect::from_xywh(x, y, w, h)?;
        let mut bg_paint = Paint::default();
        let bg_color = self.color_from_hex(&self.palette.colors.canvas);
        bg_paint.set_color(bg_color);
        pixmap.fill_rect(rect, &bg_paint, Transform::identity(), None);

        let mut border_stroke = Stroke::default();
        border_stroke.width = 1.0;
        let mut border_paint = Paint::default();
        let border_color = self.color_from_hex(&self.palette.colors.surface_panel);
        border_paint.set_color(border_color);
        let path = PathBuilder::from_rect(rect);
        pixmap.stroke_path(
            &path,
            &border_paint,
            &border_stroke,
            Transform::identity(),
            None,
        );
        Some(rect)
    }

    pub(crate) fn draw_text(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        bold: bool,
    ) {
        let max_w = pixmap.width() as f32;
        self.draw_text_clipped(pixmap, text, x, y, size, color, bold, max_w);
    }
}
