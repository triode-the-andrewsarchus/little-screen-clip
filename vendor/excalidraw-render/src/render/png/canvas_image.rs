//! The reference canvas's raster `drawImage` (#198): Chromium 149's software
//! canvas with `imageSmoothingQuality` "low" over Skia 53348aa. Every value
//! here is premultiplied RGBA8 and every coordinate is binary32, computed one
//! rounded operation at a time as the pinned C++ computes it.
//!
//! A draw is three stages:
//!
//! 1. cc's `SoftwareImageDecodeCache` picks a decode level for the draw
//!    ([`cc_level_for_size`]): the source rectangle rounded out to whole
//!    pixels is scaled to `round(|size * device scale|)`, and the chosen level
//!    is the largest whose next, ceil-halved level would still not be smaller
//!    than that target in either axis. Level 0 is the decoded image itself.
//! 2. A level above 0 is built by `SkPixmap::scalePixels` with linear filtering
//!    and nearest mipmaps ([`scale_pixels`]): Skia's own mipmap chain
//!    (floor-halved, a truncating 2x2 box or 1-2-1 taps on an odd axis) at the
//!    level `SkMipmap::ComputeLevel` picks, resampled with the raster
//!    pipeline's lowp `bilerp_clamp_8888`. When the same image and source
//!    rectangle was already built at another level earlier in the render, the
//!    new level is scaled from the smallest cached one at least its size
//!    ([`DecodeCachePlanner`]).
//! 3. `SkBitmapDevice::drawImageRect` draws the level with the legacy
//!    `SkBitmapProcState` sampler ([`device_draw_image_rect`]): a sprite copy
//!    when the device matrix is an integer translate the bitmap fills exactly,
//!    otherwise the inverse matrix, downgraded to a translate or a nearest
//!    copy when it is within Skia's tolerances, else a 4-bit bilinear filter
//!    whose x coordinate steps in 32.32 fixed point and restarts every 127
//!    pixels of a span. Paint alpha scales the shaded pixels last.
//!
//! Only `log2` is transcendental: [`skia_compute_level`] computes it through
//! [`crate::portable_math::log2`] in binary64 and rounds to binary32, i.e. a
//! correctly rounded `log2f` except when the exact value lies within binary64
//! error of a binary32 rounding boundary. Every `sqrt` is IEEE 754 correctly
//! rounded and so identical on every target.

use crate::portable_math;

use super::super::png_encoder::png_size_error;
use super::super::FormatEncodeError;

/// Skia's `SkScalarRoundToInt` / `SkRect::round`: `floor(v + 0.5)`.
fn round_half_up(value: f32) -> i64 {
    (f64::from(value) + 0.5).floor() as i64
}

/// A premultiplied RGBA8 image, or a rectangle of one: `stride` is the row
/// pitch of `data` in pixels.
#[derive(Clone, Copy, Debug)]
pub(super) struct PremulView<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: u32,
    pub(super) data: &'a [u8],
}

impl<'a> PremulView<'a> {
    pub(super) const fn new(width: u32, height: u32, data: &'a [u8]) -> Self {
        Self {
            width,
            height,
            stride: width,
            data,
        }
    }

    /// The `width` by `height` rectangle at `(x, y)`, which must lie inside.
    pub(super) fn sub(
        self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Self, FormatEncodeError> {
        if x.checked_add(width).is_none_or(|right| right > self.width)
            || y.checked_add(height)
                .is_none_or(|bottom| bottom > self.height)
        {
            return Err(FormatEncodeError::Encoding(
                "image source rectangle lies outside the image".to_owned(),
            ));
        }
        let start = (y as usize)
            .checked_mul(self.stride as usize)
            .and_then(|row| row.checked_add(x as usize))
            .and_then(|pixel| pixel.checked_mul(4))
            .ok_or_else(png_size_error)?;
        Ok(Self {
            width,
            height,
            stride: self.stride,
            data: self.data.get(start..).ok_or_else(png_size_error)?,
        })
    }

    #[inline]
    fn pixel(&self, x: usize, y: usize) -> &[u8] {
        let start = (y * self.stride as usize + x) * 4;
        &self.data[start..start + 4]
    }

    fn check(&self) -> Result<(), FormatEncodeError> {
        if self.width == 0 || self.height == 0 {
            return Err(FormatEncodeError::Encoding("empty image level".to_owned()));
        }
        let needed = (u64::from(self.height) - 1)
            .checked_mul(u64::from(self.stride))
            .and_then(|pixels| pixels.checked_add(u64::from(self.width)))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(png_size_error)?;
        if self.stride < self.width || (self.data.len() as u64) < needed {
            return Err(FormatEncodeError::Encoding(
                "image level storage is too small".to_owned(),
            ));
        }
        Ok(())
    }
}

/// An owned premultiplied RGBA8 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PremulImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) data: Vec<u8>,
}

impl PremulImage {
    fn zeroed(width: u32, height: u32) -> Result<Self, FormatEncodeError> {
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(png_size_error)?;
        let mut data = Vec::new();
        data.try_reserve_exact(bytes)
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "raster_surface",
                subject: "png_image_level",
            })?;
        data.resize(bytes, 0);
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub(super) fn view(&self) -> PremulView<'_> {
        PremulView::new(self.width, self.height, &self.data)
    }
}

/// `max(1, ceil(n / 2^level))`: cc's mip level size (`MipMapUtil`).
pub(super) fn cc_level_size(size: u32, level: u32) -> u32 {
    if level >= 32 {
        return 1;
    }
    let size = (u64::from(size) + (1_u64 << level) - 1) >> level;
    (size as u32).max(1)
}

/// `cc::MipMapUtil::GetLevelForSize`: the largest level whose next level is
/// not smaller than the `target` size in either axis.
pub(super) fn cc_level_for_size(width: u32, height: u32, target: (i64, i64)) -> u32 {
    let mut level = 0;
    loop {
        let next = (
            cc_level_size(width, level + 1),
            cc_level_size(height, level + 1),
        );
        if i64::from(next.0) < target.0 || i64::from(next.1) < target.1 {
            return level;
        }
        if cc_level_size(width, level) == 1 && cc_level_size(height, level) == 1 {
            return level;
        }
        level += 1;
    }
}

/// `SkMipmap::ComputeLevel` under `SK_SUPPORT_LEGACY_ANISOTROPIC_MIPMAP_SCALE`
/// followed by `extractLevel`'s `sk_float_round2int` (nearest mipmaps): the
/// geometric mean of the two scales with a -0.5 bias; 0 selects the base.
pub(super) fn skia_compute_level(scale_x: f32, scale_y: f32) -> u32 {
    let scale = (scale_x * scale_y).sqrt();
    if !(scale < 1.0 && scale > 0.0) {
        return 0;
    }
    // `log2f`, correctly rounded from the deterministic binary64 `log2`.
    let log2 = portable_math::log2(f64::from(scale)) as f32;
    let level = (-log2 - 0.5).max(0.0);
    if !level.is_finite() {
        return 0;
    }
    (f64::from(level) + 0.5).floor().clamp(0.0, 64.0) as u32
}

/// One `SkMipmap` level below `source`: floor-halved dimensions (at least 1);
/// each axis averages two pixels, or on an odd axis longer than 1 the three
/// pixels `2i..=2i+2` with weights 1-2-1; a 1-pixel axis keeps its pixel. The
/// weighted sum is shifted right by the weight total's log2 (truncating).
pub(super) fn skia_half(source: PremulView<'_>) -> Result<PremulImage, FormatEncodeError> {
    source.check()?;
    let (width, height) = (source.width as usize, source.height as usize);
    let taps = |size: usize, index: usize| -> ([(usize, u32); 3], usize) {
        if size == 1 {
            ([(0, 1), (0, 0), (0, 0)], 1)
        } else if size & 1 == 1 {
            ([(2 * index, 1), (2 * index + 1, 2), (2 * index + 2, 1)], 3)
        } else {
            ([(2 * index, 1), (2 * index + 1, 1), (0, 0)], 2)
        }
    };
    let mut output = PremulImage::zeroed((source.width / 2).max(1), (source.height / 2).max(1))?;
    let output_width = output.width as usize;
    for (row, line) in output.data.chunks_exact_mut(output_width * 4).enumerate() {
        let (y_taps, y_count) = taps(height, row);
        let y_total: u32 = y_taps[..y_count].iter().map(|tap| tap.1).sum();
        for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x_taps, x_count) = taps(width, column);
            let x_total: u32 = x_taps[..x_count].iter().map(|tap| tap.1).sum();
            let shift = (x_total * y_total).trailing_zeros();
            let mut sums = [0_u32; 4];
            for &(y, y_weight) in &y_taps[..y_count] {
                for &(x, x_weight) in &x_taps[..x_count] {
                    for (sum, value) in sums.iter_mut().zip(source.pixel(x, y)) {
                        *sum += u32::from(*value) * x_weight * y_weight;
                    }
                }
            }
            for (target, sum) in pixel.iter_mut().zip(sums) {
                *target = (sum >> shift) as u8;
            }
        }
    }
    Ok(output)
}

/// The dimensions of Skia's mipmap levels `1..=level` of a `width` by
/// `height` base: `max(1, floor(n / 2^i))`.
fn skia_level_dimensions(width: u32, height: u32, level: u32) -> (u32, u32) {
    let halve = |size: u32| {
        if level >= 32 {
            1
        } else {
            (size >> level).max(1)
        }
    };
    (halve(width), halve(height))
}

/// `SkMipmap::ComputeLevelCount`: halvings until both axes are 1.
fn skia_level_count(width: u32, height: u32) -> u32 {
    let largest = width.max(height);
    if largest < 2 {
        0
    } else {
        31 - largest.leading_zeros()
    }
}

/// The Skia mipmap level `scalePixels` samples to resize a `width` by
/// `height` image to `level_width` by `level_height`.
pub(super) fn scale_pixels_skia_level(
    width: u32,
    height: u32,
    level_width: u32,
    level_height: u32,
) -> u32 {
    // SkMatrix::RectToRect(source bounds, destination bounds), inverted in
    // binary32; SkMipmapAccessor decomposes the inverse and inverts again.
    let inverse_x = 1.0 / (level_width as f32 / width as f32);
    let inverse_y = 1.0 / (level_height as f32 / height as f32);
    skia_compute_level(1.0 / inverse_x, 1.0 / inverse_y).min(skia_level_count(width, height))
}

/// The pixels of Skia's mipmap chain `scalePixels` builds up to its level
/// (the levels it allocates besides the base), for the resource ledger.
pub(super) fn scale_pixels_chain_pixels(
    width: u32,
    height: u32,
    level_width: u32,
    level_height: u32,
) -> u64 {
    let level = scale_pixels_skia_level(width, height, level_width, level_height);
    (1..=level)
        .map(|index| {
            let (w, h) = skia_level_dimensions(width, height, index);
            u64::from(w) * u64::from(h)
        })
        .sum()
}

/// `SkPixmap::scalePixels(source -> level_width x level_height)` with
/// `{kLinear, kNearest}` sampling: Skia's mipmap level, then the raster
/// pipeline's lowp bilinear filter.
pub(super) fn scale_pixels(
    source: PremulView<'_>,
    level_width: u32,
    level_height: u32,
) -> Result<PremulImage, FormatEncodeError> {
    source.check()?;
    let (width, height) = (source.width, source.height);
    let inverse_x = 1.0 / (level_width as f32 / width as f32);
    let inverse_y = 1.0 / (level_height as f32 / height as f32);
    let level = scale_pixels_skia_level(width, height, level_width, level_height);
    if level == 0 {
        return lowp_bilinear(source, level_width, level_height, inverse_x, inverse_y);
    }
    let mut mip = skia_half(source)?;
    for _ in 1..level {
        mip = skia_half(mip.view())?;
    }
    // SkMipmapAccessor: Scale(level / base) . inverse.
    let kx = mip.width as f32 / width as f32;
    let ky = mip.height as f32 / height as f32;
    lowp_bilinear(
        mip.view(),
        level_width,
        level_height,
        kx * inverse_x,
        ky * inverse_y,
    )
}

/// `_mm_mulhrs_epi16`-style rounding Q15 product.
#[inline]
const fn mulhrs(a: i32, b: i32) -> i32 {
    (a * b + (1 << 14)) >> 15
}

/// SkRasterPipeline lowp `bilerp_clamp_8888` for a scale matrix: the pixel
/// centre maps through `inverse` (no fused multiply-add), each axis yields a
/// Q16 coordinate `floor(65536 c + 0.5) - 32768` whose low half is the signed
/// fraction, and two middle-form lerps with `mulhrs` blend the four clamped
/// taps.
pub(super) fn lowp_bilinear(
    source: PremulView<'_>,
    width: u32,
    height: u32,
    inverse_x: f32,
    inverse_y: f32,
) -> Result<PremulImage, FormatEncodeError> {
    source.check()?;
    if !(inverse_x.is_finite() && inverse_y.is_finite()) {
        return Err(FormatEncodeError::Encoding(
            "nonfinite image level scale".to_owned(),
        ));
    }
    let axis = |centre: f32, size: u32| -> (usize, usize, i32) {
        let q = ((65536.0_f32 * centre) + 0.5).floor() as i64 - 32768;
        let whole = q >> 16;
        let fraction = i32::from(((q ^ 0x8000) & 0xffff) as u16 as i16);
        let clamp = |value: i64| value.clamp(0, i64::from(size) - 1) as usize;
        (clamp(whole), clamp(whole + 1), fraction)
    };
    let lerp_x = |left: i32, right: i32, t: i32| -> i32 {
        ((mulhrs(t, (right - left) << 7) + ((right + left) << 7) + 1) & 0xffff) >> 1
    };
    let lerp_y = |top: i32, bottom: i32, t: i32| -> i32 {
        ((mulhrs(t, bottom - top) + (bottom + top) + 0x80) & 0xffff) >> 8
    };
    let mut output = PremulImage::zeroed(width, height)?;
    let output_width = width as usize;
    for (row, line) in output.data.chunks_exact_mut(output_width * 4).enumerate() {
        let (y0, y1, ty) = axis((row as f32 + 0.5) * inverse_y, source.height);
        for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x0, x1, tx) = axis((column as f32 + 0.5) * inverse_x, source.width);
            let (p00, p10) = (source.pixel(x0, y0), source.pixel(x1, y0));
            let (p01, p11) = (source.pixel(x0, y1), source.pixel(x1, y1));
            for (channel, target) in pixel.iter_mut().enumerate() {
                let top = lerp_x(i32::from(p00[channel]), i32::from(p10[channel]), tx);
                let bottom = lerp_x(i32::from(p01[channel]), i32::from(p11[channel]), tx);
                *target = lerp_y(top, bottom, ty) as u8;
            }
        }
    }
    Ok(output)
}

/// A binary32 rectangle `(x, y, width, height)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RectF {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

/// A binary32 affine matrix in canvas order: `x' = a x + c y + e`,
/// `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AffineF {
    pub(super) a: f32,
    pub(super) b: f32,
    pub(super) c: f32,
    pub(super) d: f32,
    pub(super) e: f32,
    pub(super) f: f32,
}

impl AffineF {
    const fn is_scale_translate(self) -> bool {
        self.b == 0.0 && self.c == 0.0
    }

    /// `SkMatrix::setConcat(self, scale-translate)`, one rounding per step.
    fn concat_scale_translate(self, sx: f32, sy: f32, tx: f32, ty: f32) -> Self {
        Self {
            a: self.a * sx,
            b: self.b * sx,
            c: self.c * sy,
            d: self.d * sy,
            e: self.a * tx + self.c * ty + self.e,
            f: self.b * tx + self.d * ty + self.f,
        }
    }
}

/// One canvas `drawImage(image, sx, sy, sw, sh, dx, dy, dw, dh)` under the
/// current transform `ctm`, with the paint alpha `alpha8`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CanvasImageDraw {
    pub(super) ctm: AffineF,
    pub(super) src: RectF,
    pub(super) dst: RectF,
    pub(super) alpha8: u8,
}

/// Skia's paint alpha for a canvas `globalAlpha`: `SkPaint::getAlpha`,
/// `round(alphaf * 255)` in binary32.
pub(super) fn paint_alpha8(opacity: f64) -> u8 {
    let alpha = (opacity as f32).clamp(0.0, 1.0);
    round_half_up(alpha * 255.0).clamp(0, 255) as u8
}

/// The cc decode level a draw uses and the rectangle it decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CcDecode {
    /// `roundOut(src)` clamped to the image, `(x, y, width, height)`.
    pub(super) rect: (u32, u32, u32, u32),
    pub(super) level: u32,
    pub(super) level_width: u32,
    pub(super) level_height: u32,
}

impl CanvasImageDraw {
    /// `SkMatrix::RectToRect(src, dst)` as `(sx, sy, tx, ty)`.
    fn rect_to_rect(src: RectF, dst: RectF) -> (f32, f32, f32, f32) {
        let sx = dst.width / src.width;
        let sy = dst.height / src.height;
        (sx, sy, dst.x - src.x * sx, dst.y - src.y * sy)
    }

    /// The cc decode for an `image_width` by `image_height` image: the source
    /// rectangle rounded out to whole pixels, the device scale of
    /// `ctm * RectToRect(src, dst)`, and `GetLevelForSize` of the rounded
    /// scaled size. `None` when the rounded-out rectangle is empty.
    pub(super) fn cc_decode(&self, image_width: u32, image_height: u32) -> Option<CcDecode> {
        let src = self.src;
        let (sx, sy, tx, ty) = Self::rect_to_rect(src, self.dst);
        let device = self.ctm.concat_scale_translate(sx, sy, tx, ty);
        let clamp_to = |value: f32, size: u32| -> u32 {
            if value.is_nan() {
                0
            } else {
                f64::from(value).clamp(0.0, f64::from(size)) as u32
            }
        };
        let left = clamp_to(src.x.floor(), image_width);
        let top = clamp_to(src.y.floor(), image_height);
        let right = clamp_to((src.x + src.width).ceil(), image_width);
        let bottom = clamp_to((src.y + src.height).ceil(), image_height);
        if right <= left || bottom <= top {
            return None;
        }
        let (width, height) = (right - left, bottom - top);
        // cc's DrawImage takes the matrix's own diagonal unless it has a skew,
        // in which case SkMatrix::decomposeScale's column lengths.
        let (scale_x, scale_y) = if device.is_scale_translate() {
            (device.a, device.d)
        } else {
            let length = |x: f32, y: f32| (x * x + y * y).sqrt();
            (length(device.a, device.b), length(device.c, device.d))
        };
        let target = |size: u32, scale: f32| -> i64 {
            let scaled = (size as f32 * scale).abs();
            if scaled.is_finite() {
                round_half_up(scaled)
            } else {
                i64::MAX
            }
        };
        let (target_width, target_height) = (target(width, scale_x), target(height, scale_y));
        let level = if target_width > 0 && target_height > 0 {
            cc_level_for_size(width, height, (target_width, target_height))
        } else {
            0
        };
        Some(CcDecode {
            rect: (left, top, width, height),
            level,
            level_width: cc_level_size(width, level),
            level_height: cc_level_size(height, level),
        })
    }

    /// The draw re-expressed against the decoded level: the source rectangle
    /// relative to the decoded rectangle, scaled by level / decoded size
    /// (`GetScaleAdjustmentForLevel`, `AdjustSrcRectForScale`).
    pub(super) fn for_level(&self, decode: &CcDecode) -> Self {
        let (left, top, width, height) = decode.rect;
        let kx = decode.level_width as f32 / width as f32;
        let ky = decode.level_height as f32 / height as f32;
        Self {
            src: RectF {
                x: (self.src.x - left as f32) * kx,
                y: (self.src.y - top as f32) * ky,
                width: self.src.width * kx,
                height: self.src.height * ky,
            },
            ..*self
        }
    }
}

/// The device pixels one draw paints: `left..right` by `top..bottom`, with
/// the premultiplied pixels of that rectangle row by row.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PaintedRect {
    pub(super) left: u32,
    pub(super) top: u32,
    pub(super) right: u32,
    pub(super) bottom: u32,
    pub(super) pixels: Vec<u8>,
}

/// `SkBitmapDevice::drawImageRect(bitmap, src, dst)` with linear sampling,
/// `kFast` constraint and no antialiasing, for a scale-translate `ctm`, on a
/// `canvas_width` by `canvas_height` device. `None` when nothing is painted.
pub(super) fn device_draw_image_rect(
    bitmap: PremulView<'_>,
    draw: &CanvasImageDraw,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<Option<PaintedRect>, FormatEncodeError> {
    bitmap.check()?;
    let ctm = draw.ctm;
    if !ctm.is_scale_translate() {
        return Err(FormatEncodeError::Encoding(
            "the canvas image sampler needs a scale-translate transform".to_owned(),
        ));
    }
    let (src, dst) = (draw.src, draw.dst);
    let (bitmap_width, bitmap_height) = (bitmap.width as f32, bitmap.height as f32);
    let (msx, msy, mtx, mty) = CanvasImageDraw::rect_to_rect(src, dst);
    let (mut left, mut top) = (src.x, src.y);
    let (mut right, mut bottom) = (src.x + src.width, src.y + src.height);
    let (mut dst_left, mut dst_top) = (dst.x, dst.y);
    let (mut dst_right, mut dst_bottom) = (dst.x + dst.width, dst.y + dst.height);
    if !(left >= 0.0 && top >= 0.0 && right <= bitmap_width && bottom <= bitmap_height) {
        // Clip the source to the bitmap and map the clipped rectangle back.
        left = left.max(0.0);
        top = top.max(0.0);
        right = right.min(bitmap_width);
        bottom = bottom.min(bitmap_height);
        dst_left = left * msx + mtx;
        dst_top = top * msy + mty;
        dst_right = right * msx + mtx;
        dst_bottom = bottom * msy + mty;
    }
    let subset = !(left <= 0.0 && top <= 0.0 && right >= bitmap_width && bottom >= bitmap_height);
    let device = ctm.concat_scale_translate(msx, msy, mtx, mty);
    if ![device.a, device.d, device.e, device.f]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(FormatEncodeError::Encoding(
            "nonfinite image device matrix".to_owned(),
        ));
    }
    let clamp_x = |value: i64| value.clamp(0, i64::from(canvas_width)) as u32;
    let clamp_y = |value: i64| value.clamp(0, i64::from(canvas_height)) as u32;
    let mut painted = None;
    if !subset
        && device.e == device.e.trunc()
        && device.f == device.f.trunc()
        && device.a > 0.0
        && device.d > 0.0
    {
        // SkDraw::drawBitmap's SkTreatAsSprite: an integer translate whose
        // mapped bitmap bounds round to the bitmap's own size.
        let (ix, iy) = (round_half_up(device.e), round_half_up(device.f));
        let mapped_right = round_half_up(bitmap_width * device.a + device.e);
        let mapped_bottom = round_half_up(bitmap_height * device.d + device.f);
        if mapped_right == ix + i64::from(bitmap.width)
            && mapped_bottom == iy + i64::from(bitmap.height)
        {
            let rect = (
                clamp_x(ix),
                clamp_y(iy),
                clamp_x(mapped_right),
                clamp_y(mapped_bottom),
            );
            let mut output = allocate_rect(rect)?;
            if let Some(output) = output.as_mut() {
                let row_bytes = (rect.2 - rect.0) as usize * 4;
                for (row, line) in output.pixels.chunks_exact_mut(row_bytes).enumerate() {
                    let y = (i64::from(rect.1) + row as i64 - iy) as usize;
                    for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let x = (i64::from(rect.0) + column as i64 - ix) as usize;
                        pixel.copy_from_slice(bitmap.pixel(x, y));
                    }
                }
            }
            painted = Some(output);
        }
    }
    let mut output = match painted {
        Some(output) => output,
        None => {
            // The destination rectangle in device space, each edge rounded.
            let xs = (ctm.a * dst_left + ctm.e, ctm.a * dst_right + ctm.e);
            let ys = (ctm.d * dst_top + ctm.f, ctm.d * dst_bottom + ctm.f);
            let rect = (
                clamp_x(round_half_up(xs.0.min(xs.1))),
                clamp_y(round_half_up(ys.0.min(ys.1))),
                clamp_x(round_half_up(xs.0.max(xs.1))),
                clamp_y(round_half_up(ys.0.max(ys.1))),
            );
            let mut output = allocate_rect(rect)?;
            if let Some(output) = output.as_mut() {
                legacy_shade(bitmap, device, output)?;
            }
            output
        }
    };
    if let Some(output) = output.as_mut() {
        if draw.alpha8 < 255 {
            let scale = u32::from(draw.alpha8) + 1;
            for value in &mut output.pixels {
                *value = ((u32::from(*value) * scale) >> 8) as u8;
            }
        }
    }
    Ok(output)
}

fn allocate_rect(rect: (u32, u32, u32, u32)) -> Result<Option<PaintedRect>, FormatEncodeError> {
    let (left, top, right, bottom) = rect;
    if left >= right || top >= bottom {
        return Ok(None);
    }
    let image = PremulImage::zeroed(right - left, bottom - top)?;
    Ok(Some(PaintedRect {
        left,
        top,
        right,
        bottom,
        pixels: image.data,
    }))
}

/// `SkScalarToFractionalInt`: `(int64)(x * 2^32)`, truncating.
fn to_fractional(value: f32) -> i64 {
    (f64::from(value) * 4_294_967_296.0) as i64
}

/// Skia's legacy `SkBitmapProcState` for the forward device matrix `device`
/// (scale-translate), shading `output`'s rectangle: the binary32 inverse,
/// the translate-only and integer-translate downgrades, else the 4-bit
/// bilinear `S32_alpha_D32_filter_DX` whose 32.32 x coordinate restarts at
/// every 127th pixel of a row's span.
fn legacy_shade(
    bitmap: PremulView<'_>,
    device: AffineF,
    output: &mut PaintedRect,
) -> Result<(), FormatEncodeError> {
    let (mut isx, mut isy) = (1.0 / device.a, 1.0 / device.d);
    let (mut itx, mut ity) = (-device.e * isx, -device.f * isy);
    if !(isx == 1.0 && isy == 1.0) {
        // just_trans_general: the inverse of the inverse within 1/32768 of 1.
        let (gx, gy) = (1.0 / isx, 1.0 / isy);
        let (gtx, gty) = (-itx * gx, -ity * gy);
        let tolerance = 1.0 / 32768.0;
        if (gx - 1.0).abs() <= tolerance && (gy - 1.0).abs() <= tolerance {
            isx = 1.0;
            isy = 1.0;
            itx = -gtx;
            ity = -gty;
        }
    }
    if !(isx.is_finite() && isy.is_finite() && itx.is_finite() && ity.is_finite()) {
        return Err(FormatEncodeError::Encoding(
            "nonfinite image sampling transform".to_owned(),
        ));
    }
    let (width, height) = (bitmap.width, bitmap.height);
    let max_x = i64::from(width) - 1;
    let max_y = i64::from(height) - 1;
    let row_bytes = (output.right - output.left) as usize * 4;
    let (left, top) = (output.left, output.top);
    if isx == 1.0 && isy == 1.0 {
        // just_trans_integral: a translate within 1/256 of whole pixels
        // copies the nearest pixels.
        let (rx, ry) = (round_half_up(itx), round_half_up(ity));
        let tolerance = 1.0 / 256.0;
        if (f64::from(itx) - rx as f64).abs() <= tolerance
            && (f64::from(ity) - ry as f64).abs() <= tolerance
        {
            for (row, line) in output.pixels.chunks_exact_mut(row_bytes).enumerate() {
                let y = (i64::from(top) + row as i64 + ry).clamp(0, max_y) as usize;
                for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let x = (i64::from(left) + column as i64 + rx).clamp(0, max_x) as usize;
                    pixel.copy_from_slice(bitmap.pixel(x, y));
                }
            }
            return Ok(());
        }
    }
    let step = to_fractional(isx);
    let start = |x: u32, y: u32| -> (i64, i64) {
        let px = isx * (x as f32 + 0.5) + itx;
        let py = isy * (y as f32 + 0.5) + ity;
        (
            to_fractional(px).saturating_sub(1 << 31),
            to_fractional(py).saturating_sub(1 << 31) >> 16,
        )
    };
    let pack = |fixed: i64, max: i64| -> (usize, usize, u32) {
        (
            (fixed >> 16).clamp(0, max) as usize,
            (fixed.saturating_add(65536) >> 16).clamp(0, max) as usize,
            ((fixed >> 12) & 0xf) as u32,
        )
    };
    for (row, line) in output.pixels.chunks_exact_mut(row_bytes).enumerate() {
        let y = top + row as u32;
        let (mut fx, mut fy) = (0_i64, 0_i64);
        for (column, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if column % 127 == 0 {
                (fx, fy) = start(left + column as u32, y);
            }
            let (y0, y1, wy) = pack(fy, max_y);
            let (x0, x1, wx) = pack(fx >> 16, max_x);
            fx = fx.saturating_add(step);
            let taps = [
                (bitmap.pixel(x0, y0), (16 - wx) * (16 - wy)),
                (bitmap.pixel(x1, y0), wx * (16 - wy)),
                (bitmap.pixel(x0, y1), (16 - wx) * wy),
                (bitmap.pixel(x1, y1), wx * wy),
            ];
            for (channel, target) in pixel.iter_mut().enumerate() {
                let sum: u32 = taps
                    .iter()
                    .map(|(tap, weight)| u32::from(tap[channel]) * weight)
                    .sum();
                *target = (sum >> 8) as u8;
            }
        }
    }
    Ok(())
}

/// The key of one cc decode: the image and its rounded-out source rectangle.
pub(super) type DecodeKey = ([u8; 32], (u32, u32, u32, u32));

/// Where a cc decode level comes from (`SoftwareImageDecodeCache`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LevelSource {
    /// The same level was already decoded earlier in the render.
    Hit,
    /// Scaled from the smallest cached level of the same key at least as
    /// large in both axes, `(width, height)`.
    Cached(u32, u32),
    /// Scaled from the decoded image's source rectangle.
    Original,
}

/// The per-render cc decode cache, planned from sizes alone: every decision
/// depends only on the draws' keys and level sizes in paint order, so the
/// resource ledger can price the render before any pixel exists.
#[derive(Debug, Default)]
pub(super) struct DecodeCachePlanner {
    entries: Vec<(DecodeKey, Vec<(u32, u32)>)>,
}

impl DecodeCachePlanner {
    /// Records one level-above-0 draw and returns where its level comes from.
    pub(super) fn plan(&mut self, key: DecodeKey, width: u32, height: u32) -> LevelSource {
        let index = match self.entries.iter().position(|(entry, _)| *entry == key) {
            Some(index) => index,
            None => {
                self.entries.push((key, Vec::new()));
                self.entries.len() - 1
            }
        };
        let sizes = &mut self.entries[index].1;
        if sizes.contains(&(width, height)) {
            return LevelSource::Hit;
        }
        let source = sizes
            .iter()
            .copied()
            .filter(|&(w, h)| w >= width && h >= height)
            .min()
            .map_or(LevelSource::Original, |(w, h)| LevelSource::Cached(w, h));
        sizes.push((width, height));
        source
    }
}

/// The decoded levels one render keeps, filled in paint order.
#[derive(Debug, Default)]
pub(super) struct DecodeCache {
    entries: Vec<(DecodeKey, Vec<PremulImage>)>,
}

impl DecodeCache {
    /// The level `decode` describes for `image` (the whole decoded image),
    /// built or reused as `source` says.
    pub(super) fn level(
        &mut self,
        key: DecodeKey,
        image: PremulView<'_>,
        decode: &CcDecode,
        source: LevelSource,
    ) -> Result<&PremulImage, FormatEncodeError> {
        let index = match self.entries.iter().position(|(entry, _)| *entry == key) {
            Some(index) => index,
            None => {
                self.entries.push((key, Vec::new()));
                self.entries.len() - 1
            }
        };
        let size = (decode.level_width, decode.level_height);
        let missing =
            || FormatEncodeError::Encoding("planned image level is not cached".to_owned());
        let levels = &mut self.entries[index].1;
        let built = match source {
            LevelSource::Hit => None,
            LevelSource::Cached(width, height) => {
                let cached = levels
                    .iter()
                    .find(|level| (level.width, level.height) == (width, height))
                    .ok_or_else(missing)?;
                Some(scale_pixels(cached.view(), size.0, size.1)?)
            }
            LevelSource::Original => {
                let (x, y, width, height) = decode.rect;
                Some(scale_pixels(
                    image.sub(x, y, width, height)?,
                    size.0,
                    size.1,
                )?)
            }
        };
        if let Some(built) = built {
            levels
                .try_reserve(1)
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "raster_surface",
                    subject: "png_image_level",
                })?;
            levels.push(built);
        }
        levels
            .iter()
            .find(|level| (level.width, level.height) == size)
            .ok_or_else(missing)
    }
}

#[cfg(test)]
#[path = "canvas_image_tests.rs"]
mod tests;
