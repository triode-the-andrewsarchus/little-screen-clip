use image::{ImageBuffer, Rgb};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use wide::f32x4;

/// Helper to apply Gaussian blur (Approx) using 3 Box Blurs
/// Optimized to minimize allocations and use SIMD
pub fn apply_gaussian_blur(image: &mut ImageBuffer<Rgb<f32>, Vec<f32>>, sigma: f32) {
    apply_gaussian_blur_cancellable(image, sigma, None);
}

/// The same blur, with optional cancellation at each pass and row/column.
pub fn apply_gaussian_blur_cancellable(
    image: &mut ImageBuffer<Rgb<f32>, Vec<f32>>,
    sigma: f32,
    cancel: Option<&AtomicBool>,
) {
    if sigma <= 0.0 || !sigma.is_finite() || image.width() == 0 || image.height() == 0 {
        return;
    }

    let width = image.width();
    let height = image.height();

    // w = sqrt(12 * sigma^2 / n + 1)
    // radius = (w - 1) / 2
    let n = 3.0;
    let w = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let radius = ((w - 1.0) / 2.0).floor() as u32;
    let radius = radius.max(1);

    // Single auxiliary buffer allocation
    let mut backbuffer: ImageBuffer<Rgb<f32>, Vec<f32>> = ImageBuffer::new(width, height);

    for _ in 0..3 {
        if cancelled(cancel) {
            return;
        }
        // Horizontal: Image -> Backbuffer
        horizontal_blur_pass(image, &mut backbuffer, radius, cancel);
        if cancelled(cancel) {
            return;
        }
        // Vertical: Backbuffer -> Image
        vertical_blur_pass(&backbuffer, image, radius, cancel);
    }
}

#[inline]
fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
}

fn horizontal_blur_pass(
    src: &ImageBuffer<Rgb<f32>, Vec<f32>>,
    dst: &mut ImageBuffer<Rgb<f32>, Vec<f32>>,
    radius: u32,
    cancel: Option<&AtomicBool>,
) {
    let width = src.width();
    let r = radius as i32;
    let weight = 1.0 / (2.0 * radius as f32 + 1.0);
    let weight_vec = f32x4::splat(weight);

    // Iterate over rows in parallel
    // We use chunks of the raw buffer to write to dst safely
    dst.par_chunks_mut((width * 3) as usize)
        .enumerate()
        .for_each(|(y, dst_row)| {
            if cancelled(cancel) {
                return;
            }
            let y = y as u32;
            let mut sum_r = 0.0;
            let mut sum_g = 0.0;
            let mut sum_b = 0.0;

            // Initial window [-r, r] centered at 0
            // Left side [-r, -1] -> clamped to 0
            let p0 = src.get_pixel(0, y).0;
            sum_r += p0[0] * (r as f32);
            sum_g += p0[1] * (r as f32);
            sum_b += p0[2] * (r as f32);

            // Right side [0, r]
            for x in 0..=r {
                let px = src.get_pixel(x.min((width - 1) as i32) as u32, y).0;
                sum_r += px[0];
                sum_g += px[1];
                sum_b += px[2];
            }

            let mut sum_vec = f32x4::from([sum_r, sum_g, sum_b, 0.0]);

            for x in 0..width {
                let avg = sum_vec * weight_vec;
                let avg_arr: [f32; 4] = avg.into();

                let pixel_idx = (x as usize) * 3;
                dst_row[pixel_idx] = avg_arr[0];
                dst_row[pixel_idx + 1] = avg_arr[1];
                dst_row[pixel_idx + 2] = avg_arr[2];

                // Update sliding window
                let out_x = (x as i32 - r).max(0) as u32;
                let in_x = (x as i32 + r + 1).min((width - 1) as i32) as u32;

                let p_out = src.get_pixel(out_x, y).0;
                let p_in = src.get_pixel(in_x, y).0;

                let v_out = f32x4::from([p_out[0], p_out[1], p_out[2], 0.0]);
                let v_in = f32x4::from([p_in[0], p_in[1], p_in[2], 0.0]);

                sum_vec += v_in - v_out;
            }
        });
}

fn vertical_blur_pass(
    src: &ImageBuffer<Rgb<f32>, Vec<f32>>,
    dst: &mut ImageBuffer<Rgb<f32>, Vec<f32>>,
    radius: u32,
    cancel: Option<&AtomicBool>,
) {
    let width = src.width() as usize;
    let height = src.height() as usize;
    if width == 0 || height == 0 {
        return;
    }
    let radius = radius as usize;
    let stride = width * 3;
    let weight = f32x4::splat(1.0 / (2.0 * radius as f32 + 1.0));
    // Each Rayon task owns complete, disjoint rows. Creating whole-image mutable
    // slices per column aliases even when the writes themselves are disjoint.
    // Bands amortize window initialization while retaining linear-time sliding
    // updates and contiguous writes; temporary storage is only one row per task.
    let band_rows = 128.max(radius.saturating_mul(4)).min(height);
    dst.par_chunks_mut(stride * band_rows)
        .enumerate()
        .for_each(|(band_index, band)| {
            if cancelled(cancel) {
                return;
            }
            let first_y = band_index * band_rows;
            let top = first_y.saturating_sub(radius);
            let bottom = first_y.saturating_add(radius).min(height - 1);
            let top_padding = radius.saturating_sub(first_y) as f32;
            let bottom_padding = first_y.saturating_add(radius).saturating_sub(height - 1) as f32;
            let mut sums: Vec<f32x4> = (0..width)
                .map(|x| {
                    let a = src.get_pixel(x as u32, 0).0;
                    let b = src.get_pixel(x as u32, height as u32 - 1).0;
                    f32x4::from([a[0], a[1], a[2], 0.0]) * top_padding
                        + f32x4::from([b[0], b[1], b[2], 0.0]) * bottom_padding
                })
                .collect();
            for y in top..=bottom {
                if cancelled(cancel) {
                    return;
                }
                for (x, sum) in sums.iter_mut().enumerate() {
                    let p = src.get_pixel(x as u32, y as u32).0;
                    *sum += f32x4::from([p[0], p[1], p[2], 0.0]);
                }
            }
            for (local_y, row) in band.chunks_exact_mut(stride).enumerate() {
                if cancelled(cancel) {
                    return;
                }
                let y = first_y + local_y;
                let out_y = y.saturating_sub(radius) as u32;
                let in_y = y.saturating_add(radius).saturating_add(1).min(height - 1) as u32;
                for (x, (sum, pixel)) in sums.iter_mut().zip(row.chunks_exact_mut(3)).enumerate() {
                    let average: [f32; 4] = (*sum * weight).into();
                    pixel.copy_from_slice(&average[..3]);
                    let a = src.get_pixel(x as u32, out_y).0;
                    let b = src.get_pixel(x as u32, in_y).0;
                    *sum +=
                        f32x4::from([b[0], b[1], b[2], 0.0]) - f32x4::from([a[0], a[1], a[2], 0.0]);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_bands_match_clamped_reference_across_band_and_image_edges() {
        for (width, height) in [(1, 1), (3, 7), (7, 301)] {
            let source = ImageBuffer::from_fn(width, height, |x, y| {
                Rgb([
                    ((x * 17 + y * 7) % 101) as f32 / 100.0,
                    y as f32 / height as f32,
                    0.75,
                ])
            });
            for radius in [0, 1, 4, 17] {
                let mut output = ImageBuffer::new(width, height);
                vertical_blur_pass(&source, &mut output, radius, None);
                for (x, y, actual) in output.enumerate_pixels() {
                    let mut expected = [0.0; 3];
                    for offset in -(radius as i64)..=radius as i64 {
                        let yy = (y as i64 + offset).clamp(0, height as i64 - 1) as u32;
                        let pixel = source.get_pixel(x, yy);
                        for (sum, value) in expected.iter_mut().zip(pixel.0) {
                            *sum += value / (2 * radius + 1) as f32;
                        }
                    }
                    for (actual, expected) in actual.0.into_iter().zip(expected) {
                        assert!(
                            (actual - expected).abs() < 0.0001,
                            "{width}x{height} r={radius} ({x},{y}): {actual} != {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cancelled_vertical_blur_leaves_the_destination_untouched() {
        let source = ImageBuffer::from_pixel(5, 300, Rgb([1.0; 3]));
        let mut output = ImageBuffer::from_pixel(5, 300, Rgb([0.25; 3]));
        vertical_blur_pass(&source, &mut output, 3, Some(&AtomicBool::new(true)));
        assert!(output.pixels().all(|pixel| pixel.0 == [0.25; 3]));
    }
}
