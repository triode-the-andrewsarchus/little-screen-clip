//! Relevant-field ICC matrix/TRC parsing. Never pass untrusted ICC bytes to
//! moxcms's allocating metadata/TRC reader. Select each last relevant tag once,
//! validate its bounded payload, and reserve curve storage fallibly.
use super::{
    color::{cms_allocation, cms_error},
    ResolveError,
};
use moxcms::{
    CicpColorPrimaries, CicpProfile, ColorProfile, DataColorSpace, Matrix3d, MatrixCoefficients,
    ProfileClass, RenderingIntent, ToneReprCurve, TransferCharacteristics, Xyzd,
};
type Result<T> = std::result::Result<T, ResolveError>;
fn span(data: &[u8], at: usize, length: usize) -> Result<&[u8]> {
    data.get(at..at.checked_add(length).ok_or(ResolveError::Unsupported)?)
        .ok_or(ResolveError::Unsupported)
}
fn word(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        span(data, at, 4)?
            .try_into()
            .map_err(|_| ResolveError::Unsupported)?,
    ))
}
fn fixed(data: &[u8], at: usize) -> Result<f64> {
    Ok(i32::from_be_bytes(
        span(data, at, 4)?
            .try_into()
            .map_err(|_| ResolveError::Unsupported)?,
    ) as f64
        / 65536.)
}
fn xyz(data: &[u8]) -> Result<Xyzd> {
    if span(data, 0, 4)? != b"XYZ " {
        return Err(ResolveError::Unsupported);
    }
    Ok(Xyzd::new(
        fixed(data, 8)?,
        fixed(data, 12)?,
        fixed(data, 16)?,
    ))
}
fn trc(data: &[u8]) -> Result<ToneReprCurve> {
    match span(data, 0, 4)? {
        b"curv" => {
            let count = word(data, 8)? as usize;
            // Preserve the qualified dependency's existing admission bound.
            if count > 40_000 {
                return Err(ResolveError::Unsupported);
            }
            let bytes = span(
                data,
                12,
                count.checked_mul(2).ok_or(ResolveError::Unsupported)?,
            )?;
            let values = cms_allocation(|| {
                let mut values = Vec::new();
                values
                    .try_reserve_exact(count)
                    .map_err(|_| ResolveError::Memory)?;
                for pair in bytes.as_chunks::<2>().0.iter() {
                    values.push(u16::from_be_bytes([pair[0], pair[1]]));
                }
                Ok(values)
            })?;
            Ok(ToneReprCurve::Lut(values))
        }
        b"para" => {
            let kind = u16::from_be_bytes(
                span(data, 8, 2)?
                    .try_into()
                    .map_err(|_| ResolveError::Unsupported)?,
            ) as usize;
            let count = *([1, 3, 4, 5, 7].get(kind).ok_or(ResolveError::Unsupported)?);
            span(data, 12, count * 4)?;
            let values = cms_allocation(|| {
                let mut values = Vec::new();
                values
                    .try_reserve_exact(count)
                    .map_err(|_| ResolveError::Memory)?;
                for i in 0..count {
                    values.push(fixed(data, 12 + i * 4)? as f32);
                }
                Ok(values)
            })?;
            if matches!(kind, 1 | 2) && values.get(1) == Some(&0.) {
                return Err(ResolveError::Unsupported);
            }
            Ok(ToneReprCurve::Parametric(values))
        }
        _ => Err(ResolveError::Unsupported),
    }
}
pub(super) fn parse(bytes: &[u8]) -> Result<ColorProfile> {
    const NAMES: [[u8; 4]; 12] = [
        *b"rXYZ", *b"gXYZ", *b"bXYZ", *b"rTRC", *b"gTRC", *b"bTRC", *b"kTRC", *b"wtpt", *b"bkpt",
        *b"lumi", *b"chad", *b"cicp",
    ];
    let count = word(bytes, 128)? as usize;
    let table = span(
        bytes,
        132,
        count.checked_mul(12).ok_or(ResolveError::Unsupported)?,
    )?;
    let mut selected = [None; 12];
    for tag in table.as_chunks::<12>().0.iter() {
        if let Some(i) = NAMES.iter().position(|n| n == &tag[..4]) {
            selected[i] = Some(span(bytes, word(tag, 4)? as usize, word(tag, 8)? as usize)?);
        }
    }
    let mut profile = ColorProfile::default();
    profile.color_space = DataColorSpace::try_from(word(bytes, 16)?).map_err(cms_error)?;
    profile.pcs = DataColorSpace::try_from(word(bytes, 20)?).map_err(cms_error)?;
    profile.profile_class = ProfileClass::try_from(word(bytes, 12)?).map_err(cms_error)?;
    profile.rendering_intent = RenderingIntent::try_from(word(bytes, 64)?).map_err(cms_error)?;
    profile.white_point = Xyzd::new(fixed(bytes, 68)?, fixed(bytes, 72)?, fixed(bytes, 76)?);
    // Version affects LUT PCS encoding/BPC, handled by the owned LUT path.
    // The matrix/shaper executor never consults version or descriptive tags.
    if profile.color_space == DataColorSpace::Rgb {
        if let Some(v) = selected[0] {
            profile.red_colorant = xyz(v)?;
        }
        if let Some(v) = selected[1] {
            profile.green_colorant = xyz(v)?;
        }
        if let Some(v) = selected[2] {
            profile.blue_colorant = xyz(v)?;
        }
        if let Some(v) = selected[3] {
            profile.red_trc = Some(trc(v)?);
        }
        if let Some(v) = selected[4] {
            profile.green_trc = Some(trc(v)?);
        }
        if let Some(v) = selected[5] {
            profile.blue_trc = Some(trc(v)?);
        }
    } else if profile.color_space == DataColorSpace::Gray {
        if let Some(v) = selected[6] {
            profile.gray_trc = Some(trc(v)?);
        }
    }
    if let Some(v) = selected[7] {
        profile.media_white_point = Some(xyz(v)?);
    }
    if let Some(v) = selected[8] {
        profile.black_point = Some(xyz(v)?);
    }
    if let Some(v) = selected[9] {
        profile.luminance = Some(xyz(v)?);
    }
    if let Some(v) = selected[10] {
        if span(v, 0, 4)? != b"sf32" || v.len() != 44 {
            return Err(ResolveError::Unsupported);
        }
        let mut m = [[0.; 3]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, value) in row.iter_mut().enumerate() {
                *value = fixed(v, 8 + (r * 3 + c) * 4)?;
            }
        }
        profile.chromatic_adaptation = Some(Matrix3d { v: m });
    }
    if matches!(
        profile.profile_class,
        ProfileClass::InputDevice | ProfileClass::DisplayDevice
    ) && matches!(
        profile.color_space,
        DataColorSpace::Rgb | DataColorSpace::YCbr | DataColorSpace::Xyz
    ) {
        if let Some(v) = selected[11] {
            if span(v, 0, 4)? != b"cicp" {
                return Err(ResolveError::Unsupported);
            }
            let values = span(v, 8, 4)?;
            profile.cicp = Some(CicpProfile {
                color_primaries: CicpColorPrimaries::try_from(values[0]).map_err(cms_error)?,
                transfer_characteristics: TransferCharacteristics::try_from(values[1])
                    .map_err(cms_error)?,
                matrix_coefficients: MatrixCoefficients::try_from(values[2]).map_err(cms_error)?,
                full_range: values[3] != 0,
            });
        }
    }
    Ok(profile)
}
