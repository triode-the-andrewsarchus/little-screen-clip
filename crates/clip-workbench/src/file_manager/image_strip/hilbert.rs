//! Hilbert color strip and spatial color source-mapping ported from PaletteAtlas.
//!
//! Based on PaletteAtlas (https://github.com/itzik123/PaletteAtlas) and Jakub Červený's
//! generalized 2D Hilbert curve ("gilbert", 2018).

use std::sync::atomic::{AtomicBool, Ordering};

/// Number of color segments in the strip.
pub const STRIP_SEGMENTS: usize = 128;

/// Maximum fraction of fully transparent pixels allowed before a segment is omitted from the strip.
pub const TRANSPARENT_THRESHOLD: f64 = 0.90;

/// Segments with OKLab L >= 0.6 use dark ink veil; darker ones use light paper veil.
const LIGHT_SEGMENT: f64 = 0.6;

/// Veil opacity (82% opacity: round(0.82 * 255) = 209).
const VEIL_OPACITY: u8 = 209;

/// The page's ink and paper colors from PaletteAtlas.
const INK: (u8, u8, u8) = (0x18, 0x14, 0x0f);
const PAPER: (u8, u8, u8) = (0xf7, 0xf1, 0xe6);

/// Color channels (RGB) for linear accumulation.
const COLOR_CHANNELS: usize = 3;

/// A rectangle longer than 3/2 side to side is cut in two along its length.
const LONG_RATIO_NUM: i32 = 3;
const LONG_RATIO_DEN: i32 = 2;

/// Standard sRGB transfer curve lookup table for byte to linear light.
static BYTE_TO_LINEAR: std::sync::LazyLock<[f64; 256]> = std::sync::LazyLock::new(|| {
    let mut table = [0.0; 256];
    for (i, val) in table.iter_mut().enumerate() {
        let encoded = i as f64 / 255.0;
        *val = if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        };
    }
    table
});

/// Decodes an 8-bit sRGB channel to linear light.
#[inline]
pub fn srgb_byte_to_linear(byte: u8) -> f64 {
    BYTE_TO_LINEAR[byte as usize]
}

/// Encodes linear light as an 8-bit sRGB channel.
#[inline]
pub fn linear_to_srgb_byte(linear: f64) -> u8 {
    let encoded = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Computes the OKLab lightness L (0.0 to 1.0) for an sRGB color.
pub fn oklab_l(r: u8, g: u8, b: u8) -> f64 {
    let r_lin = srgb_byte_to_linear(r);
    let g_lin = srgb_byte_to_linear(g);
    let b_lin = srgb_byte_to_linear(b);

    // Linear sRGB to cone responses (LMS)
    let l_lms = 0.4122214708 * r_lin + 0.5363325363 * g_lin + 0.0514459929 * b_lin;
    let m_lms = 0.2119034982 * r_lin + 0.6806995451 * g_lin + 0.1073969566 * b_lin;
    let s_lms = 0.0883024619 * r_lin + 0.2817188376 * g_lin + 0.6299787005 * b_lin;

    // Cube-rooted cone responses
    let l_ = l_lms.cbrt();
    let m_ = m_lms.cbrt();
    let s_ = s_lms.cbrt();

    // LMS to OKLab lightness
    0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_
}

/// Computes the pixels of a `width × height` image in the order of a generalized Hilbert curve.
///
/// Jakub Červený's generalization to any rectangle ("gilbert", 2018), which matches the
/// classic curve on square powers of two.
pub fn hilbert_curve(width: u32, height: u32) -> Vec<usize> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let total = (width as usize) * (height as usize);
    let mut curve = Vec::with_capacity(total);
    if width >= height {
        fill(
            0,
            0,
            width as i32,
            0,
            0,
            height as i32,
            width as i32,
            &mut curve,
        );
    } else {
        fill(
            0,
            0,
            0,
            height as i32,
            width as i32,
            0,
            width as i32,
            &mut curve,
        );
    }
    curve
}

#[allow(clippy::too_many_arguments)]
fn fill(x: i32, y: i32, ax: i32, ay: i32, bx: i32, by: i32, width: i32, curve: &mut Vec<usize>) {
    let length = (ax + ay).abs();
    let breadth = (bx + by).abs();
    let step_ax = ax.signum();
    let step_ay = ay.signum();
    let step_bx = bx.signum();
    let step_by = by.signum();

    if breadth == 1 {
        line(x, y, step_ax, step_ay, length, width, curve);
        return;
    }
    if length == 1 {
        line(x, y, step_bx, step_by, breadth, width, curve);
        return;
    }

    let mut half_ax = ax >> 1;
    let mut half_ay = ay >> 1;
    let mut half_bx = bx >> 1;
    let mut half_by = by >> 1;

    if length * LONG_RATIO_DEN > breadth * LONG_RATIO_NUM {
        if (half_ax + half_ay).abs() % 2 == 1 && length > 2 {
            half_ax += step_ax;
            half_ay += step_ay;
        }
        fill(x, y, half_ax, half_ay, bx, by, width, curve);
        fill(
            x + half_ax,
            y + half_ay,
            ax - half_ax,
            ay - half_ay,
            bx,
            by,
            width,
            curve,
        );
        return;
    }

    if (half_bx + half_by).abs() % 2 == 1 && breadth > 2 {
        half_bx += step_bx;
        half_by += step_by;
    }
    fill(x, y, half_bx, half_by, half_ax, half_ay, width, curve);
    fill(
        x + half_bx,
        y + half_by,
        ax,
        ay,
        bx - half_bx,
        by - half_by,
        width,
        curve,
    );
    fill(
        x + (ax - step_ax) + (half_bx - step_bx),
        y + (ay - step_ay) + (half_by - step_by),
        -half_bx,
        -half_by,
        -(ax - half_ax),
        -(ay - half_ay),
        width,
        curve,
    );
}

#[inline]
fn line(x: i32, y: i32, dx: i32, dy: i32, count: i32, width: i32, curve: &mut Vec<usize>) {
    for pixel in 0..count {
        let px = x + pixel * dx;
        let py = y + pixel * dy;
        curve.push((py * width + px) as usize);
    }
}

/// Maps each pixel in an image to its corresponding strip segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMap {
    pub width: u32,
    pub height: u32,
    pub segment_count: usize,
    pub segments: Vec<i32>,
}

impl SourceMap {
    /// Cuts a sequence of pixels into `segments` runs of equal length.
    pub fn from_sequence(width: u32, height: u32, sequence: &[usize], segments: usize) -> Self {
        let count = segments.clamp(1, sequence.len().max(1));
        let mut map = vec![-1i32; (width * height) as usize];
        let seq_len = sequence.len() as u64;
        for (pos, &pixel_idx) in sequence.iter().enumerate() {
            if pixel_idx < map.len() {
                let seg = ((pos as u64 * count as u64) / seq_len) as i32;
                map[pixel_idx] = seg;
            }
        }
        Self {
            width,
            height,
            segment_count: count,
            segments: map,
        }
    }

    /// The segment index of the pixel at (x, y), or -1 if outside.
    #[inline]
    pub fn segment_at(&self, x: u32, y: u32) -> i32 {
        if x < self.width && y < self.height {
            self.segments[(y * self.width + x) as usize]
        } else {
            -1
        }
    }
}

/// A visible segment in the Hilbert color strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisibleSegment {
    /// Index in the SourceMap (0..source_map.segment_count).
    pub source_segment: usize,
    /// Averaged opaque sRGB color for this segment.
    pub color: egui::Color32,
}

/// A Hilbert color strip computed from an image's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct HilbertStrip {
    pub source_map: SourceMap,
    pub visible_segments: Vec<VisibleSegment>,
    pub source_colors: Vec<Option<egui::Color32>>,
}

impl HilbertStrip {
    /// Reads an image along a Hilbert curve into `segments` segments, averaging linear light.
    ///
    /// Fully transparent pixels (`alpha == 0`) are excluded from the segment color average.
    /// Segments where more than 90% of the pixels are fully transparent are omitted from the strip.
    pub fn read(image: &image::RgbaImage, segments: usize, cancelled: &AtomicBool) -> Option<Self> {
        let width = image.width();
        let height = image.height();
        if width == 0 || height == 0 {
            return None;
        }

        let curve = hilbert_curve(width, height);
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }

        let source_map = SourceMap::from_sequence(width, height, &curve, segments);
        let count = source_map.segment_count;

        let mut sums = vec![0.0f64; count * COLOR_CHANNELS];
        let mut opacity_sums = vec![0.0f64; count];
        let mut pixel_counts = vec![0usize; count];
        let mut transparent_counts = vec![0usize; count];

        for y in 0..height {
            if y % 32 == 0 && cancelled.load(Ordering::Relaxed) {
                return None;
            }
            for x in 0..width {
                let seg = source_map.segment_at(x, y);
                if seg < 0 || seg as usize >= count {
                    continue;
                }
                let seg_idx = seg as usize;
                let pixel = image.get_pixel(x, y);
                let alpha = pixel[3];
                pixel_counts[seg_idx] += 1;
                if alpha == 0 {
                    transparent_counts[seg_idx] += 1;
                    continue;
                }
                let weight = alpha as f64 / 255.0;
                let offset = seg_idx * COLOR_CHANNELS;
                sums[offset] += weight * srgb_byte_to_linear(pixel[0]);
                sums[offset + 1] += weight * srgb_byte_to_linear(pixel[1]);
                sums[offset + 2] += weight * srgb_byte_to_linear(pixel[2]);
                opacity_sums[seg_idx] += weight;
            }
        }

        if cancelled.load(Ordering::Relaxed) {
            return None;
        }

        let mut visible_segments = Vec::new();
        let mut source_colors = vec![None; count];

        for i in 0..count {
            let total = pixel_counts[i];
            let transparent = transparent_counts[i];
            // If more than 90% of the pixels in a segment are fully transparent, omit it.
            if total == 0 || (transparent as f64 / total as f64) > TRANSPARENT_THRESHOLD {
                continue;
            }

            let opacity = opacity_sums[i];
            if opacity > 0.0 {
                let offset = i * COLOR_CHANNELS;
                let r = linear_to_srgb_byte(sums[offset] / opacity);
                let g = linear_to_srgb_byte(sums[offset + 1] / opacity);
                let b = linear_to_srgb_byte(sums[offset + 2] / opacity);
                let color = egui::Color32::from_rgb(r, g, b);
                visible_segments.push(VisibleSegment {
                    source_segment: i,
                    color,
                });
                source_colors[i] = Some(color);
            }
        }

        Some(Self {
            source_map,
            visible_segments,
            source_colors,
        })
    }

    /// Creates a mask image for the given segment where pixels belonging to `segment` are
    /// uncovered (transparent) and all other pixels are white with 82% opacity.
    pub fn create_veil_mask(&self, segment: usize) -> egui::ColorImage {
        let width = self.source_map.width as usize;
        let height = self.source_map.height as usize;
        let veil_color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, VEIL_OPACITY);

        let mut pixels = Vec::with_capacity(width * height);
        let target_seg = segment as i32;
        for &seg in &self.source_map.segments {
            if seg == target_seg {
                pixels.push(egui::Color32::TRANSPARENT);
            } else {
                pixels.push(veil_color);
            }
        }
        egui::ColorImage {
            size: [width, height],
            pixels,
        }
    }

    /// Returns the target dimmed color (INK or PAPER) for the given segment based on OKLab lightness.
    pub fn dimmed_color_for_segment(&self, segment: usize) -> (u8, u8, u8) {
        let color = self.color_at(segment).unwrap_or(egui::Color32::TRANSPARENT);
        let l = oklab_l(color.r(), color.g(), color.b());
        if l >= LIGHT_SEGMENT { INK } else { PAPER }
    }

    /// Creates a veil image for the given segment where pixels belonging to `segment` are
    /// uncovered (transparent) and all other pixels are covered in INK or PAPER with 82% opacity.
    #[cfg(test)]
    pub fn create_veil_image(&self, segment: usize) -> egui::ColorImage {
        let (vr, vg, vb) = self.dimmed_color_for_segment(segment);
        let width = self.source_map.width as usize;
        let height = self.source_map.height as usize;
        let veil_color = egui::Color32::from_rgba_unmultiplied(vr, vg, vb, VEIL_OPACITY);

        let mut pixels = Vec::with_capacity(width * height);
        let target_seg = segment as i32;
        for &seg in &self.source_map.segments {
            if seg == target_seg {
                pixels.push(egui::Color32::TRANSPARENT);
            } else {
                pixels.push(veil_color);
            }
        }
        egui::ColorImage {
            size: [width, height],
            pixels,
        }
    }

    /// Returns the color of the given source segment, if valid and not omitted.
    pub fn color_at(&self, segment: usize) -> Option<egui::Color32> {
        self.source_colors.get(segment).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SIDE: u32 = 32;

    fn curve_problem(width: u32, height: u32) -> Option<String> {
        let curve = hilbert_curve(width, height);
        let mut visits = vec![0u8; (width * height) as usize];
        for &pixel in &curve {
            if pixel < visits.len() {
                visits[pixel] += 1;
            }
        }
        if curve.len() != (width * height) as usize || visits.iter().any(|&c| c != 1) {
            return Some("not every pixel visited once".into());
        }
        let mut diagonal_steps = 0;
        for step in 1..curve.len() {
            let prev = curve[step - 1];
            let curr = curve[step];
            let prev_x = (prev as u32 % width) as i32;
            let prev_y = (prev as u32 / width) as i32;
            let curr_x = (curr as u32 % width) as i32;
            let curr_y = (curr as u32 / width) as i32;
            let dx = (curr_x - prev_x).abs();
            let dy = (curr_y - prev_y).abs();
            if dx.max(dy) != 1 {
                return Some(format!(
                    "step {step} jumps: from ({prev_x}, {prev_y}) to ({curr_x}, {curr_y})"
                ));
            }
            if dx == 1 && dy == 1 {
                diagonal_steps += 1;
            }
        }
        if diagonal_steps > 1 {
            Some(format!("{diagonal_steps} diagonal steps"))
        } else {
            None
        }
    }

    #[test]
    fn visits_every_pixel_once_each_step_to_neighbor_and_at_most_one_diagonal() {
        let mut problems = Vec::new();
        for width in 1..=MAX_SIDE {
            for height in 1..=MAX_SIDE {
                if let Some(problem) = curve_problem(width, height) {
                    problems.push(format!("{width} × {height}: {problem}"));
                }
            }
        }
        assert!(problems.is_empty(), "Hilbert curve problems: {problems:?}");
    }

    #[test]
    fn fills_aligned_square_blocks_on_power_of_two() {
        let side = 16u32;
        let curve = hilbert_curve(side, side);
        for order in 1..4 {
            let block_side = 1u32 << order;
            let block_area = (block_side * block_side) as usize;
            for start in (0..curve.len()).step_by(block_area) {
                let run = &curve[start..start + block_area];
                let xs: Vec<u32> = run.iter().map(|&p| p as u32 % side).collect();
                let ys: Vec<u32> = run.iter().map(|&p| p as u32 / side).collect();
                let min_x = *xs.iter().min().unwrap();
                let max_x = *xs.iter().max().unwrap();
                let min_y = *ys.iter().min().unwrap();
                let max_y = *ys.iter().max().unwrap();
                assert_eq!(max_x - min_x + 1, block_side);
                assert_eq!(max_y - min_y + 1, block_side);
                assert_eq!(min_x % block_side, 0);
                assert_eq!(min_y % block_side, 0);
            }
        }
    }

    #[test]
    fn source_map_bounds_and_coverage() {
        let curve = hilbert_curve(10, 10);
        let map = SourceMap::from_sequence(10, 10, &curve, 16);
        assert_eq!(map.segment_count, 16);
        for y in 0..10 {
            for x in 0..10 {
                let seg = map.segment_at(x, y);
                assert!((0..16).contains(&seg), "segment out of bounds: {seg}");
            }
        }
    }

    #[test]
    fn hilbert_strip_averaging_and_veil() {
        let mut img = image::RgbaImage::new(4, 4);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([200, 50, 50, 255]);
        }
        let cancelled = AtomicBool::new(false);
        let strip = HilbertStrip::read(&img, 4, &cancelled).unwrap();
        assert_eq!(strip.visible_segments.len(), 4);
        for seg in &strip.visible_segments {
            assert_eq!(seg.color.r(), 200);
            assert_eq!(seg.color.g(), 50);
            assert_eq!(seg.color.b(), 50);
        }

        let veil = strip.create_veil_image(0);
        assert_eq!(veil.size, [4, 4]);
        let mut uncovered = 0;
        let mut veiled = 0;
        for &px in &veil.pixels {
            if px == egui::Color32::TRANSPARENT {
                uncovered += 1;
            } else {
                veiled += 1;
                assert_eq!(px.a(), VEIL_OPACITY);
            }
        }
        assert_eq!(uncovered, 4);
        assert_eq!(veiled, 12);
    }

    #[test]
    fn color_at_returns_valid_colors_and_none_for_out_of_bounds() {
        let mut img = image::RgbaImage::new(2, 2);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba([100, 150, 200, 255]);
        }
        let cancelled = AtomicBool::new(false);
        let strip = HilbertStrip::read(&img, 2, &cancelled).unwrap();
        assert!(strip.color_at(0).is_some());
        assert!(strip.color_at(1).is_some());
        assert_eq!(strip.color_at(2), None);
    }

    #[test]
    fn veil_uses_ink_for_light_segment_and_paper_for_dark_segment() {
        let mut img = image::RgbaImage::new(2, 2);
        // Left column white ((0,0) and (0,1))
        img.put_pixel(0, 0, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(0, 1, image::Rgba([255, 255, 255, 255]));
        // Right column black ((1,0) and (1,1))
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 1, image::Rgba([0, 0, 0, 255]));

        let cancelled = AtomicBool::new(false);
        let strip = HilbertStrip::read(&img, 2, &cancelled).unwrap();

        let l0 = oklab_l(
            strip.visible_segments[0].color.r(),
            strip.visible_segments[0].color.g(),
            strip.visible_segments[0].color.b(),
        );
        let l1 = oklab_l(
            strip.visible_segments[1].color.r(),
            strip.visible_segments[1].color.g(),
            strip.visible_segments[1].color.b(),
        );
        assert!(l0 >= LIGHT_SEGMENT);
        assert!(l1 < LIGHT_SEGMENT);

        let expected_ink = egui::Color32::from_rgba_unmultiplied(INK.0, INK.1, INK.2, VEIL_OPACITY);
        let expected_paper =
            egui::Color32::from_rgba_unmultiplied(PAPER.0, PAPER.1, PAPER.2, VEIL_OPACITY);

        let veil_light = strip.create_veil_image(0);
        let veiled_px_light = veil_light
            .pixels
            .iter()
            .find(|&&p| p != egui::Color32::TRANSPARENT)
            .unwrap();
        assert_eq!(*veiled_px_light, expected_ink);

        let veil_dark = strip.create_veil_image(1);
        let veiled_px_dark = veil_dark
            .pixels
            .iter()
            .find(|&&p| p != egui::Color32::TRANSPARENT)
            .unwrap();
        assert_eq!(*veiled_px_dark, expected_paper);
    }

    #[test]
    fn cancellation_stops_read() {
        let img = image::RgbaImage::new(16, 16);
        let cancelled = AtomicBool::new(true);
        assert!(HilbertStrip::read(&img, 16, &cancelled).is_none());
    }

    #[test]
    fn transparent_pixels_excluded_from_average() {
        let mut img = image::RgbaImage::new(2, 2);
        // (0,0) is bright red; others are fully transparent
        img.put_pixel(0, 0, image::Rgba([200, 40, 40, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        img.put_pixel(0, 1, image::Rgba([0, 0, 0, 0]));
        img.put_pixel(1, 1, image::Rgba([0, 0, 0, 0]));

        let cancelled = AtomicBool::new(false);
        // 1 segment for the whole 2x2 image: 3/4 = 75% transparent <= 90% threshold
        let strip = HilbertStrip::read(&img, 1, &cancelled).unwrap();
        assert_eq!(strip.visible_segments.len(), 1);
        let color = strip.visible_segments[0].color;
        // Color should be exactly the red from (0,0), NOT darkened by (0,0,0,0)
        assert_eq!(color.r(), 200);
        assert_eq!(color.g(), 40);
        assert_eq!(color.b(), 40);
        assert_eq!(color.a(), 255);
    }

    #[test]
    fn segments_over_90_percent_transparent_are_omitted() {
        let mut img = image::RgbaImage::new(10, 2);
        // Left half (10 pixels) is 100% transparent
        for x in 0..5 {
            for y in 0..2 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
        // Right half (10 pixels) is opaque blue
        for x in 5..10 {
            for y in 0..2 {
                img.put_pixel(x, y, image::Rgba([30, 60, 240, 255]));
            }
        }

        let cancelled = AtomicBool::new(false);
        // Request 2 segments: segment 0 covers left half (100% transparent), segment 1 covers right half
        let strip = HilbertStrip::read(&img, 2, &cancelled).unwrap();
        // Segment 0 must be omitted! Only segment 1 remains.
        assert_eq!(strip.visible_segments.len(), 1);
        assert_eq!(strip.visible_segments[0].source_segment, 1);
        assert_eq!(strip.visible_segments[0].color.b(), 240);

        assert_eq!(strip.color_at(0), None);
        assert!(strip.color_at(1).is_some());
    }
}
