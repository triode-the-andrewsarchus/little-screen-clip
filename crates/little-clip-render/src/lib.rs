//! Ambient display rendering engine for multi-slot clipboard layouts, text previews, waveforms, images, and pipelines.
//!
//! Provides the primary [`ScreenRenderer`] responsible for rasterizing the 1920x462 secondary screen layout
//! to JPEG or direct tiny-skia Pixmap buffers.

use fontdue::{Font, FontSettings};
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use little_clip_core::color::parse_hex_color_rgba_or_white as parse_hex_color;
use little_clip_core::config::AppConfig;
use little_clip_core::palette::AppPalette;
use little_clip_core::{SlotId, SlotManager};
use thiserror::Error;
use tiny_skia::*;
use tracing::debug;

/// Target width in pixels for the physical secondary display.
pub const SCREEN_WIDTH: u32 = 1920;
/// Target height in pixels for the physical secondary display.
pub const SCREEN_HEIGHT: u32 = 462;

/// Returns true if `ch` is a zero-width formatting character or Unicode BOM.
#[inline]
pub fn is_zero_width_or_bom(ch: char) -> bool {
    matches!(
        ch,
        '\u{feff}' // Byte Order Mark / Zero Width No-Break Space
        | '\u{200b}' // Zero Width Space
        | '\u{200c}' // Zero Width Non-Joiner
        | '\u{200d}' // Zero Width Joiner
        | '\u{2060}' // Word Joiner
    )
}

/// Errors that can occur during layout rasterization or image encoding.
#[derive(Debug, Error)]
pub enum RenderError {
    /// Failed to load required TrueType font file.
    #[error("Font load error: {0}")]
    FontLoad(String),
    /// Failed to encode rendered frame to target format (e.g. JPEG).
    #[error("Image encode error: {0}")]
    ImageEncode(String),
}

pub mod markdown;

/// Canonical slot layout geometry computed by `little-clip-render`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotLayoutRect {
    pub slot_id: SlotId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub is_hero: bool,
}

/// Computes pixel-exact slot layout bounding boxes matching hardware display rendering.
pub fn compute_slot_layout(left_priors: usize) -> Vec<SlotLayoutRect> {
    let mut layout = Vec::with_capacity(7);

    // 1. [Undo] (x: 0, w: 240)
    layout.push(SlotLayoutRect {
        slot_id: SlotId::Undo,
        x: 0,
        y: 0,
        width: 240,
        height: SCREEN_HEIGHT,
        is_hero: false,
    });

    // 2. [Left hand] (x: 240, w: 360) - Hero Left
    layout.push(SlotLayoutRect {
        slot_id: SlotId::LeftHand,
        x: 240,
        y: 0,
        width: 360,
        height: SCREEN_HEIGHT,
        is_hero: true,
    });

    // 3. [L prior(1)] (x: 600, w: 240)
    layout.push(SlotLayoutRect {
        slot_id: SlotId::LeftPrior1,
        x: 600,
        y: 0,
        width: 240,
        height: SCREEN_HEIGHT,
        is_hero: false,
    });

    // 4. [L prior(2)] (x: 840, w: 240)
    layout.push(SlotLayoutRect {
        slot_id: SlotId::LeftPrior2,
        x: 840,
        y: 0,
        width: 240,
        height: SCREEN_HEIGHT,
        is_hero: false,
    });

    if left_priors >= 3 {
        // 5. [L prior(3)] (x: 1080, w: 240)
        layout.push(SlotLayoutRect {
            slot_id: SlotId::LeftPrior3,
            x: 1080,
            y: 0,
            width: 240,
            height: SCREEN_HEIGHT,
            is_hero: false,
        });

        // 6. [R prior(1)] (x: 1320, w: 240)
        layout.push(SlotLayoutRect {
            slot_id: SlotId::RightPrior1,
            x: 1320,
            y: 0,
            width: 240,
            height: SCREEN_HEIGHT,
            is_hero: false,
        });
    } else {
        // 5. [R prior(2)] (x: 1080, w: 240)
        layout.push(SlotLayoutRect {
            slot_id: SlotId::RightPrior2,
            x: 1080,
            y: 0,
            width: 240,
            height: SCREEN_HEIGHT,
            is_hero: false,
        });

        // 6. [R prior(1)] (x: 1320, w: 240)
        layout.push(SlotLayoutRect {
            slot_id: SlotId::RightPrior1,
            x: 1320,
            y: 0,
            width: 240,
            height: SCREEN_HEIGHT,
            is_hero: false,
        });
    }

    // 7. [Right hand] (x: 1560, w: 360) - Hero Right
    layout.push(SlotLayoutRect {
        slot_id: SlotId::RightHand,
        x: 1560,
        y: 0,
        width: 360,
        height: SCREEN_HEIGHT,
        is_hero: true,
    });

    layout
}

/// Font style variants supported by the screen renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    /// Standard regular font weight.
    Regular,
    /// Bold font weight for headers and titles.
    Bold,
    /// Italic slant for subtitles and file paths.
    Italic,
    /// Bold italic emphasis.
    BoldItalic,
    /// Monospaced font style.
    Mono,
}

/// Canvas rasterizer for rendering slot cards, thumbnails, waveforms, and code blocks onto a tiny-skia Pixmap.
pub struct ScreenRenderer {
    pub(crate) font_regular: Option<Font>,
    pub(crate) font_bold: Option<Font>,
    pub(crate) font_italic: Option<Font>,
    pub(crate) font_mono: Option<Font>,
    /// Active display configuration, including the shared appearance mode.
    pub config: AppConfig,
    /// Resolved fixed Light/Dark palette.
    pub palette: AppPalette,
}

impl ScreenRenderer {
    /// Creates a new `ScreenRenderer` with standard system fonts loaded.
    pub fn new(config: AppConfig) -> Result<Self, RenderError> {
        let (font_regular, font_bold, font_italic, font_mono) = Self::load_fonts();
        let palette = config.resolve_palette();
        Ok(Self {
            font_regular,
            font_bold,
            font_italic,
            font_mono,
            config,
            palette,
        })
    }

    /// Creates a fallback `ScreenRenderer` without loaded fonts (for headless or font-free testing).
    pub fn fallback(config: AppConfig) -> Self {
        let palette = config.resolve_palette();
        Self {
            font_regular: None,
            font_bold: None,
            font_italic: None,
            font_mono: None,
            config,
            palette,
        }
    }

    /// Creates a `ScreenRenderer` initialized with default configuration.
    pub fn default_renderer() -> Result<Self, RenderError> {
        let config = AppConfig::load();
        Self::new(config)
    }

    /// Updates the active application configuration used for rendering.
    pub fn set_config(&mut self, config: AppConfig) {
        self.palette = config.resolve_palette();
        self.config = config;
    }

    /// Converts a hex string into tiny-skia `Color`.
    #[inline]
    pub fn color_from_hex(&self, hex: &str) -> Color {
        let (r, g, b, a) = parse_hex_color(hex);
        Color::from_rgba8(r, g, b, a)
    }

    /// Converts a hex string into tiny-skia `Color` with custom alpha channel.
    #[inline]
    pub fn color_from_hex_alpha(&self, hex: &str, alpha: u8) -> Color {
        let (r, g, b, _) = parse_hex_color(hex);
        Color::from_rgba8(r, g, b, alpha)
    }

    /// Converts a hex string into tiny-skia `Color`.
    #[inline]
    pub fn hex_to_color(hex: &str) -> Color {
        let (r, g, b, a) = parse_hex_color(hex);
        Color::from_rgba8(r, g, b, a)
    }

    /// Converts a hex string into tiny-skia `Color` with custom alpha.
    #[inline]
    pub fn hex_to_color_alpha(hex: &str, alpha: u8) -> Color {
        let (r, g, b, _) = parse_hex_color(hex);
        Color::from_rgba8(r, g, b, alpha)
    }

    /// Selects the best foreground color to render on `bg` with high contrast (WCAG >= 4.5).
    pub fn contrast_color(&self, bg: Color, preferred_fg: Option<Color>) -> Color {
        let br = (bg.red() * 255.0).round() as u8;
        let bg_c = (bg.green() * 255.0).round() as u8;
        let bb = (bg.blue() * 255.0).round() as u8;
        let bg_rgb = (br, bg_c, bb);

        let pref =
            preferred_fg.unwrap_or_else(|| self.color_from_hex(&self.palette.colors.text_primary));
        let pr = (pref.red() * 255.0).round() as u8;
        let pg = (pref.green() * 255.0).round() as u8;
        let pb = (pref.blue() * 255.0).round() as u8;

        if little_clip_core::palette::contrast_ratio(bg_rgb, (pr, pg, pb)) >= 4.5 {
            return pref;
        }

        let candidates = [
            self.color_from_hex(&self.palette.colors.text_inverse),
            self.color_from_hex(&self.palette.colors.canvas),
            self.color_from_hex(&self.palette.colors.surface_recessed),
            self.color_from_hex(&self.palette.colors.surface_raised),
            self.color_from_hex(&self.palette.colors.surface_panel),
            self.color_from_hex(&self.palette.colors.surface_inverse),
        ];

        for cand in candidates {
            let cr = (cand.red() * 255.0).round() as u8;
            let cg = (cand.green() * 255.0).round() as u8;
            let cb = (cand.blue() * 255.0).round() as u8;
            if little_clip_core::palette::contrast_ratio(bg_rgb, (cr, cg, cb)) >= 4.5 {
                return cand;
            }
        }

        let black_ratio = little_clip_core::palette::contrast_ratio(bg_rgb, (0, 0, 0));
        let white_ratio = little_clip_core::palette::contrast_ratio(bg_rgb, (255, 255, 255));
        if black_ratio >= white_ratio {
            Color::from_rgba8(0, 0, 0, 255)
        } else {
            Color::from_rgba8(255, 255, 255, 255)
        }
    }

    pub fn load_fonts() -> (Option<Font>, Option<Font>, Option<Font>, Option<Font>) {
        let home_font_dir = std::env::var("HOME")
            .ok()
            .map(|h| format!("{}/.local/share/fonts", h));
        let home_fonts_legacy = std::env::var("HOME").ok().map(|h| format!("{}/.fonts", h));

        let mut regular_candidates = vec![
            "/usr/share/fonts/TTF/DejaVuSans.ttf".to_string(),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".to_string(),
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf".to_string(),
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf".to_string(),
            "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf".to_string(),
            "/usr/share/fonts/noto/NotoSans-Regular.ttf".to_string(),
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf".to_string(),
            "/usr/share/fonts/gnu-free/FreeSans.ttf".to_string(),
            "/usr/share/fonts/truetype/freefont/FreeSans.ttf".to_string(),
        ];
        let mut bold_candidates = vec![
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf".to_string(),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".to_string(),
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf".to_string(),
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf".to_string(),
            "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf".to_string(),
            "/usr/share/fonts/noto/NotoSans-Bold.ttf".to_string(),
            "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf".to_string(),
            "/usr/share/fonts/gnu-free/FreeSansBold.ttf".to_string(),
            "/usr/share/fonts/truetype/freefont/FreeSansBold.ttf".to_string(),
        ];
        let mut italic_candidates = vec![
            "/usr/share/fonts/TTF/DejaVuSans-Oblique.ttf".to_string(),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf".to_string(),
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Oblique.ttf".to_string(),
            "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf".to_string(),
            "/usr/share/fonts/liberation-sans/LiberationSans-Italic.ttf".to_string(),
            "/usr/share/fonts/noto/NotoSans-Italic.ttf".to_string(),
            "/usr/share/fonts/truetype/noto/NotoSans-Italic.ttf".to_string(),
            "/usr/share/fonts/gnu-free/FreeSansOblique.ttf".to_string(),
            "/usr/share/fonts/truetype/freefont/FreeSansOblique.ttf".to_string(),
        ];
        let mut mono_candidates = vec![
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf".to_string(),
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf".to_string(),
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSansMono.ttf".to_string(),
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf".to_string(),
            "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf".to_string(),
            "/usr/share/fonts/noto/NotoSansMono-Regular.ttf".to_string(),
            "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf".to_string(),
            "/usr/share/fonts/gnu-free/FreeMono.ttf".to_string(),
            "/usr/share/fonts/truetype/freefont/FreeMono.ttf".to_string(),
        ];

        if let Some(ref hfd) = home_font_dir {
            regular_candidates.push(format!("{}/DejaVuSans.ttf", hfd));
            bold_candidates.push(format!("{}/DejaVuSans-Bold.ttf", hfd));
            italic_candidates.push(format!("{}/DejaVuSans-Oblique.ttf", hfd));
            mono_candidates.push(format!("{}/DejaVuSansMono.ttf", hfd));
        }
        if let Some(ref hfl) = home_fonts_legacy {
            regular_candidates.push(format!("{}/DejaVuSans.ttf", hfl));
            bold_candidates.push(format!("{}/DejaVuSans-Bold.ttf", hfl));
            italic_candidates.push(format!("{}/DejaVuSans-Oblique.ttf", hfl));
            mono_candidates.push(format!("{}/DejaVuSansMono.ttf", hfl));
        }

        let try_load = |paths: &[String]| -> Option<Font> {
            paths.iter().find_map(|path| {
                std::fs::read(path)
                    .ok()
                    .and_then(|data| Font::from_bytes(data, FontSettings::default()).ok())
            })
        };

        let regular = try_load(&regular_candidates);
        let bold = try_load(&bold_candidates).or_else(|| regular.clone());
        let italic = try_load(&italic_candidates).or_else(|| regular.clone());
        let mono = try_load(&mono_candidates).or_else(|| regular.clone());

        (regular, bold, italic, mono)
    }
}

mod cards;
pub mod primitives;

pub use fontdue;
pub use primitives::ElidedText;
pub use tiny_skia;

impl ScreenRenderer {
    /// Renders the complete 1920x462 hotbar layout into a tiny-skia Pixmap.
    /// Order: \[Undo] \[Left hand] \[L prior(1)] \[L prior(2)] \[R prior(2)] \[R prior(1)] \[Right hand]
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] if allocating the canvas buffer fails or if rendering
    /// any slot element encounters an unrecoverable graphics error.
    pub fn render_pixmap(&self, manager: &SlotManager) -> Result<Pixmap, RenderError> {
        let mut pixmap = Pixmap::new(SCREEN_WIDTH, SCREEN_HEIGHT)
            .ok_or_else(|| RenderError::ImageEncode("Failed to allocate pixmap".to_string()))?;

        // Background: fixed canvas color for the shared appearance mode.
        let (bg_r, bg_g, bg_b, bg_a) = parse_hex_color(&self.palette.colors.canvas);
        pixmap.fill(Color::from_rgba8(bg_r, bg_g, bg_b, bg_a));

        // Render slots according to canonical layout
        for slot in compute_slot_layout(self.config.slots.left_priors) {
            self.draw_slot_card(
                &mut pixmap,
                manager,
                slot.slot_id,
                slot.x,
                slot.y,
                slot.width,
                slot.height,
                slot.is_hero,
            );
        }

        // Draw Debug Handler badge in bottom left of screen if enabled
        if self.config.display.show_debug_handler {
            let l_handler = manager
                .left_hand()
                .map(|i| i.handler_tag.as_str())
                .unwrap_or("EMPTY");
            let r_handler = manager
                .right_hand()
                .map(|i| i.handler_tag.as_str())
                .unwrap_or("EMPTY");
            let handler_text = format!("LH: [{}] | RH: [{}]", l_handler, r_handler);

            let badge_x = 18.0;
            let badge_y = SCREEN_HEIGHT as f32 - 42.0;
            let badge_w = 460.0;
            let badge_h = 32.0;
            if let Some(br) = Rect::from_xywh(badge_x, badge_y, badge_w, badge_h) {
                let (br_r, br_g, br_b, br_a) =
                    parse_hex_color(&self.palette.colors.surface_inverse);
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(br_r, br_g, br_b, br_a));
                pixmap.fill_rect(br, &paint, Transform::identity(), None);

                let (str_r, str_g, str_b, str_a) =
                    parse_hex_color(&self.palette.colors.text_inverse);
                let mut stroke_paint = Paint::default();
                stroke_paint.set_color(Color::from_rgba8(
                    str_r,
                    str_g,
                    str_b,
                    (str_a as f32 * 0.8) as u8,
                ));
                let mut stroke = Stroke::default();
                stroke.width = 1.5;
                let path = PathBuilder::from_rect(br);
                pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);

                self.draw_text(
                    &mut pixmap,
                    &handler_text,
                    badge_x + 14.0,
                    badge_y + 22.0,
                    14.0,
                    Color::from_rgba8(str_r, str_g, str_b, str_a),
                    true,
                );
            }
        }

        Ok(pixmap)
    }

    /// Renders the complete 1920x462 hotbar layout into a compressed JPEG.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] if rendering the pixmap fails or if JPEG compression fails.
    pub fn render_frame(&self, manager: &SlotManager) -> Result<Vec<u8>, RenderError> {
        let pixmap = self.render_pixmap(manager)?;
        self.encode_jpeg(&pixmap)
    }

    /// Renders a completely black 1920x462 frame (all pixels turned off/dark)
    /// into a compressed JPEG for screen-saver / lock mode.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError`] if allocating the canvas buffer fails or if JPEG compression fails.
    pub fn render_blank_frame(&self) -> Result<Vec<u8>, RenderError> {
        let mut pixmap = Pixmap::new(SCREEN_WIDTH, SCREEN_HEIGHT).ok_or_else(|| {
            RenderError::ImageEncode("Failed to allocate blank pixmap".to_string())
        })?;
        pixmap.fill(Color::from_rgba8(0, 0, 0, 255));
        self.encode_jpeg(&pixmap)
    }

    /// Returns the border color hex string configured for the given slot.
    pub fn slot_border_hex(&self, slot_id: SlotId) -> &str {
        match slot_id {
            SlotId::Undo => &self.palette.undo_border,
            SlotId::LeftHand => &self.palette.left_hand_border,
            SlotId::LeftPrior1 => &self.palette.left_prior1_border,
            SlotId::LeftPrior2 => &self.palette.left_prior2_border,
            SlotId::LeftPrior3 => &self.palette.left_prior3_border,
            SlotId::RightPrior2 => &self.palette.right_prior2_border,
            SlotId::RightPrior1 => &self.palette.right_prior1_border,
            SlotId::RightHand => &self.palette.right_hand_border,
        }
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
    use little_clip_core::config::WorkbenchAppearance;
    use little_clip_core::spreadsheet;
    use little_clip_core::{ClipItem, ClipPayload};

    #[test]
    fn test_tiny_skia_image_draw() {
        let mut pixmap = Pixmap::new(200, 200).unwrap();
        let rgba_data = vec![255u8; 50 * 50 * 4];
        let sub = PixmapRef::from_bytes(&rgba_data, 50, 50).unwrap();
        pixmap.draw_pixmap(
            10,
            10,
            sub,
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
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
        let long_ext =
            renderer.truncate_filename_elided("very_long_filename.jpg", 100.0, 24.0, true);
        assert_eq!(long_ext.suffix.as_deref(), Some(".jpg"));
        assert_eq!(long_ext.elision.as_deref(), Some("▸"));
        assert!(long_ext.full_text().ends_with(".jpg"));
        // 3. Long text without extension adds '▸' at the end
        let long_no_ext =
            renderer.truncate_filename_elided("very_long_document_title", 100.0, 24.0, true);
        assert_eq!(long_no_ext.elision.as_deref(), Some("▸"));
        assert_eq!(long_no_ext.suffix, None);
        assert!(long_no_ext.full_text().ends_with('▸'));
    }

    #[test]
    fn test_render_markdown_and_text() {
        let renderer = ScreenRenderer::fallback(AppConfig::default());
        let mut manager = SlotManager::new();

        let md_item = ClipItem::from_text(
            "# Heading 1\n- Item with **bold**\n> Quote\n```rust\nlet x = 1;\n```",
        );
        let txt_item = ClipItem::from_text(
            "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10\nLine 11\nLine 12",
        );

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
            paths: (1..=8)
                .map(|i| PathBuf::from(format!("img{}.png", i)))
                .collect(),
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
        let blank_jpeg = renderer
            .render_blank_frame()
            .expect("render_blank_frame should succeed");
        assert!(
            !blank_jpeg.is_empty(),
            "Blank JPEG frame should not be empty"
        );
        // Verify JPEG magic bytes FF D8
        assert_eq!(blank_jpeg[0], 0xFF);
        assert_eq!(blank_jpeg[1], 0xD8);
        // Verify well under the hardware 450 KB limit
        assert!(blank_jpeg.len() < 100_000);
    }

    #[test]
    fn switching_appearance_changes_the_little_screen_frame() {
        let mut config = AppConfig::default();
        config.display.show_debug_handler = false;
        let mut renderer = ScreenRenderer::fallback(config.clone());
        let manager = SlotManager::default();
        let dark = renderer.render_pixmap(&manager).unwrap();

        config.workbench.appearance = WorkbenchAppearance::Light;
        renderer.set_config(config);
        let light = renderer.render_pixmap(&manager).unwrap();
        assert_ne!(dark.data(), light.data());
        assert_eq!(renderer.palette.colors.canvas, "#f4f2ea");
    }

    #[test]
    fn test_render_spreadsheet_and_prose() {
        let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
        let mut manager = SlotManager::new();

        // 1. Raw CSV text
        let csv_text =
            "Item,Quantity,Price\nWidget A,12,$4.50\nWidget B,8,$19.99\nWidget C,3,$99.00";
        manager.push_left(ClipItem::from_text(csv_text));

        let pixmap = renderer.render_pixmap(&manager).unwrap();
        assert_eq!(pixmap.width(), SCREEN_WIDTH);
        assert_eq!(pixmap.height(), SCREEN_HEIGHT);

        // 2. CSV File in FileBatch
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_csv = temp_dir.path().join("test_sample.csv");
        std::fs::write(&temp_csv, csv_text).unwrap();
        let mut file_item = ClipItem::from_text("test_sample.csv");
        file_item.payload = ClipPayload::FileBatch(vec![temp_csv.clone()]);
        manager.push_left(file_item);

        let pixmap_file = renderer.render_pixmap(&manager).unwrap();
        assert_eq!(pixmap_file.width(), SCREEN_WIDTH);
        assert_eq!(pixmap_file.height(), SCREEN_HEIGHT);

        // 3. Prose text with commas
        let prose = "Hey there, how is it going? I am testing out some plain text on my clipboard, to make sure it doesn't get treated as a spreadsheet.";
        let prose_detect = spreadsheet::detect_csv_text(prose);
        assert!(
            prose_detect.is_none(),
            "Prose should not be detected as CSV spreadsheet"
        );
    }

    #[test]
    fn test_render_stl_card() {
        let temp_dir = tempfile::tempdir().unwrap();
        let stl_path = temp_dir.path().join("triangle.stl");
        std::fs::write(
            &stl_path,
            "solid triangle\n  facet normal 0 0 1\n    outer loop\n      vertex 0 0 0\n      vertex 1 0 0\n      vertex 0 1 0\n    endloop\n  endfacet\nendsolid triangle\n",
        )
        .unwrap();

        let renderer = ScreenRenderer::new(AppConfig::default()).unwrap();
        let mut manager = SlotManager::new();
        let mut item = ClipItem::from_text("triangle.stl");
        item.payload = ClipPayload::FileBatch(vec![stl_path]);
        item.title = "triangle.stl".to_string();
        item.subtitle = "STL 3D Model".to_string();
        manager.push_left(item);

        let pixmap = renderer.render_pixmap(&manager).unwrap();
        assert_eq!(pixmap.width(), SCREEN_WIDTH);
        assert_eq!(pixmap.height(), SCREEN_HEIGHT);
    }

    #[test]
    fn test_bom_and_zero_width_rendering() {
        assert!(is_zero_width_or_bom('\u{feff}'));
        assert!(is_zero_width_or_bom('\u{200b}'));
        assert!(!is_zero_width_or_bom('a'));
        assert!(!is_zero_width_or_bom(' '));

        let renderer = ScreenRenderer::fallback(AppConfig::default());

        let w_with_bom =
            renderer.measure_text_width_styled("\u{feff}1. Do not talk", 15.0, FontStyle::Mono);
        let w_without_bom =
            renderer.measure_text_width_styled("1. Do not talk", 15.0, FontStyle::Mono);
        assert_eq!(
            w_with_bom, w_without_bom,
            "BOM character must not add to measured width"
        );

        let mut pix1 = Pixmap::new(200, 50).unwrap();
        let mut pix2 = Pixmap::new(200, 50).unwrap();
        let end_x1 = renderer.draw_text_styled_clipped(
            &mut pix1,
            "\u{feff}1. Do not talk",
            10.0,
            10.0,
            15.0,
            Color::WHITE,
            FontStyle::Mono,
            200.0,
        );
        let end_x2 = renderer.draw_text_styled_clipped(
            &mut pix2,
            "1. Do not talk",
            10.0,
            10.0,
            15.0,
            Color::WHITE,
            FontStyle::Mono,
            200.0,
        );
        assert_eq!(
            end_x1, end_x2,
            "BOM character must not advance drawing cursor"
        );
        assert_eq!(
            pix1.data(),
            pix2.data(),
            "Rendered pixels with BOM must match without BOM"
        );
    }
}
