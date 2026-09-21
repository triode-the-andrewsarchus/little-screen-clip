//! Creative FX image manipulation algorithms.
//!
//! Provides:
//! 1. **Lens Chromatic Aberration**: Optical radial dispersion with falloff and color balance controls.
//! 2. **Trichromatic Camera Emulation**: RGB and OKLab 3-channel separation with per-channel offset, zoom, and auto-crop.
//! 3. **Pixel Sorting**: Scanline sorting across horizontal, vertical, and diagonal orientations with brightness, hue, and saturation metrics.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Common Types & Traits
// ─────────────────────────────────────────────────────────────────────────────

/// Supported creative effect types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CreativeFxType {
    LensAberration,
    Trichromatic,
    PixelSort,
    CmykHalftone,
    DitherPalette,
    Ntsc,
    Structure,
}

impl CreativeFxType {
    pub fn all() -> &'static [CreativeFxType] {
        &[
            CreativeFxType::LensAberration,
            CreativeFxType::Trichromatic,
            CreativeFxType::PixelSort,
            CreativeFxType::CmykHalftone,
            CreativeFxType::DitherPalette,
            CreativeFxType::Ntsc,
            CreativeFxType::Structure,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::LensAberration => "Lens Aberration",
            Self::Trichromatic => "Trichromatic Camera",
            Self::PixelSort => "Pixel Sorting",
            Self::CmykHalftone => "CMYK Halftone",
            Self::DitherPalette => "Dither & Retro Palette",
            Self::Ntsc => "NTSC & VHS Video",
            Self::Structure => "Structure",
        }
    }

    pub fn subtitle(&self) -> &'static str {
        match self {
            Self::LensAberration => "Optical radial color fringing",
            Self::Trichromatic => "3-channel alignment & drift",
            Self::PixelSort => "Algorithmic scanline sorting",
            Self::CmykHalftone => "Offset press misregistration & dots",
            Self::DitherPalette => "Ordered dithering & classic hardware",
            Self::Ntsc => "Analog broadcast & magnetic tape emulation",
            Self::Structure => "Multiscale texture & edge protection",
        }
    }

    pub fn category(&self) -> &'static str {
        match self {
            Self::LensAberration => "OPTICAL",
            Self::Trichromatic => "MULTI-CHANNEL",
            Self::PixelSort => "GLITCH ART",
            Self::CmykHalftone => "PRINT & PRESS",
            Self::DitherPalette => "RETRO HARDWARE",
            Self::Ntsc => "ANALOG VIDEO",
            Self::Structure => "DETAIL & TEXTURE",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::LensAberration => "🔍",
            Self::Trichromatic => "📷",
            Self::PixelSort => "⚡",
            Self::CmykHalftone => "🗞",
            Self::DitherPalette => "👾",
            Self::Ntsc => "📺",
            Self::Structure => "🧱",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::LensAberration => {
                "Emulates real-world optical glass dispersion. Aberration increases radially towards \
                 the image edges with customizable strength, falloff, and color balance (blue/yellow vs red/blue)."
            }
            Self::Trichromatic => {
                "Emulates historic three-color separation plate photography (e.g. Prokudin-Gorsky) \
                 or experimental OKLab perceptual drift. Allows independent X/Y offsets and zoom per channel \
                 with automatic intersection cropping."
            }
            Self::PixelSort => {
                "Sorts pixels along scanline segments bounded by luminance, hue, or saturation thresholds, \
                 creating kinetic melting, streaks, and datamosh textures."
            }
            Self::CmykHalftone => {
                "Simulates vintage multi-color offset press lithography and newsprint comics. \
                 Renders Cyan, Magenta, Yellow, and Key dot screens at traditional rosette angles \
                 with mechanical plate misregistration."
            }
            Self::DitherPalette => {
                "Applies ordered Bayer matrices, blue-noise, or Floyd-Steinberg error diffusion \
                 quantized to iconic retro hardware palettes (Game Boy, Macintosh 1-bit, CGA, PICO-8, etc.) \
                 with configurable pixel scaling."
            }
            Self::Ntsc => {
                "Accurately emulates analog NTSC composite color television transmission and VHS cassette \
                 recording artifacts, including RF static snow, luma smear, chroma bleed, tracking bars, \
                 and head-switching jitter."
            }
            Self::Structure => {
                "Selectively amplifies naturally occurring fine-to-medium surface textures \
                 (fabric, foliage, stone, skin detail) while protecting high-contrast object \
                 boundaries from conventional sharpening halos and ringing. Supports negative \
                 values for controlled edge-preserving texture smoothing."
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ─────────────────────────────────────────────────────────────────────────────
// Depth Masking Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for masking creative effects using an AI-generated grayscale depth map.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DepthMaskConfig {
    /// Whether depth-based masking is enabled for this effect.
    pub enabled: bool,
    /// Invert depth polarity (near becomes far, far becomes near).
    pub invert: bool,
    /// Depth threshold cut-off (0.0 to 1.0). When 0.0, the full grayscale depth gradient is used continuously.
    pub threshold: f32,
    /// Edge feather softness around the depth threshold (0.0 to 1.0).
    pub feather: f32,
}

impl Default for DepthMaskConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            invert: false,
            threshold: 0.0,
            feather: 0.25,
        }
    }
}

// 1. Lens Chromatic Aberration
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for lens-shaped chromatic aberration.
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

// ─────────────────────────────────────────────────────────────────────────────
// 2. Trichromatic Camera Emulation
// ─────────────────────────────────────────────────────────────────────────────

/// Color separation space for trichromatic camera.
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

// ─────────────────────────────────────────────────────────────────────────────
// 3. Pixel Sorting
// ─────────────────────────────────────────────────────────────────────────────

/// Direction along which pixel sort scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PixelSortDirection {
    #[default]
    Vertical,
    Horizontal,
    Diagonal,
}

impl PixelSortDirection {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Vertical => "Vertical (Top to Bottom)",
            Self::Horizontal => "Horizontal (Left to Right)",
            Self::Diagonal => "Diagonal (45° Angle)",
        }
    }
}

/// Metric used to sort pixels within segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PixelSortCriteria {
    #[default]
    Brightness,
    Hue,
    Saturation,
}

impl PixelSortCriteria {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Brightness => "Brightness / Luminance",
            Self::Hue => "Hue (Rainbow color wheel)",
            Self::Saturation => "Color Saturation",
        }
    }
}

/// Sorting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PixelSortOrder {
    #[default]
    Ascending,
    Descending,
}

impl PixelSortOrder {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ascending => "Ascending (Low to High)",
            Self::Descending => "Descending (High to Low)",
        }
    }
}

/// Parameters for pixel sorting effect.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PixelSortParams {
    pub direction: PixelSortDirection,
    pub criteria: PixelSortCriteria,
    pub order: PixelSortOrder,
    /// Minimum threshold (0.0 to 1.0) for pixel inclusion in sort span.
    pub min_threshold: f32,
    /// Maximum threshold (0.0 to 1.0) for pixel inclusion in sort span.
    pub max_threshold: f32,
    /// Minimum contiguous span length in pixels required to trigger sorting.
    pub min_span: usize,
    /// Invert mask selection (sort outside threshold window).
    pub invert_mask: bool,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for PixelSortParams {
    fn default() -> Self {
        Self {
            direction: PixelSortDirection::Vertical,
            criteria: PixelSortCriteria::Brightness,
            order: PixelSortOrder::Ascending,
            min_threshold: 0.25,
            max_threshold: 0.80,
            min_span: 6,
            invert_mask: false,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Calculate sort metric value in [0.0, 1.0] for an RGBA pixel.
#[inline]
pub fn pixel_metric(pixel: Rgba<u8>, criteria: PixelSortCriteria) -> f32 {
    let r = pixel[0] as f32 / 255.0;
    let g = pixel[1] as f32 / 255.0;
    let b = pixel[2] as f32 / 255.0;

    match criteria {
        PixelSortCriteria::Brightness => {
            // Perceptual relative luminance (Rec. 709)
            0.2126 * r + 0.7152 * g + 0.0722 * b
        }
        PixelSortCriteria::Hue => {
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let delta = max - min;
            if delta < 1e-5 {
                0.0
            } else if (max - r).abs() < 1e-5 {
                let h = ((g - b) / delta).rem_euclid(6.0);
                h / 6.0
            } else if (max - g).abs() < 1e-5 {
                let h = (b - r) / delta + 2.0;
                h / 6.0
            } else {
                let h = (r - g) / delta + 4.0;
                h / 6.0
            }
        }
        PixelSortCriteria::Saturation => {
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            if max < 1e-5 { 0.0 } else { (max - min) / max }
        }
    }
}

/// Apply pixel sorting to an RGBA image.
pub fn apply_pixel_sort(img: &RgbaImage, params: &PixelSortParams) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
    }

    let mut out = img.clone();
    let min_th = params.min_threshold.min(params.max_threshold);
    let max_th = params.min_threshold.max(params.max_threshold);
    let min_span = params.min_span.max(2);
    let criteria = params.criteria;
    let ascending = params.order == PixelSortOrder::Ascending;
    let invert = params.invert_mask;

    let is_in_span = |p: Rgba<u8>| -> bool {
        let val = pixel_metric(p, criteria);
        let in_range = val >= min_th && val <= max_th;
        if invert { !in_range } else { in_range }
    };

    let sort_span = |pixels: &mut [Rgba<u8>]| {
        pixels.sort_unstable_by(|a, b| {
            let va = pixel_metric(*a, criteria);
            let vb = pixel_metric(*b, criteria);
            if ascending {
                va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                vb.partial_cmp(&va).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
    };

    match params.direction {
        PixelSortDirection::Horizontal => {
            for y in 0..h {
                let mut row_pixels = Vec::with_capacity(w as usize);
                for x in 0..w {
                    row_pixels.push(*out.get_pixel(x, y));
                }
                let len = row_pixels.len();
                let mut start = 0;
                while start < len {
                    if is_in_span(row_pixels[start]) {
                        let mut end = start + 1;
                        while end < len && is_in_span(row_pixels[end]) {
                            end += 1;
                        }
                        if end - start >= min_span {
                            sort_span(&mut row_pixels[start..end]);
                        }
                        start = end;
                    } else {
                        start += 1;
                    }
                }
                for (x, p) in row_pixels.into_iter().enumerate() {
                    out.put_pixel(x as u32, y, p);
                }
            }
        }
        PixelSortDirection::Vertical => {
            for x in 0..w {
                let mut col_pixels = Vec::with_capacity(h as usize);
                for y in 0..h {
                    col_pixels.push(*out.get_pixel(x, y));
                }
                let len = col_pixels.len();
                let mut start = 0;
                while start < len {
                    if is_in_span(col_pixels[start]) {
                        let mut end = start + 1;
                        while end < len && is_in_span(col_pixels[end]) {
                            end += 1;
                        }
                        if end - start >= min_span {
                            sort_span(&mut col_pixels[start..end]);
                        }
                        start = end;
                    } else {
                        start += 1;
                    }
                }
                for (y, p) in col_pixels.into_iter().enumerate() {
                    out.put_pixel(x, y as u32, p);
                }
            }
        }
        PixelSortDirection::Diagonal => {
            let max_d = w + h - 2;
            for d in 0..=max_d {
                let mut coords = Vec::new();
                let x_start = d.saturating_sub(h - 1);
                let x_end = d.min(w - 1);
                for x in x_start..=x_end {
                    let y = d - x;
                    coords.push((x, y));
                }
                if coords.len() < min_span {
                    continue;
                }

                let mut diag_pixels: Vec<Rgba<u8>> =
                    coords.iter().map(|&(x, y)| *out.get_pixel(x, y)).collect();
                let len = diag_pixels.len();
                let mut start = 0;
                while start < len {
                    if is_in_span(diag_pixels[start]) {
                        let mut end = start + 1;
                        while end < len && is_in_span(diag_pixels[end]) {
                            end += 1;
                        }
                        if end - start >= min_span {
                            sort_span(&mut diag_pixels[start..end]);
                        }
                        start = end;
                    } else {
                        start += 1;
                    }
                }
                for (i, &(x, y)) in coords.iter().enumerate() {
                    out.put_pixel(x, y, diag_pixels[i]);
                }
            }
        }
    }

    out
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. CMYK Press Misregistration & Rosette Halftone
// ─────────────────────────────────────────────────────────────────────────────

/// Paper tint substrate colors.
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
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
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

    out
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Bayer / Blue-Noise Dithering & Retro Palettes
// ─────────────────────────────────────────────────────────────────────────────

/// Iconic retro hardware & aesthetic color palettes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum RetroPalette {
    #[default]
    GameBoy,
    Macintosh1Bit,
    CgaMode1,
    CgaMode2,
    Pico8,
    Commodore64,
    Cyberpunk,
}

impl RetroPalette {
    pub fn all() -> &'static [RetroPalette] {
        &[
            RetroPalette::GameBoy,
            RetroPalette::Macintosh1Bit,
            RetroPalette::CgaMode1,
            RetroPalette::CgaMode2,
            RetroPalette::Pico8,
            RetroPalette::Commodore64,
            RetroPalette::Cyberpunk,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::GameBoy => "Game Boy (4 Greens)",
            Self::Macintosh1Bit => "1984 Mac (1-Bit B&W)",
            Self::CgaMode1 => "CGA Mode 1 (Cyan/Magenta)",
            Self::CgaMode2 => "CGA Mode 2 (Green/Red)",
            Self::Pico8 => "PICO-8 (16 Colors)",
            Self::Commodore64 => "Commodore 64 (16 Colors)",
            Self::Cyberpunk => "Cyberpunk Neon (8 Colors)",
        }
    }

    pub fn colors(&self) -> &'static [[u8; 3]] {
        match self {
            Self::GameBoy => &[
                [15, 56, 15],   // darkest olive
                [48, 98, 48],   // dark olive
                [139, 172, 15], // light olive
                [155, 188, 15], // lightest olive
            ],
            Self::Macintosh1Bit => &[[0, 0, 0], [255, 255, 255]],
            Self::CgaMode1 => &[
                [0, 0, 0],       // Black
                [85, 255, 255],  // Cyan
                [255, 85, 255],  // Magenta
                [255, 255, 255], // White
            ],
            Self::CgaMode2 => &[
                [0, 0, 0],      // Black
                [85, 255, 85],  // Green
                [255, 85, 85],  // Red
                [255, 255, 85], // Yellow
            ],
            Self::Pico8 => &[
                [0, 0, 0],       // Black
                [29, 43, 83],    // Storm
                [126, 37, 83],   // Wine
                [0, 135, 81],    // Moss
                [171, 82, 54],   // Tan
                [95, 87, 79],    // Slate
                [194, 195, 199], // Silver
                [255, 241, 232], // White
                [255, 0, 77],    // Red
                [255, 163, 0],   // Orange
                [255, 236, 39],  // Yellow
                [0, 228, 54],    // Green
                [41, 173, 255],  // Blue
                [131, 118, 156], // Indigo
                [255, 119, 168], // Pink
                [255, 204, 170], // Peach
            ],
            Self::Commodore64 => &[
                [0, 0, 0],       // Black
                [255, 255, 255], // White
                [136, 0, 0],     // Red
                [170, 255, 238], // Cyan
                [204, 68, 204],  // Purple
                [0, 204, 85],    // Green
                [0, 0, 170],     // Blue
                [238, 238, 119], // Yellow
                [221, 136, 85],  // Orange
                [102, 68, 0],    // Brown
                [255, 119, 119], // Light Red
                [51, 51, 51],    // Dark Grey
                [119, 119, 119], // Grey
                [170, 255, 102], // Light Green
                [0, 136, 255],   // Light Blue
                [187, 187, 187], // Light Grey
            ],
            Self::Cyberpunk => &[
                [13, 2, 33],     // Deep Obsidian
                [15, 8, 75],     // Midnight Blue
                [38, 64, 139],   // Electric Blue
                [166, 0, 103],   // Neon Magenta
                [255, 0, 127],   // Hot Pink
                [0, 240, 255],   // Acid Cyan
                [255, 230, 0],   // Laser Yellow
                [255, 255, 255], // Glitch White
            ],
        }
    }
}

/// Dithering methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DitherMethod {
    #[default]
    Bayer4x4,
    Bayer8x8,
    BlueNoise,
    FloydSteinberg,
    None,
}

impl DitherMethod {
    pub fn all() -> &'static [DitherMethod] {
        &[
            DitherMethod::Bayer4x4,
            DitherMethod::Bayer8x8,
            DitherMethod::BlueNoise,
            DitherMethod::FloydSteinberg,
            DitherMethod::None,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Bayer4x4 => "Bayer 4×4 Ordered",
            Self::Bayer8x8 => "Bayer 8×8 Ordered",
            Self::BlueNoise => "Blue Noise 16×16",
            Self::FloydSteinberg => "Floyd-Steinberg Error Diffusion",
            Self::None => "None (Nearest Match)",
        }
    }
}

/// Parameters for dithering and retro palette quantization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DitherPaletteParams {
    pub palette: RetroPalette,
    pub method: DitherMethod,
    /// Pixel grid scale (1 to 8). Default is 2.
    pub pixel_scale: u32,
    /// Dither noise / diffusion strength (0.0 to 1.0). Default is 0.8.
    pub dither_strength: f32,
    /// Contrast adjustment (-50.0 to +50.0). Default is 0.0.
    pub contrast: f32,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for DitherPaletteParams {
    fn default() -> Self {
        Self {
            palette: RetroPalette::GameBoy,
            method: DitherMethod::Bayer4x4,
            pixel_scale: 2,
            dither_strength: 0.8,
            contrast: 0.0,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

const BAYER_4X4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

const BAYER_8X8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

const BLUE_NOISE_16X16: [[u8; 16]; 16] = [
    [
        0, 192, 48, 240, 12, 204, 60, 252, 3, 195, 51, 243, 15, 207, 63, 255,
    ],
    [
        128, 64, 176, 112, 140, 76, 188, 124, 131, 67, 179, 115, 143, 79, 191, 127,
    ],
    [
        32, 224, 16, 208, 44, 236, 28, 220, 35, 227, 19, 211, 47, 239, 31, 223,
    ],
    [
        160, 96, 144, 80, 172, 108, 156, 92, 163, 99, 147, 83, 175, 111, 159, 95,
    ],
    [
        8, 200, 56, 248, 4, 196, 52, 244, 11, 203, 59, 251, 7, 199, 55, 247,
    ],
    [
        136, 72, 184, 120, 132, 68, 180, 116, 139, 75, 187, 123, 135, 71, 183, 119,
    ],
    [
        40, 232, 24, 216, 36, 228, 20, 212, 43, 235, 27, 219, 39, 231, 23, 215,
    ],
    [
        168, 104, 152, 88, 164, 100, 148, 84, 171, 107, 155, 91, 167, 103, 151, 87,
    ],
    [
        2, 194, 50, 242, 14, 206, 62, 254, 1, 193, 49, 241, 13, 205, 61, 253,
    ],
    [
        130, 66, 178, 114, 142, 78, 190, 126, 129, 65, 177, 113, 141, 77, 189, 125,
    ],
    [
        34, 226, 18, 210, 46, 238, 30, 222, 33, 225, 17, 209, 45, 237, 29, 221,
    ],
    [
        162, 98, 146, 82, 174, 110, 158, 94, 161, 97, 145, 81, 173, 109, 157, 93,
    ],
    [
        10, 202, 58, 250, 6, 198, 54, 246, 9, 201, 57, 249, 5, 197, 53, 245,
    ],
    [
        138, 74, 186, 122, 134, 70, 182, 118, 137, 73, 185, 121, 133, 69, 181, 117,
    ],
    [
        42, 234, 26, 218, 38, 230, 22, 214, 41, 233, 25, 217, 37, 229, 21, 213,
    ],
    [
        170, 106, 154, 90, 166, 102, 150, 86, 169, 105, 153, 89, 165, 101, 149, 85,
    ],
];

#[inline]
fn find_nearest_palette_color(r: f32, g: f32, b: f32, palette: &[[u8; 3]]) -> [u8; 3] {
    let mut best_c = palette[0];
    let mut min_dist = f32::MAX;

    for &color in palette {
        let dr = r - color[0] as f32;
        let dg = g - color[1] as f32;
        let db = b - color[2] as f32;
        let dist = 2.0 * dr * dr + 4.0 * dg * dg + 3.0 * db * db;
        if dist < min_dist {
            min_dist = dist;
            best_c = color;
        }
    }
    best_c
}

#[inline]
fn apply_contrast_val(val: f32, contrast: f32) -> f32 {
    if contrast.abs() < 1e-4 {
        val
    } else {
        let c = (contrast * 2.55).clamp(-250.0, 250.0);
        let factor = (259.0 * (c + 255.0)) / (255.0 * (259.0 - c));
        (factor * (val - 128.0) + 128.0).clamp(0.0, 255.0)
    }
}

/// Apply dithering and quantize to retro hardware palette.
pub fn apply_dither_palette(img: &RgbaImage, params: &DitherPaletteParams) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
    }

    let scale = params.pixel_scale.clamp(1, 16) as u32;
    let dw = ((w + scale - 1) / scale).max(1);
    let dh = ((h + scale - 1) / scale).max(1);
    let colors = params.palette.colors();

    // Sample downscaled grid cells
    let mut downsampled: Vec<[f32; 3]> = Vec::with_capacity((dw * dh) as usize);
    for dy in 0..dh {
        let src_y = (dy * scale + scale / 2).min(h - 1);
        for dx in 0..dw {
            let src_x = (dx * scale + scale / 2).min(w - 1);
            let p = img.get_pixel(src_x, src_y);
            let r = apply_contrast_val(p[0] as f32, params.contrast);
            let g = apply_contrast_val(p[1] as f32, params.contrast);
            let b = apply_contrast_val(p[2] as f32, params.contrast);
            downsampled.push([r, g, b]);
        }
    }

    let mut quantized: Vec<[u8; 3]> = vec![[0, 0, 0]; (dw * dh) as usize];

    if params.method == DitherMethod::FloydSteinberg {
        let mut cur_err: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; dw as usize];
        let mut nxt_err: Vec<[f32; 3]> = vec![[0.0, 0.0, 0.0]; dw as usize];

        for dy in 0..dh {
            for dx in 0..dw {
                let idx = (dy * dw + dx) as usize;
                let base = downsampled[idx];
                let err = cur_err[dx as usize];
                let r = (base[0] + err[0]).clamp(0.0, 255.0);
                let g = (base[1] + err[1]).clamp(0.0, 255.0);
                let b = (base[2] + err[2]).clamp(0.0, 255.0);

                let chosen = find_nearest_palette_color(r, g, b, colors);
                quantized[idx] = chosen;

                let er = (r - chosen[0] as f32) * params.dither_strength;
                let eg = (g - chosen[1] as f32) * params.dither_strength;
                let eb = (b - chosen[2] as f32) * params.dither_strength;

                if dx + 1 < dw {
                    cur_err[(dx + 1) as usize][0] += er * (7.0 / 16.0);
                    cur_err[(dx + 1) as usize][1] += eg * (7.0 / 16.0);
                    cur_err[(dx + 1) as usize][2] += eb * (7.0 / 16.0);
                }
                if dy + 1 < dh {
                    if dx > 0 {
                        nxt_err[(dx - 1) as usize][0] += er * (3.0 / 16.0);
                        nxt_err[(dx - 1) as usize][1] += eg * (3.0 / 16.0);
                        nxt_err[(dx - 1) as usize][2] += eb * (3.0 / 16.0);
                    }
                    nxt_err[dx as usize][0] += er * (5.0 / 16.0);
                    nxt_err[dx as usize][1] += eg * (5.0 / 16.0);
                    nxt_err[dx as usize][2] += eb * (5.0 / 16.0);
                    if dx + 1 < dw {
                        nxt_err[(dx + 1) as usize][0] += er * (1.0 / 16.0);
                        nxt_err[(dx + 1) as usize][1] += eg * (1.0 / 16.0);
                        nxt_err[(dx + 1) as usize][2] += eb * (1.0 / 16.0);
                    }
                }
            }
            cur_err = nxt_err;
            nxt_err = vec![[0.0, 0.0, 0.0]; dw as usize];
        }
    } else {
        // Ordered dithering (Bayer, Blue Noise, or None)
        for dy in 0..dh {
            for dx in 0..dw {
                let idx = (dy * dw + dx) as usize;
                let base = downsampled[idx];
                let offset = match params.method {
                    DitherMethod::Bayer4x4 => {
                        let v = BAYER_4X4[(dy % 4) as usize][(dx % 4) as usize] as f32 / 16.0 - 0.5;
                        v * params.dither_strength * 64.0
                    }
                    DitherMethod::Bayer8x8 => {
                        let v = BAYER_8X8[(dy % 8) as usize][(dx % 8) as usize] as f32 / 64.0 - 0.5;
                        v * params.dither_strength * 64.0
                    }
                    DitherMethod::BlueNoise => {
                        let v = BLUE_NOISE_16X16[(dy % 16) as usize][(dx % 16) as usize] as f32
                            / 255.0
                            - 0.5;
                        v * params.dither_strength * 64.0
                    }
                    DitherMethod::None | DitherMethod::FloydSteinberg => 0.0,
                };

                let r = (base[0] + offset).clamp(0.0, 255.0);
                let g = (base[1] + offset).clamp(0.0, 255.0);
                let b = (base[2] + offset).clamp(0.0, 255.0);
                quantized[idx] = find_nearest_palette_color(r, g, b, colors);
            }
        }
    }

    // Upscale quantized pixel blocks to output image
    let mut out = RgbaImage::new(w, h);
    out.enumerate_rows_mut().par_bridge().for_each(|(y, row)| {
        let dy = (y / scale).min(dh - 1);
        for (x, _, pixel) in row {
            let dx = (x / scale).min(dw - 1);
            let c = quantized[(dy * dw + dx) as usize];
            let a = img.get_pixel(x, y)[3];
            *pixel = Rgba([c[0], c[1], c[2], a]);
        }
    });

    out
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. NTSC & VHS Video Emulation
// ─────────────────────────────────────────────────────────────────────────────

/// VHS cassette tape recording speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VhsTapeSpeed {
    #[default]
    SP, // Standard Play (highest quality)
    LP, // Long Play (moderate quality)
    EP, // Extended Play (heaviest degradation)
}

impl VhsTapeSpeed {
    pub fn all() -> &'static [VhsTapeSpeed] {
        &[VhsTapeSpeed::SP, VhsTapeSpeed::LP, VhsTapeSpeed::EP]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::SP => "SP (Standard Play - Clean)",
            Self::LP => "LP (Long Play - Moderate)",
            Self::EP => "EP (Extended Play - Worn)",
        }
    }
}

/// NTSC Chroma demodulation filter mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtscDemodulation {
    Notch,
    OneLineComb,
    TwoLineComb,
    Box,
}

impl NtscDemodulation {
    pub fn all() -> &'static [Self] {
        &[Self::Notch, Self::OneLineComb, Self::TwoLineComb, Self::Box]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Notch => "Notch (Dot Crawl)",
            Self::OneLineComb => "1-Line Comb",
            Self::TwoLineComb => "2-Line Comb",
            Self::Box => "Box Filter",
        }
    }
}

/// NTSC Chroma lowpass filtering intensity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtscChromaLowpass {
    None,
    Light,
    Full,
}

impl NtscChromaLowpass {
    pub fn all() -> &'static [Self] {
        &[Self::None, Self::Light, Self::Full]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "None (Sharp)",
            Self::Light => "Light",
            Self::Full => "Full (Soft)",
        }
    }
}

/// NTSC Scanline subcarrier phase shift (affects dot crawl alignment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NtscPhaseShift {
    Degrees0,
    Degrees90,
    Degrees180,
    Degrees270,
}

impl NtscPhaseShift {
    pub fn all() -> &'static [Self] {
        &[
            Self::Degrees0,
            Self::Degrees90,
            Self::Degrees180,
            Self::Degrees270,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Degrees0 => "0°",
            Self::Degrees90 => "90°",
            Self::Degrees180 => "180°",
            Self::Degrees270 => "270°",
        }
    }
}

/// Parameters for NTSC broadcast and VHS tape emulation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NtscParams {
    // ── Broadcast RF & Signal ──
    /// Horizontal luminance smear / bandwidth restriction (0.0 to 1.0). Default is 0.25.
    pub luma_smear: f32,
    /// Composite peaking / edge sharpening (0.0 to 3.0). Default is 1.0.
    pub composite_sharpening: f32,
    /// Ringing intensity around high-contrast edges (0.0 to 2.0). Default is 0.5.
    pub ringing: f32,
    /// Analog RF static / snow intensity (0.0 to 0.05). Default is 0.001.
    pub snow: f32,
    /// Horizontal static noise streaking anisotropy (0.0 to 1.0). Default is 0.5.
    pub snow_anisotropy: f32,
    /// CRT interlaced scanlines simulation. Default is false.
    pub scanlines: bool,
    /// Subcarrier phase shift angle across scanlines. Default is 180°.
    pub phase_shift: NtscPhaseShift,
    /// Demodulation filter algorithm (Notch dot crawl vs Comb clean). Default is Notch.
    pub demodulation: NtscDemodulation,
    /// Chroma lowpass filter bandwidth. Default is Full.
    pub chroma_lowpass: NtscChromaLowpass,

    // ── Chrominance & YIQ ──
    /// Horizontal chroma offset / color bleed in pixels (-8.0 to 8.0). Default is 1.0.
    pub chroma_delay_x: f32,
    /// Vertical chroma offset / scanline shift (-4 to 4 lines). Default is 0.
    pub chroma_delay_y: i32,
    /// Vertical chroma blending across adjacent scanlines. Default is true.
    pub chroma_vert_blend: bool,
    /// Global chroma phase error / master tint rotation (0.0 to 1.0). Default is 0.0.
    pub chroma_phase_error: f32,
    /// Per-scanline chroma phase jitter / color noise (0.0 to 0.15). Default is 0.005.
    pub chroma_phase_noise: f32,
    /// Analog color grain / chroma noise intensity (0.0 to 0.50). Default is 0.05.
    pub chroma_noise: f32,

    // ── Analog Noise ──
    /// Luminance channel noise grain (0.0 to 0.50). Default is 0.01.
    pub luma_noise: f32,
    /// Composite signal noise grain (0.0 to 0.50). Default is 0.05.
    pub composite_noise: f32,

    // ── VHS Tape Simulation ──
    /// Enable VHS magnetic tape emulation pass. Default is true.
    pub vhs_enabled: bool,
    /// VHS tape recording speed / quality. Default is SP.
    pub tape_speed: VhsTapeSpeed,
    /// Horizontal wave / tape wobble displacement (0.0 to 2.0). Default is 0.5.
    pub edge_wave: f32,
    /// VCR head-switching distortion at bottom of frame. Default is true.
    pub head_switching: bool,
    /// VCR tracking noise bar intensity (0.0 to 1.0). Default is 0.0.
    pub tracking_noise: f32,
    /// VHS playback head sharpening / pre-emphasis (0.0 to 1.0). Default is 0.25.
    pub vhs_sharpen: f32,
    /// VHS tape chroma loss / color dropout fading (0.0 to 1.0). Default is 0.05.
    pub vhs_chroma_loss: f32,

    // ── Deterministic Noise ──
    /// Random seed for deterministic noise generation (0 to 1000). Default is 0.
    pub random_seed: i32,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for NtscParams {
    fn default() -> Self {
        Self {
            luma_smear: 0.25,
            composite_sharpening: 1.0,
            ringing: 0.5,
            snow: 0.001,
            snow_anisotropy: 0.5,
            scanlines: false,
            phase_shift: NtscPhaseShift::Degrees180,
            demodulation: NtscDemodulation::Notch,
            chroma_lowpass: NtscChromaLowpass::Full,
            chroma_delay_x: 1.0,
            chroma_delay_y: 0,
            chroma_vert_blend: true,
            chroma_phase_error: 0.0,
            chroma_phase_noise: 0.005,
            chroma_noise: 0.05,
            luma_noise: 0.01,
            composite_noise: 0.05,
            vhs_enabled: true,
            tape_speed: VhsTapeSpeed::SP,
            edge_wave: 0.5,
            head_switching: true,
            tracking_noise: 0.0,
            vhs_sharpen: 0.25,
            vhs_chroma_loss: 0.05,
            random_seed: 0,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Convert NtscParams to an NtscEffect configuration from ntsc-rs.
pub fn params_to_ntsc_effect(params: &NtscParams) -> ntsc_rs::NtscEffect {
    let mut effect = ntsc_rs::NtscEffect::default();
    effect.random_seed = params.random_seed;
    effect.use_field = if params.scanlines {
        ntsc_rs::settings::standard::UseField::InterleavedUpper
    } else {
        ntsc_rs::settings::standard::UseField::Both
    };
    effect.luma_smear = params.luma_smear.clamp(0.0, 1.0);
    effect.composite_sharpening = params.composite_sharpening.clamp(0.0, 3.0);
    effect.snow_intensity = params.snow.clamp(0.0, 0.1);
    effect.snow_anisotropy = params.snow_anisotropy.clamp(0.0, 1.0);

    effect.chroma_delay_horizontal = params.chroma_delay_x.clamp(-12.0, 12.0);
    effect.chroma_delay_vertical = params.chroma_delay_y.clamp(-4, 4);
    effect.chroma_vert_blend = params.chroma_vert_blend;

    effect.chroma_phase_error = params.chroma_phase_error.clamp(0.0, 1.0);
    effect.chroma_phase_noise_intensity = params.chroma_phase_noise.clamp(0.0, 0.2);

    effect.video_scanline_phase_shift = match params.phase_shift {
        NtscPhaseShift::Degrees0 => ntsc_rs::settings::standard::PhaseShift::Degrees0,
        NtscPhaseShift::Degrees90 => ntsc_rs::settings::standard::PhaseShift::Degrees90,
        NtscPhaseShift::Degrees180 => ntsc_rs::settings::standard::PhaseShift::Degrees180,
        NtscPhaseShift::Degrees270 => ntsc_rs::settings::standard::PhaseShift::Degrees270,
    };

    effect.chroma_demodulation = match params.demodulation {
        NtscDemodulation::Notch => ntsc_rs::settings::standard::ChromaDemodulationFilter::Notch,
        NtscDemodulation::OneLineComb => {
            ntsc_rs::settings::standard::ChromaDemodulationFilter::OneLineComb
        }
        NtscDemodulation::TwoLineComb => {
            ntsc_rs::settings::standard::ChromaDemodulationFilter::TwoLineComb
        }
        NtscDemodulation::Box => ntsc_rs::settings::standard::ChromaDemodulationFilter::Box,
    };

    let clow = match params.chroma_lowpass {
        NtscChromaLowpass::None => ntsc_rs::settings::standard::ChromaLowpass::None,
        NtscChromaLowpass::Light => ntsc_rs::settings::standard::ChromaLowpass::Light,
        NtscChromaLowpass::Full => ntsc_rs::settings::standard::ChromaLowpass::Full,
    };
    effect.chroma_lowpass_in = clow;
    effect.chroma_lowpass_out = clow;

    if params.chroma_noise > 0.001 {
        effect.chroma_noise.enabled = true;
        effect.chroma_noise.settings.intensity = params.chroma_noise.clamp(0.0, 1.0);
    } else {
        effect.chroma_noise.enabled = false;
    }

    if params.luma_noise > 0.001 {
        effect.luma_noise.enabled = true;
        effect.luma_noise.settings.intensity = params.luma_noise.clamp(0.0, 1.0);
    } else {
        effect.luma_noise.enabled = false;
    }

    if params.composite_noise > 0.001 {
        effect.composite_noise.enabled = true;
        effect.composite_noise.settings.intensity = params.composite_noise.clamp(0.0, 1.0);
    } else {
        effect.composite_noise.enabled = false;
    }

    if params.ringing > 0.001 {
        effect.ringing.enabled = true;
        effect.ringing.settings.intensity = params.ringing;
    } else {
        effect.ringing.enabled = false;
    }

    if params.vhs_enabled {
        effect.vhs_settings.enabled = true;
        effect.vhs_settings.settings.tape_speed = match params.tape_speed {
            VhsTapeSpeed::SP => ntsc_rs::settings::standard::VHSTapeSpeed::SP,
            VhsTapeSpeed::LP => ntsc_rs::settings::standard::VHSTapeSpeed::LP,
            VhsTapeSpeed::EP => ntsc_rs::settings::standard::VHSTapeSpeed::EP,
        };
        if params.edge_wave > 0.001 {
            effect.vhs_settings.settings.edge_wave.enabled = true;
            effect.vhs_settings.settings.edge_wave.settings.intensity = params.edge_wave;
        } else {
            effect.vhs_settings.settings.edge_wave.enabled = false;
        }
        if params.vhs_sharpen > 0.001 {
            effect.vhs_settings.settings.sharpen.enabled = true;
            effect.vhs_settings.settings.sharpen.settings.intensity = params.vhs_sharpen;
        } else {
            effect.vhs_settings.settings.sharpen.enabled = false;
        }
        effect.vhs_settings.settings.chroma_loss = params.vhs_chroma_loss * 0.0005;
    } else {
        effect.vhs_settings.enabled = false;
    }

    effect.head_switching.enabled = params.head_switching;
    if params.head_switching {
        effect.head_switching.settings.height = 8;
        effect.head_switching.settings.horiz_shift = 15.0;
    }

    if params.tracking_noise > 0.001 {
        effect.tracking_noise.enabled = true;
        effect.tracking_noise.settings.noise_intensity = params.tracking_noise;
        effect.tracking_noise.settings.wave_intensity = params.tracking_noise * 1.5;
        effect.tracking_noise.settings.height = 14;
    } else {
        effect.tracking_noise.enabled = false;
    }

    effect
}

/// Apply NTSC composite and VHS tape emulation to an RGBA image.
pub fn apply_ntsc_effect(img: &RgbaImage, params: &NtscParams) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
    }

    let effect = params_to_ntsc_effect(params);
    let mut raw_bytes = img.clone().into_raw();
    let ctx = ntsc_rs::ctx::global();

    effect.apply_effect_to_buffer::<ntsc_rs::yiq_fielding::Rgbx, u8>(
        ctx,
        (w as usize, h as usize),
        &mut raw_bytes,
        0,
        [1.0, 1.0],
    );

    if let Some(mut out) = RgbaImage::from_raw(w, h, raw_bytes) {
        // Restore original alpha channel in case YIQ conversion altered it
        for (orig_p, dest_p) in img.pixels().zip(out.pixels_mut()) {
            dest_p[3] = orig_p[3];
        }
        out
    } else {
        img.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. Structure (Multiscale Detail & Edge Protection)
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for multiscale structure and texture enhancement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StructureParams {
    /// Structure strength (-100.0 to +100.0). Default is 35.0.
    /// Positive values amplify texture; negative values suppress texture.
    pub amount: f32,
    /// Detail scale emphasis (-50.0 to +50.0). Default is 0.0.
    /// Negative emphasizes fine micro-texture; positive emphasizes coarser structures.
    pub detail_scale: f32,
    /// Edge protection sensitivity (0.0 to 100.0). Default is 50.0.
    /// Higher values protect high-contrast boundaries more aggressively.
    pub edge_protection: f32,
    /// Depth map mask settings.
    pub depth_mask: DepthMaskConfig,
}

impl Default for StructureParams {
    fn default() -> Self {
        Self {
            amount: 35.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        }
    }
}

/// Compute 3 integer box blur radii approximating a Gaussian filter of standard deviation sigma.
pub fn compute_box_radii_3(sigma: f32) -> [usize; 3] {
    if sigma <= 0.05 {
        return [0, 0, 0];
    }
    let w_ideal = ((12.0 * sigma * sigma / 3.0) + 1.0).sqrt();
    let mut wl = w_ideal.floor() as usize;
    if wl % 2 == 0 {
        wl = wl.saturating_sub(1);
    }
    let wu = wl + 2;
    let m_ideal =
        (12.0 * sigma * sigma - (3 * wl * wl + 12 * wl + 9) as f32) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round().clamp(0.0, 3.0) as usize;

    let mut radii = [0usize; 3];
    for i in 0..3 {
        let w = if i < m { wl } else { wu };
        radii[i] = w / 2;
    }
    radii
}

/// 1D horizontal box blur pass using sliding-window summation on rows.
fn box_blur_horizontal(src: &[f32], dst: &mut [f32], width: usize, _height: usize, r: usize) {
    if r == 0 || width <= 1 {
        dst.copy_from_slice(src);
        return;
    }
    let r_i = r as isize;
    let w_i = width as isize;
    let inv_w = 1.0 / (2 * r + 1) as f64;

    dst.par_chunks_exact_mut(width)
        .zip(src.par_chunks_exact(width))
        .for_each(|(dst_row, src_row)| {
            let mut sum: f64 = 0.0;
            for x in -r_i..=r_i {
                let idx = x.clamp(0, w_i - 1) as usize;
                sum += src_row[idx] as f64;
            }
            dst_row[0] = (sum * inv_w) as f32;

            for x in 1..width {
                let left_idx = ((x as isize) - r_i - 1).clamp(0, w_i - 1) as usize;
                let right_idx = ((x as isize) + r_i).clamp(0, w_i - 1) as usize;
                sum += (src_row[right_idx] as f64) - (src_row[left_idx] as f64);
                dst_row[x] = (sum * inv_w) as f32;
            }
        });
}

/// Transpose a row-major matrix of dimensions (width, height) to (height, width).
fn transpose(src: &[f32], dst: &mut [f32], width: usize, height: usize) {
    dst.par_chunks_exact_mut(height)
        .enumerate()
        .for_each(|(x, dst_col)| {
            for y in 0..height {
                dst_col[y] = src[y * width + x];
            }
        });
}

/// Fast separable 3-pass box blur approximating Gaussian convolution in O(N) time.
pub fn fast_gaussian_blur(src: &[f32], dst: &mut [f32], width: usize, height: usize, sigma: f32) {
    let radii = compute_box_radii_3(sigma);
    if radii == [0, 0, 0] || width <= 1 || height <= 1 {
        dst.copy_from_slice(src);
        return;
    }

    let mut current = src.to_vec();
    let mut temp = vec![0.0f32; width * height];
    let mut transposed = vec![0.0f32; width * height];
    let mut temp_transposed = vec![0.0f32; width * height];

    for &r in &radii {
        if r > 0 {
            // Horizontal blur on rows (width x height)
            box_blur_horizontal(&current, &mut temp, width, height, r);
            // Transpose (width x height) -> (height x width)
            transpose(&temp, &mut transposed, width, height);
            // Horizontal blur on transposed rows (height x width)
            box_blur_horizontal(&transposed, &mut temp_transposed, height, width, r);
            // Transpose back (height x width) -> (width x height)
            transpose(&temp_transposed, &mut current, height, width);
        }
    }
    dst.copy_from_slice(&current);
}

/// Compute 2D gradient magnitude using Scharr 3x3 filter.
fn compute_gradient_magnitude(luma: &[f32], grad: &mut [f32], width: usize, height: usize) {
    let w = width;
    let h = height;
    let w_i = w as isize;
    let h_i = h as isize;

    grad.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, grad_row)| {
            let y_prev = ((y as isize) - 1).clamp(0, h_i - 1) as usize;
            let y_curr = y;
            let y_next = ((y as isize) + 1).clamp(0, h_i - 1) as usize;

            let row_prev = &luma[y_prev * w..(y_prev + 1) * w];
            let row_curr = &luma[y_curr * w..(y_curr + 1) * w];
            let row_next = &luma[y_next * w..(y_next + 1) * w];

            for x in 0..w {
                let x_prev = ((x as isize) - 1).clamp(0, w_i - 1) as usize;
                let x_next = ((x as isize) + 1).clamp(0, w_i - 1) as usize;

                let gx = (3.0 * (row_prev[x_next] - row_prev[x_prev])
                    + 10.0 * (row_curr[x_next] - row_curr[x_prev])
                    + 3.0 * (row_next[x_next] - row_next[x_prev]))
                    * (1.0 / 32.0);

                let gy = (3.0 * (row_next[x_prev] - row_prev[x_prev])
                    + 10.0 * (row_next[x] - row_prev[x])
                    + 3.0 * (row_next[x_next] - row_prev[x_next]))
                    * (1.0 / 32.0);

                grad_row[x] = (gx * gx + gy * gy).sqrt();
            }
        });
}

/// Smooth coring function that damps near-zero noise in detail layers.
#[inline]
fn soft_core(val: f32, tau: f32) -> f32 {
    let abs_v = val.abs();
    if abs_v <= tau * 0.01 {
        0.0
    } else {
        let ratio = abs_v / tau;
        let factor = (1.0 - (-ratio * ratio).exp()).max(0.0);
        val * factor
    }
}

/// Apply selective multiscale structure and texture enhancement with edge protection.
pub fn apply_structure(img: &RgbaImage, params: &StructureParams) -> RgbaImage {
    let (width, height) = (img.width() as usize, img.height() as usize);
    if width == 0 || height == 0 || params.amount.abs() < 0.001 {
        return img.clone();
    }

    let n_pixels = width * height;

    // 1. Convert to OKLab lightness (L) and preserve chromas & alpha
    let mut luma = vec![0.0f32; n_pixels];
    let mut chromas = vec![(0.0f32, 0.0f32, 255u8); n_pixels];

    let pixels: Vec<Rgba<u8>> = img.pixels().copied().collect();
    luma.par_chunks_exact_mut(width)
        .zip(chromas.par_chunks_exact_mut(width))
        .zip(pixels.par_chunks_exact(width))
        .for_each(|((l_row, c_row), p_row)| {
            for x in 0..width {
                let p = p_row[x];
                let r = p[0] as f32 / 255.0;
                let g = p[1] as f32 / 255.0;
                let b = p[2] as f32 / 255.0;
                let (l, a, b_ok) = srgb_to_oklab(r, g, b);
                l_row[x] = l;
                c_row[x] = (a, b_ok, p[3]);
            }
        });

    // 2. Scale factor based on resolution
    let min_dim = width.min(height) as f32;
    let s = (min_dim / 1024.0).powf(0.6).clamp(0.35, 4.0);

    // 3. Compute edge protection mask from gradient magnitude
    let mut grad_raw = vec![0.0f32; n_pixels];
    compute_gradient_magnitude(&luma, &mut grad_raw, width, height);

    let mut grad_smooth = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&grad_raw, &mut grad_smooth, width, height, 2.0 * s);

    let edge_threshold = 0.06 + (100.0 - params.edge_protection.clamp(0.0, 100.0)) * 0.0016;

    // 4. Multiscale decomposition: 4 spatial scale levels
    let sigma_1 = 1.2 * s;
    let sigma_2 = 2.8 * s;
    let sigma_3 = 6.5 * s;
    let sigma_4 = 15.0 * s;

    let delta_sigma_2 = (sigma_2 * sigma_2 - sigma_1 * sigma_1).max(0.01).sqrt();
    let delta_sigma_3 = (sigma_3 * sigma_3 - sigma_2 * sigma_2).max(0.01).sqrt();
    let delta_sigma_4 = (sigma_4 * sigma_4 - sigma_3 * sigma_3).max(0.01).sqrt();

    let mut b1 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&luma, &mut b1, width, height, sigma_1);

    let mut b2 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&b1, &mut b2, width, height, delta_sigma_2);

    let mut b3 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&b2, &mut b3, width, height, delta_sigma_3);

    let mut b4 = vec![0.0f32; n_pixels];
    fast_gaussian_blur(&b3, &mut b4, width, height, delta_sigma_4);

    // 5. Band weights modulated by detail_scale (-50 to +50)
    let k = params.detail_scale.clamp(-50.0, 50.0) / 50.0;
    let (w1, w2, w3, w4) = if k < 0.0 {
        let abs_k = -k;
        (
            0.40 * (1.0 + 0.7 * abs_k),
            1.00 * (1.0 + 0.3 * abs_k),
            0.70 * (1.0 - 0.5 * abs_k),
            0.30 * (1.0 - 0.7 * abs_k),
        )
    } else {
        (
            0.40 * (1.0 - 0.6 * k),
            1.00 * (1.0 - 0.2 * k),
            0.70 * (1.0 + 0.5 * k),
            0.30 * (1.0 + 0.9 * k),
        )
    };

    let tau_core = 0.005;

    // 6. Synthesize output pixels into raw RGBA buffer
    let mut raw_bytes = vec![0u8; n_pixels * 4];

    raw_bytes
        .par_chunks_exact_mut(width * 4)
        .enumerate()
        .for_each(|(y, out_row_bytes)| {
            let row_start = y * width;
            for x in 0..width {
                let idx = row_start + x;
                let l_orig = luma[idx];

                // Detail bandpass layers
                let d1 = l_orig - b1[idx];
                let d2 = b1[idx] - b2[idx];
                let d3 = b2[idx] - b3[idx];
                let d4 = b3[idx] - b4[idx];

                let d1_cored = soft_core(d1, tau_core);

                let total_detail = w1 * d1_cored + w2 * d2 + w3 * d3 + w4 * d4;

                // Soft edge-stop mask: 1.0 in texture/flat areas, drops near boundaries
                let g = grad_smooth[idx] / edge_threshold;
                let g3 = g * g * g;
                let edge_mask = 1.0 / (1.0 + g3);

                let delta = total_detail * edge_mask;

                // Positive amount enhances texture; negative amount suppresses texture
                let l_new = if params.amount >= 0.0 {
                    l_orig + (params.amount / 50.0) * delta
                } else {
                    l_orig + (params.amount / 100.0) * delta
                };

                let l_clamped = l_new.clamp(0.0, 1.0);
                let (a_ok, b_ok, alpha) = chromas[idx];
                let (r_norm, g_norm, b_norm) = oklab_to_srgb(l_clamped, a_ok, b_ok);

                let px_offset = x * 4;
                out_row_bytes[px_offset] = (r_norm * 255.0).round().clamp(0.0, 255.0) as u8;
                out_row_bytes[px_offset + 1] = (g_norm * 255.0).round().clamp(0.0, 255.0) as u8;
                out_row_bytes[px_offset + 2] = (b_norm * 255.0).round().clamp(0.0, 255.0) as u8;
                out_row_bytes[px_offset + 3] = alpha;
            }
        });

    RgbaImage::from_raw(width as u32, height as u32, raw_bytes).unwrap_or_else(|| img.clone())
}

// ─────────────────────────────────────────────────────────────────────────────
// Presets & Unified Interface
// ─────────────────────────────────────────────────────────────────────────────

/// Combined parameter payload for any creative effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CreativeFxParams {
    LensAberration(LensAberrationParams),
    Trichromatic(TrichromaticParams),
    PixelSort(PixelSortParams),
    CmykHalftone(CmykHalftoneParams),
    DitherPalette(DitherPaletteParams),
    Ntsc(NtscParams),
    Structure(StructureParams),
}

impl CreativeFxParams {
    pub fn depth_mask(&self) -> &DepthMaskConfig {
        match self {
            Self::LensAberration(p) => &p.depth_mask,
            Self::Trichromatic(p) => &p.depth_mask,
            Self::PixelSort(p) => &p.depth_mask,
            Self::CmykHalftone(p) => &p.depth_mask,
            Self::DitherPalette(p) => &p.depth_mask,
            Self::Ntsc(p) => &p.depth_mask,
            Self::Structure(p) => &p.depth_mask,
        }
    }

    pub fn depth_mask_mut(&mut self) -> &mut DepthMaskConfig {
        match self {
            Self::LensAberration(p) => &mut p.depth_mask,
            Self::Trichromatic(p) => &mut p.depth_mask,
            Self::PixelSort(p) => &mut p.depth_mask,
            Self::CmykHalftone(p) => &mut p.depth_mask,
            Self::DitherPalette(p) => &mut p.depth_mask,
            Self::Ntsc(p) => &mut p.depth_mask,
            Self::Structure(p) => &mut p.depth_mask,
        }
    }
}

/// Named preset configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct CreativeFxPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub params: CreativeFxParams,
}

/// Return the signature preset used for preview thumbnails in the FX list.
pub fn default_preset_for_fx(fx_type: CreativeFxType) -> CreativeFxPreset {
    match fx_type {
        CreativeFxType::LensAberration => CreativeFxPreset {
            name: "Classic Lens",
            description: "Balanced optical dispersion, strong at borders",
            params: CreativeFxParams::LensAberration(LensAberrationParams {
                strength: 45.0,
                falloff: 65.0,
                color_balance: 0.0,
                depth_mask: DepthMaskConfig::default(),
            }),
        },
        CreativeFxType::Trichromatic => CreativeFxPreset {
            name: "RGB Drift",
            description: "Distinct RGB channel separation and camera drift",
            params: CreativeFxParams::Trichromatic(TrichromaticParams {
                mode: TrichromaticMode::Rgb,
                channels: [
                    ChannelTransform {
                        offset_x: -8.0,
                        offset_y: -2.0,
                        zoom: 1.02,
                    },
                    ChannelTransform {
                        offset_x: 0.0,
                        offset_y: 0.0,
                        zoom: 1.00,
                    },
                    ChannelTransform {
                        offset_x: 8.0,
                        offset_y: 2.0,
                        zoom: 0.98,
                    },
                ],
                auto_crop: true,
                depth_mask: DepthMaskConfig::default(),
            }),
        },
        CreativeFxType::PixelSort => CreativeFxPreset {
            name: "Cascading Melt",
            description: "Vertical downward brightness streaks in mid-tones",
            params: CreativeFxParams::PixelSort(PixelSortParams {
                direction: PixelSortDirection::Vertical,
                criteria: PixelSortCriteria::Brightness,
                order: PixelSortOrder::Descending,
                min_threshold: 0.20,
                max_threshold: 0.85,
                min_span: 8,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            }),
        },
        CreativeFxType::CmykHalftone => CreativeFxPreset {
            name: "Offset Press",
            description: "Classic rosette halftone with newsprint paper tint and slight misregistration",
            params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                dot_size: 5.0,
                dot_gain: 1.1,
                paper_tint: PaperTint::Newsprint,
                cyan_offset: [-1.5, 0.5],
                magenta_offset: [1.0, -1.0],
                yellow_offset: [0.0, 1.5],
                black_offset: [0.0, 0.0],
                depth_mask: DepthMaskConfig::default(),
            }),
        },
        CreativeFxType::DitherPalette => CreativeFxPreset {
            name: "Game Boy Pocket",
            description: "4-shade classic greenish LCD dot-matrix dithering",
            params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                palette: RetroPalette::GameBoy,
                method: DitherMethod::Bayer4x4,
                pixel_scale: 2,
                dither_strength: 0.85,
                contrast: 10.0,
                depth_mask: DepthMaskConfig::default(),
            }),
        },
        CreativeFxType::Ntsc => CreativeFxPreset {
            name: "VHS SP (Home Video)",
            description: "Clean Standard Play home recording with subtle edge wave and head switching",
            params: CreativeFxParams::Ntsc(NtscParams {
                luma_smear: 0.25,
                composite_sharpening: 1.0,
                ringing: 0.4,
                snow: 0.001,
                snow_anisotropy: 0.6,
                scanlines: false,
                phase_shift: NtscPhaseShift::Degrees180,
                demodulation: NtscDemodulation::OneLineComb,
                chroma_lowpass: NtscChromaLowpass::Full,
                chroma_delay_x: 2.0,
                chroma_delay_y: 0,
                chroma_vert_blend: true,
                chroma_phase_error: 0.0,
                chroma_phase_noise: 0.008,
                chroma_noise: 0.05,
                luma_noise: 0.02,
                composite_noise: 0.05,
                vhs_enabled: true,
                tape_speed: VhsTapeSpeed::SP,
                edge_wave: 0.4,
                head_switching: true,
                tracking_noise: 0.0,
                vhs_sharpen: 0.25,
                vhs_chroma_loss: 0.05,
                random_seed: 101,
                depth_mask: DepthMaskConfig::default(),
            }),
        },
        CreativeFxType::Structure => CreativeFxPreset {
            name: "Natural Texture",
            description: "Balanced organic texture enhancement for everyday photos",
            params: CreativeFxParams::Structure(StructureParams {
                amount: 35.0,
                detail_scale: 0.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            }),
        },
    }
}

/// List all available presets for a given FX type.
pub fn presets_for_fx(fx_type: CreativeFxType) -> Vec<CreativeFxPreset> {
    match fx_type {
        CreativeFxType::LensAberration => vec![
            CreativeFxPreset {
                name: "Subtle Lens",
                description: "Delicate edge fringing mimicking premium prime lenses",
                params: CreativeFxParams::LensAberration(LensAberrationParams {
                    strength: 20.0,
                    falloff: 75.0,
                    color_balance: 0.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Classic Lens",
                description: "Noticeable optical dispersion at borders",
                params: CreativeFxParams::LensAberration(LensAberrationParams {
                    strength: 45.0,
                    falloff: 60.0,
                    color_balance: 0.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Full Dispersion",
                description: "Aberration throughout the entire frame",
                params: CreativeFxParams::LensAberration(LensAberrationParams {
                    strength: 55.0,
                    falloff: 0.0,
                    color_balance: 0.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Blue / Yellow",
                description: "Secondary spectrum with blue/yellow chromatic shift",
                params: CreativeFxParams::LensAberration(LensAberrationParams {
                    strength: 50.0,
                    falloff: 40.0,
                    color_balance: 50.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Red / Blue Anaglyph",
                description: "Strong red and blue split fringes",
                params: CreativeFxParams::LensAberration(LensAberrationParams {
                    strength: 60.0,
                    falloff: 30.0,
                    color_balance: -50.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Prism Explosion",
                description: "Extreme edge dispersion with heavy color splitting across the entire frame",
                params: CreativeFxParams::LensAberration(LensAberrationParams {
                    strength: 95.0,
                    falloff: 10.0,
                    color_balance: -40.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
        CreativeFxType::Trichromatic => vec![
            CreativeFxPreset {
                name: "RGB Drift",
                description: "Classic horizontal RGB plate misalignment",
                params: CreativeFxParams::Trichromatic(TrichromaticParams {
                    mode: TrichromaticMode::Rgb,
                    channels: [
                        ChannelTransform {
                            offset_x: -6.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                        ChannelTransform {
                            offset_x: 0.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                        ChannelTransform {
                            offset_x: 6.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                    ],
                    auto_crop: true,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Prokudin-Gorsky",
                description: "Historical 3-plate camera ghosting with slight zoom variation",
                params: CreativeFxParams::Trichromatic(TrichromaticParams {
                    mode: TrichromaticMode::Rgb,
                    channels: [
                        ChannelTransform {
                            offset_x: -4.0,
                            offset_y: -3.0,
                            zoom: 1.015,
                        },
                        ChannelTransform {
                            offset_x: 0.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                        ChannelTransform {
                            offset_x: 5.0,
                            offset_y: 3.0,
                            zoom: 0.985,
                        },
                    ],
                    auto_crop: true,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "OKLab Chroma Shift",
                description: "Opponent color drift keeping lightness stable",
                params: CreativeFxParams::Trichromatic(TrichromaticParams {
                    mode: TrichromaticMode::OkLab,
                    channels: [
                        ChannelTransform {
                            offset_x: 0.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                        ChannelTransform {
                            offset_x: -8.0,
                            offset_y: 4.0,
                            zoom: 1.02,
                        },
                        ChannelTransform {
                            offset_x: 8.0,
                            offset_y: -4.0,
                            zoom: 0.98,
                        },
                    ],
                    auto_crop: true,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "OKLab Luma Echo",
                description: "Lightness channel displaced from color channels",
                params: CreativeFxParams::Trichromatic(TrichromaticParams {
                    mode: TrichromaticMode::OkLab,
                    channels: [
                        ChannelTransform {
                            offset_x: 10.0,
                            offset_y: 6.0,
                            zoom: 1.03,
                        },
                        ChannelTransform {
                            offset_x: 0.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                        ChannelTransform {
                            offset_x: 0.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                    ],
                    auto_crop: true,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Temporal Warp",
                description: "Violent 3-channel displacement with dramatic scale differentials",
                params: CreativeFxParams::Trichromatic(TrichromaticParams {
                    mode: TrichromaticMode::Rgb,
                    channels: [
                        ChannelTransform {
                            offset_x: -28.0,
                            offset_y: -14.0,
                            zoom: 1.12,
                        },
                        ChannelTransform {
                            offset_x: 0.0,
                            offset_y: 0.0,
                            zoom: 1.00,
                        },
                        ChannelTransform {
                            offset_x: 28.0,
                            offset_y: 14.0,
                            zoom: 0.88,
                        },
                    ],
                    auto_crop: true,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
        CreativeFxType::PixelSort => vec![
            CreativeFxPreset {
                name: "Cascading Melt",
                description: "Vertical downward brightness streaks in mid-tones",
                params: CreativeFxParams::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Vertical,
                    criteria: PixelSortCriteria::Brightness,
                    order: PixelSortOrder::Descending,
                    min_threshold: 0.20,
                    max_threshold: 0.85,
                    min_span: 8,
                    invert_mask: false,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Cyberpunk Glitch",
                description: "Horizontal hue-sorted glitch streaks across bright areas",
                params: CreativeFxParams::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Horizontal,
                    criteria: PixelSortCriteria::Hue,
                    order: PixelSortOrder::Ascending,
                    min_threshold: 0.35,
                    max_threshold: 0.95,
                    min_span: 12,
                    invert_mask: false,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Highlight Smear",
                description: "Melt only the brightest specular highlights",
                params: CreativeFxParams::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Vertical,
                    criteria: PixelSortCriteria::Brightness,
                    order: PixelSortOrder::Ascending,
                    min_threshold: 0.70,
                    max_threshold: 1.00,
                    min_span: 5,
                    invert_mask: false,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Shadow Drift",
                description: "Horizontal bleed through shadow crevices",
                params: CreativeFxParams::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Horizontal,
                    criteria: PixelSortCriteria::Brightness,
                    order: PixelSortOrder::Descending,
                    min_threshold: 0.00,
                    max_threshold: 0.35,
                    min_span: 6,
                    invert_mask: false,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Diagonal Wind",
                description: "45-degree angled saturation sweep",
                params: CreativeFxParams::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Diagonal,
                    criteria: PixelSortCriteria::Saturation,
                    order: PixelSortOrder::Ascending,
                    min_threshold: 0.25,
                    max_threshold: 0.90,
                    min_span: 10,
                    invert_mask: false,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Total Data Glitch",
                description: "Wide-spectrum aggressive pixel sorting creating extreme digital melt patterns",
                params: CreativeFxParams::PixelSort(PixelSortParams {
                    direction: PixelSortDirection::Horizontal,
                    criteria: PixelSortCriteria::Brightness,
                    order: PixelSortOrder::Descending,
                    min_threshold: 0.05,
                    max_threshold: 0.98,
                    min_span: 3,
                    invert_mask: false,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
        CreativeFxType::CmykHalftone => vec![
            CreativeFxPreset {
                name: "Offset Press",
                description: "Standard rosette screen with newsprint tint and subtle misregistration",
                params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                    dot_size: 5.0,
                    dot_gain: 1.1,
                    paper_tint: PaperTint::Newsprint,
                    cyan_offset: [-1.5, 0.5],
                    magenta_offset: [1.0, -1.0],
                    yellow_offset: [0.0, 1.5],
                    black_offset: [0.0, 0.0],
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Vintage Comic",
                description: "Larger half-tone dots with noticeable cyan and magenta plate slip",
                params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                    dot_size: 7.5,
                    dot_gain: 1.25,
                    paper_tint: PaperTint::Newsprint,
                    cyan_offset: [-3.0, 1.5],
                    magenta_offset: [2.0, -2.5],
                    yellow_offset: [1.0, 3.0],
                    black_offset: [0.0, 0.0],
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Misaligned Litho",
                description: "Heavy plate shift on cardboard/kraft textured paper",
                params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                    dot_size: 4.5,
                    dot_gain: 1.05,
                    paper_tint: PaperTint::Kraft,
                    cyan_offset: [-4.0, 2.0],
                    magenta_offset: [3.0, -3.0],
                    yellow_offset: [-2.0, 4.0],
                    black_offset: [0.5, -0.5],
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Crisp Halftone",
                description: "Zero misregistration on clean white paper substrate",
                params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                    dot_size: 4.0,
                    dot_gain: 1.0,
                    paper_tint: PaperTint::White,
                    cyan_offset: [0.0, 0.0],
                    magenta_offset: [0.0, 0.0],
                    yellow_offset: [0.0, 0.0],
                    black_offset: [0.0, 0.0],
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Pop Art Poster",
                description: "Chunky dots and rich dot gain on warm cream paper",
                params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                    dot_size: 10.0,
                    dot_gain: 1.4,
                    paper_tint: PaperTint::Cream,
                    cyan_offset: [-2.0, 3.0],
                    magenta_offset: [3.0, -2.0],
                    yellow_offset: [-3.0, -2.0],
                    black_offset: [1.0, 1.0],
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Misprint Catastrophe",
                description: "Massive plate drift with heavy dot gain on cheap newsprint",
                params: CreativeFxParams::CmykHalftone(CmykHalftoneParams {
                    dot_size: 14.0,
                    dot_gain: 1.75,
                    paper_tint: PaperTint::Newsprint,
                    cyan_offset: [-8.0, 6.0],
                    magenta_offset: [7.0, -8.0],
                    yellow_offset: [-6.0, 9.0],
                    black_offset: [3.0, -4.0],
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
        CreativeFxType::DitherPalette => vec![
            CreativeFxPreset {
                name: "Game Boy Pocket",
                description: "Classic 4-shade olive green LCD screen with Bayer 4×4 dither",
                params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                    palette: RetroPalette::GameBoy,
                    method: DitherMethod::Bayer4x4,
                    pixel_scale: 2,
                    dither_strength: 0.85,
                    contrast: 10.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "1984 Macintosh",
                description: "Iconic 1-bit monochrome with Floyd-Steinberg error diffusion",
                params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                    palette: RetroPalette::Macintosh1Bit,
                    method: DitherMethod::FloydSteinberg,
                    pixel_scale: 1,
                    dither_strength: 0.90,
                    contrast: 15.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "PICO-8 Fantasy",
                description: "16-color fantasy console palette with organic Blue Noise dithering",
                params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                    palette: RetroPalette::Pico8,
                    method: DitherMethod::BlueNoise,
                    pixel_scale: 2,
                    dither_strength: 0.75,
                    contrast: 0.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "CGA Cyberpunk",
                description: "Iconic 1981 CGA Mode 1 (Black/Cyan/Magenta/White) with 8×8 Bayer",
                params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                    palette: RetroPalette::CgaMode1,
                    method: DitherMethod::Bayer8x8,
                    pixel_scale: 2,
                    dither_strength: 0.80,
                    contrast: 10.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Commodore 64",
                description: "16 classic C64 shades with smooth Floyd-Steinberg diffusion",
                params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                    palette: RetroPalette::Commodore64,
                    method: DitherMethod::FloydSteinberg,
                    pixel_scale: 2,
                    dither_strength: 0.70,
                    contrast: 5.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Neon Noir",
                description: "High-contrast 8-color neon palette with fine Blue Noise texture",
                params: CreativeFxParams::DitherPalette(DitherPaletteParams {
                    palette: RetroPalette::Cyberpunk,
                    method: DitherMethod::BlueNoise,
                    pixel_scale: 2,
                    dither_strength: 0.85,
                    contrast: 20.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
        CreativeFxType::Ntsc => vec![
            CreativeFxPreset {
                name: "Standard Composite",
                description: "Classic CRT/RF broadcast with slight luma smear, notch filter, and minimal noise",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.35,
                    composite_sharpening: 1.2,
                    ringing: 0.6,
                    snow: 0.0005,
                    snow_anisotropy: 0.5,
                    scanlines: false,
                    phase_shift: NtscPhaseShift::Degrees180,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 1.5,
                    chroma_delay_y: 0,
                    chroma_vert_blend: true,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.002,
                    chroma_noise: 0.02,
                    luma_noise: 0.01,
                    composite_noise: 0.03,
                    vhs_enabled: false,
                    tape_speed: VhsTapeSpeed::SP,
                    edge_wave: 0.0,
                    head_switching: false,
                    tracking_noise: 0.0,
                    vhs_sharpen: 0.0,
                    vhs_chroma_loss: 0.0,
                    random_seed: 42,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "VHS SP (Home Video)",
                description: "Clean Standard Play recording on high-grade tape with subtle edge wobble",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.25,
                    composite_sharpening: 1.0,
                    ringing: 0.4,
                    snow: 0.001,
                    snow_anisotropy: 0.6,
                    scanlines: false,
                    phase_shift: NtscPhaseShift::Degrees180,
                    demodulation: NtscDemodulation::OneLineComb,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 2.0,
                    chroma_delay_y: 0,
                    chroma_vert_blend: true,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.008,
                    chroma_noise: 0.05,
                    luma_noise: 0.02,
                    composite_noise: 0.05,
                    vhs_enabled: true,
                    tape_speed: VhsTapeSpeed::SP,
                    edge_wave: 0.4,
                    head_switching: true,
                    tracking_noise: 0.0,
                    vhs_sharpen: 0.25,
                    vhs_chroma_loss: 0.05,
                    random_seed: 101,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "VHS EP (Rental Tape)",
                description: "6-hour Extended Play rental tape with tracking lines, edge wave, and head switching",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.50,
                    composite_sharpening: 1.4,
                    ringing: 0.8,
                    snow: 0.003,
                    snow_anisotropy: 0.7,
                    scanlines: false,
                    phase_shift: NtscPhaseShift::Degrees180,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 3.0,
                    chroma_delay_y: 1,
                    chroma_vert_blend: false,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.035,
                    chroma_noise: 0.15,
                    luma_noise: 0.05,
                    composite_noise: 0.10,
                    vhs_enabled: true,
                    tape_speed: VhsTapeSpeed::EP,
                    edge_wave: 0.9,
                    head_switching: true,
                    tracking_noise: 0.25,
                    vhs_sharpen: 0.50,
                    vhs_chroma_loss: 0.20,
                    random_seed: 202,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Bad Tracking Glitch",
                description: "Severe VCR head misalignment with heavy tracking noise bars and tape wobble",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.40,
                    composite_sharpening: 1.6,
                    ringing: 1.2,
                    snow: 0.008,
                    snow_anisotropy: 0.8,
                    scanlines: false,
                    phase_shift: NtscPhaseShift::Degrees270,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 4.5,
                    chroma_delay_y: 2,
                    chroma_vert_blend: false,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.06,
                    chroma_noise: 0.25,
                    luma_noise: 0.08,
                    composite_noise: 0.15,
                    vhs_enabled: true,
                    tape_speed: VhsTapeSpeed::EP,
                    edge_wave: 1.5,
                    head_switching: true,
                    tracking_noise: 0.75,
                    vhs_sharpen: 0.60,
                    vhs_chroma_loss: 0.40,
                    random_seed: 333,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Late Night Broadcast",
                description: "Weak aerial broadcast television with static snow, RF noise, and CRT scanlines",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.55,
                    composite_sharpening: 0.8,
                    ringing: 0.4,
                    snow: 0.012,
                    snow_anisotropy: 0.5,
                    scanlines: true,
                    phase_shift: NtscPhaseShift::Degrees180,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Light,
                    chroma_delay_x: 2.5,
                    chroma_delay_y: 0,
                    chroma_vert_blend: true,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.02,
                    chroma_noise: 0.10,
                    luma_noise: 0.04,
                    composite_noise: 0.08,
                    vhs_enabled: false,
                    tape_speed: VhsTapeSpeed::SP,
                    edge_wave: 0.0,
                    head_switching: false,
                    tracking_noise: 0.0,
                    vhs_sharpen: 0.0,
                    vhs_chroma_loss: 0.0,
                    random_seed: 777,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "LaserDisc Hi-Fi",
                description: "High-bandwidth optical analog video with crisp detail and subtle color artifacts",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.10,
                    composite_sharpening: 1.8,
                    ringing: 0.3,
                    snow: 0.0001,
                    snow_anisotropy: 0.5,
                    scanlines: false,
                    phase_shift: NtscPhaseShift::Degrees180,
                    demodulation: NtscDemodulation::TwoLineComb,
                    chroma_lowpass: NtscChromaLowpass::None,
                    chroma_delay_x: 0.5,
                    chroma_delay_y: 0,
                    chroma_vert_blend: true,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.0,
                    chroma_noise: 0.0,
                    luma_noise: 0.0,
                    composite_noise: 0.0,
                    vhs_enabled: false,
                    tape_speed: VhsTapeSpeed::SP,
                    edge_wave: 0.0,
                    head_switching: false,
                    tracking_noise: 0.0,
                    vhs_sharpen: 0.0,
                    vhs_chroma_loss: 0.0,
                    random_seed: 0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Tape Crunch / Datamosh",
                description: "Extreme tape degradation with heavy playhead pre-emphasis, chroma bleed, and 1.85x wobble",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.70,
                    composite_sharpening: 2.2,
                    ringing: 1.5,
                    snow: 0.015,
                    snow_anisotropy: 0.85,
                    scanlines: false,
                    phase_shift: NtscPhaseShift::Degrees90,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 6.0,
                    chroma_delay_y: 3,
                    chroma_vert_blend: false,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.08,
                    chroma_noise: 0.35,
                    luma_noise: 0.12,
                    composite_noise: 0.20,
                    vhs_enabled: true,
                    tape_speed: VhsTapeSpeed::EP,
                    edge_wave: 1.85,
                    head_switching: true,
                    tracking_noise: 0.90,
                    vhs_sharpen: 0.85,
                    vhs_chroma_loss: 0.45,
                    random_seed: 555,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Pirate TV Broadcast",
                description: "Heavily distorted underground aerial transmission with dense static snow and CRT scanlines",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.80,
                    composite_sharpening: 1.2,
                    ringing: 1.0,
                    snow: 0.030,
                    snow_anisotropy: 0.95,
                    scanlines: true,
                    phase_shift: NtscPhaseShift::Degrees0,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 7.0,
                    chroma_delay_y: -2,
                    chroma_vert_blend: false,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.05,
                    chroma_noise: 0.30,
                    luma_noise: 0.15,
                    composite_noise: 0.25,
                    vhs_enabled: false,
                    tape_speed: VhsTapeSpeed::SP,
                    edge_wave: 0.0,
                    head_switching: false,
                    tracking_noise: 0.0,
                    vhs_sharpen: 0.0,
                    vhs_chroma_loss: 0.0,
                    random_seed: 888,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Rewind Glitch Meltdown",
                description: "Violent tape rewind distortion with maximum tracking noise bar, 2.0x edge wave, and dropout",
                params: CreativeFxParams::Ntsc(NtscParams {
                    luma_smear: 0.45,
                    composite_sharpening: 2.5,
                    ringing: 1.8,
                    snow: 0.020,
                    snow_anisotropy: 0.70,
                    scanlines: true,
                    phase_shift: NtscPhaseShift::Degrees270,
                    demodulation: NtscDemodulation::Notch,
                    chroma_lowpass: NtscChromaLowpass::Full,
                    chroma_delay_x: 8.0,
                    chroma_delay_y: 4,
                    chroma_vert_blend: false,
                    chroma_phase_error: 0.0,
                    chroma_phase_noise: 0.10,
                    chroma_noise: 0.40,
                    luma_noise: 0.18,
                    composite_noise: 0.30,
                    vhs_enabled: true,
                    tape_speed: VhsTapeSpeed::EP,
                    edge_wave: 2.0,
                    head_switching: true,
                    tracking_noise: 1.0,
                    vhs_sharpen: 0.95,
                    vhs_chroma_loss: 0.70,
                    random_seed: 999,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
        CreativeFxType::Structure => vec![
            CreativeFxPreset {
                name: "Natural Texture",
                description: "Balanced organic texture enhancement for everyday photos",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: 35.0,
                    detail_scale: 0.0,
                    edge_protection: 50.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Fine Micro-Detail",
                description: "Crisp emphasis on micro-textures like textile weave and paper grain",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: 45.0,
                    detail_scale: -35.0,
                    edge_protection: 60.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Architectural Clarity",
                description: "Strong structural relief for masonry, concrete, and urban forms",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: 55.0,
                    detail_scale: 25.0,
                    edge_protection: 70.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Foliage & Landscape",
                description: "Enhanced depth and definition in grass, leaves, bark, and clouds",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: 50.0,
                    detail_scale: 10.0,
                    edge_protection: 55.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Subtle Definition",
                description: "Gentle tactile boost without calling attention to processing",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: 20.0,
                    detail_scale: 0.0,
                    edge_protection: 45.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Soft Skin & Tone",
                description: "Controlled smoothing of blemishes and pores while keeping facial features razor sharp",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: -35.0,
                    detail_scale: -15.0,
                    edge_protection: 65.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
            CreativeFxPreset {
                name: "Texture Minimizer",
                description: "Strong texture suppression with pristine boundary preservation",
                params: CreativeFxParams::Structure(StructureParams {
                    amount: -70.0,
                    detail_scale: 0.0,
                    edge_protection: 80.0,
                    depth_mask: DepthMaskConfig::default(),
                }),
            },
        ],
    }
}

/// Blend original image and creative effect result using a grayscale depth map as a spatial mask.
pub fn apply_depth_mask(
    orig: &RgbaImage,
    fx: &RgbaImage,
    depth_map: &RgbaImage,
    config: &DepthMaskConfig,
) -> RgbaImage {
    if !config.enabled {
        return fx.clone();
    }

    let (width, height) = orig.dimensions();
    if (width, height) != fx.dimensions() {
        return fx.clone();
    }

    let resampled_depth: std::borrow::Cow<'_, RgbaImage> =
        if depth_map.dimensions() != (width, height) {
            std::borrow::Cow::Owned(image::imageops::resize(
                depth_map,
                width,
                height,
                image::imageops::FilterType::Triangle,
            ))
        } else {
            std::borrow::Cow::Borrowed(depth_map)
        };

    let mut out_bytes = vec![0u8; (width * height * 4) as usize];
    let orig_raw = orig.as_raw();
    let fx_raw = fx.as_raw();
    let depth_raw = resampled_depth.as_raw();

    out_bytes
        .par_chunks_exact_mut((width * 4) as usize)
        .zip(orig_raw.par_chunks_exact((width * 4) as usize))
        .zip(fx_raw.par_chunks_exact((width * 4) as usize))
        .zip(depth_raw.par_chunks_exact((width * 4) as usize))
        .for_each(|(((out_row, orig_row), fx_row), depth_row)| {
            for x in 0..(width as usize) {
                let px_idx = x * 4;
                let dr = depth_row[px_idx] as f32 / 255.0;
                let dg = depth_row[px_idx + 1] as f32 / 255.0;
                let db = depth_row[px_idx + 2] as f32 / 255.0;
                let mut d = 0.299 * dr + 0.587 * dg + 0.114 * db;

                if config.invert {
                    d = 1.0 - d;
                }

                let weight = if config.threshold > 0.001 {
                    let feather = config.feather.clamp(0.001, 1.0);
                    let low = (config.threshold - feather).clamp(0.0, 1.0);
                    let high = (config.threshold + feather).clamp(0.0, 1.0);
                    if high <= low {
                        if d >= config.threshold { 1.0 } else { 0.0 }
                    } else {
                        let t = ((d - low) / (high - low)).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    }
                } else {
                    d.clamp(0.0, 1.0)
                };

                for c in 0..3 {
                    let orig_c = orig_row[px_idx + c] as f32;
                    let fx_c = fx_row[px_idx + c] as f32;
                    let blended = (1.0 - weight) * orig_c + weight * fx_c;
                    out_row[px_idx + c] = blended.round().clamp(0.0, 255.0) as u8;
                }
                out_row[px_idx + 3] = orig_row[px_idx + 3];
            }
        });

    RgbaImage::from_raw(width, height, out_bytes).unwrap_or_else(|| fx.clone())
}

/// Apply a creative effect to an RGBA image with optional depth map masking.
pub fn apply_creative_fx_with_depth(
    img: &RgbaImage,
    depth: Option<&RgbaImage>,
    params: &CreativeFxParams,
) -> RgbaImage {
    let mask_config = params.depth_mask();
    if mask_config.enabled {
        if let Some(depth_map) = depth {
            let fx_result = match params {
                CreativeFxParams::LensAberration(p) => apply_lens_aberration(img, p),
                CreativeFxParams::Trichromatic(p) => apply_trichromatic(img, p),
                CreativeFxParams::PixelSort(p) => apply_pixel_sort(img, p),
                CreativeFxParams::CmykHalftone(p) => apply_cmyk_halftone(img, p),
                CreativeFxParams::DitherPalette(p) => apply_dither_palette(img, p),
                CreativeFxParams::Ntsc(p) => apply_ntsc_effect(img, p),
                CreativeFxParams::Structure(p) => apply_structure(img, p),
            };
            return apply_depth_mask(img, &fx_result, depth_map, mask_config);
        }
    }

    match params {
        CreativeFxParams::LensAberration(p) => apply_lens_aberration(img, p),
        CreativeFxParams::Trichromatic(p) => apply_trichromatic(img, p),
        CreativeFxParams::PixelSort(p) => apply_pixel_sort(img, p),
        CreativeFxParams::CmykHalftone(p) => apply_cmyk_halftone(img, p),
        CreativeFxParams::DitherPalette(p) => apply_dither_palette(img, p),
        CreativeFxParams::Ntsc(p) => apply_ntsc_effect(img, p),
        CreativeFxParams::Structure(p) => apply_structure(img, p),
    }
}

/// Apply any creative effect using its parameter union (without depth masking).
pub fn apply_creative_fx(img: &RgbaImage, params: &CreativeFxParams) -> RgbaImage {
    apply_creative_fx_with_depth(img, None, params)
}

// ─────────────────────────────────────────────────────────────────────────────
// Sampling & Color Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Bilinear interpolation for a single channel [0..3].
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oklab_roundtrip() {
        let colors = [
            (1.0, 0.0, 0.0),
            (0.0, 1.0, 0.0),
            (0.0, 0.0, 1.0),
            (0.5, 0.5, 0.5),
            (0.9, 0.8, 0.2),
        ];
        for (r, g, b) in colors {
            let (l, a, b_ok) = srgb_to_oklab(r, g, b);
            let (r2, g2, b2) = oklab_to_srgb(l, a, b_ok);
            assert!((r - r2).abs() < 0.01, "R mismatch: {r} vs {r2}");
            assert!((g - g2).abs() < 0.01, "G mismatch: {g} vs {g2}");
            assert!((b - b2).abs() < 0.01, "B mismatch: {b} vs {b2}");
        }
    }

    #[test]
    fn test_lens_aberration_execution() {
        let mut img = RgbaImage::new(40, 40);
        for x in 0..40 {
            for y in 0..40 {
                let v = if (x / 5 + y / 5) % 2 == 0 { 255 } else { 0 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }

        // Test with default params
        let res = apply_lens_aberration(&img, &LensAberrationParams::default());
        assert_eq!(res.width(), 40);
        assert_eq!(res.height(), 40);

        // Test with color balance +50 (blue/yellow) and -50 (red/blue)
        let res_by = apply_lens_aberration(
            &img,
            &LensAberrationParams {
                strength: 50.0,
                falloff: 0.0,
                color_balance: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_by.width(), 40);

        let res_rb = apply_lens_aberration(
            &img,
            &LensAberrationParams {
                strength: 50.0,
                falloff: 100.0,
                color_balance: -50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_rb.width(), 40);
    }

    #[test]
    fn test_trichromatic_rgb_and_oklab() {
        let mut img = RgbaImage::new(60, 60);
        for x in 0..60 {
            for y in 0..60 {
                img.put_pixel(x, y, Rgba([x as u8 * 4, y as u8 * 4, 128, 255]));
            }
        }

        let params_rgb = TrichromaticParams {
            mode: TrichromaticMode::Rgb,
            channels: [
                ChannelTransform {
                    offset_x: -2.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 2.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
            ],
            auto_crop: true,
            depth_mask: DepthMaskConfig::default(),
        };

        let res_rgb = apply_trichromatic(&img, &params_rgb);
        // Valid crop should be cropped slightly
        assert!(res_rgb.width() <= 60);
        assert!(res_rgb.height() <= 60);
        assert!(res_rgb.width() > 50);

        let params_oklab = TrichromaticParams {
            mode: TrichromaticMode::OkLab,
            channels: [
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: -2.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 0.0,
                    zoom: 1.0,
                },
                ChannelTransform {
                    offset_x: 0.0,
                    offset_y: 2.0,
                    zoom: 1.0,
                },
            ],
            auto_crop: true,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_oklab = apply_trichromatic(&img, &params_oklab);
        assert!(res_oklab.width() > 0 && res_oklab.height() > 0);
    }

    #[test]
    fn test_pixel_sort_directions() {
        let mut img = RgbaImage::new(30, 30);
        for x in 0..30 {
            for y in 0..30 {
                img.put_pixel(x, y, Rgba([((x * 13 + y * 7) % 256) as u8, 100, 200, 255]));
            }
        }

        // Test vertical sort
        let res_v = apply_pixel_sort(
            &img,
            &PixelSortParams {
                direction: PixelSortDirection::Vertical,
                criteria: PixelSortCriteria::Brightness,
                order: PixelSortOrder::Ascending,
                min_threshold: 0.2,
                max_threshold: 0.8,
                min_span: 4,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_v.width(), 30);
        assert_eq!(res_v.height(), 30);

        // Test horizontal sort
        let res_h = apply_pixel_sort(
            &img,
            &PixelSortParams {
                direction: PixelSortDirection::Horizontal,
                criteria: PixelSortCriteria::Hue,
                order: PixelSortOrder::Descending,
                min_threshold: 0.1,
                max_threshold: 0.9,
                min_span: 4,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_h.width(), 30);

        // Test diagonal sort
        let res_d = apply_pixel_sort(
            &img,
            &PixelSortParams {
                direction: PixelSortDirection::Diagonal,
                criteria: PixelSortCriteria::Saturation,
                order: PixelSortOrder::Ascending,
                min_threshold: 0.0,
                max_threshold: 1.0,
                min_span: 3,
                invert_mask: false,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_d.width(), 30);
    }

    #[test]
    fn test_cmyk_halftone_execution() {
        let mut img = RgbaImage::new(40, 40);
        for x in 0..40 {
            for y in 0..40 {
                let r = (x * 6) as u8;
                let g = (y * 6) as u8;
                let b = ((x + y) * 3) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }

        let params = CmykHalftoneParams {
            dot_size: 6.0,
            dot_gain: 1.2,
            paper_tint: PaperTint::Newsprint,
            cyan_offset: [-2.0, 1.0],
            magenta_offset: [1.5, -1.5],
            yellow_offset: [0.0, 2.0],
            black_offset: [0.0, 0.0],
            depth_mask: DepthMaskConfig::default(),
        };

        let res = apply_cmyk_halftone(&img, &params);
        assert_eq!(res.width(), 40);
        assert_eq!(res.height(), 40);

        // Verify paper tint substrate is applied in areas
        let p = res.get_pixel(0, 0);
        assert_eq!(p[3], 255);
    }

    #[test]
    fn test_dither_palette_execution() {
        let mut img = RgbaImage::new(32, 32);
        for x in 0..32 {
            for y in 0..32 {
                let v = (x * 8) as u8;
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }

        // Test Game Boy Bayer 4x4
        let res_gb = apply_dither_palette(
            &img,
            &DitherPaletteParams {
                palette: RetroPalette::GameBoy,
                method: DitherMethod::Bayer4x4,
                pixel_scale: 2,
                dither_strength: 0.8,
                contrast: 0.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_gb.width(), 32);
        assert_eq!(res_gb.height(), 32);

        // Test Mac 1-bit Floyd Steinberg
        let res_mac = apply_dither_palette(
            &img,
            &DitherPaletteParams {
                palette: RetroPalette::Macintosh1Bit,
                method: DitherMethod::FloydSteinberg,
                pixel_scale: 1,
                dither_strength: 0.9,
                contrast: 10.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_mac.width(), 32);
        assert_eq!(res_mac.height(), 32);

        // Test PICO-8 Blue Noise
        let res_pico = apply_dither_palette(
            &img,
            &DitherPaletteParams {
                palette: RetroPalette::Pico8,
                method: DitherMethod::BlueNoise,
                pixel_scale: 2,
                dither_strength: 0.7,
                contrast: -5.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        assert_eq!(res_pico.width(), 32);
    }

    #[test]
    fn test_ntsc_effect_execution() {
        let mut img = RgbaImage::new(48, 48);
        for x in 0..48 {
            for y in 0..48 {
                let r = (x * 5) as u8;
                let g = (y * 5) as u8;
                let b = ((x + y) * 2) as u8;
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }

        // Test default NTSC VHS parameters
        let params = NtscParams::default();
        let res = apply_ntsc_effect(&img, &params);
        assert_eq!(res.width(), 48);
        assert_eq!(res.height(), 48);

        // Alpha channel should remain intact
        for y in 0..48 {
            for x in 0..48 {
                assert_eq!(res.get_pixel(x, y)[3], 255);
            }
        }

        // Test with custom Bad Tracking preset params
        let tracking_params = NtscParams {
            luma_smear: 0.4,
            composite_sharpening: 1.5,
            ringing: 1.0,
            snow: 0.005,
            snow_anisotropy: 0.8,
            scanlines: false,
            phase_shift: NtscPhaseShift::Degrees270,
            demodulation: NtscDemodulation::Notch,
            chroma_lowpass: NtscChromaLowpass::Full,
            chroma_delay_x: 3.0,
            chroma_delay_y: 2,
            chroma_vert_blend: false,
            chroma_phase_error: 0.0,
            chroma_phase_noise: 0.05,
            chroma_noise: 0.20,
            luma_noise: 0.05,
            composite_noise: 0.10,
            vhs_enabled: true,
            tape_speed: VhsTapeSpeed::EP,
            edge_wave: 1.2,
            head_switching: true,
            tracking_noise: 0.5,
            vhs_sharpen: 0.5,
            vhs_chroma_loss: 0.2,
            random_seed: 42,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_tracking = apply_ntsc_effect(&img, &tracking_params);
        assert_eq!(res_tracking.width(), 48);
        assert_eq!(res_tracking.height(), 48);
    }

    #[test]
    fn test_structure_identity_at_zero() {
        let mut img = RgbaImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([(x * 7) as u8, (y * 7) as u8, ((x + y) * 3) as u8, 255]);
        }
        let params = StructureParams {
            amount: 0.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res = apply_structure(&img, &params);
        assert_eq!(res.as_raw(), img.as_raw());
    }

    #[test]
    fn test_structure_enhances_texture_variance() {
        // Create an image with mid-frequency texture pattern (checkerboard ripples)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                // Base midtone 128 + texture variation +- 15
                let tex = if ((x / 4) + (y / 4)) % 2 == 0 {
                    15i16
                } else {
                    -15i16
                };
                let val = (128 + tex) as u8;
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        // Calculate variance of original texture patch
        let calc_variance = |image: &RgbaImage| -> f32 {
            let mut sum = 0.0f32;
            let mut sum_sq = 0.0f32;
            let count = (image.width() * image.height()) as f32;
            for p in image.pixels() {
                let v = p[0] as f32;
                sum += v;
                sum_sq += v * v;
            }
            let mean = sum / count;
            sum_sq / count - mean * mean
        };

        let var_orig = calc_variance(&img);

        let params_boost = StructureParams {
            amount: 60.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_boost = apply_structure(&img, &params_boost);
        let var_boost = calc_variance(&res_boost);

        // Positive structure must amplify texture variance
        assert!(
            var_boost > var_orig * 1.15,
            "Expected texture variance to increase: orig={var_orig}, boost={var_boost}"
        );
    }

    #[test]
    fn test_structure_edge_protection_vs_unsharp_mask() {
        // Create an image with a high-contrast step edge:
        // Left half = 30 (dark), Right half = 225 (bright)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let val = if x < 32 { 30u8 } else { 225u8 };
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        // Naive unsharp mask calculation at edge:
        // blur with sigma ~2
        let mut luma = vec![0.0f32; 64 * 64];
        for (i, p) in img.pixels().enumerate() {
            luma[i] = p[0] as f32 / 255.0;
        }
        let mut blurred = vec![0.0f32; 64 * 64];
        fast_gaussian_blur(&luma, &mut blurred, 64, 64, 2.0);

        // Naive unsharp mask: out = orig + 1.2 * (orig - blur)
        // At pixel x=32 (just past edge), blur is ~ (30 + 225)/2 = 127.5
        // orig - blur = 225 - 127.5 = +97.5 => severe overshoot (halo)!
        let edge_idx_right = 32 * 64 + 32;
        let naive_unsharp_overshoot = (luma[edge_idx_right] - blurred[edge_idx_right]) * 1.2;

        // Now run Structure with edge protection
        let params = StructureParams {
            amount: 60.0,
            detail_scale: 0.0,
            edge_protection: 70.0, // strong edge protection,
            depth_mask: DepthMaskConfig::default(),
        };
        let res = apply_structure(&img, &params);
        let struct_luma_edge = res.get_pixel(32, 32)[0] as f32 / 255.0;
        let struct_overshoot = (struct_luma_edge - luma[edge_idx_right]).abs();

        // Structure overshoot at step edge must be vastly lower than naive unsharp mask
        assert!(
            struct_overshoot < naive_unsharp_overshoot * 0.25,
            "Structure must protect step edges from halos: naive_overshoot={naive_unsharp_overshoot}, struct_overshoot={struct_overshoot}"
        );
    }

    #[test]
    fn test_structure_negative_texture_suppression() {
        // Create an image with both a strong boundary and fine texture on one side:
        // x < 32: dark background (val = 40)
        // x >= 32: textured object (base = 180 + alternating 10)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                if x < 32 {
                    img.put_pixel(x, y, Rgba([40, 40, 40, 255]));
                } else {
                    let tex = if ((x / 2) + (y / 2)) % 2 == 0 {
                        12i16
                    } else {
                        -12i16
                    };
                    let val = (180 + tex) as u8;
                    img.put_pixel(x, y, Rgba([val, val, val, 255]));
                }
            }
        }

        // Measure texture variance in the textured region (x >= 40, away from the boundary)
        let calc_region_variance = |image: &RgbaImage| -> f32 {
            let mut sum = 0.0f32;
            let mut sum_sq = 0.0f32;
            let mut count = 0.0f32;
            for y in 0..64 {
                for x in 40..64 {
                    let v = image.get_pixel(x, y)[0] as f32;
                    sum += v;
                    sum_sq += v * v;
                    count += 1.0;
                }
            }
            let mean = sum / count;
            sum_sq / count - mean * mean
        };

        let var_orig = calc_region_variance(&img);

        let params_smooth = StructureParams {
            amount: -60.0, // negative structure
            detail_scale: 0.0,
            edge_protection: 65.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res_smooth = apply_structure(&img, &params_smooth);
        let var_smooth = calc_region_variance(&res_smooth);

        // Texture variance must be suppressed
        assert!(
            var_smooth < var_orig * 0.70,
            "Expected negative structure to suppress texture variance: orig={var_orig}, smooth={var_smooth}"
        );

        // Meanwhile, the step boundary at x=31 / x=32 must remain crisp
        let dark_edge = res_smooth.get_pixel(30, 32)[0];
        let bright_edge = res_smooth.get_pixel(33, 32)[0];
        assert!(
            bright_edge as i16 - dark_edge as i16 > 110,
            "Boundary contrast should be preserved under negative structure: dark={dark_edge}, bright={bright_edge}"
        );
    }

    #[test]
    fn test_structure_flat_area_coring() {
        // Create an almost flat image with subtle sub-threshold sensor noise (+-1 sRGB step)
        let mut img = RgbaImage::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let noise = if (x * 17 + y * 31) % 5 == 0 {
                    1i16
                } else {
                    0i16
                };
                let val = (180 + noise) as u8;
                img.put_pixel(x, y, Rgba([val, val, val, 255]));
            }
        }

        let params = StructureParams {
            amount: 50.0,
            detail_scale: 0.0,
            edge_protection: 50.0,
            depth_mask: DepthMaskConfig::default(),
        };
        let res = apply_structure(&img, &params);

        // Maximum pixel deviation in flat area should remain tiny (<= 1) due to coring
        for y in 0..64 {
            for x in 0..64 {
                let orig = img.get_pixel(x, y)[0] as i16;
                let processed = res.get_pixel(x, y)[0] as i16;
                assert!(
                    (processed - orig).abs() <= 1,
                    "Flat area noise should not be amplified: orig={orig}, processed={processed}"
                );
            }
        }
    }

    #[test]
    fn test_structure_detail_scale_frequency_response() {
        // Create fine pattern (period 2px) and coarse pattern (period 16px)
        let mut img_fine = RgbaImage::new(64, 64);
        let mut img_coarse = RgbaImage::new(64, 64);

        for y in 0..64 {
            for x in 0..64 {
                let f_val = if (x % 2) == 0 { 140u8 } else { 116u8 };
                let c_val = if ((x / 8) % 2) == 0 { 140u8 } else { 116u8 };
                img_fine.put_pixel(x, y, Rgba([f_val, f_val, f_val, 255]));
                img_coarse.put_pixel(x, y, Rgba([c_val, c_val, c_val, 255]));
            }
        }

        let calc_var = |image: &RgbaImage| -> f32 {
            let mut sum = 0.0f32;
            let mut sum_sq = 0.0f32;
            for p in image.pixels() {
                let v = p[0] as f32;
                sum += v;
                sum_sq += v * v;
            }
            let count = (image.width() * image.height()) as f32;
            let mean = sum / count;
            sum_sq / count - mean * mean
        };

        let var_fine_orig = calc_var(&img_fine);
        let var_coarse_orig = calc_var(&img_coarse);

        // Fine bias (-50 detail_scale)
        let res_fine_biased = apply_structure(
            &img_fine,
            &StructureParams {
                amount: 50.0,
                detail_scale: -50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_fine_under_fine_bias = calc_var(&res_fine_biased) / var_fine_orig;

        // Coarse bias (+50 detail_scale)
        let res_fine_under_coarse_bias = apply_structure(
            &img_fine,
            &StructureParams {
                amount: 50.0,
                detail_scale: 50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_fine_under_coarse_bias = calc_var(&res_fine_under_coarse_bias) / var_fine_orig;

        // Fine pattern must receive higher gain under fine bias than under coarse bias
        assert!(
            gain_fine_under_fine_bias > gain_fine_under_coarse_bias,
            "Fine bias should boost fine texture more than coarse bias: fine_bias={gain_fine_under_fine_bias}, coarse_bias={gain_fine_under_coarse_bias}"
        );

        // Similarly for coarse pattern under coarse bias vs fine bias
        let res_coarse_under_coarse_bias = apply_structure(
            &img_coarse,
            &StructureParams {
                amount: 50.0,
                detail_scale: 50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_coarse_under_coarse_bias =
            calc_var(&res_coarse_under_coarse_bias) / var_coarse_orig;

        let res_coarse_under_fine_bias = apply_structure(
            &img_coarse,
            &StructureParams {
                amount: 50.0,
                detail_scale: -50.0,
                edge_protection: 50.0,
                depth_mask: DepthMaskConfig::default(),
            },
        );
        let gain_coarse_under_fine_bias = calc_var(&res_coarse_under_fine_bias) / var_coarse_orig;

        assert!(
            gain_coarse_under_coarse_bias > gain_coarse_under_fine_bias,
            "Coarse bias should boost coarse structure more than fine bias: coarse_bias={gain_coarse_under_coarse_bias}, fine_bias={gain_coarse_under_fine_bias}"
        );
    }

    #[test]
    fn test_apply_depth_mask_continuous_blending() {
        let orig = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 255]));
        let fx = RgbaImage::from_pixel(2, 2, Rgba([200, 200, 200, 255]));
        let mut depth = RgbaImage::new(2, 2);
        depth.put_pixel(0, 0, Rgba([0, 0, 0, 255])); // 0% fx -> orig 0
        depth.put_pixel(1, 0, Rgba([128, 128, 128, 255])); // ~50% fx -> ~100
        depth.put_pixel(0, 1, Rgba([255, 255, 255, 255])); // 100% fx -> 200
        depth.put_pixel(1, 1, Rgba([64, 64, 64, 255])); // ~25% fx -> ~50

        let config = DepthMaskConfig {
            enabled: true,
            invert: false,
            threshold: 0.0,
            feather: 0.1,
        };

        let masked = apply_depth_mask(&orig, &fx, &depth, &config);
        assert_eq!(masked.get_pixel(0, 0)[0], 0);
        let p10 = masked.get_pixel(1, 0)[0];
        assert!((p10 as i32 - 100).abs() <= 2, "Expected ~100, got {}", p10);
        assert_eq!(masked.get_pixel(0, 1)[0], 200);

        // Test invert
        let inv_config = DepthMaskConfig {
            enabled: true,
            invert: true,
            threshold: 0.0,
            feather: 0.1,
        };
        let inv_masked = apply_depth_mask(&orig, &fx, &depth, &inv_config);
        assert_eq!(inv_masked.get_pixel(0, 0)[0], 200); // 1.0 - 0 = 1.0 fx
        assert_eq!(inv_masked.get_pixel(0, 1)[0], 0); // 1.0 - 1.0 = 0.0 fx
    }

    #[test]
    fn test_apply_depth_mask_threshold_and_disabled() {
        let orig = RgbaImage::from_pixel(2, 2, Rgba([50, 50, 50, 255]));
        let fx = RgbaImage::from_pixel(2, 2, Rgba([150, 150, 150, 255]));
        let mut depth = RgbaImage::new(2, 2);
        depth.put_pixel(0, 0, Rgba([50, 50, 50, 255])); // below threshold
        depth.put_pixel(1, 1, Rgba([220, 220, 220, 255])); // above threshold

        // Disabled returns fx
        let disabled_cfg = DepthMaskConfig {
            enabled: false,
            invert: false,
            threshold: 0.5,
            feather: 0.1,
        };
        let res_dis = apply_depth_mask(&orig, &fx, &depth, &disabled_cfg);
        assert_eq!(res_dis.get_pixel(0, 0)[0], 150);

        // Threshold 0.5 with feather 0.05
        let thresh_cfg = DepthMaskConfig {
            enabled: true,
            invert: false,
            threshold: 0.5,
            feather: 0.05,
        };
        let res_thresh = apply_depth_mask(&orig, &fx, &depth, &thresh_cfg);
        // (0,0) depth is ~0.20 < 0.45 -> weight 0 -> orig (50)
        assert_eq!(res_thresh.get_pixel(0, 0)[0], 50);
        // (1,1) depth is ~0.86 > 0.55 -> weight 1 -> fx (150)
        assert_eq!(res_thresh.get_pixel(1, 1)[0], 150);
    }
}
