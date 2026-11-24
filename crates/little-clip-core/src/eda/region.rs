//! Bounded raster requests in document coordinates.

use tiny_skia::{Color, Pixmap, Transform};

/// A detail raster with a fixed physical origin and pixel density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterRegion {
    pub origin: [f64; 2],
    pub size: [u32; 2],
    pub pixels_per_mm: f64,
    pub y_down: bool,
}

impl RasterRegion {
    pub const MAX_SIDE: u32 = 1600;

    /// Pixel margin for a physical edge search and antialiasing.
    pub fn halo_pixels(tolerance_mm: f64, pixels_per_mm: f64) -> Option<u32> {
        let radius = (tolerance_mm * pixels_per_mm).ceil();
        (radius.is_finite() && (0.0..f64::from(Self::MAX_SIDE / 2 - 2)).contains(&radius))
            .then(|| radius as u32 + 2)
    }

    /// World coordinates of the opposite corner of the raster.
    pub fn far_corner(self) -> [f64; 2] {
        let direction = if self.y_down { 1.0 } else { -1.0 };
        [
            self.origin[0] + f64::from(self.size[0]) / self.pixels_per_mm,
            self.origin[1] + direction * f64::from(self.size[1]) / self.pixels_per_mm,
        ]
    }

    pub(super) fn transform(self) -> Result<Transform, String> {
        let [w, h] = self.size;
        let density = self.pixels_per_mm;
        if w == 0
            || h == 0
            || w > Self::MAX_SIDE
            || h > Self::MAX_SIDE
            || !density.is_finite()
            || density <= 0.0
        {
            return Err("Invalid EDA detail viewport".into());
        }
        let sy = if self.y_down { density } else { -density };
        let transform = Transform::from_row(
            density as f32,
            0.0,
            0.0,
            sy as f32,
            (-self.origin[0] * density) as f32,
            (-self.origin[1] * sy) as f32,
        );
        if !transform.is_valid() {
            return Err("Invalid EDA detail coordinates".into());
        }
        Ok(transform)
    }

    pub(super) fn canvas(self, bg: Color) -> Result<(Pixmap, Transform, f32), String> {
        let transform = self.transform()?;
        let [w, h] = self.size;
        let mut pixmap = Pixmap::new(w, h)
            .ok_or_else(|| "Could not allocate EDA detail viewport".to_string())?;
        pixmap.fill(bg);
        Ok((pixmap, transform, transform.sx))
    }

    pub(super) fn intersects(self, bounds: [f64; 4], padding: f64) -> bool {
        let far = self.far_corner();
        bounds[0] - padding <= far[0]
            && bounds[2] + padding >= self.origin[0]
            && bounds[1] - padding <= self.origin[1].max(far[1])
            && bounds[3] + padding >= self.origin[1].min(far[1])
    }
}
