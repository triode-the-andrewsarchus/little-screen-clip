//! CMYK halftone effect.

use super::{DepthMaskConfig, sample_bilinear_pixel};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PaperTint {
    #[default]
    White,
    Newsprint,
    Kraft,
    Cream,
}

impl PaperTint {
    pub fn all() -> &'static [PaperTint] {
        &[
            PaperTint::White,
            PaperTint::Newsprint,
            PaperTint::Kraft,
            PaperTint::Cream,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::White => "White (Pure)",
            Self::Newsprint => "Newsprint (Subtle Warm)",
            Self::Kraft => "Kraft (Cardboard/Vintage)",
            Self::Cream => "Cream (Art Paper)",
        }
    }

    pub fn rgb(&self) -> [u8; 3] {
        match self {
            Self::White => [255, 255, 255],
            Self::Newsprint => [244, 238, 222],
            Self::Kraft => [218, 196, 160],
            Self::Cream => [255, 253, 240],
        }
    }
}

/// Parameters for CMYK rosette halftone and press misregistration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CmykHalftoneParams {
    /// Dot screen grid spacing in pixels (2.0 to 20.0). Default is 5.0.
    pub dot_size: f32,
    /// Dot gain factor (0.5 to 2.0). Default is 1.1.
    pub dot_gain: f32,
    /// Background paper tint.
    pub paper_tint: PaperTint,
    /// Cyan plate displacement in pixels [dx, dy]. Default [-1.5, 0.5].
    pub cyan_offset: [f32; 2],
    /// Magenta plate displacement in pixels [dx, dy]. Default [1.0, -1.0].
    pub magenta_offset: [f32; 2],
    /// Yellow plate displacement in pixels [dx, dy]. Default [0.0, 1.5].
    pub yellow_offset: [f32; 2],
    /// Black (Key) plate displacement in pixels [dx, dy]. Default [0.0, 0.0].
    pub black_offset: [f32; 2],
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for CmykHalftoneParams {
    fn default() -> Self {
        Self {
            dot_size: 5.0,
            dot_gain: 1.1,
            paper_tint: PaperTint::Newsprint,
            cyan_offset: [-1.5, 0.5],
            magenta_offset: [1.0, -1.0],
            yellow_offset: [0.0, 1.5],
            black_offset: [0.0, 0.0],
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Convert normalized sRGB to CMYK [0..1].
#[inline]
pub fn rgb_to_cmyk(r: f32, g: f32, b: f32) -> (f32, f32, f32, f32) {
    let k = (1.0 - r.max(g).max(b)).clamp(0.0, 1.0);
    if k >= 0.9999 {
        (0.0, 0.0, 0.0, 1.0)
    } else {
        let inv_k = 1.0 - k;
        let c = ((1.0 - r - k) / inv_k).clamp(0.0, 1.0);
        let m = ((1.0 - g - k) / inv_k).clamp(0.0, 1.0);
        let y = ((1.0 - b - k) / inv_k).clamp(0.0, 1.0);
        (c, m, y, k)
    }
}

/// Apply CMYK rosette halftone with plate misregistration.
pub fn apply_cmyk_halftone(img: &RgbaImage, params: &CmykHalftoneParams) -> RgbaImage {
    apply_cmyk_halftone_cancellable(img, params, None)
        .expect("uncancelled CMYK rendering cannot stop")
}

pub(super) fn apply_cmyk_halftone_cancellable(
    img: &RgbaImage,
    params: &CmykHalftoneParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if super::is_cancelled(cancel) {
        return None;
    }
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Some(img.clone());
    }

    let dot_size = params.dot_size.max(1.5);
    let dot_gain = params.dot_gain.clamp(0.5, 2.5);
    let paper_rgb = params.paper_tint.rgb();
    let pr = paper_rgb[0] as f32 / 255.0;
    let pg = paper_rgb[1] as f32 / 255.0;
    let pb = paper_rgb[2] as f32 / 255.0;

    // Standard rosette screen angles: Cyan 15°, Magenta 75°, Yellow 0°, Black 45°
    let c_angle: f32 = 15.0_f32.to_radians();
    let m_angle: f32 = 75.0_f32.to_radians();
    let y_angle: f32 = 0.0_f32.to_radians();
    let k_angle: f32 = 45.0_f32.to_radians();

    let (c_cos, c_sin) = (c_angle.cos(), c_angle.sin());
    let (m_cos, m_sin) = (m_angle.cos(), m_angle.sin());
    let (y_cos, y_sin) = (y_angle.cos(), y_angle.sin());
    let (k_cos, k_sin) = (k_angle.cos(), k_angle.sin());

    let compute_coverage = |x: f32, y: f32, cos_a: f32, sin_a: f32, ink: f32| -> f32 {
        if ink <= 0.001 {
            return 0.0;
        }
        let u = x * cos_a + y * sin_a;
        let v = -x * sin_a + y * cos_a;
        let cu = (u / dot_size).floor() * dot_size + 0.5 * dot_size;
        let cv = (v / dot_size).floor() * dot_size + 0.5 * dot_size;
        let dist = ((u - cu) * (u - cu) + (v - cv) * (v - cv)).sqrt();

        let effective_ink = (ink * dot_gain).min(1.4);
        let max_r = 0.5 * dot_size * 1.4142;
        let target_r = 0.5 * dot_size * effective_ink.sqrt() * 1.15;
        let clamped_r = target_r.min(max_r);

        (clamped_r - dist + 0.5).clamp(0.0, 1.0)
    };

    let mut out = RgbaImage::new(w, h);

    out.enumerate_rows_mut().par_bridge().for_each(|(y, row)| {
        if super::is_cancelled(cancel) {
            return;
        }
        let y_f = y as f32;
        for (x, _, pixel) in row {
            let x_f = x as f32;

            // Sample Cyan plate with its displacement
            let c_pix = sample_bilinear_pixel(
                img,
                x_f - params.cyan_offset[0],
                y_f - params.cyan_offset[1],
            );
            let (c_ink, _, _, _) = rgb_to_cmyk(
                c_pix[0] as f32 / 255.0,
                c_pix[1] as f32 / 255.0,
                c_pix[2] as f32 / 255.0,
            );
            let c_cov = compute_coverage(x_f, y_f, c_cos, c_sin, c_ink);

            // Sample Magenta plate with its displacement
            let m_pix = sample_bilinear_pixel(
                img,
                x_f - params.magenta_offset[0],
                y_f - params.magenta_offset[1],
            );
            let (_, m_ink, _, _) = rgb_to_cmyk(
                m_pix[0] as f32 / 255.0,
                m_pix[1] as f32 / 255.0,
                m_pix[2] as f32 / 255.0,
            );
            let m_cov = compute_coverage(x_f, y_f, m_cos, m_sin, m_ink);

            // Sample Yellow plate with its displacement
            let y_pix = sample_bilinear_pixel(
                img,
                x_f - params.yellow_offset[0],
                y_f - params.yellow_offset[1],
            );
            let (_, _, y_ink, _) = rgb_to_cmyk(
                y_pix[0] as f32 / 255.0,
                y_pix[1] as f32 / 255.0,
                y_pix[2] as f32 / 255.0,
            );
            let y_cov = compute_coverage(x_f, y_f, y_cos, y_sin, y_ink);

            // Sample Black plate with its displacement
            let k_pix = sample_bilinear_pixel(
                img,
                x_f - params.black_offset[0],
                y_f - params.black_offset[1],
            );
            let (_, _, _, k_ink) = rgb_to_cmyk(
                k_pix[0] as f32 / 255.0,
                k_pix[1] as f32 / 255.0,
                k_pix[2] as f32 / 255.0,
            );
            let k_cov = compute_coverage(x_f, y_f, k_cos, k_sin, k_ink);

            // Subtractive ink absorption on paper
            let r_out = (pr * (1.0 - c_cov * 0.94) * (1.0 - k_cov * 0.97)).clamp(0.0, 1.0);
            let g_out = (pg * (1.0 - m_cov * 0.94) * (1.0 - k_cov * 0.97)).clamp(0.0, 1.0);
            let b_out = (pb * (1.0 - y_cov * 0.94) * (1.0 - k_cov * 0.97)).clamp(0.0, 1.0);

            let orig_a = img.get_pixel(x, y)[3];
            *pixel = Rgba([
                (r_out * 255.0).round() as u8,
                (g_out * 255.0).round() as u8,
                (b_out * 255.0).round() as u8,
                orig_a,
            ]);
        }
    });

    (!super::is_cancelled(cancel)).then_some(out)
}
