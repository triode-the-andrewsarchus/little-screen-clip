//! Enlarge mask cutouts over a filled image, from far layers to near layers.

use super::*;
use image::RgbaImage;
use rayon::prelude::*;

pub(super) fn enlarge_masked_region(
    mut base: RgbaImage,
    source: &RgbaImage,
    weights: &[f32],
    params: &BasicControlsParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if params.masked_zoom <= 1.0 || source.is_empty() {
        return Some(base);
    }
    let slices = if params.depth_mask.source == CreativeFxMaskSource::DepthMap {
        params.depth_slices.clamp(1, 32)
    } else {
        1
    };
    // Cumulative cutouts ensure nearer layers cover farther ones. Each layer
    // retains soft coverage at its depth boundary, including removal alpha.
    for slice in 0..slices {
        if is_cancelled(cancel) {
            return None;
        }
        let low = slice as f32 / slices as f32;
        let coverage: Vec<f32> = weights
            .iter()
            .map(|w| ((w - low) * slices as f32).clamp(0.0, 1.0))
            .collect();
        let Some((cx, cy)) = mask_centroid(source, &coverage) else {
            continue;
        };
        let fraction = (slice + 1) as f32 / slices as f32;
        let zoom = 1.0
            + (params.masked_zoom.clamp(1.0, 4.0) - 1.0)
                * fraction.powf(params.zoom_falloff.clamp(0.1, 4.0));
        let width = source.width() as usize;
        base.as_mut()
            .par_chunks_exact_mut(width * 4)
            .enumerate()
            .for_each(|(y, row)| {
                if is_cancelled(cancel) {
                    return;
                }
                let sy = cy + (y as f32 - cy) / zoom;
                for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let sx = cx + (x as f32 - cx) / zoom;
                    let sampled = sample_cutout(source, &coverage, sx, sy);
                    let alpha = sampled[3];
                    let base_alpha = pixel[3] as f32 / 255.0;
                    let out_alpha = alpha + base_alpha * (1.0 - alpha);
                    if out_alpha > 0.0 {
                        for c in 0..3 {
                            pixel[c] = ((sampled[c]
                                + pixel[c] as f32 / 255.0 * base_alpha * (1.0 - alpha))
                                / out_alpha
                                * 255.0)
                                .round()
                                .clamp(0.0, 255.0) as u8;
                        }
                        pixel[3] = (out_alpha * 255.0).round() as u8;
                    }
                }
            });
    }
    if is_cancelled(cancel) {
        None
    } else {
        Some(base)
    }
}

fn mask_centroid(source: &RgbaImage, coverage: &[f32]) -> Option<(f32, f32)> {
    let mut sum = [0.0f64; 3];
    for (i, weight) in coverage.iter().enumerate() {
        let weight = *weight as f64 * source.as_raw()[i * 4 + 3] as f64 / 255.0;
        sum[0] += (i % source.width() as usize) as f64 * weight;
        sum[1] += (i / source.width() as usize) as f64 * weight;
        sum[2] += weight;
    }
    (sum[2] > 0.0).then(|| ((sum[0] / sum[2]) as f32, (sum[1] / sum[2]) as f32))
}

/// Interpolate premultiplied cutout colors, so an excluded neighbor never
/// contributes its background color to an enlarged foreground edge.
fn sample_cutout(source: &RgbaImage, coverage: &[f32], x: f32, y: f32) -> [f32; 4] {
    let x = x.clamp(0.0, source.width() as f32 - 1.0);
    let y = y.clamp(0.0, source.height() as f32 - 1.0);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(source.width() - 1);
    let y1 = (y0 + 1).min(source.height() - 1);
    let fx = x.fract();
    let fy = y.fract();
    let mut out = [0.0; 4];
    for (px, py, weight) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x1, y0, fx * (1.0 - fy)),
        (x0, y1, (1.0 - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        let pixel = source.get_pixel(px, py);
        let a = pixel[3] as f32 / 255.0 * coverage[(py * source.width() + px) as usize] * weight;
        for c in 0..3 {
            out[c] += pixel[c] as f32 / 255.0 * a;
        }
        out[3] += a;
    }
    out
}
