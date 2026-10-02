use super::*;

// The display and workbench share the same play SVG. Rasterize it once at badge size.
static PLAY_ICON: std::sync::LazyLock<Option<Pixmap>> = std::sync::LazyLock::new(|| {
    use resvg::usvg::TreeParsing;
    let tree = resvg::usvg::Tree::from_data(
        include_bytes!("../../assets/icons/play.svg"),
        &resvg::usvg::Options::default(),
    )
    .ok()?;
    let mut icon = Pixmap::new(14, 14)?;
    resvg::Tree::from_usvg(&tree).render(
        Transform::from_scale(14.0 / 24.0, 14.0 / 24.0),
        &mut icon.as_mut(),
    );
    Some(icon)
});

impl ScreenRenderer {
    pub(super) fn draw_image_thumbnail(
        &self,
        pixmap: &mut Pixmap,
        width: u32,
        height: u32,
        thumbnail: Option<&little_clip_core::ImageThumbnail>,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let box_w = (rw - 32.0).max(40.0);
        let box_h = if is_hero { 220.0 } else { 180.0 };
        let target_top = ry + 126.0;

        if let Some(thumb) = thumbnail {
            if let Some(pixmap_ref) =
                PixmapRef::from_bytes(&thumb.premultiplied_rgba, thumb.width, thumb.height)
            {
                let scale = (box_w / thumb.width as f32)
                    .min(box_h / thumb.height as f32)
                    .min(1.0);
                let draw_w = thumb.width as f32 * scale;
                let draw_h = thumb.height as f32 * scale;

                let center_x = rx + 16.0 + (box_w - draw_w) / 2.0;
                let center_y = target_top + (box_h - draw_h) / 2.0;

                // Theme-aware checkerboard backdrop for transparent image thumbnails
                self.draw_checkerboard_pattern(
                    pixmap,
                    center_x - 2.0,
                    center_y - 2.0,
                    draw_w + 4.0,
                    draw_h + 4.0,
                    8.0,
                );

                let transform =
                    Transform::from_scale(scale, scale).post_translate(center_x, center_y);

                pixmap.draw_pixmap(0, 0, pixmap_ref, &PixmapPaint::default(), transform, None);
            }
        } else {
            // Sleek placeholder while generating preview async
            if let Some(card_r) = Rect::from_xywh(rx + 16.0, target_top, box_w as f32, box_h as f32)
            {
                let mut p = Paint::default();
                p.set_color(self.color_from_hex(&self.palette.colors.surface_recessed));
                pixmap.fill_rect(card_r, &p, Transform::identity(), None);

                let mut sp = Paint::default();
                sp.set_color(self.color_from_hex(&self.palette.harmonies.base));
                let mut stroke = Stroke::default();
                stroke.width = 1.0;
                let path = PathBuilder::from_rect(card_r);
                pixmap.stroke_path(&path, &sp, &stroke, Transform::identity(), None);
            }

            self.draw_text(
                pixmap,
                "PROCESSING...",
                rx + 24.0,
                target_top + 40.0,
                20.0,
                self.color_from_hex(&self.palette.harmonies.base),
                true,
            );
        }

        // Resolution subtitle
        if width > 0 && height > 0 {
            let clip_right = rx + rw - 10.0;
            self.draw_text_clipped(
                pixmap,
                &format!("{}×{}", width, height),
                rx + 14.0,
                target_top + box_h as f32 + 10.0,
                18.0,
                self.color_from_hex(&self.palette.colors.text_secondary),
                false,
                clip_right,
            );
        }
    }

    pub(super) fn draw_multi_image_grid(
        &self,
        pixmap: &mut Pixmap,
        total_count: usize,
        preview_indices: &[usize],
        thumbnails: &[little_clip_core::ImageThumbnail],
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
        border_color: Color,
    ) {
        let box_w = (rw - 28.0).max(40.0);
        let box_h = if is_hero { 240.0 } else { 190.0 };
        let content_x = rx + 14.0;
        let target_top = ry + 126.0;

        let display_count = if thumbnails.is_empty() {
            preview_indices.len().min(4)
        } else {
            thumbnails.len()
        };

        if display_count == 0 {
            if let Some(r) = Rect::from_xywh(content_x, target_top, box_w, box_h) {
                let mut p = Paint::default();
                p.set_color(self.color_from_hex(&self.palette.colors.surface_recessed));
                pixmap.fill_rect(r, &p, Transform::identity(), None);
            }
            self.draw_text_clipped(
                pixmap,
                "IMAGE BATCH",
                content_x + 10.0,
                target_top + box_h / 2.0 - 8.0,
                16.0,
                self.color_from_hex(&self.palette.colors.text_muted),
                true,
                content_x + box_w,
            );
            return;
        }

        let gap = 6.0;

        // Compute grid cell bounds depending on available thumbnails (2, 3, or 4)
        let cell_rects: Vec<(f32, f32, f32, f32)> = match display_count {
            1 => vec![(content_x, target_top, box_w, box_h)],
            2 => {
                let cell_w = (box_w - gap) / 2.0;
                vec![
                    (content_x, target_top, cell_w, box_h),
                    (content_x + cell_w + gap, target_top, cell_w, box_h),
                ]
            }
            3 => {
                let half_h = (box_h - gap) / 2.0;
                let half_w = (box_w - gap) / 2.0;
                vec![
                    (content_x, target_top, half_w, half_h),
                    (content_x + half_w + gap, target_top, half_w, half_h),
                    (
                        content_x + (box_w - half_w) / 2.0,
                        target_top + half_h + gap,
                        half_w,
                        half_h,
                    ),
                ]
            }
            _ => {
                // 4 cells in 2x2 grid
                let half_h = (box_h - gap) / 2.0;
                let half_w = (box_w - gap) / 2.0;
                vec![
                    (content_x, target_top, half_w, half_h),
                    (content_x + half_w + gap, target_top, half_w, half_h),
                    (content_x, target_top + half_h + gap, half_w, half_h),
                    (
                        content_x + half_w + gap,
                        target_top + half_h + gap,
                        half_w,
                        half_h,
                    ),
                ]
            }
        };

        for (i, &(cx, cy, cw, ch)) in cell_rects.iter().enumerate() {
            // Card cell background
            if let Some(cr) = Rect::from_xywh(cx, cy, cw, ch) {
                let mut bg_paint = Paint::default();
                bg_paint.set_color(self.color_from_hex(&self.palette.colors.surface_recessed));
                pixmap.fill_rect(cr, &bg_paint, Transform::identity(), None);

                let mut stroke = Stroke::default();
                stroke.width = 1.0;
                let mut sp = Paint::default();
                sp.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                let path = PathBuilder::from_rect(cr);
                pixmap.stroke_path(&path, &sp, &stroke, Transform::identity(), None);
            }

            // Draw scaled thumbnail centered in cell
            if let Some(thumb) = thumbnails.get(i) {
                if let Some(pixmap_ref) =
                    PixmapRef::from_bytes(&thumb.premultiplied_rgba, thumb.width, thumb.height)
                {
                    let scale = ((cw - 4.0) / thumb.width as f32)
                        .min((ch - 4.0) / thumb.height as f32)
                        .min(1.0);
                    let dw = thumb.width as f32 * scale;
                    let dh = thumb.height as f32 * scale;

                    let img_x = cx + (cw - dw) / 2.0;
                    let img_y = cy + (ch - dh) / 2.0;

                    let transform =
                        Transform::from_scale(scale, scale).post_translate(img_x, img_y);
                    pixmap.draw_pixmap(0, 0, pixmap_ref, &PixmapPaint::default(), transform, None);
                }
            }

            // Subtle index number badge in top-left of each cell (e.g. "#1", "#2", "#11", "#12")
            let idx_num = preview_indices.get(i).copied().unwrap_or(i) + 1;
            let idx_str = format!("#{}", idx_num);
            let badge_font_size = if is_hero { 12.0 } else { 10.0 };
            let badge_w = self.measure_text_width(&idx_str, badge_font_size, true) + 8.0;
            let badge_h = badge_font_size + 4.0;
            let badge_bg = self.color_from_hex(&self.palette.colors.surface_recessed);
            let badge_fg = self.contrast_color(
                badge_bg,
                Some(self.color_from_hex(&self.palette.colors.text_primary)),
            );
            if let Some(br) = Rect::from_xywh(cx + 4.0, cy + 4.0, badge_w, badge_h) {
                let mut bp = Paint::default();
                bp.set_color(badge_bg);
                pixmap.fill_rect(br, &bp, Transform::identity(), None);
            }
            self.draw_text_clipped(
                pixmap,
                &idx_str,
                cx + 8.0,
                cy + 5.0,
                badge_font_size,
                badge_fg,
                true,
                cx + cw,
            );
        }

        // If more than 4 items, draw a distinct center overlay badge showing how many in between
        if total_count > 4 {
            let omitted = total_count - 4;
            let badge_text = format!("+{} more", omitted);
            let badge_font_size = if is_hero { 14.0 } else { 12.0 };
            let badge_w = self.measure_text_width(&badge_text, badge_font_size, true) + 16.0;
            let badge_h = badge_font_size + 8.0;

            let bx = content_x + (box_w - badge_w) / 2.0;
            let by = target_top + (box_h - badge_h) / 2.0;

            let overlay_bg = self.color_from_hex(&self.palette.colors.surface_recessed);
            let overlay_fg = self.contrast_color(
                overlay_bg,
                Some(self.color_from_hex(&self.palette.colors.text_primary)),
            );
            if let Some(br) = Rect::from_xywh(bx, by, badge_w, badge_h) {
                let mut bp = Paint::default();
                bp.set_color(overlay_bg);
                pixmap.fill_rect(br, &bp, Transform::identity(), None);

                let mut stroke = Stroke::default();
                stroke.width = 1.5;
                let mut sp = Paint::default();
                sp.set_color(border_color);
                let path = PathBuilder::from_rect(br);
                pixmap.stroke_path(&path, &sp, &stroke, Transform::identity(), None);
            }

            self.draw_text_clipped(
                pixmap,
                &badge_text,
                bx + 8.0,
                by + 4.0,
                badge_font_size,
                overlay_fg,
                true,
                bx + badge_w,
            );
        }
    }

    pub(super) fn draw_animation_thumbnail(
        &self,
        pixmap: &mut Pixmap,
        width: u32,
        height: u32,
        duration_ms: u64,
        format: &str,
        thumbnail: Option<&little_clip_core::AnimationThumbnail>,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
        elision_color: Color,
    ) {
        let box_w = (rw - 28.0).max(40.0) as u32;
        let box_h = if is_hero { 220 } else { 180 };
        let target_top = ry + 126.0;
        let clip_right = rx + rw - 10.0;

        if let Some(thumb) = thumbnail {
            // Get current millisecond timestamp to dynamically pick current frame
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let frame_idx = thumb.get_frame_index_at_time(now_ms);
            if let Some(jpeg_bytes) = thumb.frames_jpeg.get(frame_idx) {
                if let Ok(dyn_img) =
                    image::load_from_memory_with_format(jpeg_bytes, image::ImageFormat::Jpeg)
                {
                    let rgba = dyn_img.to_rgba8();
                    let (fw, fh) = (rgba.width(), rgba.height());

                    let mut premultiplied = Vec::with_capacity(rgba.len());
                    for p in rgba.chunks_exact(4) {
                        let a = p[3] as u32;
                        let r = ((p[0] as u32 * a) / 255) as u8;
                        let g = ((p[1] as u32 * a) / 255) as u8;
                        let b = ((p[2] as u32 * a) / 255) as u8;
                        premultiplied.extend_from_slice(&[r, g, b, p[3]]);
                    }

                    if let Some(pixmap_ref) = PixmapRef::from_bytes(&premultiplied, fw, fh) {
                        let center_x = rx + 14.0 + ((rw - 28.0) - fw as f32) / 2.0;
                        let center_y = target_top + (box_h as f32 - fh as f32) / 2.0;

                        // Checkerboard backdrop for animated frame (supports transparent GIF/APNG)
                        self.draw_checkerboard_pattern(
                            pixmap,
                            center_x - 2.0,
                            center_y - 2.0,
                            fw as f32 + 4.0,
                            fh as f32 + 4.0,
                            6.0,
                        );

                        pixmap.draw_pixmap(
                            center_x as i32,
                            center_y as i32,
                            pixmap_ref,
                            &PixmapPaint::default(),
                            Transform::identity(),
                            None,
                        );

                        // Looping badge in top-left of box
                        let badge = format!("{} [{}/{}]", format, frame_idx + 1, thumb.frame_count);
                        let badge_bg = self.color_from_hex(&self.palette.colors.surface_recessed);
                        let badge_fg = self.contrast_color(
                            badge_bg,
                            Some(self.color_from_hex(&self.palette.harmonies.warm_attention)),
                        );
                        self.draw_pill_badge(
                            pixmap,
                            &badge,
                            rx + 36.0,
                            target_top + 18.0,
                            14.0,
                            badge_fg,
                            badge_bg,
                            clip_right,
                        );
                        if let Some(icon) = PLAY_ICON.as_ref() {
                            if let Some(background) =
                                Rect::from_xywh(rx + 18.0, target_top + 18.0, 18.0, 22.0)
                            {
                                let mut paint = Paint::default();
                                paint.set_color(badge_bg);
                                pixmap.fill_rect(background, &paint, Transform::identity(), None);
                            }
                            let mut tinted = icon.clone();
                            let tint = badge_fg.to_color_u8();
                            for pixel in tinted.pixels_mut() {
                                let alpha = pixel.alpha();
                                if let Some(color) = tiny_skia::PremultipliedColorU8::from_rgba(
                                    (u16::from(tint.red()) * u16::from(alpha) / 255) as u8,
                                    (u16::from(tint.green()) * u16::from(alpha) / 255) as u8,
                                    (u16::from(tint.blue()) * u16::from(alpha) / 255) as u8,
                                    alpha,
                                ) {
                                    *pixel = color;
                                }
                            }
                            pixmap.draw_pixmap(
                                (rx + 20.0) as i32,
                                (target_top + 22.0) as i32,
                                tinted.as_ref(),
                                &PixmapPaint::default(),
                                Transform::identity(),
                                None,
                            );
                        }
                    }
                }
            }
        } else {
            // Placeholder while extracting frames
            if let Some(card_r) = Rect::from_xywh(rx + 14.0, target_top, box_w as f32, box_h as f32)
            {
                let mut p = Paint::default();
                p.set_color(self.color_from_hex(&self.palette.colors.surface_recessed));
                pixmap.fill_rect(card_r, &p, Transform::identity(), None);

                let mut sp = Paint::default();
                sp.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                let mut stroke = Stroke::default();
                stroke.width = 1.0;
                let path = PathBuilder::from_rect(card_r);
                pixmap.stroke_path(&path, &sp, &stroke, Transform::identity(), None);
            }

            self.draw_text_clipped(
                pixmap,
                &format!("DECODING {}...", format),
                rx + 20.0,
                target_top + 40.0,
                18.0,
                self.color_from_hex(&self.palette.harmonies.base),
                true,
                clip_right,
            );
        }

        // Resolution & duration subtitle
        let dur_str = if duration_ms > 0 {
            let mins = duration_ms / 60_000;
            let secs = (duration_ms % 60_000) / 1000;
            format!("{:02}:{:02} • ", mins, secs)
        } else {
            String::new()
        };

        let sub = if width > 0 && height > 0 {
            format!("{}{}×{} ({})", dur_str, width, height, format)
        } else {
            format!("{}{}", dur_str, format)
        };

        let elided_sub = self.truncate_filename_elided(&sub, (rw - 28.0).max(10.0), 16.0, false);
        self.draw_elided_text_clipped(
            pixmap,
            &elided_sub,
            rx + 14.0,
            target_top + box_h as f32 + 10.0,
            16.0,
            self.color_from_hex(&self.palette.colors.text_secondary),
            elision_color,
            false,
            clip_right,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_play_svg_rasterizes_for_display_badges() {
        let icon = PLAY_ICON.as_ref().expect("shared play SVG must decode");
        assert_eq!((icon.width(), icon.height()), (14, 14));
        assert!(icon.pixels().iter().any(|pixel| pixel.alpha() > 0));
    }
}
