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

    let l = 0.412_221_46 * r_lin + 0.536_332_55 * g_lin + 0.051_445_995 * b_lin;
    let m = 0.211_903_5 * r_lin + 0.680_699_5 * g_lin + 0.107_396_96 * b_lin;
    let s = 0.088_302_46 * r_lin + 0.281_718_85 * g_lin + 0.629_978_7 * b_lin;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    let oklab_l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let oklab_a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let oklab_b = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;

    (oklab_l, oklab_a, oklab_b)
}

/// Convert OKLab (L, a, b) to normalized sRGB [0..1].
#[inline]
pub fn oklab_to_srgb(l: f32, a: f32, b: f32) -> (f32, f32, f32) {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l_lin = l_ * l_ * l_;
    let m_lin = m_ * m_ * m_;
    let s_lin = s_ * s_ * s_;

    let r_lin = 4.076_744 * l_lin - 3.307_711_6 * m_lin + 0.230_969_94 * s_lin;
    let g_lin = -1.268_438 * l_lin + 2.609_757_4 * m_lin - 0.341_319_38 * s_lin;
    let b_lin = -0.0041960863 * l_lin - 0.703_418_6 * m_lin + 1.707_614_7 * s_lin;

    let r = linear_to_srgb(r_lin).clamp(0.0, 1.0);
    let g = linear_to_srgb(g_lin).clamp(0.0, 1.0);
    let b = linear_to_srgb(b_lin).clamp(0.0, 1.0);

    (r, g, b)
}
