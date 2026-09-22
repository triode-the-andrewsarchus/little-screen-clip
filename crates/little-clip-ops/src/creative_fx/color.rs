//! Shared image sampling and color-space helpers.

use image::{Rgba, RgbaImage};
#[inline]
pub fn sample_bilinear_channel(img: &RgbaImage, x: f32, y: f32, channel: usize) -> u8 {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let x_clamped = x.clamp(0.0, w - 1.0);
    let y_clamped = y.clamp(0.0, h - 1.0);

    let x0 = x_clamped.floor() as u32;
    let y0 = y_clamped.floor() as u32;
    let x1 = (x0 + 1).min(img.width() - 1);
    let y1 = (y0 + 1).min(img.height() - 1);

    let fx = x_clamped - x0 as f32;
    let fy = y_clamped - y0 as f32;

    let p00 = img.get_pixel(x0, y0)[channel] as f32;
    let p10 = img.get_pixel(x1, y0)[channel] as f32;
    let p01 = img.get_pixel(x0, y1)[channel] as f32;
    let p11 = img.get_pixel(x1, y1)[channel] as f32;

    let top = p00 * (1.0 - fx) + p10 * fx;
    let bot = p01 * (1.0 - fx) + p11 * fx;

    (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8
}

/// Bilinear interpolation for full RGBA pixel.
#[inline]
pub fn sample_bilinear_pixel(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let r = sample_bilinear_channel(img, x, y, 0);
    let g = sample_bilinear_channel(img, x, y, 1);
    let b = sample_bilinear_channel(img, x, y, 2);
    let a = sample_bilinear_channel(img, x, y, 3);
    Rgba([r, g, b, a])
}

// ─────────────────────────────────────────────────────────────────────────────
// sRGB <-> Linear RGB <-> OKLab
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
fn srgb_to_linear(c: f32) -> f32 {
    if c >= 0.04045 {
        ((c + 0.055) / 1.055).powf(2.4)
    } else {
        c / 12.92
    }
}

#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    if c >= 0.0031308 {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * c
    }
}

/// Convert normalized sRGB [0..1] to OKLab (L, a, b).
#[inline]
pub fn srgb_to_oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let r_lin = srgb_to_linear(r);
    let g_lin = srgb_to_linear(g);
    let b_lin = srgb_to_linear(b);

    let l = 0.4122214708 * r_lin + 0.5363325363 * g_lin + 0.0514459929 * b_lin;
    let m = 0.2119034982 * r_lin + 0.6806995451 * g_lin + 0.1073969566 * b_lin;
    let s = 0.0883024619 * r_lin + 0.2817188376 * g_lin + 0.6299787005 * b_lin;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    let oklab_l = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
    let oklab_a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
    let oklab_b = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

    (oklab_l, oklab_a, oklab_b)
}

/// Convert OKLab (L, a, b) to normalized sRGB [0..1].
#[inline]
pub fn oklab_to_srgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let l_lin = l_ * l_ * l_;
    let m_lin = m_ * m_ * m_;
    let s_lin = s_ * s_ * s_;

    let r_lin = 4.0767439362 * l_lin - 3.3077115913 * m_lin + 0.2309699292 * s_lin;
    let g_lin = -1.2684380046 * l_lin + 2.6097574011 * m_lin - 0.3413193965 * s_lin;
    let b_lin = -0.0041960863 * l_lin - 0.7034186147 * m_lin + 1.7076147010 * s_lin;

    let r = linear_to_srgb(r_lin).clamp(0.0, 1.0);
    let g = linear_to_srgb(g_lin).clamp(0.0, 1.0);
    let b = linear_to_srgb(b_lin).clamp(0.0, 1.0);

    (r, g, b)
}
