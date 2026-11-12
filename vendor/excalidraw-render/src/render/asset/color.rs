//! Normalization of decoded pixels: the color management that brings a
//! raster's declared ICC profile, cICP tags, gAMA/cHRM chunks or linear
//! encoding to sRGB with `moxcms`, and the final pass that zeroes invisible
//! pixels' color and derives the premultiplied copy. The PNG, JPEG, WebP, BMP
//! and ICO decoders finish their RGBA here; GIF composites its indexed frame
//! straight into both buffers itself.

use std::sync::Arc;

use moxcms::{
    Chromaticity, CicpColorPrimaries, CicpProfile, ColorPrimaries, ColorProfile, DataColorSpace,
    Layout, MatrixCoefficients, ToneReprCurve, TransferCharacteristics, TransformOptions, XyY,
    Xyzd,
};

use super::png::{PngFrameReader, PngSemantics};
use super::{try_copy, DecodedRaster, ResolveError};

pub(super) enum ColorNormalization {
    None,
    LinearSrgb,
    Profile(Box<ColorProfile>),
    Lut(Vec<super::icc_lut::Lut>),
    Hdr(Box<super::hdr::Hdr>),
}

pub(super) const LINEAR_TO_SRGB: [u8; 256] = [
    0, 13, 22, 28, 34, 38, 42, 46, 50, 53, 56, 59, 61, 64, 66, 69, 71, 73, 75, 77, 79, 81, 83, 85,
    86, 88, 90, 92, 93, 95, 96, 98, 99, 101, 102, 104, 105, 106, 108, 109, 110, 112, 113, 114, 115,
    117, 118, 119, 120, 121, 122, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136,
    137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 148, 149, 150, 151, 152, 153, 154,
    155, 155, 156, 157, 158, 159, 159, 160, 161, 162, 163, 163, 164, 165, 166, 167, 167, 168, 169,
    170, 170, 171, 172, 173, 173, 174, 175, 175, 176, 177, 178, 178, 179, 180, 180, 181, 182, 182,
    183, 184, 185, 185, 186, 187, 187, 188, 189, 189, 190, 190, 191, 192, 192, 193, 194, 194, 195,
    196, 196, 197, 197, 198, 199, 199, 200, 200, 201, 202, 202, 203, 203, 204, 205, 205, 206, 206,
    207, 208, 208, 209, 209, 210, 210, 211, 212, 212, 213, 213, 214, 214, 215, 215, 216, 216, 217,
    218, 218, 219, 219, 220, 220, 221, 221, 222, 222, 223, 223, 224, 224, 225, 226, 226, 227, 227,
    228, 228, 229, 229, 230, 230, 231, 231, 232, 232, 233, 233, 234, 234, 235, 235, 236, 236, 237,
    237, 238, 238, 238, 239, 239, 240, 240, 241, 241, 242, 242, 243, 243, 244, 244, 245, 245, 246,
    246, 246, 247, 247, 248, 248, 249, 249, 250, 250, 251, 251, 251, 252, 252, 253, 253, 254, 254,
    255, 255,
];

pub(super) fn normalize_rgba(
    rgba: &mut [u8],
    color_normalization: ColorNormalization,
) -> Result<(), ResolveError> {
    match color_normalization {
        // PNG's HDR decoder must retain source precision and derive the
        // straight and draw-time premultiplied projections together.
        ColorNormalization::Hdr(_) => Err(ResolveError::Unsupported),
        ColorNormalization::None => Ok(()),
        ColorNormalization::LinearSrgb => {
            for pixel in rgba.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel = LINEAR_TO_SRGB[usize::from(*channel)];
                }
            }
            Ok(())
        }
        ColorNormalization::Lut(lut) => {
            let lut = lut.first().ok_or(ResolveError::DecodeFailed)?;
            for p in rgba.as_chunks_mut::<4>().0.iter_mut() {
                if lut.gray() && (p[0] != p[1] || p[1] != p[2]) {
                    return Err(ResolveError::Unsupported);
                }
                let rgb = lut.pixel([p[0], p[1], p[2]])?;
                p[..3].copy_from_slice(&rgb);
            }
            Ok(())
        }
        ColorNormalization::Profile(profile) => {
            let srgb = ColorProfile::new_srgb();
            let gray = profile.color_space == DataColorSpace::Gray;
            let transform = cms_operation(CmsSite::Construct, || {
                profile.create_transform_16bit(
                    if gray {
                        Layout::GrayAlpha
                    } else {
                        Layout::Rgba
                    },
                    &srgb,
                    Layout::Rgba,
                    TransformOptions::default(),
                )
            })?;
            // The 8-bit executor's 4096-entry output curve loses near-black
            // samples even in identity transforms. Use its higher-resolution
            // path and quantize once, with bounded scratch independent of size.
            let mut source = [0_u16; 1024];
            let mut output = [0_u16; 1024];
            for block in rgba.chunks_mut(source.len()) {
                let source_len = if gray {
                    for (pixel, target) in block
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(source.as_chunks_mut::<2>().0.iter_mut())
                    {
                        if pixel[0] != pixel[1] || pixel[1] != pixel[2] {
                            return Err(ResolveError::Unsupported);
                        }
                        target[0] = u16::from(pixel[0]) * 257;
                        target[1] = u16::from(pixel[3]) * 257;
                    }
                    block.len() / 2
                } else {
                    for (target, value) in source.iter_mut().zip(block.iter()) {
                        *target = u16::from(*value) * 257;
                    }
                    block.len()
                };
                cms_operation(CmsSite::Execute, || {
                    transform.transform(&source[..source_len], &mut output[..block.len()])
                })?;
                for (target, value) in block.iter_mut().zip(output.iter()) {
                    *target = ((u32::from(*value) * 255 + 32767) / 65535) as u8;
                }
            }
            Ok(())
        }
    }
}

pub(super) fn finish_rgba(mut rgba: Vec<u8>) -> Result<DecodedRaster, ResolveError> {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] == 0 {
            pixel[..3].fill(0);
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

pub(super) fn prepare_png_color_normalization(
    bytes: &[u8],
    semantics: &PngSemantics,
    workspace_bytes: u64,
) -> Result<ColorNormalization, ResolveError> {
    // The pinned SkPngRustCodec cannot construct a narrow-range RGB space or
    // interpret transfer codes 9, 10, 12 and continues with lower metadata.
    // Limit the latter fallback to valid RGB signaling; malformed metadata
    // retains the project's declared strict unsupported-color recovery.
    if let Some(cicp) = semantics.cicp.filter(|cicp| {
        let ignored_transfer = matches!(cicp[0], 1 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 22)
            && matches!(cicp[1], 9 | 10 | 12)
            && cicp[2] == 0
            && cicp[3] == 1;
        cicp[3] != 0 && !ignored_transfer
    }) {
        if cicp == [1, 13, 0, 1] {
            return Ok(ColorNormalization::None);
        }
        if cicp == [1, 8, 0, 1] {
            return Ok(ColorNormalization::LinearSrgb);
        }
        let profile = cicp_color_profile(cicp)?;
        return Ok(if matches!(cicp[1], 16 | 18) {
            let peak = png_peak_luminance(bytes, workspace_bytes)?;
            ColorNormalization::Hdr(Box::new(super::hdr::Hdr::new(
                &profile,
                cicp[1] == 18,
                peak,
            )))
        } else {
            ColorNormalization::Profile(Box::new(profile))
        });
    }
    let mut decoder = png::Decoder::new(PngFrameReader::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: usize::try_from(workspace_bytes).map_err(|_| ResolveError::Memory)?,
    });
    let reader = decoder
        .read_info()
        .map_err(|_| ResolveError::DecodeFailed)?;
    png_color_profile(reader.info(), semantics.zero_gamma)
}

/// The content peak luminance SkHdrAgtm tone maps PQ/HLG from, in nits: a
/// positive cLLI MaxCLL, else a positive mDCV maximum luminance, else the
/// default. The pinned codec reads both through the png crate before the first
/// IDAT: a malformed chunk counts as absent, the first valid one wins, and mDCV
/// must precede PLTE and IDAT. Values are 0.0001 cd/m2 units converted in f32.
fn png_peak_luminance(bytes: &[u8], workspace_bytes: u64) -> Result<f64, ResolveError> {
    let mut decoder = png::Decoder::new(PngFrameReader::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: usize::try_from(workspace_bytes).map_err(|_| ResolveError::Memory)?,
    });
    let reader = decoder
        .read_info()
        .map_err(|_| ResolveError::DecodeFailed)?;
    let info = reader.info();
    let nits = |value: u32| f64::from(value as f32 / 10_000.0);
    Ok(info
        .content_light_level
        .map(|clli| nits(clli.max_content_light_level))
        .filter(|&peak| peak > 0.0)
        .or_else(|| {
            info.mastering_display_color_volume
                .map(|mdcv| nits(mdcv.max_luminance))
                .filter(|&peak| peak > 0.0)
        })
        .unwrap_or(super::hdr::DEFAULT_PEAK_LUMINANCE))
}

fn cicp_color_profile(cicp: [u8; 4]) -> Result<ColorProfile, ResolveError> {
    let color_primaries =
        CicpColorPrimaries::try_from(cicp[0]).map_err(|_| ResolveError::Unsupported)?;
    let transfer_characteristics =
        TransferCharacteristics::try_from(cicp[1]).map_err(|_| ResolveError::Unsupported)?;
    let matrix_coefficients =
        MatrixCoefficients::try_from(cicp[2]).map_err(|_| ResolveError::Unsupported)?;
    if !matches!(matrix_coefficients, MatrixCoefficients::Identity)
        || cicp[3] > 1
        || matches!(
            color_primaries,
            CicpColorPrimaries::Reserved | CicpColorPrimaries::Unspecified
        )
        || matches!(
            transfer_characteristics,
            TransferCharacteristics::Reserved | TransferCharacteristics::Unspecified
        )
    {
        return Err(ResolveError::Unsupported);
    }
    let mut profile = ColorProfile::new_from_cicp(CicpProfile {
        color_primaries,
        transfer_characteristics,
        matrix_coefficients,
        full_range: cicp[3] == 1,
    });
    if matches!(color_primaries, CicpColorPrimaries::Xyz) {
        // The generic xy -> XYZ constructor normalizes each primary to Y=1,
        // collapsing the zero-luminance X/Z axes. Preserve the XYZ basis and
        // its equal-energy white, then let the CMS adapt it to the D50 PCS.
        let signaling = profile.cicp;
        profile.update_rgb_colorimetry_triplet(
            XyY::new(1.0 / 3.0, 1.0 / 3.0, 1.0),
            Xyzd::new(1.0, 0.0, 0.0),
            Xyzd::new(0.0, 1.0, 0.0),
            Xyzd::new(0.0, 0.0, 1.0),
        );
        profile.cicp = signaling;
    }
    // SkNamedTransferFn follows H.273's reference EOTF (BT.1886 gamma 2.4),
    // rather than inverting the camera OETF for these display-referred codes.
    if matches!(cicp[1], 1 | 6 | 11 | 14 | 15) {
        profile.cicp = None;
        let curve = ToneReprCurve::Parametric(vec![2.4]);
        profile.red_trc = Some(curve.clone());
        profile.green_trc = Some(curve.clone());
        profile.blue_trc = Some(curve);
    }
    Ok(profile)
}

fn png_color_profile(
    info: &png::Info<'_>,
    zero_gamma: bool,
) -> Result<ColorNormalization, ResolveError> {
    if let Some(profile) = info.icc_profile.as_deref() {
        // Device-link, abstract and named-color profiles are not image source
        // spaces. Parsing their tag tables alone does not establish a usable
        // PNG interpretation. Direct SVG never invokes this projection step.
        if !matches!(
            profile.get(12..16),
            Some(b"mntr" | b"scnr" | b"prtr" | b"spac")
        ) {
            return Err(ResolveError::Unsupported);
        }
        let bytes = validate_png_icc_direction(profile)?;
        if bytes.get(16..20) == Some(b"GRAY")
            && !matches!(
                info.color_type,
                png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha
            )
        {
            return Err(ResolveError::Unsupported);
        }
        if let Some(lut) = super::icc_lut::Lut::parse(bytes)? {
            return Ok(ColorNormalization::Lut(lut.stored()?));
        }
        let had_saturation = super::icc_lut::has_tag(bytes, b"A2B2")?;
        let profile = cms_parse(|| super::icc_matrix::parse(bytes))?;
        if !matches!(
            profile.color_space,
            DataColorSpace::Rgb | DataColorSpace::Gray
        ) || (profile.color_space == DataColorSpace::Gray
            && !matches!(
                info.color_type,
                png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha
            ))
        {
            return Err(ResolveError::Unsupported);
        }
        if had_saturation
            && profile.red_trc.is_none()
            && profile.green_trc.is_none()
            && profile.blue_trc.is_none()
            && profile.gray_trc.is_none()
        {
            // skcms's default source priority is A2B0, then A2B1. An
            // otherwise valid saturation-only space is not selected.
            return png_legacy_color_profile(info, zero_gamma);
        }
        // Skia tags a profile it snaps to sRGB as sRGB, and Blink then needs
        // no transform: the pixels draw untransformed.
        if profile.color_space == DataColorSpace::Rgb && super::skia_srgb::snaps_to_srgb(&profile) {
            return Ok(ColorNormalization::None);
        }
        return Ok(ColorNormalization::Profile(Box::new(profile)));
    }
    png_legacy_color_profile(info, zero_gamma)
}

/// A BMP V4/V5 calibrated-RGB colour space: `to_xyz_d50` holds the red, green
/// and blue colorants as columns, and each channel decodes with the 16.16
/// fixed-point exponent Blink reads (`SkFixedToFloat`). Skia draws the tagged
/// image with its gamma stage, which maps 0 and 1 to themselves and a zero
/// exponent to 1 elsewhere; a negative exponent names no usable curve.
pub(super) fn calibrated_rgb_normalization(
    to_xyz_d50: &[[f32; 3]; 3],
    gammas: [u32; 3],
) -> Result<ColorNormalization, ResolveError> {
    let mut profile = ColorProfile::new_srgb();
    profile.cicp = None;
    let colorant = |column: usize| {
        Xyzd::new(
            f64::from(to_xyz_d50[0][column]),
            f64::from(to_xyz_d50[1][column]),
            f64::from(to_xyz_d50[2][column]),
        )
    };
    profile.red_colorant = colorant(0);
    profile.green_colorant = colorant(1);
    profile.blue_colorant = colorant(2);
    let mut curves = Vec::with_capacity(3);
    for gamma in gammas {
        let gamma = gamma as i32 as f32 * (1.0 / 65536.0);
        if gamma < 0.0 {
            return Err(ResolveError::Unsupported);
        }
        curves.push(if gamma == 0.0 {
            let mut table = vec![u16::MAX; 256];
            table[0] = 0;
            ToneReprCurve::Lut(table)
        } else {
            ToneReprCurve::Parametric(vec![gamma])
        });
    }
    let mut curves = curves.into_iter();
    profile.red_trc = curves.next();
    profile.green_trc = curves.next();
    profile.blue_trc = curves.next();
    if super::skia_srgb::snaps_to_srgb(&profile) {
        return Ok(ColorNormalization::None);
    }
    Ok(ColorNormalization::Profile(Box::new(profile)))
}

pub(super) fn parse_icc_profile(bytes: &[u8]) -> Result<ColorNormalization, ResolveError> {
    let bytes = validate_png_icc_direction(bytes)?;
    if let Some(lut) = super::icc_lut::Lut::parse(bytes)? {
        return Ok(ColorNormalization::Lut(lut.stored()?));
    }
    cms_parse(|| super::icc_matrix::parse(bytes))
        .map(Box::new)
        .map(ColorNormalization::Profile)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CmsSite {
    Parse,
    Construct,
    Execute,
    Allocation,
    ProgramAllocation,
}
pub(super) fn cms_error(error: moxcms::CmsError) -> ResolveError {
    if matches!(error, moxcms::CmsError::OutOfMemory(_)) {
        ResolveError::Memory
    } else {
        ResolveError::Unsupported
    }
}
pub(super) fn cms_operation<T>(
    site: CmsSite,
    operation: impl FnOnce() -> Result<T, moxcms::CmsError>,
) -> Result<T, ResolveError> {
    if cms_refused(site) {
        return Err(cms_error(moxcms::CmsError::OutOfMemory(1)));
    }
    operation().map_err(cms_error)
}
pub(super) fn cms_parse<T>(
    operation: impl FnOnce() -> Result<T, ResolveError>,
) -> Result<T, ResolveError> {
    if cms_refused(CmsSite::Parse) {
        return Err(cms_error(moxcms::CmsError::OutOfMemory(1)));
    }
    operation()
}
pub(super) fn cms_allocation<T>(
    operation: impl FnOnce() -> Result<T, ResolveError>,
) -> Result<T, ResolveError> {
    if cms_refused(CmsSite::Allocation) {
        return Err(ResolveError::Memory);
    }
    operation()
}
pub(super) fn cms_program_allocation<T>(
    operation: impl FnOnce() -> Result<T, ResolveError>,
) -> Result<T, ResolveError> {
    if cms_refused(CmsSite::ProgramAllocation) {
        return Err(ResolveError::Memory);
    }
    operation()
}
fn cms_refused(_site: CmsSite) -> bool {
    #[cfg(test)]
    {
        CMS_FAILURE.with(|s| match s.get() {
            Some((site, 0)) if site == _site => true,
            Some((site, n)) if site == _site => {
                s.set(Some((site, n - 1)));
                false
            }
            _ => false,
        })
    }
    #[cfg(not(test))]
    {
        false
    }
}
#[cfg(test)]
thread_local! {static CMS_FAILURE:std::cell::Cell<Option<(CmsSite,usize)>>=const {std::cell::Cell::new(None)};}
#[cfg(test)]
pub(crate) fn with_cms_failure<T>(site: CmsSite, body: impl FnOnce() -> T) -> T {
    with_cms_failure_after(site, 0, body)
}
#[cfg(test)]
pub(crate) fn with_cms_failure_after<T>(site: CmsSite, skip: usize, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<(CmsSite, usize)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CMS_FAILURE.with(|s| s.set(self.0));
        }
    }
    let _restore = Restore(CMS_FAILURE.with(|s| s.replace(Some((site, skip)))));
    body()
}

fn png_legacy_color_profile(
    info: &png::Info<'_>,
    zero_gamma: bool,
) -> Result<ColorNormalization, ResolveError> {
    if info.srgb.is_some() {
        return Ok(ColorNormalization::None);
    }
    // A zero gAMA is no usable transfer: unsupported whatever the
    // chromaticities, as the strict policy declares.
    if zero_gamma {
        return Err(ResolveError::Unsupported);
    }
    // The reference intentionally does not guess a transfer function from
    // chromaticities alone (SkPngRustCodec::CreateColorProfile).
    if info.gamma().is_none() {
        return Ok(ColorNormalization::None);
    }
    if info.chromaticities().is_none()
        && info.gamma().is_some_and(|gamma| {
            let neutral = gamma.into_value() * 2.2;
            neutral > 0.95 && neutral < 1.05
        })
    {
        return Ok(ColorNormalization::None);
    }
    if info.chromaticities().is_none()
        && info
            .gamma()
            .is_some_and(|gamma| gamma.into_scaled() == 100_000)
    {
        return Ok(ColorNormalization::LinearSrgb);
    }

    let mut profile = ColorProfile::new_srgb();
    profile.cicp = None;
    if let Some(chromaticities) = info.chromaticities() {
        let valid = [
            chromaticities.white,
            chromaticities.red,
            chromaticities.green,
            chromaticities.blue,
        ]
        .into_iter()
        .all(|(x, y)| u64::from(x.into_scaled()) + u64::from(y.into_scaled()) <= 100_000);
        if !valid {
            return Err(ResolveError::Unsupported);
        }
        let value = |scaled: png::ScaledFloat| f64::from(scaled.into_scaled()) / 100_000.0;
        profile.update_rgb_colorimetry(
            XyY::new(
                value(chromaticities.white.0),
                value(chromaticities.white.1),
                1.0,
            ),
            ColorPrimaries {
                red: Chromaticity {
                    x: value(chromaticities.red.0) as f32,
                    y: value(chromaticities.red.1) as f32,
                },
                green: Chromaticity {
                    x: value(chromaticities.green.0) as f32,
                    y: value(chromaticities.green.1) as f32,
                },
                blue: Chromaticity {
                    x: value(chromaticities.blue.0) as f32,
                    y: value(chromaticities.blue.1) as f32,
                },
            },
        );
    }
    if let Some(gamma) = info.gamma() {
        let gamma = gamma.into_value();
        if gamma <= 0.0 {
            return Err(ResolveError::Unsupported);
        }
        // curve_from_gamma stores an ICC u8Fixed8 exponent, which saturates
        // above 255.996 and rounds tiny exponents to zero. gAMA has a much
        // wider domain; retain the actual power in a parametric curve.
        let curve = ToneReprCurve::Parametric(vec![1.0 / gamma]);
        profile.red_trc = Some(curve.clone());
        profile.green_trc = Some(curve.clone());
        profile.blue_trc = Some(curve);
    }
    Ok(ColorNormalization::Profile(Box::new(profile)))
}

fn validate_png_icc_direction(bytes: &[u8]) -> Result<&[u8], ResolveError> {
    let word = |at| -> Result<usize, ResolveError> {
        let data = bytes.get(at..at + 4).ok_or(ResolveError::Unsupported)?;
        usize::try_from(u32::from_be_bytes(
            data.try_into().map_err(|_| ResolveError::Unsupported)?,
        ))
        .map_err(|_| ResolveError::Unsupported)
    };
    let count = word(128)?;
    // These invariants apply to every ICC execution path, including the owned
    // LUT path which deliberately never invokes the dependency's parser.
    if bytes.get(36..40) != Some(b"acsp") || !matches!(bytes.get(8), Some(2..=4)) || word(64)? > 3 {
        return Err(ResolveError::Unsupported);
    }
    moxcms::ProfileClass::try_from(word(12)? as u32).map_err(cms_error)?;
    DataColorSpace::try_from(word(16)? as u32).map_err(cms_error)?;
    DataColorSpace::try_from(word(20)? as u32).map_err(cms_error)?;
    if word(0)? > bytes.len() || word(0)? < 132 {
        return Err(ResolveError::Unsupported);
    }
    let bytes = bytes.get(..word(0)?).ok_or(ResolveError::Unsupported)?;
    let size = count
        .checked_mul(12)
        .and_then(|v| v.checked_add(132))
        .ok_or(ResolveError::Unsupported)?;
    let table = bytes.get(132..size).ok_or(ResolveError::Unsupported)?;
    for tag in table.as_chunks::<12>().0.iter() {
        let offset = usize::try_from(u32::from_be_bytes(
            tag[4..8]
                .try_into()
                .map_err(|_| ResolveError::Unsupported)?,
        ))
        .map_err(|_| ResolveError::Unsupported)?;
        let length = usize::try_from(u32::from_be_bytes(
            tag[8..12]
                .try_into()
                .map_err(|_| ResolveError::Unsupported)?,
        ))
        .map_err(|_| ResolveError::Unsupported)?;
        let end = offset
            .checked_add(length)
            .ok_or(ResolveError::Unsupported)?;
        if offset < size || bytes.get(offset..end).is_none() {
            return Err(ResolveError::Unsupported);
        }
        if matches!(&tag[..4], b"A2B0" | b"A2B1" | b"A2B2") {
            let offset = usize::try_from(u32::from_be_bytes(
                tag[4..8]
                    .try_into()
                    .map_err(|_| ResolveError::Unsupported)?,
            ))
            .map_err(|_| ResolveError::Unsupported)?;
            let end = offset.checked_add(4).ok_or(ResolveError::Unsupported)?;
            if !matches!(bytes.get(offset..end), Some(b"mft1" | b"mft2" | b"mAB ")) {
                return Err(ResolveError::Unsupported);
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{parse_icc_profile, validate_png_icc_direction, ResolveError};

    /// moxcms 0.9.1 reads an all-zero ICC version field as v2.0; the renderer
    /// never hands untrusted bytes to that parser, and its own header gate
    /// keeps rejecting a zero major version on every ICC path.
    #[test]
    fn zero_icc_version_is_unsupported() {
        let mut header = vec![0; 132];
        header[..4].copy_from_slice(&132u32.to_be_bytes());
        header[12..16].copy_from_slice(b"mntr");
        header[16..20].copy_from_slice(b"RGB ");
        header[20..24].copy_from_slice(b"XYZ ");
        header[36..40].copy_from_slice(b"acsp");
        header[8] = 2;
        assert!(validate_png_icc_direction(&header).is_ok());
        header[8] = 0;
        assert_eq!(
            validate_png_icc_direction(&header),
            Err(ResolveError::Unsupported)
        );
        assert!(parse_icc_profile(&header).is_err());
    }
}
