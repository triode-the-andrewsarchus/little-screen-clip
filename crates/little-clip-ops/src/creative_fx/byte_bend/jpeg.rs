use super::{ByteBendParams, Edits, address_offset, is_cancelled};
use image::RgbaImage;
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;

const SIDE: usize = 8;
const COEFFICIENTS: usize = 5;
// Scaled first non-constant cosine basis for an 8-pixel JPEG block.
const WAVE: [i32; SIDE] = [16, 13, 9, 3, -3, -9, -13, -16];

struct BlockImage<'a> {
    image: &'a RgbaImage,
    columns: usize,
}

impl<'a> BlockImage<'a> {
    fn new(image: &'a RgbaImage) -> Self {
        Self {
            image,
            columns: (image.width() as usize).div_ceil(SIDE),
        }
    }

    fn len(&self) -> usize {
        self.columns * (self.image.height() as usize).div_ceil(SIDE) * COEFFICIENTS
    }

    fn coefficients(&self, block: usize) -> [u8; COEFFICIENTS] {
        let left = (block % self.columns * SIDE) as u32;
        let top = (block / self.columns * SIDE) as u32;
        let mut sums = [0; COEFFICIENTS];
        for (y, wave_y) in WAVE.into_iter().enumerate() {
            let y = (top + y as u32).min(self.image.height() - 1);
            for (x, wave_x) in WAVE.into_iter().enumerate() {
                let x = (left + x as u32).min(self.image.width() - 1);
                let [r, g, b, _] = self.image.get_pixel(x, y).0.map(i32::from);
                let luma = (77 * r + 150 * g + 29 * b) / 256;
                sums[0] += luma - 128;
                sums[1] += (b - luma) / 2;
                sums[2] += (r - luma) / 2;
                sums[3] += (luma - 128) * wave_x / 8;
                sums[4] += (luma - 128) * wave_y / 8;
            }
        }
        sums.map(|sum| (sum / 64).clamp(-128, 127) as i8 as u8)
    }
}

pub(super) fn stream_len(image: &RgbaImage) -> usize {
    BlockImage::new(image).len()
}

pub(super) fn sample(image: &RgbaImage, address: u16) -> (usize, u8) {
    let blocks = BlockImage::new(image);
    let index = address_offset(address.into(), blocks.len());
    let coefficients = blocks.coefficients(index / COEFFICIENTS);
    (index, coefficients[index % COEFFICIENTS])
}

pub(super) fn bend(
    image: &RgbaImage,
    output: &mut RgbaImage,
    params: &ByteBendParams,
    cancel: Option<&AtomicBool>,
) {
    let blocks = BlockImage::new(image);
    let edits = Edits::new(params, blocks.len());
    let stride = image.width() as usize * 4;
    output
        .par_chunks_mut(stride * SIDE)
        .enumerate()
        .for_each(|(row, band)| {
            let first_block = row * blocks.columns;
            let end = (first_block + blocks.columns) * COEFFICIENTS;
            if !edits.overlaps(first_block * COEFFICIENTS..end) {
                return;
            }
            for column in 0..blocks.columns {
                if is_cancelled(cancel) {
                    return;
                }
                let block = first_block + column;
                let start = block * COEFFICIENTS;
                if edits.overlaps(start..start + COEFFICIENTS) {
                    let delta = coefficient_deltas(&blocks, block, &edits);
                    add_block_delta(band, stride, column * SIDE, delta);
                }
            }
        });
}

fn coefficient_deltas(blocks: &BlockImage<'_>, block: usize, edits: &Edits) -> [i32; COEFFICIENTS] {
    let original = blocks.coefficients(block);
    std::array::from_fn(|channel| {
        let index = block * COEFFICIENTS + channel;
        let Some((_, edit)) = edits.0.iter().find(|(range, _)| range.contains(&index)) else {
            return 0;
        };
        let byte = original[channel];
        let changed = edit.replace(byte, index, |back| {
            blocks.coefficients(block.saturating_sub(back))[channel]
        });
        i32::from(changed as i8) - i32::from(byte as i8)
    })
}

// Apply only the inverse transform of the changed coefficients, retaining the
// original detail outside this small model and preserving the alpha channel.
fn add_block_delta(band: &mut [u8], stride: usize, left: usize, delta: [i32; COEFFICIENTS]) {
    let [luma, cb, cr, horizontal, vertical] = delta;
    let width = (stride / 4 - left).min(SIDE);
    let start = left * 4;
    for (row, wave_y) in band.chunks_exact_mut(stride).zip(WAVE) {
        for (pixel, wave_x) in row[start..start + width * 4].chunks_exact_mut(4).zip(WAVE) {
            let y = luma + (horizontal * wave_x + vertical * wave_y) / 16;
            let channels = [y + 2 * cr, y - (154 * cr + 58 * cb) / 150, y + 2 * cb];
            for (byte, change) in pixel.iter_mut().zip(channels) {
                *byte = (i32::from(*byte) + change).clamp(0, 255) as u8;
            }
        }
    }
}
