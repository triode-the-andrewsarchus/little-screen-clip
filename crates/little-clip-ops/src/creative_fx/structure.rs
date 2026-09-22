//! Edge-protected multiscale structure effect.

use super::{DepthMaskConfig, oklab_to_srgb, srgb_to_oklab};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StructureParams {
    /// Structure strength (-100.0 to +100.0). Default is 35.0.
    /// Positive values amplify texture; negative values suppress texture.
    pub amount: f32,
    /// Detail scale emphasis (-50.0 to +50.0). Default is 0.0.
    /// Negative emphasizes fine micro-texture; positive emphasizes coarser structures.
    pub detail_scale: f32,
    /// Edge protection sensitivity (0.0 to 100.0). Default is 50.0.
    /// Higher values protect high-contrast boundaries more aggressively.
    pub edge_protection: f32,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for StructureParams {
    fn default() -> Self {
        Self {
            amount: 35.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Compute 3 integer box blur radii approximating a Gaussian filter of standard deviation sigma.
pub fn compute_box_radii_3(sigma: f32) -> [usize; 3] {
    if sigma <= 0.05 {
        return [0, 0, 0];
    }
    let w_ideal = ((12.0 * sigma * sigma / 3.0) + 1.0).sqrt();
    let mut wl = w_ideal.floor() as usize;
    if wl % 2 == 0 {
        wl = wl.saturating_sub(1);
    }
    let wu = wl + 2;
    let m_ideal =
        (12.0 * sigma * sigma - (3 * wl * wl + 12 * wl + 9) as f32) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round().clamp(0.0, 3.0) as usize;

    let mut radii = [0usize; 3];
    for i in 0..3 {
        let w = if i < m { wl } else { wu };
        radii[i] = w / 2;
    }
    radii
}

/// 1D horizontal box blur pass using sliding-window summation on rows.
fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, _height: usize, r: usize) {
    if r == 0 || width <= 1 {
        dst.copy_from_slice(src);
        return;
    }
    let r_i = r as isize;
    let w_i = width as isize;
    let inv_w = 1.0 / (2 * r + 1) as f64;

    dst.par_chunks_exact_mut(width)
        .zip(src.par_chunks_exact(width))
        .for_each(|(dst_row, src_row)| {
            let mut sum: f64 = 0.0;
            for x in -r_i..=r_i {
                let idx = x.clamp(0, w_i - 1) as usize;
                sum += src_row[idx] as f64;
            }
            dst_row[0] = (sum * inv_w) as f32;

            for x in 1..width {
                let left_idx = ((x as isize) - r_i - 1).clamp(0, w_i - 1) as usize;
                let right_idx = ((x as isize) + r_i).clamp(0, w_i - 1) as usize;
                sum += (src_row[right_idx] as f64) - (src_row[left_idx] as f64);
                dst_row[x] = (sum * inv_w) as f32;
            }
        });
}

/// Transpose a row-major matrix of dimensions (width, height) to (height, width).
fn transpose(src: &[f32], dst: &mut [f32], width: usize, height: usize) {
    dst.par_chunks_exact_mut(height)
        .enumerate()
        .for_each(|(x, dst_col)| {
            for y in 0..height {
                dst_col[y] = src[y * width + x];
            }
        });
}

/// Fast separable 3-pass box blur approximating Gaussian convolution in O(N) time.
pub fn fast_gaussian_blur(src: &[f32], dst: &mut [f32], width: usize, height: usize, sigma: f32) {
    let radii = compute_box_radii_3(sigma);
    if radii == [0, 0, 0] || width <= 1 || height <= 1 {
        dst.copy_from_slice(src);
        return;
    }

    let mut current = src.to_vec();
    let mut temp = vec![0.0f32; width * height];
    let mut transposed = vec![0.0f32; width * height];
    let mut temp_transposed = vec![0.0f32; width * height];

    for &r in &radii {
        if r > 0 {
            // Horizontal blur on rows (width x height)
            box_blur_horizontal(&current, &mut temp, width, height, r);
            // Transpose (width x height) -> (height x width)
            transpose(&temp, &mut transposed, width, height);
            // Horizontal blur on transposed rows (height x width)
            box_blur_horizontal(&transposed, &mut temp_transposed, height, width, r);
            // Transpose back (height x width) -> (width x height)
            transpose(&temp_transposed, &mut current, height, width);
        }
    }
    dst.copy_from_slice(&current);
}

/// Compute 2D gradient magnitude using Scharr 3x3 filter.
fn compute_gradient_magnitude(luma: &[f32], grad: &mut [f32], width: usize, height: usize) {
    let w = width;
    let h = height;
    let w_i = w as isize;
    let h_i = h as isize;

    grad.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, grad_row)| {
            let y_prev = ((y as isize) - 1).clamp(0, h_i - 1) as usize;
            let y_curr = y;
            let y_next = ((y as isize) + 1).clamp(0, h_i - 1) as usize;

            let row_prev = &luma[y_prev * w..(y_prev + 1) * w];
            let row_curr = &luma[y_curr * w..(y_curr + 1) * w];
            let row_next = &luma[y_next * w..(y_next + 1) * w];

            for x in 0..w {
                let x_prev = ((x as isize) - 1).clamp(0, w_i - 1) as usize;
                let x_next = ((x as isize) + 1).clamp(0, w_i - 1) as usize;

                let gx = (3.0 * (row_prev[x_next] - row_prev[x_prev])
                    + 10.0 * (row_curr[x_next] - row_curr[x_prev])
                    + 3.0 * (row_next[x_next] - row_next[x_prev]))
                    * (1.0 / 32.0);

                let gy = (3.0 * (row_next[x_prev] - row_prev[x_prev])
                    + 10.0 * (row_next[x] - row_prev[x])
                    + 3.0 * (row_next[x_next] - row_prev[x_next]))
                    * (1.0 / 32.0);

                grad_row[x] = (gx * gx + gy * gy).sqrt();
            }
        });
}

/// Smooth coring function that damps near-zero noise in detail layers.
#[inline]
fn soft_core(val: f32, tau: f32) -> f32 {
    let abs_v = val.abs();
    if abs_v <= tau * 0.01 {
        0.0
    } else {
        let ratio = abs_v / tau;
        let factor = (1.0 - (-ratio * ratio).exp()).max(0.0);
        val * factor
    }
}

/// Apply selective multiscale structure and texture enhancement with edge protection.
pub fn apply_structure(img: &RgbaImage, params: &StructureParams) -> RgbaImage {
    let (width, height) = (img.width() as usize, img.height() as usize);
    if width == 0 || height == 0 || params.amount.abs() < 0.001 {
        return img.clone();
    }

    let n_pixels = width * height;

    // 1. Convert to OKLab lightness (L) and preserve chromas & alpha
    let mut luma = vec![0.0f32; n_pixels];
    let mut chromas = vec![(0.0f32, 0.0f32, 255u8); n_pixels];

    let pixels: Vec<Rgba<u8>> = img.pixels().copied().collect();
    luma.par_chunks_exact_mut(width)
        .zip(chromas.par_chunks_exact_mut(width))
        .zip(pixels.par_chunks_exact(width))
        .for_each(|((l_row, c_row), p_row)| {
            for x in 0..width {
                let p = p_row[x];
                let r = p[0] as f32 / 255.0;
                let g = p[1] as f32 / 255.0;
                let b = p[2] as f32 / 255.0;
                let (l, a, b_ok) = srgb_to_oklab(r, g, b);
                l_row[x] = l;
                c_row[x] = (a, b_ok, p[3]);
            }
        });

    // 2. Scale factor based on resolution
    let min_dim = width.min(height) as f32;
    let s = (min_dim / 1024.0).powf(0.6).clamp(0.35, 4.0);

    // 3. Compute edge protection mask from gradient magnitude
    let mut grad_raw = vec![0.0f32; n_pixels];
    compute_gradient_magnitude(&luma, &mut grad_raw, width, height);

    let mut grad_smooth = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&grad_raw, &mut grad_smooth, width, height, 2.0 * s);

    let edge_threshold = 0.06 + (100.0 - params.edge_protection.clamp(0.0, 100.0)) * 0.0016;

    // 4. Multiscale decomposition: 4 spatial scale levels
    let sigma_1 = 1.2 * s;
    let sigma_2 = 2.8 * s;
    let sigma_3 = 6.5 * s;
    let sigma_4 = 15.0 * s;

    let delta_sigma_2 = (sigma_2 * sigma_2 - sigma_1 * sigma_1).max(0.01).sqrt();
    let delta_sigma_3 = (sigma_3 * sigma_3 - sigma_2 * sigma_2).max(0.01).sqrt();
    let delta_sigma_4 = (sigma_4 * sigma_4 - sigma_3 * sigma_3).max(0.01).sqrt();

    let mut b1 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&luma, &mut b1, width, height, sigma_1);

    let mut b2 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&b1, &mut b2, width, height, delta_sigma_2);

    let mut b3 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&b2, &mut b3, width, height, delta_sigma_3);

    let mut b4 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&b3, &mut b4, width, height, delta_sigma_4);

    // 5. Band weights modulated by detail_scale (-50 to +50)
    let k = params.detail_scale.clamp(-50.0, 50.0) / 50.0;
    let (w1, w2, w3, w4) = if k < 0.0 {
        let abs_k = -k;
        (
            0.40 * (1.0 + 0.7 * abs_k),
            1.00 * (1.0 + 0.3 * abs_k),
            0.70 * (1.0 - 0.5 * abs_k),
            0.30 * (1.0 - 0.7 * abs_k),
        )
    } else {
        (
            0.40 * (1.0 - 0.6 * k),
            1.00 * (1.0 - 0.2 * k),
            0.70 * (1.0 + 0.5 * k),
            0.30 * (1.0 + 0.9 * k),
        )
    };

    let tau_core = 0.005;

    // 6. Synthesize output pixels into raw RGBA buffer
    let mut raw_bytes = vec![0u8; n_pixels * 4];

    raw_bytes
        .par_chunks_exact_mut(width * 4)
        .enumerate()
        .for_each(|(y, out_row_bytes)| {
            let row_start = y * width;
            for x in 0..width {
                let idx = row_start + x;
                let l_orig = luma[idx];

                // Detail bandpass layers
                let d1 = l_orig - b1[idx];
                let d2 = b1[idx] - b2[idx];
                let d3 = b2[idx] - b3[idx];
                let d4 = b3[idx] - b4[idx];

                let d1_cored = soft_core(d1, tau_core);

                let total_detail = w1 * d1_cored + w2 * d2 + w3 * d3 + w4 * d4;

                // Soft edge-stop mask: 1.0 in texture/flat areas, drops near boundaries
                let g = grad_smooth[idx] / edge_threshold;
                let g3 = g * g * g;
                let edge_mask = 1.0 / (1.0 + g3);

                let delta = total_detail * edge_mask;

                // Positive amount enhances texture; negative amount suppresses texture
                let l_new = if params.amount >= 0.0 {
                    l_orig + (params.amount / 50.0) * delta
                } else {
                    l_orig + (params.amount / 100.0) * delta
                };

                let l_clamped = l_new.clamp(0.0, 1.0);
                let (a_ok, b_ok, alpha) = chromas[idx];
                let (r_norm, g_norm, b_norm) = oklab_to_srgb(l_clamped, a_ok, b_ok);

                let px_offset = x * 4;
                out_row_bytes[px_offset] = (r_norm * 255.0).round().clamp(0.0, 255.0) as u8;
                out_row_bytes[px_offset + 1] = (g_norm * 255.0).round().clamp(0.0, 255.0) as u8;
                out_row_bytes[px_offset + 2] = (b_norm * 255.0).round().clamp(0.0, 255.0) as u8;
                out_row_bytes[px_offset + 3] = alpha;
            }
        });

    RgbaImage::from_raw(width as u32, height as u32, raw_bytes).unwrap_or_else(|| img.clone())
}
