//! Palette dithering effect.

use super::DepthMaskConfig;
use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
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
    apply_dither_palette_cancellable(img, params, None).expect("uncancelled dithering cannot stop")
}

pub(super) fn apply_dither_palette_cancellable(
    img: &RgbaImage,
    params: &DitherPaletteParams,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Option<RgbaImage> {
    if super::is_cancelled(cancel) {
        return None;
    }
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Some(img.clone());
    }

    let scale = params.pixel_scale.clamp(1, 16) as u32;
    let dw = ((w + scale - 1) / scale).max(1);
    let dh = ((h + scale - 1) / scale).max(1);
    let colors = params.palette.colors();

    // Sample downscaled grid cells
    let mut downsampled: Vec<[f32; 3]> = Vec::with_capacity((dw * dh) as usize);
    for dy in 0..dh {
        if super::is_cancelled(cancel) {
            return None;
        }
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
            if super::is_cancelled(cancel) {
                return None;
            }
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
            if super::is_cancelled(cancel) {
                return None;
            }
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
        if super::is_cancelled(cancel) {
            return;
        }
        let dy = (y / scale).min(dh - 1);
        for (x, _, pixel) in row {
            let dx = (x / scale).min(dw - 1);
            let c = quantized[(dy * dw + dx) as usize];
            let a = img.get_pixel(x, y)[3];
            *pixel = Rgba([c[0], c[1], c[2], a]);
        }
    });

    (!super::is_cancelled(cancel)).then_some(out)
}
