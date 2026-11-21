//! Display-only fill for source mosaics with unobserved areas.
use super::Id;
use image::{ImageBuffer, Rgb, RgbImage, Rgba, RgbaImage, imageops};
use std::ops::Range;

type Samples = ImageBuffer<Rgba<f32>, Vec<f32>>;

pub(super) fn albedo(source: RgbaImage, id: Id) -> Option<RgbImage> {
    // Only maps audited to contain uniform, zero-valued no-data use a black
    // mask. Iapetus, for example, has genuinely very dark terrain.
    let black_is_missing = matches!(
        id,
        199 | 401 | 501 | 701..=705 | 799 | 801 | 901 | 999 | 2000001
    );
    let rows = if id == 799 {
        // OPAL's three filters stop at different latitudes. Exclude the
        // resulting colored stripes at the southern edge of the mosaic.
        0..source.height() * 52 / 100
    } else {
        0..source.height()
    };
    let coverage = |pixel: &Rgba<u8>, y: u32| opacity(pixel, y, black_is_missing, &rows);
    if source
        .enumerate_pixels()
        .all(|(_, y, p)| coverage(p, y) == 1.0)
    {
        return Some(ImageBuffer::from_fn(
            source.width(),
            source.height(),
            |x, y| {
                let [r, g, b, _] = source.get_pixel(x, y).0;
                Rgb([r, g, b])
            },
        ));
    }

    let (width, height) = (source.width().min(192), source.height().min(96));
    let mut cells = Samples::new(width, height);
    let mut mean = [0.0; 4];
    for (x, y, pixel) in source.enumerate_pixels() {
        let valid = coverage(pixel, y);
        let cell = cells.get_pixel_mut(x * width / source.width(), y * height / source.height());
        for (i, total) in mean.iter_mut().enumerate() {
            let sample = if i == 3 {
                valid
            } else {
                valid * f32::from(pixel[i])
            };
            *total += sample;
            cell[i] += sample;
        }
    }
    if mean[3] == 0.0 {
        return None;
    }

    // Blur only observed colors, normalizing by the blurred coverage. Repeat
    // longitude before filtering so the antimeridian has the same neighbors
    // as every other meridian.
    // Samples use pixel values in the 0..1 range before the image filter.
    let scale = (width * height) as f32 / source.width() as f32 / source.height() as f32;
    let tiled = Samples::from_fn(width * 3, height, |x, y| {
        Rgba(std::array::from_fn(|i| {
            cells.get_pixel(x % width, y)[i] * scale / if i == 3 { 1.0 } else { 255.0 }
        }))
    });
    let blurred = imageops::blur(&tiled, (width as f32 * 0.035).max(0.7));
    let edge = imageops::blur(&tiled, (width as f32 * 0.011).max(0.65));
    let average: [f32; 3] = std::array::from_fn(|i| mean[i] / mean[3]);
    let infill = ImageBuffer::<Rgb<f32>, Vec<f32>>::from_fn(width * 3, height, |x, y| {
        let local = *blurred.get_pixel(x, y);
        let mirrored = *blurred.get_pixel(x, height - 1 - y);
        let copy = mix_sample(mirrored, average, 0.20);
        Rgb(mix_sample(local, copy, 0.08).map(|color| color / 255.0))
    });
    Some(ImageBuffer::from_fn(
        source.width(),
        source.height(),
        |x, y| {
            let pixel = source.get_pixel(x, y);
            let sx = (x as f32 + 0.5) * width as f32 / source.width() as f32 - 0.5 + width as f32;
            let sy = (y as f32 + 0.5) * height as f32 / source.height() as f32 - 0.5;
            let sy = sy.clamp(0.0, height as f32 - 1.0);
            let nearby = imageops::interpolate_bilinear(&edge, sx, sy).map_or(0.0, |p| p[3]);
            // Let the blurred estimate continue a short way into a mosaic,
            // where the last strip of observations often has a dark edge.
            let feather = ((nearby - 0.55) / 0.34).clamp(0.0, 1.0);
            let weight = coverage(pixel, y) * feather;
            let original = std::array::from_fn(|i| pixel[i]);
            if weight >= 1.0 {
                return Rgb(original);
            }
            let fill = imageops::interpolate_bilinear(&infill, sx, sy)
                .unwrap_or(Rgb(average.map(|v| v / 255.0)));
            Rgb(std::array::from_fn(|i| {
                (weight * f32::from(original[i]) + (1.0 - weight) * fill[i] * 255.0).round() as u8
            }))
        },
    ))
}

fn opacity(pixel: &Rgba<u8>, y: u32, black_is_missing: bool, rows: &Range<u32>) -> f32 {
    if !rows.contains(&y) {
        return 0.0;
    }
    let alpha = f32::from(pixel[3]) / 255.0;
    if !black_is_missing {
        return alpha;
    }
    let peak = pixel[0].max(pixel[1]).max(pixel[2]);
    alpha * ((f32::from(peak) - 8.0) / 12.0).clamp(0.0, 1.0)
}

fn mix_sample(sample: Rgba<f32>, fallback: [f32; 3], fade: f32) -> [f32; 3] {
    let coverage = sample[3];
    let blend = (coverage / fade).clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        let value = (255.0 * sample[i] / coverage.max(1e-6)).min(255.0);
        fallback[i] + (value - fallback[i]) * blend
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_mosaics_keep_observed_texels_and_fill_black_and_transparent_gaps() {
        let source = RgbaImage::from_fn(80, 40, |x, y| {
            if y < 12 {
                if x < 40 {
                    Rgba([0, 0, 0, 255])
                } else {
                    Rgba([255, 0, 0, 0])
                }
            } else {
                Rgba([80 + (x % 8) as u8 * 8, 100, 120, 255])
            }
        });
        let map = albedo(source, 705).unwrap();
        assert_eq!(map.get_pixel(15, 32).0, [136, 100, 120]);
        for x in [0, 20, 60, 79] {
            let [red, green, blue] = map.get_pixel(x, 4).0;
            assert!((86..130).contains(&red));
            assert!((95..=105).contains(&green));
            assert!((115..=125).contains(&blue));
        }
        assert!(map.get_pixel(0, 4)[0].abs_diff(map.get_pixel(79, 4)[0]) < 8);
    }

    #[test]
    fn a_missing_hemisphere_retains_blurred_variation_from_observed_terrain() {
        let source = RgbaImage::from_fn(128, 64, |x, y| {
            if y < 29 {
                Rgba([0, 0, 0, 255])
            } else if x < 64 {
                Rgba([72, 94, 110, 255])
            } else {
                Rgba([131, 153, 168, 255])
            }
        });
        let map = albedo(source, 705).unwrap();
        assert!(map.get_pixel(92, 10)[0] > map.get_pixel(34, 10)[0] + 45);
        assert!(map.get_pixel(0, 10)[0].abs_diff(map.get_pixel(127, 10)[0]) < 12);
    }

    #[test]
    fn iapetus_dark_terrain_is_observed_and_empty_maps_have_no_albedo() {
        let dark = RgbaImage::from_pixel(8, 4, Rgba([8, 8, 8, 255]));
        assert_eq!(albedo(dark, 608).unwrap().get_pixel(0, 0).0, [8; 3]);
        assert!(albedo(RgbaImage::new(8, 4), 705).is_none());
    }
}
