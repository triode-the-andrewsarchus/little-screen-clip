//! Full-resolution document resampling for normalized neural backward maps.

use image::{Rgba, RgbaImage};

/// A channel-first floating-point map with validated dimensions and values.
#[derive(Clone, Copy)]
pub struct PlanarMap<'a, const CHANNELS: usize> {
    values: &'a [f32],
    width: usize,
    height: usize,
}

impl<'a, const CHANNELS: usize> PlanarMap<'a, CHANNELS> {
    /// Creates a channel-first map from finite values in row-major order.
    ///
    /// # Errors
    ///
    /// Returns an error for dimensions smaller than two, an inconsistent buffer
    /// length, dimension overflow, or any non-finite value.
    pub fn new(values: &'a [f32], width: usize, height: usize) -> Result<Self, String> {
        let expected = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(CHANNELS));
        if width < 2 || height < 2 || expected != Some(values.len()) {
            return Err("Invalid document map dimensions".into());
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err("Document map contains non-finite values".into());
        }
        Ok(Self {
            values,
            width,
            height,
        })
    }

    /// Bilinear sampling with edge padding; coordinates are in map pixels.
    fn sample(&self, channel: usize, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.width - 1) as f32);
        let y = y.clamp(0.0, (self.height - 1) as f32);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let at = |xx, yy| self.values[(channel * self.height + yy) * self.width + xx];
        let top = at(x0, y0) * (1.0 - (x - x0 as f32)) + at(x1, y0) * (x - x0 as f32);
        let bottom = at(x0, y1) * (1.0 - (x - x0 as f32)) + at(x1, y1) * (x - x0 as f32);
        top * (1.0 - (y - y0 as f32)) + bottom * (y - y0 as f32)
    }
}

struct Source<'a> {
    image: &'a RgbaImage,
    shadow: Option<PlanarMap<'a, 3>>,
    brightness_scale: f32,
    border: u8,
}

impl Source<'_> {
    fn pixel(&self, x: i64, y: i64) -> [f32; 4] {
        if x < 0 || y < 0 || x >= self.image.width() as i64 || y >= self.image.height() as i64 {
            return [
                self.border as f32,
                self.border as f32,
                self.border as f32,
                255.0,
            ];
        }
        let mut pixel = self.image.get_pixel(x as u32, y as u32).0.map(f32::from);
        if let Some(shadow) = self.shadow {
            // PyTorch interpolate(..., align_corners=False), in source coordinates.
            let sx = (x as f32 + 0.5) * shadow.width as f32 / self.image.width() as f32 - 0.5;
            let sy = (y as f32 + 0.5) * shadow.height as f32 / self.image.height() as f32 - 0.5;
            for (channel, value) in pixel[..3].iter_mut().enumerate() {
                let light = shadow.sample(channel, sx, sy) * 0.5 + 0.5;
                let divisor =
                    (light * self.brightness_scale + 1.0 - self.brightness_scale).max(0.1);
                *value = (*value / divisor).clamp(0.0, 255.0);
            }
        }
        pixel
    }

    fn sample(&self, x: f32, y: f32) -> Rgba<u8> {
        // An out-of-bounds grid is padded, not clamped to the page edge.
        if x < -1.0 || y < -1.0 || x > self.image.width() as f32 || y > self.image.height() as f32 {
            return Rgba([self.border, self.border, self.border, 255]);
        }
        let x0 = x.floor() as i64;
        let y0 = y.floor() as i64;
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let points = [
            (x0, y0, (1.0 - fx) * (1.0 - fy)),
            (x0 + 1, y0, fx * (1.0 - fy)),
            (x0, y0 + 1, (1.0 - fx) * fy),
            (x0 + 1, y0 + 1, fx * fy),
        ];
        let mut sum = [0.0; 4];
        for (xx, yy, weight) in points {
            let p = self.pixel(xx, yy);
            sum[3] += p[3] * weight;
            for c in 0..3 {
                sum[c] += p[c] * (p[3] / 255.0) * weight;
            }
        }
        let alpha = sum[3];
        let mut result = [0; 4];
        if alpha > 0.0 {
            for c in 0..3 {
                result[c] = (sum[c] * 255.0 / alpha).clamp(0.0, 255.0) as u8;
            }
        }
        result[3] = alpha.round().clamp(0.0, 255.0) as u8;
        Rgba(result)
    }
}

/// Unwarps to the source dimensions, optionally correcting SyntheticDoc shading.
///
/// Grid channels are normalized source x/y coordinates. Grid interpolation and
/// source sampling both use `align_corners=True`. `border` is 0 for UVDoc and
/// 128 for SyntheticDoc's zero-padded, [-1, 1] image representation. Alpha is
/// interpolated in premultiplied space, independently of illumination correction.
///
/// # Errors
///
/// Returns an error if the source exceeds the shared raster allocation/dimension
/// limits, is smaller than two pixels on either axis, or is cancelled. Checks
/// cancellation between rows, including during brightness measurement.
pub fn remap_document(
    image: &RgbaImage,
    grid: PlanarMap<'_, 2>,
    shadow: Option<PlanarMap<'_, 3>>,
    border: u8,
    cancelled: &impl Fn() -> bool,
) -> Result<RgbaImage, String> {
    little_clip_core::item::raster_preview_limits()
        .reserve_buffer(image.width(), image.height(), image::ColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    if image.width() < 2 || image.height() < 2 {
        return Err("Document image must be at least 2 × 2 pixels".into());
    }
    let mut source = Source {
        image,
        shadow,
        brightness_scale: 0.0,
        border,
    };
    if shadow.is_some() {
        let mut total = 0_u64;
        for row in image.rows() {
            if cancelled() {
                return Err("Document unwarp cancelled".into());
            }
            total += row
                .map(|p| p.0[..3].iter().map(|&v| v as u64).sum::<u64>())
                .sum::<u64>();
        }
        let brightness =
            total as f64 / (image.width() as f64 * image.height() as f64 * 3.0 * 255.0);
        source.brightness_scale = (0.4 / brightness.max(1e-6)).min(1.0) as f32;
    }
    let mut output = RgbaImage::new(image.width(), image.height());
    for (y, row) in output.rows_mut().enumerate() {
        if cancelled() {
            return Err("Document unwarp cancelled".into());
        }
        let gy = y as f32 * (grid.height - 1) as f32 / (image.height() - 1) as f32;
        for (x, pixel) in row.enumerate() {
            let gx = x as f32 * (grid.width - 1) as f32 / (image.width() - 1) as f32;
            let sx = (grid.sample(0, gx, gy) + 1.0) * 0.5 * (image.width() - 1) as f32;
            let sy = (grid.sample(1, gx, gy) + 1.0) * 0.5 * (image.height() - 1) as f32;
            *pixel = source.sample(sx, sy);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    const IDENTITY: [f32; 8] = [-1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0];

    #[test]
    fn identity_grid_preserves_original_dimensions_and_pixels() {
        let source =
            RgbaImage::from_fn(7, 5, |x, y| Rgba([(x * 30) as u8, (y * 40) as u8, 99, 255]));
        let output = remap_document(
            &source,
            PlanarMap::new(&IDENTITY, 2, 2).unwrap(),
            None,
            0,
            &|| false,
        )
        .unwrap();
        assert_eq!(output.dimensions(), source.dimensions());
        for (a, b) in output.as_raw().iter().zip(source.as_raw()) {
            assert!(a.abs_diff(*b) <= 1);
        }
    }

    #[test]
    fn transparent_samples_do_not_bleed_hidden_rgb_and_shading_preserves_alpha() {
        let source = RgbaImage::from_fn(2, 2, |x, _| {
            if x == 0 {
                Rgba([255, 0, 0, 0])
            } else {
                Rgba([0, 0, 100, 128])
            }
        });
        let grid = [0.0; 8];
        let shadow = [0.0; 12];
        let output = remap_document(
            &source,
            PlanarMap::new(&grid, 2, 2).unwrap(),
            Some(PlanarMap::new(&shadow, 2, 2).unwrap()),
            0,
            &|| false,
        )
        .unwrap();
        assert_eq!(*output.get_pixel(0, 0), Rgba([0, 0, 200, 64]));
    }

    #[test]
    fn invalid_maps_and_cancellation_are_errors() {
        assert!(PlanarMap::<2>::new(&[f32::NAN; 8], 2, 2).is_err());
        assert!(PlanarMap::<2>::new(&IDENTITY, usize::MAX, 2).is_err());
        let source = RgbaImage::new(4, 4);
        assert!(
            remap_document(
                &source,
                PlanarMap::new(&IDENTITY, 2, 2).unwrap(),
                None,
                0,
                &|| true
            )
            .is_err()
        );
    }

    #[test]
    fn outside_grid_uses_each_models_normalized_padding() {
        let source = RgbaImage::from_pixel(3, 3, Rgba([50, 60, 70, 255]));
        let outside = [10.0; 8];
        for border in [0, 128] {
            let output = remap_document(
                &source,
                PlanarMap::new(&outside, 2, 2).unwrap(),
                None,
                border,
                &|| false,
            )
            .unwrap();
            assert_eq!(*output.get_pixel(0, 0), Rgba([border, border, border, 255]));
        }
    }
}
