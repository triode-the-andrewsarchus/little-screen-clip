use super::BrushTipKind;
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

pub(super) fn png(bytes: &[u8], opaque: OpaqueCoverage) -> Option<(BrushTipKind, u32, u32)> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let mut limits = crate::item::raster_preview_limits();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.into_rgba8();
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
        alpha: gray.into_raw(),
    })
}
