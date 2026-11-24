//! Decode reMarkable paper templates (SVG/PNG) into a [`BackgroundImage`].
//!
//! reMarkable ships each template as both an `.svg` and a `.png` under
//! `/usr/share/remarkable/templates/`. SVG is preferred (crisp at any size and
//! it rasterizes straight into a `tiny-skia` pixmap via `resvg`); PNG is the
//! fallback for templates that only ship one variant. The decoded raster fills
//! the page canvas and is composited beneath the strokes by
//! [`crate::render::render_page_with_background`].
//!
//! This module is gated behind the `templates` feature. The core renderer also
//! uses `resvg` for deterministic typed text.

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::render::BackgroundImage;

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("no template image data provided")]
    NoData,
    #[error("invalid raster size: {width}x{height}")]
    InvalidSize { width: u32, height: u32 },
    #[error("SVG decode failed: {0}")]
    Svg(String),
    #[error("PNG decode failed: {0}")]
    Png(String),
}

/// Decode a template to a canvas-sized background, preferring SVG.
///
/// Tries `svg` first; if it fails (or is absent) and `png` is present, falls
/// back to PNG. Returns [`TemplateError::NoData`] when neither is supplied.
///
/// # Errors
/// Returns an error if the chosen format fails to decode and no usable
/// fallback remains.
pub fn decode_template(
    svg: Option<&[u8]>,
    png: Option<&[u8]>,
    width: u32,
    height: u32,
) -> Result<BackgroundImage, TemplateError> {
    if let Some(data) = svg {
        let result = decode_svg(data, width, height);
        // Only swallow the SVG error if we still have a PNG to try.
        if result.is_ok() || png.is_none() {
            return result;
        }
    }
    match png {
        Some(data) => decode_png(data, width, height),
        None => Err(TemplateError::NoData),
    }
}

/// Rasterize an SVG template to `width`×`height`, scaling its viewbox to fill.
///
/// # Errors
/// Returns [`TemplateError::Svg`] if the bytes aren't valid SVG and
/// [`TemplateError::InvalidSize`] if the target dimensions are zero/too large.
#[allow(clippy::cast_precision_loss)]
pub fn decode_svg(bytes: &[u8], width: u32, height: u32) -> Result<BackgroundImage, TemplateError> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options)
        .map_err(|e| TemplateError::Svg(e.to_string()))?;

    let mut pixmap =
        Pixmap::new(width, height).ok_or(TemplateError::InvalidSize { width, height })?;

    let size = tree.size();
    let sx = width as f32 / size.width().max(1.0);
    let sy = height as f32 / size.height().max(1.0);
    resvg::render(&tree, Transform::from_scale(sx, sy), &mut pixmap.as_mut());

    Ok(BackgroundImage::from_pixmap(&pixmap))
}

/// Decode a PNG template and scale it to `width`×`height`.
///
/// # Errors
/// Returns [`TemplateError::Png`] on a malformed PNG and
/// [`TemplateError::InvalidSize`] if a target/native pixmap can't be allocated.
#[allow(clippy::cast_precision_loss)]
pub fn decode_png(bytes: &[u8], width: u32, height: u32) -> Result<BackgroundImage, TemplateError> {
    let native =
        Pixmap::decode_png(bytes).map_err(|error| TemplateError::Png(error.to_string()))?;

    if native.width() == width && native.height() == height {
        return Ok(BackgroundImage::from_pixmap(&native));
    }

    // Native size differs from the canvas (e.g. a --width override): rescale.
    let mut scaled =
        Pixmap::new(width, height).ok_or(TemplateError::InvalidSize { width, height })?;
    let sx = width as f32 / native.width() as f32;
    let sy = height as f32 / native.height() as f32;
    let paint = PixmapPaint {
        quality: FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    scaled.draw_pixmap(
        0,
        0,
        native.as_ref(),
        &paint,
        Transform::from_scale(sx, sy),
        None,
    );
    Ok(BackgroundImage::from_pixmap(&scaled))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4×4 SVG: white background with a black rule across the middle. Stands
    /// in for a lined paper template.
    const LINED_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">
        <rect width="4" height="4" fill="#ffffff"/>
        <rect y="2" width="4" height="1" fill="#000000"/>
    </svg>"##;

    fn pixel(bg: &BackgroundImage, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * bg.width + x) * 4) as usize;
        (bg.rgba[i], bg.rgba[i + 1], bg.rgba[i + 2], bg.rgba[i + 3])
    }

    #[test]
    fn decode_svg_rasterizes_to_requested_size() {
        let bg = decode_svg(LINED_SVG, 8, 8).unwrap();
        assert_eq!((bg.width, bg.height), (8, 8));
        assert_eq!(bg.rgba.len(), 8 * 8 * 4);
        // Top row is the white background; the rule sits lower down.
        assert_eq!(pixel(&bg, 0, 0), (0xFF, 0xFF, 0xFF, 0xFF));
        let any_ink = (0..bg.height).any(|y| {
            let (r, g, b, _) = pixel(&bg, 4, y);
            r < 0x40 && g < 0x40 && b < 0x40
        });
        assert!(any_ink, "expected the black rule somewhere in the column");
    }

    #[test]
    fn decode_template_prefers_svg_then_falls_back_to_png() {
        // Valid SVG wins even when a PNG is also supplied.
        let bg = decode_template(Some(LINED_SVG), Some(b"not-a-png"), 4, 4).unwrap();
        assert_eq!((bg.width, bg.height), (4, 4));

        // Broken SVG + no PNG surfaces the SVG error.
        assert!(matches!(
            decode_template(Some(b"<not-svg"), None, 4, 4),
            Err(TemplateError::Svg(_))
        ));

        // Nothing supplied.
        assert!(matches!(
            decode_template(None, None, 4, 4),
            Err(TemplateError::NoData)
        ));
    }

    #[test]
    fn decode_png_round_trips_through_svg_render() {
        // Produce a PNG by rendering the SVG, then decode it back.
        let svg_bg = decode_svg(LINED_SVG, 4, 4).unwrap();
        let pixmap = Pixmap::from_vec(
            svg_bg.rgba.clone(),
            tiny_skia::IntSize::from_wh(4, 4).unwrap(),
        )
        .unwrap();
        let png_bytes = pixmap.encode_png().unwrap();

        let png_bg = decode_png(&png_bytes, 4, 4).unwrap();
        assert_eq!((png_bg.width, png_bg.height), (4, 4));
        assert_eq!(png_bg.rgba.len(), 4 * 4 * 4);
    }
}
