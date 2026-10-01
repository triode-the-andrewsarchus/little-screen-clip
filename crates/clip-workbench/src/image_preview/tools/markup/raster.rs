use super::*;

/// Rasterizes all placed vector markup layers onto a clean clone of the input image.
pub fn rasterize_markup(base: &RgbaImage, items: &[MarkupShape]) -> RgbaImage {
    if items.is_empty() {
        return base.clone();
    }
    let (w, h) = (base.width(), base.height());
    if w == 0 || h == 0 {
        return base.clone();
    }

    let mut pixmap = match tiny_skia::Pixmap::new(w, h) {
        Some(p) => p,
        None => return base.clone(),
    };

    let (font_reg, font_bold, _, _) = ScreenRenderer::load_fonts();
    let font_ref = font_bold.as_ref().or(font_reg.as_ref());

    for item in items {
        match item {
            MarkupShape::Arrow {
                start,
                end,
                color,
                width,
            } => {
                let sx = start.x * w as f32;
                let sy = start.y * h as f32;
                let ex = end.x * w as f32;
                let ey = end.y * h as f32;

                let mut stroke = tiny_skia::Stroke::default();
                stroke.width = *width;
                stroke.line_cap = tiny_skia::LineCap::Round;

                let mut pb = tiny_skia::PathBuilder::new();
                pb.move_to(sx, sy);
                pb.line_to(ex, ey);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.stroke_path(
                        &path,
                        &paint,
                        &stroke,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }

                // Arrow head
                let angle = (ey - sy).atan2(ex - sx);
                let head_len = (*width * 3.5).clamp(12.0, 48.0);
                let head_angle = 30.0_f32.to_radians();
                let p1x = ex - (angle - head_angle).cos() * head_len;
                let p1y = ey - (angle - head_angle).sin() * head_len;
                let p2x = ex - (angle + head_angle).cos() * head_len;
                let p2y = ey - (angle + head_angle).sin() * head_len;

                let mut head_pb = tiny_skia::PathBuilder::new();
                head_pb.move_to(ex, ey);
                head_pb.line_to(p1x, p1y);
                head_pb.line_to(p2x, p2y);
                head_pb.close();
                if let Some(head_path) = head_pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.fill_path(
                        &head_path,
                        &paint,
                        tiny_skia::FillRule::Winding,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
            MarkupShape::Line {
                start,
                end,
                color,
                width,
            } => {
                let sx = start.x * w as f32;
                let sy = start.y * h as f32;
                let ex = end.x * w as f32;
                let ey = end.y * h as f32;

                let mut stroke = tiny_skia::Stroke::default();
                stroke.width = *width;
                stroke.line_cap = tiny_skia::LineCap::Round;

                let mut pb = tiny_skia::PathBuilder::new();
                pb.move_to(sx, sy);
                pb.line_to(ex, ey);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.stroke_path(
                        &path,
                        &paint,
                        &stroke,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
            MarkupShape::Rectangle {
                rect,
                color,
                width,
                filled,
                ..
            } => {
                let rx = rect.min.x * w as f32;
                let ry = rect.min.y * h as f32;
                let rw = (rect.width() * w as f32).max(1.0);
                let rh = (rect.height() * h as f32).max(1.0);

                if let Some(skia_rect) = tiny_skia::Rect::from_xywh(rx, ry, rw, rh) {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    if *filled {
                        pixmap.fill_rect(skia_rect, &paint, tiny_skia::Transform::identity(), None);
                    } else {
                        let mut stroke = tiny_skia::Stroke::default();
                        stroke.width = *width;
                        let path = tiny_skia::PathBuilder::from_rect(skia_rect);
                        pixmap.stroke_path(
                            &path,
                            &paint,
                            &stroke,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
            }
            MarkupShape::Circle {
                rect,
                color,
                width,
                filled,
            } => {
                let cx = (rect.min.x + rect.width() * 0.5) * w as f32;
                let cy = (rect.min.y + rect.height() * 0.5) * h as f32;
                let rad = ((rect.width().min(rect.height()) * 0.5) * w as f32).max(1.0);

                let mut pb = tiny_skia::PathBuilder::new();
                pb.push_circle(cx, cy, rad);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    if *filled {
                        pixmap.fill_path(
                            &path,
                            &paint,
                            tiny_skia::FillRule::Winding,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    } else {
                        let mut stroke = tiny_skia::Stroke::default();
                        stroke.width = *width;
                        pixmap.stroke_path(
                            &path,
                            &paint,
                            &stroke,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
            }
            MarkupShape::Highlighter { rect, color } => {
                let rx = rect.min.x * w as f32;
                let ry = rect.min.y * h as f32;
                let rw = (rect.width() * w as f32).max(1.0);
                let rh = (rect.height() * h as f32).max(1.0);

                if let Some(skia_rect) = tiny_skia::Rect::from_xywh(rx, ry, rw, rh) {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), 90);
                    pixmap.fill_rect(skia_rect, &paint, tiny_skia::Transform::identity(), None);
                }
            }
            MarkupShape::StepBadge {
                center,
                radius,
                number,
                color,
            } => {
                let cx = center.x * w as f32;
                let cy = center.y * h as f32;
                let r_px = if *radius <= 1.0_f32 {
                    *radius * h as f32
                } else {
                    *radius
                };
                let badge_rad = r_px.max(8.0_f32);

                let mut pb = tiny_skia::PathBuilder::new();
                pb.push_circle(cx, cy, badge_rad);
                if let Some(path) = pb.finish() {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.fill_path(
                        &path,
                        &paint,
                        tiny_skia::FillRule::Winding,
                        tiny_skia::Transform::identity(),
                        None,
                    );

                    let mut stroke = tiny_skia::Stroke::default();
                    stroke.width = (badge_rad * 0.1_f32).clamp(1.5_f32, 16.0_f32);
                    let mut border_paint = tiny_skia::Paint::default();
                    border_paint.set_color_rgba8(255, 255, 255, 220);
                    pixmap.stroke_path(
                        &path,
                        &border_paint,
                        &stroke,
                        tiny_skia::Transform::identity(),
                        None,
                    );

                    if let Some(font) = font_ref {
                        let text_str = number.to_string();
                        let font_size = step_badge_font_size(badge_rad, *number);
                        draw_text_skia(
                            &mut pixmap,
                            font,
                            &text_str,
                            cx,
                            cy - font_size * 0.46_f32,
                            font_size,
                            [255, 255, 255, 255],
                            true,
                        );
                    }
                }
            }
            MarkupShape::Text {
                pos,
                text,
                font_size,
            } => {
                if text.is_empty() {
                    continue;
                }
                let tx = pos.x * w as f32;
                let ty = pos.y * h as f32;
                let pad_x = (*font_size * 0.35).clamp(6.0, 24.0);
                let pad_y = (*font_size * 0.15).clamp(3.0, 12.0);

                if let Some(font) = font_ref {
                    let text_w = measure_text_skia(font, text, *font_size);
                    let text_h = *font_size * 1.25;
                    let box_w = text_w + pad_x * 2.0;
                    let box_h = text_h + pad_y * 2.0;

                    if let Some(box_rect) = tiny_skia::Rect::from_xywh(tx, ty, box_w, box_h) {
                        let mut bg_paint = tiny_skia::Paint::default();
                        bg_paint.set_color_rgba8(12, 12, 12, 245);
                        pixmap.fill_rect(
                            box_rect,
                            &bg_paint,
                            tiny_skia::Transform::identity(),
                            None,
                        );

                        let mut stroke = tiny_skia::Stroke::default();
                        stroke.width = 1.5;
                        let mut stroke_paint = tiny_skia::Paint::default();
                        stroke_paint.set_color_rgba8(75, 75, 75, 255);
                        let border_path = tiny_skia::PathBuilder::from_rect(box_rect);
                        pixmap.stroke_path(
                            &border_path,
                            &stroke_paint,
                            &stroke,
                            tiny_skia::Transform::identity(),
                            None,
                        );

                        draw_text_skia(
                            &mut pixmap,
                            font,
                            text,
                            tx + pad_x,
                            ty + pad_y,
                            *font_size,
                            [255, 255, 255, 255],
                            false,
                        );
                    }
                }
            }
        }
    }

    // Blend pixmap cleanly onto clone of base image
    let mut out = base.clone();
    for y in 0..h {
        for x in 0..w {
            if let Some(pixel) = pixmap.pixel(x, y) {
                let a = pixel.alpha() as u32;
                if a > 0 {
                    let r = pixel.red() as u32;
                    let g = pixel.green() as u32;
                    let b = pixel.blue() as u32;
                    let dst = out.get_pixel_mut(x, y);
                    let inv_a = 255 - a;
                    dst[0] = ((r * 255 + dst[0] as u32 * inv_a) / 255).min(255) as u8;
                    dst[1] = ((g * 255 + dst[1] as u32 * inv_a) / 255).min(255) as u8;
                    dst[2] = ((b * 255 + dst[2] as u32 * inv_a) / 255).min(255) as u8;
                    dst[3] = (a + (dst[3] as u32 * inv_a) / 255).min(255) as u8;
                }
            }
        }
    }
    out
}

pub(super) fn draw_text_skia(
    pixmap: &mut tiny_skia::Pixmap,
    font: &little_clip_render::fontdue::Font,
    text: &str,
    mut start_x: f32,
    base_y: f32,
    font_size: f32,
    color: [u8; 4],
    center_align: bool,
) {
    if center_align {
        let total_w = measure_text_skia(font, text, font_size);
        start_x -= total_w * 0.5;
    }
    let mut cur_x = start_x;
    for ch in text.chars() {
        let (metrics, bitmap) = font.rasterize(ch, font_size);
        let gx = cur_x + metrics.xmin as f32;
        let gy = base_y + font_size - metrics.ymin as f32 - metrics.height as f32;
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
                        let c = tiny_skia::Color::from_rgba8(out_r, out_g, out_b, out_a)
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

pub(super) fn measure_text_skia(
    font: &little_clip_render::fontdue::Font,
    text: &str,
    font_size: f32,
) -> f32 {
    let mut total_w = 0.0;
    for ch in text.chars() {
        let metrics = font.metrics(ch, font_size);
        total_w += metrics.advance_width;
    }
    total_w
}
