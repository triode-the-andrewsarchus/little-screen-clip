//! 64-bit difference hash (d-hash) computed from downsampled grayscale gradients.
use little_clip_core::ImageThumbnail;
use std::{path::Path, sync::atomic::AtomicBool};

/// Horizontal bit dimension of the difference hash.
pub const HASH_WIDTH: usize = 8;
/// Vertical bit dimension of the difference hash.
pub const HASH_HEIGHT: usize = 8;
/// Number of sample columns (9 columns yield 8 horizontal differences).
pub const GRID_COLS: usize = HASH_WIDTH + 1;
/// Number of sample rows in the luminance grid.
pub const GRID_ROWS: usize = HASH_HEIGHT;
/// Target thumbnail edge length in pixels used for d-hash generation.
pub const DHASH_THUMBNAIL_EDGE: u32 = 64;

// ITU-R BT.601 luma coefficients: Y = 0.299 R + 0.587 G + 0.114 B
const LUMA_RED_WEIGHT: u32 = 299;
const LUMA_GREEN_WEIGHT: u32 = 587;
const LUMA_BLUE_WEIGHT: u32 = 114;
const LUMA_DENOMINATOR: u32 = 1000;

/// Computes a 64-bit difference hash from a thumbnail buffer.
///
/// Samples a 9-wide by 8-high grid of grayscale luminance values and compares
/// adjacent horizontal pixels ($I(x, y) < I(x+1, y)$) to form 64 gradient bits.
pub fn compute_dhash(thumbnail: &ImageThumbnail) -> u64 {
    let width = thumbnail.width as usize;
    let height = thumbnail.height as usize;
    if width < 2 || height < 1 || thumbnail.premultiplied_rgba.len() < width * height * 4 {
        return 0;
    }

    let mut gray = [[0u16; GRID_COLS]; GRID_ROWS];
    for (y, gray_row) in gray.iter_mut().enumerate() {
        let src_y = (y * height) / GRID_ROWS;
        let row_offset = src_y * width * 4;
        for (x, gray_val) in gray_row.iter_mut().enumerate() {
            let src_x = (x * width) / GRID_COLS;
            let idx = row_offset + src_x * 4;
            if idx + 3 < thumbnail.premultiplied_rgba.len() {
                let a = thumbnail.premultiplied_rgba[idx + 3] as u32;
                let (r, g, b) = if let Some(a_val) = std::num::NonZeroU32::new(a) {
                    (
                        ((thumbnail.premultiplied_rgba[idx] as u32 * 255) / a_val.get()).min(255),
                        ((thumbnail.premultiplied_rgba[idx + 1] as u32 * 255) / a_val.get())
                            .min(255),
                        ((thumbnail.premultiplied_rgba[idx + 2] as u32 * 255) / a_val.get())
                            .min(255),
                    )
                } else {
                    (0, 0, 0)
                };
                *gray_val = ((r * LUMA_RED_WEIGHT + g * LUMA_GREEN_WEIGHT + b * LUMA_BLUE_WEIGHT)
                    / LUMA_DENOMINATOR) as u16;
            }
        }
    }

    let mut hash = 0u64;
    for (y, gray_row) in gray.iter().enumerate().take(HASH_HEIGHT) {
        for x in 0..HASH_WIDTH {
            if gray_row[x] < gray_row[x + 1] {
                hash |= 1u64 << (y * HASH_WIDTH + x);
            }
        }
    }
    hash
}

/// Computes a difference hash directly from an image file path.
///
/// Returns [`None`] if the thumbnail cannot be decoded or if `cancelled` is set.
pub fn compute_dhash_from_path(path: &Path, cancelled: &AtomicBool) -> Option<u64> {
    // 64px is sufficient resolution for a 9x8 gradient hash and is extremely fast.
    let thumb = super::super::thumbnail::load(path, DHASH_THUMBNAIL_EDGE, 1, cancelled)?;
    Some(compute_dhash(&thumb))
}

/// Computes the Hamming distance (number of differing bits) between two 64-bit hashes.
#[allow(dead_code)]
#[inline]
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
