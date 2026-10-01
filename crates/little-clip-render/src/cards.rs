//! Card renderers for slot cards, media, spreadsheets, 3D models, and formatted text.

use crate::is_zero_width_or_bom;
use crate::{FontStyle, ScreenRenderer};
use little_clip_core::color::parse_hex_color_rgba_or_white as parse_hex_color;
use little_clip_core::{ClipItem, ClipPayload, SlotId, SlotManager};
use little_clip_core::{midi, model3d, network_preview, spreadsheet, timestamps, units};
use std::path::Path;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use tiny_skia::*;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

impl ScreenRenderer {
    /// Draws a single slot card on the canvas with rounded header, status badges, and content previews.
    pub(crate) fn draw_slot_card(
        &self,
        pixmap: &mut Pixmap,
        manager: &SlotManager,
        slot_id: SlotId,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        is_hero: bool,
    ) {
        let is_cursor = manager.cursor == slot_id;
        let is_selected = manager.selected.contains(&slot_id);

        let margin = 4.0;
        let rect_x = x as f32 + margin;
        let rect_y = y as f32 + margin;
        let rect_w = w as f32 - (margin * 2.0);
        let rect_h = h as f32 - (margin * 2.0);

        let border_hex = if is_cursor {
            &self.palette.cursor_border
        } else if is_selected {
            &self.palette.selected_border
        } else {
            self.slot_border_hex(slot_id)
        };
        let (br_r, br_g, br_b, br_a) = parse_hex_color(border_hex);
        let border_color = Color::from_rgba8(br_r, br_g, br_b, br_a);

        // Fill background
        if let Some(cr) = Rect::from_xywh(rect_x, rect_y, rect_w, rect_h) {
            let bg_hex = match slot_id {
                SlotId::Undo => &self.palette.colors.surface_recessed,
                SlotId::LeftHand | SlotId::RightHand => &self.palette.colors.surface_raised,
                SlotId::LeftPrior1
                | SlotId::LeftPrior2
                | SlotId::LeftPrior3
                | SlotId::RightPrior2
                | SlotId::RightPrior1 => &self.palette.colors.surface_panel,
            };
            let (bg_r, bg_g, bg_b, bg_a) = parse_hex_color(bg_hex);
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(bg_r, bg_g, bg_b, bg_a));
            pixmap.fill_rect(cr, &paint, Transform::identity(), None);

            // Card Border
            let stroke_width = if is_hero || is_cursor || is_selected {
                3.0
            } else {
                1.5
            };
            let mut stroke = Stroke::default();
            stroke.width = stroke_width;
            let mut stroke_paint = Paint::default();
            stroke_paint.set_color(border_color);
            let path = PathBuilder::from_rect(cr);
            pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
        }

        // Header Pill / Slot Label (Font 30px for hero, 24px for prior)
        let header_text = slot_id.label();
        let header_hex = self.slot_border_hex(slot_id);
        let (hdr_r, hdr_g, hdr_b, hdr_a) = parse_hex_color(header_hex);
        let header_color = Color::from_rgba8(hdr_r, hdr_g, hdr_b, hdr_a);

        let font_size = if is_hero { 26.0 } else { 21.0 };
        let card_clip_right = rect_x + rect_w - 6.0;
        self.draw_text_clipped(
            pixmap,
            &header_text,
            rect_x + 14.0,
            rect_y + 14.0,
            font_size,
            header_color,
            true,
            card_clip_right,
        );

        // Content
        if let Some(item) = manager.get(slot_id) {
            self.draw_item_content(
                pixmap,
                item,
                rect_x,
                rect_y,
                rect_w,
                rect_h,
                is_hero,
                border_color,
            );
        } else {
            // Empty placeholder
            self.draw_text_clipped(
                pixmap,
                "EMPTY",
                rect_x + 14.0,
                rect_y + 54.0,
                if is_hero { 26.0 } else { 21.0 },
                self.color_from_hex(&self.palette.colors.text_muted),
                false,
                card_clip_right,
            );
        }
    }

    fn draw_item_content(
        &self,
        pixmap: &mut Pixmap,
        item: &ClipItem,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
        border_color: Color,
    ) {
        let (t_r, t_g, t_b, t_a) = parse_hex_color(&self.palette.colors.text_primary);
        let (s_r, s_g, s_b, s_a) = parse_hex_color(&self.palette.colors.text_secondary);
        let (u_r, u_g, u_b, u_a) = parse_hex_color(&self.palette.url_color);

        let content_x = rx + 14.0;
        let max_content_w = (rw - 28.0).max(10.0);
        let clip_right = rx + rw - 10.0;

        // Resolve elision character color:
        // Default: "border" -> dynamically matches each slot box's border color
        let elision_color = if self
            .config
            .display
            .elision_color
            .eq_ignore_ascii_case("border")
        {
            border_color
        } else {
            let (er, eg, eb, ea) = parse_hex_color(&self.config.display.elision_color);
            Color::from_rgba8(er, eg, eb, ea)
        };

        // Title: 28px for hero (down from 34px), 21px for history (smaller-but-not-too-small)
        let title_font_size = if is_hero { 28.0 } else { 21.0 };
        let title_color = Color::from_rgba8(t_r, t_g, t_b, t_a);
        let elided_title =
            self.truncate_filename_elided(&item.title, max_content_w, title_font_size, true);
        self.draw_elided_text_clipped(
            pixmap,
            &elided_title,
            content_x,
            ry + 52.0,
            title_font_size,
            title_color,
            elision_color,
            true,
            clip_right,
        );

        // Subtitle: 19px for hero, 16px for history
        let sub_font_size = if is_hero { 19.0 } else { 16.0 };
        let subtitle_color = Color::from_rgba8(s_r, s_g, s_b, s_a);
        let elided_subtitle =
            self.truncate_filename_elided(&item.subtitle, max_content_w, sub_font_size, false);
        self.draw_elided_text_clipped(
            pixmap,
            &elided_subtitle,
            content_x,
            ry + 88.0,
            sub_font_size,
            subtitle_color,
            elision_color,
            false,
            clip_right,
        );

        // Payload-specific preview starting around y = 124.0
        match &item.payload {
            ClipPayload::Text(text) => {
                let ext_hint = Path::new(&item.title)
                    .extension()
                    .and_then(|s| s.to_str())
                    .or_else(|| {
                        let trimmed = text.trim();
                        let p = Path::new(trimmed);
                        if p.is_file() {
                            p.extension().and_then(|s| s.to_str())
                        } else {
                            None
                        }
                    });

                let (preview_text, effective_hint) = {
                    let trimmed = text.trim();
                    let p = Path::new(trimmed);
                    if p.is_file() && little_clip_core::syntax::is_code_file(p) {
                        if let Ok(file_content) = std::fs::read_to_string(p) {
                            let clean_content = file_content
                                .trim_start_matches(is_zero_width_or_bom)
                                .to_string();
                            (clean_content, p.extension().and_then(|s| s.to_str()))
                        } else {
                            (text.clone(), ext_hint)
                        }
                    } else {
                        (text.clone(), ext_hint)
                    }
                };

                self.draw_text_preview(
                    pixmap,
                    &preview_text,
                    effective_hint,
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                    elision_color,
                );
            }
            ClipPayload::Markdown(content) => {
                self.draw_markdown_preview(
                    pixmap,
                    content,
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                    border_color,
                    elision_color,
                );
            }
            ClipPayload::Color { hex, r, g, b } => {
                // Large color swatch
                let swatch_w = rw - 32.0;
                let swatch_h = 160.0;
                if let Some(sr) = Rect::from_xywh(rx + 16.0, ry + 130.0, swatch_w, swatch_h) {
                    let mut paint = Paint::default();
                    paint.set_color(Color::from_rgba8(*r, *g, *b, 255));
                    pixmap.fill_rect(sr, &paint, Transform::identity(), None);
                }
                // Hex label (Font 42px bold)
                let label_color = self.color_from_hex(&self.palette.colors.text_primary);
                self.draw_text(pixmap, hex, rx + 16.0, ry + 320.0, 42.0, label_color, true);
            }
            ClipPayload::Url(url) => {
                let url_str = url.as_str();
                let line1: String = url_str
                    .chars()
                    .take(if is_hero { 26 } else { 12 })
                    .collect();
                let line2: String = url_str
                    .chars()
                    .skip(if is_hero { 26 } else { 12 })
                    .take(if is_hero { 26 } else { 12 })
                    .collect();

                self.draw_text(
                    pixmap,
                    &line1,
                    rx + 16.0,
                    ry + 130.0,
                    22.0,
                    Color::from_rgba8(u_r, u_g, u_b, u_a),
                    false,
                );
                if !line2.is_empty() {
                    self.draw_text(
                        pixmap,
                        &line2,
                        rx + 16.0,
                        ry + 160.0,
                        22.0,
                        Color::from_rgba8(u_r, u_g, u_b, u_a),
                        false,
                    );
                }
            }
            ClipPayload::Image {
                width,
                height,
                thumbnail,
                ..
            } => {
                self.draw_image_thumbnail(
                    pixmap,
                    *width,
                    *height,
                    thumbnail.as_ref(),
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                );
            }
            ClipPayload::MultiImage {
                total_count,
                preview_indices,
                thumbnails,
                ..
            } => {
                self.draw_multi_image_grid(
                    pixmap,
                    *total_count,
                    preview_indices,
                    thumbnails,
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                    border_color,
                );
            }
            ClipPayload::Animation {
                width,
                height,
                duration_ms,
                format,
                thumbnail,
                ..
            } => {
                self.draw_animation_thumbnail(
                    pixmap,
                    *width,
                    *height,
                    *duration_ms,
                    format,
                    thumbnail.as_ref(),
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                    elision_color,
                );
            }
            ClipPayload::AudioRef {
                duration_ms,
                sample_rate,
                channels,
                format,
                waveform,
                ..
            } => {
                self.draw_audio_waveform(
                    pixmap,
                    *duration_ms,
                    *sample_rate,
                    *channels,
                    format,
                    waveform.as_ref(),
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                );
            }
            ClipPayload::FileBatch(files) => {
                if files.len() == 1 {
                    let p = &files[0];
                    if p.is_file() && spreadsheet::is_spreadsheet_file(p) {
                        if let Ok(sheet_data) = spreadsheet::load_spreadsheet(p) {
                            if let Some(first_sheet) = sheet_data.sheets.first() {
                                self.draw_spreadsheet_card(
                                    pixmap,
                                    &sheet_data.title,
                                    first_sheet,
                                    rx,
                                    ry,
                                    rw,
                                    rh,
                                    is_hero,
                                );
                                return;
                            }
                        }
                    }
                    if p.is_file()
                        && (p
                            .extension()
                            .and_then(|s| s.to_str())
                            .map(|e| {
                                e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi")
                            })
                            .unwrap_or(false))
                    {
                        if let Ok(midi_data) = midi::MidiData::parse_file(p) {
                            self.draw_midi_card(pixmap, &midi_data, rx, ry, rw, rh, is_hero);
                            return;
                        }
                    }
                    if p.is_file() && model3d::is_3d_model_file(p) {
                        if let Ok(model) = model3d::Model3d::load_cached(p) {
                            self.draw_model3d_card(pixmap, &model, rx, ry, rw, rh, is_hero);
                            return;
                        }
                    }
                    if p.is_file() && little_clip_core::syntax::is_code_file(p) {
                        if let Ok(file_content) = std::fs::read_to_string(p) {
                            let clean_content = file_content
                                .trim_start_matches(is_zero_width_or_bom)
                                .to_string();
                            let ext = p.extension().and_then(|s| s.to_str());
                            self.draw_text_preview(
                                pixmap,
                                &clean_content,
                                ext,
                                rx,
                                ry,
                                rw,
                                rh,
                                is_hero,
                                elision_color,
                            );
                            return;
                        }
                    }
                }
                self.draw_text(
                    pixmap,
                    &format!("FILES: {}", files.len()),
                    rx + 16.0,
                    ry + 130.0,
                    26.0,
                    self.color_from_hex(&self.palette.harmonies.base),
                    true,
                );
            }
            ClipPayload::Directory(_) => {
                self.draw_text(
                    pixmap,
                    "DIRECTORY",
                    rx + 16.0,
                    ry + 130.0,
                    26.0,
                    self.color_from_hex(&self.palette.harmonies.base),
                    true,
                );
            }
        }
    }

    fn draw_image_thumbnail(
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

    fn draw_multi_image_grid(
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

    fn draw_animation_thumbnail(
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
                        let badge =
                            format!("▶ {} [{}/{}]", format, frame_idx + 1, thumb.frame_count);
                        let badge_bg = self.color_from_hex(&self.palette.colors.surface_recessed);
                        let badge_fg = self.contrast_color(
                            badge_bg,
                            Some(self.color_from_hex(&self.palette.harmonies.warm_attention)),
                        );
                        self.draw_pill_badge(
                            pixmap,
                            &badge,
                            rx + 18.0,
                            target_top + 18.0,
                            14.0,
                            badge_fg,
                            badge_bg,
                            clip_right,
                        );
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

    fn draw_audio_waveform(
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

    /// TrueType text width measurement with styling.

    fn draw_markdown_spans(
        &self,
        pixmap: &mut Pixmap,
        spans: &[crate::markdown::MarkdownSpan],
        mut cx: f32,
        y: f32,
        size: f32,
        default_color: Color,
        clip_right: f32,
    ) -> f32 {
        use crate::markdown::MarkdownSpan;
        let url_color = self.color_from_hex(&self.palette.url_color);
        let title_color = self.color_from_hex(&self.palette.colors.text_primary);
        let italic_color = self.color_from_hex_alpha(&self.palette.colors.text_muted, 240);
        let code_color = self.color_from_hex(&self.palette.undo_border);

        for span in spans {
            if cx >= clip_right {
                break;
            }
            match span {
                MarkdownSpan::Text(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        default_color,
                        FontStyle::Regular,
                        clip_right,
                    );
                }
                MarkdownSpan::Bold(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        title_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                }
                MarkdownSpan::Italic(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        italic_color,
                        FontStyle::Italic,
                        clip_right,
                    );
                }
                MarkdownSpan::BoldItalic(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        title_color,
                        FontStyle::BoldItalic,
                        clip_right,
                    );
                }
                MarkdownSpan::Code(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        code_color,
                        FontStyle::Mono,
                        clip_right,
                    );
                }
                MarkdownSpan::Link { label, .. } => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        label,
                        cx,
                        y,
                        size,
                        url_color,
                        FontStyle::Regular,
                        clip_right,
                    );
                }
            }
        }
        cx
    }

    fn draw_markdown_preview(
        &self,
        pixmap: &mut Pixmap,
        content: &str,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
        border_color: Color,
        elision_color: Color,
    ) {
        use crate::markdown::MarkdownLine;
        let body_color = self.color_from_hex(&self.palette.colors.text_muted);
        let content_x = rx + 14.0;
        let max_content_w = (rw - 28.0).max(10.0);
        let clip_right = rx + rw - 10.0;

        let clean_content = content.trim_start_matches(is_zero_width_or_bom);
        let lines = crate::markdown::parse_markdown(clean_content);
        let total_lines = lines.len();
        let max_y = ry + rh - 22.0;
        let mut y = ry + 124.0;

        for (idx, line) in lines.iter().enumerate() {
            let is_near_bottom = y + 26.0 > max_y;
            let has_more = idx + 1 < total_lines;

            if is_near_bottom && has_more {
                let remaining = total_lines - idx;
                let badge = format!("▼ +{} more lines", remaining);
                self.draw_footer_badge(pixmap, &badge, content_x, y, 16.0, clip_right);
                break;
            }

            if y > max_y {
                break;
            }

            match line {
                MarkdownLine::Heading {
                    level,
                    text: _,
                    spans,
                } => {
                    let (head_size, head_color_pref, lead_symbol) = match level {
                        1 => (
                            if is_hero { 21.0 } else { 18.0 },
                            self.color_from_hex(&self.palette.harmonies.base),
                            "# ",
                        ),
                        2 => (
                            if is_hero { 19.0 } else { 16.5 },
                            self.color_from_hex(&self.palette.harmonies.triadic_1),
                            "## ",
                        ),
                        _ => (
                            if is_hero { 17.5 } else { 15.0 },
                            self.color_from_hex(&self.palette.harmonies.triadic_2),
                            "### ",
                        ),
                    };
                    let head_color = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(head_color_pref),
                    );

                    let cx = self.draw_text_styled_clipped(
                        pixmap,
                        lead_symbol,
                        content_x,
                        y,
                        head_size,
                        elision_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    self.draw_markdown_spans(
                        pixmap, spans, cx, y, head_size, head_color, clip_right,
                    );
                    y += if is_hero { 26.0 } else { 22.0 };
                }
                MarkdownLine::Bullet { indent, spans } => {
                    let indent_px = (*indent as f32 * 4.0).min(16.0);
                    let bullet_x = content_x + indent_px;
                    let cx = self.draw_text_styled_clipped(
                        pixmap,
                        "• ",
                        bullet_x,
                        y,
                        if is_hero { 18.0 } else { 15.0 },
                        border_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    self.draw_markdown_spans(
                        pixmap, spans, cx, y, font_size, body_color, clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Numbered { number, spans } => {
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    let cx = self.draw_text_styled_clipped(
                        pixmap,
                        &format!("{} ", number),
                        content_x,
                        y,
                        font_size,
                        border_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    self.draw_markdown_spans(
                        pixmap, spans, cx, y, font_size, body_color, clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Quote { spans } => {
                    if let Some(qr) =
                        Rect::from_xywh(content_x, y, 2.5, if is_hero { 18.0 } else { 15.0 })
                    {
                        let mut qp = Paint::default();
                        qp.set_color(self.color_from_hex(&self.palette.harmonies.base));
                        pixmap.fill_rect(qr, &qp, Transform::identity(), None);
                    }
                    let font_size = if is_hero { 17.5 } else { 14.5 };
                    let quote_fg = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(self.color_from_hex(&self.palette.colors.text_secondary)),
                    );
                    self.draw_markdown_spans(
                        pixmap,
                        spans,
                        content_x + 10.0,
                        y,
                        font_size,
                        quote_fg,
                        clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Rule => {
                    if let Some(rr) = Rect::from_xywh(content_x, y + 6.0, max_content_w, 1.0) {
                        let mut rp = Paint::default();
                        rp.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                        pixmap.fill_rect(rr, &rp, Transform::identity(), None);
                    }
                    y += 14.0;
                }
                MarkdownLine::CodeFence { language } => {
                    let label = if language.is_empty() {
                        "```".to_string()
                    } else {
                        format!("``` {}", language)
                    };
                    let fence_fg = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(self.color_from_hex(&self.palette.colors.text_muted)),
                    );
                    self.draw_text_styled_clipped(
                        pixmap,
                        &label,
                        content_x,
                        y,
                        14.0,
                        fence_fg,
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += 18.0;
                }
                MarkdownLine::CodeLine { line } => {
                    let font_size = if is_hero { 16.0 } else { 13.5 };
                    let code_bg = self.color_from_hex(&self.palette.colors.surface_recessed);
                    let code_fg = self.contrast_color(
                        code_bg,
                        Some(self.color_from_hex(&self.palette.harmonies.warm_attention)),
                    );
                    if let Some(bg) =
                        Rect::from_xywh(content_x, y - 2.0, max_content_w, font_size + 4.0)
                    {
                        let mut p = Paint::default();
                        p.set_color(code_bg);
                        pixmap.fill_rect(bg, &p, Transform::identity(), None);
                    }
                    self.draw_text_styled_clipped(
                        pixmap,
                        line,
                        content_x + 6.0,
                        y,
                        font_size,
                        code_fg,
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += if is_hero { 22.0 } else { 19.0 };
                }
                MarkdownLine::Paragraph { spans } => {
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    self.draw_markdown_spans(
                        pixmap, spans, content_x, y, font_size, body_color, clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Empty => {
                    y += if is_hero { 10.0 } else { 8.0 };
                }
            }
        }
    }

    fn draw_unit_conversion_card(
        &self,
        pixmap: &mut Pixmap,
        conv: &units::UnitConversion,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let bg_col = self.color_from_hex(&self.palette.colors.canvas);
        let f_high = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let f_low = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_muted)),
        );
        let base_c = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.harmonies.base)),
        );
        let diff_add =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_add)));
        let diff_mod =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_mod)));

        self.draw_pill_badge(
            pixmap,
            "UNIT CONVERSION",
            content_x,
            y,
            14.0,
            diff_add,
            Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
            clip_right,
        );
        y += 28.0;

        self.draw_text_clipped(
            pixmap,
            &format!("Input: {}", conv.original),
            content_x,
            y,
            if is_hero { 18.0 } else { 15.0 },
            f_low,
            false,
            clip_right,
        );
        y += 24.0;

        self.draw_text_clipped(
            pixmap,
            &conv.primary_conversion,
            content_x,
            y,
            if is_hero { 24.0 } else { 20.0 },
            f_high,
            true,
            clip_right,
        );
        y += if is_hero { 32.0 } else { 28.0 };

        if let Some(ref sec) = conv.secondary_conversion {
            self.draw_text_clipped(
                pixmap,
                &format!("≈ {}", sec),
                content_x,
                y,
                if is_hero { 18.0 } else { 15.0 },
                base_c,
                false,
                clip_right,
            );
            y += 26.0;
        }

        if let Some(ref rot) = conv.rule_of_thumb {
            self.draw_text_clipped(
                pixmap,
                &format!("* {}", rot),
                content_x,
                y,
                if is_hero { 16.0 } else { 13.0 },
                diff_mod,
                false,
                clip_right,
            );
        }
    }

    fn draw_network_preview_card(
        &self,
        pixmap: &mut Pixmap,
        net: &network_preview::NetworkPreview,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let bg_col = self.color_from_hex(&self.palette.colors.canvas);
        let f_high = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let f_low = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_muted)),
        );
        let base_c = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.harmonies.base)),
        );
        let triadic_1 = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.harmonies.triadic_1)),
        );
        let diff_add =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_add)));
        let diff_del =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_del)));

        match net {
            network_preview::NetworkPreview::Ip {
                ip,
                rfc_class,
                description,
                ping_status,
            } => {
                self.draw_pill_badge(
                    pixmap,
                    "IP ADDRESS",
                    content_x,
                    y,
                    14.0,
                    base_c,
                    Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
                    clip_right,
                );
                y += 28.0;

                self.draw_text_clipped(
                    pixmap,
                    &ip.to_string(),
                    content_x,
                    y,
                    if is_hero { 24.0 } else { 20.0 },
                    f_high,
                    true,
                    clip_right,
                );
                y += if is_hero { 32.0 } else { 28.0 };

                self.draw_text_clipped(
                    pixmap,
                    rfc_class,
                    content_x,
                    y,
                    if is_hero { 17.0 } else { 14.0 },
                    triadic_1,
                    true,
                    clip_right,
                );
                y += 24.0;

                if let Some(desc) = description {
                    self.draw_text_clipped(
                        pixmap,
                        desc,
                        content_x,
                        y,
                        if is_hero { 15.0 } else { 13.0 },
                        f_low,
                        false,
                        clip_right,
                    );
                    y += 24.0;
                }

                if let Some(ping) = ping_status {
                    let is_online = ping.starts_with("Online");
                    let ping_col = if is_online { diff_add } else { diff_del };
                    self.draw_text_clipped(
                        pixmap,
                        &format!("Ping: {}", ping),
                        content_x,
                        y,
                        if is_hero { 16.0 } else { 14.0 },
                        ping_col,
                        true,
                        clip_right,
                    );
                }
            }
            network_preview::NetworkPreview::Mac {
                formatted,
                vendor,
                mac_type,
            } => {
                self.draw_pill_badge(
                    pixmap,
                    "MAC ADDRESS",
                    content_x,
                    y,
                    14.0,
                    triadic_1,
                    Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
                    clip_right,
                );
                y += 28.0;

                self.draw_text_clipped(
                    pixmap,
                    formatted,
                    content_x,
                    y,
                    if is_hero { 22.0 } else { 18.0 },
                    f_high,
                    true,
                    clip_right,
                );
                y += if is_hero { 32.0 } else { 28.0 };

                self.draw_text_clipped(
                    pixmap,
                    &format!("Vendor: {}", vendor),
                    content_x,
                    y,
                    if is_hero { 17.0 } else { 14.0 },
                    base_c,
                    true,
                    clip_right,
                );
                y += 24.0;

                self.draw_text_clipped(
                    pixmap,
                    mac_type,
                    content_x,
                    y,
                    if is_hero { 15.0 } else { 13.0 },
                    f_low,
                    false,
                    clip_right,
                );
            }
        }
    }

    fn draw_timestamp_card(
        &self,
        pixmap: &mut Pixmap,
        ts: &timestamps::TimestampPreview,
        rx: f32,
        ry: f32,
        rw: f32,
        _rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let bg_col = self.color_from_hex(&self.palette.colors.canvas);
        let f_high = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let f_med = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_secondary)),
        );
        let f_low = self.contrast_color(
            bg_col,
            Some(self.color_from_hex(&self.palette.colors.text_muted)),
        );
        let diff_add =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_add)));
        let diff_mod =
            self.contrast_color(bg_col, Some(self.color_from_hex(&self.palette.diff_mod)));

        self.draw_pill_badge(
            pixmap,
            "TIMESTAMP",
            content_x,
            y,
            14.0,
            diff_mod,
            Self::hex_to_color_alpha(&self.palette.colors.surface_recessed, 230),
            clip_right,
        );
        y += 28.0;

        self.draw_text_clipped(
            pixmap,
            &ts.human_readable,
            content_x,
            y,
            if is_hero { 20.0 } else { 16.0 },
            f_high,
            true,
            clip_right,
        );
        y += if is_hero { 28.0 } else { 24.0 };

        self.draw_text_clipped(
            pixmap,
            &format!("Relative: {}", ts.relative),
            content_x,
            y,
            if is_hero { 17.0 } else { 14.0 },
            diff_add,
            true,
            clip_right,
        );
        y += 24.0;

        self.draw_text_clipped(
            pixmap,
            &format!("UTC: {}", ts.utc_iso),
            content_x,
            y,
            if is_hero { 15.0 } else { 13.0 },
            f_med,
            false,
            clip_right,
        );
        y += 24.0;

        self.draw_text_clipped(
            pixmap,
            &format!("Epoch: {} ({})", ts.epoch_sec, ts.day_info),
            content_x,
            y,
            if is_hero { 15.0 } else { 13.0 },
            f_low,
            false,
            clip_right,
        );
    }

    fn draw_spreadsheet_card(
        &self,
        pixmap: &mut Pixmap,
        _title: &str,
        sheet: &spreadsheet::SpreadsheetSheet,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let badge_bg = self.color_from_hex(&self.palette.diff_add);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        self.draw_pill_badge(
            pixmap,
            "SPREADSHEET",
            content_x,
            y,
            14.0,
            badge_fg,
            badge_bg,
            clip_right,
        );
        y += 28.0;

        let sheet_info = format!(
            "{} ({}r × {}c)",
            sheet.name, sheet.total_rows, sheet.total_cols
        );
        self.draw_text_clipped(
            pixmap,
            &sheet_info,
            content_x,
            y,
            if is_hero { 16.0 } else { 13.0 },
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            clip_right,
        );
        y += 22.0;

        let max_rows = if is_hero { 9 } else { 7 };
        let max_cols = if is_hero { 4 } else { 3 };
        let slice = sheet.upper_left_slice(max_rows, max_cols);
        if slice.is_empty() {
            return;
        }

        let num_cols = slice[0].len().max(1);
        let avail_w = clip_right - content_x;
        let col_w = avail_w / num_cols as f32;
        let row_h = if is_hero { 26.0 } else { 22.0 };
        let font_size = if is_hero { 14.0 } else { 12.0 };

        for (r_idx, row) in slice.iter().enumerate() {
            let row_y = y + (r_idx as f32 * row_h);
            if row_y + row_h > ry + rh - 30.0 {
                break;
            }

            let is_header = r_idx == 0;
            let row_bg = if is_header {
                self.color_from_hex(&self.palette.colors.surface_panel)
            } else if r_idx % 2 == 1 {
                self.color_from_hex(&self.palette.colors.surface_recessed)
            } else {
                self.color_from_hex(&self.palette.colors.canvas)
            };

            if let Some(rect) = Rect::from_xywh(content_x, row_y, avail_w, row_h) {
                let mut paint = Paint::default();
                paint.set_color(row_bg);
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);

                let mut stroke = Stroke::default();
                stroke.width = 0.5;
                let mut stroke_paint = Paint::default();
                stroke_paint.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                let path = PathBuilder::from_rect(rect);
                pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
            }

            for (c_idx, cell_val) in row.iter().enumerate() {
                let cell_x = content_x + (c_idx as f32 * col_w) + 4.0;
                let cell_clip = content_x + ((c_idx + 1) as f32 * col_w) - 2.0;
                let cell_color = if is_header {
                    self.contrast_color(
                        row_bg,
                        Some(self.color_from_hex(&self.palette.harmonies.base)),
                    )
                } else {
                    self.contrast_color(
                        row_bg,
                        Some(self.color_from_hex(&self.palette.colors.text_primary)),
                    )
                };

                let cell_y = row_y + (row_h - font_size) / 2.0 - 1.0;
                self.draw_text_clipped(
                    pixmap, cell_val, cell_x, cell_y, font_size, cell_color, is_header, cell_clip,
                );
            }
        }

        let footer_y = ry + rh - 22.0;
        self.draw_text_clipped(
            pixmap,
            "> Open in Workbench for full table",
            content_x,
            footer_y,
            12.0,
            self.color_from_hex(&self.palette.colors.text_muted),
            false,
            clip_right,
        );
    }

    fn draw_midi_card(
        &self,
        pixmap: &mut Pixmap,
        midi: &midi::MidiData,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let badge_bg = self.color_from_hex(&self.palette.harmonies.base);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        self.draw_pill_badge(
            pixmap,
            "MIDI PIANO ROLL",
            content_x,
            y,
            14.0,
            badge_fg,
            badge_bg,
            clip_right,
        );
        y += 28.0;

        let info = format!(
            "{} • {} notes • {:.1}s • {:.0} BPM",
            midi.title, midi.total_notes, midi.duration_sec, midi.bpm
        );
        self.draw_text_clipped(
            pixmap,
            &info,
            content_x,
            y,
            if is_hero { 16.0 } else { 13.0 },
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            clip_right,
        );
        y += 22.0;

        let roll_x = content_x;
        let roll_y = y;
        let roll_w = clip_right - content_x;
        let roll_h = (ry + rh - 30.0 - roll_y).max(40.0);

        if let Some(_rect) = self.draw_card_viewport_frame(pixmap, roll_x, roll_y, roll_w, roll_h) {
            let kb_w = 16.0f32;
            if let Some(kb_rect) = Rect::from_xywh(roll_x, roll_y, kb_w, roll_h) {
                let mut kb_paint = Paint::default();
                kb_paint.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                pixmap.fill_rect(kb_rect, &kb_paint, Transform::identity(), None);
            }

            let min_k = midi.min_key.min(midi.max_key) as f32;
            let max_k = midi.max_key.max(midi.min_key + 1) as f32;
            let key_span = (max_k - min_k + 1.0).max(1.0);
            let dur_span = midi.duration_sec.max(0.1);

            let grid_x = roll_x + kb_w + 2.0;
            let grid_w = roll_w - kb_w - 4.0;

            for note in &midi.notes {
                let norm_start = (note.start_sec / dur_span).clamp(0.0, 1.0);
                let norm_dur = (note.duration_sec / dur_span).clamp(0.005, 1.0);
                let norm_pitch = ((note.key as f32 - min_k) / key_span).clamp(0.0, 1.0);

                let nx = grid_x + norm_start * grid_w;
                let nw = (norm_dur * grid_w).max(3.0).min(grid_x + grid_w - nx);
                let note_h = (roll_h / key_span).clamp(3.0, 12.0);
                let ny = roll_y + (1.0 - norm_pitch) * (roll_h - note_h);

                if let Some(n_rect) = Rect::from_xywh(nx, ny, nw, note_h) {
                    let mut n_paint = Paint::default();
                    let (r, g, b) = self.palette.midi_note_color(
                        note.track,
                        note.channel,
                        midi.track_count,
                        note.key,
                        note.velocity,
                    );
                    n_paint.set_color(Color::from_rgba8(r, g, b, 240));
                    pixmap.fill_rect(n_rect, &n_paint, Transform::identity(), None);
                }
            }
        }

        let footer_y = ry + rh - 22.0;
        self.draw_text_clipped(
            pixmap,
            "> Space in Workbench to play audio & visualize",
            content_x,
            footer_y,
            12.0,
            self.color_from_hex(&self.palette.colors.text_muted),
            false,
            clip_right,
        );
    }

    fn draw_model3d_card(
        &self,
        pixmap: &mut Pixmap,
        mesh: &model3d::Model3d,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
    ) {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let mut y = ry + 124.0;

        let badge_bg = self.color_from_hex(&self.palette.diff_mod);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let badge_label = format!("3D {} MODEL", mesh.format.badge_label());
        self.draw_pill_badge(
            pixmap,
            &badge_label,
            content_x,
            y,
            14.0,
            badge_fg,
            badge_bg,
            clip_right,
        );
        y += 28.0;

        let info = format!(
            "{} • {} facets • {:.1}×{:.1}×{:.1}mm",
            mesh.name,
            mesh.triangles.len(),
            mesh.size.x,
            mesh.size.y,
            mesh.size.z
        );
        self.draw_text_clipped(
            pixmap,
            &info,
            content_x,
            y,
            if is_hero { 16.0 } else { 13.0 },
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            clip_right,
        );
        y += 22.0;

        let vp_x = content_x;
        let vp_y = y;
        let vp_w = clip_right - content_x;
        let vp_h = (ry + rh - 30.0 - vp_y).max(40.0);

        if let Some(_rect) = self.draw_card_viewport_frame(pixmap, vp_x, vp_y, vp_w, vp_h) {
            let now_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            // Continuous, perfectly smooth 360-degree rotation every 12 seconds with sub-millisecond precision
            let angle_y = ((now_millis % 12_000) as f32 / 12_000.0) * std::f32::consts::TAU;
            let tilt_x = 0.35f32;

            let sin_y = angle_y.sin();
            let cos_y = angle_y.cos();
            let sin_x = tilt_x.sin();
            let cos_x = tilt_x.cos();

            let is_z_up = mesh.format == model3d::ModelFormat::Stl;
            let center = mesh.center;
            let max_dim = mesh.max_dimension.max(0.001);
            let scale = (vp_w.min(vp_h) * 0.42) / (max_dim * 0.5);

            let cx = vp_x + vp_w * 0.5;
            let cy = vp_y + vp_h * 0.5;
            let light_dir = model3d::Vec3::new(0.4, 0.6, 0.7).normalize();

            let budget = self.config.model3d.preview_triangle_limit.max(100);
            let source_triangles = if !mesh.thumbnail_triangles.is_empty() {
                &mesh.thumbnail_triangles
            } else {
                &mesh.triangles
            };
            let step = source_triangles.len().div_ceil(budget).max(1);

            let mut projected = Vec::with_capacity(source_triangles.len() / step + 1);
            for tri in source_triangles.iter().step_by(step) {
                let (nx, ny, nz) = if is_z_up {
                    (tri.normal.x, tri.normal.z, -tri.normal.y)
                } else {
                    (tri.normal.x, tri.normal.y, tri.normal.z)
                };

                let rotated_normal = model3d::Vec3::new(
                    nx * cos_y + nz * sin_y,
                    ny * cos_x - (-nx * sin_y + nz * cos_y) * sin_x,
                    ny * sin_x + (-nx * sin_y + nz * cos_y) * cos_x,
                )
                .normalize();

                if rotated_normal.z <= 0.02 {
                    continue;
                }

                let transform_v = |v: model3d::Vec3| -> (f32, f32, f32) {
                    let (dx, dy, dz) = if is_z_up {
                        (v.x - center.x, v.z - center.z, -(v.y - center.y))
                    } else {
                        (v.x - center.x, v.y - center.y, v.z - center.z)
                    };

                    let rx = dx * cos_y + dz * sin_y;
                    let rz = -dx * sin_y + dz * cos_y;
                    let ry = dy;

                    let fy = ry * cos_x - rz * sin_x;
                    let fz = ry * sin_x + rz * cos_x;

                    (cx + rx * scale, cy - fy * scale, fz)
                };

                let (p1x, p1y, p1z) = transform_v(tri.v1);
                let (p2x, p2y, p2z) = transform_v(tri.v2);
                let (p3x, p3y, p3z) = transform_v(tri.v3);

                let cross_2d = (p2x - p1x) * (p3y - p1y) - (p2y - p1y) * (p3x - p1x);
                if cross_2d.abs() < 0.2 {
                    continue;
                }

                let avg_z = (p1z + p2z + p3z) / 3.0;
                let diff = rotated_normal.dot(light_dir).max(0.18).min(1.0);
                projected.push((avg_z, diff, tri.color, (p1x, p1y), (p2x, p2y), (p3x, p3y)));
            }

            projected.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            let base_c = self.color_from_hex(&self.palette.harmonies.base);
            let bg_c = self.color_from_hex(&self.palette.colors.surface_recessed);

            let mut line_stroke = Stroke::default();
            line_stroke.width = 1.2;
            let mut line_paint = Paint::default();
            let wire_c = self.color_from_hex(&self.palette.harmonies.base);
            line_paint.set_color(Color::from_rgba8(
                (wire_c.red() * 255.0) as u8,
                (wire_c.green() * 255.0) as u8,
                (wire_c.blue() * 255.0) as u8,
                220,
            ));

            for (_, diff, tri_color, (p1x, p1y), (p2x, p2y), (p3x, p3y)) in projected {
                let mut pb = PathBuilder::new();
                pb.move_to(p1x, p1y);
                pb.line_to(p2x, p2y);
                pb.line_to(p3x, p3y);
                pb.close();

                if let Some(t_path) = pb.finish() {
                    let tri_base_c = if let Some(c) = tri_color {
                        Color::from_rgba(c[0], c[1], c[2], c[3]).unwrap_or(base_c)
                    } else {
                        base_c
                    };
                    let r = ((bg_c.red() * (1.0 - diff * 0.7) + tri_base_c.red() * diff * 0.7)
                        * 255.0)
                        .clamp(0.0, 255.0) as u8;
                    let g = ((bg_c.green() * (1.0 - diff * 0.7) + tri_base_c.green() * diff * 0.7)
                        * 255.0)
                        .clamp(0.0, 255.0) as u8;
                    let b = ((bg_c.blue() * (1.0 - diff * 0.7) + tri_base_c.blue() * diff * 0.7)
                        * 255.0)
                        .clamp(0.0, 255.0) as u8;

                    let mut fill_paint = Paint::default();
                    fill_paint.set_color(Color::from_rgba8(r, g, b, 235));
                    pixmap.fill_path(
                        &t_path,
                        &fill_paint,
                        tiny_skia::FillRule::Winding,
                        Transform::identity(),
                        None,
                    );

                    pixmap.stroke_path(
                        &t_path,
                        &line_paint,
                        &line_stroke,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }

        let footer_y = ry + rh - 22.0;
        self.draw_text_clipped(
            pixmap,
            "> Open in Workbench to orbit 3D model",
            content_x,
            footer_y,
            12.0,
            self.color_from_hex(&self.palette.colors.text_muted),
            false,
            clip_right,
        );
    }

    fn draw_text_preview(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        ext_hint: Option<&str>,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
        _elision_color: Color,
    ) {
        let clean_text = text.trim_start_matches(is_zero_width_or_bom);
        let trimmed = clean_text.trim();

        // 1. Unit conversion preview
        if let Some(conv) = units::detect_unit_conversion(trimmed) {
            self.draw_unit_conversion_card(pixmap, &conv, rx, ry, rw, rh, is_hero);
            return;
        }

        // 2. IP or MAC network preview
        if let Some(net) = network_preview::detect_network_preview(trimmed) {
            self.draw_network_preview_card(pixmap, &net, rx, ry, rw, rh, is_hero);
            return;
        }

        // 3. Timestamp preview
        if let Some(ts) = timestamps::detect_timestamp(trimmed) {
            self.draw_timestamp_card(pixmap, &ts, rx, ry, rw, rh, is_hero);
            return;
        }

        // 4. Spreadsheet preview (CSV text or spreadsheet file)
        let p = Path::new(trimmed);
        let spreadsheet_opt = if p.is_file() && spreadsheet::is_spreadsheet_file(p) {
            spreadsheet::load_spreadsheet(p).ok()
        } else if ext_hint
            .and_then(little_clip_core::syntax::map_extension_to_language)
            .is_some()
        {
            // Known text/code files follow the same extension-based route as
            // the workbench. Source delimiters are not evidence of a table.
            None
        } else {
            spreadsheet::detect_csv_text(trimmed)
        };
        if let Some(ref sheet_data) = spreadsheet_opt {
            if let Some(first_sheet) = sheet_data.sheets.first() {
                self.draw_spreadsheet_card(
                    pixmap,
                    &sheet_data.title,
                    first_sheet,
                    rx,
                    ry,
                    rw,
                    rh,
                    is_hero,
                );
                return;
            }
        }

        // 5. MIDI File Preview
        if p.is_file()
            && (p
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
                .unwrap_or(false))
        {
            if let Ok(midi_data) = midi::MidiData::parse_file(p) {
                self.draw_midi_card(pixmap, &midi_data, rx, ry, rw, rh, is_hero);
                return;
            }
        }

        // 6. 3D Mesh Preview (rotates slowly in preview)
        if p.is_file() && model3d::is_3d_model_file(p) {
            if let Ok(model) = model3d::Model3d::load_cached(p) {
                self.draw_model3d_card(pixmap, &model, rx, ry, rw, rh, is_hero);
                return;
            }
        }

        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let text_font_size = if is_hero { 18.0 } else { 15.0 };
        let line_height = if is_hero { 24.0 } else { 20.0 };

        // Classification above uses the normalized text, but the previous renderer
        // accidentally switched back to `text` here. A clipboard/file payload with
        // a leading newline or space therefore produced a visibly indented first
        // glyph in the tiny preview.
        let lines: Vec<&str> = trimmed.lines().collect();
        let total_lines = lines.len();
        let max_y = ry + rh - 22.0;
        let mut line_y = ry + 124.0;

        let lang_id = little_clip_core::syntax::detect_language(trimmed, ext_hint);
        let syntax = SYNTAX_SET
            .find_syntax_by_extension(lang_id)
            .or_else(|| SYNTAX_SET.find_syntax_by_token(lang_id))
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

        let theme = THEME_SET
            .themes
            .get("base16-ocean.dark")
            .or_else(|| THEME_SET.themes.values().next())
            .expect("default theme");

        let mut highlighter = HighlightLines::new(syntax, theme);

        for (idx, line) in lines.iter().enumerate() {
            let is_near_bottom = line_y + line_height > max_y;
            let has_more = idx + 1 < total_lines;

            if is_near_bottom && has_more {
                let remaining = total_lines - idx;
                let badge = format!(
                    "▼ +{} more lines ({})",
                    remaining,
                    little_clip_core::syntax::language_display_name(lang_id)
                );
                self.draw_footer_badge(
                    pixmap,
                    &badge,
                    content_x,
                    line_y,
                    text_font_size,
                    clip_right,
                );
                break;
            }

            if line_y > max_y {
                break;
            }

            let ranges = highlighter
                .highlight_line(line, &SYNTAX_SET)
                .unwrap_or_default();

            let mut cx = content_x;
            for (style, token_str) in ranges {
                if cx >= clip_right {
                    break;
                }
                let fg = Color::from_rgba8(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                    255,
                );
                let font_style = if style
                    .font_style
                    .contains(syntect::highlighting::FontStyle::BOLD)
                {
                    FontStyle::Bold
                } else {
                    FontStyle::Mono
                };
                cx = self.draw_text_styled_clipped(
                    pixmap,
                    token_str,
                    cx,
                    line_y,
                    text_font_size,
                    fg,
                    font_style,
                    clip_right,
                );
            }

            line_y += line_height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::config::AppConfig;

    #[test]
    fn code_file_extension_takes_precedence_over_spreadsheet_heuristics() {
        let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
        // Semicolons in consecutive imports satisfy the table shape heuristic.
        let source = "use crate::one;\nuse crate::two;\nuse crate::three;";
        assert!(spreadsheet::detect_csv_text(source).is_some());
        let mut code = Pixmap::new(500, 400).unwrap();
        let mut table = code.clone();
        renderer.draw_text_preview(
            &mut code,
            source,
            Some("rs"),
            0.0,
            0.0,
            500.0,
            400.0,
            true,
            Color::WHITE,
        );
        renderer.draw_text_preview(
            &mut table,
            source,
            None,
            0.0,
            0.0,
            500.0,
            400.0,
            true,
            Color::WHITE,
        );
        assert_ne!(
            code.data(),
            table.data(),
            "Rust source must use the code preview"
        );
    }
}
