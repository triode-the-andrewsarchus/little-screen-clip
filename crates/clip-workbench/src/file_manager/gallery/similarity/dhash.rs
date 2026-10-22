//! 64-bit difference hash (d-hash) computed from downsampled grayscale gradients.
use little_clip_core::ImageThumbnail;
use std::{path::Path, sync::atomic::AtomicBool};

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

    let mut gray = [[0u16; 9]; 8];
    for y in 0..8 {
        let src_y = (y * height) / 8;
        let row_offset = src_y * width * 4;
        for x in 0..9 {
            let src_x = (x * width) / 9;
            let idx = row_offset + src_x * 4;
            if idx + 3 < thumbnail.premultiplied_rgba.len() {
                let a = thumbnail.premultiplied_rgba[idx + 3] as u32;
                let (r, g, b) = if a > 0 {
                    (
                        ((thumbnail.premultiplied_rgba[idx] as u32) * 255 / a).min(255),
                        ((thumbnail.premultiplied_rgba[idx + 1] as u32) * 255 / a).min(255),
                        ((thumbnail.premultiplied_rgba[idx + 2] as u32) * 255 / a).min(255),
                    )
                } else {
                    (0, 0, 0)
                };
                // ITU-R BT.601 luma formula
                gray[y][x] = ((r * 299 + g * 587 + b * 114) / 1000) as u16;
            }
        }
    }

    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            if gray[y][x] < gray[y][x + 1] {
                hash |= 1u64 << (y * 8 + x);
            }
        }
    }
    hash
}

/// Computes the d-hash directly from a regular image file using thumbnail loading.
pub fn compute_dhash_from_path(path: &Path, cancelled: &AtomicBool) -> Option<u64> {
    // 64px is sufficient resolution for a 9x8 gradient hash and is extremely fast.
    let thumb = super::super::thumbnail::load(path, 64, 1, cancelled)?;
    Some(compute_dhash(&thumb))
}

/// Computes the Hamming distance (number of bit differences) between two 64-bit hashes.
#[allow(dead_code)]
#[inline]
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
