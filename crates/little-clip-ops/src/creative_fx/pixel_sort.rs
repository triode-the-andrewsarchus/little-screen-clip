//! Pixel sorting effect.

use super::DepthMaskConfig;
use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
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
