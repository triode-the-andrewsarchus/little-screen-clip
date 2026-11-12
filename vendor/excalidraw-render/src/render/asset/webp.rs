//! The WebP codec: RIFF chunk walking that validates the container and each
//! animation frame as a standalone image, the animation work estimate, the
//! embedded ICC profile, and the default frame decode with color
//! normalization applied.
//!
//! The pinned reference reads WebP through libwebp's demuxer and Blink's
//! `WEBPImageDecoder`. `canonical_webp` reduces a container to what they
//! interpret before `image-webp` sees it: bytes after the declared RIFF end are
//! ignored, a VP8X reserved flag bit rejects the file, reserved ANMF flag bits
//! are ignored, a frame's size is its bitstream's, and ICCP, EXIF, XMP and
//! unknown chunks and their flags are dropped. Blink never applies WebP EXIF
//! orientation. It applies an ICC profile only for a still image whose VP8X
//! ICC flag is set, and only when the profile parses as an RGB profile;
//! otherwise the profile is ignored.

use std::borrow::Cow;
use std::io::Cursor;
use std::sync::Arc;

use image_webp::{DecodingError as WebPDecodingError, WebPDecoder};

use super::color::{finish_rgba, normalize_rgba, ColorNormalization};
use super::{decoder_workspace_bytes, try_copy, DecodedRaster, ResolveError};

pub(super) const WEBP_RIFF_SIGNATURE: &[u8; 4] = b"RIFF";
pub(super) const WEBP_SIGNATURE: &[u8; 4] = b"WEBP";

const VP8X_VALID_FLAGS: u8 = 0x3e;
const VP8X_ICC_FLAG: u8 = 0x20;
const VP8X_ALPHA_FLAG: u8 = 0x10;
const VP8X_ANIMATION_FLAG: u8 = 0x02;

#[derive(Clone, Copy)]
pub(super) struct WebPSemantics {
    pub(super) animation_frames: u32,
}

/// The declared RIFF extent; libwebp's demuxer never reads past it.
fn riff_extent(bytes: &[u8]) -> Result<&[u8], ResolveError> {
    let riff_size = u32::from_le_bytes(
        bytes
            .get(4..8)
            .ok_or(ResolveError::WebPDecodeFailed)?
            .try_into()
            .map_err(|_| ResolveError::WebPDecodeFailed)?,
    );
    if riff_size < 8 {
        return Err(ResolveError::WebPDecodeFailed);
    }
    let end = usize::try_from(riff_size)
        .ok()
        .and_then(|size| size.checked_add(8))
        .ok_or(ResolveError::WebPDecodeFailed)?;
    bytes.get(..end).ok_or(ResolveError::WebPDecodeFailed)
}

/// The bitstream's own frame size: VP8L's 14-bit fields, or a VP8 key frame's.
fn bitstream_dimensions(chunks: &[u8]) -> Option<(u32, u32)> {
    let (kind, data, next) = webp_chunk_at(chunks, 0).ok()?;
    let (kind, data) = if kind == *b"ALPH" {
        let (kind, data, _) = webp_chunk_at(chunks, next).ok()?;
        (kind, data)
    } else {
        (kind, data)
    };
    match &kind {
        b"VP8L" if data.first() == Some(&0x2f) => {
            let bits = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8 " if data.get(3..6) == Some(&[0x9d, 0x01, 0x2a]) => {
            let size = |at: usize| {
                Some(u32::from(
                    u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?) & 0x3fff,
                ))
            };
            Some((size(6)?, size(8)?))
        }
        _ => None,
    }
}

/// The container as the pinned reference interprets it (see the module
/// documentation). Simple-format files only lose bytes past the RIFF end.
pub(super) fn canonical_webp(bytes: &[u8]) -> Result<Cow<'_, [u8]>, ResolveError> {
    let bytes = riff_extent(bytes)?;
    let (first, vp8x, _) = webp_chunk_at(bytes, 12)?;
    if first != *b"VP8X" {
        return Ok(Cow::Borrowed(bytes));
    }
    let flags = *vp8x.first().ok_or(ResolveError::WebPDecodeFailed)?;
    if flags & !VP8X_VALID_FLAGS != 0 {
        return Err(ResolveError::WebPDecodeFailed);
    }
    // image-webp decodes the alpha the flag declares; the declared flag itself
    // only selects Blink's premultiplication (`webp_declares_alpha`), so the
    // canonical flag states whether the bitstream carries alpha.
    let mut bitstream_alpha = false;
    let mut offset = 12_usize;
    let mut first = true;
    while offset < bytes.len() {
        let (kind, data, next) = webp_chunk_at(bytes, offset)?;
        match &kind {
            b"VP8X" if !first => return Err(ResolveError::WebPDecodeFailed),
            b"ALPH" => bitstream_alpha = true,
            b"VP8L" => bitstream_alpha |= webp_lossless_has_alpha(data)?,
            b"ANMF" => {
                bitstream_alpha |= validate_webp_frame_chunks(
                    data.get(16..).ok_or(ResolveError::WebPDecodeFailed)?,
                )?;
            }
            _ => {}
        }
        first = false;
        offset = next;
    }
    let mut body = Vec::new();
    body.try_reserve_exact(bytes.len())
        .map_err(|_| ResolveError::Memory)?;
    body.extend_from_slice(b"WEBP");
    let mut offset = 12_usize;
    while offset < bytes.len() {
        let (kind, data, next) = webp_chunk_at(bytes, offset)?;
        let mut data = Cow::Borrowed(data);
        match &kind {
            b"VP8X" => {
                let data = data.to_mut();
                data[0] = (flags & VP8X_ANIMATION_FLAG)
                    | if bitstream_alpha { VP8X_ALPHA_FLAG } else { 0 };
            }
            b"ANMF" => {
                if data.len() < 16 {
                    return Err(ResolveError::WebPDecodeFailed);
                }
                let dimensions = bitstream_dimensions(&data[16..]);
                let data = data.to_mut();
                data[15] &= 0x03;
                if let Some((width, height)) = dimensions {
                    data[6..9].copy_from_slice(&(width - 1).to_le_bytes()[..3]);
                    data[9..12].copy_from_slice(&(height - 1).to_le_bytes()[..3]);
                }
            }
            b"ANIM" | b"ALPH" | b"VP8 " | b"VP8L" => {}
            _ => {
                offset = next;
                continue;
            }
        }
        let length = u32::try_from(data.len()).map_err(|_| ResolveError::WebPDecodeFailed)?;
        body.extend_from_slice(&kind);
        body.extend_from_slice(&length.to_le_bytes());
        body.extend_from_slice(&data);
        if data.len() & 1 == 1 {
            body.push(0);
        }
        offset = next;
    }
    let riff_size = u32::try_from(body.len()).map_err(|_| ResolveError::WebPDecodeFailed)?;
    let mut canonical = Vec::new();
    canonical
        .try_reserve_exact(body.len() + 8)
        .map_err(|_| ResolveError::Memory)?;
    canonical.extend_from_slice(b"RIFF");
    canonical.extend_from_slice(&riff_size.to_le_bytes());
    canonical.extend_from_slice(&body);
    Ok(Cow::Owned(canonical))
}

/// Whether Blink premultiplies the decoded samples: the VP8X alpha flag of an
/// extended file, and always for a simple file, whose demuxed flags carry the
/// bitstream's alpha. With the flag clear Blink decodes straight samples into
/// a buffer it treats as premultiplied.
pub(super) fn webp_declares_alpha(bytes: &[u8]) -> Result<bool, ResolveError> {
    let bytes = riff_extent(bytes)?;
    let (first, vp8x, _) = webp_chunk_at(bytes, 12)?;
    Ok(first != *b"VP8X"
        || vp8x
            .first()
            .is_some_and(|flags| flags & VP8X_ALPHA_FLAG != 0))
}

/// The ICC profile Blink applies: the first ICCP chunk of a still extended
/// image whose VP8X ICC flag is set.
pub(super) fn webp_icc_profile(bytes: &[u8]) -> Result<Option<&[u8]>, ResolveError> {
    let bytes = riff_extent(bytes)?;
    let (first, vp8x, _) = webp_chunk_at(bytes, 12)?;
    let flags = if first == *b"VP8X" {
        vp8x.first().copied()
    } else {
        None
    };
    if flags.is_none_or(|flags| flags & VP8X_ICC_FLAG == 0 || flags & VP8X_ANIMATION_FLAG != 0) {
        return Ok(None);
    }
    let mut offset = 12_usize;
    while offset < bytes.len() {
        let (kind, data, next) = webp_chunk_at(bytes, offset)?;
        if kind == *b"ICCP" {
            return Ok(Some(data));
        }
        offset = next;
    }
    Ok(None)
}

pub(super) fn webp_decoder(bytes: &[u8]) -> Result<WebPDecoder<Cursor<&[u8]>>, ResolveError> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(map_webp_error)?;
    let (width, height) = decoder.dimensions();
    decoder.set_memory_limit(
        usize::try_from(decoder_workspace_bytes(width, height)?)
            .map_err(|_| ResolveError::Memory)?,
    );
    Ok(decoder)
}

pub(super) fn read_webp_dimensions(bytes: &[u8]) -> Result<(u32, u32), ResolveError> {
    if !bytes.starts_with(WEBP_RIFF_SIGNATURE) || bytes.get(8..12) != Some(WEBP_SIGNATURE) {
        return Err(ResolveError::Unsupported);
    }
    let bytes = canonical_webp(bytes)?;
    let decoder = webp_decoder(&bytes)?;
    let dimensions = decoder.dimensions();
    if dimensions.0 == 0 || dimensions.1 == 0 {
        return Err(ResolveError::WebPDecodeFailed);
    }
    Ok(dimensions)
}

pub(super) fn webp_animation_validation_work(bytes: &[u8]) -> Result<u64, ResolveError> {
    let bytes = canonical_webp(bytes)?;
    let decoder = webp_decoder(&bytes)?;
    let (frames, additional_frame_pixels) = scan_webp_animation(&bytes, decoder.dimensions())?;
    if frames != decoder.num_frames() {
        return Err(ResolveError::WebPDecodeFailed);
    }
    additional_frame_pixels
        .checked_mul(4)
        .ok_or(ResolveError::AssetWork {
            attempted: u64::MAX,
        })
}

fn scan_webp_animation(bytes: &[u8], canvas: (u32, u32)) -> Result<(u32, u64), ResolveError> {
    let declared_size = usize::try_from(u32::from_le_bytes(
        bytes
            .get(4..8)
            .ok_or(ResolveError::WebPDecodeFailed)?
            .try_into()
            .map_err(|_| ResolveError::WebPDecodeFailed)?,
    ))
    .map_err(|_| ResolveError::WebPDecodeFailed)?;
    if declared_size.checked_add(8) != Some(bytes.len()) {
        return Err(ResolveError::WebPDecodeFailed);
    }
    let mut offset = 12_usize;
    let mut frames = 0_u32;
    let mut additional_frame_pixels = 0_u64;
    while offset < bytes.len() {
        let (kind, data, next) = webp_chunk_at(bytes, offset)?;
        if kind == *b"ANMF" {
            let frame_header = data.get(..16).ok_or(ResolveError::WebPDecodeFailed)?;
            let x = read_webp_u24(&frame_header[..3])?.saturating_mul(2);
            let y = read_webp_u24(&frame_header[3..6])?.saturating_mul(2);
            let width = u64::from(read_webp_u24(&frame_header[6..9])?).saturating_add(1);
            let height = u64::from(read_webp_u24(&frame_header[9..12])?).saturating_add(1);
            if u64::from(x).saturating_add(width) > u64::from(canvas.0)
                || u64::from(y).saturating_add(height) > u64::from(canvas.1)
            {
                return Err(ResolveError::WebPDecodeFailed);
            }
            validate_webp_frame_chunks(data.get(16..).ok_or(ResolveError::WebPDecodeFailed)?)?;
            frames = frames
                .checked_add(1)
                .ok_or(ResolveError::WebPDecodeFailed)?;
            if frames > 1 {
                additional_frame_pixels = additional_frame_pixels
                    .checked_add(width.checked_mul(height).ok_or(ResolveError::AssetWork {
                        attempted: u64::MAX,
                    })?)
                    .ok_or(ResolveError::AssetWork {
                        attempted: u64::MAX,
                    })?;
            }
        }
        offset = next;
    }
    if offset != bytes.len() {
        return Err(ResolveError::WebPDecodeFailed);
    }
    Ok((frames, additional_frame_pixels))
}

fn webp_chunk_at(bytes: &[u8], offset: usize) -> Result<([u8; 4], &[u8], usize), ResolveError> {
    let header = bytes
        .get(offset..offset.saturating_add(8))
        .ok_or(ResolveError::WebPDecodeFailed)?;
    let kind = header[..4]
        .try_into()
        .map_err(|_| ResolveError::WebPDecodeFailed)?;
    let length = usize::try_from(u32::from_le_bytes(
        header[4..8]
            .try_into()
            .map_err(|_| ResolveError::WebPDecodeFailed)?,
    ))
    .map_err(|_| ResolveError::WebPDecodeFailed)?;
    let data_start = offset
        .checked_add(8)
        .ok_or(ResolveError::WebPDecodeFailed)?;
    let data_end = data_start
        .checked_add(length)
        .ok_or(ResolveError::WebPDecodeFailed)?;
    let data = bytes
        .get(data_start..data_end)
        .ok_or(ResolveError::WebPDecodeFailed)?;
    let next = data_end
        .checked_add(length & 1)
        .ok_or(ResolveError::WebPDecodeFailed)?;
    bytes
        .get(data_end..next)
        .ok_or(ResolveError::WebPDecodeFailed)?;
    Ok((kind, data, next))
}

fn validate_webp_frame_chunks(chunks: &[u8]) -> Result<bool, ResolveError> {
    let (first, first_data, next) = webp_chunk_at(chunks, 0)?;
    match first {
        kind if kind == *b"VP8 " => {
            if next != chunks.len() {
                return Err(ResolveError::WebPDecodeFailed);
            }
            Ok(false)
        }
        kind if kind == *b"VP8L" => {
            if next != chunks.len() {
                return Err(ResolveError::WebPDecodeFailed);
            }
            webp_lossless_has_alpha(first_data)
        }
        kind if kind == *b"ALPH" => {
            let (second, _, end) = webp_chunk_at(chunks, next)?;
            if second != *b"VP8 " || end != chunks.len() {
                return Err(ResolveError::WebPDecodeFailed);
            }
            Ok(true)
        }
        _ => Err(ResolveError::WebPDecodeFailed),
    }
}

fn webp_lossless_has_alpha(data: &[u8]) -> Result<bool, ResolveError> {
    if data.first() != Some(&0x2f) {
        return Err(ResolveError::WebPDecodeFailed);
    }
    let header = u32::from_le_bytes(
        data.get(1..5)
            .ok_or(ResolveError::WebPDecodeFailed)?
            .try_into()
            .map_err(|_| ResolveError::WebPDecodeFailed)?,
    );
    Ok(header & (1 << 28) != 0)
}

fn read_webp_u24(bytes: &[u8]) -> Result<u32, ResolveError> {
    let bytes: [u8; 3] = bytes
        .try_into()
        .map_err(|_| ResolveError::WebPDecodeFailed)?;
    Ok(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
}

pub(super) fn validate_webp(bytes: &[u8]) -> Result<(u32, u32, WebPSemantics), ResolveError> {
    let bytes = canonical_webp(bytes)?;
    let bytes = bytes.as_ref();
    let mut decoder = webp_decoder(bytes)?;
    let (width, height) = decoder.dimensions();
    let animation_frames = decoder.num_frames().max(1);
    if decoder.is_animated() {
        scan_webp_animation(bytes, (width, height))?;
        validate_webp_animation_frames(bytes)?;
    } else {
        let output_size = decoder.output_buffer_size().ok_or(ResolveError::Memory)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(output_size)
            .map_err(|_| ResolveError::Memory)?;
        output.resize(output_size, 0);
        decoder.read_image(&mut output).map_err(map_webp_error)?;
    }
    Ok((width, height, WebPSemantics { animation_frames }))
}

fn validate_webp_animation_frames(bytes: &[u8]) -> Result<(), ResolveError> {
    let mut offset = 12_usize;
    while offset < bytes.len() {
        let (kind, data, next) = webp_chunk_at(bytes, offset)?;
        if kind == *b"ANMF" {
            validate_webp_animation_frame(data)?;
        }
        offset = next;
    }
    Ok(())
}

fn validate_webp_animation_frame(frame: &[u8]) -> Result<(), ResolveError> {
    let (standalone, width, height) = standalone_webp_frame(frame)?;
    let mut decoder = webp_decoder(&standalone)?;
    if decoder.dimensions() != (width, height) || decoder.is_animated() {
        return Err(ResolveError::WebPDecodeFailed);
    }
    let output_size = decoder.output_buffer_size().ok_or(ResolveError::Memory)?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_size)
        .map_err(|_| ResolveError::Memory)?;
    output.resize(output_size, 0);
    decoder.read_image(&mut output).map_err(map_webp_error)
}

fn standalone_webp_frame(frame: &[u8]) -> Result<(Vec<u8>, u32, u32), ResolveError> {
    let frame_header = frame.get(..16).ok_or(ResolveError::WebPDecodeFailed)?;
    let width = read_webp_u24(&frame_header[6..9])?.saturating_add(1);
    let height = read_webp_u24(&frame_header[9..12])?.saturating_add(1);
    let chunks = frame.get(16..).ok_or(ResolveError::WebPDecodeFailed)?;
    let has_alpha = validate_webp_frame_chunks(chunks)?;
    let needs_vp8x = has_alpha && chunks.starts_with(b"ALPH");
    let body_size = chunks
        .len()
        .checked_add(if needs_vp8x { 18 } else { 0 })
        .ok_or(ResolveError::Memory)?;
    let riff_size = body_size
        .checked_add(4)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or(ResolveError::Memory)?;
    let standalone_size = body_size.checked_add(12).ok_or(ResolveError::Memory)?;
    let mut standalone = Vec::new();
    standalone
        .try_reserve_exact(standalone_size)
        .map_err(|_| ResolveError::Memory)?;
    standalone.extend_from_slice(b"RIFF");
    standalone.extend_from_slice(&riff_size.to_le_bytes());
    standalone.extend_from_slice(b"WEBP");
    if needs_vp8x {
        standalone.extend_from_slice(b"VP8X");
        standalone.extend_from_slice(&10_u32.to_le_bytes());
        standalone.extend_from_slice(&[0x10, 0, 0, 0]);
        // ANMF and VP8X both store width/height minus one as little-endian u24.
        standalone.extend_from_slice(&frame_header[6..12]);
    }
    standalone.extend_from_slice(chunks);
    Ok((standalone, width, height))
}

pub(super) fn decode_webp_default_frame(
    original: &[u8],
    source_width: u32,
    source_height: u32,
    pixels: u64,
    color_normalization: ColorNormalization,
) -> Result<DecodedRaster, ResolveError> {
    let declares_alpha = webp_declares_alpha(original)?;
    let bytes = canonical_webp(original)?;
    let bytes = bytes.as_ref();
    // With the alpha flag clear, the reference drops a still lossy image's
    // ALPH chunk: the image is opaque with its raw VP8 colors. Lossless
    // in-bitstream alpha and an animation frame's alpha stay (see the
    // reinterpretation below).
    let mut lossy_alpha = false;
    let decoder = webp_decoder(bytes)?;
    // Frame zero is drawn onto transparent black. Do not delegate this to the
    // animation compositor: opaque source channels do not imply an opaque
    // canvas, and blending against an empty destination must preserve samples.
    let mut first_frame = None;
    let mut placement = (0_u32, 0_u32, source_width, source_height);
    if decoder.is_animated() {
        let mut offset = 12;
        while offset < bytes.len() {
            let (kind, data, next) = webp_chunk_at(bytes, offset)?;
            if kind == *b"ANMF" {
                let header = data.get(..16).ok_or(ResolveError::WebPDecodeFailed)?;
                let (standalone, width, height) = standalone_webp_frame(data)?;
                let x = read_webp_u24(&header[..3])?
                    .checked_mul(2)
                    .ok_or(ResolveError::WebPDecodeFailed)?;
                let y = read_webp_u24(&header[3..6])?
                    .checked_mul(2)
                    .ok_or(ResolveError::WebPDecodeFailed)?;
                if x.checked_add(width).is_none_or(|v| v > source_width)
                    || y.checked_add(height).is_none_or(|v| v > source_height)
                {
                    return Err(ResolveError::WebPDecodeFailed);
                }
                placement = (x, y, width, height);
                first_frame = Some(standalone);
                break;
            }
            offset = next;
        }
        if first_frame.is_none() {
            return Err(ResolveError::WebPDecodeFailed);
        }
    } else {
        let mut offset = 12;
        while offset < bytes.len() {
            let (kind, _, next) = webp_chunk_at(bytes, offset)?;
            lossy_alpha |= kind == *b"ALPH";
            offset = next;
        }
    }
    let drop_alpha = !declares_alpha && lossy_alpha;
    let mut decoder = if let Some(frame) = &first_frame {
        drop(decoder);
        let decoder = webp_decoder(frame)?;
        if decoder.dimensions() != (placement.2, placement.3) || decoder.is_animated() {
            return Err(ResolveError::WebPDecodeFailed);
        }
        decoder
    } else {
        decoder
    };
    let output_size = decoder.output_buffer_size().ok_or(ResolveError::Memory)?;
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(output_size)
        .map_err(|_| ResolveError::Memory)?;
    decoded.resize(output_size, 0);
    decoder.read_image(&mut decoded).map_err(map_webp_error)?;
    let channels = if decoder.has_alpha() { 4_u8 } else { 3_u8 };
    drop(decoder);
    drop(first_frame);

    let (x, y, width, height) = placement;
    let frame_pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ResolveError::Memory)?;
    let expected = usize::try_from(
        frame_pixels
            .checked_mul(u64::from(channels))
            .ok_or(ResolveError::Memory)?,
    )
    .map_err(|_| ResolveError::Memory)?;
    if decoded.len() != expected {
        return Err(ResolveError::WebPDecodeFailed);
    }
    let x = usize::try_from(x).map_err(|_| ResolveError::Memory)?;
    let y = usize::try_from(y).map_err(|_| ResolveError::Memory)?;
    let width = usize::try_from(width).map_err(|_| ResolveError::Memory)?;
    let canvas_width = usize::try_from(source_width).map_err(|_| ResolveError::Memory)?;

    let rgba_size = usize::try_from(pixels.checked_mul(4).ok_or(ResolveError::Memory)?)
        .map_err(|_| ResolveError::Memory)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(rgba_size)
        .map_err(|_| ResolveError::Memory)?;
    rgba.resize(rgba_size, 0);
    for (i, pixel) in decoded.chunks_exact(usize::from(channels)).enumerate() {
        let row = y.checked_add(i / width).ok_or(ResolveError::Memory)?;
        let col = x.checked_add(i % width).ok_or(ResolveError::Memory)?;
        let start = row
            .checked_mul(canvas_width)
            .and_then(|n| n.checked_add(col))
            .and_then(|n| n.checked_mul(4))
            .ok_or(ResolveError::Memory)?;
        let end = start.checked_add(4).ok_or(ResolveError::Memory)?;
        let target = rgba
            .get_mut(start..end)
            .ok_or(ResolveError::WebPDecodeFailed)?;
        target[..3].copy_from_slice(&pixel[..3]);
        target[3] = if channels == 4 && !drop_alpha {
            pixel[3]
        } else {
            255
        };
        if target[3] == 0 {
            target.fill(0);
        }
    }
    drop(decoded);
    // With the alpha flag clear, Blink's buffer holds straight samples it
    // treats as premultiplied: the drawn color is their unpremultiplication,
    // clamped, and an untransformed draw uses the samples themselves.
    let reinterpreted = if !declares_alpha && channels == 4 && !drop_alpha {
        let samples = try_copy(&rgba)?;
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u32::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .map_or(0, |value| value.min(255) as u8);
            }
        }
        Some(samples)
    } else {
        None
    };
    let untransformed = matches!(color_normalization, ColorNormalization::None);
    normalize_rgba(&mut rgba, color_normalization)?;
    if !untransformed {
        return finish_rgba(rgba);
    }
    if let Some(samples) = reinterpreted {
        return Ok(DecodedRaster {
            rgba: Arc::new(rgba),
            premultiplied_rgba: Arc::new(samples),
        });
    }
    // Chromium's untransformed WebP path requests libwebp's premultiplied
    // output, which truncates c*a/255. Keep the straight samples for PDF;
    // color-transformed images continue through the ordinary rounded path.
    let mut premultiplied_rgba = try_copy(&rgba)?;
    for pixel in premultiplied_rgba.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u16::from(*channel) * alpha / 255) as u8;
        }
    }
    Ok(DecodedRaster {
        rgba: Arc::new(rgba),
        premultiplied_rgba: Arc::new(premultiplied_rgba),
    })
}

pub(super) fn map_webp_error(error: WebPDecodingError) -> ResolveError {
    if matches!(error, WebPDecodingError::MemoryLimitExceeded) {
        ResolveError::Memory
    } else {
        ResolveError::WebPDecodeFailed
    }
}
