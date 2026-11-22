//! Editable codec-inspired image data for the Byte Bend effect.

mod jpeg;

use super::{DepthMaskConfig, is_cancelled};
use image::{ImageFormat, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{ops::Range, sync::atomic::AtomicBool};

const MIN_BAND_ROWS: usize = 32;
const ADDRESS_SPACE: u64 = 65_536;

/// The data model used to bend the current image pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ByteBendProfile {
    #[default]
    Rgb,
    Png,
    Jpeg,
    WebP,
}

impl ByteBendProfile {
    /// Selects a model from the encoded image signature.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match image::guess_format(bytes) {
            Ok(ImageFormat::Png) => Self::Png,
            Ok(ImageFormat::Jpeg) => Self::Jpeg,
            Ok(ImageFormat::WebP) => Self::WebP,
            _ => Self::Rgb,
        }
    }

    /// Returns the active map's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB scanlines",
            Self::Png => "PNG scanlines",
            Self::Jpeg => "JPEG blocks",
            Self::WebP => "WebP tile prediction",
        }
    }
}

/// Maximum number of staged edits in the byte map.
pub const MAX_BYTE_BEND_EDITS: usize = 16;

/// A replacement rule for a region of image data.
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

/// One edit in the normalized address space of the active data model.
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
    #[serde(default)]
    pub profile: ByteBendProfile,
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

    fn replace(self, byte: u8, index: usize, echo: impl FnOnce(usize) -> u8) -> u8 {
        match self.mode {
            ByteBendMode::FlipBits => byte ^ (1 << ((self.variation.wrapping_add(2)) % 8)),
            ByteBendMode::Repeat => echo(usize::from(self.variation) + 1),
            ByteBendMode::Dropout => {
                let run = usize::from(self.variation).max(1) * 3;
                if self.variation == 0 || (index / run).is_multiple_of(2) {
                    0
                } else {
                    byte
                }
            }
        }
    }
}

/// Returns the offset and modeled byte sampled by the map at a normalized address.
pub fn byte_bend_sample(
    image: &RgbaImage,
    profile: ByteBendProfile,
    address: u16,
) -> Option<(usize, u8)> {
    if image.is_empty() {
        return None;
    }
    if profile == ByteBendProfile::Jpeg {
        return Some(jpeg::sample(image, address));
    }
    let index = address_offset(address, image.as_raw().len() / 4 * 3);
    Some((
        index,
        RowPrediction::new(image, profile).sample(image, index),
    ))
}

struct Edits(Vec<(Range<usize>, ByteBendEdit)>);

impl Edits {
    fn new(params: &ByteBendParams, len: usize) -> Self {
        // The extra slot is reserved for an audition on top of the staged edits.
        Self(
            params
                .edits
                .iter()
                .rev()
                .take(MAX_BYTE_BEND_EDITS + 1)
                .map(|edit| (edit.window(len), *edit))
                .collect(),
        )
    }

    fn overlaps(&self, range: Range<usize>) -> bool {
        self.0
            .iter()
            .any(|(edit, _)| edit.start < range.end && range.start < edit.end)
    }
}

const WEBP_TILE_SIDE: usize = 16;

struct RowPrediction {
    stride: usize,
    band_rows: usize,
    tiled: bool,
}

impl RowPrediction {
    fn new(image: &RgbaImage, profile: ByteBendProfile) -> Self {
        let tiled = profile == ByteBendProfile::WebP;
        let band_rows = if tiled {
            WEBP_TILE_SIDE
        } else {
            (image.height() as usize).div_ceil(12).max(MIN_BAND_ROWS)
        };
        Self {
            stride: image.width() as usize * 4,
            band_rows,
            tiled,
        }
    }

    fn sample(&self, image: &RgbaImage, index: usize) -> u8 {
        let offset = index / 3 * 4 + index % 3;
        let x = offset % self.stride / 4;
        let y = offset / self.stride % self.band_rows;
        let raw = image.as_raw();
        raw[offset].wrapping_sub(self.predict(raw, offset, x, y))
    }

    fn predict(&self, bytes: &[u8], offset: usize, x: usize, y: usize) -> u8 {
        let has_left = self.tiled && !x.is_multiple_of(WEBP_TILE_SIDE);
        match (has_left, y > 0) {
            (true, true) => {
                let left = i16::from(bytes[offset - 4]);
                let top = i16::from(bytes[offset - self.stride]);
                let top_left = i16::from(bytes[offset - self.stride - 4]);
                (left + top - top_left).clamp(0, 255) as u8
            }
            (true, false) => bytes[offset - 4],
            (false, true) => bytes[offset - self.stride],
            (false, false) => 0,
        }
    }
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
    if params.profile == ByteBendProfile::Jpeg {
        jpeg::bend(image, &mut output, params, cancel);
        return (!is_cancelled(cancel)).then_some(output);
    }
    let prediction = RowPrediction::new(image, params.profile);
    let band_stride = prediction.stride * prediction.band_rows;
    let len = image.as_raw().len() / 4 * 3;
    let edits = Edits::new(params, len);
    output
        .par_chunks_mut(band_stride)
        .enumerate()
        .for_each(|(band, result)| {
            let bytes_per_band = band_stride / 4 * 3;
            let start = band * bytes_per_band;
            let end = start + result.len() / 4 * 3;
            if edits.overlaps(start..end) {
                bend_band(
                    result,
                    band * band_stride,
                    &prediction,
                    image,
                    &edits,
                    cancel,
                );
            }
        });
    (!is_cancelled(cancel)).then_some(output)
}

// Decode against already bent neighbors. The prediction resets at each band
// boundary and, for the WebP profile, at each column of tiles.
fn bend_band(
    result: &mut [u8],
    band_start: usize,
    prediction: &RowPrediction,
    image: &RgbaImage,
    edits: &Edits,
    cancel: Option<&AtomicBool>,
) {
    let source = &image.as_raw()[band_start..band_start + result.len()];
    let byte_start = band_start / 4 * 3;
    let row_stride = prediction.stride;
    for (y, source_row) in source.chunks_exact(row_stride).enumerate() {
        if is_cancelled(cancel) {
            return;
        }
        let row = y * row_stride;
        let row_start = byte_start + row / 4 * 3;
        let row_end = row_start + row_stride / 4 * 3;
        let row_edits: Vec<_> = edits
            .0
            .iter()
            .filter(|(range, _)| range.start < row_end && row_start < range.end)
            .collect();
        for (x, pixel) in source_row.chunks_exact(4).enumerate() {
            for (channel, &original) in pixel[..3].iter().enumerate() {
                let offset = row + x * 4 + channel;
                let index = row_start + x * 3 + channel;
                let residual = original.wrapping_sub(prediction.predict(source, offset, x, y));
                let changed = row_edits
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                    .map_or(residual, |(_, edit)| {
                        edit.replace(residual, index, |back| {
                            prediction
                                .sample(image, index.saturating_sub(row_stride / 4 * 3 * back))
                        })
                    });
                result[offset] = prediction
                    .predict(result, offset, x, y)
                    .wrapping_add(changed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn flip(profile: ByteBendProfile, address: u16, variation: u8) -> ByteBendParams {
        ByteBendParams {
            profile,
            edits: vec![ByteBendEdit {
                address,
                span: 0,
                mode: ByteBendMode::FlipBits,
                variation,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn corrupted_residual_propagates_to_the_band_boundary_and_keeps_alpha() {
        let source = RgbaImage::from_pixel(1, 64, Rgba([60, 90, 120, 200]));
        let params = flip(ByteBendProfile::Png, 0, 0);
        assert_eq!(
            byte_bend_sample(&source, ByteBendProfile::Png, 2_048),
            Some((6, 0))
        );
        let result = apply_byte_bend_cancellable(&source, &params, None).unwrap();
        assert_eq!(result.get_pixel(0, 0).0, [56, 90, 120, 200]);
        assert_eq!(result.get_pixel(0, 31).0, [56, 90, 120, 200]);
        assert_eq!(result.get_pixel(0, 32), source.get_pixel(0, 32));
    }

    #[test]
    fn webp_prediction_propagates_across_and_down_one_tile() {
        let source = RgbaImage::from_pixel(19, 21, Rgba([60, 90, 120, 200]));
        let params = flip(ByteBendProfile::WebP, 0, 0);
        let result = apply_byte_bend_cancellable(&source, &params, None).unwrap();
        assert_eq!(result.get_pixel(15, 15).0, [56, 90, 120, 200]);
        assert_eq!(result.get_pixel(16, 15), source.get_pixel(16, 15));
        assert_eq!(result.get_pixel(15, 16), source.get_pixel(15, 16));
    }

    #[test]
    fn jpeg_bends_a_frequency_coefficient_inside_one_block() {
        let source = RgbaImage::from_pixel(17, 9, Rgba([128, 128, 128, 200]));
        let params = flip(ByteBendProfile::Jpeg, 7_000, 4);
        assert_eq!(
            byte_bend_sample(&source, params.profile, 7_000),
            Some((3, 0))
        );
        let result = apply_byte_bend_cancellable(&source, &params, None).unwrap();
        assert_eq!(result.get_pixel(0, 0).0, [192, 192, 192, 200]);
        assert_eq!(result.get_pixel(7, 7).0, [64, 64, 64, 200]);
        assert_eq!(result.get_pixel(8, 0), source.get_pixel(8, 0));
        assert_eq!(result.get_pixel(0, 8), source.get_pixel(0, 8));

        let mut edge = params;
        edge.edits[0].address = u16::MAX;
        let result = apply_byte_bend_cancellable(&source, &edge, None).unwrap();
        assert_eq!(result.get_pixel(16, 8).0, [192, 192, 192, 200]);
        assert_eq!(result.get_pixel(15, 8), source.get_pixel(15, 8));
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
