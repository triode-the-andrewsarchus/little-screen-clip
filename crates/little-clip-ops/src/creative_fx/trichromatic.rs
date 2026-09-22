//! Trichromatic camera emulation effect.

use super::{
    DepthMaskConfig, oklab_to_srgb, sample_bilinear_channel, sample_bilinear_pixel, srgb_to_oklab,
};
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TrichromaticMode {
    #[default]
    Rgb,
    OkLab,
}

impl TrichromaticMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Rgb => "RGB (Red / Green / Blue)",
            Self::OkLab => "OKLab (Lightness / a / b)",
        }
    }

    pub fn channel_names(&self) -> [&'static str; 3] {
        match self {
            Self::Rgb => ["Red Channel", "Green Channel", "Blue Channel"],
            Self::OkLab => ["L (Lightness)", "a (Green-Red)", "b (Blue-Yellow)"],
        }
    }
}

/// Geometric transform for a single channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChannelTransform {
    /// Horizontal shift in pixels (-100.0 to 100.0). Default is 0.0.
    pub offset_x: f32,
    /// Vertical shift in pixels (-100.0 to 100.0). Default is 0.0.
    pub offset_y: f32,
    /// Zoom factor (0.50 to 1.50). Default is 1.0.
    pub zoom: f32,
}

impl Default for ChannelTransform {
    fn default() -> Self {
        Self {
            offset_x: 0.0,
            offset_y: 0.0,
            zoom: 1.0,
        }
    }
}

/// Full trichromatic camera emulation parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrichromaticParams {
    pub mode: TrichromaticMode,
    /// Transforms for channels [0, 1, 2].
    pub channels: [ChannelTransform; 3],
    /// Whether to automatically crop to the intersection window containing all 3 channels.
    pub auto_crop: bool,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for TrichromaticParams {
    fn default() -> Self {
        Self {
            mode: TrichromaticMode::Rgb,
            channels: [
                ChannelTransform {
                    offset_x: -3.0,
                    offset_y: 0.0,
                    zoom: 1.00,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    zoom: 1.00,
                },
                ChannelTransform {
                    offset_x: 3.0,
                    offset_y: 0.0,
                    zoom: 1.00,
                },
            ],
            auto_crop: true,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Compute the intersection bounding box in output space containing all 3 channels.
pub fn trichromatic_intersection_box(
    w: u32,
    h: u32,
    channels: &[ChannelTransform; 3],
) -> (u32, u32, u32, u32) {
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;

    let mut x_min = f32::MIN;
    let mut x_max = f32::MAX;
    let mut y_min = f32::MIN;
    let mut y_max = f32::MAX;

    for ch in channels {
        let z = ch.zoom.clamp(0.2, 5.0);
        // Source coord is: xs = cx + (X - cx - dx) / z
        // Condition: 0 <= xs <= w - 1
        // => -cx * z + dx <= X - cx <= (w - 1 - cx) * z + dx
        let ch_x_min = cx + ch.offset_x - cx * z;
        let ch_x_max = cx + ch.offset_x + (w as f32 - 1.0 - cx) * z;

        let ch_y_min = cy + ch.offset_y - cy * z;
        let ch_y_max = cy + ch.offset_y + (h as f32 - 1.0 - cy) * z;

        x_min = x_min.max(ch_x_min);
        x_max = x_max.min(ch_x_max);
        y_min = y_min.max(ch_y_min);
        y_max = y_max.min(ch_y_max);
    }

    if x_max > x_min + 2.0 && y_max > y_min + 2.0 {
        let start_x = (x_min.ceil().max(0.0) as u32).min(w.saturating_sub(1));
        let end_x = (x_max.floor().max(start_x as f32) as u32).min(w);
        let start_y = (y_min.ceil().max(0.0) as u32).min(h.saturating_sub(1));
        let end_y = (y_max.floor().max(start_y as f32) as u32).min(h);

        let crop_w = end_x.saturating_sub(start_x).max(1);
        let crop_h = end_y.saturating_sub(start_y).max(1);
        (start_x, start_y, crop_w, crop_h)
    } else {
        (0, 0, w, h)
    }
}

/// Apply trichromatic camera emulation with 3-channel offsets, zoom, and auto-cropping.
pub fn apply_trichromatic(img: &RgbaImage, params: &TrichromaticParams) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
    }

    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;

    let (start_x, start_y, out_w, out_h) = if params.auto_crop {
        trichromatic_intersection_box(w, h, &params.channels)
    } else {
        (0, 0, w, h)
    };

    let mut out = RgbaImage::new(out_w, out_h);

    out.enumerate_rows_mut()
        .par_bridge()
        .for_each(|(out_y, row)| {
            let orig_y = (start_y + out_y) as f32;
            for (out_x, _, pixel) in row {
                let orig_x = (start_x + out_x) as f32;

                // Compute source coordinates for all 3 channels
                let mut src_coords = [(0.0_f32, 0.0_f32); 3];
                for i in 0..3 {
                    let ch = &params.channels[i];
                    let z = ch.zoom.clamp(0.2, 5.0);
                    let xs = cx + (orig_x - cx - ch.offset_x) / z;
                    let ys = cy + (orig_y - cy - ch.offset_y) / z;
                    src_coords[i] = (xs, ys);
                }

                match params.mode {
                    TrichromaticMode::Rgb => {
                        let r = sample_bilinear_channel(img, src_coords[0].0, src_coords[0].1, 0);
                        let g = sample_bilinear_channel(img, src_coords[1].0, src_coords[1].1, 1);
                        let b = sample_bilinear_channel(img, src_coords[2].0, src_coords[2].1, 2);
                        let a = sample_bilinear_channel(img, src_coords[1].0, src_coords[1].1, 3);
                        *pixel = Rgba([r, g, b, a]);
                    }
                    TrichromaticMode::OkLab => {
                        // Channel 0 -> Lightness (L)
                        let p0 = sample_bilinear_pixel(img, src_coords[0].0, src_coords[0].1);
                        let (l0, _, _) = srgb_to_oklab(
                            p0[0] as f32 / 255.0,
                            p0[1] as f32 / 255.0,
                            p0[2] as f32 / 255.0,
                        );

                        // Channel 1 -> Green-Red (a)
                        let p1 = sample_bilinear_pixel(img, src_coords[1].0, src_coords[1].1);
                        let (_, a1, _) = srgb_to_oklab(
                            p1[0] as f32 / 255.0,
                            p1[1] as f32 / 255.0,
                            p1[2] as f32 / 255.0,
                        );

                        // Channel 2 -> Blue-Yellow (b)
                        let p2 = sample_bilinear_pixel(img, src_coords[2].0, src_coords[2].1);
                        let (_, _, b2) = srgb_to_oklab(
                            p2[0] as f32 / 255.0,
                            p2[1] as f32 / 255.0,
                            p2[2] as f32 / 255.0,
                        );

                        let (r, g, b) = oklab_to_srgb(l0, a1, b2);
                        let a = p1[3];
                        *pixel = Rgba([
                            (r * 255.0).round().clamp(0.0, 255.0) as u8,
                            (g * 255.0).round().clamp(0.0, 255.0) as u8,
                            (b * 255.0).round().clamp(0.0, 255.0) as u8,
                            a,
                        ]);
                    }
                }
            }
        });

    out
}
