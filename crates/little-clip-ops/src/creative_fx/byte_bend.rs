//! Editable RGB prediction residuals for the Byte Bend effect.

use super::{DepthMaskConfig, is_cancelled};
use image::RgbaImage;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{ops::Range, sync::atomic::AtomicBool};

const MIN_BAND_ROWS: usize = 32;
const ADDRESS_SPACE: u64 = 65_536;

/// Maximum number of staged edits in the byte map.
pub const MAX_BYTE_BEND_EDITS: usize = 16;

/// A replacement rule for a region of prediction bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ByteBendMode {
    #[default]
    FlipBits,
    Repeat,
    Dropout,
}

impl ByteBendMode {
    /// Returns the control label for this mutation.
    pub fn label(self) -> &'static str {
        match self {
            Self::FlipBits => "Bit Flip",
            Self::Repeat => "Echo",
            Self::Dropout => "Dropout",
        }
    }
}

/// One edit in the normalized address space of the RGB stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteBendEdit {
    pub address: u16,
    pub span: u16,
    pub mode: ByteBendMode,
    pub variation: u8,
}

/// Staged byte edits and the optional spatial mask.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ByteBendParams {
    pub edits: Vec<ByteBendEdit>,
    pub depth_mask: DepthMaskConfig,
}

fn address_offset(address: u16, len: usize) -> usize {
    (u64::from(address) * len as u64 / ADDRESS_SPACE) as usize
}

impl ByteBendEdit {
    fn window(self, len: usize) -> Range<usize> {
        let center = address_offset(self.address, len);
        let size = address_offset(self.span, len).max(1).min(len);
        let start = center.saturating_sub(size / 2).min(len - size);
        start..start + size
    }

    fn replace(self, residual: u8, index: usize, image: &RgbaImage) -> u8 {
        match self.mode {
            ByteBendMode::FlipBits => residual ^ (1 << ((self.variation.wrapping_add(2)) % 8)),
            ByteBendMode::Repeat => {
                let stride = image.width() as usize * 3;
                let back = stride * (usize::from(self.variation) + 1);
                sample_residual(image, index.saturating_sub(back))
            }
            ByteBendMode::Dropout => {
                let run = usize::from(self.variation).max(1) * 3;
                if self.variation == 0 || (index / run).is_multiple_of(2) {
                    0
                } else {
                    residual
                }
            }
        }
    }
}

/// Returns the byte offset and residual sampled by the map at a normalized address.
pub fn byte_bend_sample(image: &RgbaImage, address: u16) -> Option<(usize, u8)> {
    if image.is_empty() {
        return None;
    }
    let index = address_offset(address, image.as_raw().len() / 4 * 3);
    Some((index, sample_residual(image, index)))
}

fn band_rows(image: &RgbaImage) -> usize {
    (image.height() as usize).div_ceil(12).max(MIN_BAND_ROWS)
}

fn sample_residual(image: &RgbaImage, index: usize) -> u8 {
    let width = image.width() as usize;
    let pixel = index / 3;
    let offset = pixel * 4 + index % 3;
    let should_predict = !(pixel / width).is_multiple_of(band_rows(image));
    let predicted = if should_predict {
        image.as_raw()[offset - width * 4]
    } else {
        0
    };
    image.as_raw()[offset].wrapping_sub(predicted)
}

pub(super) fn apply_byte_bend_cancellable(
    image: &RgbaImage,
    params: &ByteBendParams,
    cancel: Option<&AtomicBool>,
) -> Option<RgbaImage> {
    if is_cancelled(cancel) {
        return None;
    }
    let mut output = image.clone();
    if image.is_empty() || params.edits.is_empty() {
        return Some(output);
    }
    let row_stride = image.width() as usize * 4;
    let band_stride = row_stride * band_rows(image);
    let len = image.as_raw().len() / 4 * 3;
    // The extra slot is reserved for an audition on top of the staged edits.
    let edits: Vec<_> = params
        .edits
        .iter()
        .rev()
        .take(MAX_BYTE_BEND_EDITS + 1)
        .map(|edit| (edit.window(len), *edit))
        .collect();
    output
        .par_chunks_mut(band_stride)
        .enumerate()
        .for_each(|(band, result)| {
            let bytes_per_band = band_stride / 4 * 3;
            let start = band * bytes_per_band;
            let end = start + result.len() / 4 * 3;
            if edits
                .iter()
                .any(|(range, _)| range.start < end && start < range.end)
            {
                bend_band(
                    result,
                    band * band_stride,
                    row_stride,
                    image,
                    &edits,
                    cancel,
                );
            }
        });
    (!is_cancelled(cancel)).then_some(output)
}

// Predict from the previous decoded row. A damaged residual propagates until
// the next row-band boundary, where the stream has an independent reference.
fn bend_band(
    result: &mut [u8],
    band_start: usize,
    row_stride: usize,
    image: &RgbaImage,
    edits: &[(Range<usize>, ByteBendEdit)],
    cancel: Option<&AtomicBool>,
) {
    let source = &image.as_raw()[band_start..band_start + result.len()];
    let byte_start = band_start / 4 * 3;
    for row in (0..result.len()).step_by(row_stride) {
        if is_cancelled(cancel) {
            return;
        }
        let row_start = byte_start + row / 4 * 3;
        let row_end = row_start + row_stride / 4 * 3;
        let row_edits: Vec<_> = edits
            .iter()
            .filter(|(range, _)| range.start < row_end && row_start < range.end)
            .collect();
        for (x, pixel) in source[row..row + row_stride].chunks_exact(4).enumerate() {
            for (channel, &original) in pixel[..3].iter().enumerate() {
                let offset = row + x * 4 + channel;
                let (source_up, result_up) = if row == 0 {
                    (0, 0)
                } else {
                    (source[offset - row_stride], result[offset - row_stride])
                };
                let index = row_start + x * 3 + channel;
                let residual = original.wrapping_sub(source_up);
                let changed = row_edits
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                    .map_or(residual, |(_, edit)| edit.replace(residual, index, image));
                result[offset] = result_up.wrapping_add(changed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn corrupted_residual_propagates_to_the_band_boundary_and_keeps_alpha() {
        let source = RgbaImage::from_pixel(1, 64, Rgba([60, 90, 120, 200]));
        let params = ByteBendParams {
            edits: vec![ByteBendEdit {
                address: 0,
                span: 0,
                mode: ByteBendMode::FlipBits,
                variation: 0,
            }],
            ..Default::default()
        };
        assert_eq!(byte_bend_sample(&source, 2_048), Some((6, 0)));
        let result = apply_byte_bend_cancellable(&source, &params, None).unwrap();
        assert_eq!(result.get_pixel(0, 0).0, [56, 90, 120, 200]);
        assert_eq!(result.get_pixel(0, 31).0, [56, 90, 120, 200]);
        assert_eq!(result.get_pixel(0, 32), source.get_pixel(0, 32));
    }

    #[test]
    fn untouched_data_round_trips_and_cancelled_work_stops() {
        let source = RgbaImage::from_pixel(4, 8, Rgba([60, 90, 120, 200]));
        let params = ByteBendParams::default();
        assert_eq!(
            apply_byte_bend_cancellable(&source, &params, None),
            Some(source.clone())
        );
        assert!(
            apply_byte_bend_cancellable(&source, &params, Some(&AtomicBool::new(true))).is_none()
        );
    }
}
