use super::{
    Canvas, FABRIC, SHADOW_WIDTH, THREAD_WIDTH, check_cancelled, pixmap, shadow_paint,
    stitch_groups, stroke_style, thread_paint,
};
use crate::embroidery::Stitch;
use rayon::prelude::*;
use resvg::tiny_skia::{Color, FillRule, Path, PathStroker, Pixmap, Transform};
use std::sync::atomic::AtomicBool;

const MAX_BANDS: u32 = 8;
const CLIP_PADDING: u32 = 4;
const PREPARED_GROUP_LIMIT: usize = 16;

pub(super) fn worth_parallelizing(pixmap: &Pixmap, stitches: usize) -> bool {
    stitches >= 20_000
        && pixmap.width() * pixmap.height() >= 350_000
        && rayon::current_num_threads() > 1
}

pub(super) fn sew(
    canvas: &mut Canvas,
    stitches: &[Stitch],
    cancelled: &AtomicBool,
) -> Result<(), String> {
    check_cancelled(cancelled)?;
    let width = canvas.pixmap.width();
    let height = canvas.pixmap.height();
    let count = (rayon::current_num_threads() as u32).min(MAX_BANDS);
    let rows = height.div_ceil(count);
    let mut bands = (0..height)
        .step_by(rows as usize)
        .map(|start| Band::new(width, height, start, rows))
        .collect::<Result<Vec<_>, _>>()?;

    // Limit the number of stroked outlines in memory and paint each batch in sewing order.
    let transform = canvas.raster_transform;
    let resolution = PathStroker::compute_resolution_scale(&transform);
    let mut groups = stitch_groups(stitches).peekable();
    let mut batch = Vec::with_capacity(PREPARED_GROUP_LIMIT);
    while groups.peek().is_some() {
        batch.clear();
        batch.extend(groups.by_ref().take(PREPARED_GROUP_LIMIT));
        let paths = batch
            .par_iter()
            .map(|group| {
                check_cancelled(cancelled)?;
                Ok::<_, String>(canvas.path(group).map(|path| StrokedGroup {
                    color: group[0].color,
                    paths: [SHADOW_WIDTH, THREAD_WIDTH].map(|width| {
                        path.stroke(&stroke_style(width), resolution)
                            .and_then(|outline| outline.transform(transform))
                    }),
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        bands
            .par_iter_mut()
            .try_for_each(|band| band.paint(&paths, cancelled))?;
    }

    let stride = width as usize * 4;
    for (index, (dst, band)) in canvas
        .pixmap
        .data_mut()
        .chunks_mut(rows as usize * stride)
        .zip(bands)
        .enumerate()
    {
        let offset = (index * rows as usize - band.top as usize) * stride;
        dst.copy_from_slice(&band.pixels.data()[offset..offset + dst.len()]);
    }
    Ok(())
}

struct StrokedGroup {
    color: [u8; 3],
    paths: [Option<Path>; 2],
}

struct Band {
    top: u32,
    pixels: Pixmap,
}

impl Band {
    fn new(width: u32, height: u32, start: u32, rows: u32) -> Result<Self, String> {
        // Discard the padded rows so the rasterizer's clipping edge stays outside the image seam.
        let top = start.saturating_sub(CLIP_PADDING);
        let bottom = (start + rows + CLIP_PADDING).min(height);
        let mut pixels = pixmap(width, bottom - top)?;
        let [r, g, b] = FABRIC;
        pixels.fill(Color::from_rgba8(r, g, b, 255));
        Ok(Self { top, pixels })
    }

    fn paint(
        &mut self,
        groups: &[Option<StrokedGroup>],
        cancelled: &AtomicBool,
    ) -> Result<(), String> {
        let top = self.top as f32;
        let bottom = top + self.pixels.height() as f32;
        let transform = Transform::from_translate(0.0, -top);
        let shadow = shadow_paint();
        for group in groups.iter().flatten() {
            check_cancelled(cancelled)?;
            let thread = thread_paint(group.color);
            for (path, paint) in group.paths.iter().zip([&shadow, &thread]) {
                if let Some(path) = path
                    && path.bounds().bottom() > top
                    && path.bounds().top() < bottom
                {
                    self.pixels
                        .fill_path(path, paint, FillRule::Winding, transform, None);
                }
            }
        }
        Ok(())
    }
}
