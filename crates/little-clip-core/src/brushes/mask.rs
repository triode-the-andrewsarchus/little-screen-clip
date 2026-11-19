use super::{BrushTipKind, ParametricShape};
use image::{DynamicImage, GrayImage, ImageFormat, ImageReader};
use std::io::Cursor;

pub(super) const PREVIEW_EDGE: u32 = 192;
pub(super) const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const PNG_IEND: &[u8; 8] = b"IEND\xaeB`\x82";

pub(super) fn embedded_png(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let start = bytes
        .windows(PNG_MAGIC.len())
        .position(|magic| magic == PNG_MAGIC)?;
    let end = bytes[start..]
        .windows(PNG_IEND.len())
        .position(|magic| magic == PNG_IEND)?
        + start
        + PNG_IEND.len();
    Some((&bytes[start..end], &bytes[end..]))
}

pub(super) enum OpaqueCoverage {
    Light,
    Dark,
}

pub(super) fn png_reader(bytes: &[u8]) -> ImageReader<Cursor<&[u8]>> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = crate::item::raster_preview_limits();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader
}

pub(super) fn png(bytes: &[u8], opaque: OpaqueCoverage) -> Option<(BrushTipKind, u32, u32)> {
    let rgba = png_reader(bytes).decode().ok()?.into_rgba8();
    let (width, height) = rgba.dimensions();
    let has_alpha = rgba.pixels().any(|pixel| pixel[3] < 250);
    let alpha = rgba
        .pixels()
        .map(|pixel| {
            if has_alpha {
                return pixel[3];
            }
            let luma =
                ((u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29)
                    >> 8) as u8;
            match opaque {
                OpaqueCoverage::Light => luma,
                OpaqueCoverage::Dark => 255 - luma,
            }
        })
        .collect();
    Some((raster(width, height, alpha)?, width, height))
}

pub(super) fn raster(width: u32, height: u32, alpha: Vec<u8>) -> Option<BrushTipKind> {
    let gray = GrayImage::from_raw(width, height, alpha)?;
    let gray = if width.max(height) > PREVIEW_EDGE {
        DynamicImage::ImageLuma8(gray)
            .thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
            .into_luma8()
    } else {
        gray
    };
    Some(BrushTipKind::AlphaMask {
        width: gray.width(),
        height: gray.height(),
        alpha: gray.into_raw().into(),
    })
}

pub(super) fn generated(ratio: f32, coverage: impl Fn(f32, f32) -> f32) -> BrushTipKind {
    let width = PREVIEW_EDGE - 1;
    let height = ((width as f32 * ratio).round() as u32).clamp(3, width) | 1;
    let axis = |pixel, length| 2.0 * pixel as f32 / (length - 1) as f32 - 1.0;
    let alpha = (0..height)
        .flat_map(|y| (0..width).map(move |x| (axis(x, width), axis(y, height))))
        .map(|(x, y)| (255.0 * coverage(x, y).clamp(0.0, 1.0)).round() as u8)
        .collect();
    BrushTipKind::AlphaMask {
        width,
        height,
        alpha,
    }
}

pub(super) fn shape_distance(shape: ParametricShape, x: f32, y: f32) -> f32 {
    match shape {
        ParametricShape::Circle => (x * x + y * y).sqrt(),
        ParametricShape::Square => x.abs().max(y.abs()),
        ParametricShape::Diamond => x.abs() + y.abs(),
    }
}

pub(super) fn fold_spikes(x: f32, y: f32, spikes: u8) -> (f32, f32) {
    if spikes <= 2 {
        return (x, y);
    }
    let sector = std::f32::consts::TAU / f32::from(spikes);
    let angle = (y.atan2(x) + sector * 0.5).rem_euclid(sector) - sector * 0.5;
    let (sin, cos) = angle.sin_cos();
    let radius = x.hypot(y);
    (radius * cos, radius * sin)
}
