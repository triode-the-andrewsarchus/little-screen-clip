//! Standalone BMP, read as the pinned reference's Blink `BMPImageReader`
//! (Chromium 149) reads it.
//!
//! Header rules: the 14-byte file header needs only the `BM` signature and the
//! pixel-data offset (its file-size and reserved fields are ignored; zero means
//! "directly after the header, masks and palette"). The info header is OS/2 1.x
//! (12 bytes), Windows V3+ (40, 52, 56, 108, 124) or OS/2 2.x (16..=64 in
//! multiples of four, or 42 or 46) and must lie wholly in the file and before
//! a non-zero pixel offset. `IsInfoHeaderValid` then applies: positive width,
//! non-zero height, top-down only for Windows V3+, depths 1/4/8/24 everywhere
//! plus 2/16/32 for Windows V3+, RLE8/RLE4 with a depth no larger than their
//! own (the depth is then corrected), `[ALPHA]BITFIELDS` only for Windows V3+
//! 16/32 bpp, RLE24 (compression 4 at 24 bpp) only for OS/2 2.x, no JPEG, PNG
//! or Huffman payloads, and both dimensions below 65536.
//!
//! Palettes: `biClrUsed` of zero or above the depth's maximum means the
//! maximum; a palette overlapping the pixel offset is truncated to the whole
//! entries before it, and an index beyond it draws opaque black.
//!
//! Masks: non-bitfield 16/24/32 bpp use the fixed 5-5-5 or 8-8-8 layouts, while
//! the header's (V2+) or trailing (V3 `BITFIELDS`/`ALPHABITFIELDS`) masks are
//! trimmed to the pixel depth, must neither overlap nor have gaps, may be
//! empty, and keep the top eight bits of a wider channel. Narrower channels
//! expand as `round(v * 255 / max)`. Alpha comes only from a V3+ header mask
//! or `ALPHABITFIELDS`; BI_RGB 32 bpp under a 40-byte header is opaque. Until a
//! non-zero alpha is seen, zero alpha draws opaque; the first non-zero alpha
//! after a zero one clears everything decoded so far to transparent.
//!
//! RLE8/RLE4/RLE24 start transparent: delta and end-of-line holes and anything
//! after an early end-of-bitmap stay transparent, runs past the row end are
//! clipped, out-of-range indices draw black and top-down streams are honored.
//! The reference paints whatever it decoded when the stream runs out or turns
//! invalid (an overlong literal or delta, or data after the last row), so the
//! walk simply stops there.
//!
//! Colour: V4/V5 headers carry a colour space. Calibrated RGB (type 0) takes
//! its three endpoints' X and Y as chromaticities with a D65 white
//! (`skcms_PrimariesToXYZD50`; unusable endpoints ignore the space) and the
//! 16.16 gammas as decoding exponents; `sRGB`, `Win ` and linked or unknown
//! types draw as sRGB. `MBED` (V5 only) names an ICC profile at an offset from
//! the info header; a profile outside the file fails the image, and the
//! caller parses it, failing the image when it does not parse.
//!
//! Declared divergence: where non-RLE pixel data ends before the last row's
//! padded end the reference paints the complete rows and leaves the rest
//! transparent; the project instead reports the image as decode-failed.
//!
//! ICO entries: the same reader runs on an icon's DIB as Blink's
//! `BMPImageReader(parent, image_offset, 0, /*is_in_ico=*/true)` does. There is
//! no file header: the info header starts at the entry's offset and the masks
//! or palette, then the pixels, follow it directly (the whole palette must be
//! present). The header's height is halved (truncating) before its sign selects
//! top-down rows, OS/2 headers are invalid, and `BI_RGB` 32 bpp reads its alpha
//! byte. After the colour rows comes the 1 bpp AND mask, applied unless the
//! depth is 16 or 32 with an alpha mask and a non-zero alpha was seen: each set
//! bit clears its pixel to transparent black. An RLE stream's mask starts at its
//! end-of-bitmap marker (Blink does not step past it); an RLE stream that stops
//! elsewhere paints what it decoded, without the mask.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Compression {
    Rgb,
    Rle8,
    Rle4,
    Bitfields,
    AlphaBitfields,
    Rle24,
}

impl Compression {
    const fn rle(self) -> bool {
        matches!(self, Self::Rle8 | Self::Rle4 | Self::Rle24)
    }
}

/// The colour space a V4/V5 header declares, when it is not sRGB.
#[derive(Clone)]
pub(super) enum ColorSpace {
    Srgb,
    /// Columns are the red, green and blue colorants in D50 XYZ; the gammas
    /// are raw 16.16 fixed-point decoding exponents.
    Calibrated {
        to_xyz_d50: [[f32; 3]; 3],
        gammas: [u32; 3],
    },
    Embedded {
        offset: usize,
        length: usize,
    },
}

#[derive(Clone)]
pub(super) struct Info {
    pub(super) width: u32,
    pub(super) height: u32,
    top_down: bool,
    bit_count: u16,
    compression: Compression,
    pixel_offset: usize,
    palette_offset: usize,
    palette_entries: usize,
    palette_stride: usize,
    masks: [u32; 4],
    shifts: [u32; 4],
    /// Bits of a channel narrower than eight bits, or zero.
    narrow_bits: [u32; 4],
    pub(super) color_space: ColorSpace,
    in_ico: bool,
}

impl Info {
    /// The embedded ICC profile's bytes, for `MBED` colour spaces.
    pub(super) fn embedded_profile<'a>(&self, bytes: &'a [u8]) -> Option<&'a [u8]> {
        match self.color_space {
            ColorSpace::Embedded { offset, length } => bytes.get(offset..offset + length),
            _ => None,
        }
    }

    fn padded_row_bytes(&self, pixels: usize, rle: bool) -> Result<(usize, usize), ()> {
        let bits = usize::from(self.bit_count);
        let unpadded = if bits < 16 {
            pixels.div_ceil(8 / bits)
        } else {
            pixels.checked_mul(bits / 8).ok_or(())?
        };
        let align = if rle { 1 } else { 3 };
        Ok((unpadded, unpadded.checked_add(align).ok_or(())? & !align))
    }
}

pub(super) fn validate(bytes: &[u8]) -> Result<Info, ()> {
    let info = parse(bytes)?;
    require_rows(bytes, &info)?;
    Ok(info)
}

/// Strict recovery: every non-RLE colour row, including its padding, must be
/// present, and in an icon so must every AND-mask row the reader will apply
/// whatever the pixels hold.
fn require_rows(bytes: &[u8], info: &Info) -> Result<(), ()> {
    if info.compression.rle() {
        return Ok(());
    }
    let width = usize::try_from(info.width).map_err(|_| ())?;
    let height = usize::try_from(info.height).map_err(|_| ())?;
    let (_, padded) = info.padded_row_bytes(width, false)?;
    let mut end = info
        .pixel_offset
        .checked_add(padded.checked_mul(height).ok_or(())?)
        .ok_or(())?;
    if info.in_ico && (info.bit_count < 16 || info.masks[3] == 0) {
        end = end
            .checked_add(mask_row_bytes(width).checked_mul(height).ok_or(())?)
            .ok_or(())?;
    }
    if end > bytes.len() {
        return Err(());
    }
    Ok(())
}

/// The padded bytes of one AND-mask row.
fn mask_row_bytes(width: usize) -> usize {
    (width.div_ceil(8) + 3) & !3
}

pub(super) fn dimensions(bytes: &[u8]) -> Result<(u32, u32), ()> {
    let info = parse(bytes)?;
    Ok((info.width, info.height))
}

pub(super) fn validation_work(bytes: &[u8]) -> Result<u64, ()> {
    parse(bytes)?.validation_work()
}

/// The DIB of an ICO entry whose info header starts at `offset` in the file.
pub(super) fn parse_icon(bytes: &[u8], offset: usize) -> Result<Info, ()> {
    parse_at(
        bytes,
        Source {
            header_offset: u32::try_from(offset).map_err(|_| ())?,
            img_data_offset: 0,
            in_ico: true,
        },
    )
}

/// [`parse_icon`] with the strict row requirement of [`validate`].
pub(super) fn validate_icon(bytes: &[u8], offset: usize) -> Result<Info, ()> {
    let info = parse_icon(bytes, offset)?;
    require_rows(bytes, &info)?;
    Ok(info)
}

impl Info {
    pub(super) fn validation_work(&self) -> Result<u64, ()> {
        if self.compression.rle() {
            u64::from(self.width)
                .checked_mul(u64::from(self.height))
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(())
        } else {
            Ok(0)
        }
    }
}

const HEADER_OFFSET: u32 = 14;

/// How the reader was constructed: where the info header starts, the pixel
/// offset a file header names (zero when none), and whether it reads an icon.
#[derive(Clone, Copy)]
struct Source {
    header_offset: u32,
    img_data_offset: u32,
    in_ico: bool,
}

fn parse(bytes: &[u8]) -> Result<Info, ()> {
    if bytes.len() < 14 || bytes.get(..2) != Some(b"BM") {
        return Err(());
    }
    parse_at(
        bytes,
        Source {
            header_offset: HEADER_OFFSET,
            img_data_offset: read_u32(bytes, 10)?,
            in_ico: false,
        },
    )
}

#[allow(clippy::too_many_lines)]
fn parse_at(bytes: &[u8], source: Source) -> Result<Info, ()> {
    let Source {
        header_offset,
        img_data_offset,
        in_ico,
    } = source;
    let size = read_u32(bytes, usize::try_from(header_offset).map_err(|_| ())?)?;
    let header_end = header_offset.checked_add(size).ok_or(())?;
    if img_data_offset != 0 && img_data_offset < header_end {
        return Err(());
    }
    let mut os21x = false;
    let mut os22x = false;
    if size == 12 {
        os21x = true;
    } else if matches!(size, 40 | 52 | 56 | 108 | 124) {
        // Windows V3+.
    } else if (16..=64).contains(&size) && (size % 4 == 0 || size == 42 || size == 46) {
        os22x = true;
    } else {
        return Err(());
    }
    let header_start = usize::try_from(header_offset).map_err(|_| ())?;
    let header_len = usize::try_from(size).map_err(|_| ())?;
    let header = bytes
        .get(header_start..header_start.checked_add(header_len).ok_or(())?)
        .ok_or(())?;
    let field16 = |offset: usize| read_u16(header, offset);
    let field32 = |offset: usize| read_u32(header, offset);

    let mut compression = Compression::Rgb;
    let mut clr_used = 0_u32;
    let width: i64;
    let height: i64;
    let mut top_down = false;
    let mut bit_count: u16;
    let mut masks = [0_u32; 4];
    let mut color_space = ColorSpace::Srgb;
    let mut profile: Option<(u32, u32)> = None;
    if os21x {
        width = i64::from(field16(4)?);
        height = i64::from(field16(6)?);
        bit_count = field16(10)?;
    } else {
        width = i64::from(field32(4)? as i32);
        let mut signed_height = field32(8)? as i32;
        if in_ico {
            // The stored height covers the colour rows and the AND mask.
            signed_height /= 2;
        }
        if signed_height == i32::MIN {
            return Err(());
        }
        top_down = signed_height < 0;
        height = i64::from(signed_height).abs();
        bit_count = field16(14)?;
        let mut huffman = false;
        if size >= 20 {
            compression = match (field32(16)?, bit_count) {
                (3, 1) => {
                    os22x = true;
                    huffman = true;
                    Compression::Rgb
                }
                (4, 24) => {
                    os22x = true;
                    Compression::Rle24
                }
                (0, _) => Compression::Rgb,
                (1, _) => Compression::Rle8,
                (2, _) => Compression::Rle4,
                (3, _) => Compression::Bitfields,
                (6, _) => Compression::AlphaBitfields,
                _ => return Err(()),
            };
        }
        // OS/2 2.x Huffman bitmaps are valid but never decoded.
        if huffman {
            return Err(());
        }
        if size >= 36 {
            clr_used = field32(32)?;
        }
        let color_space_in_header = matches!(size, 108 | 124);
        let alpha_in_header = (size == 56 && !os22x) || color_space_in_header;
        let rgb_in_header = (size == 52 && !os22x) || alpha_in_header;
        if rgb_in_header {
            masks[0] = field32(40)?;
            masks[1] = field32(44)?;
            masks[2] = field32(48)?;
        }
        if alpha_in_header {
            masks[3] = field32(52)?;
        }
        if color_space_in_header {
            match field32(56)? {
                0 => {
                    let chromaticity = |offset| -> Result<f32, ()> {
                        Ok(field32(offset)? as f32 * (1.0 / 1_073_741_824.0_f32))
                    };
                    if let Some(to_xyz_d50) = primaries_to_xyz_d50(
                        [
                            chromaticity(60)?,
                            chromaticity(64)?,
                            chromaticity(72)?,
                            chromaticity(76)?,
                            chromaticity(84)?,
                            chromaticity(88)?,
                        ],
                        [0.31271, 0.32902],
                    ) {
                        color_space = ColorSpace::Calibrated {
                            to_xyz_d50,
                            gammas: [field32(96)?, field32(100)?, field32(104)?],
                        };
                    }
                }
                0x4d42_4544 if size >= 120 => {
                    profile = Some((header_offset.wrapping_add(field32(112)?), field32(116)?));
                }
                // sRGB, "Win ", "LINK" and unknown types.
                _ => {}
            }
        }
    }

    // IsInfoHeaderValid.
    if width <= 0 || height == 0 || ((in_ico || top_down) && (os21x || os22x)) {
        return Err(());
    }
    if !matches!(bit_count, 1 | 4 | 8 | 24)
        && (os21x || os22x || !matches!(bit_count, 0 | 2 | 16 | 32))
    {
        return Err(());
    }
    let valid_compression = match compression {
        Compression::Rgb => bit_count != 0,
        Compression::Rle8 => bit_count != 0 && bit_count <= 8,
        Compression::Rle4 => bit_count != 0 && bit_count <= 4,
        Compression::Bitfields | Compression::AlphaBitfields => {
            !os21x && !os22x && matches!(bit_count, 16 | 32)
        }
        Compression::Rle24 => os22x && bit_count == 24,
    };
    if !valid_compression || width >= 1 << 16 || height >= 1 << 16 {
        return Err(());
    }

    if bit_count < 16 {
        let max_colors = 1_u32 << bit_count;
        if clr_used == 0 || clr_used > max_colors {
            clr_used = max_colors;
        }
    }
    match compression {
        Compression::Rle8 => bit_count = 8,
        Compression::Rle4 => bit_count = 4,
        _ => {}
    }

    // ProcessEmbeddedColorProfile.
    if let Some((offset, length)) = profile.filter(|(offset, _)| *offset != 0) {
        let offset = usize::try_from(offset).map_err(|_| ())?;
        let length = usize::try_from(length).map_err(|_| ())?;
        if offset > bytes.len() || bytes.len() - offset < length {
            return Err(());
        }
        color_space = ColorSpace::Embedded { offset, length };
    }

    let mut decoded_offset = header_end;
    let mut shifts = [0_u32; 4];
    let mut narrow_bits = [0_u32; 4];
    let mut palette_offset = 0;
    let mut palette_entries = 0;
    let palette_stride = if os21x { 3 } else { 4 };
    if bit_count >= 16 {
        let alpha_in_header = (size == 56 && !os22x) || matches!(size, 108 | 124);
        let rgb_in_header = (size == 52 && !os22x) || alpha_in_header;
        if !matches!(
            compression,
            Compression::Bitfields | Compression::AlphaBitfields
        ) {
            let bits = if bit_count == 16 { 5 } else { 8 };
            for (index, mask) in masks[..3].iter_mut().enumerate() {
                let index = u32::try_from(index).map_err(|_| ())?;
                *mask =
                    ((1_u32 << (bits * (3 - index))) - 1) ^ ((1_u32 << (bits * (2 - index))) - 1);
            }
        } else if !rgb_in_header {
            let read_alpha = compression == Compression::AlphaBitfields;
            let length = if read_alpha { 16 } else { 12 };
            let masks_end = header_end.checked_add(length).ok_or(())?;
            if img_data_offset != 0 && img_data_offset < masks_end {
                return Err(());
            }
            let start = usize::try_from(decoded_offset).map_err(|_| ())?;
            masks[0] = read_u32(bytes, start)?;
            masks[1] = read_u32(bytes, start + 4)?;
            masks[2] = read_u32(bytes, start + 8)?;
            if read_alpha {
                masks[3] = read_u32(bytes, start + 12)?;
            }
            decoded_offset = masks_end;
        }
        if !alpha_in_header && compression != Compression::AlphaBitfields {
            // Inside an icon a 32 bpp BI_RGB pixel's fourth byte is alpha.
            masks[3] = if in_ico && compression != Compression::Bitfields && bit_count == 32 {
                0xff00_0000
            } else {
                0
            };
        }
        for index in 0..4 {
            if bit_count < 32 {
                masks[index] &= (1_u32 << bit_count) - 1;
            }
            let mut mask = masks[index];
            if mask == 0 {
                continue;
            }
            if masks[..index].iter().any(|other| mask & other != 0) {
                return Err(());
            }
            shifts[index] = mask.trailing_zeros();
            mask >>= shifts[index];
            let bits = mask.trailing_ones();
            if mask >> bits != 0 {
                return Err(());
            }
            if bits >= 8 {
                shifts[index] += bits - 8;
            } else {
                narrow_bits[index] = bits;
            }
        }
    } else {
        let stride = palette_stride as u32;
        let mut colors = clr_used;
        let table_end = header_end
            .checked_add(colors.checked_mul(stride).ok_or(())?)
            .ok_or(())?;
        if img_data_offset != 0 && img_data_offset < table_end {
            colors = (img_data_offset - header_end) / stride;
        }
        let table_bytes = usize::try_from(colors * stride).map_err(|_| ())?;
        let start = usize::try_from(decoded_offset).map_err(|_| ())?;
        if start > bytes.len() || bytes.len() - start < table_bytes {
            return Err(());
        }
        palette_offset = start;
        palette_entries = usize::try_from(colors).map_err(|_| ())?;
        decoded_offset = decoded_offset.checked_add(colors * stride).ok_or(())?;
    }
    if img_data_offset != 0 {
        decoded_offset = img_data_offset;
    }

    Ok(Info {
        width: u32::try_from(width).map_err(|_| ())?,
        height: u32::try_from(height).map_err(|_| ())?,
        top_down,
        bit_count,
        compression,
        pixel_offset: usize::try_from(decoded_offset).map_err(|_| ())?,
        palette_offset,
        palette_entries,
        palette_stride,
        masks,
        shifts,
        narrow_bits,
        color_space,
        in_ico,
    })
}

/// `skcms_PrimariesToXYZD50` in its single-precision arithmetic, with the
/// matrix inversion in double precision as skcms performs it.
fn primaries_to_xyz_d50(primaries: [f32; 6], white: [f32; 2]) -> Option<[[f32; 3]; 3]> {
    if primaries
        .iter()
        .chain(white.iter())
        .any(|value| !(0.0..=1.0).contains(value))
    {
        return None;
    }
    let [rx, ry, gx, gy, bx, by] = primaries;
    let [wx, wy] = white;
    let xyz = [
        [rx, gx, bx],
        [ry, gy, by],
        [1.0 - rx - ry, 1.0 - gx - gy, 1.0 - bx - by],
    ];
    let inverse = invert(&xyz)?;
    let white_xyz = [wx / wy, 1.0, (1.0 - wx - wy) / wy];
    let scale = multiply_vector(&inverse, white_xyz);
    let to_xyz = concat(
        &xyz,
        &[
            [scale[0], 0.0, 0.0],
            [0.0, scale[1], 0.0],
            [0.0, 0.0, scale[2]],
        ],
    );
    // skcms_AdaptToXYZD50 (Bradford).
    const XYZ_TO_LMS: [[f32; 3]; 3] = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    const LMS_TO_XYZ: [[f32; 3]; 3] = [
        [0.986_992_9, -0.147_054_3, 0.159_962_7],
        [0.432_305_3, 0.518_360_3, 0.049_291_2],
        [-0.008_528_7, 0.040_042_8, 0.968_486_7],
    ];
    let source_cone = multiply_vector(&XYZ_TO_LMS, white_xyz);
    let destination_cone = multiply_vector(&XYZ_TO_LMS, [0.96422, 1.0, 0.82521]);
    let adapt = [
        [destination_cone[0] / source_cone[0], 0.0, 0.0],
        [0.0, destination_cone[1] / source_cone[1], 0.0],
        [0.0, 0.0, destination_cone[2] / source_cone[2]],
    ];
    let adapt = concat(&LMS_TO_XYZ, &concat(&adapt, &XYZ_TO_LMS));
    Some(concat(&adapt, &to_xyz))
}

fn multiply_vector(matrix: &[[f32; 3]; 3], vector: [f32; 3]) -> [f32; 3] {
    matrix.map(|row| row[0] * vector[0] + row[1] * vector[1] + row[2] * vector[2])
}

fn concat(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (row, out_row) in out.iter_mut().enumerate() {
        for (column, value) in out_row.iter_mut().enumerate() {
            *value = a[row][0] * b[0][column] + a[row][1] * b[1][column] + a[row][2] * b[2][column];
        }
    }
    out
}

/// `skcms_Matrix3x3_invert`.
fn invert(src: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let a = src.map(|row| row.map(f64::from));
    let [[a00, a01, a02], [a10, a11, a12], [a20, a21, a22]] = a;
    let b0 = a00 * a11 - a01 * a10;
    let b1 = a00 * a12 - a02 * a10;
    let b2 = a01 * a12 - a02 * a11;
    let b3 = a20;
    let b4 = a21;
    let b5 = a22;
    let determinant = b0 * b5 - b1 * b4 + b2 * b3;
    if determinant == 0.0 {
        return None;
    }
    let inverse_determinant = 1.0 / determinant;
    if inverse_determinant > f64::from(f32::MAX)
        || inverse_determinant < -f64::from(f32::MAX)
        || !(inverse_determinant as f32).is_finite()
    {
        return None;
    }
    let scale = |value: f64| value * inverse_determinant;
    let out = [
        [
            scale(a11 * b5 - a12 * b4),
            scale(a02 * b4 - a01 * b5),
            scale(b2),
        ],
        [
            scale(a12 * b3 - a10 * b5),
            scale(a00 * b5 - a02 * b3),
            scale(-b1),
        ],
        [
            scale(a10 * b4 - a11 * b3),
            scale(a01 * b3 - a00 * b4),
            scale(b0),
        ],
    ]
    .map(|row| row.map(|value| value as f32));
    out.iter()
        .flatten()
        .all(|value| value.is_finite())
        .then_some(out)
}

/// The decoder's cursor and alpha state, shared between the non-RLE rows and
/// RLE literal runs.
struct Frame<'a> {
    bytes: &'a [u8],
    info: &'a Info,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    x: usize,
    /// Output row, top-origin; may step past either end.
    y: isize,
    seen_zero_alpha: bool,
    seen_non_zero_alpha: bool,
}

impl Frame<'_> {
    fn past_end(&self, rows: usize) -> bool {
        let rows = isize::try_from(rows).unwrap_or(isize::MAX);
        if self.info.top_down {
            self.y.saturating_add(rows) >= isize::try_from(self.height).unwrap_or(isize::MAX)
        } else {
            self.y.saturating_sub(rows) < 0
        }
    }

    fn next_row(&mut self, rows: usize) {
        let rows = isize::try_from(rows).unwrap_or(isize::MAX);
        self.y = if self.info.top_down {
            self.y.saturating_add(rows)
        } else {
            self.y.saturating_sub(rows)
        };
    }

    fn set(&mut self, color: [u8; 4]) {
        if let Some(y) = usize::try_from(self.y)
            .ok()
            .filter(|y| *y < self.height && self.x < self.width)
        {
            let destination = (y * self.width + self.x) * 4;
            self.rgba[destination..destination + 4].copy_from_slice(&color);
        }
        self.x += 1;
    }

    fn palette(&self, index: usize) -> [u8; 4] {
        if index >= self.info.palette_entries {
            return [0, 0, 0, 255];
        }
        let offset = self.info.palette_offset + index * self.info.palette_stride;
        match self.bytes.get(offset..offset + 3) {
            Some(color) => [color[2], color[1], color[0], 255],
            None => [0, 0, 0, 255],
        }
    }

    fn component(&self, pixel: u32, index: usize) -> u8 {
        let value = ((pixel & self.info.masks[index]) >> self.info.shifts[index]) as u8;
        match self.info.narrow_bits[index] {
            0 => value,
            bits => {
                let maximum = (1_u32 << bits) - 1;
                ((2 * u32::from(value) * 255 + maximum) / (2 * maximum)) as u8
            }
        }
    }

    /// `ProcessNonRLEData`: `pixels` pixels of the current row starting at
    /// `offset`, or `None` when they run past the row or the data.
    fn pixels(&mut self, offset: usize, pixels: usize, rle: bool) -> Option<usize> {
        let end_x = self.x.checked_add(pixels)?;
        if end_x > self.width {
            return None;
        }
        let (unpadded, padded) = self.info.padded_row_bytes(pixels, rle).ok()?;
        let data = self.bytes.get(offset..offset.checked_add(padded)?)?;
        let bits = usize::from(self.info.bit_count);
        if bits < 16 {
            let mask = ((1_u32 << bits) - 1) as u8;
            for byte in &data[..unpadded] {
                let mut byte = *byte;
                for _ in 0..8 / bits {
                    if self.x >= end_x {
                        break;
                    }
                    let color = self.palette(usize::from((byte >> (8 - bits)) & mask));
                    self.set(color);
                    byte = byte.checked_shl(bits as u32).unwrap_or(0);
                }
            }
        } else {
            let bytes_per_pixel = bits / 8;
            for pixel in data[..unpadded].chunks_exact(bytes_per_pixel) {
                let mut word = [0_u8; 4];
                word[..bytes_per_pixel].copy_from_slice(pixel);
                let pixel = u32::from_le_bytes(word);
                let mut alpha = if self.info.masks[3] == 0 {
                    255
                } else {
                    self.component(pixel, 3)
                };
                if !self.seen_non_zero_alpha && alpha == 0 {
                    self.seen_zero_alpha = true;
                    alpha = 255;
                } else {
                    self.seen_non_zero_alpha = true;
                    if self.seen_zero_alpha {
                        self.rgba.fill(0);
                        self.seen_zero_alpha = false;
                    }
                }
                let color = [
                    self.component(pixel, 0),
                    self.component(pixel, 1),
                    self.component(pixel, 2),
                    alpha,
                ];
                self.set(color);
            }
        }
        Some(offset + padded)
    }

    /// `ProcessRLEData`, stopping wherever the reference stops painting. The
    /// offset of an end-of-bitmap marker, where the reference considers the
    /// stream complete, or `None` for any other stop.
    fn rle(&mut self) -> Option<usize> {
        let bytes = self.bytes;
        let mut offset = self.info.pixel_offset;
        while let Some(&[count, code]) = bytes.get(offset..offset.saturating_add(2)) {
            let past_end = self.past_end(0);
            if (count != 0 || code != 1) && past_end {
                return None;
            }
            if count == 0 {
                match code {
                    0 => {
                        self.x = 0;
                        self.next_row(1);
                        offset += 2;
                    }
                    1 => return Some(offset),
                    2 => {
                        let &[dx, dy] = bytes.get(offset + 2..offset.saturating_add(4))? else {
                            return None;
                        };
                        if self.x + usize::from(dx) > self.width || self.past_end(usize::from(dy)) {
                            return None;
                        }
                        self.x += usize::from(dx);
                        self.next_row(usize::from(dy));
                        offset += 4;
                    }
                    literal => offset = self.pixels(offset + 2, usize::from(literal), true)?,
                }
                continue;
            }
            let end_x = (self.x + usize::from(count)).min(self.width);
            if self.info.compression == Compression::Rle24 {
                let &[_, blue, green, red] = bytes.get(offset..offset.saturating_add(4))? else {
                    return None;
                };
                while self.x < end_x {
                    self.set([red, green, blue, 255]);
                }
                offset += 4;
            } else {
                let indices = if self.info.compression == Compression::Rle4 {
                    [usize::from(code >> 4), usize::from(code & 0x0f)]
                } else {
                    [usize::from(code); 2]
                };
                let mut which = 0;
                while self.x < end_x {
                    let color = self.palette(indices[which]);
                    self.set(color);
                    which ^= 1;
                }
                offset += 2;
            }
        }
        None
    }

    /// The icon's AND mask from `offset`: 1 bpp rows in the colour rows'
    /// order, each set bit clearing its pixel to transparent black.
    fn and_mask(&mut self, mut offset: usize) -> Result<(), ()> {
        let stride = mask_row_bytes(self.width);
        self.x = 0;
        self.y = if self.info.top_down {
            0
        } else {
            isize::try_from(self.height).map_err(|_| ())? - 1
        };
        while !self.past_end(0) {
            let row = self
                .bytes
                .get(offset..offset.checked_add(stride).ok_or(())?)
                .ok_or(())?;
            let y = usize::try_from(self.y).map_err(|_| ())?;
            for x in 0..self.width {
                if row[x / 8] & (0x80 >> (x % 8)) != 0 {
                    let destination = (y * self.width + x) * 4;
                    self.rgba[destination..destination + 4].fill(0);
                }
            }
            offset += stride;
            self.next_row(1);
        }
        Ok(())
    }

    /// Whether the icon reader goes on to the AND mask.
    fn applies_and_mask(&self) -> bool {
        self.info.in_ico
            && (self.info.bit_count < 16 || self.info.masks[3] == 0 || !self.seen_non_zero_alpha)
    }
}

pub(super) fn decode(bytes: &[u8], info: &Info) -> Result<Vec<u8>, ()> {
    let width = usize::try_from(info.width).map_err(|_| ())?;
    let height = usize::try_from(info.height).map_err(|_| ())?;
    let rgba_size = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(())?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(rgba_size).map_err(|_| ())?;
    rgba.resize(rgba_size, 0);
    let mut frame = Frame {
        bytes,
        info,
        width,
        height,
        rgba,
        x: 0,
        y: if info.top_down {
            0
        } else {
            isize::try_from(height).map_err(|_| ())? - 1
        },
        seen_zero_alpha: false,
        seen_non_zero_alpha: false,
    };
    let mask_offset = if info.compression.rle() {
        match frame.rle() {
            Some(offset) => offset,
            None => return Ok(frame.rgba),
        }
    } else {
        let mut offset = info.pixel_offset;
        while !frame.past_end(0) {
            offset = frame.pixels(offset, width, false).ok_or(())?;
            frame.x = 0;
            frame.next_row(1);
        }
        offset
    };
    if frame.applies_and_mask() {
        frame.and_mask(mask_offset)?;
    }
    Ok(frame.rgba)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ()> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset.checked_add(2).ok_or(())?)
            .ok_or(())?
            .try_into()
            .map_err(|_| ())?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ()> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset.checked_add(4).ok_or(())?)
            .ok_or(())?
            .try_into()
            .map_err(|_| ())?,
    ))
}
