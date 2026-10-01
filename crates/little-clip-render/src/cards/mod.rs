//! Card renderers for slot cards, media, spreadsheets, 3D models, and formatted text.

mod audio;
mod images;
mod rich_media;
mod structured;
mod text;

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
}

#[cfg(test)]
mod tests;
