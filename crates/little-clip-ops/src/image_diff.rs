//! Perceptual image comparison engine.
//!
//! Provides multithreaded YIQ-space perceptual image comparison (compatible with ODiff/pixelmatch),
//! producing difference masks, dimmed overlays with highlighted diff pixels, and diff percentage metrics.

use image::{ImageBuffer, RgbaImage};
use little_clip_core::CommandRequest;
use std::path::Path;

/// Errors that can occur during image diff decoding and comparison.
#[derive(Debug, thiserror::Error)]
pub enum ImageDiffError {
    /// Failed to decode image buffer.
    #[error("Failed to decode image: {0}")]
    DecodeError(#[from] image::ImageError),
    /// File I/O failure while reading image from disk.
    #[error("Failed to read file '{0}': {1}")]
    IoError(String, std::io::Error),
    /// Image data was empty or invalid format.
    #[error("Unsupported or empty image data")]
    EmptyData,
    /// SVG rendering failed (e.g. rsvg-convert missing or invalid SVG markup).
    #[error("Failed to render SVG vector data: {0}. Ensure 'rsvg-convert' (librsvg) is installed.")]
    SvgRenderError(String),
}

/// Configuration options for perceptual image comparison.
#[derive(Debug, Clone)]
pub struct ImageDiffOptions {
    /// Perceptual difference threshold in range 0.0 .. 1.0 (default 0.1, matching ODiff).
    pub threshold: f32,
    /// RGBA color used to highlight differing pixels (default: [255, 0, 0, 255] bright red).
    pub diff_color: [u8; 4],
    /// Whether to generate the black-background diff mask (matching ODiff demo).
    pub output_mask: bool,
    /// Whether to generate the dimmed base overlay with glowing diff pixels.
    pub output_overlay: bool,
}

impl Default for ImageDiffOptions {
    fn default() -> Self {
        Self {
            threshold: 0.1,
            diff_color: [255, 0, 0, 255],
            output_mask: true,
            output_overlay: true,
        }
    }
}

/// Results of a perceptual image comparison.
#[derive(Debug, Clone)]
pub struct ImageDiffResult {
    /// Comparison canvas width in pixels.
    pub width: u32,
    /// Comparison canvas height in pixels.
    pub height: u32,
    /// Total number of differing pixels detected.
    pub diff_count: usize,
    /// Total pixels evaluated across width × height.
    pub total_pixels: usize,
    /// Percentage of differing pixels (0.0 to 100.0).
    pub diff_percentage: f64,
    /// True if both images have identical dimensions and zero differing pixels.
    pub is_identical: bool,
    /// Raw RGBA bytes for base image.
    pub base_rgba: Vec<u8>,
    /// Raw RGBA bytes for comparison image.
    pub comp_rgba: Vec<u8>,
    /// Raw RGBA bytes for diff mask (black background with diff color markers).
    pub mask_rgba: Vec<u8>,
    /// Raw RGBA bytes for dimmed overlay with diff highlights.
    pub overlay_rgba: Vec<u8>,
}

impl ImageDiffResult {
    /// Encodes the diff mask image (black background + red diff pixels) as PNG bytes
    pub fn encode_mask_png(&self) -> Result<Vec<u8>, image::ImageError> {
        let img: RgbaImage = ImageBuffer::from_raw(self.width, self.height, self.mask_rgba.clone())
            .ok_or_else(|| {
                image::ImageError::Parameter(image::error::ParameterError::from_kind(
                    image::error::ParameterErrorKind::DimensionMismatch,
                ))
            })?;
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        Ok(buf.into_inner())
    }

    /// Encodes the diff overlay image (dimmed base + red diff pixels) as PNG bytes
    pub fn encode_overlay_png(&self) -> Result<Vec<u8>, image::ImageError> {
        let img: RgbaImage =
            ImageBuffer::from_raw(self.width, self.height, self.overlay_rgba.clone()).ok_or_else(
                || {
                    image::ImageError::Parameter(image::error::ParameterError::from_kind(
                        image::error::ParameterErrorKind::DimensionMismatch,
                    ))
                },
            )?;
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        Ok(buf.into_inner())
    }
}

/// Writes diff highlight color into mask and overlay buffers at `idx`.
#[inline(always)]
fn write_diff_pixel(
    m_chunk: &mut [u8],
    o_chunk: &mut [u8],
    idx: usize,
    diff_r: u8,
    diff_g: u8,
    diff_b: u8,
    diff_a: u8,
) {
    m_chunk[idx] = diff_r;
    m_chunk[idx + 1] = diff_g;
    m_chunk[idx + 2] = diff_b;
    m_chunk[idx + 3] = diff_a;

    o_chunk[idx] = diff_r;
    o_chunk[idx + 1] = diff_g;
    o_chunk[idx + 2] = diff_b;
    o_chunk[idx + 3] = diff_a;
}

/// Writes matching pixel as black in mask and dimmed grayscale in overlay at `idx`.
#[inline(always)]
fn write_identical_pixel(m_chunk: &mut [u8], o_chunk: &mut [u8], idx: usize, r: u8, g: u8, b: u8) {
    m_chunk[idx] = 0;
    m_chunk[idx + 1] = 0;
    m_chunk[idx + 2] = 0;
    m_chunk[idx + 3] = 255;

    let gray = ((0.299 * (r as f32) + 0.587 * (g as f32) + 0.114 * (b as f32)) * 0.45) as u8;
    o_chunk[idx] = gray;
    o_chunk[idx + 1] = gray;
    o_chunk[idx + 2] = gray;
    o_chunk[idx + 3] = 255;
}

/// Computes the perceptual difference between two RGBA images using the NTSC YIQ color space
/// (matching ODiff and pixelmatch algorithms), accelerated across all CPU cores.
pub fn compare_rgba_images(
    base: &RgbaImage,
    comp: &RgbaImage,
    options: &ImageDiffOptions,
) -> ImageDiffResult {
    let (w1, h1) = base.dimensions();
    let (w2, h2) = comp.dimensions();

    let width = w1.max(w2);
    let height = h1.max(h2);
    let total_pixels = (width as usize) * (height as usize);

    if total_pixels == 0 {
        return ImageDiffResult {
            width: 0,
            height: 0,
            diff_count: 0,
            total_pixels: 0,
            diff_percentage: 0.0,
            is_identical: true,
            base_rgba: Vec::new(),
            comp_rgba: Vec::new(),
            mask_rgba: Vec::new(),
            overlay_rgba: Vec::new(),
        };
    }

    let threshold_sq = options.threshold * options.threshold;
    let buf_len = total_pixels * 4;

    let diff_r = options.diff_color[0];
    let diff_g = options.diff_color[1];
    let diff_b = options.diff_color[2];
    let diff_a = options.diff_color[3];

    let same_dimensions = w1 == w2 && h1 == h2;

    let (base_rgba, comp_rgba, mask_rgba, overlay_rgba, diff_count) = if same_dimensions {
        let base_raw = base.as_raw();
        let comp_raw = comp.as_raw();

        let base_rgba = base_raw.clone();
        let comp_rgba = comp_raw.clone();
        let mut mask_rgba = vec![0u8; buf_len];
        let mut overlay_rgba = vec![0u8; buf_len];

        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(height as usize)
            .max(1);

        let rows_per_thread = (height as usize + num_threads - 1) / num_threads;

        let diff_count = std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(num_threads);

            let mask_chunks: Vec<&mut [u8]> = mask_rgba
                .chunks_mut(rows_per_thread * width as usize * 4)
                .collect();
            let overlay_chunks: Vec<&mut [u8]> = overlay_rgba
                .chunks_mut(rows_per_thread * width as usize * 4)
                .collect();

            for (i, (m_chunk, o_chunk)) in mask_chunks
                .into_iter()
                .zip(overlay_chunks.into_iter())
                .enumerate()
            {
                let start_row = i * rows_per_thread;
                let start_byte = start_row * width as usize * 4;
                let len = m_chunk.len();
                let b_chunk = &base_raw[start_byte..start_byte + len];
                let c_chunk = &comp_raw[start_byte..start_byte + len];

                handles.push(s.spawn(move || {
                    let mut local_diff = 0usize;
                    let num_pixels = len / 4;

                    for p in 0..num_pixels {
                        let idx = p * 4;
                        let b = &b_chunk[idx..idx + 4];
                        let c = &c_chunk[idx..idx + 4];

                        let is_diff = is_pixel_different_fast(b, c, threshold_sq);
                        if is_diff {
                            local_diff += 1;
                            write_diff_pixel(m_chunk, o_chunk, idx, diff_r, diff_g, diff_b, diff_a);
                        } else {
                            write_identical_pixel(m_chunk, o_chunk, idx, b[0], b[1], b[2]);
                        }
                    }

                    local_diff
                }));
            }

            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .sum::<usize>()
        });

        (base_rgba, comp_rgba, mask_rgba, overlay_rgba, diff_count)
    } else {
        // Dimension mismatch fallback: normalized grid with bounds checking
        let mut base_rgba = vec![0u8; buf_len];
        let mut comp_rgba = vec![0u8; buf_len];
        let mut mask_rgba = vec![0u8; buf_len];
        let mut overlay_rgba = vec![0u8; buf_len];

        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(height as usize)
            .max(1);

        let rows_per_thread = (height as usize + num_threads - 1) / num_threads;

        let diff_count = std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(num_threads);

            let base_chunks = base_rgba.chunks_mut(rows_per_thread * width as usize * 4);
            let comp_chunks = comp_rgba.chunks_mut(rows_per_thread * width as usize * 4);
            let mask_chunks = mask_rgba.chunks_mut(rows_per_thread * width as usize * 4);
            let overlay_chunks = overlay_rgba.chunks_mut(rows_per_thread * width as usize * 4);

            let zipped = base_chunks
                .zip(comp_chunks)
                .zip(mask_chunks)
                .zip(overlay_chunks);

            for (i, (((b_chunk, c_chunk), m_chunk), o_chunk)) in zipped.enumerate() {
                let start_row = (i * rows_per_thread) as u32;
                let end_row = ((i + 1) * rows_per_thread as usize).min(height as usize) as u32;

                handles.push(s.spawn(move || {
                    let mut local_diff = 0usize;

                    for y in start_row..end_row {
                        let row_offset = ((y - start_row) * width * 4) as usize;

                        for x in 0..width {
                            let idx = row_offset + (x * 4) as usize;

                            let p1 = if x < w1 && y < h1 {
                                Some(base.get_pixel(x, y))
                            } else {
                                None
                            };

                            let p2 = if x < w2 && y < h2 {
                                Some(comp.get_pixel(x, y))
                            } else {
                                None
                            };

                            if let Some(pix) = p1 {
                                b_chunk[idx] = pix[0];
                                b_chunk[idx + 1] = pix[1];
                                b_chunk[idx + 2] = pix[2];
                                b_chunk[idx + 3] = pix[3];
                            }

                            if let Some(pix) = p2 {
                                c_chunk[idx] = pix[0];
                                c_chunk[idx + 1] = pix[1];
                                c_chunk[idx + 2] = pix[2];
                                c_chunk[idx + 3] = pix[3];
                            }

                            let is_different = match (p1, p2) {
                                (Some(b), Some(c)) => {
                                    is_pixel_different_fast(&b.0, &c.0, threshold_sq)
                                }
                                _ => true,
                            };

                            if is_different {
                                local_diff += 1;
                                write_diff_pixel(
                                    m_chunk, o_chunk, idx, diff_r, diff_g, diff_b, diff_a,
                                );
                            } else if let Some(b) = p1 {
                                write_identical_pixel(m_chunk, o_chunk, idx, b[0], b[1], b[2]);
                            } else {
                                m_chunk[idx] = 0;
                                m_chunk[idx + 1] = 0;
                                m_chunk[idx + 2] = 0;
                                m_chunk[idx + 3] = 255;

                                o_chunk[idx] = 15;
                                o_chunk[idx + 1] = 15;
                                o_chunk[idx + 2] = 15;
                                o_chunk[idx + 3] = 255;
                            }
                        }
                    }

                    local_diff
                }));
            }

            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .sum::<usize>()
        });

        (base_rgba, comp_rgba, mask_rgba, overlay_rgba, diff_count)
    };

    let diff_percentage = if total_pixels > 0 {
        (diff_count as f64 / total_pixels as f64) * 100.0
    } else {
        0.0
    };

    let is_identical = diff_count == 0 && w1 == w2 && h1 == h2;

    ImageDiffResult {
        width,
        height,
        diff_count,
        total_pixels,
        diff_percentage,
        is_identical,
        base_rgba,
        comp_rgba,
        mask_rgba,
        overlay_rgba,
    }
}

/// Computes perceptual color delta between two RGBA pixels in YIQ space.
/// Optimized with 32-bit equality check, zero-allocation opaque fast-path, and SIMD-friendly vector operations.
#[inline(always)]
fn is_pixel_different_fast(b: &[u8], c: &[u8], threshold_sq: f32) -> bool {
    let u1 = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
    let u2 = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
    if u1 == u2 {
        return false;
    }

    if b[3] == 0 && c[3] == 0 {
        return false;
    }

    if b[3] == 255 && c[3] == 255 {
        // Fast-path for opaque pixels (common case)
        let dr = (b[0] as f32 - c[0] as f32) * (1.0 / 255.0);
        let dg = (b[1] as f32 - c[1] as f32) * (1.0 / 255.0);
        let db = (b[2] as f32 - c[2] as f32) * (1.0 / 255.0);

        let dy = 0.29889531 * dr + 0.58662247 * dg + 0.11448223 * db;
        let di = 0.59597799 * dr - 0.27417610 * dg - 0.32180189 * db;
        let dq = 0.21147017 * dr - 0.52261711 * dg + 0.31114694 * db;

        let delta = 0.5053 * (dy * dy) + 0.299 * (di * di) + 0.1957 * (dq * dq);
        return delta > threshold_sq;
    }

    let a1 = b[3] as f32 / 255.0;
    let a2 = c[3] as f32 / 255.0;

    let delta_a = (a1 - a2).abs();
    if delta_a * delta_a > threshold_sq {
        return true;
    }

    let r1 = ((b[0] as f32 / 255.0) * a1) + (1.0 - a1);
    let g1 = ((b[1] as f32 / 255.0) * a1) + (1.0 - a1);
    let b1_c = ((b[2] as f32 / 255.0) * a1) + (1.0 - a1);

    let r2 = ((c[0] as f32 / 255.0) * a2) + (1.0 - a2);
    let g2 = ((c[1] as f32 / 255.0) * a2) + (1.0 - a2);
    let b2_c = ((c[2] as f32 / 255.0) * a2) + (1.0 - a2);

    let y1 = 0.29889531 * r1 + 0.58662247 * g1 + 0.11448223 * b1_c;
    let i1 = 0.59597799 * r1 - 0.27417610 * g1 - 0.32180189 * b1_c;
    let q1 = 0.21147017 * r1 - 0.52261711 * g1 + 0.31114694 * b1_c;

    let y2 = 0.29889531 * r2 + 0.58662247 * g2 + 0.11448223 * b2_c;
    let i2 = 0.59597799 * r2 - 0.27417610 * g2 - 0.32180189 * b2_c;
    let q2 = 0.21147017 * r2 - 0.52261711 * g2 + 0.31114694 * b2_c;

    let dy = y1 - y2;
    let di = i1 - i2;
    let dq = q1 - q2;

    let delta = 0.5053 * (dy * dy) + 0.299 * (di * di) + 0.1957 * (dq * dq);
    delta > threshold_sq
}

/// Decodes image bytes into an `RgbaImage`, supporting SVG rendering via rsvg-convert fallback.
pub fn load_rgba_from_bytes(bytes: &[u8]) -> Result<RgbaImage, ImageDiffError> {
    load_rgba_with_mime(bytes, None)
}

fn load_rgba_with_mime(bytes: &[u8], mime: Option<&str>) -> Result<RgbaImage, ImageDiffError> {
    if bytes.is_empty() {
        return Err(ImageDiffError::EmptyData);
    }

    if let Ok(dyn_img) =
        little_clip_core::item::image_reader(bytes, mime).and_then(|reader| reader.decode())
    {
        return Ok(dyn_img.to_rgba8());
    }

    // Try SVG conversion
    if is_svg(bytes) {
        match render_svg_to_png(bytes) {
            Ok(png_bytes) => {
                if let Ok(dyn_img) = image::load_from_memory(&png_bytes) {
                    return Ok(dyn_img.to_rgba8());
                }
            }
            Err(e) => return Err(ImageDiffError::SvgRenderError(e)),
        }
    }

    Err(ImageDiffError::DecodeError(image::ImageError::Decoding(
        image::error::DecodingError::new(
            image::error::ImageFormatHint::Unknown,
            "Failed to decode image from memory",
        ),
    )))
}

/// Loads an `RgbaImage` from a filesystem path.
pub fn load_rgba_from_path(path: &Path) -> Result<RgbaImage, ImageDiffError> {
    let bytes =
        std::fs::read(path).map_err(|e| ImageDiffError::IoError(path.display().to_string(), e))?;
    let mime = little_clip_core::item::guess_image_mime(path);
    load_rgba_with_mime(&bytes, Some(&mime))
}

/// Compares two images provided as byte slices (PNG, JPEG, WebP, BMP, SVG).
pub fn compare_image_bytes(
    base_bytes: &[u8],
    comp_bytes: &[u8],
    options: &ImageDiffOptions,
) -> Result<ImageDiffResult, ImageDiffError> {
    compare_image_bytes_with_mime(base_bytes, None, comp_bytes, None, options)
}

/// Compares images with MIME hints for formats without signatures, such as TGA.
pub fn compare_image_bytes_with_mime(
    base_bytes: &[u8],
    base_mime: Option<&str>,
    comp_bytes: &[u8],
    comp_mime: Option<&str>,
    options: &ImageDiffOptions,
) -> Result<ImageDiffResult, ImageDiffError> {
    if base_bytes.is_empty() || comp_bytes.is_empty() {
        return Err(ImageDiffError::EmptyData);
    }

    // Decode base and comp concurrently across threads
    let (base_res, comp_res) = std::thread::scope(|s| {
        let h1 = s.spawn(|| load_rgba_with_mime(base_bytes, base_mime));
        let h2 = s.spawn(|| load_rgba_with_mime(comp_bytes, comp_mime));
        (h1.join().unwrap(), h2.join().unwrap())
    });

    let base = base_res?;
    let comp = comp_res?;
    Ok(compare_rgba_images(&base, &comp, options))
}

/// Downsamples an RGBA buffer so neither width nor height exceeds `max_dim`.
/// Uses direct nearest-neighbor resampling without intermediate buffer allocations.
pub fn downsample_rgba(width: u32, height: u32, rgba: &[u8], max_dim: u32) -> (u32, u32, Vec<u8>) {
    if width == 0 || height == 0 || (width <= max_dim && height <= max_dim) {
        return (width, height, rgba.to_vec());
    }

    let scale = (max_dim as f32 / width as f32).min(max_dim as f32 / height as f32);
    let new_w = ((width as f32 * scale).round() as u32).max(1);
    let new_h = ((height as f32 * scale).round() as u32).max(1);

    let mut out = vec![0u8; (new_w * new_h * 4) as usize];
    let x_scale = width as f64 / new_w as f64;
    let y_scale = height as f64 / new_h as f64;

    for dy in 0..new_h {
        let sy = ((dy as f64 + 0.5) * y_scale).floor() as u32;
        let sy = sy.min(height - 1);
        let src_row = (sy * width * 4) as usize;
        let dst_row = (dy * new_w * 4) as usize;

        for dx in 0..new_w {
            let sx = ((dx as f64 + 0.5) * x_scale).floor() as u32;
            let sx = sx.min(width - 1);
            let s_idx = src_row + (sx * 4) as usize;
            let d_idx = dst_row + (dx * 4) as usize;

            out[d_idx..d_idx + 4].copy_from_slice(&rgba[s_idx..s_idx + 4]);
        }
    }

    (new_w, new_h, out)
}

/// Sniffs SVG XML headers from the start of image bytes.
fn is_svg(bytes: &[u8]) -> bool {
    let header = std::str::from_utf8(bytes.get(..1024).unwrap_or(bytes)).unwrap_or("");
    let trimmed = header.trim_start();
    trimmed.starts_with("<svg")
        || (trimmed.starts_with("<?xml") && header.contains("<svg"))
        || header.contains("<svg")
}

/// Invokes `rsvg-convert` CLI to render SVG bytes to PNG format.
fn render_svg_to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let output = CommandRequest::new("rsvg-convert")
        .arg("-a")
        .stdin_bytes(bytes)
        .run()
        .map_err(|error| format!("Failed to execute 'rsvg-convert': {error}"))?;

    if output.status.success() && !output.stdout.is_empty() {
        Ok(output.stdout)
    } else {
        let err_msg = output.stderr_lossy().trim().to_string();
        Err(if err_msg.is_empty() {
            format!(
                "'rsvg-convert' exited with status {:?}",
                output.status.code()
            )
        } else {
            err_msg
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn test_identical_images() {
        let mut base = RgbaImage::new(10, 10);
        for pixel in base.pixels_mut() {
            *pixel = Rgba([50, 100, 150, 255]);
        }
        let comp = base.clone();

        let res = compare_rgba_images(&base, &comp, &ImageDiffOptions::default());
        assert!(res.is_identical);
        assert_eq!(res.diff_count, 0);
        assert_eq!(res.diff_percentage, 0.0);
    }

    #[test]
    fn test_subtle_and_clear_differences() {
        let mut base = RgbaImage::new(20, 20);
        for pixel in base.pixels_mut() {
            *pixel = Rgba([30, 30, 30, 255]);
        }
        let mut comp = base.clone();

        // Alter 4 pixels in comp
        comp.put_pixel(5, 5, Rgba([255, 0, 0, 255]));
        comp.put_pixel(5, 6, Rgba([255, 0, 0, 255]));
        comp.put_pixel(6, 5, Rgba([255, 0, 0, 255]));
        comp.put_pixel(6, 6, Rgba([255, 0, 0, 255]));

        let res = compare_rgba_images(&base, &comp, &ImageDiffOptions::default());
        assert!(!res.is_identical);
        assert_eq!(res.diff_count, 4);
        assert_eq!(res.total_pixels, 400);
        assert_eq!(res.diff_percentage, 1.0);

        // Verify that diff mask has red pixel at (5, 5)
        let idx = ((5 * 20 + 5) * 4) as usize;
        assert_eq!(res.mask_rgba[idx], 255);
        assert_eq!(res.mask_rgba[idx + 1], 0);
        assert_eq!(res.mask_rgba[idx + 2], 0);

        // Verify that an unchanged pixel (0, 0) is black in mask
        assert_eq!(res.mask_rgba[0], 0);
        assert_eq!(res.mask_rgba[1], 0);
        assert_eq!(res.mask_rgba[2], 0);
        assert_eq!(res.mask_rgba[3], 255);
    }

    #[test]
    fn test_dimension_mismatch() {
        let base = RgbaImage::new(10, 10);
        let comp = RgbaImage::new(10, 15); // 5 rows larger

        let res = compare_rgba_images(&base, &comp, &ImageDiffOptions::default());
        assert!(!res.is_identical);
        assert_eq!(res.width, 10);
        assert_eq!(res.height, 15);
        assert_eq!(res.total_pixels, 150);
        // The 5 extra rows (50 pixels) must be diffed
        assert!(res.diff_count >= 50);
    }

    #[test]
    fn test_threshold_sensitivity() {
        let mut base = RgbaImage::new(1, 1);
        base.put_pixel(0, 0, Rgba([100, 100, 100, 255]));

        let mut comp = RgbaImage::new(1, 1);
        comp.put_pixel(0, 0, Rgba([102, 100, 100, 255])); // Tiny change

        // Strict threshold (0.01) detects tiny difference
        let strict = ImageDiffOptions {
            threshold: 0.001,
            ..Default::default()
        };
        let res_strict = compare_rgba_images(&base, &comp, &strict);
        assert_eq!(res_strict.diff_count, 1);

        // Relaxed threshold (0.1) ignores tiny compression artifact
        let relaxed = ImageDiffOptions {
            threshold: 0.1,
            ..Default::default()
        };
        let res_relaxed = compare_rgba_images(&base, &comp, &relaxed);
        assert_eq!(res_relaxed.diff_count, 0);
    }

    #[test]
    fn test_png_encoding_roundtrip() {
        let mut base = RgbaImage::new(4, 4);
        base.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
        let comp = RgbaImage::new(4, 4);

        let res = compare_rgba_images(&base, &comp, &ImageDiffOptions::default());
        let mask_png = res.encode_mask_png().expect("mask png encoding");
        let overlay_png = res.encode_overlay_png().expect("overlay png encoding");

        assert!(!mask_png.is_empty());
        assert!(!overlay_png.is_empty());
        assert_eq!(&mask_png[1..4], b"PNG");
    }
}
