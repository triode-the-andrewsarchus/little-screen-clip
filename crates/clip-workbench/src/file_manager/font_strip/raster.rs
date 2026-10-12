use super::{Document, SAMPLES};
use little_clip_core::ImageThumbnail;
use std::sync::atomic::{AtomicBool, Ordering};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

struct Outline(PathBuilder);
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}
fn draw(
    pixmap: &mut Pixmap,
    face: &ttf_parser::Face<'_>,
    text: &str,
    row: f32,
    height: f32,
    color: [u8; 3],
    token: &AtomicBool,
) -> Option<()> {
    let extent = (f32::from(face.ascender()) - f32::from(face.descender()))
        .max(f32::from(face.units_per_em()));
    let scale = (height - 2.0).max(1.0) / extent;
    let advance = text
        .chars()
        .filter_map(|c| face.glyph_index(c))
        .map(|g| f32::from(face.glyph_hor_advance(g).unwrap_or(0)))
        .sum::<f32>();
    let scale = scale.min((pixmap.width() as f32 - 4.0).max(1.0) / advance.max(1.0));
    let baseline = row + (height - extent * scale) / 2.0 + f32::from(face.ascender()) * scale;
    let mut x = if text == "Aa" {
        ((pixmap.width() as f32 - advance * scale) / 2.0).max(2.0)
    } else {
        2.0
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    for ch in text.chars() {
        if token.load(Ordering::Relaxed) {
            return None;
        }
        let Some(glyph) = face.glyph_index(ch) else {
            x += f32::from(face.units_per_em()) * scale * 0.5;
            continue;
        };
        let mut outline = Outline(PathBuilder::new());
        face.outline_glyph(glyph, &mut outline);
        if let Some(path) = outline.0.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::from_row(scale, 0.0, 0.0, -scale, x, baseline),
                None,
            );
        }
        x += f32::from(face.glyph_hor_advance(glyph).unwrap_or(0)) * scale;
    }
    Some(())
}
pub(super) fn thumbnail(document: &Document, token: &AtomicBool) -> Option<ImageThumbnail> {
    let face = ttf_parser::Face::parse(&document.bytes, 0).ok()?;
    let mut pixmap = Pixmap::new(128, 128)?;
    draw(&mut pixmap, &face, "Aa", 0.0, 128.0, [255; 3], token)?;
    Some(ImageThumbnail {
        width: 128,
        height: 128,
        premultiplied_rgba: pixmap.take(),
    })
}
pub(in crate::file_manager) fn render(
    document: &Document,
    width: u32,
    height: u32,
    color: [u8; 3],
    token: &AtomicBool,
) -> Option<ImageThumbnail> {
    let width = width.clamp(1, 4096);
    let height = height.clamp(1, 512);
    let face = ttf_parser::Face::parse(&document.bytes, 0).ok()?;
    let mut pixmap = Pixmap::new(width, height)?;
    let row_height = height as f32 / 3.0;
    for (index, text) in SAMPLES.iter().enumerate() {
        draw(
            &mut pixmap,
            &face,
            text,
            index as f32 * row_height,
            row_height,
            color,
            token,
        )?;
    }
    Some(ImageThumbnail {
        width,
        height,
        premultiplied_rgba: pixmap.take(),
    })
}
