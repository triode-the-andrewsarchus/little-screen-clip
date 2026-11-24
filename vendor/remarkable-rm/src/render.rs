//! Rasterize a parsed [`Page`] to a PNG image.
//!
//! Strokes, typed text and glyph highlights are drawn over an optional background
//! raster — a paper template via
//! [`render_page_with_background`] and the `templates` feature, or plain white
//! when none is supplied. Pen colors map to the fixed palette in spec §7.3; the
//! highlighter is alpha-blended so overlapping highlight strokes don't fully
//! obscure ink.

use tiny_skia::{
    Color, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, PixmapRef,
    Stroke, Transform,
};

use crate::document::{Layer, Page};
use crate::scene::line::{Line, Pen, PenColor};

/// Default canvas for a reMarkable 2 page (firmware 3.x).
pub const DEFAULT_WIDTH: u32 = 1404;
pub const DEFAULT_HEIGHT: u32 = 1872;

/// Highlighter alpha (0x40 ≈ 0.25) — matches `lines-are-rusty`. Higher
/// obscures underlying ink; lower stops reading as "highlight."
const HIGHLIGHTER_ALPHA_U8: u8 = 0x40;

#[derive(Debug, Clone, Copy)]
pub struct RenderOptions {
    /// Output width; sets a uniform scale relative to the native page width.
    pub width: u32,
    /// Output viewport height. A taller canvas reveals extended content without
    /// stretching strokes; a shorter canvas crops it.
    pub height: u32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }
}

impl RenderOptions {
    /// Pick the page width from `paper_size`, falling back to the firmware
    /// default. Extend the nominal height when visible ink or text runs below
    /// the screen, retaining a 50-pixel bottom margin on expanded pages.
    #[must_use]
    pub fn for_page(page: &Page) -> Self {
        let mut options = match page.paper_size {
            Some((w, h)) => Self {
                width: w,
                height: h,
            },
            None => Self::default(),
        };
        let text_bottom = page
            .text
            .as_ref()
            .and_then(|text| text.layout().ok())
            .map_or(0.0, |layout| layout.bottom);
        let bottom = page
            .layers
            .iter()
            .filter(|layer| layer.visible)
            .flat_map(|layer| &layer.lines)
            .flat_map(|line| &line.points)
            .map(|point| point.y)
            .filter(|y| y.is_finite())
            .fold(text_bottom, f32::max);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        if bottom > options.height as f32 {
            options.height = (bottom.ceil() as u32).saturating_add(50);
        }
        options
    }
}

/// A pre-decoded background raster to composite beneath the strokes — e.g. a
/// notebook's paper template (lined/grid/dotted).
///
/// Pixels are **premultiplied** RGBA8 (`tiny-skia`'s native layout), row-major,
/// exactly `width * height * 4` bytes. Build one with the decoders in
/// [`crate::templates`] (behind the `templates` feature).
#[derive(Debug, Clone)]
pub struct BackgroundImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl BackgroundImage {
    /// Copy a rasterized pixmap's premultiplied pixels into a `BackgroundImage`.
    /// Used by the template decoders, which render into a `Pixmap`.
    #[cfg(feature = "templates")]
    pub(crate) fn from_pixmap(pixmap: &Pixmap) -> Self {
        Self {
            width: pixmap.width(),
            height: pixmap.height(),
            rgba: pixmap.data().to_vec(),
        }
    }
}

/// Rasterize a parsed page into PNG-encoded bytes (strokes on white).
///
/// # Errors
/// Returns an error if the canvas dimensions are zero or exceed
/// `tiny-skia`'s allocation limit, or if the PNG encoder fails.
pub fn render_page(page: &Page, opts: RenderOptions) -> Result<Vec<u8>, RenderError> {
    render_page_with_background(page, opts, None)
}

/// Rasterize a parsed page, optionally compositing `background` beneath the
/// strokes. With `background = None` this is identical to [`render_page`].
///
/// # Errors
/// Returns an error if the canvas dimensions are zero or exceed
/// `tiny-skia`'s allocation limit, or if the PNG encoder fails.
pub fn render_page_with_background(
    page: &Page,
    opts: RenderOptions,
    background: Option<&BackgroundImage>,
) -> Result<Vec<u8>, RenderError> {
    let mut pixmap = Pixmap::new(opts.width, opts.height).ok_or(RenderError::InvalidCanvas {
        width: opts.width,
        height: opts.height,
    })?;
    pixmap.fill(Color::WHITE);

    if let Some(bg) = background {
        draw_background(&mut pixmap, bg, opts);
    }

    let transform = page_to_canvas_transform(page, opts);

    for layer in &page.layers {
        if !layer.visible {
            continue;
        }
        draw_layer(&mut pixmap, layer, transform);
    }

    crate::render_text::draw_text(page, &mut pixmap, opts)?;

    pixmap
        .encode_png()
        .map_err(|e| RenderError::Encode(e.to_string()))
}

/// Composite a background raster to fill the canvas, beneath the strokes.
///
/// Templates are authored at the device-canvas aspect, so the scale here is
/// normally 1:1; the explicit scale guards against a cached raster being
/// reused at a different output size (e.g. a `--width` override).
#[allow(clippy::cast_precision_loss)]
fn draw_background(pixmap: &mut Pixmap, bg: &BackgroundImage, opts: RenderOptions) {
    let Some(bg_ref) = PixmapRef::from_bytes(&bg.rgba, bg.width, bg.height) else {
        return;
    };
    let sx = opts.width as f32 / bg.width.max(1) as f32;
    let sy = opts.height as f32 / bg.height.max(1) as f32;
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    pixmap.draw_pixmap(0, 0, bg_ref, &paint, Transform::from_scale(sx, sy), None);
}

/// reMarkable stroke coordinates use a top-center origin: x = 0 is the page
/// midline, y = 0 is the top edge. Map that native page space onto the
/// requested output canvas with one tiny-skia transform so geometry and
/// effective stroke widths scale together.
#[allow(clippy::cast_precision_loss)]
fn page_to_canvas_transform(page: &Page, opts: RenderOptions) -> Transform {
    let source = RenderOptions::for_page(page);
    let scale_x = opts.width as f32 / source.width as f32;
    // Width determines physical scale. A taller output is an extended viewport,
    // not a request to stretch the tablet's nominal screen height.
    let scale_y = scale_x;
    Transform::from_row(scale_x, 0.0, 0.0, scale_y, opts.width as f32 / 2.0, 0.0)
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid canvas dimensions: {width}x{height}")]
    InvalidCanvas { width: u32, height: u32 },
    #[error("PNG encoding failed: {0}")]
    Encode(String),
    #[error("text rendering failed: {0}")]
    Text(String),
}

#[allow(clippy::cast_possible_truncation)]
fn draw_layer(pixmap: &mut Pixmap, layer: &Layer, transform: Transform) {
    for glyph in &layer.glyphs {
        let mut paint = Paint {
            blend_mode: tiny_skia::BlendMode::Multiply,
            ..Paint::default()
        };
        paint.set_color(glyph_color(glyph));
        for rect in &glyph.rectangles {
            if let Some(rect) = tiny_skia::Rect::from_xywh(
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
            ) {
                pixmap.fill_rect(rect, &paint, transform, None);
            }
        }
    }
    for line in &layer.lines {
        if is_eraser(line.tool) {
            continue;
        }
        draw_line(pixmap, line, transform);
    }
}

/// Returns the color used to multiply a snapped text highlight over the page.
pub fn glyph_color(glyph: &crate::scene::glyph::GlyphRange) -> Color {
    glyph.color_rgba.map_or_else(
        || pen_color_to_rgba(glyph.color),
        |[r, g, b, a]| Color::from_rgba8(r, g, b, a),
    )
}

/// Returns the prepared text geometry as SVG paths with the bundled typefaces.
///
/// # Errors
/// Returns a text layout or SVG normalization error.
pub fn text_paths(page: &Page) -> Result<Option<String>, RenderError> {
    Ok(crate::render_text::prepare(page)?
        .map(|tree| tree.to_string(&resvg::usvg::WriteOptions::default())))
}

struct SegmentInk {
    width: f32,
    opacity: f32,
    fade: f32,
}

#[allow(clippy::cast_possible_truncation)]
fn segment_ink(line: &Line, point: &crate::Point, last_width: f32) -> SegmentInk {
    let pressure = f32::from(point.pressure) / 255.0;
    let speed = f32::from(point.speed) / 4.0;
    let tilt = f32::from(point.direction) * std::f32::consts::TAU / 255.0;
    let base_width = line.thickness_scale as f32;
    let raw_width = f32::from(point.width) / 4.0;
    let (width, opacity, fade) = match line.tool {
        Pen::PencilV1 | Pen::PencilV2 => (
            (0.7 * ((0.8 * base_width + 0.5 * pressure) * raw_width
                - 0.25 * tilt.powf(1.8)
                - 0.6 * speed / 50.0))
                .min(base_width * 10.0),
            (pressure - 0.1 * speed / 35.0 - 0.1).clamp(0.0, 1.0),
            0.0,
        ),
        Pen::BallpointV1 | Pen::BallpointV2 => (
            0.5 + pressure + raw_width - 0.5 * speed / 50.0,
            1.0,
            (1.0 - (-0.1 * speed / 35.0 + 1.2 * pressure + 0.5).clamp(0.0, 1.0)).min(60.0 / 255.0),
        ),
        Pen::PaintbrushV1 | Pen::PaintbrushV2 => (
            0.5 * ((1.0 + 1.4 * pressure) * raw_width - 0.5 * tilt - speed / 50.0),
            1.0,
            1.0 - ((pressure.powf(1.5) - 0.2 * speed / 50.0) * 1.5).clamp(0.0, 1.0),
        ),
        Pen::Calligraphy => (
            0.9 * ((1.0 + pressure) * raw_width - 0.3 * tilt) + 0.1 * last_width,
            1.0,
            0.0,
        ),
        Pen::MarkerV1 | Pen::MarkerV2 => {
            (0.9 * (raw_width - 0.4 * tilt) + 0.1 * last_width, 1.0, 0.0)
        }
        Pen::MechanicalPencilV1 | Pen::MechanicalPencilV2 => (base_width * base_width, 0.7, 0.0),
        Pen::FinelinerV1 | Pen::FinelinerV2 => (base_width * 1.8, 1.0, 0.0),
        Pen::Shader => (raw_width, 0.25, 0.0),
        _ => (raw_width, 1.0, 0.0),
    };
    SegmentInk {
        width,
        opacity,
        fade,
    }
}

fn draw_line(pixmap: &mut Pixmap, line: &Line, transform: Transform) {
    visit_pen_segments(line, |points, color, stroke, blend_mode| {
        let mut path = PathBuilder::new();
        path.move_to(points[0].x, points[0].y);
        for point in &points[1..] {
            path.line_to(point.x, point.y);
        }
        if points.len() == 1 {
            path.line_to(points[0].x, points[0].y);
        }
        if let Some(path) = path.finish() {
            let mut paint = Paint {
                anti_alias: true,
                blend_mode,
                ..Paint::default()
            };
            paint.set_color(color);
            pixmap.stroke_path(&path, &paint, &stroke, transform, None);
        }
    });
}

/// Visits the renderer's styled segments in native page coordinates and paint order.
///
/// Empirical pressure, tilt and speed models come from rmc / remarkable-rm
/// (see `THIRD_PARTY.md`). Point width is encoded in quarter-pixels.
#[allow(clippy::cast_possible_truncation)]
pub fn visit_pen_segments(
    line: &Line,
    mut visit: impl FnMut(&[crate::Point], Color, Stroke, tiny_skia::BlendMode),
) {
    let segment_len = match line.tool {
        Pen::PencilV1
        | Pen::PencilV2
        | Pen::PaintbrushV1
        | Pen::PaintbrushV2
        | Pen::Calligraphy => 2,
        Pen::BallpointV1 | Pen::BallpointV2 => 5,
        Pen::MarkerV1 | Pen::MarkerV2 => 3,
        _ => line.points.len().max(1),
    };
    let mut last_width = 0.0;
    for start in (0..line.points.len()).step_by(segment_len) {
        let point = line.points[start];
        let SegmentInk {
            width,
            mut opacity,
            fade,
        } = segment_ink(line, &point, last_width);
        last_width = width;
        let width = width.max(0.0);
        if width <= 0.0 || !width.is_finite() {
            continue;
        }
        let points =
            &line.points[start.saturating_sub(1)..(start + segment_len).min(line.points.len())];
        let base = if let Some([r, g, b, a]) = line.color_rgba {
            if a < 255 {
                opacity = f32::from(a) / 255.0;
            }
            Color::from_rgba8(r, g, b, 255)
        } else {
            pen_color_to_rgba(line.color)
        };
        let color = Color::from_rgba(
            base.red() + (1.0 - base.red()) * fade,
            base.green() + (1.0 - base.green()) * fade,
            base.blue() + (1.0 - base.blue()) * fade,
            opacity * base.alpha(),
        )
        .unwrap();
        if color.alpha() == 0.0 {
            continue;
        }
        let highlighter = matches!(line.tool, Pen::HighlighterV1 | Pen::HighlighterV2);
        let blend = if highlighter || line.tool == Pen::Shader {
            tiny_skia::BlendMode::Multiply
        } else {
            tiny_skia::BlendMode::SourceOver
        };
        visit(
            points,
            color,
            Stroke {
                width,
                line_cap: if highlighter {
                    LineCap::Square
                } else {
                    LineCap::Round
                },
                line_join: LineJoin::Round,
                ..Stroke::default()
            },
            blend,
        );
    }
}

fn is_eraser(pen: Pen) -> bool {
    matches!(pen, Pen::Eraser | Pen::EraserAreaSelect)
}

/// Map the tablet's pen-color enum to an RGBA color.
///
/// `Highlight` is rendered semi-transparent so overlapping highlighter
/// strokes layer like a real highlighter; every other color is opaque.
fn pen_color_to_rgba(color: PenColor) -> Color {
    match color {
        PenColor::Black => Color::from_rgba8(0, 0, 0, 0xFF),
        PenColor::Gray => Color::from_rgba8(0x80, 0x80, 0x80, 0xFF),
        PenColor::White => Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xFF),
        PenColor::Yellow => Color::from_rgba8(0xFF, 0xC4, 0x00, 0xFF),
        PenColor::Green => Color::from_rgba8(0x00, 0x99, 0x33, 0xFF),
        PenColor::Pink => Color::from_rgba8(0xFF, 0x40, 0x80, 0xFF),
        PenColor::Blue => Color::from_rgba8(0x14, 0x4F, 0xC4, 0xFF),
        PenColor::Red => Color::from_rgba8(0xCC, 0x33, 0x00, 0xFF),
        PenColor::GrayOverlap => Color::from_rgba8(0x60, 0x60, 0x60, 0xFF),
        PenColor::Highlight => Color::from_rgba8(0xFF, 0xF2, 0x00, HIGHLIGHTER_ALPHA_U8),
        PenColor::GreenV2 => Color::from_rgba8(0x35, 0xB5, 0x57, 0xFF),
        PenColor::Cyan => Color::from_rgba8(0x00, 0xB7, 0xC4, 0xFF),
        PenColor::Magenta => Color::from_rgba8(0xC4, 0x35, 0xB5, 0xFF),
        PenColor::YellowV2 => Color::from_rgba8(0xE6, 0xC1, 0x00, 0xFF),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt::CrdtId;
    use crate::scene::line::Point;

    fn line_with_points(points: Vec<Point>, color: PenColor, tool: Pen) -> Line {
        Line {
            tool,
            color,
            color_rgba: None,
            thickness_scale: 1.0,
            starting_length: 0.0,
            points,
            timestamp: CrdtId { author: 1, seq: 1 },
            move_id: None,
        }
    }

    fn point(x: f32, y: f32) -> Point {
        Point {
            x,
            y,
            speed: 0,
            width: 1024,
            direction: 0,
            pressure: 200,
        }
    }

    fn page_with_lines(lines: Vec<Line>, paper_size: Option<(u32, u32)>) -> Page {
        Page {
            layers: vec![Layer {
                glyphs: Vec::new(),
                node_id: CrdtId { author: 1, seq: 2 },
                name: "L1".into(),
                visible: true,
                lines,
            }],
            text: None,
            paper_size,
        }
    }

    fn count_non_white_pixels_in_columns(pixmap: &Pixmap, start_x: u32, end_x: u32) -> usize {
        let width = usize::try_from(pixmap.width()).unwrap();
        let start_x = usize::try_from(start_x).unwrap();
        let end_x = usize::try_from(end_x).unwrap();

        pixmap
            .pixels()
            .iter()
            .enumerate()
            .filter(|(idx, pixel)| {
                let x = idx % width;
                (start_x..end_x).contains(&x)
                    && (pixel.red() != 0xFF || pixel.green() != 0xFF || pixel.blue() != 0xFF)
            })
            .count()
    }

    #[test]
    fn empty_page_renders_white_canvas() {
        let page = Page::default();
        let png = render_page(&page, RenderOptions::default()).unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();
        assert_eq!(decoded.width(), DEFAULT_WIDTH);
        assert_eq!(decoded.height(), DEFAULT_HEIGHT);
        // Every pixel is white.
        let any_non_white = decoded.pixels().iter().any(|p| {
            !(p.red() == 0xFF && p.green() == 0xFF && p.blue() == 0xFF && p.alpha() == 0xFF)
        });
        assert!(!any_non_white, "empty page should be uniformly white");
    }

    #[test]
    fn black_stroke_paints_non_white_pixels() {
        // Stroke coords are top-center; pick values that remain on canvas
        // after the transform applied by `render_page`.
        let line = line_with_points(
            vec![point(-300.0, 100.0), point(300.0, 800.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let page = page_with_lines(vec![line], None);
        let png = render_page(&page, RenderOptions::default()).unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();
        let dark_pixel_count = decoded
            .pixels()
            .iter()
            .filter(|p| p.red() < 0x40 && p.green() < 0x40 && p.blue() < 0x40)
            .count();
        assert!(dark_pixel_count > 100, "expected stroke ink on canvas");
    }

    #[test]
    fn background_fills_canvas_beneath_strokes() {
        // Opaque red fills every pixel (premultiplied == straight when a=0xFF).
        let (w, h) = (40u32, 40u32);
        let rgba: Vec<u8> = std::iter::repeat_n([0xFF, 0x00, 0x00, 0xFF], (w * h) as usize)
            .flatten()
            .collect();
        let background = BackgroundImage {
            width: w,
            height: h,
            rgba,
        };
        // One black stroke kept away from the (0,0) corner.
        let line = line_with_points(
            vec![point(-5.0, 10.0), point(10.0, 30.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let page = page_with_lines(vec![line], Some((w, h)));
        let png = render_page_with_background(
            &page,
            RenderOptions {
                width: w,
                height: h,
            },
            Some(&background),
        )
        .unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();

        // The corner (no ink) shows through as the background color, not white.
        let corner = decoded.pixel(0, 0).unwrap();
        assert_eq!(
            (corner.red(), corner.green(), corner.blue()),
            (0xFF, 0x00, 0x00),
            "background should replace the white fill"
        );
        // The stroke still paints ink on top of the background.
        let any_ink = decoded
            .pixels()
            .iter()
            .any(|p| p.red() < 0x40 && p.green() < 0x40 && p.blue() < 0x40);
        assert!(any_ink, "stroke should composite over the background");
    }

    #[test]
    fn no_background_matches_plain_render() {
        let line = line_with_points(
            vec![point(-100.0, 50.0), point(100.0, 400.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let page = page_with_lines(vec![line], None);
        let plain = render_page(&page, RenderOptions::default()).unwrap();
        let via_bg = render_page_with_background(&page, RenderOptions::default(), None).unwrap();
        assert_eq!(plain, via_bg, "None background must equal render_page");
    }

    #[test]
    fn invisible_layer_is_skipped() {
        let line = line_with_points(
            vec![point(-300.0, 200.0), point(300.0, 800.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let page = Page {
            layers: vec![Layer {
                glyphs: Vec::new(),
                node_id: CrdtId { author: 1, seq: 2 },
                name: "hidden".into(),
                visible: false,
                lines: vec![line],
            }],
            text: None,
            paper_size: None,
        };
        let png = render_page(&page, RenderOptions::default()).unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();
        let any_ink = decoded
            .pixels()
            .iter()
            .any(|p| p.red() != 0xFF || p.green() != 0xFF || p.blue() != 0xFF);
        assert!(!any_ink, "invisible layer should not paint");
    }

    #[test]
    fn eraser_strokes_are_skipped() {
        let line = line_with_points(
            vec![point(-400.0, 50.0), point(400.0, 900.0)],
            PenColor::Black,
            Pen::Eraser,
        );
        let page = page_with_lines(vec![line], None);
        let png = render_page(&page, RenderOptions::default()).unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();
        let any_ink = decoded
            .pixels()
            .iter()
            .any(|p| p.red() != 0xFF || p.green() != 0xFF || p.blue() != 0xFF);
        assert!(!any_ink, "Eraser strokes must not paint");
    }

    #[test]
    fn for_page_uses_paper_size_when_present() {
        let page = Page {
            paper_size: Some((800, 600)),
            ..Page::default()
        };
        let opts = RenderOptions::for_page(&page);
        assert_eq!((opts.width, opts.height), (800, 600));
    }

    #[test]
    fn extended_ink_grows_canvas_without_stretching_coordinates() {
        let page = page_with_lines(
            vec![line_with_points(
                vec![point(-20.0, 2100.0), point(20.0, 2100.0)],
                PenColor::Black,
                Pen::FinelinerV2,
            )],
            Some((1404, 1872)),
        );
        let opts = RenderOptions::for_page(&page);
        assert_eq!((opts.width, opts.height), (1404, 2150));
        let image = Pixmap::decode_png(&render_page(&page, opts).unwrap()).unwrap();
        assert!(image.pixels()[2100 * 1404 + 702].red() < 240);
    }

    #[test]
    fn tall_output_preserves_physical_scale_in_both_axes() {
        let page = page_with_lines(
            vec![line_with_points(
                vec![point(-20.0, 100.0), point(20.0, 100.0)],
                PenColor::Black,
                Pen::FinelinerV2,
            )],
            Some((1404, 1872)),
        );
        let image = Pixmap::decode_png(
            &render_page(
                &page,
                RenderOptions {
                    width: 702,
                    height: 1500,
                },
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            image.pixels()[50 * 702 + 351].red() < 240,
            "half-width output must place native y=100 at y=50 regardless of viewport height"
        );
    }

    #[test]
    fn for_page_falls_back_to_default() {
        let page = Page::default();
        let opts = RenderOptions::for_page(&page);
        assert_eq!((opts.width, opts.height), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
    }

    #[test]
    fn explicit_output_size_scales_strokes_into_requested_canvas() {
        let left = line_with_points(
            vec![point(-650.0, 120.0), point(-650.0, 900.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let right = line_with_points(
            vec![point(650.0, 120.0), point(650.0, 900.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let page = page_with_lines(vec![left, right], None);
        let png = render_page(
            &page,
            RenderOptions {
                width: 702,
                height: 936,
            },
        )
        .unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();

        assert!(
            count_non_white_pixels_in_columns(&decoded, 0, 60) > 50,
            "expected ink near the left edge after scaling"
        );
        assert!(
            count_non_white_pixels_in_columns(&decoded, 642, 702) > 50,
            "expected ink near the right edge after scaling"
        );
    }

    #[test]
    fn scaling_uses_page_paper_size_instead_of_default_width() {
        let left = line_with_points(
            vec![point(-350.0, 60.0), point(-350.0, 540.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let right = line_with_points(
            vec![point(350.0, 60.0), point(350.0, 540.0)],
            PenColor::Black,
            Pen::FinelinerV2,
        );
        let page = page_with_lines(vec![left, right], Some((800, 600)));
        let png = render_page(
            &page,
            RenderOptions {
                width: 400,
                height: 300,
            },
        )
        .unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();

        assert!(
            count_non_white_pixels_in_columns(&decoded, 0, 40) > 20,
            "expected ink near the left edge when scaling from paper_size width"
        );
        assert!(
            count_non_white_pixels_in_columns(&decoded, 360, 400) > 20,
            "expected ink near the right edge when scaling from paper_size width"
        );
    }
}
