//! The GIF codec, read as the pinned reference reads it: Chromium 149 decodes
//! GIF through Skia's `SkWuffsCodec` (Wuffs 0.3.4, quirk
//! `IGNORE_TOO_MUCH_PIXEL_DATA` only). A block walk counts frames and prices
//! the additional frames' pixels; the default frame is decoded by this
//! module's own LZW decoder and composited onto a transparent canvas.
//!
//! As the reference does:
//! - any extension is skipped whatever its label or block size, also in
//!   GIF87a; application data (ICC, XMP) is never buffered or applied;
//! - a byte other than an extension or image introducer ends the stream, so a
//!   missing or different trailer and trailing bytes are ignored;
//! - a malformed graphic control extension before the first image rejects the
//!   file, and anything malformed after it ends the frame count;
//! - reserved bits, disposal values and the background index are ignored;
//! - the canvas is the logical screen grown to the first frame's extent; a
//!   zero-size first frame, or no image at all, draws a transparent screen;
//! - a pixel index outside the palette, or any index without a palette, is
//!   opaque black unless it is the transparent index;
//! - once the first frame's pixels are complete, the rest of its data
//!   (excess pixels, a later invalid code, a missing end code) is ignored.
//!
//! One declared divergence remains (#85): where the first frame's data ends
//! or fails before its pixels are complete, the reference paints the partial
//! frame; the renderer rejects the file with exact decode-failed recovery.
//! Later frames only count; their data is not decoded.

use std::sync::Arc;

use super::{try_copy, DecodedRaster, ResolveError};

pub(super) const GIF87A_SIGNATURE: &[u8; 6] = b"GIF87a";
pub(super) const GIF89A_SIGNATURE: &[u8; 6] = b"GIF89a";

#[derive(Clone, Copy)]
pub(super) struct GifSemantics {
    pub(super) animation_frames: u32,
}

/// The first frame: placement, palette, transparency and its LZW data span.
struct FirstFrame {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    interlaced: bool,
    palette: Option<(usize, usize)>,
    transparent: Option<u8>,
    code_size: u8,
    data: (usize, usize),
}

struct GifStructure {
    width: u32,
    height: u32,
    frames: u32,
    additional_frame_pixels: u64,
    first: Option<FirstFrame>,
}

fn failed() -> ResolveError {
    ResolveError::GifDecodeFailed
}

fn table_entries(packed: u8) -> usize {
    2 << (packed & 0x07)
}

/// Skips data sub-blocks from `offset`, returning the end, or `None` when the
/// input ends first.
fn sub_blocks_end(bytes: &[u8], mut offset: usize) -> Option<usize> {
    loop {
        let length = usize::from(*bytes.get(offset)?);
        offset = offset.checked_add(1 + length)?;
        if offset > bytes.len() {
            return None;
        }
        if length == 0 {
            return Some(offset);
        }
    }
}

fn read_structure(bytes: &[u8]) -> Result<GifStructure, ResolveError> {
    if !bytes.starts_with(GIF87A_SIGNATURE) && !bytes.starts_with(GIF89A_SIGNATURE) {
        return Err(ResolveError::Unsupported);
    }
    let screen = bytes.get(6..13).ok_or_else(failed)?;
    let screen_width = u32::from(u16::from_le_bytes([screen[0], screen[1]]));
    let screen_height = u32::from(u16::from_le_bytes([screen[2], screen[3]]));
    let mut offset = 13_usize;
    let mut global = None;
    if screen[4] & 0x80 != 0 {
        let length = table_entries(screen[4]) * 3;
        bytes.get(offset..offset + length).ok_or_else(failed)?;
        global = Some((offset, length));
        offset += length;
    }
    let mut frames = 0_u32;
    let mut additional_frame_pixels = 0_u64;
    let mut first = None;
    let mut transparent = None;
    // A fault after the first image only ends the count; before it, it
    // rejects the file.
    let stop = |frames: u32| if frames == 0 { Err(failed()) } else { Ok(()) };
    while let Some(&block) = bytes.get(offset) {
        match block {
            0x21 => {
                let Some(&label) = bytes.get(offset + 1) else {
                    stop(frames)?;
                    break;
                };
                let mut next = offset + 2;
                if label == 0xf9 {
                    match bytes.get(next..next + 5) {
                        Some(control) if control[0] == 4 => {
                            transparent = (control[1] & 0x01 != 0).then_some(control[4]);
                            next += 5;
                        }
                        _ => {
                            stop(frames)?;
                            break;
                        }
                    }
                }
                match sub_blocks_end(bytes, next) {
                    Some(end) => offset = end,
                    None => {
                        stop(frames)?;
                        break;
                    }
                }
            }
            0x2c => {
                let Some(descriptor) = bytes.get(offset + 1..offset + 10) else {
                    stop(frames)?;
                    break;
                };
                let left = u32::from(u16::from_le_bytes([descriptor[0], descriptor[1]]));
                let top = u32::from(u16::from_le_bytes([descriptor[2], descriptor[3]]));
                let width = u32::from(u16::from_le_bytes([descriptor[4], descriptor[5]]));
                let height = u32::from(u16::from_le_bytes([descriptor[6], descriptor[7]]));
                let packed = descriptor[8];
                let mut next = offset + 10;
                let mut palette = global;
                if packed & 0x80 != 0 {
                    let length = table_entries(packed) * 3;
                    if bytes.get(next..next + length).is_none() {
                        stop(frames)?;
                        break;
                    }
                    palette = Some((next, length));
                    next += length;
                }
                let Some(&code_size) = bytes.get(next) else {
                    stop(frames)?;
                    break;
                };
                let data_start = next + 1;
                let data_end = sub_blocks_end(bytes, data_start);
                if frames == 0 {
                    first = Some(FirstFrame {
                        left,
                        top,
                        width,
                        height,
                        interlaced: packed & 0x40 != 0,
                        palette,
                        transparent,
                        code_size,
                        data: (data_start, data_end.unwrap_or(bytes.len())),
                    });
                } else {
                    additional_frame_pixels = additional_frame_pixels
                        .checked_add(u64::from(width) * u64::from(height))
                        .ok_or(ResolveError::AssetWork {
                            attempted: u64::MAX,
                        })?;
                }
                frames = frames.checked_add(1).ok_or_else(failed)?;
                transparent = None;
                match data_end {
                    Some(end) => offset = end,
                    None => break,
                }
            }
            _ => break,
        }
    }
    let (width, height) = match &first {
        Some(frame) if frame.width > 0 && frame.height > 0 => (
            screen_width.max(frame.left + frame.width),
            screen_height.max(frame.top + frame.height),
        ),
        _ => (screen_width, screen_height),
    };
    if width == 0 || height == 0 {
        return Err(failed());
    }
    Ok(GifStructure {
        width,
        height,
        frames,
        additional_frame_pixels,
        first,
    })
}

/// The first frame's color indices in raster order, or a decode failure when
/// its data ends or fails before every pixel is known.
fn decode_indices(bytes: &[u8], frame: &FirstFrame) -> Result<Vec<u8>, ResolveError> {
    let pixels = usize::try_from(u64::from(frame.width) * u64::from(frame.height))
        .map_err(|_| ResolveError::Memory)?;
    let mut sequence = Vec::new();
    sequence
        .try_reserve_exact(pixels)
        .map_err(|_| ResolveError::Memory)?;
    if pixels == 0 {
        return Ok(sequence);
    }
    if !(2..=8).contains(&frame.code_size) {
        return Err(failed());
    }
    // Concatenated sub-block payload, read bit by bit.
    let mut data = Vec::new();
    let mut offset = frame.data.0;
    while offset < frame.data.1 {
        let length = usize::from(bytes[offset]);
        if length == 0 {
            break;
        }
        let end = (offset + 1 + length).min(frame.data.1);
        data.try_reserve(end - offset - 1)
            .map_err(|_| ResolveError::Memory)?;
        data.extend_from_slice(&bytes[offset + 1..end]);
        offset = end;
    }
    let clear = 1_usize << frame.code_size;
    let end_code = clear + 1;
    let mut prefix = [0_u16; 4096];
    let mut suffix = [0_u8; 4096];
    let mut first_byte = [0_u8; 4096];
    let mut lengths = [0_u16; 4096];
    for code in 0..clear {
        suffix[code] = code as u8;
        first_byte[code] = code as u8;
        lengths[code] = 1;
    }
    let mut next = end_code + 1;
    let mut width = u32::from(frame.code_size) + 1;
    let mut previous: Option<usize> = None;
    let mut bit = 0_usize;
    let mut scratch = [0_u8; 4096];
    while sequence.len() < pixels {
        if bit + width as usize > data.len() * 8 {
            return Err(failed());
        }
        let mut code = 0_usize;
        for i in 0..width as usize {
            let position = bit + i;
            code |= usize::from((data[position / 8] >> (position % 8)) & 1) << i;
        }
        bit += width as usize;
        if code == clear {
            next = end_code + 1;
            width = u32::from(frame.code_size) + 1;
            previous = None;
            continue;
        }
        if code == end_code {
            return Err(failed());
        }
        let Some(prev) = previous else {
            if code >= clear {
                return Err(failed());
            }
            sequence.push(code as u8);
            previous = Some(code);
            continue;
        };
        let (entry, first) = if code < next {
            (code, first_byte[code])
        } else if code == next {
            (prev, first_byte[prev])
        } else {
            return Err(failed());
        };
        // Expand `entry`, then the KwKwK byte when `code == next`.
        let length = usize::from(lengths[entry]);
        let mut current = entry;
        for position in (0..length).rev() {
            scratch[position] = suffix[current];
            current = usize::from(prefix[current]);
        }
        let mut emitted = &scratch[..length];
        let kwkwk = [first];
        let remaining = pixels - sequence.len();
        emitted = &emitted[..emitted.len().min(remaining)];
        sequence.extend_from_slice(emitted);
        if code == next && sequence.len() < pixels {
            sequence.extend_from_slice(&kwkwk);
        }
        if next < 4096 {
            prefix[next] = prev as u16;
            suffix[next] = first;
            first_byte[next] = first_byte[prev];
            lengths[next] = lengths[prev] + 1;
            next += 1;
            if next == 1 << width && width < 12 {
                width += 1;
            }
        }
        previous = Some(code);
    }
    Ok(sequence)
}

pub(super) fn read_gif_dimensions(bytes: &[u8]) -> Result<(u32, u32), ResolveError> {
    let structure = read_structure(bytes)?;
    Ok((structure.width, structure.height))
}

pub(super) fn gif_animation_validation_work(
    bytes: &[u8],
    _pixels: u64,
) -> Result<u64, ResolveError> {
    read_structure(bytes)?
        .additional_frame_pixels
        .checked_mul(4)
        .ok_or(ResolveError::AssetWork {
            attempted: u64::MAX,
        })
}

pub(super) fn validate_gif(bytes: &[u8]) -> Result<(u32, u32, GifSemantics), ResolveError> {
    let structure = read_structure(bytes)?;
    if let Some(first) = &structure.first {
        decode_indices(bytes, first)?;
    }
    Ok((
        structure.width,
        structure.height,
        GifSemantics {
            animation_frames: structure.frames.max(1),
        },
    ))
}

pub(super) fn decode_gif_default_frame(
    bytes: &[u8],
    width: u32,
    height: u32,
    pixels: u64,
) -> Result<DecodedRaster, ResolveError> {
    let structure = read_structure(bytes)?;
    if (structure.width, structure.height) != (width, height) {
        return Err(failed());
    }
    let rgba_size = usize::try_from(pixels.checked_mul(4).ok_or(ResolveError::Memory)?)
        .map_err(|_| ResolveError::Memory)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(rgba_size)
        .map_err(|_| ResolveError::Memory)?;
    rgba.resize(rgba_size, 0);
    if let Some(frame) = &structure.first {
        let indices = decode_indices(bytes, frame)?;
        let palette = frame
            .palette
            .map(|(offset, length)| &bytes[offset..offset + length]);
        let frame_width = frame.width as usize;
        let canvas_width = width as usize;
        let rows: Vec<usize> = if frame.interlaced {
            let h = frame.height as usize;
            (0..h)
                .step_by(8)
                .chain((4..h).step_by(8))
                .chain((2..h).step_by(4))
                .chain((1..h).step_by(2))
                .collect()
        } else {
            (0..frame.height as usize).collect()
        };
        for (row_index, row) in rows.into_iter().enumerate() {
            for x in 0..frame_width {
                let index = indices[row_index * frame_width + x];
                let destination =
                    ((frame.top as usize + row) * canvas_width + frame.left as usize + x) * 4;
                let pixel = rgba
                    .get_mut(destination..destination + 4)
                    .ok_or_else(failed)?;
                if frame.transparent == Some(index) {
                    pixel.fill(0);
                    continue;
                }
                let color = palette
                    .and_then(|palette| {
                        palette.get(usize::from(index) * 3..usize::from(index) * 3 + 3)
                    })
                    .unwrap_or(&[0, 0, 0]);
                pixel[..3].copy_from_slice(color);
                pixel[3] = 255;
            }
        }
    }
    let mut premultiplied_rgba = try_copy(&rgba)?;
    for pixel in premultiplied_rgba.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    Ok(DecodedRaster {
        rgba: Arc::new(rgba),
        premultiplied_rgba: Arc::new(premultiplied_rgba),
    })
}
