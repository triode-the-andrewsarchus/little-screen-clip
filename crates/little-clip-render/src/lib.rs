use fontdue::{Font, FontSettings};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use little_clip_core::config::{parse_hex_color, AppConfig};
use little_clip_core::{ClipItem, ClipPayload, SlotId, SlotManager};
use std::path::Path;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use thiserror::Error;
use tiny_skia::*;
use tracing::debug;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_nonewlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

pub const SCREEN_WIDTH: u32 = 1920;
pub const SCREEN_HEIGHT: u32 = 462;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("Font load error: {0}")]
    FontLoad(String),
    #[error("Image encode error: {0}")]
    ImageEncode(String),
}

pub mod markdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
}

pub struct ScreenRenderer {
    font_regular: Option<Font>,
    font_bold: Option<Font>,
    font_italic: Option<Font>,
    font_mono: Option<Font>,
    pub config: AppConfig,
}

impl ScreenRenderer {
    pub fn new(config: AppConfig) -> Result<Self, RenderError> {
        let (font_regular, font_bold, font_italic, font_mono) = Self::load_fonts();
        Ok(Self {
            font_regular,
            font_bold,
            font_italic,
            font_mono,
            config,
        })
    }

    pub fn fallback(config: AppConfig) -> Self {
        Self {
            font_regular: None,
            font_bold: None,
            font_italic: None,
            font_mono: None,
            config,
        }
    }

    pub fn default_renderer() -> Result<Self, RenderError> {
        let config = AppConfig::load();
        Self::new(config)
    }

    pub fn set_config(&mut self, config: AppConfig) {
        self.config = config;
    }

    fn load_fonts() -> (Option<Font>, Option<Font>, Option<Font>, Option<Font>) {
        let regular_paths = [
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        ];
        let bold_paths = [
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
        ];
        let italic_paths = [
            "/usr/share/fonts/TTF/DejaVuSans-Oblique.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Oblique.ttf",
        ];
        let mono_paths = [
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansMono.ttf",
        ];

        let regular = regular_paths.iter().find_map(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|data| Font::from_bytes(data, FontSettings::default()).ok())
        });

        let bold = bold_paths.iter().find_map(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|data| Font::from_bytes(data, FontSettings::default()).ok())
        });

        let italic = italic_paths.iter().find_map(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|data| Font::from_bytes(data, FontSettings::default()).ok())
        });

        let mono = mono_paths.iter().find_map(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|data| Font::from_bytes(data, FontSettings::default()).ok())
        });

        (regular, bold, italic, mono)
    }
}

pub use tiny_skia;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElidedText {
    pub prefix: String,
    pub elision: Option<String>,
    pub suffix: Option<String>,
}

impl ElidedText {
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
    /// Renders the complete 1920x462 hotbar layout into a tiny-skia Pixmap.
    /// Order: [Undo] [Left hand] [L prior(1)] [L prior(2)] [R prior(2)] [R prior(1)] [Right hand]
    pub fn render_pixmap(&self, manager: &SlotManager) -> Result<Pixmap, RenderError> {
        let mut pixmap = Pixmap::new(SCREEN_WIDTH, SCREEN_HEIGHT)
            .ok_or_else(|| RenderError::ImageEncode("Failed to allocate pixmap".to_string()))?;

        // Background: Configurable canvas color
        let (bg_r, bg_g, bg_b, bg_a) = parse_hex_color(&self.config.colors.background);
        pixmap.fill(Color::from_rgba8(bg_r, bg_g, bg_b, bg_a));

        // 1. [Undo] (x: 0, w: 240)
        self.draw_slot_card(&mut pixmap, manager, SlotId::Undo, 0, 0, 240, SCREEN_HEIGHT, false);

        // 2. [Left hand] (x: 240, w: 360) - Hero Left
        self.draw_slot_card(&mut pixmap, manager, SlotId::LeftHand, 240, 0, 360, SCREEN_HEIGHT, true);

        // 3. [L prior(1)] (x: 600, w: 240)
        self.draw_slot_card(&mut pixmap, manager, SlotId::LeftPrior1, 600, 0, 240, SCREEN_HEIGHT, false);

        // 4. [L prior(2)] (x: 840, w: 240)
        self.draw_slot_card(&mut pixmap, manager, SlotId::LeftPrior2, 840, 0, 240, SCREEN_HEIGHT, false);

        // 5. [R prior(2)] (x: 1080, w: 240)
        self.draw_slot_card(&mut pixmap, manager, SlotId::RightPrior2, 1080, 0, 240, SCREEN_HEIGHT, false);

        // 6. [R prior(1)] (x: 1320, w: 240)
        self.draw_slot_card(&mut pixmap, manager, SlotId::RightPrior1, 1320, 0, 240, SCREEN_HEIGHT, false);

        // 7. [Right hand] (x: 1560, w: 360) - Hero Right
        self.draw_slot_card(&mut pixmap, manager, SlotId::RightHand, 1560, 0, 360, SCREEN_HEIGHT, true);

        // Draw Debug Handler badge in bottom left of screen if enabled
        if self.config.display.show_debug_handler {
            let l_handler = manager.left_hand().map(|i| i.handler_tag.as_str()).unwrap_or("EMPTY");
            let r_handler = manager.right_hand().map(|i| i.handler_tag.as_str()).unwrap_or("EMPTY");
            let handler_text = format!("LH: [{}] | RH: [{}]", l_handler, r_handler);

            let badge_x = 18.0;
            let badge_y = SCREEN_HEIGHT as f32 - 42.0;
            let badge_w = 460.0;
            let badge_h = 32.0;
            if let Some(br) = Rect::from_xywh(badge_x, badge_y, badge_w, badge_h) {
                let (br_r, br_g, br_b, br_a) = parse_hex_color(&self.config.colors.debug_badge_bg);
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(br_r, br_g, br_b, br_a));
                pixmap.fill_rect(br, &paint, Transform::identity(), None);

                let (str_r, str_g, str_b, str_a) = parse_hex_color(&self.config.colors.debug_badge_text);
                let mut stroke_paint = Paint::default();
                stroke_paint.set_color(Color::from_rgba8(str_r, str_g, str_b, (str_a as f32 * 0.8) as u8));
                let mut stroke = Stroke::default();
                stroke.width = 1.5;
                let path = PathBuilder::from_rect(br);
                pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
            }

            let (txt_r, txt_g, txt_b, txt_a) = parse_hex_color(&self.config.colors.debug_badge_text);
            self.draw_text(
                &mut pixmap,
                &handler_text,
                badge_x + 12.0,
                badge_y + 4.0,
                18.0,
                Color::from_rgba8(txt_r, txt_g, txt_b, txt_a),
                true,
            );
        }

        Ok(pixmap)
    }

    /// Renders the complete 1920x462 hotbar layout into a compressed JPEG.
    pub fn render_frame(&self, manager: &SlotManager) -> Result<Vec<u8>, RenderError> {
        let pixmap = self.render_pixmap(manager)?;
        self.encode_jpeg(&pixmap)
    }

    /// Renders a completely black 1920x462 frame (all pixels turned off/dark)
    /// into a compressed JPEG for screen-saver / lock mode.
    pub fn render_blank_frame(&self) -> Result<Vec<u8>, RenderError> {
        let mut pixmap = Pixmap::new(SCREEN_WIDTH, SCREEN_HEIGHT)
            .ok_or_else(|| RenderError::ImageEncode("Failed to allocate blank pixmap".to_string()))?;
        pixmap.fill(Color::from_rgba8(0, 0, 0, 255));
        self.encode_jpeg(&pixmap)
    }

    fn draw_slot_card(
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
        let pad = 6.0;
        let rect_x = x as f32 + pad;
        let rect_y = y as f32 + pad;
        let rect_w = w as f32 - pad * 2.0;
        let rect_h = h as f32 - pad * 2.0;

        let is_selected = manager.selected.contains(&slot_id);
        let is_cursor = manager.cursor == slot_id;

        // Card background fill
        let bg_hex = if is_hero {
            &self.config.colors.hero_card_bg
        } else if slot_id == SlotId::Undo {
            &self.config.colors.undo_card_bg
        } else {
            &self.config.colors.prior_card_bg
        };
        let (bg_r, bg_g, bg_b, bg_a) = parse_hex_color(bg_hex);
        let bg_color = Color::from_rgba8(bg_r, bg_g, bg_b, bg_a);

        let card_rect = Rect::from_xywh(rect_x, rect_y, rect_w, rect_h);
        if let Some(cr) = card_rect {
            let mut paint = Paint::default();
            paint.set_color(bg_color);
            pixmap.fill_rect(cr, &paint, Transform::identity(), None);

            // Card Border
            let border_hex = if is_cursor {
                &self.config.colors.cursor_border
            } else if is_selected {
                &self.config.colors.selected_border
            } else {
                match slot_id {
                    SlotId::Undo => &self.config.colors.undo_border,
                    SlotId::LeftHand => &self.config.colors.left_hand_border,
                    SlotId::LeftPrior1 => &self.config.colors.left_prior1_border,
                    SlotId::LeftPrior2 => &self.config.colors.left_prior2_border,
                    SlotId::RightPrior2 => &self.config.colors.right_prior2_border,
                    SlotId::RightPrior1 => &self.config.colors.right_prior1_border,
                    SlotId::RightHand => &self.config.colors.right_hand_border,
                }
            };
            let (br_r, br_g, br_b, br_a) = parse_hex_color(border_hex);
            let border_color = Color::from_rgba8(br_r, br_g, br_b, br_a);

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

        // Card Border Color for slot
        let border_hex = if is_cursor {
            &self.config.colors.cursor_border
        } else if is_selected {
            &self.config.colors.selected_border
        } else {
            match slot_id {
                SlotId::Undo => &self.config.colors.undo_border,
                SlotId::LeftHand => &self.config.colors.left_hand_border,
                SlotId::LeftPrior1 => &self.config.colors.left_prior1_border,
                SlotId::LeftPrior2 => &self.config.colors.left_prior2_border,
                SlotId::RightPrior2 => &self.config.colors.right_prior2_border,
                SlotId::RightPrior1 => &self.config.colors.right_prior1_border,
                SlotId::RightHand => &self.config.colors.right_hand_border,
            }
        };
        let (br_r, br_g, br_b, br_a) = parse_hex_color(border_hex);
        let border_color = Color::from_rgba8(br_r, br_g, br_b, br_a);

        // Header Pill / Slot Label (Font 30px for hero, 24px for prior)
        let header_text = slot_id.label();
        let header_hex = match slot_id {
            SlotId::Undo => &self.config.colors.undo_border,
            SlotId::LeftHand => &self.config.colors.left_hand_border,
            SlotId::LeftPrior1 => &self.config.colors.left_prior1_border,
            SlotId::LeftPrior2 => &self.config.colors.left_prior2_border,
            SlotId::RightPrior2 => &self.config.colors.right_prior2_border,
            SlotId::RightPrior1 => &self.config.colors.right_prior1_border,
            SlotId::RightHand => &self.config.colors.right_hand_border,
        };
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
            self.draw_item_content(pixmap, item, rect_x, rect_y, rect_w, rect_h, is_hero, border_color);
        } else {
            // Empty placeholder
            self.draw_text_clipped(
                pixmap,
                "EMPTY",
                rect_x + 14.0,
                rect_y + 54.0,
                if is_hero { 26.0 } else { 21.0 },
                Color::from_rgba8(75, 85, 99, 255),
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
        let (t_r, t_g, t_b, t_a) = parse_hex_color(&self.config.colors.title_color);
        let (s_r, s_g, s_b, s_a) = parse_hex_color(&self.config.colors.subtitle_color);
        let (u_r, u_g, u_b, u_a) = parse_hex_color(&self.config.colors.url_color);

        let content_x = rx + 14.0;
        let max_content_w = (rw - 28.0).max(10.0);
        let clip_right = rx + rw - 10.0;

        // Resolve elision character color:
        // Default: "border" -> dynamically matches each slot box's border color
        let elision_color = if self.config.display.elision_color.eq_ignore_ascii_case("border") {
            border_color
        } else {
            let (er, eg, eb, ea) = parse_hex_color(&self.config.display.elision_color);
            Color::from_rgba8(er, eg, eb, ea)
        };

        // Title: 28px for hero (down from 34px), 21px for history (smaller-but-not-too-small)
        let title_font_size = if is_hero { 28.0 } else { 21.0 };
        let title_color = Color::from_rgba8(t_r, t_g, t_b, t_a);
        let elided_title = self.truncate_filename_elided(&item.title, max_content_w, title_font_size, true);
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
        let elided_subtitle = self.truncate_filename_elided(&item.subtitle, max_content_w, sub_font_size, false);
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
                            (file_content, p.extension().and_then(|s| s.to_str()))
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
                self.draw_text(
                    pixmap,
                    hex,
                    rx + 16.0,
                    ry + 320.0,
                    42.0,
                    Color::from_rgba8(255, 255, 255, 255),
                    true,
                );
            }
            ClipPayload::Url(url) => {
                let url_str = url.as_str();
                let line1: String = url_str.chars().take(if is_hero { 26 } else { 12 }).collect();
                let line2: String = url_str.chars().skip(if is_hero { 26 } else { 12 }).take(if is_hero { 26 } else { 12 }).collect();

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
            ClipPayload::Image { width, height, thumbnail, .. } => {
                self.draw_image_thumbnail(pixmap, *width, *height, thumbnail.as_ref(), rx, ry, rw, rh, is_hero);
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
                if files.len() == 1 && little_clip_core::syntax::is_code_file(&files[0]) {
                    let p = &files[0];
                    if let Ok(file_content) = std::fs::read_to_string(p) {
                        let ext = p.extension().and_then(|s| s.to_str());
                        self.draw_text_preview(
                            pixmap,
                            &file_content,
                            ext,
                            rx,
                            ry,
                            rw,
                            rh,
                            is_hero,
                            elision_color,
                        );
                    } else {
                        self.draw_text(
                            pixmap,
                            &format!("FILES: {}", files.len()),
                            rx + 16.0,
                            ry + 130.0,
                            26.0,
                            Color::from_rgba8(250, 204, 21, 255),
                            true,
                        );
                    }
                } else {
                    self.draw_text(
                        pixmap,
                        &format!("FILES: {}", files.len()),
                        rx + 16.0,
                        ry + 130.0,
                        26.0,
                        Color::from_rgba8(250, 204, 21, 255),
                        true,
                    );
                }
            }
            ClipPayload::Directory(_) => {
                self.draw_text(
                    pixmap,
                    "DIRECTORY",
                    rx + 16.0,
                    ry + 130.0,
                    26.0,
                    Color::from_rgba8(59, 130, 246, 255),
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
            if let Some(pixmap_ref) = PixmapRef::from_bytes(&thumb.premultiplied_rgba, thumb.width, thumb.height) {
                let scale = (box_w / thumb.width as f32)
                    .min(box_h / thumb.height as f32)
                    .min(1.0);
                let draw_w = thumb.width as f32 * scale;
                let draw_h = thumb.height as f32 * scale;

                let center_x = rx + 16.0 + (box_w - draw_w) / 2.0;
                let center_y = target_top + (box_h - draw_h) / 2.0;

                // Card backdrop for thumbnail
                if let Some(bg_r) = Rect::from_xywh(center_x - 2.0, center_y - 2.0, draw_w + 4.0, draw_h + 4.0) {
                    let mut bg_paint = Paint::default();
                    bg_paint.set_color(Color::from_rgba8(35, 40, 55, 255));
                    pixmap.fill_rect(bg_r, &bg_paint, Transform::identity(), None);
                }

                let transform = Transform::from_scale(scale, scale)
                    .post_translate(center_x, center_y);

                pixmap.draw_pixmap(
                    0,
                    0,
                    pixmap_ref,
                    &PixmapPaint::default(),
                    transform,
                    None,
                );
            }
        } else {
            // Sleek placeholder while generating preview async
            if let Some(card_r) = Rect::from_xywh(rx + 16.0, target_top, box_w as f32, box_h as f32) {
                let mut p = Paint::default();
                p.set_color(Color::from_rgba8(25, 30, 42, 255));
                pixmap.fill_rect(card_r, &p, Transform::identity(), None);

                let mut sp = Paint::default();
                sp.set_color(Color::from_rgba8(56, 189, 248, 120));
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
                Color::from_rgba8(56, 189, 248, 255),
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
                Color::from_rgba8(148, 163, 184, 255),
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
                p.set_color(Color::from_rgba8(20, 24, 34, 255));
                pixmap.fill_rect(r, &p, Transform::identity(), None);
            }
            self.draw_text_clipped(
                pixmap,
                "IMAGE BATCH",
                content_x + 10.0,
                target_top + box_h / 2.0 - 8.0,
                16.0,
                Color::from_rgba8(100, 116, 139, 255),
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
                    (content_x + (box_w - half_w) / 2.0, target_top + half_h + gap, half_w, half_h),
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
                    (content_x + half_w + gap, target_top + half_h + gap, half_w, half_h),
                ]
            }
        };

        for (i, &(cx, cy, cw, ch)) in cell_rects.iter().enumerate() {
            // Card cell background
            if let Some(cr) = Rect::from_xywh(cx, cy, cw, ch) {
                let mut bg_paint = Paint::default();
                bg_paint.set_color(Color::from_rgba8(18, 22, 32, 230));
                pixmap.fill_rect(cr, &bg_paint, Transform::identity(), None);

                let mut stroke = Stroke::default();
                stroke.width = 1.0;
                let mut sp = Paint::default();
                sp.set_color(Color::from_rgba8(51, 65, 85, 180));
                let path = PathBuilder::from_rect(cr);
                pixmap.stroke_path(&path, &sp, &stroke, Transform::identity(), None);
            }

            // Draw scaled thumbnail centered in cell
            if let Some(thumb) = thumbnails.get(i) {
                if let Some(pixmap_ref) = PixmapRef::from_bytes(&thumb.premultiplied_rgba, thumb.width, thumb.height) {
                    let scale = ((cw - 4.0) / thumb.width as f32)
                        .min((ch - 4.0) / thumb.height as f32)
                        .min(1.0);
                    let dw = thumb.width as f32 * scale;
                    let dh = thumb.height as f32 * scale;

                    let img_x = cx + (cw - dw) / 2.0;
                    let img_y = cy + (ch - dh) / 2.0;

                    let transform = Transform::from_scale(scale, scale).post_translate(img_x, img_y);
                    pixmap.draw_pixmap(0, 0, pixmap_ref, &PixmapPaint::default(), transform, None);
                }
            }

            // Subtle index number badge in top-left of each cell (e.g. "#1", "#2", "#11", "#12")
            let idx_num = preview_indices.get(i).copied().unwrap_or(i) + 1;
            let idx_str = format!("#{}", idx_num);
            let badge_font_size = if is_hero { 12.0 } else { 10.0 };
            let badge_w = self.measure_text_width(&idx_str, badge_font_size, true) + 8.0;
            let badge_h = badge_font_size + 4.0;
            if let Some(br) = Rect::from_xywh(cx + 4.0, cy + 4.0, badge_w, badge_h) {
                let mut bp = Paint::default();
                bp.set_color(Color::from_rgba8(15, 23, 42, 210));
                pixmap.fill_rect(br, &bp, Transform::identity(), None);
            }
            self.draw_text_clipped(
                pixmap,
                &idx_str,
                cx + 8.0,
                cy + 5.0,
                badge_font_size,
                Color::from_rgba8(148, 163, 184, 255),
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

            if let Some(br) = Rect::from_xywh(bx, by, badge_w, badge_h) {
                let mut bp = Paint::default();
                bp.set_color(Color::from_rgba8(15, 23, 42, 245));
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
                Color::from_rgba8(248, 250, 252, 255),
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
                if let Ok(dyn_img) = image::load_from_memory_with_format(jpeg_bytes, image::ImageFormat::Jpeg) {
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

                        // Card backdrop for animated frame
                        if let Some(bg_r) = Rect::from_xywh(center_x - 2.0, center_y - 2.0, fw as f32 + 4.0, fh as f32 + 4.0) {
                            let mut bg_paint = Paint::default();
                            bg_paint.set_color(Color::from_rgba8(35, 40, 55, 255));
                            pixmap.fill_rect(bg_r, &bg_paint, Transform::identity(), None);
                        }

                        pixmap.draw_pixmap(
                            center_x as i32,
                            center_y as i32,
                            pixmap_ref,
                            &PixmapPaint::default(),
                            Transform::identity(),
                            None,
                        );

                        // Looping badge in top-left of box
                        let badge = format!("▶ {} [{}/{}]", format, frame_idx + 1, thumb.frame_count);
                        self.draw_text_clipped(
                            pixmap,
                            &badge,
                            rx + 18.0,
                            target_top + 18.0,
                            14.0,
                            Color::from_rgba8(251, 191, 36, 230),
                            true,
                            clip_right,
                        );
                    }
                }
            }
        } else {
            // Placeholder while extracting frames
            if let Some(card_r) = Rect::from_xywh(rx + 14.0, target_top, box_w as f32, box_h as f32) {
                let mut p = Paint::default();
                p.set_color(Color::from_rgba8(25, 30, 42, 255));
                pixmap.fill_rect(card_r, &p, Transform::identity(), None);

                let mut sp = Paint::default();
                sp.set_color(Color::from_rgba8(168, 85, 247, 120));
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
                Color::from_rgba8(168, 85, 247, 255),
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
            Color::from_rgba8(148, 163, 184, 255),
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
                    Color::from_rgba8(148, 163, 184, 220),
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
                    Color::from_rgba8(148, 163, 184, 180),
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
                    Color::from_rgba8(99, 102, 241, 255),
                    true,
                );
                self.draw_text(
                    pixmap,
                    &format!("Decoding {} at {:.1} kHz", format, sample_rate as f64 / 1000.0),
                    content_x,
                    content_y + 65.0,
                    18.0,
                    Color::from_rgba8(148, 163, 184, 220),
                    false,
                );
                if let Some(r) = Rect::from_xywh(content_x, content_y + 110.0, content_w, 3.0) {
                    let mut paint = Paint::default();
                    paint.set_color(Color::from_rgba8(99, 102, 241, 140));
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
        let num_bars = ((content_w / bar_spacing).floor() as usize).max(10).min(total_peaks);
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
                Color::from_rgba8(148, 163, 184, 180),
                true,
            );
            self.draw_text(
                pixmap,
                "R",
                content_x,
                content_y + ch_h + 24.0,
                14.0,
                Color::from_rgba8(148, 163, 184, 180),
                true,
            );

            // Center divider line
            let div_y = content_y + ch_h + 7.0;
            if let Some(r) = Rect::from_xywh(content_x + 16.0, div_y, content_w - 16.0, 1.0) {
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(51, 65, 85, 120));
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

            // Subtle center axis
            if let Some(r) = Rect::from_xywh(content_x, center_y, content_w, 1.0) {
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(51, 65, 85, 120));
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

            self.draw_text(
                pixmap,
                &start_str,
                content_x,
                time_y,
                16.0,
                Color::from_rgba8(251, 191, 36, 230),
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
                Color::from_rgba8(251, 191, 36, 220),
                true,
            );

            let end_x = (content_x + content_w - 42.0).max(content_x + 50.0);
            self.draw_text(
                pixmap,
                &end_str,
                end_x,
                time_y,
                16.0,
                Color::from_rgba8(251, 191, 36, 230),
                false,
            );
        } else {
            self.draw_text(
                pixmap,
                "00:00",
                content_x,
                time_y,
                16.0,
                Color::from_rgba8(100, 116, 139, 200),
                false,
            );
            let end_x = (content_x + content_w - 42.0).max(content_x + 50.0);
            self.draw_text(
                pixmap,
                &dur_text,
                end_x,
                time_y,
                16.0,
                Color::from_rgba8(100, 116, 139, 200),
                false,
            );
        }
    }

    /// TrueType text width measurement with styling.
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
            text.chars().map(|ch| font.metrics(ch, size).advance_width).sum()
        } else {
            text.chars().count() as f32 * size * 0.6
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
                prefix: name.chars().next().map(|c| c.to_string()).unwrap_or_default(),
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
                                data[idx + 1] = ((cg * a_norm + data[idx + 1] as u32 * inv_a) / 255) as u8;
                                data[idx + 2] = ((cb * a_norm + data[idx + 2] as u32 * inv_a) / 255) as u8;
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

    fn draw_text_clipped(
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

    fn draw_elided_text_clipped(
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
        cx = self.draw_text_clipped(pixmap, &elided.prefix, cx, y, size, base_color, bold, clip_right);
        if let Some(ref elision_str) = elided.elision {
            cx = self.draw_text_clipped(pixmap, elision_str, cx, y, size, elision_color, bold, clip_right);
        }
        if let Some(ref suffix_str) = elided.suffix {
            cx = self.draw_text_clipped(pixmap, suffix_str, cx, y, size, base_color, bold, clip_right);
        }
        cx
    }

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
        let (u_r, u_g, u_b, u_a) = parse_hex_color(&self.config.colors.url_color);
        let url_color = Color::from_rgba8(u_r, u_g, u_b, u_a);
        let title_color = Color::from_rgba8(248, 250, 252, 255);
        let italic_color = Color::from_rgba8(203, 213, 225, 240);
        let code_color = Color::from_rgba8(245, 158, 11, 255);

        for span in spans {
            if cx >= clip_right {
                break;
            }
            match span {
                MarkdownSpan::Text(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap, s, cx, y, size, default_color, FontStyle::Regular, clip_right,
                    );
                }
                MarkdownSpan::Bold(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap, s, cx, y, size, title_color, FontStyle::Bold, clip_right,
                    );
                }
                MarkdownSpan::Italic(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap, s, cx, y, size, italic_color, FontStyle::Italic, clip_right,
                    );
                }
                MarkdownSpan::BoldItalic(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap, s, cx, y, size, title_color, FontStyle::BoldItalic, clip_right,
                    );
                }
                MarkdownSpan::Code(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap, s, cx, y, size, code_color, FontStyle::Mono, clip_right,
                    );
                }
                MarkdownSpan::Link { label, .. } => {
                    cx = self.draw_text_styled_clipped(
                        pixmap, label, cx, y, size, url_color, FontStyle::Regular, clip_right,
                    );
                }
            }
        }
        cx
    }

    fn draw_footer_badge(
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
        if let Some(rect) = Rect::from_xywh(x, y - 2.0, badge_w.min(clip_right - x), badge_h) {
            let mut paint = Paint::default();
            paint.set_color(Color::from_rgba8(30, 41, 59, 230));
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);

            let mut stroke = Stroke::default();
            stroke.width = 1.0;
            let mut stroke_paint = Paint::default();
            stroke_paint.set_color(Color::from_rgba8(71, 85, 105, 180));
            let path = PathBuilder::from_rect(rect);
            pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
        }

        self.draw_text_clipped(
            pixmap,
            badge_text,
            x + 8.0,
            y,
            size - 2.0,
            Color::from_rgba8(148, 163, 184, 255),
            true,
            clip_right,
        );
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
        let (b_r, b_g, b_b, b_a) = parse_hex_color(&self.config.colors.body_color);
        let body_color = Color::from_rgba8(b_r, b_g, b_b, b_a);
        let content_x = rx + 14.0;
        let max_content_w = (rw - 28.0).max(10.0);
        let clip_right = rx + rw - 10.0;

        let lines = crate::markdown::parse_markdown(content);
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
                MarkdownLine::Heading { level, text: _, spans } => {
                    let (head_size, head_color, lead_symbol) = match level {
                        1 => (
                            if is_hero { 21.0 } else { 18.0 },
                            Color::from_rgba8(96, 165, 250, 255),
                            "# ",
                        ),
                        2 => (
                            if is_hero { 19.0 } else { 16.5 },
                            Color::from_rgba8(147, 197, 253, 255),
                            "## ",
                        ),
                        _ => (
                            if is_hero { 17.5 } else { 15.0 },
                            Color::from_rgba8(191, 219, 254, 255),
                            "### ",
                        ),
                    };

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
                    self.draw_markdown_spans(pixmap, spans, cx, y, head_size, head_color, clip_right);
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
                    self.draw_markdown_spans(pixmap, spans, cx, y, font_size, body_color, clip_right);
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
                    self.draw_markdown_spans(pixmap, spans, cx, y, font_size, body_color, clip_right);
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Quote { spans } => {
                    if let Some(qr) = Rect::from_xywh(content_x, y, 2.5, if is_hero { 18.0 } else { 15.0 }) {
                        let mut qp = Paint::default();
                        qp.set_color(Color::from_rgba8(71, 85, 105, 255));
                        pixmap.fill_rect(qr, &qp, Transform::identity(), None);
                    }
                    let font_size = if is_hero { 17.5 } else { 14.5 };
                    self.draw_markdown_spans(pixmap, spans, content_x + 10.0, y, font_size, Color::from_rgba8(148, 163, 184, 255), clip_right);
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Rule => {
                    if let Some(rr) = Rect::from_xywh(content_x, y + 6.0, max_content_w, 1.0) {
                        let mut rp = Paint::default();
                        rp.set_color(Color::from_rgba8(51, 65, 85, 255));
                        pixmap.fill_rect(rr, &rp, Transform::identity(), None);
                    }
                    y += 14.0;
                }
                MarkdownLine::CodeFence { language } => {
                    let label = if language.is_empty() { "```".to_string() } else { format!("``` {}", language) };
                    self.draw_text_styled_clipped(
                        pixmap,
                        &label,
                        content_x,
                        y,
                        14.0,
                        Color::from_rgba8(100, 116, 139, 255),
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += 18.0;
                }
                MarkdownLine::CodeLine { line } => {
                    let font_size = if is_hero { 16.0 } else { 13.5 };
                    if let Some(bg) = Rect::from_xywh(content_x, y - 2.0, max_content_w, font_size + 4.0) {
                        let mut p = Paint::default();
                        p.set_color(Color::from_rgba8(15, 23, 42, 220));
                        pixmap.fill_rect(bg, &p, Transform::identity(), None);
                    }
                    self.draw_text_styled_clipped(
                        pixmap,
                        line,
                        content_x + 6.0,
                        y,
                        font_size,
                        Color::from_rgba8(245, 158, 11, 255),
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += if is_hero { 22.0 } else { 19.0 };
                }
                MarkdownLine::Paragraph { spans } => {
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    self.draw_markdown_spans(pixmap, spans, content_x, y, font_size, body_color, clip_right);
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Empty => {
                    y += if is_hero { 10.0 } else { 8.0 };
                }
            }
        }
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
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let text_font_size = if is_hero { 18.0 } else { 15.0 };
        let line_height = if is_hero { 24.0 } else { 20.0 };

        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();
        let max_y = ry + rh - 22.0;
        let mut line_y = ry + 124.0;

        let lang_id = little_clip_core::syntax::detect_language(text, ext_hint);
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
                let badge = format!("▼ +{} more lines ({})", remaining, little_clip_core::syntax::language_display_name(lang_id));
                self.draw_footer_badge(pixmap, &badge, content_x, line_y, text_font_size, clip_right);
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
                let fg = Color::from_rgba8(style.foreground.r, style.foreground.g, style.foreground.b, 255);
                let font_style = if style.font_style.contains(syntect::highlighting::FontStyle::BOLD) {
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


    fn draw_text(
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

    /// Compresses the tiny-skia Pixmap to a JPEG under the 450 KB hardware ceiling.
    fn encode_jpeg(&self, pixmap: &Pixmap) -> Result<Vec<u8>, RenderError> {
        let width = pixmap.width();
        let height = pixmap.height();

        // Convert Premultiplied RGBA to standard RGB8
        let mut rgb_data = vec![0u8; (width * height * 3) as usize];
        let pixels = pixmap.data();

        if self.config.display.rotate_180 {
            // Apply 180° rotation during RGB copy
            for y in 0..height {
                for x in 0..width {
                    let src_idx = ((y * width + x) * 4) as usize;
                    let dst_x = width - 1 - x;
                    let dst_y = height - 1 - y;
                    let dst_idx = ((dst_y * width + dst_x) * 3) as usize;

                    let a = pixels[src_idx + 3] as f32 / 255.0;
                    if a > 0.0 {
                        rgb_data[dst_idx] = ((pixels[src_idx] as f32) / a).min(255.0) as u8;
                        rgb_data[dst_idx + 1] = ((pixels[src_idx + 1] as f32) / a).min(255.0) as u8;
                        rgb_data[dst_idx + 2] = ((pixels[src_idx + 2] as f32) / a).min(255.0) as u8;
                    } else {
                        rgb_data[dst_idx] = 0;
                        rgb_data[dst_idx + 1] = 0;
                        rgb_data[dst_idx + 2] = 0;
                    }
                }
            }
        } else {
            for y in 0..height {
                for x in 0..width {
                    let src_idx = ((y * width + x) * 4) as usize;
                    let dst_idx = ((y * width + x) * 3) as usize;
                    let a = pixels[src_idx + 3] as f32 / 255.0;
                    if a > 0.0 {
                        rgb_data[dst_idx] = ((pixels[src_idx] as f32) / a).min(255.0) as u8;
                        rgb_data[dst_idx + 1] = ((pixels[src_idx + 1] as f32) / a).min(255.0) as u8;
                        rgb_data[dst_idx + 2] = ((pixels[src_idx + 2] as f32) / a).min(255.0) as u8;
                    } else {
                        rgb_data[dst_idx] = 0;
                        rgb_data[dst_idx + 1] = 0;
                        rgb_data[dst_idx + 2] = 0;
                    }
                }
            }
        }

        // Compress to JPEG with quality ~85
        let mut jpeg_bytes = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_bytes, 85);
        encoder
            .encode(&rgb_data, width, height, ExtendedColorType::Rgb8)
            .map_err(|e| RenderError::ImageEncode(e.to_string()))?;

        debug!("Encoded 1920x462 JPEG: {} bytes", jpeg_bytes.len());
        Ok(jpeg_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tiny_skia_image_draw() {
        let mut pixmap = Pixmap::new(200, 200).unwrap();
        let rgba_data = vec![255u8; 50 * 50 * 4];
        let sub = PixmapRef::from_bytes(&rgba_data, 50, 50).unwrap();
        pixmap.draw_pixmap(10, 10, sub, &PixmapPaint::default(), Transform::identity(), None);
    }

    #[test]
    fn test_truncate_filename_elided() {
        let renderer = ScreenRenderer::fallback(AppConfig::default());
        
        // 1. Short filename fits as-is
        let short = renderer.truncate_filename_elided("test.png", 200.0, 24.0, true);
        assert_eq!(short.prefix, "test.png");
        assert_eq!(short.elision, None);
        assert_eq!(short.full_text(), "test.png");

        // 2. Long filename with extension preserves extension and uses single-character elision '▸'
        let long_ext = renderer.truncate_filename_elided("very_long_filename.jpg", 100.0, 24.0, true);
        assert_eq!(long_ext.suffix.as_deref(), Some(".jpg"));
        assert_eq!(long_ext.elision.as_deref(), Some("▸"));
        assert!(long_ext.full_text().ends_with(".jpg"));
        // 3. Long text without extension adds '▸' at the end
        let long_no_ext = renderer.truncate_filename_elided("very_long_document_title", 100.0, 24.0, true);
        assert_eq!(long_no_ext.elision.as_deref(), Some("▸"));
        assert_eq!(long_no_ext.suffix, None);
        assert!(long_no_ext.full_text().ends_with('▸'));
    }

    #[test]
    fn test_render_markdown_and_text() {
        let renderer = ScreenRenderer::fallback(AppConfig::default());
        let mut manager = SlotManager::new();

        let md_item = ClipItem::from_text("# Heading 1\n- Item with **bold**\n> Quote\n```rust\nlet x = 1;\n```");
        let txt_item = ClipItem::from_text("Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10\nLine 11\nLine 12");

        manager.push_left(md_item);
        manager.push_right(txt_item);

        let pixmap = renderer.render_pixmap(&manager);
        assert!(pixmap.is_ok());
        let pixmap = pixmap.unwrap();
        assert_eq!(pixmap.width(), SCREEN_WIDTH);
        assert_eq!(pixmap.height(), SCREEN_HEIGHT);
    }

    #[test]
    fn test_render_multi_image() {
        use little_clip_core::ImageThumbnail;
        use std::path::PathBuf;

        let renderer = ScreenRenderer::fallback(AppConfig::default());
        let mut manager = SlotManager::new();

        let thumb1 = ImageThumbnail {
            width: 40,
            height: 40,
            premultiplied_rgba: vec![255; 40 * 40 * 4],
        };
        let thumb2 = thumb1.clone();
        let thumb3 = thumb1.clone();
        let thumb4 = thumb1.clone();

        let mut multi_item = ClipItem::from_text("test");
        multi_item.payload = ClipPayload::MultiImage {
            paths: (1..=8).map(|i| PathBuf::from(format!("img{}.png", i))).collect(),
            total_count: 8,
            preview_indices: vec![0, 1, 6, 7],
            thumbnails: vec![thumb1, thumb2, thumb3, thumb4],
        };
        multi_item.title = "img1.png (+7 pics)".to_string();
        multi_item.subtitle = "8 pictures • 12.5 MB".to_string();
        multi_item.handler_tag = "MULTI-PICTURE".to_string();

        manager.push_left(multi_item);
        let pixmap = renderer.render_pixmap(&manager);
        assert!(pixmap.is_ok());
        let pixmap = pixmap.unwrap();
        assert_eq!(pixmap.width(), SCREEN_WIDTH);
        assert_eq!(pixmap.height(), SCREEN_HEIGHT);
    }

    #[test]
    fn test_render_blank_frame() {
        let renderer = ScreenRenderer::fallback(AppConfig::default());
        let blank_jpeg = renderer.render_blank_frame().expect("render_blank_frame should succeed");
        assert!(!blank_jpeg.is_empty(), "Blank JPEG frame should not be empty");
        // Verify JPEG magic bytes FF D8
        assert_eq!(blank_jpeg[0], 0xFF);
        assert_eq!(blank_jpeg[1], 0xD8);
        // Verify well under the hardware 450 KB limit
        assert!(blank_jpeg.len() < 100_000);
    }
}
