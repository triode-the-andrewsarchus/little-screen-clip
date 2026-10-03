//! Neutral photographic adjustments and an editable RGB tone curve.

use super::*;
use image::RgbaImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Defines one normalized input/output point on the RGB tone curve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    /// Input intensity in `0.0..=1.0`.
    pub input: f32,
    /// Output intensity in `0.0..=1.0`.
    pub output: f32,
}

/// Configures photographic adjustments, RGB curves, and masked enlargement.
///
/// Defaults preserve the original image. Enlargement requires an enabled mask
/// and a corresponding mask image. Cutouts grow about their coverage centroid
/// and cover existing pixels, keeping an opaque source canvas filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BasicControlsParams {
    /// Contrast adjustment in `-100.0..=100.0`, neutral at zero.
    pub contrast: f32,
    /// Saturation adjustment in `-100.0..=100.0`; -100 removes color.
    pub saturation: f32,
    /// Highlight adjustment in `-100.0..=100.0`, neutral at zero.
    pub highlights: f32,
    /// Shadow adjustment in `-100.0..=100.0`, neutral at zero.
    pub shadows: f32,
    /// RGB tone curve, evaluated after tone and saturation adjustments.
    pub curve: Vec<CurvePoint>,
    /// Spatial mask and foreground/background polarity.
    pub depth_mask: DepthMaskConfig,
    /// Maximum enlargement in `1.0..=4.0`; 1 means no zoom.
    pub masked_zoom: f32,
    /// Depth layer count in `1..=32`; removal alpha uses one cutout.
    pub depth_slices: u32,
    /// Exponent of the depth-to-enlargement ramp in `0.1..=4.0`.
    ///
    /// Slice `i` of `n` uses `1 + (masked_zoom - 1) * (i / n)^zoom_falloff`.
    /// Higher values concentrate enlargement toward the selected near region;
    /// inversion reverses which end of the depth gradient is selected.
    pub zoom_falloff: f32,
}

impl Default for BasicControlsParams {
    fn default() -> Self {
        Self {
            contrast: 0.0,
            saturation: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            curve: vec![
                CurvePoint {
                    input: 0.0,
                    output: 0.0,
                },
                CurvePoint {
                    input: 1.0,
                    output: 1.0,
                },
            ],
            depth_mask: DepthMaskConfig::default(),
            masked_zoom: 1.0,
            depth_slices: 8,
            zoom_falloff: 1.0,
        }
    }
}

/// Returns a piecewise linear curve's output at an input intensity.
///
/// Points must have ordered, distinct input coordinates. Inputs outside the
/// curve use the nearest endpoint; an empty curve returns the input unchanged.
///
/// # Examples
///
/// ```
/// use little_clip_ops::creative_fx::{CurvePoint, tone_curve_value};
///
/// let curve = [
///     CurvePoint { input: 0.0, output: 0.0 },
///     CurvePoint { input: 0.5, output: 0.75 },
///     CurvePoint { input: 1.0, output: 1.0 },
/// ];
/// assert_eq!(tone_curve_value(&curve, 0.5), 0.75);
/// ```
pub fn tone_curve_value(points: &[CurvePoint], input: f32) -> f32 {
    let Some(first) = points.first() else {
        return input;
    };
    if input <= first.input {
        return first.output;
    }
    for pair in points.windows(2) {
        if input <= pair[1].input {
            let t = (input - pair[0].input) / (pair[1].input - pair[0].input);
            return pair[0].output + t * (pair[1].output - pair[0].output);
        }
    }
    points.last().map_or(input, |p| p.output)
}

pub(crate) fn apply_basic_controls_cancellable(
    img: &RgbaImage,
    params: &BasicControlsParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if is_cancelled(cancel) {
        return None;
    }
    let mut points = params.curve.clone();
    points.retain(|p| p.input.is_finite() && p.output.is_finite());
    for p in &mut points {
        p.input = p.input.clamp(0.0, 1.0);
        p.output = p.output.clamp(0.0, 1.0);
    }
    points.sort_by(|a, b| a.input.total_cmp(&b.input));
    points.dedup_by(|a, b| a.input == b.input);
    let curve: [f32; 256] = std::array::from_fn(|i| tone_curve_value(&points, i as f32 / 255.0));
    let contrast = (params.contrast.clamp(-100.0, 100.0) / 100.0).exp2();
    let saturation = 1.0 + params.saturation.clamp(-100.0, 100.0) / 100.0;
    let mut out = img.clone();
    out.as_mut().par_chunks_mut(4096).for_each(|chunk| {
        if is_cancelled(cancel) {
            return;
        }
        for pixel in chunk.as_chunks_mut::<4>().0 {
            let mut rgb =
                [pixel[0], pixel[1], pixel[2]].map(|v| (v as f32 / 255.0 - 0.5) * contrast + 0.5);
            let luma = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).clamp(0.0, 1.0);
            let tone = 0.5
                * (params.shadows.clamp(-100.0, 100.0) / 100.0 * (1.0 - luma).powi(2)
                    + params.highlights.clamp(-100.0, 100.0) / 100.0 * luma.powi(2));
            for c in 0..3 {
                rgb[c] = (luma + (rgb[c] - luma) * saturation + tone).clamp(0.0, 1.0);
                let index = rgb[c] * 255.0;
                let low = index.floor() as usize;
                let high = (low + 1).min(255);
                let value = curve[low] + (curve[high] - curve[low]) * index.fract();
                pixel[c] = (value * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
    if is_cancelled(cancel) {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn render(img: &RgbaImage, p: BasicControlsParams) -> RgbaImage {
        apply_creative_fx(img, &CreativeFxParams::BasicControls(p))
    }

    #[test]
    fn neutral_controls_preserve_every_channel_and_alpha() {
        let img = RgbaImage::from_fn(256, 8, |x, y| {
            Rgba([
                x as u8,
                (255 - x) as u8,
                (x * y % 256) as u8,
                (y * 31) as u8,
            ])
        });
        assert_eq!(render(&img, BasicControlsParams::default()), img);
    }

    #[test]
    fn photographic_controls_change_their_target_tones() {
        let img = RgbaImage::from_fn(3, 1, |x, _| {
            [
                Rgba([40, 40, 40, 123]),
                Rgba([210, 210, 210, 255]),
                Rgba([220, 100, 50, 255]),
            ][x as usize]
        });
        let contrast = render(
            &img,
            BasicControlsParams {
                contrast: 60.0,
                ..Default::default()
            },
        );
        assert!(contrast.get_pixel(0, 0)[0] < 40);
        assert!(contrast.get_pixel(1, 0)[0] > 210);
        let shadows = render(
            &img,
            BasicControlsParams {
                shadows: 60.0,
                ..Default::default()
            },
        );
        assert!(shadows.get_pixel(0, 0)[0] - 40 > shadows.get_pixel(1, 0)[0] - 210);
        let highlights = render(
            &img,
            BasicControlsParams {
                highlights: -60.0,
                ..Default::default()
            },
        );
        assert!(210 - highlights.get_pixel(1, 0)[0] > 40 - highlights.get_pixel(0, 0)[0]);
        let mono = render(
            &img,
            BasicControlsParams {
                saturation: -100.0,
                ..Default::default()
            },
        );
        let color = mono.get_pixel(2, 0);
        assert_eq!(color[0], color[1]);
        assert_eq!(color[1], color[2]);
        assert_eq!(mono.get_pixel(0, 0)[3], 123);
    }

    #[test]
    fn edited_curve_maps_midtones_and_sanitizes_serialized_points() {
        let img = RgbaImage::from_pixel(1, 1, Rgba([128, 128, 128, 255]));
        let curve = vec![
            CurvePoint {
                input: 1.0,
                output: 1.0,
            },
            CurvePoint {
                input: 0.5,
                output: 0.75,
            },
            CurvePoint {
                input: 0.0,
                output: 0.0,
            },
            CurvePoint {
                input: 0.5,
                output: 0.75,
            },
            CurvePoint {
                input: f32::NAN,
                output: 0.0,
            },
        ];
        let result = render(
            &img,
            BasicControlsParams {
                curve,
                ..Default::default()
            },
        );
        assert!((result.get_pixel(0, 0)[0] as i32 - 192).abs() <= 1);
    }

    #[test]
    fn basic_controls_use_depth_and_alpha_masks_with_both_polarities() {
        let img = RgbaImage::from_pixel(3, 1, Rgba([100, 100, 100, 255]));
        let depth = RgbaImage::from_fn(3, 1, |x, _| Rgba([if x == 0 { 0 } else { 255 }; 4]));
        let alpha = RgbaImage::from_fn(3, 1, |x, _| Rgba([0, 0, 0, if x == 0 { 0 } else { 255 }]));
        for source in [
            CreativeFxMaskSource::DepthMap,
            CreativeFxMaskSource::BackgroundRemovalAlpha,
        ] {
            for invert in [false, true] {
                let params = CreativeFxParams::BasicControls(BasicControlsParams {
                    shadows: 100.0,
                    depth_mask: DepthMaskConfig {
                        enabled: true,
                        source,
                        invert,
                        ..Default::default()
                    },
                    ..Default::default()
                });
                let out = apply_creative_fx_with_masks(&img, Some(&depth), Some(&alpha), &params);
                let untouched = if invert { 1 } else { 0 };
                let adjusted = if invert { 0 } else { 1 };
                assert_eq!(out.get_pixel(untouched, 0), img.get_pixel(untouched, 0));
                assert!(out.get_pixel(adjusted, 0)[0] > 100);
            }
        }
    }

    #[test]
    fn cutout_zoom_enlarges_foreground_without_holes_or_color_fringe() {
        let img = RgbaImage::from_fn(21, 9, |x, _| {
            if (8..=12).contains(&x) {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let alpha = RgbaImage::from_fn(21, 9, |x, _| {
            Rgba([0, 0, 0, if (8..=12).contains(&x) { 255 } else { 0 }])
        });
        let mut p = BasicControlsParams {
            masked_zoom: 2.0,
            depth_mask: DepthMaskConfig {
                enabled: true,
                source: CreativeFxMaskSource::BackgroundRemovalAlpha,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = apply_creative_fx_with_masks(
            &img,
            None,
            Some(&alpha),
            &CreativeFxParams::BasicControls(p.clone()),
        );
        assert_eq!(out.dimensions(), img.dimensions());
        assert!(out.pixels().all(|p| p[3] == 255));
        assert_eq!(*out.get_pixel(6, 4), Rgba([255, 0, 0, 255]));
        assert_eq!(out.get_pixel(0, 4), img.get_pixel(0, 4));
        p.masked_zoom = 1.0;
        assert_eq!(
            apply_creative_fx_with_masks(
                &img,
                None,
                Some(&alpha),
                &CreativeFxParams::BasicControls(p)
            ),
            img
        );
    }

    #[test]
    fn inverted_background_zoom_and_missing_masks_keep_a_filled_canvas() {
        let img = RgbaImage::from_fn(21, 9, |x, y| {
            Rgba([(x * 12) as u8, (y * 28) as u8, 90, 255])
        });
        let alpha = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 0]));
        let p = CreativeFxParams::BasicControls(BasicControlsParams {
            masked_zoom: 2.0,
            depth_mask: DepthMaskConfig {
                enabled: true,
                source: CreativeFxMaskSource::BackgroundRemovalAlpha,
                invert: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let out = apply_creative_fx_with_masks(&img, None, Some(&alpha), &p);
        assert_ne!(out, img);
        assert!(out.pixels().all(|p| p[3] == 255));
        assert_eq!(apply_creative_fx_with_masks(&img, None, None, &p), img);
    }

    #[test]
    fn depth_slice_count_falloff_and_inversion_change_enlargement() {
        let img = RgbaImage::from_fn(32, 12, |x, y| {
            Rgba([
                (x * 7) as u8,
                (y * 21) as u8,
                ((x * 13 + y * 19) % 256) as u8,
                255,
            ])
        });
        let depth = RgbaImage::from_fn(32, 12, |x, _| Rgba([(x * 255 / 31) as u8; 4]));
        let mut p = BasicControlsParams {
            masked_zoom: 2.0,
            depth_slices: 1,
            depth_mask: DepthMaskConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let run = |p: &BasicControlsParams| {
            apply_creative_fx_with_masks(
                &img,
                Some(&depth),
                None,
                &CreativeFxParams::BasicControls(p.clone()),
            )
        };
        let single = run(&p);
        p.depth_slices = 8;
        let layered = run(&p);
        assert_ne!(single, layered);
        p.zoom_falloff = 4.0;
        let falloff = run(&p);
        assert_ne!(layered, falloff);
        p.depth_mask.invert = true;
        let inverted = run(&p);
        assert_ne!(falloff, inverted);
        assert!(inverted.pixels().all(|p| p[3] == 255));
    }

    #[test]
    fn basic_controls_respect_cancellation_and_empty_images() {
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let params = CreativeFxParams::BasicControls(BasicControlsParams::default());
        assert!(
            apply_creative_fx_with_masks_cancellable(
                &RgbaImage::new(4, 4),
                None,
                None,
                &params,
                &cancel
            )
            .is_none()
        );
        assert_eq!(
            apply_creative_fx(&RgbaImage::new(0, 0), &params).dimensions(),
            (0, 0)
        );
    }
}
