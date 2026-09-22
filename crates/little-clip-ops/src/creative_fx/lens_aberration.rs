//! Lens chromatic aberration effect.

use super::{DepthMaskConfig, sample_bilinear_channel};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LensAberrationParams {
    /// Aberration magnitude (0.0 to 100.0). Default is 25.0.
    pub strength: f32,
    /// Falloff distribution (0.0 to 100.0).
    /// At 0, aberration is applied through the entire image.
    /// At 100, aberration is only near the edges like a physical lens. Default is 50.0.
    pub falloff: f32,
    /// Color balance (-50.0 to +50.0). Default is 0.0.
    /// At +50, aberration is blue/yellow; at -50, aberration is red/blue.
    pub color_balance: f32,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for LensAberrationParams {
    fn default() -> Self {
        Self {
            strength: 25.0,
            falloff: 50.0,
            color_balance: 0.0,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Apply lens-shaped chromatic aberration to an RGBA image.
pub fn apply_lens_aberration(img: &RgbaImage, params: &LensAberrationParams) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || params.strength <= 0.001 {
        return img.clone();
    }

    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let max_radius = (cx * cx + cy * cy).sqrt().max(1.0);

    // Maximum displacement in pixels (scale with image diagonal)
    let max_disp = (params.strength / 100.0) * (w.max(h) as f32) * 0.035;
    let falloff_ratio = (params.falloff / 100.0).clamp(0.0, 1.0);

    // Color balance factors for R, G, B
    let cb = (params.color_balance / 50.0).clamp(-1.0, 1.0);
    let (s_r, s_g, s_b) = if cb >= 0.0 {
        // cb = 0: R=+1, G=0, B=-1 (balanced)
        // cb = +1: R=-1, G=-1, B=+1 (blue vs yellow)
        (1.0 - 2.0 * cb, -cb, -1.0 + 2.0 * cb)
    } else {
        // cb < 0: R=+1, B=-1, G blends towards cyan/red
        let t = -cb;
        (1.0, -0.5 * t, -1.0)
    };

    let mut out = RgbaImage::new(w, h);

    out.enumerate_rows_mut().par_bridge().for_each(|(y, row)| {
        let dy = y as f32 - cy;
        for (x, _, pixel) in row {
            let dx = x as f32 - cx;
            let r = (dx * dx + dy * dy).sqrt();
            let r_norm = (r / max_radius).clamp(0.0, 1.0);

            // Unit direction from optical center
            let (ux, uy) = if r > 1e-4 {
                (dx / r, dy / r)
            } else {
                (0.0, 0.0)
            };

            // Falloff interpolation:
            // At falloff 0: W(r) = 1.0 (uniform across the entire image)
            // At falloff 100: W(r) = r_norm^2 (only near edges)
            let w_r = (1.0 - falloff_ratio) * 1.0 + falloff_ratio * (r_norm * r_norm);
            let disp = max_disp * w_r;

            // Sample R, G, B with their respective displacements
            let rx = x as f32 + ux * disp * s_r;
            let ry = y as f32 + uy * disp * s_r;

            let gx = x as f32 + ux * disp * s_g;
            let gy = y as f32 + uy * disp * s_g;

            let bx = x as f32 + ux * disp * s_b;
            let by = y as f32 + uy * disp * s_b;

            let r_val = sample_bilinear_channel(img, rx, ry, 0);
            let g_val = sample_bilinear_channel(img, gx, gy, 1);
            let b_val = sample_bilinear_channel(img, bx, by, 2);
            let a_val = img.get_pixel(x, y)[3];

            *pixel = Rgba([r_val, g_val, b_val, a_val]);
        }
    });

    out
}
