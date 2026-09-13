//! 4-Point Perspective Deskew & Projective Homography Rectification.
//!
//! Enables un-skewing and rectifying photos taken at an angle (documents, whiteboards, screens, signs)
//! into flat, rectangular images using 8-parameter projective mapping and bilinear interpolation.

use image::{Rgba, RgbaImage};

/// 4 ordered corner points of a quadrangle in the image.
///
/// Order: Top-Left (0), Top-Right (1), Bottom-Right (2), Bottom-Left (3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadCorners {
    pub top_left: [f32; 2],
    pub top_right: [f32; 2],
    pub bottom_right: [f32; 2],
    pub bottom_left: [f32; 2],
}

impl QuadCorners {
    pub fn new(top_left: [f32; 2], top_right: [f32; 2], bottom_right: [f32; 2], bottom_left: [f32; 2]) -> Self {
        Self {
            top_left,
            top_right,
            bottom_right,
            bottom_left,
        }
    }

    /// Default quad inset from image boundaries by a margin fraction (e.g. 0.05 for 5%).
    pub fn default_inset(width: f32, height: f32, margin_frac: f32) -> Self {
        let mx = width * margin_frac;
        let my = height * margin_frac;
        Self {
            top_left: [mx, my],
            top_right: [width - mx, my],
            bottom_right: [width - mx, height - my],
            bottom_left: [mx, height - my],
        }
    }

    /// Array of 4 points in order: TL, TR, BR, BL.
    pub fn as_array(&self) -> [[f32; 2]; 4] {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left]
    }

    /// Set corner by index (0: TL, 1: TR, 2: BR, 3: BL).
    pub fn set_corner(&mut self, idx: usize, pt: [f32; 2]) {
        match idx {
            0 => self.top_left = pt,
            1 => self.top_right = pt,
            2 => self.bottom_right = pt,
            3 => self.bottom_left = pt,
            _ => {}
        }
    }

    /// Estimate natural rectified width and height based on the average/maximum edge lengths.
    pub fn estimate_dimensions(&self) -> (u32, u32) {
        let dist = |p1: [f32; 2], p2: [f32; 2]| -> f32 {
            let dx = p2[0] - p1[0];
            let dy = p2[1] - p1[1];
            (dx * dx + dy * dy).sqrt()
        };

        let top_w = dist(self.top_left, self.top_right);
        let bottom_w = dist(self.bottom_left, self.bottom_right);
        let left_h = dist(self.top_left, self.bottom_left);
        let right_h = dist(self.top_right, self.bottom_right);

        let w = (top_w.max(bottom_w) + 1.0).round().max(4.0) as u32;
        let h = (left_h.max(right_h) + 1.0).round().max(4.0) as u32;

        (w, h)
    }

    /// Estimate minimal-crop dimensions that preserve the entire source image scene after deskewing.
    pub fn estimate_minimal_crop_dimensions(
        &self,
        src_w: u32,
        src_h: u32,
        target_w: u32,
        target_h: u32,
    ) -> (u32, u32) {
        if let Some(homography) = UnitToQuadHomography::from_quad(self) {
            let (s_min, s_max, t_min, t_max) = homography.minimal_crop_unit_bounds(src_w, src_h);
            let span_s = (s_max - s_min).max(1.0);
            let span_t = (t_max - t_min).max(1.0);
            let w = ((span_s * target_w as f64).round() as u32).clamp(target_w, 8192);
            let h = ((span_t * target_h as f64).round() as u32).clamp(target_h, 8192);
            (w, h)
        } else {
            (target_w, target_h)
        }
    }
}

/// Cropping strategy when rectifying perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeskewCropMode {
    /// Crops tightly to the quad boundaries, discarding surrounding content.
    #[default]
    CropToQuad,
    /// Preserves the entire rectified scene, cropping only as strictly necessary around the transformed bounds.
    MinimalCrop,
}

impl DeskewCropMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CropToQuad => "Crop to Quad",
            Self::MinimalCrop => "Minimal Crop",
        }
    }
}

/// Projective homography mapping coefficients from unit square [0,1]^2 to arbitrary quad.
#[derive(Debug, Clone, Copy)]
struct UnitToQuadHomography {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
}

impl UnitToQuadHomography {
    /// Compute mapping from unit square [0,1]^2 to quad (x0,y0)..(x3,y3).
    fn from_quad(corners: &QuadCorners) -> Option<Self> {
        let x0 = corners.top_left[0] as f64;
        let y0 = corners.top_left[1] as f64;
        let x1 = corners.top_right[0] as f64;
        let y1 = corners.top_right[1] as f64;
        let x2 = corners.bottom_right[0] as f64;
        let y2 = corners.bottom_right[1] as f64;
        let x3 = corners.bottom_left[0] as f64;
        let y3 = corners.bottom_left[1] as f64;

        let dx1 = x1 - x2;
        let dx2 = x3 - x2;
        let dx3 = x0 - x1 + x2 - x3;
        let dy1 = y1 - y2;
        let dy2 = y3 - y2;
        let dy3 = y0 - y1 + y2 - y3;

        let (a, b, c, d, e, f, g, h) = if dx3.abs() < 1e-9 && dy3.abs() < 1e-9 {
            // Affine parallelogram
            (x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0.0, 0.0)
        } else {
            let det = dx1 * dy2 - dx2 * dy1;
            if det.abs() < 1e-12 {
                return None;
            }
            let g = (dx3 * dy2 - dx2 * dy3) / det;
            let h = (dx1 * dy3 - dx3 * dy1) / det;
            let a = x1 - x0 + g * x1;
            let b = x3 - x0 + h * x3;
            let c = x0;
            let d = y1 - y0 + g * y1;
            let e = y3 - y0 + h * y3;
            let f = y0;
            (a, b, c, d, e, f, g, h)
        };

        Some(Self { a, b, c, d, e, f, g, h })
    }

    /// Map unit square coordinate (s, t) in [0, 1]^2 to source image pixel coordinate (x, y).
    #[inline(always)]
    fn map(&self, s: f64, t: f64) -> (f32, f32) {
        let denom = self.g * s + self.h * t + 1.0;
        if denom.abs() < 1e-12 {
            return (self.c as f32, self.f as f32);
        }
        let x = (self.a * s + self.b * t + self.c) / denom;
        let y = (self.d * s + self.e * t + self.f) / denom;
        (x as f32, y as f32)
    }

    /// Map source image pixel coordinate (x, y) to unit square coordinate (s, t).
    fn map_inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let a_inv = self.e - self.f * self.h;
        let b_inv = self.c * self.h - self.b;
        let c_inv = self.b * self.f - self.c * self.e;

        let d_inv = self.f * self.g - self.d;
        let e_inv = self.a - self.c * self.g;
        let f_inv = self.c * self.d - self.a * self.f;

        let g_inv = self.d * self.h - self.e * self.g;
        let h_inv = self.b * self.g - self.a * self.h;
        let k_inv = self.a * self.e - self.b * self.d;

        let k_prime = g_inv * x + h_inv * y + k_inv;
        if k_prime.abs() < 1e-6 {
            return None;
        }

        let s_prime = a_inv * x + b_inv * y + c_inv;
        let t_prime = d_inv * x + e_inv * y + f_inv;

        Some((s_prime / k_prime, t_prime / k_prime))
    }

    /// Calculate (s_min, s_max, t_min, t_max) in unit-quad space that encloses the entire source image.
    fn minimal_crop_unit_bounds(&self, src_w: u32, src_h: u32) -> (f64, f64, f64, f64) {
        let mut s_min = 0.0f64;
        let mut s_max = 1.0f64;
        let mut t_min = 0.0f64;
        let mut t_max = 1.0f64;

        let w_f = src_w as f64;
        let h_f = src_h as f64;

        let sample_pts = [
            (0.0, 0.0),
            (w_f * 0.5, 0.0),
            (w_f, 0.0),
            (w_f, h_f * 0.5),
            (w_f, h_f),
            (w_f * 0.5, h_f),
            (0.0, h_f),
            (0.0, h_f * 0.5),
        ];

        for &(sx, sy) in &sample_pts {
            if let Some((s, t)) = self.map_inverse(sx, sy) {
                if s.is_finite() && t.is_finite() && s >= -5.0 && s <= 6.0 && t >= -5.0 && t <= 6.0 {
                    s_min = s_min.min(s);
                    s_max = s_max.max(s);
                    t_min = t_min.min(t);
                    t_max = t_max.max(t);
                }
            }
        }

        (s_min, s_max, t_min, t_max)
    }
}

/// Bilinear interpolation sampler with boundary clamping.
#[inline(always)]
fn sample_bilinear(src: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let w = src.width() as i32;
    let h = src.height() as i32;

    let x_clamped = x.clamp(0.0, (w - 1) as f32);
    let y_clamped = y.clamp(0.0, (h - 1) as f32);

    let x0 = x_clamped.floor() as i32;
    let y0 = y_clamped.floor() as i32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);

    let fx = x_clamped - x0 as f32;
    let fy = y_clamped - y0 as f32;
    let one_fx = 1.0 - fx;
    let one_fy = 1.0 - fy;

    let p00 = src.get_pixel(x0 as u32, y0 as u32).0;
    let p10 = src.get_pixel(x1 as u32, y0 as u32).0;
    let p01 = src.get_pixel(x0 as u32, y1 as u32).0;
    let p11 = src.get_pixel(x1 as u32, y1 as u32).0;

    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = one_fx * (p00[c] as f32) + fx * (p10[c] as f32);
        let bottom = one_fx * (p01[c] as f32) + fx * (p11[c] as f32);
        let val = one_fy * top + fy * bottom;
        out[c] = val.clamp(0.0, 255.0).round() as u8;
    }

    Rgba(out)
}

/// Rectify a perspective quad from `src` with the chosen cropping mode.
pub fn rectify_perspective_with_mode(
    src: &RgbaImage,
    corners: &QuadCorners,
    target_w: u32,
    target_h: u32,
    mode: DeskewCropMode,
) -> Result<RgbaImage, String> {
    if target_w < 2 || target_h < 2 {
        return Err("Target dimensions must be at least 2x2".to_string());
    }

    let homography = UnitToQuadHomography::from_quad(corners)
        .ok_or_else(|| "Degenerate quad: points are collinear or self-intersecting".to_string())?;

    let (s_min, s_max, t_min, t_max, out_w, out_h) = match mode {
        DeskewCropMode::CropToQuad => (0.0, 1.0, 0.0, 1.0, target_w, target_h),
        DeskewCropMode::MinimalCrop => {
            let (s_min, s_max, t_min, t_max) = homography.minimal_crop_unit_bounds(src.width(), src.height());
            let span_s = (s_max - s_min).max(1.0);
            let span_t = (t_max - t_min).max(1.0);
            let w = ((span_s * target_w as f64).round() as u32).clamp(target_w, 8192);
            let h = ((span_t * target_h as f64).round() as u32).clamp(target_h, 8192);
            (s_min, s_max, t_min, t_max, w, h)
        }
    };

    let mut dst = RgbaImage::new(out_w, out_h);

    let max_u = (out_w - 1) as f64;
    let max_v = (out_h - 1) as f64;
    let span_s = s_max - s_min;
    let span_t = t_max - t_min;

    let src_w_f = src.width() as f32;
    let src_h_f = src.height() as f32;

    for v in 0..out_h {
        let t = if max_v <= 0.0 { t_min } else { t_min + (v as f64 / max_v) * span_t };
        for u in 0..out_w {
            let s = if max_u <= 0.0 { s_min } else { s_min + (u as f64 / max_u) * span_s };
            let (src_x, src_y) = homography.map(s, t);

            if src_x >= 0.0 && src_x <= src_w_f - 1.0 && src_y >= 0.0 && src_y <= src_h_f - 1.0 {
                let pixel = sample_bilinear(src, src_x, src_y);
                dst.put_pixel(u, v, pixel);
            } else {
                dst.put_pixel(u, v, Rgba([0, 0, 0, 0]));
            }
        }
    }

    Ok(dst)
}

/// Rectify a perspective quad from `src` into a flat rectangular image of size `(target_w, target_h)` (crops to quad).
pub fn rectify_perspective(
    src: &RgbaImage,
    corners: &QuadCorners,
    target_w: u32,
    target_h: u32,
) -> Result<RgbaImage, String> {
    rectify_perspective_with_mode(src, corners, target_w, target_h, DeskewCropMode::CropToQuad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_rectification() {
        let mut src = RgbaImage::new(100, 80);
        for (x, y, p) in src.enumerate_pixels_mut() {
            *p = Rgba([(x * 2) as u8, (y * 3) as u8, 128, 255]);
        }

        let corners = QuadCorners::new(
            [0.0, 0.0],
            [99.0, 0.0],
            [99.0, 79.0],
            [0.0, 79.0],
        );

        let (w, h) = corners.estimate_dimensions();
        assert_eq!(w, 100);
        assert_eq!(h, 80);

        let dst = rectify_perspective(&src, &corners, 100, 80).expect("Rectification failed");
        assert_eq!(dst.width(), 100);
        assert_eq!(dst.height(), 80);

        // Center pixel should match closely
        let src_px = src.get_pixel(50, 40);
        let dst_px = dst.get_pixel(50, 40);
        assert_eq!(src_px[0], dst_px[0]);
        assert_eq!(src_px[1], dst_px[1]);
        assert_eq!(src_px[2], dst_px[2]);
        assert_eq!(src_px[3], dst_px[3]);
    }

    #[test]
    fn test_trapezoid_deskew() {
        let mut src = RgbaImage::new(200, 200);
        for p in src.pixels_mut() {
            *p = Rgba([255, 255, 255, 255]);
        }

        // Draw a trapezoid simulating an angled document
        let corners = QuadCorners::new(
            [40.0, 20.0],  // TL
            [160.0, 20.0], // TR
            [190.0, 180.0],// BR
            [10.0, 180.0], // BL
        );

        let (w, h) = corners.estimate_dimensions();
        assert!(w >= 120);
        assert!(h >= 160);

        let dst = rectify_perspective(&src, &corners, w, h).expect("Deskew failed");
        assert_eq!(dst.width(), w);
        assert_eq!(dst.height(), h);
    }
}
