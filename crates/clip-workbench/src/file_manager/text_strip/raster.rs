use super::{COLORS, Document};
use little_clip_core::ImageThumbnail;
use std::sync::atomic::AtomicBool;

/// Aggregate source bins directly into device-pixel columns, never a huge image.
pub(in crate::file_manager) fn render(
    document: &Document,
    width: u32,
    height: u32,
    colors: [[u8; 3]; COLORS.len()],
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    let width = width.clamp(1, 2048);
    let height = height.clamp(1, 128);
    let columns = (width as usize).min(document.bins.len());
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    for x in 0..columns {
        super::stream::check_cancel(cancelled).ok()?;
        let start = x * document.bins.len() / columns;
        let end = ((x + 1) * document.bins.len() / columns).max(start + 1);
        let bins = &document.bins[start..end];
        let count = bins.last()?.end.saturating_sub(bins[0].start).max(1);
        let slot_start = x * width as usize / columns;
        let slot_end = ((x + 1) * width as usize / columns).max(slot_start + 1);
        // Expand short documents across the strip, leaving at most two device
        // pixels between source slots. Dense documents have contiguous bars.
        let slot_width = slot_end - slot_start;
        let stroke_width = slot_width - if slot_width > 2 { 2 } else { 0 };
        let x0 = slot_start + (slot_end - slot_start - stroke_width) / 2;
        let x1 = x0 + stroke_width;
        for y in 0..height as usize {
            let c0 = y * document.columns / height as usize;
            let c1 =
                (((y + 1) * document.columns / height as usize).max(c0 + 1)).min(document.columns);
            let mut categories = [0_u64; COLORS.len()];
            for bin in bins {
                for cell in &bin.cells[c0..c1] {
                    for (sum, value) in categories.iter_mut().zip(cell) {
                        *sum += u64::from(*value);
                    }
                }
            }
            let (category, dominant) = categories
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)?;
            if *dominant == 0 {
                continue;
            }
            let occupancy =
                categories.iter().sum::<u64>() as f64 / (count * (c1 - c0) as u64) as f64;
            let alpha = (255.0 * occupancy.clamp(0.0, 1.0)) as u8;
            for px in x0..x1 {
                let offset = (y * width as usize + px) * 4;
                for (target, value) in pixels[offset..offset + 3].iter_mut().zip(colors[category]) {
                    *target = (u16::from(value) * u16::from(alpha) / 255) as u8;
                }
                pixels[offset + 3] = alpha;
            }
        }
    }
    Some(ImageThumbnail {
        width,
        height,
        premultiplied_rgba: pixels,
    })
}

pub(in crate::file_manager) fn palette_colors(
    palette: &little_clip_core::palette::AppPalette,
) -> [[u8; 3]; COLORS.len()] {
    COLORS.map(|color| {
        let color = crate::text_preview::map_syntect_color_to_theme(color, palette);
        [color.r(), color.g(), color.b()]
    })
}
